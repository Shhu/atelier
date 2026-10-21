//! Mouse-manipulated rectangle: 8 handles + move.

use eframe::egui::{CursorIcon, Pos2, Rect, Vec2, pos2, vec2};

/// Minimum size (image pixels) of a resized rectangle.
const MIN_SIZE: f32 = 1.0;

/// Distance (in screen pixels) at which a handle or an edge is grabbed.
pub const GRAB_RADIUS: f32 = 8.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NW,
    NE,
    SW,
    SE,
}

impl Handle {
    /// Cursor of the handle, oriented by the element's rotation (degrees).
    pub fn cursor(self, angle: f32) -> CursorIcon {
        let base = match self {
            Handle::Move => return CursorIcon::Move,
            Handle::E => 0.0,
            Handle::SE => 45.0,
            Handle::S => 90.0,
            Handle::SW => 135.0,
            Handle::W => 180.0,
            Handle::NW => 225.0,
            Handle::N => 270.0,
            Handle::NE => 315.0,
        };
        match (((base + angle).rem_euclid(180.0) / 45.0).round() as i32) % 4 {
            0 => CursorIcon::ResizeHorizontal,
            1 => CursorIcon::ResizeNwSe,
            2 => CursorIcon::ResizeVertical,
            _ => CursorIcon::ResizeNeSw,
        }
    }

    fn left(self) -> bool {
        matches!(self, Handle::W | Handle::NW | Handle::SW)
    }

    fn right(self) -> bool {
        matches!(self, Handle::E | Handle::NE | Handle::SE)
    }

    fn top(self) -> bool {
        matches!(self, Handle::N | Handle::NW | Handle::NE)
    }

    fn bottom(self) -> bool {
        matches!(self, Handle::S | Handle::SW | Handle::SE)
    }

    fn is_corner(self) -> bool {
        matches!(self, Handle::NW | Handle::NE | Handle::SW | Handle::SE)
    }
}

/// Positions of a rectangle's 8 handles (corners, then edge midpoints).
pub fn handle_positions(r: Rect) -> [(Handle, Pos2); 8] {
    let c = r.center();
    [
        (Handle::NW, r.left_top()),
        (Handle::NE, r.right_top()),
        (Handle::SW, r.left_bottom()),
        (Handle::SE, r.right_bottom()),
        (Handle::N, pos2(c.x, r.top())),
        (Handle::S, pos2(c.x, r.bottom())),
        (Handle::W, pos2(r.left(), c.y)),
        (Handle::E, pos2(r.right(), c.y)),
    ]
}

/// Which handle is under `p`? Rectangle and point in screen coordinates.
/// Edges can be grabbed along their whole length, the inside is for moving.
/// Like Photoshop, the inside of a rectangle too thin for handles inside
/// moves it: its handles are then grabbed from outside.
pub fn hit_test(r: Rect, p: Pos2) -> Option<Handle> {
    let inset = |side: f32| {
        if side < 4.0 * GRAB_RADIUS {
            0.0
        } else {
            GRAB_RADIUS
        }
    };
    let inside = r.shrink2(vec2(inset(r.width()), inset(r.height())));
    if inside.contains(p) {
        return Some(Handle::Move);
    }
    // The nearest handle: on a small rectangle, they overlap.
    if let Some((h, _)) = handle_positions(r)
        .into_iter()
        .map(|(h, hp)| (h, hp.distance(p)))
        .filter(|&(_, d)| d <= GRAB_RADIUS)
        .min_by(|a, b| a.1.total_cmp(&b.1))
    {
        return Some(h);
    }
    let in_x = p.x >= r.left() - GRAB_RADIUS && p.x <= r.right() + GRAB_RADIUS;
    let in_y = p.y >= r.top() - GRAB_RADIUS && p.y <= r.bottom() + GRAB_RADIUS;
    let near = |a: f32, b: f32| (a - b).abs() <= GRAB_RADIUS * 0.6;
    if in_y && near(p.x, r.left()) {
        Some(Handle::W)
    } else if in_y && near(p.x, r.right()) {
        Some(Handle::E)
    } else if in_x && near(p.y, r.top()) {
        Some(Handle::N)
    } else if in_x && near(p.y, r.bottom()) {
        Some(Handle::S)
    } else if r.contains(p) {
        Some(Handle::Move)
    } else {
        None
    }
}

