//! Layer styles drawn on worker threads for the display: on a large layer
//! they take up to a second (distance fields, blurs), so the view keeps
//! showing the former ones until the new ones are ready.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::RgbaImage;

use super::{Layer, LayerKind, PixelRect, StylesCache, intersect, next_rev};
use crate::compose;
use crate::compose::styles::{Effects, Styles, render_styles};

/// Below this many pixels, styles are drawn at once (quick enough).
const AT_ONCE: u64 = 1 << 20;

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
        let (image, mask, offset) = (
            self.shared_image(),
            self.active_mask().cloned(),
            self.offset,
        );
        let make = move |fx: &Effects| {
            let shape = match mask {
                Some(m) => Arc::new(m.apply_to(&image, offset)),
                None => image,
            };
            render_styles(&shape, fx)
        };
        match self.draw_aside(key, make) {
            Ok(styles) => styles,
            Err(()) => self.styles(held),
        }
    }

    /// The styles for `key` once `make` drew them on a worker thread, the
    /// former ones meanwhile. `Err`: no thread to draw them.
    fn draw_aside(
        &self,
        key: Key,
        make: impl FnOnce(&Effects) -> Styles + Send + 'static,
    ) -> Result<Option<Arc<Styles>>, ()> {
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
                offset: job.offset.into(),
                moved: Default::default(),
            });
            if job.key == key {
                return Ok(Some(styles));
            }
        }
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
/// take: a group's layers, or what its mask leaves of the canvas.
pub fn styles_area(layers: &[Layer], i: usize, canvas: PixelRect) -> Option<PixelRect> {
    let l = &layers[i];
    if l.is_group() {
        let mut j = i;
        let mut area: Option<PixelRect> = None;
        while j > 0 && layers[j - 1].depth > l.depth {
            j -= 1;
            let c = &layers[j];
            if c.is_group() {
                continue;
            }
            let e = c.extent_with(c.styles_cached().as_deref()).unwrap_or(canvas);
            area = Some(area.map_or(e, |a| a.union(e)));
        }
        let m = SHAPE_MARGIN as i64;
        let grown = PixelRect {
            x: canvas.x - m,
            y: canvas.y - m,
            w: canvas.w + 2 * SHAPE_MARGIN,
            h: canvas.h + 2 * SHAPE_MARGIN,
        };
        let area = intersect(area?, grown)?;
        return match l.active_mask().filter(|m| m.default == 0) {
            Some(m) => intersect(area, m.rect(l.offset)),
            None => Some(area),
        };
    }
    match l.active_mask().filter(|m| m.default == 0) {
        Some(m) => intersect(m.rect(l.offset), canvas),
        None => Some(canvas),
    }
}

/// Fingerprint of what the shape of layer `i` is drawn from.
fn shape_key(layers: &[Layer], i: usize, canvas: PixelRect) -> u64 {
    let mut h = DefaultHasher::new();
    let l = &layers[i];
    (canvas, l.offset, l.mask_key()).hash(&mut h);
    if let LayerKind::Fill(_) = &l.kind {
        format!("{:?}", l.kind).hash(&mut h);
    }
    if l.is_group() {
        let mut j = i;
        while j > 0 && layers[j - 1].depth > l.depth {
            j -= 1;
            let c = &layers[j];
            (c.id, c.rev, c.offset, c.visible, c.clipped, c.depth, c.mask_key()).hash(&mut h);
            (c.opacity.to_bits(), c.fill.to_bits(), c.blend).hash(&mut h);
            if !c.is_group() {
                format!("{:?}", c.kind).hash(&mut h);
            }
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
    let l = &layers[i];
    if l.has_pixels() {
        return l.styles_shown(held, later);
    }
    if !l.styleable() || !l.effects.rendered() {
        return None;
    }
    let key = Key {
        rev: shape_key(layers, i, canvas),
        mask: 0,
        effects: l.effects.clone(),
    };
    if let Some(c) = l.styles_cache.borrow().as_ref()
        && c.effects == key.effects
        && (held || c.rev == key.rev)
    {
        return Some(l.placed_styles(c));
    }
    let Some(area) = styles_area(layers, i, canvas) else {
        return l.styles_cached();
    };
    if later && u64::from(area.w) * u64::from(area.h) > AT_ONCE {
        // The layer and those it shows, on their own.
        let held_below = (0..i).rev().take_while(|&j| layers[j].depth > l.depth);
        let start = i - if l.is_group() { held_below.count() } else { 0 };
        let part: Vec<Layer> = layers[start..=i].to_vec();
        let (at, offset) = (i - start, l.offset);
        let make = move |fx: &Effects| {
            styles_from(&draw_shape(&part, at, canvas, area), area, offset, fx)
        };
        if let Ok(styles) = l.draw_aside(key.clone(), make) {
            return styles;
        }
    }
    let shape = draw_shape(layers, i, canvas, area);
    let styles = Arc::new(Styles {
        version: next_rev(),
        ..styles_from(&shape, area, l.offset, &l.effects)
    });
    *l.styles_cache.borrow_mut() = Some(StylesCache {
        rev: key.rev,
        mask: 0,
        effects: key.effects,
        styles: Arc::clone(&styles),
        offset: l.offset.into(),
        moved: Default::default(),
    });
    Some(styles)
}

#[cfg(test)]
mod tests {
    use crate::compose::styles::StrokeFx;
    use crate::document::{Document, Fill, NewMask, PixelRect, Selection};
    use crate::pixel_ops::Adjustment;
    use image::{Rgba, RgbaImage};

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

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
        assert_eq!(doc.active_layer().blend, crate::compose::BlendMode::PassThrough);
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
}
