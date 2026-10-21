//! Content-Aware Fill: patch-based inpainting. The hole is filled with
//! pieces of the rest of the image so that every 7 × 7 patch around it
//! looks like a patch found elsewhere (Wexler, Shechtman and Irani's
//! completion), the nearest patches being found by PatchMatch (Barnes et
//! al.). Coarse to fine: solved on a reduced image first, whose answer
//! starts the next level.

use image::RgbaImage;

use super::Progress;
use crate::pixel_ops::threads;

/// Patch radius: 7 × 7 patches.
const R: i32 = 3;
const SIDE: i32 = 2 * R + 1;
/// Coarsest level: the hole fits in a few patches, or the image would be
/// too small for patches to mean anything.
const COARSE_HOLE: i32 = 2 * SIDE;
const COARSE_SIDE: i32 = 8 * SIDE;
const MAX_LEVELS: usize = 10;
/// The confidence in a hole pixel falls by this factor per pixel from its
/// edge: patches over known pixels weigh more in the vote.
const CONFIDENCE_DECAY: f32 = 1.3;

/// Premultiplied 8-bit color.
type Pix = [u8; 4];

/// Fills the pixels of `image` where `hole` (one flag per pixel) with
/// patches of the pixels where `allowed` (all others when `None`). `None`
/// when cancelled, or when no patch can be copied.
pub fn inpaint(
    image: &RgbaImage,
    hole: &[bool],
    allowed: Option<&[bool]>,
    progress: &Progress,
) -> Option<RgbaImage> {
    let (w, h) = (image.width() as i32, image.height() as i32);
    if !hole.contains(&true) {
        return Some(image.clone());
    }
    let px: Vec<Pix> = image
        .pixels()
        .zip(hole)
        .map(|(p, &hole)| if hole { [0; 4] } else { premultiply(p.0) })
        .collect();
    let allowed: Vec<bool> = match allowed {
        Some(a) => a.iter().zip(hole).map(|(&a, &h)| a && !h).collect(),
        None => hole.iter().map(|&h| !h).collect(),
    };
    let mut levels = vec![Level::new(w, h, px, hole.to_vec(), allowed)];
    loop {
        let last = levels.last().expect("one level");
        let [x0, y0, x1, y1] = last.hole_box();
        if levels.len() >= MAX_LEVELS
            || (x1 - x0).max(y1 - y0) <= COARSE_HOLE
            || last.w.min(last.h) / 2 < COARSE_SIDE
        {
            break;
        }
        let next = last.reduced();
        if next.sources.is_empty() {
            break;
        }
        levels.push(next);
    }
    if levels.last().is_none_or(|l| l.sources.is_empty()) {
        return None;
    }

    // Work of each level, for the progress.
    let schedule: Vec<usize> = (0..levels.len())
        .map(|i| em_iterations(levels.len() - 1 - i, i == levels.len() - 1))
        .collect();
    let total: f32 = levels
        .iter()
        .zip(&schedule)
        .map(|(l, &n)| l.targets().count as f32 * n as f32)
        .sum();
    let mut done = 0.0;

    let mut previous: Option<(Level, Field)> = None;
    while let Some(mut level) = levels.pop() {
        let depth = levels.len();
        let mut field = level.targets();
        match &previous {
            None => {
                level.onion_fill();
                field.randomize(&level, depth as u64);
            }
            Some((coarse, coarse_field)) => {
                level.upsample_hole(coarse);
                field.upsample(&level, coarse_field, depth as u64);
            }
        }
        let iterations = schedule[depth];
        for em in 0..iterations {
            if progress.cancelled() {
                return None;
            }
            field.measure(&level);
            let passes = if em == 0 { 4 } else { 2 };
            for pass in 0..passes {
                field.search(&level, pass, (depth * 64 + em * 8 + pass) as u64);
            }
            level.vote(&field);
            done += field.count as f32;
            progress.set(done / total);
        }
        previous = Some((level, field));
    }
    let (finest, _) = previous?;
    let mut out = image.clone();
    for (i, p) in out.pixels_mut().enumerate() {
        if hole[i] {
            p.0 = unpremultiply(finest.px[i]);
        }
    }
    Some(out)
}

