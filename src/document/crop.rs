//! The Crop tool's cut (the canvas, what falls outside it if asked), and
//! the changes of the whole document computed on a work copy, off the UI
//! thread: Image Rotation, the crop turned or resampled. Put in place, they
//! are one step, whatever they went through.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::imageops::FilterType;
use image::RgbaImage;

use super::{Document, Fill, Layer, LayerKind, PixelRect, State, guides, intersect, next_rev, vectors};
use crate::i18n::t;
use crate::mask_ops::Affine;

/// The highest resolution a crop gives (pixels per inch): what a Photoshop
/// document can hold.
pub const MAX_RESOLUTION: f32 = 65000.0;

/// A crop: the box in the canvas, turned by `angle` (degrees, clockwise),
/// then what is left resampled to `size`, at `resolution` pixels per inch.
#[derive(Clone, Copy, Debug)]
pub struct CropOrder {
    pub rect: PixelRect,
    pub angle: f32,
    pub delete: bool,
    pub size: Option<(u32, u32)>,
    pub resolution: f32,
    pub filter: FilterType,
}

/// A change of the whole document.
#[derive(Clone, Copy, Debug)]
pub enum WholeChange {
    /// Image Rotation, in degrees, clockwise.
    Rotate(f32),
    Crop(CropOrder),
    /// Flip Canvas, horizontal or vertical.
    Flip(bool),
}

/// The work of a [`WholeChange`], to run on any thread.
pub struct WholeJob {
    work: Document,
    change: WholeChange,
    cancel: Arc<AtomicBool>,
}

/// What a [`WholeJob`] made, for [`Document::put_whole`].
pub struct WholeDone {
    state: State,
    changed: bool,
    /// Where the change takes a canvas point (texts follow it).
    pub map: Affine,
    /// The box's size, resampled or not.
    pub size: (u32, u32),
    pub resampled: bool,
}

impl Document {
    /// The job of `change` on the current state; `cancel` set gives it up.
    pub fn whole_job(&self, change: WholeChange, cancel: Arc<AtomicBool>) -> WholeJob {
        let mut work = Document::new(RgbaImage::new(0, 0), None);
        work.restore(self.state());
        work.next_id = self.next_id;
        WholeJob {
            work,
            change,
            cancel,
        }
    }

    /// `done` in place, as one step named `name`. Returns whether anything
    /// changed. The state before is kept whole by that step, whatever the
    /// history budget: nothing in between counts against it.
    pub fn put_whole(&mut self, done: WholeDone, name: String) -> bool {
        if !done.changed {
            return false;
        }
        self.checkpoint(name);
        let mut state = done.state;
        state.id = next_rev();
        self.restore(state);
        // Outside the state: the work copy's drop doesn't reach it.
        self.ruler = None;
        true
    }

    /// Crop to `r`; with `delete`, the layers' pixels and masks beyond the
    /// new canvas go as well. One step.
    #[cfg(test)]
    pub fn crop_area(&mut self, r: PixelRect, delete: bool) {
        self.crop_area_shifted(r, delete);
    }

    /// [`Self::crop_area`]: how much further the canvas's origin went, the
    /// artboards being kept.
    fn crop_area_shifted(&mut self, r: PixelRect, delete: bool) -> (i64, i64) {
        let mark = self.history_mark();
        self.crop(r);
        let shift = self.crop_artboards();
        if delete && self.delete_outside_canvas() {
            self.join_since(mark);
        }
        shift
    }

    /// After a crop: the artboards the box touched are cut to it, and the
    /// others stay as they are; the canvas remains the box of them all.
    fn crop_artboards(&mut self) -> (i64, i64) {
        let canvas = self.full_rect();
        for layer in &mut self.layers {
            let Some(r) = layer.artboard_rect() else {
                continue;
            };
            if let (Some(k), Some(a)) = (intersect(r, canvas), &mut layer.artboard) {
                a.size = (k.w, k.h);
                layer.offset = (k.x, k.y);
            }
        }
        self.fit_artboards().map_or((0, 0), |b| (b.x, b.y))
    }

    /// Cuts the plain pixel layers and every mask to the canvas, as its own
    /// step. Returns whether anything went.
    fn delete_outside_canvas(&mut self) -> bool {
        self.delete_outside(self.full_rect())
    }

