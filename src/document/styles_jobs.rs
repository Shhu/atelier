//! Layer styles drawn on worker threads for the display: on a large layer
//! they take up to a second (distance fields, blurs), so the view keeps
//! showing the former ones until the new ones are ready.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::RgbaImage;

use super::{Fill, Layer, LayerKind, PixelRect, StylesCache, intersect, next_rev};
use crate::compose;
use crate::compose::styles::{Effects, Styles, render_styles};

/// Below this many pixels, styles are drawn at once (quick enough).
const AT_ONCE: u64 = 1 << 20;

/// The same for a shape to composite first, which a drag can change on
/// every frame.
const SHAPE_AT_ONCE: u64 = 1 << 18;

/// What styles are drawn from: pixel revision, mask fingerprint, settings.
#[derive(Clone, PartialEq)]
struct Key {
    rev: u64,
    mask: u64,
    effects: Effects,
}

/// Styles being drawn for a layer (`None` until done), and the layer's
/// place then.
pub(super) struct Job {
    key: Key,
    offset: (i64, i64),
    done: Mutex<Option<Arc<Styles>>>,
}

/// The styles being drawn for a layer, shared by its copies (the display
/// composites copies of the layers while a mask is painted): one job at a
/// time, a newer request waits for it to end.
pub(super) type JobSlot = Arc<Mutex<Option<Arc<Job>>>>;

/// Jobs still running.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Whether styles (or feathered masks) are being drawn (the display then
/// looks again soon).
pub fn styles_pending() -> bool {
    RUNNING.load(Ordering::Relaxed) > 0 || super::masks::feathers_pending()
}

impl Layer {
    /// The styles as the display shows them (see [`Self::styles`]): those
    /// of the current pixels, mask and settings when drawn; otherwise they
    /// are drawn on a worker thread, and meanwhile the former ones show
    /// (none for a layer that never had any).
    pub fn styles_later(&self, held: bool) -> Option<Arc<Styles>> {
        if !self.has_pixels() || !self.effects.rendered() {
            return None;
        }
        let (w, h) = self.image.dimensions();
        if u64::from(w) * u64::from(h) <= AT_ONCE {
            return self.styles(held);
        }
        let key = Key {
            rev: self.rev,
            mask: self.mask_key(),
            effects: self.effects.clone(),
        };
        if let Some(c) = self.styles_cache.borrow().as_ref()
            && c.effects == key.effects
            && (held || (c.rev, c.mask) == (key.rev, key.mask))
        {
            return Some(self.placed_styles(c));
        }
        let prepare = || {
            let (image, mask, offset) = (
                self.shared_image(),
                self.active_mask().cloned(),
                self.offset,
            );
            move |fx: &Effects| {
                let shape = match mask {
                    Some(m) => Arc::new(m.apply_to(&image, offset)),
                    None => image,
                };
                render_styles(&shape, fx)
            }
        };
        match self.draw_aside(key, prepare) {
            Ok(styles) => styles,
            Err(()) => self.styles(held),
        }
    }

