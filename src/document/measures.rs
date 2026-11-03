//! What the Color Sampler and Ruler tools leave on a document: the sampling
//! points (saved with it, in its history) and the measure (neither), and how
//! they follow the canvas.

use super::{Document, PixelRect, intersect};
use crate::compose;

/// Points a document can hold.
pub const MAX_SAMPLERS: usize = 10;

/// A sampling point: canvas coordinates of the point clicked (the pixel
/// under it is read).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorSampler {
    pub x: f32,
    pub y: f32,
}

impl ColorSampler {
    pub fn pixel(self) -> (i64, i64) {
        (self.x.floor() as i64, self.y.floor() as i64)
    }
}

/// The Ruler tool's line, in canvas coordinates: from `vertex` to `first`;
/// `second`: the protractor's other arm, from the same vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    pub vertex: (f32, f32),
    pub first: (f32, f32),
    pub second: Option<(f32, f32)>,
}

fn direction(from: (f32, f32), to: (f32, f32)) -> f32 {
    // Counter-clockwise from the right, the canvas's y going down.
    (from.1 - to.1).atan2(to.0 - from.0).to_degrees()
}

fn length(from: (f32, f32), to: (f32, f32)) -> f32 {
    (to.0 - from.0).hypot(to.1 - from.1)
}

impl Measure {
    /// The line's direction (−180..180, counter-clockwise from horizontal),
    /// or with a protractor the angle between its arms (0..180).
    pub fn angle(&self) -> f32 {
        let a = direction(self.vertex, self.first);
        match self.second {
            None => a,
            Some(second) => {
                let between = (a - direction(self.vertex, second)).abs() % 360.0;
                if between > 180.0 {
                    360.0 - between
                } else {
                    between
                }
            }
        }
    }

    /// Length of the line (the first arm).
    pub fn length(&self) -> f32 {
        length(self.vertex, self.first)
    }

    /// Length of the protractor's other arm.
    pub fn second_length(&self) -> Option<f32> {
        self.second.map(|s| length(self.vertex, s))
    }

    /// The clockwise turn (−45..45 degrees) that brings the line onto the
    /// nearest axis.
    pub fn straighten_angle(&self) -> f32 {
        let a = direction(self.vertex, self.first);
        let r = a.rem_euclid(90.0);
        if r > 45.0 { r - 90.0 } else { r }
    }
}

/// The largest upright rectangle inside a `w`×`h` rectangle turned by
/// `degrees` around its center: its width and height.
pub fn inner_size(w: f32, h: f32, degrees: f32) -> (f32, f32) {
    let t = degrees.to_radians();
    let (s, c) = (t.sin().abs(), t.cos().abs());
    let (long, short) = (w.max(h), w.min(h));
    let (a, b) = if short <= 2.0 * s * c * long || (s - c).abs() < 1e-6 {
        // Two corners touch the long side.
        let x = 0.5 * short;
        if w >= h {
            (x / s, x / c)
        } else {
            (x / c, x / s)
        }
    } else {
        let d = c * c - s * s;
        ((w * c - h * s) / d, (h * c - w * s) / d)
    };
    (a.max(1.0).min(w), b.max(1.0).min(h))
}

impl Document {
    /// The image as shown at `pixel`, averaged over `radius` pixels around
    /// it (within the canvas, by coverage); `None` where nothing shows.
    pub fn sampled(&self, pixel: (i64, i64), radius: i64) -> Option<[u8; 3]> {
        self.sampled_all(&[pixel], radius).pop().flatten()
    }

    /// [`Self::sampled`] at each of `pixels`, the layers composed once.
    pub fn sampled_all(&self, pixels: &[(i64, i64)], radius: i64) -> Vec<Option<[u8; 3]>> {
        let canvas = self.full_rect();
        let side = (radius * 2 + 1) as u32;
        let rects: Vec<_> = pixels
            .iter()
            .map(|p| {
                let around = PixelRect {
                    x: p.0 - radius,
                    y: p.1 - radius,
                    w: side,
                    h: side,
                };
                intersect(around, canvas)
            })
            .collect();
        if rects.iter().all(Option::is_none) {
            return vec![None; pixels.len()];
        }
        let options = compose::Options {
            held: self.effects_held,
            later_styles: true,
            ..Default::default()
        };
        let layers = self.render_layers();
        let plan = compose::plan(&layers, canvas, &mut compose::FullSize, &options);
        rects
            .into_iter()
            .map(|rect| {
                let image = plan.render(rect?);
                let (mut sum, mut weight) = ([0u64; 3], 0u64);
                for p in image.pixels() {
                    let a = u64::from(p[3]);
                    weight += a;
                    for (s, &c) in sum.iter_mut().zip(&p.0[..3]) {
                        *s += u64::from(c) * a;
                    }
                }
                (weight > 0).then(|| sum.map(|s| ((s + weight / 2) / weight) as u8))
            })
            .collect()
    }

