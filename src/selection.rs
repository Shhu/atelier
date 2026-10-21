//! Pixel-accurate selection: a coverage (0..255) per canvas pixel, combinable
//! (add, subtract, intersect) like Photoshop, and the selections saved in
//! the document.

use std::sync::Arc;

use image::{GrayImage, Luma};

use crate::document::{Layer, PixelRect, intersect, layer_clip};
use crate::mask_ops::{self, Affine, Modify, Refine, par_collect, par_rows};
use crate::pixel_ops::{Clip, ClipShape};

/// Selection (canvas coordinates): a rectangle, possibly cut out by a mask,
/// or everything but that when it is inverted.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// Bounding box of the selected pixels (before inversion).
    pub rect: PixelRect,
    /// Coverage of each pixel of `rect` (0..255); `None`: full rectangle.
    pub mask: Option<Arc<GrayImage>>,
    pub inverted: bool,
}

impl From<PixelRect> for Selection {
    fn from(rect: PixelRect) -> Self {
        Self {
            rect,
            mask: None,
            inverted: false,
        }
    }
}

/// How a new selection combines with the previous one (Shift: add,
/// Alt: subtract, Shift+Alt: intersect).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl SelectMode {
    /// Mode forced by the held keys, otherwise `default`.
    pub fn from_modifiers(shift: bool, alt: bool, default: SelectMode) -> SelectMode {
        match (shift, alt) {
            (true, true) => SelectMode::Intersect,
            (true, false) => SelectMode::Add,
            (false, true) => SelectMode::Subtract,
            (false, false) => default,
        }
    }
}

impl Selection {
    /// Same selection (without comparing masks pixel by pixel).
    pub fn same(&self, other: &Selection) -> bool {
        self.rect == other.rect
            && self.inverted == other.inverted
            && match (&self.mask, &other.mask) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }

    /// Coverage of canvas pixel (x, y).
    pub fn coverage(&self, x: i64, y: i64) -> u8 {
        let r = self.rect;
        let inner = if x < r.x || y < r.y || x >= r.right() || y >= r.bottom() {
            0
        } else {
            match &self.mask {
                Some(m) => m.get_pixel((x - r.x) as u32, (y - r.y) as u32).0[0],
                None => 255,
            }
        };
        if self.inverted { 255 - inner } else { inner }
    }

