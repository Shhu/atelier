//! Planes of floats (one value per pixel) and the filters the development
//! needs: box and Gaussian blurs, guided filters (edge-preserving
//! smoothing), resampling. In parallel.

use crate::filters::par_rows;

#[derive(Clone, Debug)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Plane {
    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize) -> f32 + Sync) -> Plane {
        let mut data = vec![0.0; w * h];
        par_rows(&mut data, w.max(1), |y0, band| {
            for (i, v) in band.iter_mut().enumerate() {
                *v = f(y0 * w + i);
            }
        });
        Plane { w, h, data }
    }

    /// `f` of this plane's and `other`'s values.
    pub fn zip(&self, other: &Plane, f: impl Fn(f32, f32) -> f32 + Sync) -> Plane {
        Plane::from_fn(self.w, self.h, |i| f(self.data[i], other.data[i]))
    }

    pub fn map(&self, f: impl Fn(f32) -> f32 + Sync) -> Plane {
        Plane::from_fn(self.w, self.h, |i| f(self.data[i]))
    }

    /// Average over a (2r + 1)² square, clipped to the plane.
    pub fn box_blur(&self, r: usize) -> Plane {
        let (w, h) = (self.w, self.h);
        if r == 0 || w == 0 || h == 0 {
            return self.clone();
        }
        let mut wide = vec![0.0f32; w * h];
        par_rows(&mut wide, w, |y0, band| {
            let mut prefix = vec![0.0f64; w + 1];
            for (i, out) in band.chunks_exact_mut(w).enumerate() {
                let row = &self.data[(y0 + i) * w..][..w];
                for x in 0..w {
                    prefix[x + 1] = prefix[x] + row[x] as f64;
                }
                for (x, o) in out.iter_mut().enumerate() {
                    let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
                    *o = ((prefix[b] - prefix[a]) / (b - a) as f64) as f32;
                }
            }
        });
        let mut data = vec![0.0f32; w * h];
        par_rows(&mut data, w, |y0, band| {
            let rows = band.len() / w;
            let mut acc = vec![0.0f64; w];
            for y in y0.saturating_sub(r)..(y0 + r + 1).min(h) {
                for (a, v) in acc.iter_mut().zip(&wide[y * w..][..w]) {
                    *a += *v as f64;
                }
            }
            for i in 0..rows {
                let y = y0 + i;
                let count = ((y + r).min(h - 1) - y.saturating_sub(r) + 1) as f64;
                for (o, a) in band[i * w..][..w].iter_mut().zip(&acc) {
                    *o = (*a / count) as f32;
                }
                if y + r + 1 < h {
                    for (a, v) in acc.iter_mut().zip(&wide[(y + r + 1) * w..][..w]) {
                        *a += *v as f64;
                    }
                }
                if y >= r {
                    for (a, v) in acc.iter_mut().zip(&wide[(y - r) * w..][..w]) {
                        *a -= *v as f64;
                    }
                }
            }
        });
        Plane { w, h, data }
    }

    /// Gaussian blur of standard deviation `sigma` (pixels): an exact
    /// kernel when small, three box blurs otherwise.
    pub fn gaussian(&self, sigma: f32) -> Plane {
        if sigma < 0.2 || self.w == 0 || self.h == 0 {
            return self.clone();
        }
        if sigma < 3.0 {
            let r = (sigma * 3.0).ceil() as usize;
            let mut kernel: Vec<f32> = (0..=2 * r)
                .map(|i| {
                    let d = i as f32 - r as f32;
                    (-d * d / (2.0 * sigma * sigma)).exp()
                })
                .collect();
            let sum: f32 = kernel.iter().sum();
            kernel.iter_mut().for_each(|k| *k /= sum);
            return self.convolve(&kernel);
        }
        // Box sizes giving the same variance (Kovesi).
        let n = 3.0;
        let ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
        let mut lower = ideal.floor() as i32;
        if lower % 2 == 0 {
            lower -= 1;
        }
        let l = lower as f32;
        let m = ((12.0 * sigma * sigma - n * l * l - 4.0 * n * l - 3.0 * n) / (-4.0 * l - 4.0))
            .round() as i32;
        let mut out = self.clone();
        for i in 0..3 {
            let size = if i < m { lower } else { lower + 2 };
            out = out.box_blur(((size - 1) / 2).max(0) as usize);
        }
        out
    }

    /// Separable convolution by a symmetric `kernel`, edges repeated.
    fn convolve(&self, kernel: &[f32]) -> Plane {
        let (w, h) = (self.w, self.h);
        let r = kernel.len() / 2;
        let mut wide = vec![0.0f32; w * h];
        par_rows(&mut wide, w, |y0, band| {
            for (i, out) in band.chunks_exact_mut(w).enumerate() {
                let row = &self.data[(y0 + i) * w..][..w];
                for (x, o) in out.iter_mut().enumerate() {
                    *o = kernel
                        .iter()
                        .enumerate()
                        .map(|(k, wt)| row[(x + k).saturating_sub(r).min(w - 1)] * wt)
                        .sum();
                }
            }
        });
        let mut data = vec![0.0f32; w * h];
        par_rows(&mut data, w, |y0, band| {
            for (i, out) in band.chunks_exact_mut(w).enumerate() {
                let y = y0 + i;
                for (k, wt) in kernel.iter().enumerate() {
                    let sy = (y + k).saturating_sub(r).min(h - 1);
                    for (o, v) in out.iter_mut().zip(&wide[sy * w..][..w]) {
                        *o += v * wt;
                    }
                }
            }
        });
        Plane { w, h, data }
    }

    /// Average of `s` × `s` blocks (the last ones partial).
    fn reduced(&self, s: usize) -> Plane {
        let (tw, th) = (self.w.div_ceil(s), self.h.div_ceil(s));
        Plane::from_fn(tw, th, |i| {
            let (tx, ty) = (i % tw, i / tw);
            let (x1, y1) = ((tx * s + s).min(self.w), (ty * s + s).min(self.h));
            let mut sum = 0.0;
            for y in ty * s..y1 {
                sum += self.data[y * self.w + tx * s..y * self.w + x1]
                    .iter()
                    .sum::<f32>();
            }
            sum / ((x1 - tx * s) * (y1 - ty * s)) as f32
        })
    }

    /// Bilinear enlargement of a plane reduced by `s` back to w × h.
    fn enlarged(&self, s: usize, w: usize, h: usize) -> Plane {
        let (lw, lh) = (self.w, self.h);
        Plane::from_fn(w, h, |i| {
            let (x, y) = (i % w, i / w);
            let fx = ((x as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (lw - 1) as f32);
            let fy = ((y as f32 + 0.5) / s as f32 - 0.5).clamp(0.0, (lh - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(lw - 1), (y0 + 1).min(lh - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let at = |x: usize, y: usize| self.data[y * lw + x];
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// Guided filter (He et al.): `input` smoothed over radius `r` except
/// across the edges of `guide` stronger than `eps` (a variance). The
/// coefficients are computed on planes reduced by `s` (fast guided filter).
pub fn guided(guide: &Plane, input: &Plane, r: usize, eps: f32, s: usize) -> Plane {
    let s = s.max(1);
    let (gi, pi) = if s > 1 {
        (guide.reduced(s), input.reduced(s))
    } else {
        (guide.clone(), input.clone())
    };
    let r = (r / s).max(1);
    let mean_i = gi.box_blur(r);
    let mean_p = pi.box_blur(r);
    let corr_ii = gi.map(|v| v * v).box_blur(r);
    let corr_ip = gi.zip(&pi, |a, b| a * b).box_blur(r);
    let (w, h) = (gi.w, gi.h);
    let a = Plane::from_fn(w, h, |i| {
        let var = corr_ii.data[i] - mean_i.data[i] * mean_i.data[i];
        let cov = corr_ip.data[i] - mean_i.data[i] * mean_p.data[i];
        cov / (var.max(0.0) + eps)
    });
    let b = Plane::from_fn(w, h, |i| mean_p.data[i] - a.data[i] * mean_i.data[i]);
    let mut mean_a = a.box_blur(r);
    let mut mean_b = b.box_blur(r);
    if s > 1 {
        mean_a = mean_a.enlarged(s, guide.w, guide.h);
        mean_b = mean_b.enlarged(s, guide.w, guide.h);
    }
    Plane::from_fn(guide.w, guide.h, |i| {
        mean_a.data[i] * guide.data[i] + mean_b.data[i]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(w: usize, h: usize) -> Plane {
        Plane::from_fn(w, h, |i| if i % w < w / 2 { 0.0 } else { 4.0 })
    }

    #[test]
    fn blurs_keep_the_average_and_spread() {
        let p = step(40, 30);
        for blurred in [p.box_blur(3), p.gaussian(1.0), p.gaussian(6.0)] {
            let mean: f32 = blurred.data.iter().sum::<f32>() / blurred.data.len() as f32;
            assert!((mean - 2.0).abs() < 0.05, "{mean}");
            let edge = blurred.data[10 * 40 + 19];
            assert!(edge > 0.1 && edge < 2.0, "{edge}");
        }
        // Constant planes stay constant.
        let flat = Plane::from_fn(17, 9, |_| 0.5);
        assert!(
            flat.gaussian(5.0)
                .data
                .iter()
                .all(|v| (v - 0.5).abs() < 1e-5)
        );
    }

    #[test]
    fn guided_filter_keeps_edges_and_smooths_texture() {
        let (w, h) = (64, 48);
        // A strong edge with a fine texture on both sides.
        let p = Plane::from_fn(w, h, |i| {
            let (x, y) = (i % w, i / w);
            let base = if x < w / 2 { 0.0 } else { 4.0 };
            base + if (x + y) % 2 == 0 { 0.1 } else { -0.1 }
        });
        for s in [1, 4] {
            let q = guided(&p, &p, 8, 0.2, s);
            let left = q.data[20 * w + w / 2 - 3];
            let right = q.data[20 * w + w / 2 + 2];
            assert!(left < 0.6 && right > 3.4, "edge kept: {left} {right}");
            let texture = (q.data[10 * w + 10] - q.data[10 * w + 11]).abs();
            assert!(texture < 0.05, "texture smoothed: {texture}");
        }
    }
}
