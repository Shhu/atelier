//! Ruler guides of a document and the grid: positions in canvas pixels,
//! how guides follow canvas changes, ruler graduations.

use crate::snap::SnapTargets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// A line across the canvas at a constant y.
    Horizontal,
    /// A line down the canvas at a constant x.
    Vertical,
}

/// A guide: a line across the whole canvas, dragged from a ruler. It belongs
/// to the document (saved with it, part of its history).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub orientation: Orientation,
    /// Canvas pixels from the top (horizontal) or left (vertical) edge; may
    /// lie outside the canvas.
    pub position: f32,
}

impl Guide {
    pub fn new(orientation: Orientation, position: f32) -> Self {
        Self {
            orientation,
            position,
        }
    }
}

/// The canvas origin moved by (`dx`, `dy`): guides stay on the same pixels.
pub fn shift(guides: &mut [Guide], dx: f32, dy: f32) {
    for g in guides {
        g.position += match g.orientation {
            Orientation::Horizontal => dy,
            Orientation::Vertical => dx,
        };
    }
}

/// The image was resized by (`sx`, `sy`).
pub fn scale(guides: &mut [Guide], sx: f32, sy: f32) {
    for g in guides {
        g.position *= match g.orientation {
            Orientation::Horizontal => sy,
            Orientation::Vertical => sx,
        };
    }
}

/// The canvas (`width` × `height`) was flipped horizontally or vertically.
pub fn flip(guides: &mut [Guide], horizontal: bool, (width, height): (u32, u32)) {
    for g in guides {
        match (g.orientation, horizontal) {
            (Orientation::Vertical, true) => g.position = width as f32 - g.position,
            (Orientation::Horizontal, false) => g.position = height as f32 - g.position,
            _ => {}
        }
    }
}

/// The canvas `old` (width, height) turned by `degrees` clockwise into
/// `new`. Quarter turns carry guides exactly (a vertical guide becomes
/// horizontal); other angles keep them where they were relative to the
/// center.
pub fn rotate(guides: &mut [Guide], degrees: f32, old: (u32, u32), new: (u32, u32)) {
    let (w, h) = (old.0 as f32, old.1 as f32);
    for g in guides.iter_mut() {
        let p = g.position;
        *g = match (degrees.rem_euclid(360.0) as u32, g.orientation) {
            (90, Orientation::Vertical) => Guide::new(Orientation::Horizontal, p),
            (90, Orientation::Horizontal) => Guide::new(Orientation::Vertical, h - p),
            (180, Orientation::Vertical) => Guide::new(Orientation::Vertical, w - p),
            (180, Orientation::Horizontal) => Guide::new(Orientation::Horizontal, h - p),
            (270, Orientation::Vertical) => Guide::new(Orientation::Horizontal, w - p),
            (270, Orientation::Horizontal) => Guide::new(Orientation::Vertical, p),
            _ => *g,
        };
    }
    if degrees.rem_euclid(90.0) != 0.0 {
        let dx = (new.0 as f32 - w) / 2.0;
        let dy = (new.1 as f32 - h) / 2.0;
        shift(guides, dx, dy);
    }
}

/// Lines of a grid along an axis of `length` px: every `spacing` px
/// divided into `subdivisions`; `true` for the main lines.
pub fn grid_lines(length: f32, spacing: f32, subdivisions: u32) -> Vec<(f32, bool)> {
    let step = spacing / subdivisions.max(1) as f32;
    if step <= 0.0 {
        return Vec::new();
    }
    let count = (length / step).floor() as u32;
    (0..=count)
        .map(|i| (i as f32 * step, i % subdivisions.max(1) == 0))
        .collect()
}

/// Adds the guides as snap targets.
pub fn add_targets(targets: &mut SnapTargets, guides: &[Guide]) {
    for g in guides {
        match g.orientation {
            Orientation::Horizontal => targets.ys.push(g.position),
            Orientation::Vertical => targets.xs.push(g.position),
        }
    }
}

/// Adds the grid lines (subdivisions included) of a canvas as snap targets.
pub fn add_grid_targets(
    targets: &mut SnapTargets,
    (width, height): (u32, u32),
    spacing: f32,
    subdivisions: u32,
) {
    let lines = |length: u32| grid_lines(length as f32, spacing, subdivisions);
    targets.xs.extend(lines(width).into_iter().map(|(x, _)| x));
    targets.ys.extend(lines(height).into_iter().map(|(y, _)| y));
}

/// The value of `targets` closest to `value`, within `threshold`.
pub fn nearest(value: f32, targets: &[f32], threshold: f32) -> Option<f32> {
    targets
        .iter()
        .copied()
        .filter(|t| (t - value).abs() <= threshold)
        .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
}

/// The guide at canvas point (`x`, `y`), within `tolerance` canvas pixels
/// (the topmost one: the last added).
pub fn hit(guides: &[Guide], (x, y): (f32, f32), tolerance: f32) -> Option<usize> {
    guides.iter().rposition(|g| {
        let along = match g.orientation {
            Orientation::Horizontal => y,
            Orientation::Vertical => x,
        };
        (along - g.position).abs() <= tolerance
    })
}

