//! Edit › Transform through [`xform`] maps (skew, distort, perspective,
//! warp, flips): of the active layer (a group: of its layers, the masks of
//! its adjustment and fill layers included), or of its mask alone.

use std::sync::Arc;

use image::imageops::FilterType;
use image::{GrayImage, RgbaImage};

use super::{Document, Layer, Mask, PixelRect, Selection, intersect, smart};
use crate::xform::{self, Bounds, Filter, Grid, Source, Transform};

/// What a transform changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The active layer (a group: with its layers), with its linked mask.
    Layer,
    /// The active layer's mask alone.
    Mask,
}

fn bounds(r: PixelRect) -> Bounds {
    [r.x as f64, r.y as f64, r.right() as f64, r.bottom() as f64]
}

fn pixel_rect(g: Grid) -> PixelRect {
    PixelRect {
        x: g.x,
        y: g.y,
        w: g.w,
        h: g.h,
    }
}

/// The pixels covering what `area` becomes, within `limit` if given.
fn grid(t: &Transform, area: PixelRect, limit: Option<PixelRect>) -> Option<Grid> {
    let g = match t.bounds(bounds(area)) {
        Some(b) => Grid::covering(b, 1.0),
        None => {
            let l = limit?;
            Grid::covering(bounds(l), 1.0)
        }
    };
    let r = match limit {
        Some(l) => intersect(pixel_rect(g), l)?,
        None => pixel_rect(g),
    };
    Some(Grid {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        px: 1.0,
    })
}

/// `image` (a mask's, its top-left corner at `origin`, `outside` beyond
/// it) as `t` transforms it, cut to `limit`: the new image and its origin.
fn transform_mask(
    image: &GrayImage,
    origin: (i64, i64),
    outside: u8,
    t: &Transform,
    filter: Filter,
    limit: PixelRect,
) -> (GrayImage, (i64, i64)) {
    let area = PixelRect {
        x: origin.0,
        y: origin.1,
        w: image.width(),
        h: image.height(),
    };
    let Some(g) = grid(t, area, Some(limit)) else {
        return (GrayImage::new(0, 0), origin);
    };
    let src = Source {
        image,
        origin: [origin.0 as f64, origin.1 as f64],
        px: 1.0,
    };
    (
        xform::transform_gray(src, t, g, filter, outside),
        (g.x, g.y),
    )
}

/// What a transform computes for a layer: its new pixels and its mask's
/// new image, with their canvas positions.
pub struct Transformed {
    id: u64,
    pixels: Option<(RgbaImage, (i64, i64))>,
    mask: Option<(GrayImage, (i64, i64))>,
    /// A smart object rendered again from its contents.
    smart: Option<smart::SmartRender>,
}

/// A layer's images to transform (see [`Document::transform_job`]).
struct Sources {
    id: u64,
    /// Its pixels, at its offset, and the box of their visible part.
    pixels: Option<(Arc<RgbaImage>, (i64, i64), PixelRect)>,
    /// Its mask's image, origin and value outside.
    mask: Option<(GrayImage, (i64, i64), u8)>,
    /// A smart object's rendering through the transform.
    smart: Option<smart::RenderJob>,
}

/// The pixel work of a transform, to run on any thread.
pub struct TransformJob {
    t: Transform,
    filter: Filter,
    limit: PixelRect,
    layers: Vec<Sources>,
}

impl TransformJob {
    /// Number of pixels it computes, about (to run small ones at once).
    pub fn size(&self) -> u64 {
        let r = |l: &Sources| -> u64 {
            let p = l
                .pixels
                .as_ref()
                .map_or(0, |(_, _, c)| c.w as u64 * c.h as u64);
            let m = l.mask.as_ref().map_or(0, |(m, ..)| m.len() as u64);
            p + m + l.smart.as_ref().map_or(0, smart::RenderJob::size)
        };
        self.layers.iter().map(r).sum()
    }

