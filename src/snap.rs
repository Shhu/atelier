//! Smart guides: align a moved element with snap targets.

use eframe::egui::{Rect, Vec2, vec2};

/// Snap targets, in canvas coordinates.
#[derive(Clone, Debug, Default)]
pub struct SnapTargets {
    /// X coordinates of the vertical targets.
    pub xs: Vec<f32>,
    /// Y coordinates of the horizontal targets.
    pub ys: Vec<f32>,
}

impl SnapTargets {
    /// Adds a rectangle's edges and center.
    pub fn add_rect(&mut self, r: Rect) {
        self.xs.extend([r.left(), r.center().x, r.right()]);
        self.ys.extend([r.top(), r.center().y, r.bottom()]);
    }
}

/// Result of snapping: offset to apply and guides to show.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Snap {
    pub offset: Vec2,
    /// Vertical target reached (x coordinate), if any.
    pub guide_x: Option<f32>,
    /// Horizontal target reached (y coordinate), if any.
    pub guide_y: Option<f32>,
}

/// Finds, on each axis, the target nearest to an edge or the center
/// of `rect`, within `threshold`.
pub fn snap(rect: Rect, targets: &SnapTargets, threshold: f32) -> Snap {
    let (dx, guide_x) = nearest(
        [rect.left(), rect.center().x, rect.right()],
        &targets.xs,
        threshold,
    );
    let (dy, guide_y) = nearest(
        [rect.top(), rect.center().y, rect.bottom()],
        &targets.ys,
        threshold,
    );
    Snap {
        offset: vec2(dx, dy),
        guide_x,
        guide_y,
    }
}

fn nearest(points: [f32; 3], targets: &[f32], threshold: f32) -> (f32, Option<f32>) {
    let mut best: Option<(f32, f32)> = None;
    for &t in targets {
        for p in points {
            let d = t - p;
            if d.abs() <= threshold && best.is_none_or(|(bd, _)| d.abs() < bd.abs()) {
                best = Some((d, t));
            }
        }
    }
    best.map_or((0.0, None), |(d, t)| (d, Some(t)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    fn canvas() -> SnapTargets {
        let mut t = SnapTargets::default();
        t.add_rect(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 80.0)));
        t
    }

    #[test]
    fn centers_snap_to_canvas_center() {
        // 20 × 10 element centered at (52, 38): frame center at (50, 40).
        let r = Rect::from_center_size(pos2(52.0, 38.0), vec2(20.0, 10.0));
        let s = snap(r, &canvas(), 5.0);
        assert_eq!(s.offset, vec2(-2.0, 2.0));
        assert_eq!((s.guide_x, s.guide_y), (Some(50.0), Some(40.0)));
    }

    #[test]
    fn edges_snap_to_canvas_edges() {
        let r = Rect::from_min_size(pos2(3.0, 30.0), vec2(20.0, 10.0));
        let s = snap(r, &canvas(), 5.0);
        assert_eq!(s.offset.x, -3.0);
        assert_eq!(s.guide_x, Some(0.0));
    }

    #[test]
    fn nothing_snaps_beyond_threshold() {
        let r = Rect::from_center_size(pos2(30.0, 20.0), vec2(10.0, 10.0));
        assert_eq!(snap(r, &canvas(), 3.0), Snap::default());
    }

    #[test]
    fn closest_target_wins() {
        let mut t = canvas();
        t.add_rect(Rect::from_min_max(pos2(48.5, 0.0), pos2(49.0, 1.0)));
        let r = Rect::from_center_size(pos2(48.0, 20.0), vec2(10.0, 10.0));
        // 48 → 48.5 (left edge of the second rect) rather than 50.
        assert_eq!(snap(r, &t, 5.0).guide_x, Some(48.5));
    }
}