/// Expectation-maximization iterations at a level `steps` finer than the
/// coarsest: many where it is cheap, few on the full image.
fn em_iterations(steps: usize, coarsest: bool) -> usize {
    if coarsest {
        12
    } else {
        6usize.saturating_sub(steps).max(2)
    }
}

fn premultiply(p: [u8; 4]) -> Pix {
    let a = p[3] as u32;
    let c = |v: u8| ((v as u32 * a + 127) / 255) as u8;
    [c(p[0]), c(p[1]), c(p[2]), p[3]]
}

fn unpremultiply(p: Pix) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0; 4];
    }
    let c = |v: u8| ((v as u32).min(a) * 255 + a / 2) / a;
    [c(p[0]) as u8, c(p[1]) as u8, c(p[2]) as u8, p[3]]
}

/// The image at one level of reduction.
struct Level {
    w: i32,
    h: i32,
    px: Vec<Pix>,
    hole: Vec<bool>,
    /// May be copied (known, within the sampling area).
    allowed: Vec<bool>,
    /// Centers of the patches that may be copied: whole, all allowed.
    valid: Vec<bool>,
    sources: Vec<u32>,
}

impl Level {
    fn new(w: i32, h: i32, px: Vec<Pix>, hole: Vec<bool>, allowed: Vec<bool>) -> Self {
        // Patches with no forbidden pixel, by a summed-area table.
        let (uw, uh) = (w as usize, h as usize);
        let mut sat = vec![0u32; (uw + 1) * (uh + 1)];
        for y in 0..uh {
            let mut row = 0;
            for x in 0..uw {
                row += u32::from(!allowed[y * uw + x]);
                sat[(y + 1) * (uw + 1) + x + 1] = sat[y * (uw + 1) + x + 1] + row;
            }
        }
        let mut valid = vec![false; uw * uh];
        let mut sources = Vec::new();
        for y in R..h - R {
            for x in R..w - R {
                let (x0, y0, x1, y1) = (
                    (x - R) as usize,
                    (y - R) as usize,
                    (x + R + 1) as usize,
                    (y + R + 1) as usize,
                );
                let bad = sat[y1 * (uw + 1) + x1] + sat[y0 * (uw + 1) + x0]
                    - sat[y0 * (uw + 1) + x1]
                    - sat[y1 * (uw + 1) + x0];
                if bad == 0 {
                    let i = (y * w + x) as usize;
                    valid[i] = true;
                    sources.push(i as u32);
                }
            }
        }
        Self {
            w,
            h,
            px,
            hole,
            allowed,
            valid,
            sources,
        }
    }

    /// Box of the hole: x0, y0, x1, y1 (exclusive).
    fn hole_box(&self) -> [i32; 4] {
        let mut b = [self.w, self.h, 0, 0];
        for (i, _) in self.hole.iter().enumerate().filter(|(_, h)| **h) {
            let (x, y) = (i as i32 % self.w, i as i32 / self.w);
            b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
        }
        b
    }