    pub fn run(self) -> Vec<Transformed> {
        let Self {
            t,
            filter,
            limit,
            layers,
        } = self;
        layers
            .into_iter()
            .map(|l| Transformed {
                id: l.id,
                pixels: l.pixels.and_then(|(image, offset, c)| {
                    let g = grid(&t, c, None)?;
                    let region = if (c.x, c.y, c.w, c.h)
                        == (offset.0, offset.1, image.width(), image.height())
                    {
                        image
                    } else {
                        let (x, y) = ((c.x - offset.0) as u32, (c.y - offset.1) as u32);
                        Arc::new(image::imageops::crop_imm(&*image, x, y, c.w, c.h).to_image())
                    };
                    let src = Source {
                        image: &*region,
                        origin: [c.x as f64, c.y as f64],
                        px: 1.0,
                    };
                    Some((xform::transform_rgba(src, &t, g, filter), (g.x, g.y)))
                }),
                mask: l.mask.map(|(m, origin, outside)| {
                    transform_mask(&m, origin, outside, &t, filter, limit)
                }),
                smart: l.smart.map(smart::RenderJob::run),
            })
            .collect()
    }
}

/// Parts each segment of a shape is cut in before it is warped.
const WARP_CUTS: usize = 8;

/// The map of a vector shape following `t`, if it can (an affine map keeps
/// it a live shape; beyond a perspective's horizon, it can't).
fn shape_map(t: &Transform) -> Option<xform::Homography> {
    match t.map {
        xform::Map::Projective(h) => Some(h),
        xform::Map::Warp(_) => None,
    }
}

impl Document {
    /// Whether Edit › Transform changes the active layer's mask alone:
    /// the mask of an adjustment or fill layer, or a targeted mask that
    /// isn't linked to its layer.
    pub fn transforms_mask(&self) -> bool {
        let l = &self.layers[self.active];
        let Some(m) = &l.mask else { return false };
        match l.kind {
            super::LayerKind::Adjustment(_) | super::LayerKind::Fill(_) => true,
            _ => self.mask_target == Some(l.id) && !m.linked(),
        }
    }

    /// Box (canvas) of what layer `i`'s mask does: its pixels unlike its
    /// default.
    pub fn mask_bounds(&self, i: usize) -> Option<PixelRect> {
        let l = &self.layers[i];
        let m = l.mask.as_ref()?;
        let image = m.image();
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (x, y, p) in image.enumerate_pixels() {
            if p.0[0] != m.default {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
            }
        }
        let (ox, oy) = m.origin(l.offset);
        (x1 > x0).then(|| PixelRect {
            x: ox + x0 as i64,
            y: oy + y0 as i64,
            w: x1 - x0,
            h: y1 - y0,
        })
    }

    /// Box (canvas) of the pixels `t` makes of `layers`: to refuse a
    /// result too large before computing it. `None` if they have none, or
    /// if part of them goes beyond the horizon.
    pub fn transformed_bounds(&self, layers: &[usize], t: &Transform) -> Option<PixelRect> {
        layers
            .iter()
            .copied()
            .filter(|&i| self.layers[i].has_pixels())
            .filter_map(|i| self.layers[i].content_bounds())
            .map(|r| grid(t, r, None).map(pixel_rect))
            .reduce(|a, b| Some(a?.union(b?)))
            .flatten()
    }

    /// Whether layer `i` is a shape that follows `t` as a vector (a warp
    /// bends its path).
    fn follows_as_shape(&self, i: usize, t: &Transform) -> bool {
        let l = &self.layers[i];
        if l.shape.is_some() && matches!(t.map, xform::Map::Warp(_)) {
            return true;
        }
        let (Some(shape), Some(h)) = (&l.shape, shape_map(t)) else {
            return false;
        };
        let o = crate::vector::pt(l.offset.0 as f32, l.offset.1 as f32);
        h.is_affine() || shape.map_points(|p| vector_point(&h, p + o)).is_some()
    }