    /// Cuts the plain pixel layers and every mask to `area`, as its own
    /// step, keeping what still shows: a layer on an artboard keeps its
    /// board (it shows only there, and the board is cut on its own), and
    /// styled pixels keep how far their styles reach. Texts, shapes, smart
    /// objects and artboards are not pixels to cut, nor what a gradient
    /// overlay spans (its box would change): they stay whole. Returns
    /// whether anything went.
    fn delete_outside(&mut self, area: PixelRect) -> bool {
        let overhangs = |r: PixelRect, k: PixelRect| r.w > 0 && r.h > 0 && intersect(r, k) != Some(r);
        let cuttable: Vec<(usize, PixelRect)> = (0..self.layers.len())
            .filter_map(|i| {
                let l = &self.layers[i];
                let kept = self.kept_area(i, area)?;
                let cut = plain_pixels(l) && overhangs(l.rect(), kept)
                    || l.mask.as_ref().is_some_and(|m| overhangs(m.rect(l.offset), kept));
                cut.then_some((i, kept))
            })
            .collect();
        if cuttable.is_empty() {
            return false;
        }
        self.checkpoint(t!("menu-image-crop"));
        for (i, kept) in cuttable {
            let layer = &mut self.layers[i];
            let old = layer.offset;
            if plain_pixels(layer) && overhangs(layer.rect(), kept) {
                let r = layer.rect();
                let (image, offset) = match intersect(r, kept) {
                    Some(k) => (
                        image::imageops::crop_imm(
                            layer.image(),
                            (k.x - r.x) as u32,
                            (k.y - r.y) as u32,
                            k.w,
                            k.h,
                        )
                        .to_image(),
                        (k.x, k.y),
                    ),
                    None => (RgbaImage::new(0, 0), (0, 0)),
                };
                layer.replace_pixels(image);
                layer.reframe(offset);
            }
            layer.edit_mask(old, |m| m.cut_to(kept));
        }
        true
    }

    /// What of layer `i` cut to `area` still shows: `area`, or its board,
    /// grown by the reach of the styles drawn from it (its own and the
    /// groups' holding it). `None`: it must stay whole.
    fn kept_area(&self, i: usize, area: PixelRect) -> Option<PixelRect> {
        let mut reach = 0i64;
        let mut kept = area;
        let mut at = Some(i);
        while let Some(j) = at {
            let l = &self.layers[j];
            let fx = &l.effects;
            if fx.any() {
                // It spans the box of what it is drawn on.
                if fx.gradient_overlay.enabled {
                    return None;
                }
                let offset = |s: &crate::compose::styles::ShadowFx| match s.enabled {
                    true => s.dx.abs().max(s.dy.abs()),
                    false => 0.0,
                };
                let offset = offset(&fx.shadow).max(offset(&fx.inner_shadow));
                reach += fx.margin() as i64 + offset.min(1e6).ceil() as i64;
            }
            if j != i
                && let Some(board) = l.artboard_rect()
            {
                kept = board;
            }
            at = self.parent(j);
        }
        Some(PixelRect {
            x: kept.x - reach,
            y: kept.y - reach,
            w: kept.w + 2 * reach as u32,
            h: kept.h + 2 * reach as u32,
        })
    }

    /// The canvas turned by `degrees`: its map, once done.
    fn turn_for_whole(&mut self, degrees: f32, cancel: &AtomicBool) -> Option<Affine> {
        let old = self.canvas;
        self.rotate_canvas_unless(degrees, cancel).then(|| {
            let new = self.canvas;
            let (s, c) = vectors::sin_cos(degrees.rem_euclid(360.0));
            let (ox, oy) = (old.0 as f32 / 2.0, old.1 as f32 / 2.0);
            let (nx, ny) = (new.0 as f32 / 2.0, new.1 as f32 / 2.0);
            Affine([c, -s, nx - (c * ox - s * oy), s, c, ny - (s * ox + c * oy)])
        })
    }
}

