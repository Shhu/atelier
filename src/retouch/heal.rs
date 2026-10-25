//! The healing blend: a source's texture (its gradients) with the
//! destination's color and lighting, which it meets seamlessly at the edge
//! of the healed area. It solves a Poisson equation: the result is the
//! source plus a membrane, the smoothest surface (harmonic) that bridges
//! the difference destination − source at the edge.

use super::Px;
use crate::filters::par_rows;

const ZERO: Px = [0.0; 4];

/// First guess: relaxation sweeps on the finest grid, twice as many on
/// each coarser one (cascadic multigrid).
const GUESS_SWEEPS: usize = 4;
const MAX_SWEEPS: usize = 128;
/// Then multigrid cycles, until no cell is off by more than this (0..255).
const TOLERANCE: f32 = 0.05;
const MAX_CYCLES: usize = 16;
/// Sweeps before and after each coarse correction.
const SMOOTHING: usize = 2;
/// Over-relaxation of the sweeps.
const OMEGA: f32 = 1.5;

/// Over `w` × `h` cells: `src` where `inside`, shifted by the membrane that
/// meets `dest` around; `dest` elsewhere.
pub fn heal(dest: &[Px], src: &[Px], inside: &[bool], w: usize, h: usize) -> Vec<Px> {
    heal_until(dest, src, inside, w, h, &|| false).expect("never stopped")
}

/// [`heal`], given up (`None`) as soon as `stop` says so, between grids
/// and cycles.
pub fn heal_until(
    dest: &[Px],
    src: &[Px],
    inside: &[bool],
    w: usize,
    h: usize,
    stop: &(dyn Fn() -> bool + Sync),
) -> Option<Vec<Px>> {
    let mut diff: Vec<Px> = dest
        .iter()
        .zip(src)
        .zip(inside)
        .map(|((d, s), &free)| {
            if free {
                ZERO
            } else {
                std::array::from_fn(|c| d[c] - s[c])
            }
        })
        .collect();
    if !membrane_until(&mut diff, inside, w, h, stop) {
        return None;
    }
    let healed = diff
        .iter()
        .zip(src)
        .zip(dest)
        .zip(inside)
        .map(|(((m, s), d), &free)| {
            if free {
                std::array::from_fn(|c| s[c] + m[c])
            } else {
                *d
            }
        })
        .collect();
    Some(healed)
}

/// Solves Laplace's equation where `free`: each free cell becomes the mean
/// of its neighbors (within the grid), the others keep their value.
/// Multigrid: a first guess from coarser grids, then cycles correcting
/// the remaining error on coarser grids.
#[cfg(test)]
pub fn membrane(values: &mut [Px], free: &[bool], w: usize, h: usize) {
    membrane_until(values, free, w, h, &|| false);
}

/// [`membrane`], given up (`false`) as soon as `stop` says so.
fn membrane_until(
    values: &mut [Px],
    free: &[bool],
    w: usize,
    h: usize,
    stop: &(dyn Fn() -> bool + Sync),
) -> bool {
    let fixed = free.iter().filter(|&&f| !f).count();
    if fixed == 0 || fixed == free.len() {
        return true;
    }
    if !first_guess(values, free, w, h, 0, stop) {
        return false;
    }
    for _ in 0..MAX_CYCLES {
        if stop() {
            return false;
        }
        cycle(values, None, free, w, h);
        let residual = residual(values, None, free, w, h);
        if residual
            .iter()
            .all(|r| r.iter().all(|v| v.abs() < TOLERANCE))
        {
            break;
        }
    }
    true
}