    /// The canvas becomes `canvas`, a point (x, y) going to `map`(x, y): the
    /// sampling points follow, those falling off it go. The measure, for
    /// what was there, is dropped.
    pub(super) fn follow_measures(
        &mut self,
        canvas: (u32, u32),
        map: impl Fn(f32, f32) -> (f32, f32),
    ) {
        self.ruler = None;
        self.samplers = self
            .samplers
            .iter()
            .map(|s| {
                let (x, y) = map(s.x, s.y);
                ColorSampler { x, y }
            })
            .filter(|s| {
                (0.0..canvas.0 as f32).contains(&s.x) && (0.0..canvas.1 as f32).contains(&s.y)
            })
            .collect();
    }
}

/// Where the canvas's turn by `degrees` (clockwise) takes a point.
pub(super) fn turned(
    degrees: f32,
    old: (u32, u32),
    new: (u32, u32),
) -> impl Fn(f32, f32) -> (f32, f32) {
    let (w, h) = (old.0 as f32, old.1 as f32);
    let (nw, nh) = (new.0 as f32, new.1 as f32);
    let turn = degrees.rem_euclid(360.0);
    let (s, c) = (turn.to_radians().sin(), turn.to_radians().cos());
    move |x, y| {
        // Exact for the quarter turns.
        if turn == 90.0 {
            (h - y, x)
        } else if turn == 180.0 {
            (w - x, h - y)
        } else if turn == 270.0 {
            (y, w - x)
        } else {
            let (dx, dy) = (x - w / 2.0, y - h / 2.0);
            (nw / 2.0 + dx * c - dy * s, nh / 2.0 + dx * s + dy * c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(to: (f32, f32)) -> Measure {
        Measure {
            vertex: (0.0, 0.0),
            first: to,
            second: None,
        }
    }

    #[test]
    fn the_angle_counts_counter_clockwise_and_straightening_takes_the_nearest_axis() {
        assert!((line((10.0, -10.0)).angle() - 45.0).abs() < 1e-4);
        assert!((line((10.0, 10.0)).angle() + 45.0).abs() < 1e-4);
        assert!((line((-10.0, 0.0)).angle().abs() - 180.0).abs() < 1e-4);
        // A line going up by 10°: a clockwise turn of 10°.
        let up = line((100.0, -(10f32.to_radians().tan()) * 100.0));
        assert!((up.straighten_angle() - 10.0).abs() < 1e-3);
        let down = line((100.0, (10f32.to_radians().tan()) * 100.0));
        assert!((down.straighten_angle() + 10.0).abs() < 1e-3);
        // Nearly vertical: on the vertical axis, not a quarter turn.
        let tall = line((5.0, -100.0));
        assert!(tall.straighten_angle().abs() < 3.0);
        // Drawn right to left: the same small turn.
        let back = line((-100.0, (10f32.to_radians().tan()) * 100.0));
        assert!((back.straighten_angle() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn a_protractor_gives_the_angle_between_its_arms() {
        let m = Measure {
            vertex: (10.0, 10.0),
            first: (20.0, 10.0),
            second: Some((10.0, 0.0)),
        };
        assert!((m.angle() - 90.0).abs() < 1e-4);
        assert_eq!(m.length(), 10.0);
        assert_eq!(m.second_length(), Some(10.0));
    }

    #[test]
    fn the_inner_rectangle_fits_in_the_turned_one() {
        assert_eq!(inner_size(400.0, 300.0, 0.0), (400.0, 300.0));
        for degrees in [1.0f32, 5.0, 10.0, 30.0, 45.0, -20.0] {
            let (a, b) = inner_size(400.0, 300.0, degrees);
            let t = degrees.to_radians();
            let (s, c) = (t.sin().abs(), t.cos().abs());
            // Its turned-back box stays within the rectangle.
            assert!(a * c + b * s <= 400.5, "{degrees}");
            assert!(a * s + b * c <= 300.5, "{degrees}");
            assert!(a > 50.0 && b > 50.0, "{degrees}: {a}x{b}");
        }
    }

    #[test]
    fn points_follow_a_canvas_turn() {
        let m = turned(90.0, (400, 300), (300, 400));
        assert_eq!(m(10.0, 20.0), (280.0, 10.0));
        let m = turned(180.0, (400, 300), (400, 300));
        assert_eq!(m(10.0, 20.0), (390.0, 280.0));
        let m = turned(-90.0, (400, 300), (300, 400));
        assert_eq!(m(10.0, 20.0), (20.0, 390.0));
    }
}