    /// Half the size: a pixel is in the hole if one of its four is, and
    /// may be copied if all of them may.
    fn reduced(&self) -> Level {
        let (w, h) = ((self.w + 1) / 2, (self.h + 1) / 2);
        let n = (w * h) as usize;
        let (mut px, mut hole, mut allowed) = (vec![[0; 4]; n], vec![false; n], vec![true; n]);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                let mut sum = [0u32; 4];
                let mut count = 0;
                for (cx, cy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (fx, fy) = (2 * x + cx, 2 * y + cy);
                    if fx >= self.w || fy >= self.h {
                        allowed[i] = false;
                        continue;
                    }
                    let j = (fy * self.w + fx) as usize;
                    hole[i] |= self.hole[j];
                    allowed[i] &= self.allowed[j];
                    if !self.hole[j] {
                        for (s, &v) in sum.iter_mut().zip(&self.px[j]) {
                            *s += v as u32;
                        }
                        count += 1;
                    }
                }
                if count > 0 && !hole[i] {
                    px[i] = sum.map(|v| ((v + count / 2) / count) as u8);
                }
            }
        }
        let allowed = allowed.iter().zip(&hole).map(|(&a, &h)| a && !h).collect();
        Level::new(w, h, px, hole, allowed)
    }

    /// First guess on the coarsest level: the hole filled from its edge
    /// inward, a ring at a time, each pixel the center of the patch that
    /// best matches what is already known around it.
    fn onion_fill(&mut self) {
        let (w, h) = (self.w, self.h);
        let mut known: Vec<bool> = self.hole.iter().map(|h| !h).collect();
        // Beyond so many comparisons, a sample of the sources.
        let holes = self.hole.iter().filter(|&&h| h).count();
        let step = (holes * self.sources.len() / 2_000_000).max(1);
        let candidates: Vec<u32> = self.sources.iter().step_by(step).copied().collect();
        loop {
            let front: Vec<usize> = (0..(w * h) as usize)
                .filter(|&i| {
                    let (x, y) = (i as i32 % w, i as i32 / w);
                    !known[i]
                        && (-1..=1).any(|dy| {
                            (-1..=1).any(|dx| {
                                let (nx, ny) = (x + dx, y + dy);
                                nx >= 0
                                    && ny >= 0
                                    && nx < w
                                    && ny < h
                                    && known[(ny * w + nx) as usize]
                            })
                        })
                })
                .collect();
            if front.is_empty() {
                break;
            }
            let mut picks = vec![[0u8; 4]; front.len()];
            // Threads for the larger rings only.
            let parts = (front.len() * candidates.len() / 100_000).clamp(1, threads());
            let per = front.len().div_ceil(parts);
            let (this, known_now, candidates) = (&*self, &known, &candidates);
            std::thread::scope(|scope| {
                for (out, part) in picks.chunks_mut(per).zip(front.chunks(per)) {
                    scope.spawn(move || {
                        for (slot, &i) in out.iter_mut().zip(part) {
                            let at = (i as i32 % w, i as i32 / w);
                            let mut best = (u32::MAX, candidates[0]);
                            for &s in candidates {
                                let d = this.known_distance(at, s, known_now, best.0);
                                if d < best.0 {
                                    best = (d, s);
                                }
                            }
                            *slot = this.px[best.1 as usize];
                        }
                    });
                }
            });
            for (&i, p) in front.iter().zip(picks) {
                self.px[i] = p;
                known[i] = true;
            }
        }
    }

    /// [`Self::distance`] over the pixels known so far only.
    fn known_distance(&self, (px, py): (i32, i32), s: u32, known: &[bool], limit: u32) -> u32 {
        let (sx, sy) = (s as i32 % self.w, s as i32 / self.w);
        let mut d = 0u32;
        for dy in -R..=R {
            let y = py + dy;
            if y < 0 || y >= self.h {
                continue;
            }
            for dx in -R..=R {
                let x = px + dx;
                let i = (y * self.w + x) as usize;
                if x < 0 || x >= self.w || !known[i] {
                    continue;
                }
                let (a, b) = (self.px[i], self.px[((sy + dy) * self.w + sx + dx) as usize]);
                for c in 0..4 {
                    let e = a[c] as i32 - b[c] as i32;
                    d += (e * e) as u32;
                }
            }
            if d >= limit {
                return d;
            }
        }
        d
    }

    /// The hole of this level from the solution of the coarser one.
    fn upsample_hole(&mut self, coarse: &Level) {
        for y in 0..self.h {
            for x in 0..self.w {
                let i = (y * self.w + x) as usize;
                if self.hole[i] {
                    self.px[i] = coarse.px[((y / 2) * coarse.w + x / 2) as usize];
                }
            }
        }
    }

    fn random_source(&self, rng: &mut Rng) -> u32 {
        self.sources[rng.below(self.sources.len() as u32) as usize]
    }

    /// Squared difference between the patches centered on `p` and `s` (a
    /// valid source), over the pixels of `p`'s patch within the image;
    /// stops once over `limit`.
    fn distance(&self, (px, py): (i32, i32), s: u32, limit: u32) -> u32 {
        let (sx, sy) = (s as i32 % self.w, s as i32 / self.w);
        let mut d = 0u32;
        for dy in -R..=R {
            let y = py + dy;
            if y < 0 || y >= self.h {
                continue;
            }
            let (row, srow) = (y * self.w, (sy + dy) * self.w);
            for dx in -R..=R {
                let x = px + dx;
                if x < 0 || x >= self.w {
                    continue;
                }
                let a = self.px[(row + x) as usize];
                let b = self.px[(srow + sx + dx) as usize];
                for c in 0..4 {
                    let e = a[c] as i32 - b[c] as i32;
                    d += (e * e) as u32;
                }
            }
            if d >= limit {
                return d;
            }
        }
        d
    }

    /// Each hole pixel becomes the weighted mean of what the patches over
    /// it would put there; patches matching better weigh more.
    fn vote(&mut self, field: &Field) {
        let [x0, y0, x1, y1] = field.hole;
        let bw = (x1 - x0) as usize;
        if bw == 0 {
            return;
        }
        // Weight exp(−d / 2σ²), σ² the typical distance per pixel.
        let mut typical: Vec<f32> = field
            .cells
            .iter()
            .filter(|c| c.target)
            .map(Cell::per_pixel)
            .collect();
        let k = (typical.len() * 3 / 4).min(typical.len().saturating_sub(1));
        let sigma2 = if typical.is_empty() {
            1.0
        } else {
            *typical.select_nth_unstable_by(k, f32::total_cmp).1 + 1.0
        };
        let mut out = vec![[0u8; 4]; bw * (y1 - y0) as usize];
        let this = &*self;
        crate::filters::par_rows(&mut out, bw, |first, band| {
            for (r, row) in band.chunks_exact_mut(bw).enumerate() {
                let y = y0 + (first + r) as i32;
                for (k, slot) in row.iter_mut().enumerate() {
                    let x = x0 + k as i32;
                    let i = (y * this.w + x) as usize;
                    if !this.hole[i] {
                        continue;
                    }
                    let (mut sum, mut total) = ([0.0f32; 4], 0.0f32);
                    for py in y - R..=y + R {
                        for px in x - R..=x + R {
                            let Some(cell) = field.at(px, py).filter(|c| c.target) else {
                                continue;
                            };
                            let weight = (-cell.per_pixel() / (2.0 * sigma2)).exp().max(1e-6)
                                * cell.confidence;
                            let (sx, sy) =
                                (cell.source as i32 % this.w, cell.source as i32 / this.w);
                            let p = this.px[((sy + y - py) * this.w + sx + x - px) as usize];
                            for c in 0..4 {
                                sum[c] += p[c] as f32 * weight;
                            }
                            total += weight;
                        }
                    }
                    if total > 0.0 {
                        *slot = sum.map(|v| (v / total).round().clamp(0.0, 255.0) as u8);
                    }
                }
            }
        });
        for (r, row) in out.chunks_exact(bw).enumerate() {
            let y = y0 + r as i32;
            for (k, &p) in row.iter().enumerate() {
                let i = (y * self.w + x0 + k as i32) as usize;
                if self.hole[i] {
                    self.px[i] = p;
                }
            }
        }
    }
}