/// The problem solved on a grid half as fine, whose solution starts the
/// relaxation of this one. A coarse cell is fixed to the mean of its fixed
/// cells, if any.
fn first_guess(
    values: &mut [Px],
    free: &[bool],
    w: usize,
    h: usize,
    depth: u32,
    stop: &(dyn Fn() -> bool + Sync),
) -> bool {
    if stop() {
        return false;
    }
    let fixed = free.iter().filter(|&&f| !f).count();
    if fixed == 0 || fixed == free.len() {
        return true;
    }
    if w > 3 && h > 3 {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut coarse = vec![ZERO; cw * ch];
        let mut coarse_free = vec![true; cw * ch];
        for cy in 0..ch {
            for cx in 0..cw {
                let (mut sum, mut n) = (ZERO, 0.0);
                for (x, y) in children(cx, cy, w, h) {
                    if !free[y * w + x] {
                        add(&mut sum, values[y * w + x]);
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    coarse[cy * cw + cx] = sum.map(|v| v / n);
                    coarse_free[cy * cw + cx] = false;
                }
            }
        }
        if !first_guess(&mut coarse, &coarse_free, cw, ch, depth + 1, stop) {
            return false;
        }
        prolong(values, free, w, &coarse, (cw, ch), false);
    } else {
        // A few cells: from the mean of the fixed ones.
        let mut sum = ZERO;
        for (v, _) in values.iter().zip(free).filter(|(_, f)| !**f) {
            add(&mut sum, *v);
        }
        let mean = sum.map(|v| v / fixed as f32);
        for (v, _) in values.iter_mut().zip(free).filter(|(_, f)| **f) {
            *v = mean;
        }
    }
    let sweeps = (GUESS_SWEEPS << depth.min(8)).min(MAX_SWEEPS);
    Checker::new(values, None, free, w, h).relax(sweeps, values);
    true
}

/// A multigrid cycle on L u = f (L: sum of the differences to the
/// neighbors; f: zero when `None`): relaxed, then the error is solved for
/// on a grid half as fine (where its four cells are free) and added,
/// then relaxed again.
fn cycle(u: &mut [Px], f: Option<&[Px]>, free: &[bool], w: usize, h: usize) {
    if w <= 3 || h <= 3 {
        Checker::new(u, f, free, w, h).relax(MAX_SWEEPS, u);
        return;
    }
    Checker::new(u, f, free, w, h).relax(SMOOTHING, u);
    let r = residual(u, f, free, w, h);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut coarse_f = vec![ZERO; cw * ch];
    let mut coarse_free = vec![false; cw * ch];
    for cy in 0..ch {
        for cx in 0..cw {
            let (x, y) = (2 * cx, 2 * cy);
            let cells = [
                y * w + x,
                y * w + x + 1,
                (y + 1) * w + x,
                (y + 1) * w + x + 1,
            ];
            if x + 1 < w && y + 1 < h && cells.iter().all(|&i| free[i]) {
                let i = cy * cw + cx;
                coarse_free[i] = true;
                // The coarse Laplacian spans twice the distance: four times
                // the fine one.
                for j in cells {
                    add(&mut coarse_f[i], r[j]);
                }
            }
        }
    }
    if coarse_free.contains(&true) {
        let mut error = vec![ZERO; cw * ch];
        cycle(&mut error, Some(&coarse_f), &coarse_free, cw, ch);
        prolong(u, free, w, &error, (cw, ch), true);
    }
    Checker::new(u, f, free, w, h).relax(SMOOTHING, u);
}

/// Cells of the fine grid under coarse cell (cx, cy).
fn children(cx: usize, cy: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    (2 * cy..(2 * cy + 2).min(h))
        .flat_map(move |y| (2 * cx..(2 * cx + 2).min(w)).map(move |x| (x, y)))
}

/// f − L u on the free cells (zero on the others).
fn residual(u: &[Px], f: Option<&[Px]>, free: &[bool], w: usize, h: usize) -> Vec<Px> {
    let mut r = vec![ZERO; w * h];
    rows(&mut r, w, |y0, band| {
        for (k, cell) in band.iter_mut().enumerate() {
            let (x, y) = (k % w, y0 + k / w);
            let i = y * w + x;
            if !free[i] {
                continue;
            }
            let (mut sum, mut n) = (ZERO, 0.0);
            for j in neighbors(x, y, w, h).into_iter().flatten() {
                add(&mut sum, u[j]);
                n += 1.0;
            }
            let rhs = f.map_or(ZERO, |f| f[i]);
            *cell = std::array::from_fn(|c| rhs[c] - (sum[c] - n * u[i][c]));
        }
    });
    r
}

/// Indices of the neighbors of (x, y) within the grid.
fn neighbors(x: usize, y: usize, w: usize, h: usize) -> [Option<usize>; 4] {
    let i = y * w + x;
    [
        (x > 0).then(|| i - 1),
        (x + 1 < w).then(|| i + 1),
        (y > 0).then(|| i - w),
        (y + 1 < h).then(|| i + w),
    ]
}

/// Bilinear interpolation of a coarse grid onto the free cells of this
/// one: replacing them, or added to them (`plus`).
fn prolong(
    values: &mut [Px],
    free: &[bool],
    w: usize,
    coarse: &[Px],
    (cw, ch): (usize, usize),
    plus: bool,
) {
    let at = |x: usize, y: usize| coarse[y * cw + x];
    rows(values, w, |y0, band| {
        for (r, line) in band.chunks_exact_mut(w).enumerate() {
            let y = y0 + r;
            let v = ((y as f32 - 0.5) / 2.0).clamp(0.0, (ch - 1) as f32);
            let (cy0, ty) = (v.floor() as usize, v.fract());
            let cy1 = (cy0 + 1).min(ch - 1);
            for (x, cell) in line.iter_mut().enumerate() {
                if !free[y * w + x] {
                    continue;
                }
                let u = ((x as f32 - 0.5) / 2.0).clamp(0.0, (cw - 1) as f32);
                let (cx0, tx) = (u.floor() as usize, u.fract());
                let cx1 = (cx0 + 1).min(cw - 1);
                let value: Px = std::array::from_fn(|c| {
                    let top = at(cx0, cy0)[c] * (1.0 - tx) + at(cx1, cy0)[c] * tx;
                    let bottom = at(cx0, cy1)[c] * (1.0 - tx) + at(cx1, cy1)[c] * tx;
                    top * (1.0 - ty) + bottom * ty
                });
                if plus {
                    add(cell, value);
                } else {
                    *cell = value;
                }
            }
        }
    });
}

/// Runs `f` on bands of rows in parallel, for grids large enough to be
/// worth the threads.
fn rows<T: Send>(data: &mut [T], row: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if data.len() < 1 << 17 {
        f(0, data);
    } else {
        par_rows(data, row, f);
    }
}

fn add(sum: &mut Px, v: Px) {
    for c in 0..4 {
        sum[c] += v[c];
    }
}

/// The grid split like a checkerboard: the neighbors of a cell are all of
/// the other color, so one color is relaxed at once, in parallel, from the
/// other (red-black Gauss-Seidel). Cell (x, y) is in color (x + y) % 2, at
/// index x / 2 of its row.
struct Checker {
    w: usize,
    h: usize,
    /// Cells per row and color.
    half: usize,
    values: [Vec<Px>; 2],
    /// Right-hand side f of L u = f (zero when `None`).
    rhs: Option<[Vec<Px>; 2]>,
    free: [Vec<bool>; 2],
}

impl Checker {
    fn new(values: &[Px], rhs: Option<&[Px]>, free: &[bool], w: usize, h: usize) -> Self {
        let half = w.div_ceil(2);
        let split = |all: &[Px]| {
            let mut out = [vec![ZERO; half * h], vec![ZERO; half * h]];
            for y in 0..h {
                for x in 0..w {
                    out[(x + y) & 1][y * half + x / 2] = all[y * w + x];
                }
            }
            out
        };
        let mut f = [vec![false; half * h], vec![false; half * h]];
        for y in 0..h {
            for x in 0..w {
                f[(x + y) & 1][y * half + x / 2] = free[y * w + x];
            }
        }
        Self {
            w,
            h,
            half,
            values: split(values),
            rhs: rhs.map(split),
            free: f,
        }
    }

    /// `sweeps` relaxations of both colors, then the free cells back into
    /// `out`.
    fn relax(mut self, sweeps: usize, out: &mut [Px]) {
        for _ in 0..sweeps {
            self.sweep(0);
            self.sweep(1);
        }
        let (w, half) = (self.w, self.half);
        for y in 0..self.h {
            for x in 0..w {
                let (color, i) = ((x + y) & 1, y * half + x / 2);
                if self.free[color][i] {
                    out[y * w + x] = self.values[color][i];
                }
            }
        }
    }

    fn sweep(&mut self, color: usize) {
        let (w, h, half) = (self.w, self.h, self.half);
        let [first, second] = &mut self.values;
        let (target, other) = if color == 0 {
            (first, &*second)
        } else {
            (second, &*first)
        };
        let free = &self.free[color];
        let rhs = self.rhs.as_ref().map(|r| &r[color]);
        rows(target, half, |y0, band| {
            for (r, row) in band.chunks_exact_mut(half).enumerate() {
                let y = y0 + r;
                // x of this color's cells in this row: 2 j + parity.
                let parity = (color + y) & 1;
                for (j, cell) in row.iter_mut().enumerate() {
                    if !free[y * half + j] {
                        continue;
                    }
                    let x = 2 * j + parity;
                    let (mut sum, mut n) = (ZERO, 0.0);
                    if x > 0 {
                        add(&mut sum, other[y * half + (x - 1) / 2]);
                        n += 1.0;
                    }
                    if x + 1 < w {
                        add(&mut sum, other[y * half + x.div_ceil(2)]);
                        n += 1.0;
                    }
                    if y > 0 {
                        add(&mut sum, other[(y - 1) * half + x / 2]);
                        n += 1.0;
                    }
                    if y + 1 < h {
                        add(&mut sum, other[(y + 1) * half + x / 2]);
                        n += 1.0;
                    }
                    let (old, f) = (*cell, rhs.map_or(ZERO, |r| r[y * half + j]));
                    *cell =
                        std::array::from_fn(|c| old[c] + OMEGA * ((sum[c] - f[c]) / n - old[c]));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gray of a premultiplied opaque pixel.
    fn gray(v: f32) -> Px {
        [v, v, v, 255.0]
    }

    fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Vec<bool> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                (x - cx).powi(2) + (y - cy).powi(2) < r * r
            })
            .collect()
    }

    /// The membrane between fixed values is harmonic: a linear function
    /// fixed on the border is filled in exactly, and the solver agrees
    /// with a long plain relaxation on an irregular area.
    #[test]
    fn membrane_is_harmonic() {
        let (w, h) = (70, 40);
        let linear = |i: usize| gray(20.0 + (i % w) as f32 * 2.5 - (i / w) as f32 * 1.5);
        let free: Vec<bool> = (0..w * h)
            .map(|i| (1..w - 1).contains(&(i % w)) && (1..h - 1).contains(&(i / w)))
            .collect();
        let mut v: Vec<Px> = (0..w * h)
            .map(|i| if free[i] { ZERO } else { linear(i) })
            .collect();
        membrane(&mut v, &free, w, h);
        for (i, p) in v.iter().enumerate() {
            assert!((p[0] - linear(i)[0]).abs() < 0.5, "{i}: {p:?}");
        }

        let (w, h) = (64, 48);
        let free = disc(w, h, 30.0, 22.0, 17.0);
        let fixed = |i: usize| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            gray(100.0 + 60.0 * (x / 9.0).sin() + y)
        };
        let start: Vec<Px> = (0..w * h)
            .map(|i| if free[i] { ZERO } else { fixed(i) })
            .collect();
        let mut fast = start.clone();
        membrane(&mut fast, &free, w, h);
        let mut exact = start;
        Checker::new(&exact, None, &free, w, h).relax(20_000, &mut exact);
        let worst = (0..w * h)
            .map(|i| (fast[i][0] - exact[i][0]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 1.0, "off by {worst}");
    }

    /// Healing a textured source onto a smooth, differently lit area: at
    /// the edge the result meets the destination; inside, it has the
    /// source's texture: their gradients differ by a smooth (harmonic)
    /// field, their Laplacians match.
    #[test]
    fn heal_is_seamless() {
        let (w, h) = (96, 80);
        let dest: Vec<Px> = (0..w * h)
            .map(|i| gray(60.0 + (i % w) as f32 * 0.8 + (i / w) as f32 * 0.3))
            .collect();
        let src: Vec<Px> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                gray(170.0 + 25.0 * (x / 3.0).sin() * (y / 4.0).cos())
            })
            .collect();
        let inside = disc(w, h, 48.0, 40.0, 30.0);
        let out = heal(&dest, &src, &inside, w, h);
        let at = |x: usize, y: usize| y * w + x;
        let laplacian = |v: &[Px], x: usize, y: usize| {
            v[at(x - 1, y)][0] + v[at(x + 1, y)][0] + v[at(x, y - 1)][0] + v[at(x, y + 1)][0]
                - 4.0 * v[at(x, y)][0]
        };
        let (mut edge, mut n_edge, mut residual, mut n_inner) = (0.0, 0, 0.0, 0);
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = at(x, y);
                if !inside[i] {
                    assert_eq!(out[i], dest[i]);
                    continue;
                }
                let neighbors = [at(x - 1, y), at(x + 1, y), at(x, y - 1), at(x, y + 1)];
                if neighbors.iter().any(|&j| !inside[j]) {
                    // On the edge: near the destination, not the source
                    // (about 110 apart).
                    edge += (out[i][0] - dest[i][0]).abs();
                    n_edge += 1;
                } else {
                    residual += (laplacian(&out, x, y) - laplacian(&src, x, y)).abs();
                    n_inner += 1;
                }
            }
        }
        let (edge, residual) = (edge / n_edge as f32, residual / n_inner as f32);
        assert!(edge < 12.0, "edge off by {edge} on average");
        assert!(residual < 0.2, "Laplacians off by {residual} on average");
        // The texture is there: not a smooth fill.
        let spread = |v: &[Px]| {
            let inner: Vec<f32> = (0..w * h).filter(|&i| inside[i]).map(|i| v[i][0]).collect();
            let mean = inner.iter().sum::<f32>() / inner.len() as f32;
            inner.iter().map(|v| (v - mean).abs()).sum::<f32>() / inner.len() as f32
        };
        assert!(spread(&out) > 0.6 * spread(&src));
    }
}