/// A handle drag in progress. Everything is in image coordinates.
#[derive(Clone, Copy, Debug)]
pub struct RectDrag {
    pub handle: Handle,
    start_rect: Rect,
    start_pos: Pos2,
}

impl RectDrag {
    pub fn new(handle: Handle, start_rect: Rect, start_pos: Pos2) -> Self {
        Self {
            handle,
            start_rect,
            start_pos,
        }
    }

    /// Center of the rectangle at the start of the drag.
    pub fn start_center(&self) -> Pos2 {
        self.start_rect.center()
    }

    /// Resulting rectangle when the pointer is at `pos`.
    ///
    /// With `keep_ratio`, a corner keeps the opposite corner fixed, an edge keeps
    /// the center of the other axis fixed.
    pub fn update(&self, pos: Pos2, keep_ratio: bool) -> Rect {
        let min_size = MIN_SIZE;
        let r = self.start_rect;
        let d: Vec2 = pos - self.start_pos;
        let h = self.handle;
        if h == Handle::Move {
            return r.translate(d);
        }

        let (mut x0, mut y0, mut x1, mut y1) = (r.min.x, r.min.y, r.max.x, r.max.y);
        if h.left() {
            x0 = (x0 + d.x).min(x1 - min_size);
        }
        if h.right() {
            x1 = (x1 + d.x).max(x0 + min_size);
        }
        if h.top() {
            y0 = (y0 + d.y).min(y1 - min_size);
        }
        if h.bottom() {
            y1 = (y1 + d.y).max(y0 + min_size);
        }

        if keep_ratio && r.width() > 0.0 && r.height() > 0.0 {
            let (sx, sy) = ((x1 - x0) / r.width(), (y1 - y0) / r.height());
            let s = if h.is_corner() {
                if (sx - 1.0).abs() > (sy - 1.0).abs() {
                    sx
                } else {
                    sy
                }
            } else if h.left() || h.right() {
                sx
            } else {
                sy
            };
            let s = s.max(min_size / r.width().min(r.height()));
            let (w, ht) = (r.width() * s, r.height() * s);

            if h.left() {
                x0 = x1 - w;
            } else if h.right() {
                x1 = x0 + w;
            } else {
                let cx = r.center().x;
                (x0, x1) = (cx - w / 2.0, cx + w / 2.0);
            }
            if h.top() {
                y0 = y1 - ht;
            } else if h.bottom() {
                y1 = y0 + ht;
            } else {
                let cy = r.center().y;
                (y0, y1) = (cy - ht / 2.0, cy + ht / 2.0);
            }
        }
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    /// Like [`RectDrag::update`], but around the starting center (Alt, like
    /// Photoshop): the opposite edge or corner moves symmetrically.
    pub fn update_from_center(&self, pos: Pos2, keep_ratio: bool) -> Rect {
        let r = self.update(pos, keep_ratio);
        let h = self.handle;
        if h == Handle::Move {
            return r;
        }
        let c = self.start_rect.center();
        let half = |d: f32| d.max(MIN_SIZE / 2.0);
        let (mut x0, mut y0, mut x1, mut y1) = (r.min.x, r.min.y, r.max.x, r.max.y);
        if h.left() {
            let d = half(c.x - x0);
            (x0, x1) = (c.x - d, c.x + d);
        } else if h.right() {
            let d = half(x1 - c.x);
            (x0, x1) = (c.x - d, c.x + d);
        }
        if h.top() {
            let d = half(c.y - y0);
            (y0, y1) = (c.y - d, c.y + d);
        } else if h.bottom() {
            let d = half(y1 - c.y);
            (y0, y1) = (c.y - d, c.y + d);
        }
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    /// Pointer brought back onto the horizontal or vertical axis of the starting
    /// point (Shift during a move).
    pub fn constrain_axis(&self, pos: Pos2) -> Pos2 {
        self.start_pos + constrain_axis(pos - self.start_pos)
    }

    /// Axes left free by [`RectDrag::constrain_axis`] for the pointer at `pos`.
    pub fn free_axes(&self, pos: Pos2) -> Vec2 {
        free_axes(pos - self.start_pos)
    }
}

/// Keeps the largest component of a move: horizontal or vertical
/// move only (Shift, like Photoshop).
pub fn constrain_axis(d: Vec2) -> Vec2 {
    d * free_axes(d)
}

/// Axes of a move kept by [`constrain_axis`]: 1 on the free axis, 0 on the
/// other.
pub fn free_axes(d: Vec2) -> Vec2 {
    if d.x.abs() >= d.y.abs() {
        vec2(1.0, 0.0)
    } else {
        vec2(0.0, 1.0)
    }
}

/// Rounds a rectangle's edges to the nearest pixel.
///
/// Halves are always rounded up (not "away from zero" like
/// `f32::round`): a rectangle with an integer size thus keeps its size,
/// even when centered on a negative half-pixel.
pub fn round_rect(r: Rect) -> Rect {
    let round = |p: Pos2| pos2((p.x + 0.5).floor(), (p.y + 0.5).floor());
    Rect::from_min_max(round(r.min), round(r.max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::vec2;

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(pos2(x0, y0), pos2(x1, y1))
    }

    #[test]
    fn hit_test_finds_corners_edges_and_inside() {
        let r = rect(100.0, 100.0, 300.0, 200.0);
        assert_eq!(hit_test(r, pos2(102.0, 98.0)), Some(Handle::NW));
        assert_eq!(hit_test(r, pos2(300.0, 200.0)), Some(Handle::SE));
        assert_eq!(hit_test(r, pos2(150.0, 101.0)), Some(Handle::N));
        assert_eq!(hit_test(r, pos2(299.0, 130.0)), Some(Handle::E));
        assert_eq!(hit_test(r, pos2(200.0, 150.0)), Some(Handle::Move));
        assert_eq!(hit_test(r, pos2(50.0, 50.0)), None);
    }

    #[test]
    fn small_and_thin_elements_move_from_inside() {
        // A 400 × 3 rule: every pixel inside moves it, handles around it.
        let line = rect(100.0, 100.0, 500.0, 103.0);
        for x in [110.0, 300.0, 490.0] {
            assert_eq!(hit_test(line, pos2(x, 101.5)), Some(Handle::Move));
        }
        assert_eq!(hit_test(line, pos2(300.0, 97.0)), Some(Handle::N));
        assert_eq!(hit_test(line, pos2(300.0, 106.0)), Some(Handle::S));
        assert_eq!(hit_test(line, pos2(503.0, 101.5)), Some(Handle::E));
        assert_eq!(hit_test(line, pos2(96.0, 96.0)), Some(Handle::NW));
        // A 12 × 12 icon: all of it moves.
        let icon = rect(100.0, 100.0, 112.0, 112.0);
        for (x, y) in [(100.5, 100.5), (106.0, 106.0), (111.5, 106.0)] {
            assert_eq!(hit_test(icon, pos2(x, y)), Some(Handle::Move));
        }
        assert_eq!(hit_test(icon, pos2(115.0, 115.0)), Some(Handle::SE));
    }

    #[test]
    fn free_edge_drag_moves_one_side() {
        let d = RectDrag::new(Handle::E, rect(0.0, 0.0, 100.0, 50.0), pos2(100.0, 25.0));
        assert_eq!(
            d.update(pos2(130.0, 40.0), false),
            rect(0.0, 0.0, 130.0, 50.0)
        );
    }

    #[test]
    fn edge_cannot_cross_opposite_side() {
        let d = RectDrag::new(Handle::W, rect(0.0, 0.0, 100.0, 50.0), pos2(0.0, 25.0));
        assert_eq!(
            d.update(pos2(500.0, 25.0), false),
            rect(99.0, 0.0, 100.0, 50.0)
        );
    }

    #[test]
    fn corner_with_ratio_keeps_opposite_corner_and_proportions() {
        let d = RectDrag::new(Handle::SE, rect(10.0, 10.0, 110.0, 60.0), pos2(110.0, 60.0));
        let r = d.update(pos2(210.0, 70.0), true);
        assert_eq!(r.min, pos2(10.0, 10.0));
        assert_eq!(r.width(), 200.0);
        assert_eq!(r.height(), 100.0);
    }

    #[test]
    fn nw_corner_with_ratio_anchors_bottom_right() {
        let d = RectDrag::new(Handle::NW, rect(0.0, 0.0, 100.0, 100.0), pos2(0.0, 0.0));
        let r = d.update(pos2(50.0, 10.0), true);
        assert_eq!(r, rect(50.0, 50.0, 100.0, 100.0));
    }

    #[test]
    fn edge_with_ratio_scales_other_axis_around_center() {
        let d = RectDrag::new(Handle::E, rect(0.0, 0.0, 100.0, 50.0), pos2(100.0, 25.0));
        let r = d.update(pos2(200.0, 25.0), true);
        assert_eq!(r, rect(0.0, -25.0, 200.0, 75.0));
    }

    #[test]
    fn rounding_keeps_integer_sizes() {
        // Real case: 511 px canvas centered at -149.5 + 255.5, resized to 512 px.
        let c = pos2(106.0, 50.5);
        for size in [511.0, 512.0, 513.0] {
            let r = round_rect(Rect::from_center_size(c, vec2(size, size)));
            assert_eq!((r.width(), r.height()), (size, size));
        }
        let r = round_rect(rect(-149.5, -3.5, 362.5, 8.5));
        assert_eq!((r.width(), r.height()), (512.0, 12.0));
    }

    #[test]
    fn cursors_follow_rotation() {
        assert_eq!(Handle::E.cursor(0.0), CursorIcon::ResizeHorizontal);
        assert_eq!(Handle::E.cursor(90.0), CursorIcon::ResizeVertical);
        assert_eq!(Handle::N.cursor(45.0), CursorIcon::ResizeNeSw);
        assert_eq!(Handle::SE.cursor(-45.0), CursorIcon::ResizeHorizontal);
    }

    #[test]
    fn alt_resizes_around_center() {
        let d = RectDrag::new(Handle::E, rect(0.0, 0.0, 100.0, 50.0), pos2(100.0, 25.0));
        let r = d.update_from_center(pos2(120.0, 25.0), false);
        assert_eq!(r, rect(-20.0, 0.0, 120.0, 50.0));
        let d = RectDrag::new(Handle::SE, rect(0.0, 0.0, 100.0, 50.0), pos2(100.0, 50.0));
        let r = d.update_from_center(pos2(150.0, 75.0), true);
        assert_eq!(r, rect(-50.0, -25.0, 150.0, 75.0));
    }

    #[test]
    fn shift_constrains_move_to_one_axis() {
        let d = RectDrag::new(Handle::Move, rect(0.0, 0.0, 10.0, 10.0), pos2(5.0, 5.0));
        assert_eq!(d.constrain_axis(pos2(12.0, 7.0)), pos2(12.0, 5.0));
        assert_eq!(d.constrain_axis(pos2(4.0, -3.0)), pos2(5.0, -3.0));
    }

    #[test]
    fn move_translates() {
        let d = RectDrag::new(Handle::Move, rect(0.0, 0.0, 10.0, 10.0), pos2(5.0, 5.0));
        assert_eq!(d.update(pos2(8.0, 1.0), true), rect(3.0, -4.0, 13.0, 6.0));
    }
}