/// A patch around the hole and its nearest patch found so far.
#[derive(Clone, Copy, Default)]
struct Cell {
    /// Its patch overlaps the hole.
    target: bool,
    source: u32,
    distance: u32,
    /// Pixels of the patch within the image.
    pixels: u32,
    /// Mean confidence of its pixels (see [`CONFIDENCE_DECAY`]).
    confidence: f32,
}

impl Cell {
    fn per_pixel(&self) -> f32 {
        self.distance as f32 / self.pixels.max(1) as f32
    }
}

/// Nearest-neighbor field over the box of the patches overlapping the hole.
struct Field {
    /// Box of the cells (centers), x0, y0, x1, y1.
    rect: [i32; 4],
    /// Box of the hole.
    hole: [i32; 4],
    cells: Vec<Cell>,
    count: usize,
}

impl Level {
    /// Confidence of each pixel: 1 if known, less and less into the hole.
    fn confidence(&self) -> Vec<f32> {
        let (w, h) = (self.w as usize, self.h as usize);
        // Chamfer distance to the nearest known pixel, two passes.
        let mut d: Vec<f32> = self
            .hole
            .iter()
            .map(|&h| if h { f32::MAX } else { 0.0 })
            .collect();
        let diagonal = std::f32::consts::SQRT_2;
        for y in 0..h {
            for x in 0..w {
                let mut v = d[y * w + x];
                if x > 0 {
                    v = v.min(d[y * w + x - 1] + 1.0);
                }
                if y > 0 {
                    v = v.min(d[(y - 1) * w + x] + 1.0);
                    if x > 0 {
                        v = v.min(d[(y - 1) * w + x - 1] + diagonal);
                    }
                    if x + 1 < w {
                        v = v.min(d[(y - 1) * w + x + 1] + diagonal);
                    }
                }
                d[y * w + x] = v;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let mut v = d[y * w + x];
                if x + 1 < w {
                    v = v.min(d[y * w + x + 1] + 1.0);
                }
                if y + 1 < h {
                    v = v.min(d[(y + 1) * w + x] + 1.0);
                    if x + 1 < w {
                        v = v.min(d[(y + 1) * w + x + 1] + diagonal);
                    }
                    if x > 0 {
                        v = v.min(d[(y + 1) * w + x - 1] + diagonal);
                    }
                }
                d[y * w + x] = v;
            }
        }
        d.iter().map(|&d| CONFIDENCE_DECAY.powf(-d)).collect()
    }