    /// The pixel work of `t` on the active layer's mask, or on `layers`
    /// (each on its own: a group's layers are to be listed) and their linked
    /// masks; [`Self::apply_transformed`] puts its result in place. Shapes
    /// follow as vectors there.
    pub fn transform_job(
        &self,
        target: Target,
        layers: &[usize],
        t: &Transform,
        filter: FilterType,
    ) -> TransformJob {
        // Masks are kept over the canvas and what the source becomes.
        let canvas = self.full_rect();
        let limit = t
            .bounds(t.src)
            .map(|b| pixel_rect(Grid::covering(b, 1.0)).union(canvas))
            .unwrap_or(canvas);
        let mask = |l: &Layer| {
            let m = l.mask.as_ref()?;
            let image = m.image();
            (image.width() > 0 && image.height() > 0)
                .then(|| (image.clone(), m.origin(l.offset), m.default))
        };
        let sources = match target {
            Target::Mask => {
                let l = &self.layers[self.active];
                vec![Sources {
                    id: l.id,
                    pixels: None,
                    mask: mask(l),
                    smart: None,
                }]
            }
            Target::Layer => layers
                .iter()
                .map(|&i| {
                    let l = &self.layers[i];
                    // A smart object is rendered again from its contents.
                    let smart = smart::transformed_job(l, t, self.shape_limit());
                    let pixels = (!self.follows_as_shape(i, t) && smart.is_none())
                        .then(|| l.content_bounds().filter(|_| l.has_pixels()))
                        .flatten()
                        .map(|c| (l.shared_image(), l.offset, c));
                    Sources {
                        id: l.id,
                        pixels,
                        mask: mask(l).filter(|_| l.mask.as_ref().is_some_and(Mask::linked)),
                        smart,
                    }
                })
                .collect(),
        };
        TransformJob {
            t: *t,
            filter: Filter::from(filter),
            limit,
            layers: sources,
        }
    }

    /// Puts the result of [`Self::transform_job`] in place, as the undo
    /// step `name`: texts become pixels, shapes follow as vectors, smart
    /// objects stay smart.
    pub fn apply_transformed(
        &mut self,
        target: Target,
        t: &Transform,
        mut done: Vec<Transformed>,
        name: String,
    ) {
        self.checkpoint(name);
        let limit = self.shape_limit();
        // By id: the layers may have moved since the job started (a layer
        // deleted meanwhile is skipped).
        let ids: Vec<u64> = done.iter().map(|d| d.id).collect();
        for id in ids {
            let Some(i) = self.layer_index(id) else {
                continue;
            };
            let shape = target == Target::Layer && self.follows_as_shape(i, t);
            let layer = &mut self.layers[i];
            let Some(result) = done.iter_mut().find(|d| d.id == layer.id) else {
                continue;
            };
            let old = layer.offset;
            if shape && matches!(t.map, xform::Map::Warp(_)) {
                // Cut in short segments, which then follow the warp closely.
                if let Some(s) = layer.shape.as_mut() {
                    for sub in &mut s.path.subpaths {
                        *sub = sub.subdivided(WARP_CUTS);
                    }
                }
                layer.map_shape_with(
                    old,
                    |p| {
                        let [x, y] = t.forward([p.x as f64, p.y as f64])?;
                        Some(crate::vector::pt(x as f32, y as f32))
                    },
                    limit,
                );
            } else if shape && let Some(h) = shape_map(t) {
                if h.is_affine() {
                    let [a, b, c, d, e, f, .., k] = h.0;
                    let m = [a, b, c, d, e, f].map(|v| (v / k) as f32);
                    layer.map_shape(old, &crate::mask_ops::Affine(m), limit);
                } else {
                    layer.map_shape_with(old, |p| vector_point(&h, p), limit);
                }
            }
            if let Some((image, at)) = &result.pixels {
                layer.replace_pixels(image.clone());
                layer.offset = *at;
            }
            if let Some(r) = result.smart.take() {
                layer.put_smart(r);
            }
            let new = layer.offset;
            if let Some(m) = &mut layer.mask {
                match &result.mask {
                    Some((image, origin)) => m.replace(image.clone(), *origin, new),
                    // Its default everywhere: nothing moves.
                    None => {
                        let origin = m.origin(old);
                        m.set_origin(origin, new);
                    }
                }
            }
        }
    }

    /// [`Self::transform_job`] run here, then applied.
    #[cfg(test)]
    pub fn transform_target(
        &mut self,
        target: Target,
        layers: &[usize],
        t: &Transform,
        filter: FilterType,
        name: String,
    ) {
        let done = self.transform_job(target, layers, t, filter).run();
        self.apply_transformed(target, t, done, name);
    }

    /// Names the last undo step `name` (History panel).
    pub fn rename_last_step(&mut self, name: String) {
        if let Some(step) = self.undo.back_mut() {
            step.name = name;
        }
    }

    /// The steps recorded since `mark` become one, under the last one's
    /// name.
    pub fn join_since(&mut self, mark: super::HistoryMark) {
        while self.undo.len() >= 2 && self.undo[self.undo.len() - 2].seq > mark.seq {
            self.join_last_step();
        }
    }

