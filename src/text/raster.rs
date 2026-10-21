//! Anti-aliased filling of glyph outlines (exact coverage by signed area
//! accumulation, non-zero winding for non-overlapping contours).

/// Coverage accumulator over `w` × `h` pixels.
pub struct Raster {
    w: usize,
    h: usize,
    acc: Vec<f32>,
    last: (f32, f32),
    first: (f32, f32),
}

impl Raster {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            // One spare cell: a line ending on the right edge spills into it.
            acc: vec![0.0; w * h + 2],
            last: (0.0, 0.0),
            first: (0.0, 0.0),
        }
    }

    pub fn move_to(&mut self, p: (f32, f32)) {
        self.close();
        self.first = p;
        self.last = p;
    }

    pub fn line_to(&mut self, p: (f32, f32)) {
        self.line(self.last, p);
        self.last = p;
    }

    pub fn quad_to(&mut self, c: (f32, f32), p: (f32, f32)) {
        let p0 = self.last;
        let dd = ((p0.0 - 2.0 * c.0 + p.0).powi(2) + (p0.1 - 2.0 * c.1 + p.1).powi(2)).sqrt();
        // Uniform steps: within 0.05 px of the curve.
        let n = ((dd * 5.0).sqrt().ceil() as usize).clamp(1, 100);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            self.line_to((
                u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p.0,
                u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p.1,
            ));
        }
    }

    pub fn cubic_to(&mut self, c1: (f32, f32), c2: (f32, f32), p: (f32, f32)) {
        let p0 = self.last;
        let d = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
            ((a.0 - 2.0 * b.0 + c.0).powi(2) + (a.1 - 2.0 * b.1 + c.1).powi(2)).sqrt()
        };
        let dd = d(p0, c1, c2).max(d(c1, c2, p));
        let n = ((dd * 15.0).sqrt().ceil() as usize).clamp(1, 150);
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            self.line_to((
                a * p0.0 + b * c1.0 + c * c2.0 + e * p.0,
                a * p0.1 + b * c1.1 + c * c2.1 + e * p.1,
            ));
        }
    }

    /// Closes the current contour.
    pub fn close(&mut self) {
        if self.last != self.first {
            self.line(self.last, self.first);
        }
        self.last = self.first;
    }

    fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if (p0.1 - p1.1).abs() <= f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        // Kept inside horizontally: the caller sizes the raster to the path.
        let clamp_x = |x: f32| x.clamp(0.0, self.w as f32);
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
        }
        let y_start = p0.1.max(0.0) as usize;
        let y_end = (self.h).min(p1.1.ceil().max(0.0) as usize);
        for y in y_start..y_end {
            let row = y * self.w;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let x_next = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < x_next {
                (clamp_x(x), clamp_x(x_next))
            } else {
                (clamp_x(x_next), clamp_x(x))
            };
            let x0_floor = x0.floor();
            let x0i = x0_floor as usize;
            let x1_ceil = x1.ceil();
            let x1i = x1_ceil as usize;
            if x1i <= x0i + 1 {
                let xm = 0.5 * (x0 + x1) - x0_floor;
                self.acc[row + x0i] += d - d * xm;
                self.acc[row + x0i + 1] += d * xm;
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0_floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1_ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                self.acc[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.acc[row + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.acc[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.acc[row + x1i - 1] += d * (1.0 - a2 - am);
                }
                self.acc[row + x1i] += d * am;
            }
            x = x_next;
        }
    }

    /// Coverage of each pixel (0–1), row by row.
    pub fn coverage(mut self) -> Vec<f32> {
        self.close();
        let mut sum = 0.0;
        self.acc.truncate(self.w * self.h);
        for a in &mut self.acc {
            sum += *a;
            *a = sum.abs().min(1.0);
        }
        self.acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(r: &mut Raster, x0: f32, y0: f32, x1: f32, y1: f32) {
        r.move_to((x0, y0));
        r.line_to((x1, y0));
        r.line_to((x1, y1));
        r.line_to((x0, y1));
        r.close();
    }

    #[test]
    fn fills_shapes_with_exact_coverage() {
        let mut r = Raster::new(8, 8);
        square(&mut r, 2.0, 2.0, 6.0, 6.0);
        // A hole, the other way round.
        r.move_to((3.0, 3.0));
        r.line_to((3.0, 5.0));
        r.line_to((5.0, 5.0));
        r.line_to((5.0, 3.0));
        let c = r.coverage();
        assert_eq!(c[2 * 8 + 2], 1.0);
        assert_eq!(c[4 * 8 + 4], 0.0, "hole");
        assert_eq!(c[0], 0.0);
        assert_eq!(c.iter().sum::<f32>(), 12.0);

        // Half a pixel covered at the edges.
        let mut r = Raster::new(4, 1);
        square(&mut r, 0.5, 0.0, 2.5, 1.0);
        let c = r.coverage();
        assert!((c[0] - 0.5).abs() < 1e-5 && c[1] == 1.0 && (c[2] - 0.5).abs() < 1e-5);
        assert_eq!(c[3], 0.0);

        // A disc made of curves has about its area.
        let mut r = Raster::new(40, 40);
        let k = 0.552_284_8 * 15.0;
        r.move_to((35.0, 20.0));
        r.cubic_to((35.0, 20.0 + k), (20.0 + k, 35.0), (20.0, 35.0));
        r.cubic_to((20.0 - k, 35.0), (5.0, 20.0 + k), (5.0, 20.0));
        r.cubic_to((5.0, 20.0 - k), (20.0 - k, 5.0), (20.0, 5.0));
        r.quad_to((35.0, 5.0), (35.0, 20.0));
        let area: f32 = r.coverage().iter().sum();
        // Three quarters of a disc, and a triangle with its parabolic segment.
        let expected = std::f32::consts::PI * 225.0 * 0.75 + 112.5 * (1.0 + 2.0 / 3.0);
        assert!((area - expected).abs() < 3.0, "{area} vs {expected}");
    }
}