const IDENTITY: Affine = Affine([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

/// `a`, then `b`.
fn then(Affine(a): Affine, Affine(b): Affine) -> Affine {
    Affine([
        b[0] * a[0] + b[1] * a[3],
        b[0] * a[1] + b[1] * a[4],
        b[0] * a[2] + b[1] * a[5] + b[2],
        b[3] * a[0] + b[4] * a[3],
        b[3] * a[1] + b[4] * a[4],
        b[3] * a[2] + b[4] * a[5] + b[5],
    ])
}

/// `r` around `center` before the canvas turned by `map`, in the turned
/// canvas: same size, around the new place of its center.
fn turned_box(r: PixelRect, center: (f32, f32), Affine(m): Affine) -> PixelRect {
    let x = m[0] * center.0 + m[1] * center.1 + m[2];
    let y = m[3] * center.0 + m[4] * center.1 + m[5];
    PixelRect {
        x: (x - r.w as f32 / 2.0).round() as i64,
        y: (y - r.h as f32 / 2.0).round() as i64,
        ..r
    }
}

impl WholeJob {
    /// Number of pixels it computes, about (to run small ones at once).
    pub fn size(&self) -> u64 {
        let turning = match self.change {
            WholeChange::Rotate(_) | WholeChange::Flip(_) => true,
            WholeChange::Crop(c) => c.angle != 0.0 || c.size.is_some() || c.delete,
        };
        if !turning {
            return 0;
        }
        let area = |w: u32, h: u32| u64::from(w) * u64::from(h);
        let d = &self.work;
        let layers = d.layers.iter().map(|l| {
            let mask = l.mask.as_ref().map_or(0, |m| {
                let (w, h) = m.image().dimensions();
                area(w, h)
            });
            area(l.image().width(), l.image().height()) + mask
        });
        let channels = d.channels.iter().map(|c| area(c.mask.width(), c.mask.height()));
        // Pattern tiles turn or mirror too.
        let tiles = d.layers.iter().map(|l| {
            let fill = match &l.kind {
                LayerKind::Fill(Fill::Pattern(f)) => Some(f),
                _ => None,
            };
            let fx = &l.effects.pattern_overlay;
            let overlay = fx.in_use().then_some(&fx.fill);
            fill.into_iter()
                .chain(overlay)
                .map(|f| area(f.pattern.image.width(), f.pattern.image.height()))
                .sum::<u64>()
        });
        layers.chain(channels).chain(tiles).sum()
    }

    /// A smart object it renders again runs smart filters (whose cost its
    /// size can't tell).
    pub fn has_filters(&self) -> bool {
        let renders = match self.change {
            WholeChange::Rotate(_) | WholeChange::Flip(_) => true,
            WholeChange::Crop(c) => c.angle != 0.0 || c.size.is_some(),
        };
        let filtered = |l: &Layer| l.smart.as_ref().is_some_and(|s| !s.filters.is_empty());
        renders && self.work.layers.iter().any(filtered)
    }

    /// The work; `None`: cancelled.
    pub fn run(self) -> Option<WholeDone> {
        let Self {
            mut work,
            change,
            cancel,
        } = self;
        let mut done = WholeDone {
            state: work.state(),
            changed: false,
            map: IDENTITY,
            size: work.canvas,
            resampled: false,
        };
        let mut meta_changed = false;
        match change {
            WholeChange::Rotate(degrees) => {
                if let Some(map) = work.turn_for_whole(degrees, &cancel) {
                    done.map = map;
                }
            }
            WholeChange::Flip(horizontal) => {
                let (w, h) = (work.canvas.0 as f32, work.canvas.1 as f32);
                work.flip_canvas_unless(horizontal, &cancel);
                done.map = Affine(match horizontal {
                    true => [-1.0, 0.0, w, 0.0, 1.0, 0.0],
                    false => [1.0, 0.0, 0.0, 0.0, -1.0, h],
                });
            }
            WholeChange::Crop(c) => {
                let center = (
                    c.rect.x as f32 + c.rect.w as f32 / 2.0,
                    c.rect.y as f32 + c.rect.h as f32 / 2.0,
                );
                let mut r = c.rect;
                let quarter = (c.angle / 90.0).fract() == 0.0;
                let upright_guides = work.guides.clone();
                if c.delete && c.angle != 0.0 {
                    // What the turned box can't reach goes first: only the
                    // rest is turned (a few pixels to spare for the filter).
                    let (w, h) = crate::rotate::rotated_size(r.w as f32, r.h as f32, c.angle);
                    let spare = 4.0;
                    let (x0, y0) = (center.0 - w / 2.0 - spare, center.1 - h / 2.0 - spare);
                    let reach = PixelRect {
                        x: x0.floor() as i64,
                        y: y0.floor() as i64,
                        w: (w + 2.0 * spare).ceil() as u32 + 1,
                        h: (h + 2.0 * spare).ceil() as u32 + 1,
                    };
                    work.delete_outside(reach);
                }
                if c.angle != 0.0
                    && let Some(map) = work.turn_for_whole(-c.angle, &cancel)
                {
                    done.map = map;
                    r = turned_box(r, center, map);
                }
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let (dx, dy) = work.crop_area_shifted(r, c.delete);
                let shift = (-r.x - dx, -r.y - dy);
                done.map = then(
                    done.map,
                    Affine([1.0, 0.0, shift.0 as f32, 0.0, 1.0, shift.1 as f32]),
                );
                if c.angle != 0.0 && !quarter {
                    // Guides can't turn: they keep their place on the box.
                    let to = (
                        r.w as f32 / 2.0 - dx as f32,
                        r.h as f32 / 2.0 - dy as f32,
                    );
                    work.guides = upright_guides;
                    guides::shift(&mut work.guides, to.0 - center.0, to.1 - center.1);
                }
                done.size = (r.w, r.h);
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                if let Some((w, h)) = c.size {
                    // The box takes that size; artboards outside it follow.
                    let (sx, sy) = (w as f64 / r.w as f64, h as f64 / r.h as f64);
                    let target = if work.canvas == (r.w, r.h) {
                        (w, h)
                    } else {
                        (
                            (work.canvas.0 as f64 * sx).round() as u32,
                            (work.canvas.1 as f64 * sy).round() as u32,
                        )
                    };
                    if super::check_size(target.0, target.1).is_ok()
                        && work.resize_image_unless(target.0, target.1, c.filter, &cancel)
                    {
                        done.map = then(
                            done.map,
                            Affine([sx as f32, 0.0, 0.0, 0.0, sy as f32, 0.0]),
                        );
                        done.resampled = true;
                        done.size = (w, h);
                    }
                }
                if c.resolution > 0.0 && c.resolution.is_finite() {
                    let density = f64::from(c.resolution.clamp(1.0, MAX_RESOLUTION));
                    let current = work.file_metadata.image.as_ref().and_then(|m| m.density);
                    if current != Some((density, density)) {
                        let meta = work.file_metadata.image.get_or_insert_default();
                        Arc::make_mut(meta).density = Some((density, density));
                        meta_changed = true;
                    }
                }
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        done.changed = work.can_undo() || meta_changed;
        done.state = work.state();
        Some(done)
    }
}

/// Pixels that can be cut: not what renders them.
fn plain_pixels(l: &Layer) -> bool {
    l.has_pixels() && l.text.is_none() && l.shape.is_none() && l.smart.is_none() && l.artboard.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

    fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    #[test]
    fn delete_cropped_pixels_cuts_the_layers_in_one_step() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 80, RED), None);
        doc.crop_area(rect(10, 20, 30, 40), true);
        assert_eq!((doc.width(), doc.height()), (30, 40));
        assert_eq!(doc.layers[0].rect(), rect(0, 0, 30, 40));
        assert!(doc.undo());
        assert_eq!((doc.width(), doc.height()), (100, 80));
        assert_eq!(doc.layers[0].rect(), rect(0, 0, 100, 80));
    }

    #[test]
    fn without_delete_the_pixels_stay() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 80, RED), None);
        doc.crop_area(rect(10, 20, 30, 40), false);
        assert_eq!(doc.layers[0].rect(), rect(-10, -20, 100, 80));
    }

    #[test]
    fn a_layer_entirely_outside_is_emptied_and_a_mask_is_cut() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 80, RED), None);
        doc.add_layer("far".into(), RgbaImage::from_pixel(5, 5, RED), (90, 70));
        doc.active = 0;
        let sel = crate::selection::Selection::from(rect(0, 0, 100, 80));
        assert!(doc.add_mask(crate::document::NewMask::RevealSelection, Some(&sel)));
        doc.crop_area(rect(0, 0, 20, 20), true);
        assert_eq!(doc.layers[1].image().width(), 0);
        let (layer, mask) = (&doc.layers[0], doc.layers[0].mask.as_ref().expect("a mask"));
        assert_eq!(mask.rect(layer.offset), rect(0, 0, 20, 20));
    }
}