    /// The patches overlapping the hole, not matched yet.
    fn targets(&self) -> Field {
        let hole = self.hole_box();
        let rect = [
            (hole[0] - R).max(0),
            (hole[1] - R).max(0),
            (hole[2] + R).min(self.w),
            (hole[3] + R).min(self.h),
        ];
        let (bw, bh) = (
            (rect[2] - rect[0]).max(0) as usize,
            (rect[3] - rect[1]).max(0) as usize,
        );
        let mut cells = vec![Cell::default(); bw * bh];
        let mut count = 0;
        for (i, _) in self.hole.iter().enumerate().filter(|(_, h)| **h) {
            let (x, y) = (i as i32 % self.w, i as i32 / self.w);
            for cy in (y - R).max(rect[1])..(y + R + 1).min(rect[3]) {
                for cx in (x - R).max(rect[0])..(x + R + 1).min(rect[2]) {
                    let cell = &mut cells[(cy - rect[1]) as usize * bw + (cx - rect[0]) as usize];
                    if !cell.target {
                        cell.target = true;
                        let span =
                            |c: i32, n: i32| ((c + R).min(n - 1) - (c - R).max(0) + 1) as u32;
                        cell.pixels = span(cx, self.w) * span(cy, self.h);
                        count += 1;
                    }
                }
            }
        }
        let confidence = self.confidence();
        let width = bw.max(1);
        for (i, cell) in cells.iter_mut().enumerate().filter(|(_, c)| c.target) {
            let (cx, cy) = (rect[0] + (i % width) as i32, rect[1] + (i / width) as i32);
            let mut sum = 0.0;
            for y in (cy - R).max(0)..(cy + R + 1).min(self.h) {
                for x in (cx - R).max(0)..(cx + R + 1).min(self.w) {
                    sum += confidence[(y * self.w + x) as usize];
                }
            }
            cell.confidence = sum / cell.pixels as f32;
        }
        Field {
            rect,
            hole,
            cells,
            count,
        }
    }
}

impl Field {
    fn width(&self) -> usize {
        (self.rect[2] - self.rect[0]) as usize
    }

    fn at(&self, x: i32, y: i32) -> Option<&Cell> {
        let [x0, y0, x1, y1] = self.rect;
        (x >= x0 && y >= y0 && x < x1 && y < y1)
            .then(|| &self.cells[(y - y0) as usize * self.width() + (x - x0) as usize])
    }

    fn randomize(&mut self, level: &Level, seed: u64) {
        let mut rng = Rng::new(seed);
        for cell in self.cells.iter_mut().filter(|c| c.target) {
            cell.source = level.random_source(&mut rng);
        }
    }

    /// Each patch starts from where the coarser level's patch over it
    /// pointed, scaled.
    fn upsample(&mut self, level: &Level, coarse: &Field, seed: u64) {
        let mut rng = Rng::new(seed);
        let [x0, y0, ..] = self.rect;
        let width = self.width();
        for (i, cell) in self.cells.iter_mut().enumerate().filter(|(_, c)| c.target) {
            let (x, y) = (x0 + (i % width) as i32, y0 + (i / width) as i32);
            let parent = coarse.at(x / 2, y / 2).filter(|c| c.target);
            let guess = parent.and_then(|c| {
                let cw = (level.w + 1) / 2;
                let (sx, sy) = (c.source as i32 % cw, c.source as i32 / cw);
                let (sx, sy) = (2 * sx + (x & 1), 2 * sy + (y & 1));
                let s = (sy * level.w + sx) as usize;
                (sx < level.w && sy < level.h && level.valid[s]).then_some(s as u32)
            });
            cell.source = guess.unwrap_or_else(|| level.random_source(&mut rng));
        }
    }