    /// The styles for `key` once what `prepare` returns drew them on a
    /// worker thread (prepared only when a job starts), the former ones
    /// meanwhile. `Err`: no thread to draw them.
    fn draw_aside<M>(
        &self,
        key: Key,
        prepare: impl FnOnce() -> M,
    ) -> Result<Option<Arc<Styles>>, ()>
    where
        M: FnOnce(&Effects) -> Styles + Send + 'static,
    {
        let mut slot = self.styles_job.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = slot.as_ref() {
            let done = job.done.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let Some(styles) = done else {
                // Still drawing (for these settings or older ones): the
                // former styles meanwhile.
                return Ok(self.styles_cached());
            };
            *self.styles_cache.borrow_mut() = Some(StylesCache {
                rev: job.key.rev,
                mask: job.key.mask,
                effects: job.key.effects.clone(),
                styles: Arc::clone(&styles),
                // Drawn for the same shape, they follow the layer.
                offset: if job.key == key {
                    self.offset
                } else {
                    job.offset
                }
                .into(),
                moved: Default::default(),
            });
            if job.key == key {
                return Ok(Some(styles));
            }
        }
        let make = prepare();
        let job = Arc::new(Job {
            key,
            offset: self.offset,
            done: Mutex::new(None),
        });
        let work = Arc::clone(&job);
        RUNNING.fetch_add(1, Ordering::Relaxed);
        let spawned = std::thread::Builder::new()
            .name("styles".into())
            .spawn(move || {
                /// The count goes down even if drawing fails.
                struct Running;
                impl Drop for Running {
                    fn drop(&mut self) {
                        RUNNING.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                let _running = Running;
                let styles = Arc::new(Styles {
                    version: next_rev(),
                    ..make(&work.key.effects)
                });
                *work.done.lock().unwrap_or_else(|e| e.into_inner()) = Some(styles);
            });
        if spawned.is_err() {
            RUNNING.fetch_sub(1, Ordering::Relaxed);
            return Err(());
        }
        *slot = Some(job);
        drop(slot);
        Ok(self.styles_cached())
    }

    /// Its pixels are about to change: the styles drawn for them stay
    /// where the layer is now (see [`Self::placed_styles`]).
    pub(super) fn anchor_styles(&self) {
        if let Some(c) = self.styles_cache.borrow().as_ref()
            && c.rev == self.rev
        {
            c.offset.set(self.offset);
            c.moved.set((0.0, 0.0));
        }
    }

    /// Its shape was just moved by `d` (and drawn again): the styles drawn
    /// for its former pixels move with it, to the nearest pixel, until
    /// drawn for the new ones (a drag holds them).
    pub(super) fn carry_styles(&self, d: crate::vector::Pt) {
        if let Some(c) = self.styles_cache.borrow().as_ref()
            && c.rev != self.rev
        {
            let (mx, my) = c.moved.get();
            let (tx, ty) = (mx + d.x, my + d.y);
            let (rx, ry) = (tx.round(), ty.round());
            let at = c.offset.get();
            c.offset.set((at.0 + rx as i64, at.1 + ry as i64));
            c.moved.set((tx - rx, ty - ry));
        }
    }

    /// The last styles drawn for the layer, whatever its pixels and
    /// settings since.
    pub fn styles_cached(&self) -> Option<Arc<Styles>> {
        if !self.effects.rendered() {
            return None;
        }
        let cache = self.styles_cache.borrow();
        cache.as_ref().map(|c| self.placed_styles(c))
    }

    /// Styles drawn for other pixels stay where they were in the canvas
    /// (a shape edited: its image's box, so the layer's place, changes);
    /// those of the same pixels move with the layer.
    pub(super) fn placed_styles(&self, c: &StylesCache) -> Arc<Styles> {
        if c.rev == self.rev {
            c.offset.set(self.offset);
            c.moved.set((0.0, 0.0));
            return Arc::clone(&c.styles);
        }
        let at = c.offset.get();
        let (dx, dy) = (at.0 - self.offset.0, at.1 - self.offset.1);
        if (dx, dy) == (0, 0) {
            return Arc::clone(&c.styles);
        }
        let shift = |list: &[crate::compose::styles::FxImage]| {
            let mut list = list.to_vec();
            for f in &mut list {
                f.offset = (f.offset.0 + dx, f.offset.1 + dy);
            }
            list
        };
        Arc::new(Styles {
            exterior: shift(&c.styles.exterior),
            interior: shift(&c.styles.interior),
            version: c.styles.version,
        })
    }
}

/// Beyond the canvas, what is left of a shape to draw styles from: enough
/// for what reaches into it.
const SHAPE_MARGIN: u32 = 512;

/// The canvas area the shape of layer `i`, without pixels of its own, can
/// take: what a group's layers show, or what its mask leaves of the canvas.
/// The styles of its layers are taken as last drawn (they are drawn before
/// it: see [`compose::plan`]).
pub fn styles_area(layers: &[Layer], i: usize, canvas: PixelRect) -> Option<PixelRect> {
    shape_area(layers, i, canvas, false)
}

/// See [`styles_area`]. `fresh`: the styles of its layers drawn now.
fn shape_area(layers: &[Layer], i: usize, canvas: PixelRect, fresh: bool) -> Option<PixelRect> {
    let l = &layers[i];
    let hiding = l.active_mask().filter(|m| m.default == 0);
    if !l.is_group() {
        return match hiding {
            Some(m) => intersect(m.rect(l.offset), canvas),
            None => Some(canvas),
        };
    }
    let mut j = i;
    let mut area: Option<PixelRect> = None;
    while j > 0 && layers[j - 1].depth > l.depth {
        j -= 1;
        let c = &layers[j];
        // What is clipped stays within its base, what is hidden shows
        // nothing; a nested group answers for its own layers.
        if c.depth != l.depth + 1 || !c.visible || c.clipped {
            continue;
        }
        let e = if c.is_group() {
            let inner = shape_area(layers, j, canvas, fresh);
            let styles = match fresh {
                true => styles_for(layers, j, canvas, false, false, inner),
                false => c.styles_cached(),
            };
            inner.map(|a| c.with_styles_extent(a, styles.as_deref()))
        } else {
            let styles = match fresh {
                true => styles_of(layers, j, canvas, false, false),
                false => c.styles_cached(),
            };
            match c.kind {
                // It adds no opacity, only what its styles draw.
                LayerKind::Adjustment(_) => c.styles_box(styles.as_deref()),
                _ => Some(c.extent_with(styles.as_deref()).unwrap_or(canvas)),
            }
        };
        if let Some(e) = e.filter(|e| e.w > 0 && e.h > 0) {
            area = Some(area.map_or(e, |a| a.union(e)));
        }
    }
    let m = SHAPE_MARGIN as i64;
    let grown = PixelRect {
        x: canvas.x - m,
        y: canvas.y - m,
        w: canvas.w + 2 * SHAPE_MARGIN,
        h: canvas.h + 2 * SHAPE_MARGIN,
    };
    let area = intersect(area?, grown)?;
    match hiding {
        Some(m) => intersect(area, m.rect(l.offset)),
        None => Some(area),
    }
}

/// What the kind of `l` does to the opacity of a shape: a solid color and
/// an adjustment don't change it, a gradient or pattern can, by its place
/// in the canvas too.
fn hash_kind_shape(l: &Layer, h: &mut DefaultHasher) {
    std::mem::discriminant(&l.kind).hash(h);
    if let LayerKind::Fill(fill) = &l.kind
        && !matches!(fill, Fill::Solid(_))
    {
        format!("{fill:?}").hash(h);
        l.offset.hash(h);
    }
}

/// Fingerprint of what the shape of layer `i` is drawn from, over `area`,
/// everything relative to the layer: a whole group moved keeps its styles,
/// which move with it (see [`styles_for`]), as long as it isn't cut
/// differently.
fn shape_key(layers: &[Layer], i: usize, canvas: PixelRect, area: Option<PixelRect>) -> u64 {
    let mut h = DefaultHasher::new();
    let l = &layers[i];
    let at = |o: (i64, i64)| (o.0 - l.offset.0, o.1 - l.offset.1);
    let mask = |c: &Layer| {
        c.active_mask()
            .map_or(0, |m| m.key_from(c.offset, l.offset))
    };
    (canvas, area.map(|a| (at((a.x, a.y)), a.w, a.h)), mask(l)).hash(&mut h);
    hash_kind_shape(l, &mut h);
    if l.is_group() {
        let mut j = i;
        while j > 0 && layers[j - 1].depth > l.depth {
            j -= 1;
            let c = &layers[j];
            (
                c.id,
                c.rev,
                at(c.offset),
                c.visible,
                c.clipped,
                c.depth,
                mask(c),
            )
                .hash(&mut h);
            (c.opacity.to_bits(), c.fill.to_bits(), c.blend).hash(&mut h);
            hash_kind_shape(c, &mut h);
            format!("{:?}", c.effects).hash(&mut h);
        }
    }
    h.finish()
}

/// The shape of layer `i` over `area`: what it shows, whole and in Normal
/// mode, an adjustment layer its mask.
fn draw_shape(layers: &[Layer], i: usize, canvas: PixelRect, area: PixelRect) -> RgbaImage {
    let options = compose::Options {
        only: Some(i),
        no_styles: Some(i),
        ..Default::default()
    };
    let mut plan = compose::plan(layers, canvas, &mut compose::FullSize, &options);
    for node in &mut plan.nodes {
        match node {
            compose::Node::Layer(n) => {
                (n.opacity, n.fill) = (1.0, 1.0);
                if let compose::Content::Adjust(_) = n.content {
                    n.content = compose::Content::Solid([1.0; 3]);
                }
            }
            compose::Node::Group(g) => (g.opacity, g.fill) = (1.0, 1.0),
        }
    }
    plan.render(area)
}

/// Styles drawn from a shape over `area`, placed relative to the layer.
fn styles_from(shape: &RgbaImage, area: PixelRect, offset: (i64, i64), fx: &Effects) -> Styles {
    let mut styles = render_styles(shape, fx);
    let (dx, dy) = (area.x - offset.0, area.y - offset.1);
    for f in styles.exterior.iter_mut().chain(&mut styles.interior) {
        f.offset = (f.offset.0 + dx, f.offset.1 + dy);
    }
    styles
}

/// The styles of layer `i` as shown, whatever its kind: a group draws them
/// from what its layers show, a fill or adjustment layer from its content
/// through its mask. `later`: see [`Layer::styles_later`].
pub fn styles_of(
    layers: &[Layer],
    i: usize,
    canvas: PixelRect,
    held: bool,
    later: bool,
) -> Option<Arc<Styles>> {
    let area = match layers[i].has_pixels() {
        true => None,
        false => shape_area(layers, i, canvas, !later && !held),
    };
    styles_for(layers, i, canvas, held, later, area)
}

/// [`styles_of`], the layer's shape taking `area` (see [`shape_area`]; for
/// a layer without pixels).
fn styles_for(
    layers: &[Layer],
    i: usize,
    canvas: PixelRect,
    held: bool,
    later: bool,
    area: Option<PixelRect>,
) -> Option<Arc<Styles>> {
    let l = &layers[i];
    if l.has_pixels() {
        return l.styles_shown(held, later);
    }
    if !l.styleable() || !l.effects.rendered() {
        return None;
    }
    let key = Key {
        rev: shape_key(layers, i, canvas, area),
        mask: 0,
        effects: l.effects.clone(),
    };
    if let Some(c) = l.styles_cache.borrow().as_ref()
        && c.effects == key.effects
    {
        if c.rev == key.rev {
            // Drawn for this shape, wherever the layer is now.
            c.offset.set(l.offset);
            c.moved.set((0.0, 0.0));
            return Some(Arc::clone(&c.styles));
        }
        if held {
            return Some(l.placed_styles(c));
        }
    }
    let Some(area) = area else {
        // No shape left: nothing to draw from, nothing shows.
        return Some(l.keep_styles(&key, Styles::default()));
    };
    if later && u64::from(area.w) * u64::from(area.h) > SHAPE_AT_ONCE {
        let prepare = || {
            // The layer and those it shows, on their own.
            let held_below = (0..i).rev().take_while(|&j| layers[j].depth > l.depth);
            let start = i - if l.is_group() { held_below.count() } else { 0 };
            let part: Vec<Layer> = layers[start..=i].to_vec();
            let (at, offset) = (i - start, l.offset);
            move |fx: &Effects| styles_from(&draw_shape(&part, at, canvas, area), area, offset, fx)
        };
        if let Ok(styles) = l.draw_aside(key.clone(), prepare) {
            return styles;
        }
    }
    let shape = draw_shape(layers, i, canvas, area);
    Some(l.keep_styles(&key, styles_from(&shape, area, l.offset, &l.effects)))
}

impl Layer {
    /// `styles`, just drawn for `key`, as the last ones drawn.
    fn keep_styles(&self, key: &Key, styles: Styles) -> Arc<Styles> {
        let styles = Arc::new(Styles {
            version: next_rev(),
            ..styles
        });
        *self.styles_cache.borrow_mut() = Some(StylesCache {
            rev: key.rev,
            mask: key.mask,
            effects: key.effects.clone(),
            styles: Arc::clone(&styles),
            offset: self.offset.into(),
            moved: Default::default(),
        });
        styles
    }

    /// A copy under its own `id`: same styles so far, its own jobs.
    pub(super) fn copy_as(&self, id: u64) -> Layer {
        Layer {
            id,
            styles_job: Default::default(),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::compose;
    use crate::compose::styles::StrokeFx;
    use crate::document::{Document, Fill, LayerKind, NewMask, PixelRect, Selection};
    use crate::pixel_ops::{AdjustKind, Adjustment};
    use image::{Rgba, RgbaImage};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// White 40 × 40, a group over it holding a red square at (15, 15), 10
    /// wide (the group active).
    fn group_doc() -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(40, 40, Rgba([255; 4])), None);
        doc.add_group();
        doc.add_layer(
            "square".into(),
            RgbaImage::from_pixel(10, 10, Rgba(RED)),
            (15, 15),
        );
        doc.active = 2;
        assert!(doc.active_layer().is_group());
        doc
    }

    fn stroke() -> StrokeFx {
        StrokeFx {
            enabled: true,
            color: GREEN,
            width: 2.0,
            opacity: 1.0,
            blend: crate::compose::BlendMode::Normal,
        }
    }

    #[test]
    fn group_styles_follow_what_its_layers_show() {
        let mut doc = group_doc();
        doc.active_layer_mut().effects.stroke = stroke();
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(14, 20).0, GREEN, "stroke around the layers");
        assert_eq!(flat.get_pixel(20, 20).0, RED);
        assert_eq!(flat.get_pixel(5, 5).0, [255; 4]);
        // A color overlay covers the group's shape only.
        doc.active_layer_mut().effects.color_overlay.enabled = true;
        doc.active_layer_mut().effects.color_overlay.color = [0, 0, 255, 255];
        doc.active_layer_mut().effects.color_overlay.opacity = 1.0;
        doc.active_layer_mut().effects.color_overlay.blend = crate::compose::BlendMode::Normal;
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(20, 20).0, [0, 0, 255, 255]);
        assert_eq!(flat.get_pixel(5, 5).0, [255; 4]);
        // The styles follow when a layer of the group changes.
        doc.layers[1].offset = (25, 15);
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(24, 20).0, GREEN);
        assert_eq!(flat.get_pixel(14, 20).0, [255; 4]);
    }

    /// Fill opacity fades the layers, not the styles; opacity both.
    #[test]
    fn group_fill_keeps_styles() {
        let mut doc = group_doc();
        doc.active_layer_mut().effects.stroke = stroke();
        doc.active_layer_mut().fill = 0.0;
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(14, 20).0, GREEN);
        assert_eq!(flat.get_pixel(20, 20).0, [255; 4], "the content is gone");
        doc.active_layer_mut().fill = 1.0;
        doc.active_layer_mut().opacity = 0.0;
        assert_eq!(doc.flatten().get_pixel(14, 20).0, [255; 4]);
    }

    /// Pass Through with styles: drawn apart, in Normal.
    #[test]
    fn pass_through_group_with_styles_composites_apart() {
        let mut doc = group_doc();
        assert_eq!(
            doc.active_layer().blend,
            crate::compose::BlendMode::PassThrough
        );
        doc.active_layer_mut().effects.stroke = stroke();
        assert_eq!(doc.flatten().get_pixel(20, 20).0, RED);
    }

    /// A fill's styles draw from its mask.
    #[test]
    fn fill_layer_styles_use_its_mask() {
        let mut doc = Document::new(RgbaImage::from_pixel(40, 40, Rgba([255; 4])), None);
        let area: Selection = PixelRect {
            x: 15,
            y: 15,
            w: 10,
            h: 10,
        }
        .into();
        doc.add_fill_layer(Fill::Solid([255, 0, 0]), Some(&area));
        assert!(doc.active_layer().mask.is_some());
        doc.active_layer_mut().effects.stroke = stroke();
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(20, 20).0, RED);
        assert_eq!(flat.get_pixel(14, 20).0, GREEN);
        assert_eq!(flat.get_pixel(5, 5).0, [255; 4]);
    }

    /// An adjustment layer keeps adjusting; its styles draw at its mask.
    #[test]
    fn adjustment_layer_styles_use_its_mask() {
        let mut doc = Document::new(RgbaImage::from_pixel(40, 40, Rgba([0, 0, 0, 255])), None);
        let area: Selection = PixelRect {
            x: 15,
            y: 15,
            w: 10,
            h: 10,
        }
        .into();
        doc.add_adjustment_layer(Adjustment::Invert, Some(&area));
        doc.active_layer_mut().effects.stroke = stroke();
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(20, 20).0, [255; 4], "inverted");
        assert_eq!(flat.get_pixel(14, 20).0, GREEN);
        assert_eq!(flat.get_pixel(5, 5).0, [0, 0, 0, 255]);
    }

    /// The display's styles, drawn aside on a large group, end up the same
    /// as drawn at once; the extent includes them.
    #[test]
    fn large_group_styles_are_drawn_aside() {
        let mut doc = Document::new(RgbaImage::from_pixel(2000, 1000, Rgba([255; 4])), None);
        doc.add_group();
        doc.add_layer(
            "square".into(),
            RgbaImage::from_pixel(10, 10, Rgba(RED)),
            (15, 15),
        );
        doc.layers[1].replace_pixels(RgbaImage::from_pixel(1800, 800, Rgba(RED)));
        doc.active = 2;
        doc.active_layer_mut().effects.stroke = stroke();
        let canvas = doc.full_rect();
        assert!(super::styles_of(&doc.layers, 2, canvas, false, true).is_none());
        let mut styles = None;
        for _ in 0..2000 {
            styles = super::styles_of(&doc.layers, 2, canvas, false, true);
            if styles.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let styles = styles.expect("drawn");
        let at_once = super::styles_of(&doc.layers, 2, canvas, false, false).unwrap();
        assert_eq!(styles.extent(), at_once.extent());
        assert!(!doc.active_layer().effects.color_overlay.enabled);
    }

    /// A mask on the group shapes its styles.
    #[test]
    fn group_mask_shapes_its_styles() {
        let mut doc = group_doc();
        doc.active_layer_mut().effects.color_overlay.enabled = true;
        doc.active_layer_mut().effects.color_overlay.opacity = 1.0;
        doc.active_layer_mut().effects.color_overlay.color = [0, 0, 255, 255];
        assert!(doc.add_mask(NewMask::HideAll, None));
        assert_eq!(doc.flatten().get_pixel(20, 20).0, [255; 4]);
    }

    /// The styles of a large layer are drawn in the background: none at
    /// first, then the same as drawn at once; after a change, the former
    /// ones until the new ones are ready.
    #[test]
    fn large_styles_are_drawn_in_the_background() {
        let (w, h) = (1100, 1000);
        let mut doc = Document::new(RgbaImage::new(w, h), None);
        let square = RgbaImage::from_fn(w, h, |x, y| {
            let inside = (300..700).contains(&x) && (200..600).contains(&y);
            Rgba(if inside { [0, 0, 255, 255] } else { [0; 4] })
        });
        doc.add_layer("s".into(), square, (0, 0));
        let fx = &mut doc.active_layer_mut().effects;
        fx.stroke.enabled = true;
        fx.stroke.width = 3.0;
        let ready = |doc: &Document| {
            let start = std::time::Instant::now();
            loop {
                let layer = doc.active_layer();
                if let Some(s) = layer.styles_later(false)
                    && layer
                        .styles_cached()
                        .is_some_and(|c| c.version == s.version)
                {
                    let direct = layer.clone();
                    *direct.styles_cache.borrow_mut() = None;
                    if direct.styles(false).is_some_and(|d| {
                        d.exterior.len() == s.exterior.len()
                            && d.exterior
                                .iter()
                                .zip(&s.exterior)
                                .all(|(a, b)| *a.coverage == *b.coverage)
                    }) {
                        return s;
                    }
                }
                assert!(start.elapsed().as_secs() < 60, "styles never came");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        assert!(doc.active_layer().styles_later(false).is_none(), "none yet");
        let first = ready(&doc);
        doc.active_layer_mut()
            .pixels_mut()
            .put_pixel(100, 100, Rgba([255, 0, 0, 255]));
        let meanwhile = doc.active_layer().styles_later(false).expect("the former");
        assert_eq!(meanwhile.version, first.version);
        let next = ready(&doc);
        assert_ne!(next.version, first.version);
    }

    /// The display's plan until nothing is being drawn aside any more.
    fn settle(doc: &Document) {
        let opts = compose::Options {
            later_styles: true,
            canvas: Some(doc.full_rect()),
            ..Default::default()
        };
        let start = std::time::Instant::now();
        loop {
            let _ = compose::signatures(&doc.layers, &opts);
            let _ = compose::plan(&doc.layers, doc.full_rect(), &mut compose::FullSize, &opts);
            // Nothing running after a whole frame: that frame had all it needs.
            if !super::styles_pending() {
                let _ = compose::signatures(&doc.layers, &opts);
                let _ = compose::plan(&doc.layers, doc.full_rect(), &mut compose::FullSize, &opts);
                if !super::styles_pending() {
                    return;
                }
            }
            assert!(start.elapsed().as_secs() < 120, "styles never came");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// What the display shows now.
    fn shown(doc: &Document) -> RgbaImage {
        let opts = compose::Options {
            later_styles: true,
            canvas: Some(doc.full_rect()),
            ..Default::default()
        };
        compose::plan(&doc.layers, doc.full_rect(), &mut compose::FullSize, &opts)
            .render(doc.full_rect())
    }

    fn colored(color: [u8; 4], width: f32) -> StrokeFx {
        StrokeFx {
            color,
            width,
            ..stroke()
        }
    }

    /// `group_doc` at another size: a `side`-wide square at `at`.
    fn square_group(canvas: u32, side: u32, at: (i64, i64)) -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(canvas, canvas, Rgba([255; 4])), None);
        doc.add_group();
        doc.add_layer(
            "square".into(),
            RgbaImage::from_pixel(side, side, Rgba(RED)),
            at,
        );
        doc.active = 2;
        doc
    }

    /// A group or fill that lost its shape keeps no styles: on the display
    /// and in exports, like a fresh one.
    #[test]
    fn no_shape_no_styles() {
        let blank = |doc: &Document| doc.flatten().pixels().all(|p| p.0 == [255; 4]);
        // Its only layer deleted.
        let mut doc = square_group(100, 20, (40, 40));
        doc.layers[2].effects.stroke = colored(GREEN, 5.0);
        assert_eq!(doc.flatten().get_pixel(37, 50).0, GREEN);
        doc.active = 1;
        assert!(doc.delete_active());
        assert!(blank(&doc), "layer deleted");
        assert!(shown(&doc).pixels().all(|p| p.0 == [255; 4]));
        // Its only layer far away, then back.
        let mut doc = square_group(100, 20, (40, 40));
        doc.layers[2].effects.stroke = colored(GREEN, 5.0);
        doc.flatten();
        doc.layers[1].offset = (5000, 5000);
        assert!(blank(&doc), "layer moved away");
        doc.layers[1].offset = (40, 40);
        assert_eq!(doc.flatten().get_pixel(37, 50).0, GREEN);
        // A fill whose mask left the canvas.
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        let area: Selection = PixelRect {
            x: 40,
            y: 40,
            w: 20,
            h: 20,
        }
        .into();
        doc.add_fill_layer(Fill::Solid([255, 0, 0]), Some(&area));
        doc.active_layer_mut().effects.stroke = colored(GREEN, 5.0);
        assert_eq!(doc.flatten().get_pixel(37, 50).0, GREEN);
        doc.active_layer_mut().translate(1000, 1000);
        assert!(blank(&doc), "mask moved away");
    }

    /// A large layer of a group draws its styles aside: the group's shape,
    /// first cut without them, is redrawn once they are there, so the
    /// display settles on what an export of a new document gives, and an
    /// export after the display too.
    #[test]
    fn group_redraws_when_the_styles_of_its_layers_arrive() {
        let build = || {
            let mut doc = square_group(1500, 10, (0, 0));
            doc.layers[1].replace_pixels(RgbaImage::from_pixel(1100, 1100, Rgba(RED)));
            doc.layers[1].offset = (200, 200);
            doc.layers[1].effects.stroke = colored(GREEN, 40.0);
            doc.layers[2].effects.stroke = colored(BLUE, 10.0);
            doc
        };
        let doc = build();
        settle(&doc);
        let display = shown(&doc);
        let fresh = build().flatten();
        assert_eq!(
            fresh.get_pixel(155, 700).0,
            BLUE,
            "around the layer's stroke"
        );
        assert_eq!(display.get_pixel(155, 700).0, BLUE);
        assert!(display.as_raw() == fresh.as_raw(), "the display settles");
        assert!(
            doc.flatten().as_raw() == fresh.as_raw(),
            "the export follows"
        );
    }

    /// What a nested group's styles add is part of the outer group's shape,
    /// as if they were on its layer.
    #[test]
    fn nested_group_styles_are_in_the_parent_shape() {
        let build = |on_group: bool| {
            let mut doc = Document::new(RgbaImage::from_pixel(120, 120, Rgba([255; 4])), None);
            doc.add_group();
            doc.add_group();
            doc.add_layer(
                "sq".into(),
                RgbaImage::from_pixel(20, 20, Rgba(RED)),
                (50, 50),
            );
            assert_eq!(
                doc.layers.iter().map(|l| l.depth).collect::<Vec<_>>(),
                [0, 2, 1, 0]
            );
            let target = if on_group { 2 } else { 1 };
            doc.layers[target].effects.stroke = colored(GREEN, 8.0);
            doc.layers[3].effects.stroke = colored(BLUE, 3.0);
            doc
        };
        let (layer, group) = (build(false).flatten(), build(true).flatten());
        assert_eq!(layer.get_pixel(40, 60).0, BLUE);
        assert_eq!(group.get_pixel(40, 60).0, BLUE);
        assert_eq!(group.get_pixel(45, 60).0, GREEN);
    }

    /// Merging keeps what the styles draw beyond the layers, for every kind.
    #[test]
    fn merges_keep_the_styles_of_every_kind() {
        // Merge Group, with a stroke beyond its layer.
        let mut doc = square_group(100, 20, (40, 40));
        doc.active_layer_mut().effects.stroke = colored(GREEN, 5.0);
        let before = doc.flatten();
        doc.merge_group().unwrap();
        assert!(doc.flatten().as_raw() == before.as_raw(), "Merge Group");
        // Nested styled group merged with its parent.
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.add_group();
        doc.add_group();
        doc.add_layer(
            "sq".into(),
            RgbaImage::from_pixel(20, 20, Rgba(RED)),
            (40, 40),
        );
        doc.layers[2].effects.stroke = colored(GREEN, 5.0);
        doc.layers[3].effects.stroke = colored(BLUE, 2.0);
        doc.active = 3;
        let before = doc.flatten();
        doc.merge_group().unwrap();
        assert!(doc.flatten().as_raw() == before.as_raw(), "nested");
        // Merge Down of an adjustment with a drop shadow.
        let shadow = |doc: &mut Document| {
            let s = &mut doc.active_layer_mut().effects.shadow;
            (s.enabled, s.dx, s.dy, s.blur, s.opacity) = (true, 30.0, 0.0, 0.0, 1.0);
        };
        let area: Selection = PixelRect {
            x: 45,
            y: 45,
            w: 10,
            h: 10,
        }
        .into();
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.add_layer(
            "sq".into(),
            RgbaImage::from_pixel(20, 20, Rgba(RED)),
            (40, 40),
        );
        doc.add_adjustment_layer(Adjustment::Invert, Some(&area));
        shadow(&mut doc);
        let before = doc.flatten();
        doc.merge_down().unwrap();
        assert!(doc.flatten().as_raw() == before.as_raw(), "adjustment");
        // Merge Down of a masked fill with a stroke.
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.add_layer(
            "sq".into(),
            RgbaImage::from_pixel(20, 20, Rgba(RED)),
            (40, 40),
        );
        doc.add_fill_layer(Fill::Solid([0, 0, 255]), Some(&area));
        doc.active_layer_mut().effects.stroke = colored(GREEN, 15.0);
        let before = doc.flatten();
        doc.merge_down().unwrap();
        assert!(doc.flatten().as_raw() == before.as_raw(), "fill");
        // Merge Down of a pixel layer with a stroke, and Merge Visible.
        let build = || {
            let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([0; 4])), None);
            doc.add_layer(
                "a".into(),
                RgbaImage::from_pixel(20, 20, Rgba(RED)),
                (40, 40),
            );
            doc.add_layer(
                "b".into(),
                RgbaImage::from_pixel(5, 5, Rgba(BLUE)),
                (45, 45),
            );
            doc.active_layer_mut().effects.stroke = colored(GREEN, 30.0);
            doc
        };
        let mut doc = build();
        let before = doc.flatten();
        assert_eq!(before.get_pixel(20, 47).0, GREEN);
        doc.merge_down().unwrap();
        assert!(doc.flatten().as_raw() == before.as_raw(), "pixel layer");
        let mut doc = build();
        doc.layers[0].visible = false;
        let visible = doc.flatten();
        doc.merge_visible().unwrap();
        assert!(doc.flatten().as_raw() == visible.as_raw(), "Merge Visible");
    }

    /// Convert to Smart Object keeps the look, styles of a masked fill and
    /// of a group included.
    #[test]
    fn smart_object_bounds_hold_the_styles() {
        let area: Selection = PixelRect {
            x: 45,
            y: 45,
            w: 10,
            h: 10,
        }
        .into();
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.add_fill_layer(Fill::Solid([0, 0, 255]), Some(&area));
        doc.active_layer_mut().effects.stroke = colored(GREEN, 15.0);
        let before = doc.flatten();
        let i = doc.active;
        assert!(doc.convert_to_smart(&[i]));
        assert!(doc.flatten().as_raw() == before.as_raw(), "fill");
        // A layer past the canvas: the group's stroke beyond it stays.
        let mut doc = square_group(100, 20, (90, 40));
        doc.active_layer_mut().effects.stroke = colored(GREEN, 15.0);
        let before = doc.flatten();
        let i = doc.active;
        assert!(doc.convert_to_smart(&[i]));
        assert!(doc.flatten().as_raw() == before.as_raw(), "group");
    }

    /// A group with styles still being drawn composites as it will with
    /// them: fill, Pass Through and isolation don't jump when they arrive.
    #[test]
    fn styles_being_drawn_dont_change_how_a_group_composites() {
        let mut doc = square_group(1500, 10, (0, 0));
        doc.layers[1].replace_pixels(RgbaImage::from_pixel(
            1400,
            1400,
            Rgba([128, 128, 128, 255]),
        ));
        doc.layers[1].blend = compose::BlendMode::Multiply;
        doc.layers[0].replace_pixels(RgbaImage::from_pixel(1500, 1500, Rgba(RED)));
        doc.layers[2].effects.stroke = stroke();
        doc.layers[2].fill = 0.25;
        let first = shown(&doc).get_pixel(100, 100).0;
        assert!(doc.layers[2].styles_cached().is_none(), "still being drawn");
        settle(&doc);
        assert_eq!(first, shown(&doc).get_pixel(100, 100).0);
        // A group without styles passes through.
        doc.layers[2].effects = Default::default();
        let plain = shown(&doc).get_pixel(100, 100).0;
        assert_ne!(first, plain);
    }

    /// What a group's overlays span is what it shows: neither hidden layers
    /// nor what an adjustment doesn't draw.
    #[test]
    fn hidden_layers_are_out_of_the_box() {
        let mut doc = Document::new(RgbaImage::from_pixel(200, 100, Rgba([255; 4])), None);
        doc.add_group();
        doc.add_layer(
            "a".into(),
            RgbaImage::from_pixel(20, 20, Rgba(RED)),
            (150, 10),
        );
        doc.add_layer(
            "b".into(),
            RgbaImage::from_pixel(20, 20, Rgba(RED)),
            (10, 10),
        );
        let g = doc.layers.iter().position(|l| l.is_group()).unwrap();
        let a = doc.layers.iter().position(|l| l.name == "a").unwrap();
        doc.layers[g].effects.gradient_overlay.enabled = true;
        doc.layers[g].effects.gradient_overlay.fill.angle = 0.0;
        let canvas = doc.full_rect();
        assert_eq!(super::styles_area(&doc.layers, g, canvas).unwrap().w, 160);
        let wide = doc.flatten().get_pixel(29, 15).0;
        doc.layers[a].visible = false;
        assert_eq!(super::styles_area(&doc.layers, g, canvas).unwrap().w, 20);
        assert_ne!(
            doc.flatten().get_pixel(29, 15).0,
            wide,
            "the gradient refits"
        );
        // A hidden nested group, all of it.
        let mut doc = Document::new(RgbaImage::from_pixel(200, 100, Rgba([255; 4])), None);
        doc.add_group();
        doc.add_group();
        doc.add_layer(
            "far".into(),
            RgbaImage::from_pixel(20, 20, Rgba(RED)),
            (150, 10),
        );
        let (inner, outer) = (2, 3);
        assert_eq!(doc.layers[inner].depth, 1);
        let mut near = doc.layers[1].copy_as(1000);
        (near.depth, near.offset) = (1, (10, 10));
        doc.layers.insert(outer, near);
        let outer = outer + 1;
        assert_eq!(
            super::styles_area(&doc.layers, outer, doc.full_rect())
                .unwrap()
                .w,
            160
        );
        doc.layers[inner].visible = false;
        assert_eq!(
            super::styles_area(&doc.layers, outer, doc.full_rect())
                .unwrap()
                .w,
            20
        );
    }

    /// A whole group moved keeps its styles (no drawing), which move with
    /// it; cut by the edge or with one layer moved alone, they are redrawn.
    #[test]
    fn moving_a_whole_group_keeps_its_styles() {
        let mut doc = square_group(1000, 100, (400, 400));
        let g = doc.active;
        let fx = &mut doc.layers[g].effects;
        fx.stroke = colored(GREEN, 5.0);
        fx.shadow.enabled = true;
        settle(&doc);
        let canvas = doc.full_rect();
        let first = super::styles_of(&doc.layers, g, canvas, false, true).unwrap();
        let by = |doc: &mut Document, d: i64| {
            for j in doc.moving_with(g) {
                doc.layers[j].offset.0 += d;
            }
        };
        by(&mut doc, 30);
        let moved = super::styles_of(&doc.layers, g, canvas, false, true).unwrap();
        assert_eq!(moved.version, first.version, "not drawn again");
        // In its new place, like one drawn there.
        let mut fresh = square_group(1000, 100, (430, 400));
        fresh.layers[g].effects = doc.layers[g].effects.clone();
        let shown_now = shown(&doc);
        assert!(shown_now.as_raw() == fresh.flatten().as_raw());
        assert!(doc.flatten().as_raw() == shown_now.as_raw());
        // An unlinked mask stays in the canvas: the shape changes.
        let mut doc = square_group(1000, 100, (400, 400));
        doc.layers[g].effects.stroke = colored(GREEN, 5.0);
        assert!(doc.add_mask(NewMask::RevealAll, None));
        let offset = doc.layers[g].offset;
        doc.layers[g]
            .mask
            .as_mut()
            .unwrap()
            .set_linked(false, offset);
        let first = super::styles_of(&doc.layers, g, canvas, false, false).unwrap();
        by(&mut doc, 30);
        let moved = super::styles_of(&doc.layers, g, canvas, false, false).unwrap();
        assert_ne!(moved.version, first.version);
        // One layer alone.
        let mut doc = square_group(1000, 100, (400, 400));
        doc.layers[g].effects.stroke = colored(GREEN, 5.0);
        let first = super::styles_of(&doc.layers, g, canvas, false, false).unwrap();
        doc.layers[1].offset.0 += 1;
        let moved = super::styles_of(&doc.layers, g, canvas, false, false).unwrap();
        assert_ne!(moved.version, first.version);
        // Reaching the edge of what is kept: cut differently.
        let mut doc = square_group(1000, 100, (400, 400));
        doc.layers[g].effects.stroke = colored(GREEN, 5.0);
        let first = super::styles_of(&doc.layers, g, canvas, false, false).unwrap();
        by(&mut doc, 1100);
        let moved = super::styles_of(&doc.layers, g, canvas, false, false).unwrap();
        assert_ne!(moved.version, first.version);
    }

    /// A layer dragged alone in a group of more than a quarter megapixel:
    /// the display draws aside, not on every frame of the drag.
    #[test]
    fn dragging_a_layer_of_a_group_draws_aside() {
        let mut doc = square_group(2000, 600, (400, 400));
        let g = doc.active;
        doc.layers[g].effects.stroke = stroke();
        settle(&doc);
        let canvas = doc.full_rect();
        let first = super::styles_of(&doc.layers, g, canvas, false, true).unwrap();
        doc.layers[1].offset.0 += 1;
        let meanwhile = super::styles_of(&doc.layers, g, canvas, false, true).unwrap();
        assert_eq!(meanwhile.version, first.version, "the former ones");
        settle(&doc);
        let next = super::styles_of(&doc.layers, g, canvas, false, true).unwrap();
        assert_ne!(next.version, first.version);
    }

    /// The shape is opacity only: colors and adjustments of a group's
    /// layers don't redraw its styles, a gradient's opacity does.
    #[test]
    fn color_changes_keep_the_shape() {
        let mut doc = square_group(100, 20, (40, 40));
        let area: Selection = PixelRect {
            x: 10,
            y: 10,
            w: 10,
            h: 10,
        }
        .into();
        doc.active = 1;
        doc.add_adjustment_layer(Adjustment::Invert, None);
        doc.add_fill_layer(Fill::Solid([1, 2, 3]), Some(&area));
        let (adj, fill) = (
            doc.layers
                .iter()
                .position(|l| matches!(l.kind, LayerKind::Adjustment(_)))
                .unwrap(),
            doc.layers
                .iter()
                .position(|l| matches!(l.kind, LayerKind::Fill(_)))
                .unwrap(),
        );
        let g = doc.layers.iter().position(|l| l.is_group()).unwrap();
        assert!(doc.layers[adj].depth > 0 && doc.layers[fill].depth > 0);
        doc.layers[g].effects.stroke = stroke();
        let canvas = doc.full_rect();
        let version = |doc: &Document| {
            super::styles_of(&doc.layers, g, canvas, false, false)
                .unwrap()
                .version
        };
        let first = version(&doc);
        doc.layers[adj].kind = LayerKind::Adjustment(AdjustKind::Posterize.initial());
        assert_eq!(version(&doc), first, "adjustment");
        doc.layers[fill].kind = LayerKind::Fill(Fill::Solid([9, 9, 9]));
        assert_eq!(version(&doc), first, "solid color");
        doc.layers[fill].opacity = 0.5;
        assert_ne!(version(&doc), first, "opacity");
        // A fill's own color, styled on its own.
        let fill_styled = &mut doc.layers[fill];
        fill_styled.effects.stroke = stroke();
        let own = super::styles_of(&doc.layers, fill, canvas, false, false)
            .unwrap()
            .version;
        doc.layers[fill].kind = LayerKind::Fill(Fill::Solid([200, 9, 9]));
        let same = super::styles_of(&doc.layers, fill, canvas, false, false)
            .unwrap()
            .version;
        assert_eq!(own, same);
    }

    /// Copies of a layer don't share its job: each gets its own styles.
    #[test]
    fn a_copy_has_its_own_job_slot() {
        let mut doc = square_group(100, 20, (40, 40));
        doc.duplicate_active();
        let copy = doc.active;
        let original = doc.layers.iter().position(|l| l.is_group()).unwrap();
        assert_ne!(copy, original);
        let same = std::sync::Arc::ptr_eq(
            &doc.layers[original].styles_job,
            &doc.layers[copy].styles_job,
        );
        assert!(!same);
    }
}