    /// Is point (x, y) selected (at least half)?
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.coverage(x.floor() as i64, y.floor() as i64) >= 128
    }

    pub fn translate(self, dx: i64, dy: i64) -> Selection {
        Selection {
            rect: self.rect.translate(dx, dy),
            ..self
        }
    }

    /// The selection limited to the canvas; `None` if it selects nothing
    /// there.
    pub fn within(self, canvas: PixelRect) -> Option<Selection> {
        if self.inverted {
            return self.bounds(canvas).map(|_| self);
        }
        let inside = intersect(self.rect, canvas)?;
        match &self.mask {
            None => Some(inside.into()),
            Some(_) if inside == self.rect => Some(self),
            Some(_) => Self::from_mask(&self.to_mask(inside), (inside.x, inside.y)),
        }
    }

    /// Bounding box of the selected pixels of canvas `canvas`.
    pub fn bounds(&self, canvas: PixelRect) -> Option<PixelRect> {
        if self.inverted {
            let covers = self.mask.is_none()
                && self.rect.x <= canvas.x
                && self.rect.y <= canvas.y
                && self.rect.right() >= canvas.right()
                && self.rect.bottom() >= canvas.bottom();
            (!covers).then_some(canvas)
        } else {
            intersect(self.rect, canvas)
        }
    }

    /// Editable pixels of `layer` (layer coordinates), within `canvas`.
    pub fn clip(&self, layer: &Layer, canvas: PixelRect) -> Option<Clip> {
        let bounds = self.bounds(canvas)?;
        let shape = match &self.mask {
            Some(m) => Some(ClipShape::Mask(
                Arc::clone(m),
                (self.rect.x - layer.offset.0, self.rect.y - layer.offset.1),
            )),
            None if self.inverted => Some(ClipShape::Rect(layer_clip(layer, self.rect))),
            None => None,
        };
        Some(Clip {
            rect: layer_clip(layer, bounds),
            shape,
            inverted: self.inverted,
        })
    }

    /// Coverage over the area `area` (canvas coordinates).
    pub fn to_mask(&self, area: PixelRect) -> GrayImage {
        self.area_mask(area, None)
    }

    /// Coverage over `area`, which may reach beyond `canvas`: there, with
    /// `Some((canvas, true))`, nothing is selected; with `false`, the
    /// selection goes on as at the nearest canvas pixel (Photoshop's "Apply
    /// effect at canvas bounds" off).
    fn area_mask(&self, area: PixelRect, canvas: Option<(PixelRect, bool)>) -> GrayImage {
        let mut out = GrayImage::new(area.w, area.h);
        let row = area.w as usize;
        par_rows(out.as_mut(), row, |y0, band| {
            for (i, v) in band.iter_mut().enumerate() {
                let (mut x, mut y) = (area.x + (i % row) as i64, area.y + (y0 + i / row) as i64);
                if let Some((c, at_bounds)) = canvas
                    && !c.contains(x, y)
                {
                    if at_bounds {
                        continue;
                    }
                    x = x.clamp(c.x, c.right() - 1);
                    y = y.clamp(c.y, c.bottom() - 1);
                }
                *v = self.coverage(x, y);
            }
        });
        out
    }

    /// Runs `f` on the coverage around the selection, with a margin of
    /// `reach` pixels (see [`Self::area_mask`] for `at_bounds`); the result
    /// is kept within the canvas.
    fn processed(
        &self,
        reach: u32,
        canvas: PixelRect,
        at_bounds: bool,
        f: impl FnOnce(&mut GrayImage),
    ) -> Option<Selection> {
        let inner = if self.inverted {
            canvas
        } else {
            intersect(self.rect, canvas)?
        };
        let m = reach as i64;
        let area = PixelRect {
            x: inner.x - m,
            y: inner.y - m,
            w: inner.w + 2 * reach,
            h: inner.h + 2 * reach,
        };
        let mut mask = self.area_mask(area, Some((canvas, at_bounds)));
        f(&mut mask);
        let keep = intersect(area, canvas)?;
        let (dx, dy) = ((keep.x - area.x) as u32, (keep.y - area.y) as u32);
        let kept = image::imageops::crop_imm(&mask, dx, dy, keep.w, keep.h).to_image();
        Self::from_mask(&kept, (keep.x, keep.y))
    }

    /// Select › Modify (Border, Smooth, Expand, Contract, Feather).
    pub fn modified(&self, op: Modify, canvas: PixelRect, at_bounds: bool) -> Option<Selection> {
        self.processed(op.reach(), canvas, at_bounds, |m| mask_ops::modify(m, op))
    }

    /// The global refinements of Select and Mask.
    pub fn refined(&self, refine: &Refine, canvas: PixelRect) -> Option<Selection> {
        self.processed(refine.reach(), canvas, false, |m| {
            mask_ops::refine(m, refine, 1.0)
        })
    }

    /// Transform Selection: the selection moved by `map` (canvas point →
    /// canvas point). A rectangle scaled or moved stays a rectangle.
    pub fn transformed(&self, map: Affine, canvas: PixelRect) -> Option<Selection> {
        let r = self.rect;
        let [a, b, c, d, e, f] = map.0;
        if self.mask.is_none() && b == 0.0 && d == 0.0 && a > 0.0 && e > 0.0 {
            let (x0, y0) = (a * r.x as f32 + c, e * r.y as f32 + f);
            let (x1, y1) = (a * r.right() as f32 + c, e * r.bottom() as f32 + f);
            let (x0, y0) = (x0.round() as i64, y0.round() as i64);
            let (x1, y1) = (x1.round() as i64, y1.round() as i64);
            let moved = PixelRect {
                x: x0,
                y: y0,
                w: (x1 - x0).max(1) as u32,
                h: (y1 - y0).max(1) as u32,
            };
            return Selection {
                inverted: self.inverted,
                ..moved.into()
            }
            .within(canvas);
        }
        let inverse = map.inverse()?;
        let corners = [
            (r.x, r.y),
            (r.right(), r.y),
            (r.x, r.bottom()),
            (r.right(), r.bottom()),
        ]
        .map(|(x, y)| map.apply(x as f32, y as f32));
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in corners {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let (x0, y0) = (x0.floor() as i64, y0.floor() as i64);
        let bbox = PixelRect {
            x: x0,
            y: y0,
            w: (x1.ceil() as i64 - x0).max(1) as u32,
            h: (y1.ceil() as i64 - y0).max(1) as u32,
        };
        let moved = intersect(bbox, canvas).and_then(|area| {
            let full;
            let src = match &self.mask {
                Some(m) => m.as_ref(),
                None => {
                    full = GrayImage::from_pixel(r.w, r.h, Luma([255]));
                    &full
                }
            };
            let mask = mask_ops::resample(src, (r.x, r.y), area, inverse);
            Self::from_mask(&mask, (area.x, area.y))
        });
        match (moved, self.inverted) {
            (moved, false) => moved,
            (Some(s), true) => Selection {
                inverted: true,
                ..s
            }
            .within(canvas),
            (None, true) => Some(canvas.into()),
        }
    }

    /// Selection from a coverage placed at `origin`, reduced to the bounding
    /// box of its non-zero pixels; `None` if it is empty.
    pub fn from_mask(mask: &GrayImage, origin: (i64, i64)) -> Option<Selection> {
        // Box of the non-zero pixels, by bands of rows in parallel.
        let row = mask.width() as usize;
        let raw = mask.as_raw();
        let boxes = par_collect(mask.height() as usize, |rows| {
            let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
            for y in rows {
                let line = &raw[y * row..(y + 1) * row];
                if let (Some(first), Some(last)) = (
                    line.iter().position(|&v| v > 0),
                    line.iter().rposition(|&v| v > 0),
                ) {
                    (x0, y0, x1, y1) = (x0.min(first), y0.min(y), x1.max(last), y1.max(y));
                }
            }
            vec![(x0, y0, x1, y1)]
        });
        let (x0, y0, x1, y1) = boxes.into_iter().fold(
            (usize::MAX, usize::MAX, 0, 0),
            |(a, b, c, d), (x0, y0, x1, y1)| (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        );
        if x0 > x1 {
            return None;
        }
        let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
        let mut cropped = GrayImage::new(bw as u32, bh as u32);
        par_rows(cropped.as_mut(), bw, |first, band| {
            for (i, line) in band.chunks_exact_mut(bw).enumerate() {
                let start = (y0 + first + i) * row + x0;
                line.copy_from_slice(&raw[start..start + bw]);
            }
        });
        let full = cropped.as_raw().iter().all(|&v| v == 255);
        let (x0, y0, bw, bh) = (x0 as u32, y0 as u32, bw as u32, bh as u32);
        Some(Selection {
            rect: PixelRect {
                x: origin.0 + x0 as i64,
                y: origin.1 + y0 as i64,
                w: bw,
                h: bh,
            },
            mask: (!full).then(|| Arc::new(cropped)),
            inverted: false,
        })
    }

    /// Combines `new` with the current selection according to `mode`.
    pub fn combine(
        current: Option<&Selection>,
        new: Selection,
        mode: SelectMode,
        canvas: PixelRect,
    ) -> Option<Selection> {
        let current = match (current, mode) {
            (_, SelectMode::Replace) | (None, SelectMode::Add) => return new.within(canvas),
            (None, _) => return None,
            (Some(c), _) => c,
        };
        // Only where either one can select: their boxes, unless one is
        // inverted (it then reaches the whole canvas).
        let area = if current.inverted || new.inverted {
            canvas
        } else {
            let (a, b) = (current.rect, new.rect);
            let area = match mode {
                SelectMode::Add | SelectMode::Replace => Some(a.union(b)),
                SelectMode::Subtract => Some(a),
                SelectMode::Intersect => intersect(a, b),
            };
            intersect(area?, canvas)?
        };
        let (mut mixed, b) = (current.to_mask(area), new.to_mask(area));
        let (row, b) = (area.w as usize, b.as_raw());
        par_rows(mixed.as_mut(), row, |y0, band| {
            for (i, a) in band.iter_mut().enumerate() {
                let b = b[y0 * row + i];
                *a = match mode {
                    SelectMode::Add | SelectMode::Replace => (*a).max(b),
                    // What the new one covers comes off: the same soft-edged
                    // area taken away leaves nothing, not a faint ring.
                    SelectMode::Subtract => a.saturating_sub(b),
                    SelectMode::Intersect => (*a).min(b),
                };
            }
        });
        Self::from_mask(&mixed, (area.x, area.y))
    }

    /// Closed polygon (canvas coordinates), with anti-aliased edges or each
    /// pixel in or out.
    pub fn from_polygon(
        points: &[(f32, f32)],
        canvas: PixelRect,
        antialias: bool,
    ) -> Option<Selection> {
        if points.len() < 3 {
            return None;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in points {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let (x0, y0) = (x0.floor() as i64, y0.floor() as i64);
        let bbox = PixelRect {
            x: x0,
            y: y0,
            w: (x1.ceil() as i64 - x0 + 1) as u32,
            h: (y1.ceil() as i64 - y0 + 1) as u32,
        };
        let area = intersect(bbox, canvas)?;
        let mut mask = fill_polygon(points, area.w, area.h, (area.x, area.y));
        if !antialias {
            for p in mask.pixels_mut() {
                p.0[0] = if p.0[0] >= 128 { 255 } else { 0 };
            }
        }
        Self::from_mask(&mask, (area.x, area.y))
    }

    /// Elliptical Marquee: the ellipse inscribed in `rect`, within the
    /// canvas.
    pub fn ellipse(rect: PixelRect, canvas: PixelRect, antialias: bool) -> Option<Selection> {
        let area = intersect(rect, canvas)?;
        let window = area.translate(-rect.x, -rect.y);
        let mask = mask_ops::ellipse((rect.w, rect.h), window, antialias);
        Self::from_mask(&mask, (area.x, area.y))
    }

    /// Outline (canvas coordinates): segments between selected and unselected
    /// pixels, grouped into horizontal and vertical lines.
    pub fn outline(&self, canvas: PixelRect) -> Vec<[(i64, i64); 2]> {
        if self.mask.is_none() {
            let rect_edges = |r: PixelRect| {
                let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
                [
                    [(x0, y0), (x1, y0)],
                    [(x1, y0), (x1, y1)],
                    [(x1, y1), (x0, y1)],
                    [(x0, y1), (x0, y0)],
                ]
            };
            let mut edges = rect_edges(self.rect).to_vec();
            if self.inverted {
                edges.extend(rect_edges(canvas));
            }
            return edges;
        }
        // Area examined: the selection, plus a one-pixel border; what is
        // selected there first, in parallel.
        let area = if self.inverted { canvas } else { self.rect };
        let border = PixelRect {
            x: area.x - 1,
            y: area.y - 1,
            w: area.w + 2,
            h: area.h + 2,
        };
        let coverage = self.area_mask(border, Some((canvas, true)));
        let (row, raw) = (border.w as usize, coverage.as_raw());
        let inside =
            |x: i64, y: i64| raw[(y - border.y) as usize * row + (x - border.x) as usize] >= 128;
        // Horizontal edges: between (x, y-1) and (x, y), by bands of rows.
        let mut edges = par_collect(area.h as usize + 1, |rows| {
            let mut edges = Vec::new();
            for y in rows.map(|i| area.y + i as i64) {
                let mut run: Option<i64> = None;
                for x in area.x..=area.right() {
                    let edge = x < area.right() && inside(x, y - 1) != inside(x, y);
                    match (edge, run) {
                        (true, None) => run = Some(x),
                        (false, Some(start)) => {
                            edges.push([(start, y), (x, y)]);
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
            edges
        });
        // Vertical edges: between (x-1, y) and (x, y), by bands of columns.
        edges.extend(par_collect(area.w as usize + 1, |columns| {
            let mut edges = Vec::new();
            for x in columns.map(|i| area.x + i as i64) {
                let mut run: Option<i64> = None;
                for y in area.y..=area.bottom() {
                    let edge = y < area.bottom() && inside(x - 1, y) != inside(x, y);
                    match (edge, run) {
                        (true, None) => run = Some(y),
                        (false, Some(start)) => {
                            edges.push([(x, start), (x, y)]);
                            run = None;
                        }
                        _ => {}
                    }
                }
            }
            edges
        }));
        edges
    }
}

/// A selection saved in the document (Select › Save Selection), like
/// Photoshop's alpha channels: a coverage the size of the canvas.
#[derive(Clone, Debug)]
pub struct Channel {
    pub name: String,
    pub mask: Arc<GrayImage>,
}

impl Channel {
    /// After a change of the canvas, each channel's mask through `f`.
    pub fn map_all(channels: &mut [Channel], f: impl Fn(&GrayImage) -> GrayImage) {
        for c in channels {
            c.mask = Arc::new(f(&c.mask));
        }
    }
}

/// Fills a polygon (even-odd rule) on a `w` × `h` image whose corner is at
/// `origin`; 4 sub-scanlines per pixel and exact horizontal coverage to
/// anti-alias the edges.
pub fn fill_polygon(points: &[(f32, f32)], w: u32, h: u32, origin: (i64, i64)) -> GrayImage {
    const SUB: usize = 4;
    let mut mask = GrayImage::new(w, h);
    let mut row = vec![0.0f32; w as usize + 1];
    let mut crossings = Vec::new();
    let n = points.len();
    for y in 0..h {
        row.iter_mut().for_each(|v| *v = 0.0);
        for s in 0..SUB {
            let sy = origin.1 as f32 + y as f32 + (s as f32 + 0.5) / SUB as f32;
            crossings.clear();
            for i in 0..n {
                let (ax, ay) = points[i];
                let (bx, by) = points[(i + 1) % n];
                if (ay <= sy) != (by <= sy) {
                    crossings.push(ax + (sy - ay) / (by - ay) * (bx - ax) - origin.0 as f32);
                }
            }
            crossings.sort_by(f32::total_cmp);
            for pair in crossings.chunks_exact(2) {
                let (a, b) = (pair[0].clamp(0.0, w as f32), pair[1].clamp(0.0, w as f32));
                if b <= a {
                    continue;
                }
                // Exact coverage of segment [a, b] on each pixel.
                let (ia, ib) = (a.floor() as usize, b.floor() as usize);
                if ia == ib {
                    row[ia] += b - a;
                } else {
                    row[ia] += ia as f32 + 1.0 - a;
                    for v in &mut row[ia + 1..ib] {
                        *v += 1.0;
                    }
                    row[ib] += b - ib as f32;
                }
            }
        }
        for x in 0..w {
            let cov = (row[x as usize] / SUB as f32).clamp(0.0, 1.0);
            mask.put_pixel(x, y, Luma([(cov * 255.0).round() as u8]));
        }
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    #[test]
    fn combine_modes() {
        let canvas = rect(0, 0, 20, 10);
        let a: Selection = rect(0, 0, 10, 10).into();
        let b: Selection = rect(5, 0, 10, 10).into();
        let add = Selection::combine(Some(&a), b.clone(), SelectMode::Add, canvas).unwrap();
        assert_eq!(add.rect, rect(0, 0, 15, 10));
        assert!(add.mask.is_none());
        let sub = Selection::combine(Some(&a), b.clone(), SelectMode::Subtract, canvas).unwrap();
        assert_eq!(sub.rect, rect(0, 0, 5, 10));
        let inter = Selection::combine(Some(&a), b.clone(), SelectMode::Intersect, canvas).unwrap();
        assert_eq!(inter.rect, rect(5, 0, 5, 10));
        // Disjoint add: mask with a gap.
        let c: Selection = rect(15, 0, 5, 5).into();
        let two = Selection::combine(Some(&a), c, SelectMode::Add, canvas).unwrap();
        assert!(two.mask.is_some());
        assert!(two.contains(16.0, 1.0) && !two.contains(12.0, 1.0) && !two.contains(16.0, 7.0));
        assert!(Selection::combine(Some(&a), a.clone(), SelectMode::Subtract, canvas).is_none());
    }

    /// Combining works on the selections' boxes only, with the same result
    /// as over the whole canvas.
    #[test]
    fn combine_matches_pixel_by_pixel_modes() {
        let canvas = rect(0, 0, 300, 200);
        let soft = GrayImage::from_fn(60, 40, |x, y| Luma([((x + y) * 3).min(255) as u8]));
        let soft = Selection::from_mask(&soft, (100, 100)).unwrap();
        let small: Selection = rect(140, 120, 50, 30).into();
        for mode in [SelectMode::Add, SelectMode::Subtract, SelectMode::Intersect] {
            let got = Selection::combine(Some(&soft), small.clone(), mode, canvas);
            let points = (0..300)
                .step_by(3)
                .flat_map(|x| (0..200).map(move |y| (x, y)));
            for (x, y) in points {
                let (a, b) = (soft.coverage(x, y), small.coverage(x, y));
                let want = match mode {
                    SelectMode::Subtract => a.saturating_sub(b),
                    SelectMode::Intersect => a.min(b),
                    _ => a.max(b),
                };
                assert_eq!(got.as_ref().map_or(0, |s| s.coverage(x, y)), want);
            }
        }
        let apart: Selection = rect(250, 0, 10, 10).into();
        assert!(Selection::combine(Some(&soft), apart, SelectMode::Intersect, canvas).is_none());
        // Inverted, it reaches the whole canvas.
        let inverted = Selection {
            inverted: true,
            ..soft
        };
        let add = Selection::combine(Some(&inverted), small, SelectMode::Add, canvas).unwrap();
        assert_eq!((add.coverage(0, 0), add.coverage(150, 130)), (255, 255));
    }

    #[test]
    fn polygon_is_filled_with_smooth_edges() {
        let canvas = rect(0, 0, 20, 20);
        let tri = [(0.0, 0.0), (20.0, 0.0), (0.0, 20.0)];
        let sel = Selection::from_polygon(&tri, canvas, true).unwrap();
        assert!(sel.contains(2.0, 2.0) && !sel.contains(18.0, 18.0));
        // Pixel cut by the diagonal: half covered.
        let c = sel.coverage(9, 10);
        assert!((100..=160).contains(&c), "{c}");
        let square = [(2.0, 2.0), (6.0, 2.0), (6.0, 6.0), (2.0, 6.0)];
        let sel = Selection::from_polygon(&square, canvas, true).unwrap();
        assert_eq!(sel.rect, rect(2, 2, 4, 4));
        assert!(sel.mask.is_none());
    }

    /// Near the canvas edge, Expand and Contract treat what lies beyond as
    /// selected, unless the effect applies at the canvas bounds.
    #[test]
    fn modify_at_canvas_bounds() {
        let canvas = rect(0, 0, 100, 100);
        let sel: Selection = rect(0, 20, 50, 50).into();
        let grown = sel.modified(Modify::Expand(5.0), canvas, false).unwrap();
        assert_eq!(grown.rect, rect(0, 15, 55, 60));
        let shrunk = sel.modified(Modify::Contract(5.0), canvas, false).unwrap();
        assert_eq!(
            (shrunk.coverage(0, 40), shrunk.coverage(44, 40)),
            (255, 255)
        );
        assert_eq!(shrunk.coverage(45, 40), 0);
        let at_bounds = sel.modified(Modify::Contract(5.0), canvas, true).unwrap();
        assert_eq!(
            (at_bounds.coverage(4, 40), at_bounds.coverage(5, 40)),
            (0, 255)
        );
        // Inverted: the hole shrinks when the selection expands.
        let inverted = Selection {
            inverted: true,
            ..Selection::from(rect(40, 40, 20, 20))
        };
        let grown = inverted
            .modified(Modify::Expand(5.0), canvas, false)
            .unwrap();
        assert_eq!((grown.coverage(40, 50), grown.coverage(44, 50)), (255, 255));
        assert_eq!((grown.coverage(45, 50), grown.coverage(0, 0)), (0, 255));
        assert!(sel.modified(Modify::Contract(30.0), canvas, true).is_none());
    }

    #[test]
    fn transform_moves_scales_and_turns() {
        let canvas = rect(0, 0, 200, 200);
        let sel: Selection = rect(10, 10, 20, 10).into();
        // Twice as wide, moved: still a plain rectangle.
        let wide = sel.transformed(Affine([2.0, 0.0, 5.0, 0.0, 1.0, 3.0]), canvas);
        assert_eq!(wide, Some(rect(25, 13, 40, 10).into()));
        // A quarter turn around the rectangle's center (20, 15).
        let turn = Affine([0.0, -1.0, 35.0, 1.0, 0.0, -5.0]);
        let turned = sel.transformed(turn, canvas).unwrap();
        assert_eq!(turned.rect, rect(15, 5, 10, 20));
        assert!(turned.contains(20.0, 10.0) && !turned.contains(12.0, 15.0));
        // Inverted: what is selected outside follows.
        let inverted = Selection {
            inverted: true,
            ..sel
        };
        let turned = inverted.transformed(turn, canvas).unwrap();
        assert!(turned.inverted && turned.contains(12.0, 15.0) && !turned.contains(20.0, 10.0));
    }

    #[test]
    fn ellipse_and_polygon_stay_within_the_canvas() {
        let canvas = rect(0, 0, 50, 50);
        let e = Selection::ellipse(rect(-50, 0, 100, 50), canvas, true).unwrap();
        assert_eq!((e.rect.x, e.rect.right()), (0, 50));
        assert_eq!(e.coverage(0, 25), 255);
        assert_eq!(e.coverage(49, 0), 0);
        let hard = Selection::from_polygon(&[(0.0, 0.0), (30.0, 0.0), (0.0, 31.0)], canvas, false);
        let hard = hard.unwrap();
        assert!(hard.to_mask(hard.rect).pixels().all(|p| p.0[0] % 255 == 0));
        let off = [(100.0, 100.0), (120.0, 100.0), (100.0, 120.0)];
        assert!(Selection::from_polygon(&off, canvas, true).is_none());
    }

    /// The outline's segments add up to every boundary between a selected
    /// and an unselected pixel (the canvas edge included), inverted too.
    #[test]
    fn outline_covers_every_boundary() {
        let canvas = rect(0, 0, 40, 30);
        let noisy = GrayImage::from_fn(37, 23, |x, y| {
            Luma([if (x * 7 + y * 13) % 5 < 2 || x < 3 {
                200
            } else {
                0
            }])
        });
        let sel = Selection::from_mask(&noisy, (3, 7)).unwrap();
        for sel in [
            sel.clone(),
            Selection {
                inverted: true,
                ..sel
            },
        ] {
            let inside = |x: i64, y: i64| canvas.contains(x, y) && sel.contains(x as f32, y as f32);
            let mut expected = 0;
            for y in -1..=30 {
                for x in -1..=40 {
                    expected += i64::from(inside(x, y) != inside(x + 1, y));
                    expected += i64::from(inside(x, y) != inside(x, y + 1));
                }
            }
            let length: i64 = sel
                .outline(canvas)
                .iter()
                .map(|[a, b]| (b.0 - a.0).abs() + (b.1 - a.1).abs())
                .sum();
            assert_eq!(length, expected, "inverted: {}", sel.inverted);
        }
    }

    #[test]
    fn outline_follows_mask() {
        let canvas = rect(0, 0, 10, 10);
        let a: Selection = rect(0, 0, 2, 2).into();
        let sel =
            Selection::combine(Some(&a), rect(4, 0, 2, 2).into(), SelectMode::Add, canvas).unwrap();
        // Two squares: 2 × 4 sides.
        assert_eq!(sel.outline(canvas).len(), 8);
        let within = sel.clone().translate(-1, 0).within(canvas).unwrap();
        assert_eq!(within.rect, rect(0, 0, 5, 2));
    }
}