    /// Distances of the current matches (the hole changed).
    fn measure(&mut self, level: &Level) {
        let [x0, y0, ..] = self.rect;
        let width = self.width();
        crate::filters::par_rows(&mut self.cells, width.max(1), |first, band| {
            for (k, cell) in band.iter_mut().enumerate().filter(|(_, c)| c.target) {
                let (x, y) = (x0 + (k % width) as i32, y0 + (first + k / width) as i32);
                cell.distance = level.distance((x, y), cell.source, u32::MAX);
            }
        });
    }

    /// A PatchMatch pass: each patch tries its neighbors' matches, shifted
    /// (propagation), then random ones around its own, nearer and nearer.
    /// Bands of rows run in parallel; their edges move from one pass to
    /// the next, and every other pass scans backward.
    fn search(&mut self, level: &Level, pass: usize, seed: u64) {
        let [x0, y0, ..] = self.rect;
        let width = self.width();
        if width == 0 {
            return;
        }
        let forward = pass.is_multiple_of(2);
        let reach = level.w.max(level.h);
        let w = level.w;
        bands(&mut self.cells, width, pass, |first, band| {
            let mut rng = Rng::new(seed ^ ((first as u64 + 1) << 20));
            let rows = band.len() / width;
            for step in 0..band.len() {
                let k = if forward { step } else { band.len() - 1 - step };
                if !band[k].target {
                    continue;
                }
                let (bx, by) = (k % width, k / width);
                let (x, y) = (x0 + bx as i32, y0 + (first + by) as i32);
                let mut best = band[k];
                let consider = |s: u32, best: &mut Cell| {
                    if s != best.source {
                        let d = level.distance((x, y), s, best.distance);
                        if d < best.distance {
                            (best.source, best.distance) = (s, d);
                        }
                    }
                };
                // Propagation, from the neighbors already visited.
                let (dx, dy): (i32, i32) = if forward { (-1, -1) } else { (1, 1) };
                let left = (bx as i32 + dx >= 0 && (bx as i32 + dx) < width as i32)
                    .then(|| (k as i32 + dx) as usize);
                let up = (by as i32 + dy >= 0 && (by as i32 + dy) < rows as i32)
                    .then(|| (k as i32 + dy * width as i32) as usize);
                for (n, shift) in [(left, -dx), (up, -dy * w)] {
                    if let Some(n) = n.filter(|&n| band[n].target) {
                        let s = band[n].source as i32 + shift;
                        let (sx, sy) = (s.rem_euclid(w), s.div_euclid(w));
                        if s >= 0
                            && sy < level.h
                            && (sx - band[n].source as i32 % w).abs() <= 1
                            && level.valid[s as usize]
                        {
                            consider(s as u32, &mut best);
                        }
                    }
                }
                // Random search around the best, then one anywhere.
                let mut radius = reach;
                while radius >= 1 {
                    let (sx, sy) = (best.source as i32 % w, best.source as i32 / w);
                    let cx = (sx + rng.span(radius)).clamp(R, w - 1 - R);
                    let cy = (sy + rng.span(radius)).clamp(R, level.h - 1 - R);
                    let s = (cy * w + cx) as usize;
                    if level.valid[s] {
                        consider(s as u32, &mut best);
                    }
                    radius /= 2;
                }
                consider(level.random_source(&mut rng), &mut best);
                band[k] = best;
            }
        });
    }
}