    /// The last undo step becomes part of the one before it, under its
    /// name: a transform of lifted pixels and their lift are one step.
    pub fn join_last_step(&mut self) {
        if self.undo.len() >= 2
            && let Some(last) = self.undo.pop_back()
            && let Some(prev) = self.undo.back_mut()
        {
            prev.name = last.name;
            self.step += 1;
        }
    }
}

/// Where `h` takes a point of a vector shape.
fn vector_point(h: &xform::Homography, p: crate::vector::Pt) -> Option<crate::vector::Pt> {
    let [x, y] = h.apply([p.x as f64, p.y as f64])?;
    Some(crate::vector::pt(x as f32, y as f32))
}

/// `sel` as `t` transforms the pixels it selects, within `canvas`; `None`
/// if nothing remains selected (an inverted selection isn't kept).
pub fn transformed_selection(
    sel: &Selection,
    t: &Transform,
    canvas: PixelRect,
) -> Option<Selection> {
    if sel.inverted {
        return None;
    }
    let g = grid(t, sel.rect, Some(canvas))?;
    let coverage = sel.to_mask(sel.rect);
    let src = Source {
        image: &coverage,
        origin: [sel.rect.x as f64, sel.rect.y as f64],
        px: 1.0,
    };
    let moved = xform::transform_gray(src, t, g, Filter::Bilinear, 0);
    Selection::from_mask(&moved, (g.x, g.y))?.within(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Layer, NewMask};
    use crate::pixel_ops::Adjustment;
    use crate::xform::Homography;
    use image::{GrayImage, Luma, Rgba, RgbaImage};

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

    /// 40 × 30 canvas; a 10 × 6 red layer at (4, 2), with a mask hiding
    /// its left column.
    fn doc() -> Document {
        let mut doc = Document::new(RgbaImage::new(40, 30), None);
        doc.add_layer("red".into(), RgbaImage::from_pixel(10, 6, RED), (4, 2));
        doc.add_mask(NewMask::RevealAll, None);
        let mask = GrayImage::from_fn(10, 6, |x, _| Luma([if x == 0 { 0 } else { 255 }]));
        let layer = doc.active_layer_mut();
        let offset = layer.offset;
        layer.mask.as_mut().unwrap().replace(mask, (4, 2), offset);
        doc.target_mask(false);
        doc
    }

    fn mask_at(l: &Layer, x: i64, y: i64) -> f32 {
        l.mask.as_ref().unwrap().value_at(x, y, l.offset)
    }

    fn flip(src: PixelRect) -> Transform {
        let b = bounds(src);
        let center = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        Transform::projective(b, Homography::scale(center, -1.0, 1.0))
    }

    /// `t` applied to the active layer (a group: its layers).
    fn transform(doc: &mut Document, t: &Transform, filter: FilterType) {
        let layers: Vec<usize> = doc.block(doc.active).collect();
        doc.transform_target(Target::Layer, &layers, t, filter, "Transform".into());
    }

    #[test]
    fn flipping_a_layer_flips_its_mask() {
        let mut doc = doc();
        let src = doc.element_bounds(doc.active).unwrap();
        transform(&mut doc, &flip(src), FilterType::Lanczos3);
        let l = doc.active_layer();
        assert_eq!(l.rect(), src);
        // The hidden column went to the right.
        assert_eq!(mask_at(l, 4, 3), 1.0);
        assert_eq!(mask_at(l, 13, 3), 0.0);
        assert_eq!(doc.flatten().get_pixel(13, 3).0[3], 0);
        assert_eq!(doc.flatten().get_pixel(4, 3).0, RED.0);
        // One undo step.
        assert!(doc.undo());
        assert_eq!(mask_at(doc.active_layer(), 4, 3), 0.0);
    }

    #[test]
    fn a_quarter_turn_of_a_layer_turns_its_mask() {
        let mut doc = doc();
        let src = doc.element_bounds(doc.active).unwrap();
        // (4, 2)–(14, 8) turned clockwise into (6, 0)–(12, 10).
        let quad = [[12.0, 0.0], [12.0, 10.0], [6.0, 10.0], [6.0, 0.0]];
        let h = Homography::rect_to_quad(bounds(src), quad).unwrap();
        let t = Transform::projective(bounds(src), h);
        transform(&mut doc, &t, FilterType::Triangle);
        let l = doc.active_layer();
        assert_eq!(
            l.rect(),
            PixelRect {
                x: 6,
                y: 0,
                w: 6,
                h: 10
            }
        );
        // The left column is now the top row.
        assert_eq!(mask_at(l, 8, 0), 0.0);
        assert_eq!(mask_at(l, 8, 1), 1.0);
        assert_eq!(l.pixel_at(8, 5), Some(RED.0));
    }

    #[test]
    fn a_group_transforms_its_adjustment_masks() {
        let mut doc = doc();
        doc.add_adjustment_layer(Adjustment::Invert, None);
        let adjust = doc.active;
        let mask = GrayImage::from_fn(40, 30, |x, _| Luma([if x < 5 { 255 } else { 0 }]));
        let l = doc.active_layer_mut();
        let offset = l.offset;
        let m = l.mask.as_mut().unwrap();
        m.replace(mask, (0, 0), offset);
        m.default = 0;
        assert!(doc.group_layers(&[adjust - 1, adjust]));
        let src = doc.element_bounds(doc.active).unwrap();
        transform(&mut doc, &flip(src), FilterType::Nearest);
        // The adjustment's mask turned around the group's center (x = 9).
        let adjustment = &doc.layers[adjust];
        assert_eq!(mask_at(adjustment, 1, 5), 0.0);
        assert_eq!(mask_at(adjustment, 15, 5), 1.0);
        assert_eq!(mask_at(&doc.layers[adjust - 1], 13, 3), 0.0);
    }

    #[test]
    fn a_free_transform_of_a_group_moves_its_adjustment_masks() {
        let mut doc = doc();
        doc.add_adjustment_layer(Adjustment::Invert, None);
        let adjust = doc.active;
        let mask = GrayImage::from_fn(10, 6, |x, _| Luma([if x < 5 { 255 } else { 0 }]));
        let l = doc.active_layer_mut();
        let offset = l.offset;
        let m = l.mask.as_mut().unwrap();
        m.replace(mask, (4, 2), offset);
        m.default = 0;
        assert!(doc.group_layers(&[adjust - 1, adjust]));
        let src = doc.element_bounds(doc.active).unwrap();
        let place = crate::document::Placement::new((24, 12), (10, 6));
        doc.transform_active(src, place, FilterType::Nearest, false);
        let adjustment = &doc.layers[adjust];
        assert_eq!(mask_at(adjustment, 5, 3), 0.0);
        assert_eq!(mask_at(adjustment, 25, 13), 1.0);
        assert_eq!(mask_at(adjustment, 31, 13), 0.0);
    }

    #[test]
    fn an_unlinked_targeted_mask_moves_alone() {
        let mut doc = doc();
        doc.target_mask(true);
        doc.set_mask_linked(false);
        assert!(doc.transforms_mask());
        let src = doc.mask_bounds(doc.active).unwrap();
        assert_eq!(
            src,
            PixelRect {
                x: 4,
                y: 2,
                w: 1,
                h: 6
            }
        );
        let t = Transform::projective(
            bounds(src),
            Homography::affine([1.0, 0.0, 5.0, 0.0, 1.0, 0.0]),
        );
        doc.transform_target(Target::Mask, &[], &t, FilterType::Triangle, "Move".into());
        let l = doc.active_layer();
        assert_eq!(l.rect().x, 4, "the pixels stay");
        assert_eq!(mask_at(l, 4, 3), 1.0);
        assert_eq!(mask_at(l, 9, 3), 0.0);
    }

    #[test]
    fn selections_follow_the_pixels() {
        let sel: Selection = PixelRect {
            x: 2,
            y: 2,
            w: 4,
            h: 4,
        }
        .into();
        let t = Transform::projective(
            bounds(sel.rect),
            Homography::affine([1.0, 0.0, 10.0, 0.0, 1.0, 0.0]),
        );
        let canvas = PixelRect {
            x: 0,
            y: 0,
            w: 40,
            h: 30,
        };
        let moved = transformed_selection(&sel, &t, canvas).unwrap();
        assert_eq!(
            moved.rect,
            PixelRect {
                x: 12,
                y: 2,
                w: 4,
                h: 4
            }
        );
    }
}
