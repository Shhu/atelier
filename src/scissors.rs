//! Magnetic Lasso ("intelligent scissors", Mortensen and Barrett): the
//! stroke between two points follows the cheapest path, and the image's
//! edges are cheap.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use image::RgbaImage;

/// Cost of crossing each pixel, low on edges.
pub struct EdgeMap {
    width: u32,
    height: u32,
    /// Edge strength (0..1).
    strength: Vec<f32>,
}

impl EdgeMap {
    /// Edges of the image (Sobel on each channel, alpha included).
    pub fn new(image: &RgbaImage) -> Self {
        let (w, h) = image.dimensions();
        let (wi, hi) = (w as i64, h as i64);
        let raw = image.as_raw();
        let at = |x: i64, y: i64, c: usize| {
            let (x, y) = (x.clamp(0, wi - 1), y.clamp(0, hi - 1));
            raw[((y * wi + x) * 4) as usize + c] as f32
        };
        let mut strength = vec![0.0f32; (w * h) as usize];
        for y in 0..hi {
            for x in 0..wi {
                let mut best = 0.0f32;
                for c in 0..4 {
                    let gx = at(x + 1, y - 1, c) + 2.0 * at(x + 1, y, c) + at(x + 1, y + 1, c)
                        - at(x - 1, y - 1, c)
                        - 2.0 * at(x - 1, y, c)
                        - at(x - 1, y + 1, c);
                    let gy = at(x - 1, y + 1, c) + 2.0 * at(x, y + 1, c) + at(x + 1, y + 1, c)
                        - at(x - 1, y - 1, c)
                        - 2.0 * at(x, y - 1, c)
                        - at(x + 1, y - 1, c);
                    best = best.max((gx * gx + gy * gy).sqrt());
                }
                strength[(y * wi + x) as usize] = best;
            }
        }
        // Normalized on a sharp edge (98th percentile), so as not to depend
        // on a single high-contrast pixel.
        let mut sorted: Vec<f32> = strength.iter().copied().filter(|&s| s > 0.0).collect();
        let reference = if sorted.is_empty() {
            1.0
        } else {
            let k = (sorted.len() * 98 / 100).min(sorted.len() - 1);
            *sorted.select_nth_unstable_by(k, f32::total_cmp).1
        };
        for s in &mut strength {
            *s = (*s / reference).min(1.0);
        }
        Self {
            width: w,
            height: h,
            strength,
        }
    }

    fn index(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }

    /// Cost of entering pixel (x, y).
    fn cost(&self, x: u32, y: u32) -> f32 {
        let s = self.strength[self.index(x, y)];
        0.02 + (1.0 - s) * (1.0 - s)
    }

    /// The point with the strongest edge within `radius` of (x, y) (the
    /// nearest one at equal strength): the stroke snaps to it.
    pub fn snap(&self, x: i64, y: i64, radius: u32) -> (u32, u32) {
        let clamp = |v: i64, max: u32| v.clamp(0, max as i64 - 1) as u32;
        let (cx, cy) = (clamp(x, self.width), clamp(y, self.height));
        let r = radius as i64;
        let mut best = (cx, cy);
        let mut best_score = f32::NEG_INFINITY;
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dy * dy;
                if d2 > r * r {
                    continue;
                }
                let (px, py) = (cx as i64 + dx, cy as i64 + dy);
                if px < 0 || py < 0 || px >= self.width as i64 || py >= self.height as i64 {
                    continue;
                }
                let score = self.strength[self.index(px as u32, py as u32)] - d2 as f32 * 0.001;
                if score > best_score {
                    best_score = score;
                    best = (px as u32, py as u32);
                }
            }
        }
        best
    }

    /// Least-cost path from `from` to `to` (8-connectivity), searched in the
    /// box of the two points enlarged by `margin`. Points from `from` to `to`.
    pub fn path(&self, from: (u32, u32), to: (u32, u32), margin: u32) -> Vec<(u32, u32)> {
        let x0 = from.0.min(to.0).saturating_sub(margin);
        let y0 = from.1.min(to.1).saturating_sub(margin);
        let x1 = (from.0.max(to.0) + margin + 1).min(self.width);
        let y1 = (from.1.max(to.1) + margin + 1).min(self.height);
        let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let local = |(x, y): (u32, u32)| (y - y0) as usize * bw + (x - x0) as usize;
        let mut dist = vec![f32::INFINITY; bw * bh];
        let mut prev = vec![u32::MAX; bw * bh];
        let mut heap = BinaryHeap::new();
        let (start, goal) = (local(from), local(to));
        dist[start] = 0.0;
        heap.push(Reverse((0u32, start as u32)));
        const STEPS: [(i64, i64, f32); 8] = [
            (1, 0, 1.0),
            (-1, 0, 1.0),
            (0, 1, 1.0),
            (0, -1, 1.0),
            (1, 1, std::f32::consts::SQRT_2),
            (1, -1, std::f32::consts::SQRT_2),
            (-1, 1, std::f32::consts::SQRT_2),
            (-1, -1, std::f32::consts::SQRT_2),
        ];
        while let Some(Reverse((d, i))) = heap.pop() {
            let i = i as usize;
            let d = f32::from_bits(d);
            if d > dist[i] {
                continue;
            }
            if i == goal {
                break;
            }
            let (lx, ly) = ((i % bw) as i64, (i / bw) as i64);
            for (dx, dy, len) in STEPS {
                let (nx, ny) = (lx + dx, ly + dy);
                if nx < 0 || ny < 0 || nx >= bw as i64 || ny >= bh as i64 {
                    continue;
                }
                let j = ny as usize * bw + nx as usize;
                let nd = d + len * self.cost(x0 + nx as u32, y0 + ny as u32);
                if nd < dist[j] {
                    dist[j] = nd;
                    prev[j] = i as u32;
                    // Positive floats compare like their bits.
                    heap.push(Reverse((nd.to_bits(), j as u32)));
                }
            }
        }
        let mut path = Vec::new();
        let mut i = goal;
        while i != start {
            path.push((x0 + (i % bw) as u32, y0 + (i / bw) as u32));
            if prev[i] == u32::MAX {
                // Unreachable (should not happen): straight line.
                return vec![from, to];
            }
            i = prev[i] as usize;
        }
        path.push(from);
        path.reverse();
        path
    }
}

/// Length of a pixel path.
pub fn length(path: &[(u32, u32)]) -> f32 {
    path.windows(2)
        .map(|w| {
            let (dx, dy) = (w[1].0 as f32 - w[0].0 as f32, w[1].1 as f32 - w[0].1 as f32);
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_follows_an_edge() {
        // Dark left half, light right half: vertical edge at x = 20.
        let img = RgbaImage::from_fn(40, 40, |x, _| {
            if x < 20 {
                image::Rgba([20, 20, 20, 255])
            } else {
                image::Rgba([230, 230, 230, 255])
            }
        });
        let map = EdgeMap::new(&img);
        // Snapping: a click 4 px from the edge lands on it.
        let a = map.snap(16, 5, 6);
        assert!((19..=20).contains(&a.0), "{a:?}");
        let b = map.snap(24, 35, 6);
        let path = map.path(a, b, 10);
        assert_eq!(path.first(), Some(&a));
        assert_eq!(path.last(), Some(&b));
        assert!(path.iter().all(|p| (19..=20).contains(&p.0)), "{path:?}");
        assert!((length(&path) - 30.0).abs() < 2.0);
    }
}