/// Runs `f` on bands of rows (`row` cells each) in parallel; band edges
/// depend on `pass`. `f` gets the band's first row.
fn bands<T: Send>(data: &mut [T], row: usize, pass: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / row;
    let n = threads().min(rows / 8).max(1);
    if n == 1 {
        return f(0, data);
    }
    let per = rows.div_ceil(n);
    // The first band shrinks by half a band every other pass.
    let first = if pass % 4 >= 2 { per / 2 } else { per }.max(1);
    let f = &f;
    std::thread::scope(|s| {
        let (head, mut rest) = data.split_at_mut(first.min(rows) * row);
        s.spawn(move || f(0, head));
        let mut start = first.min(rows);
        while !rest.is_empty() {
            let take = (per * row).min(rest.len());
            let (band, tail) = rest.split_at_mut(take);
            let at = start;
            s.spawn(move || f(at, band));
            start += per;
            rest = tail;
        }
    });
}

/// Xorshift: fast, reproducible random numbers.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// 0..n.
    fn below(&mut self, n: u32) -> u32 {
        (((self.next() >> 32) * n as u64) >> 32) as u32
    }

    /// −r..=r.
    fn span(&mut self, r: i32) -> i32 {
        self.below(2 * r as u32 + 1) as i32 - r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A periodic texture: diagonal stripes and dots, period 12 × 10.
    fn texture(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let stripe = if (x + y) % 12 < 5 { 200 } else { 60 };
            let dot = if x % 12 == 3 && y % 10 < 3 { 255 } else { 0 };
            image::Rgba([stripe, (stripe / 2).max(dot), 120 + (y % 10) as u8 * 8, 255])
        })
    }

    fn error(a: &RgbaImage, b: &RgbaImage, hole: &[bool]) -> f32 {
        let (mut sum, mut n) = (0.0, 0.0);
        for ((p, q), &h) in a.pixels().zip(b.pixels()).zip(hole) {
            if h {
                sum += (0..3).map(|c| p.0[c].abs_diff(q.0[c]) as f32).sum::<f32>() / 3.0;
                n += 1.0;
            }
        }
        sum / n
    }

    /// A hole in a periodic texture is filled with the texture, in phase.
    #[test]
    fn fills_a_periodic_texture() {
        let (w, h) = (160, 120);
        let truth = texture(w, h);
        let hole: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                (55..95).contains(&x) && (40..78).contains(&y)
            })
            .collect();
        let mut damaged = truth.clone();
        for (p, &h) in damaged.pixels_mut().zip(&hole) {
            if h {
                p.0 = [255, 0, 255, 255];
            }
        }
        let t = std::time::Instant::now();
        let filled = inpaint(&damaged, &hole, None, &Progress::default()).unwrap();
        eprintln!("filled in {:?}", t.elapsed());
        let e = error(&filled, &truth, &hole);
        // A blend of the two stripe colors would be off by about 50.
        assert!(e < 8.0, "mean error {e}");
        // Outside the hole, nothing changed.
        for ((p, q), &h) in filled.pixels().zip(truth.pixels()).zip(&hole) {
            if !h {
                assert_eq!(p, q);
            }
        }
    }

    /// Only the allowed area is copied: a hole in a gray band, next to a
    /// red area that is not allowed, is filled with gray.
    #[test]
    fn copies_only_from_the_allowed_area() {
        let (w, h) = (90, 60);
        let image = RgbaImage::from_fn(w, h, |x, _| {
            if x < 45 {
                image::Rgba([128, 128, 128, 255])
            } else {
                image::Rgba([230, 20, 20, 255])
            }
        });
        let hole: Vec<bool> = (0..w * h)
            .map(|i| (20..30).contains(&(i % w)) && (20..30).contains(&(i / w)))
            .collect();
        let allowed: Vec<bool> = (0..w * h).map(|i| i % w < 45).collect();
        let filled = inpaint(&image, &hole, Some(&allowed), &Progress::default()).unwrap();
        for (p, &h) in filled.pixels().zip(&hole) {
            if h {
                assert_eq!(p.0, [128, 128, 128, 255]);
            }
        }
        // Nothing allowed: nothing to copy.
        let none = vec![false; (w * h) as usize];
        assert!(inpaint(&image, &hole, Some(&none), &Progress::default()).is_none());
    }

    #[test]
    fn cancel_stops() {
        let image = texture(100, 100);
        let hole: Vec<bool> = (0..100 * 100)
            .map(|i| (30..70).contains(&(i % 100)) && (30..70).contains(&(i / 100)))
            .collect();
        let progress = Progress::default();
        progress.cancel();
        assert!(inpaint(&image, &hole, None, &progress).is_none());
    }
}