/// Ruler graduation at `zoom` (screen points per canvas pixel): the step
/// between numbered ticks (canvas pixels) and how many intervals divide it.
pub fn ruler_step(zoom: f32) -> (f32, u32) {
    /// Room for a number.
    const MIN_LABEL_GAP: f32 = 56.0;
    /// Smallest gap between two ticks.
    const MIN_TICK_GAP: f32 = 4.0;
    let mut step = 1.0;
    for i in 0.. {
        let s = [1.0, 2.0, 5.0][i % 3] * 10f32.powi((i / 3) as i32);
        step = s;
        if s * zoom >= MIN_LABEL_GAP {
            break;
        }
    }
    let divisions = [10, 5, 4, 2]
        .into_iter()
        .find(|&n| {
            let minor = step / n as f32;
            minor >= 1.0 && minor.fract() == 0.0 && minor * zoom >= MIN_TICK_GAP
        })
        .unwrap_or(1);
    (step, divisions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::snap;
    use eframe::egui::{Rect, pos2, vec2};

    fn h(p: f32) -> Guide {
        Guide::new(Orientation::Horizontal, p)
    }

    fn v(p: f32) -> Guide {
        Guide::new(Orientation::Vertical, p)
    }

    #[test]
    fn guides_follow_canvas_changes() {
        let mut g = [v(10.0), h(20.0)];
        shift(&mut g, 5.0, -5.0);
        assert_eq!(g, [v(15.0), h(15.0)]);
        scale(&mut g, 2.0, 0.5);
        assert_eq!(g, [v(30.0), h(7.5)]);
        flip(&mut g, true, (100, 50));
        assert_eq!(g, [v(70.0), h(7.5)]);
        flip(&mut g, false, (100, 50));
        assert_eq!(g, [v(70.0), h(42.5)]);
    }

    #[test]
    fn quarter_turns_swap_orientations() {
        // 100 × 50 canvas; a point (10, 20) goes to (50 − 20, 10) clockwise.
        let start = [v(10.0), h(20.0)];
        let mut g = start;
        rotate(&mut g, 90.0, (100, 50), (50, 100));
        assert_eq!(g, [h(10.0), v(30.0)]);
        rotate(&mut g, 270.0, (50, 100), (100, 50));
        assert_eq!(g, start);
        rotate(&mut g, 180.0, (100, 50), (100, 50));
        assert_eq!(g, [v(90.0), h(30.0)]);
        // Any other angle: same place relative to the center.
        let mut g = start;
        rotate(&mut g, 30.0, (100, 50), (120, 80));
        assert_eq!(g, [v(20.0), h(35.0)]);
    }

    #[test]
    fn grid_has_main_lines_and_subdivisions() {
        let lines = grid_lines(250.0, 100.0, 4);
        assert_eq!(lines.len(), 11);
        assert_eq!(lines[0], (0.0, true));
        assert_eq!(lines[1], (25.0, false));
        assert_eq!(lines[4], (100.0, true));
        assert_eq!(lines[10], (250.0, false));
        assert!(grid_lines(100.0, 0.0, 4).is_empty());
    }

    #[test]
    fn elements_snap_to_guides_and_grid() {
        let mut targets = SnapTargets::default();
        add_targets(&mut targets, &[v(40.0), h(70.0)]);
        // 20 × 10 element whose left edge is at 42 and center at y 68.
        let r = Rect::from_min_size(pos2(42.0, 63.0), vec2(20.0, 10.0));
        let s = snap(r, &targets, 5.0);
        assert_eq!(s.offset, vec2(-2.0, 2.0));
        assert_eq!((s.guide_x, s.guide_y), (Some(40.0), Some(70.0)));

        let mut grid = SnapTargets::default();
        add_grid_targets(&mut grid, (200, 100), 100.0, 4);
        let r = Rect::from_min_size(pos2(27.0, 49.0), vec2(10.0, 10.0));
        let s = snap(r, &grid, 3.0);
        assert_eq!(s.offset, vec2(-2.0, 1.0));
    }

    #[test]
    fn nearest_target_and_hit_test() {
        assert_eq!(nearest(48.0, &[0.0, 50.0, 47.0], 3.0), Some(47.0));
        assert_eq!(nearest(48.0, &[0.0, 60.0], 3.0), None);
        let g = [v(10.0), h(10.0), v(12.0)];
        assert_eq!(hit(&g, (11.5, 50.0), 1.0), Some(2));
        assert_eq!(hit(&g, (50.0, 10.5), 1.0), Some(1));
        assert_eq!(hit(&g, (50.0, 50.0), 1.0), None);
    }

    #[test]
    fn ruler_numbers_keep_their_room() {
        // 100 %: a number every 100 px, ticks every 10.
        assert_eq!(ruler_step(1.0), (100.0, 10));
        // 25 %: every 500 px.
        assert_eq!(ruler_step(0.25), (500.0, 10));
        // 1600 %: every 5 px, a tick per pixel.
        assert_eq!(ruler_step(16.0), (5.0, 5));
        // 6400 %: every pixel, no ticks between.
        assert_eq!(ruler_step(64.0), (1.0, 1));
        for zoom in [0.02, 0.1, 0.33, 3.0, 7.0] {
            let (step, n) = ruler_step(zoom);
            assert!(step * zoom >= 56.0 && step * zoom < 56.0 * 2.5 + 1.0);
            assert!(step / n as f32 * zoom >= 4.0);
        }
    }
}
