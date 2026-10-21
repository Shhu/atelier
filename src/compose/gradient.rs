//! Gradients of fill layers, Gradient Overlay and Gradient Map: the
//! gradients of the Gradient Editor (see [`crate::paint::gradient`]),
//! laid over a box.

use crate::paint::gradient::{Gradient, Shape};

/// A gradient laid over a box: its colors (fixed, see
/// [`Gradient::resolved`]), shape, angle (degrees, counterclockwise, 0 =
/// left to right), scale and direction.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientFill {
    pub gradient: Gradient,
    pub style: Shape,
    pub angle: f32,
    /// 1 = the gradient spans the box.
    pub scale: f32,
    pub reverse: bool,
}

impl Default for GradientFill {
    fn default() -> Self {
        Self {
            gradient: Gradient::default(),
            style: Shape::Linear,
            angle: 90.0,
            scale: 1.0,
            reverse: false,
        }
    }
}

impl GradientFill {
    /// Position (0..1) in the gradient of point (x, y), for a gradient
    /// centered on the box `[x, y, w, h]`.
    pub fn t(&self, x: f32, y: f32, [bx, by, bw, bh]: [f32; 4]) -> f32 {
        let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
        let (s, c) = self.angle.to_radians().sin_cos();
        // Direction on screen (y goes down).
        let (dx, dy) = (c, -s);
        let (px, py) = (x - cx, y - cy);
        // Half the box's extent along the direction.
        let half = ((bw * c).abs() + (bh * s).abs()) / 2.0;
        let half = (half * self.scale.max(0.01)).max(0.5);
        let along = px * dx + py * dy;
        let across = -px * dy + py * dx;
        let t = match self.style {
            Shape::Linear => along / half / 2.0 + 0.5,
            Shape::Reflected => (along / half).abs(),
            Shape::Radial => (px * px + py * py).sqrt() / half,
            Shape::Diamond => (along.abs() + across.abs()) / half,
            Shape::Angle => {
                let a = (-across).atan2(along);
                (a / std::f32::consts::TAU).rem_euclid(1.0)
            }
        };
        let t = t.clamp(0.0, 1.0);
        if self.reverse { 1.0 - t } else { t }
    }
}

/// Entries of a [`Ramp`]'s color table.
const RAMP_SIZE: usize = 1024;

/// A gradient fill ready to be drawn: its colors sampled once.
pub struct Ramp {
    pub fill: GradientFill,
    colors: Vec<[f32; 4]>,
}

impl Ramp {
    pub fn new(fill: &GradientFill) -> Self {
        Self {
            colors: fill.gradient.ramp(RAMP_SIZE),
            fill: fill.clone(),
        }
    }

    /// Non-premultiplied color (0..1) at point (x, y), the gradient laid
    /// over the box `[x, y, w, h]`.
    #[inline]
    pub fn at(&self, x: f32, y: f32, bounds: [f32; 4]) -> [f32; 4] {
        let t = self.fill.t(x, y, bounds);
        self.colors[(t * (RAMP_SIZE - 1) as f32).round() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_and_radial_positions() {
        let fill = GradientFill {
            angle: 0.0,
            ..Default::default()
        };
        let r#box = [0.0, 0.0, 100.0, 50.0];
        assert!(fill.t(0.0, 25.0, r#box) < 0.01);
        assert!((fill.t(50.0, 10.0, r#box) - 0.5).abs() < 0.01);
        assert!(fill.t(100.0, 25.0, r#box) > 0.99);
        // 90°: from the bottom up.
        let up = GradientFill::default();
        assert!(up.t(10.0, 50.0, r#box) < 0.01 && up.t(10.0, 0.0, r#box) > 0.99);
        let radial = GradientFill {
            style: Shape::Radial,
            reverse: true,
            ..fill.clone()
        };
        assert!(radial.t(50.0, 25.0, r#box) > 0.99);
        let ramp = Ramp::new(&fill);
        assert_eq!(ramp.at(100.0, 25.0, r#box), [1.0; 4]);
        assert_eq!(ramp.at(0.0, 25.0, r#box), [0.0, 0.0, 0.0, 1.0]);
    }
}
