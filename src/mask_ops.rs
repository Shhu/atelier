//! Selection masks (a coverage 0..255 per pixel) and what the Select menu
//! does to them, like Photoshop: Modify (border, smooth, expand, contract,
//! feather), Grow and Similar, Color Range, the refinements of Select and
//! Mask, the shapes of the marquees and the resampling of Transform
//! Selection. The work is shared between threads by bands of rows.

use std::sync::OnceLock;

use image::{GrayImage, RgbaImage, imageops};

use crate::document::PixelRect;
use crate::pixel_ops::distance_field;

/// Number of worker threads.
fn threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

/// Runs `f(first_row, band)` on bands of whole rows (`row` elements each)
/// of `data`, in parallel for large data.
pub fn par_rows<T: Send>(data: &mut [T], row: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / row.max(1);
    let n = threads().min(rows);
    if n <= 1 || data.len() < 1 << 15 {
        return f(0, data);
    }
    let per = rows.div_ceil(n);
    let f = &f;
    std::thread::scope(|s| {
        for (i, band) in data.chunks_mut(per * row).enumerate() {
            s.spawn(move || f(i * per, band));
        }
    });
}

/// Runs `f` on parts of `0..n` in parallel; their results, in order.
pub fn par_collect<T: Send>(
    n: usize,
    f: impl Fn(std::ops::Range<usize>) -> Vec<T> + Sync,
) -> Vec<T> {
    let parts = threads().min(n);
    if parts <= 1 {
        return f(0..n);
    }
    let per = n.div_ceil(parts);
    let f = &f;
    std::thread::scope(|s| {
        let parts: Vec<_> = (0..n)
            .step_by(per)
            .map(|a| s.spawn(move || f(a..(a + per).min(n))))
            .collect();
        parts
            .into_iter()
            .flat_map(|p| p.join().expect("a worker panicked"))
            .collect()
    })
}

// ------------------------------------------------------------ Modify

/// Select › Modify, sizes in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Modify {
    /// A band of this width, centered on the edge.
    Border(f32),
    /// Rounds the outline; what is smaller than this radius goes away.
    Smooth(f32),
    Expand(f32),
    Contract(f32),
    /// A soft edge of this radius.
    Feather(f32),
}

impl Modify {
    /// How far (px) from a pixel the result depends on the mask: the margin
    /// the processed area needs around the selection.
    pub fn reach(self) -> u32 {
        let r = match self {
            Modify::Border(w) => w / 2.0 + w / 4.0,
            Modify::Smooth(r) | Modify::Feather(r) => 1.5 * r,
            Modify::Expand(r) | Modify::Contract(r) => r,
        };
        r.ceil() as u32 + 2
    }
}

/// Applies `op` to `mask`, the area processed with its margin.
pub fn modify(mask: &mut GrayImage, op: Modify) {
    match op {
        Modify::Border(w) => border(mask, w),
        Modify::Smooth(r) => smooth(mask, r),
        Modify::Expand(r) => offset_edge(mask, r),
        Modify::Contract(r) => offset_edge(mask, -r),
        Modify::Feather(r) => feather(mask, r),
    }
}

/// Signed distance (px) from each pixel's center to the mask's 50 %
/// contour: positive inside, negative outside.
fn signed_distance(mask: &GrayImage) -> Vec<f32> {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let inside: Vec<bool> = mask.as_raw().iter().map(|&v| v >= 128).collect();
    let to_inside = distance_field(&inside, w, h);
    let outside: Vec<bool> = inside.iter().map(|&i| !i).collect();
    let mut s = distance_field(&outside, w, h);
    par_rows(&mut s, w, |y0, band| {
        let start = y0 * w;
        for (i, d) in band.iter_mut().enumerate() {
            let j = start + i;
            *d = if inside[j] {
                *d - 0.5
            } else {
                0.5 - to_inside[j]
            };
        }
    });
    s
}

/// Step of a [`Profile`] (px).
const STEP: f32 = 0.25;

/// Mean coverage of a mask by signed distance to its contour: how soft its
/// edge is (anti-aliased, feathered), to keep that softness when the
/// contour moves.
struct Profile {
    /// Distance of the first value: 0 there and below, the last value (255)
    /// beyond the end.
    from: f32,
    values: Vec<f32>,
}

impl Profile {
    fn of(mask: &[u8], s: &[f32]) -> Profile {
        // Soft pixels, and at least a pixel either side of the contour (a
        // hard edge then stays one anti-aliased pixel wide).
        let (mut lo, mut hi) = (-1.0f32, 1.0f32);
        for (&v, &d) in mask.iter().zip(s) {
            if v > 0 && v < 255 {
                lo = lo.min(d);
                hi = hi.max(d);
            }
        }
        let from = (lo / STEP).floor() * STEP - STEP;
        let n = ((hi - from) / STEP).ceil() as usize + 2;
        let end = from + (n - 1) as f32 * STEP;
        let (mut sum, mut count) = (vec![0.0f64; n], vec![0u32; n]);
        for (&v, &d) in mask.iter().zip(s) {
            if d >= from && d <= end {
                let i = ((d - from) / STEP).round() as usize;
                sum[i] += v as f64;
                count[i] += 1;
            }
        }
        let mut known: Vec<Option<f32>> = (0..n)
            .map(|i| (count[i] > 0).then(|| (sum[i] / count[i] as f64) as f32))
            .collect();
        known[0] = Some(0.0);
        known[n - 1] = Some(255.0);
        // Gaps: interpolated between their neighbors; then never less
        // covered further inside.
        let mut values = vec![0.0f32; n];
        let mut last = 0;
        for i in 1..n {
            if let Some(v) = known[i] {
                let a = known[last].unwrap_or(0.0);
                for (k, out) in values.iter_mut().enumerate().take(i + 1).skip(last) {
                    let t = (k - last) as f32 / (i - last) as f32;
                    *out = a + (v - a) * t;
                }
                last = i;
            }
        }
        for i in 1..n {
            values[i] = values[i].max(values[i - 1]);
        }
        Profile { from, values }
    }

    fn at(&self, d: f32) -> f32 {
        let t = (d - self.from) / STEP;
        let last = self.values.len() - 1;
        if t <= 0.0 {
            return self.values[0];
        }
        if t >= last as f32 {
            return self.values[last];
        }
        let i = t.floor() as usize;
        let f = t - i as f32;
        self.values[i] * (1.0 - f) + self.values[i + 1] * f
    }
}

/// Expand (`by` > 0) or Contract: the contour moves by `by` px, round at the
/// corners, and keeps the softness of the edge.
fn offset_edge(mask: &mut GrayImage, by: f32) {
    let s = signed_distance(mask);
    let profile = Profile::of(mask.as_raw(), &s);
    let w = mask.width() as usize;
    par_rows(mask.as_mut(), w, |y0, band| {
        let start = y0 * w;
        for (i, v) in band.iter_mut().enumerate() {
            let moved = profile.at(s[start + i] + by).round() as u8;
            // Never less selected when expanding, never more when contracting.
            *v = if by > 0.0 {
                (*v).max(moved)
            } else {
                (*v).min(moved)
            };
        }
    });
}

/// Border: a band of `width` px centered on the contour, with soft edges.
fn border(mask: &mut GrayImage, width: f32) {
    let s = signed_distance(mask);
    let half = width / 2.0;
    let ramp = (half / 2.0).max(1.0);
    let w = mask.width() as usize;
    par_rows(mask.as_mut(), w, |y0, band| {
        let start = y0 * w;
        for (i, v) in band.iter_mut().enumerate() {
            let c = ((half - s[start + i].abs()) / ramp + 0.5).clamp(0.0, 1.0);
            *v = (c * 255.0).round() as u8;
        }
    });
}

/// Smooth: each pixel takes what most of its neighbors within `radius` are
/// (a blur, then a sharp threshold at half), which rounds the corners and
/// removes specks and holes.
fn smooth(mask: &mut GrayImage, radius: f32) {
    let sigma = radius / 2.0;
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let mut v: Vec<f32> = mask.as_raw().iter().map(|&c| c as f32).collect();
    gaussian(&mut v, w, h, sigma);
    // Steepness for a one-pixel transition after the blur.
    let k = (2.5 * sigma).max(1.0);
    for (out, b) in mask.as_mut().iter_mut().zip(v) {
        *out = ((b - 127.5) * k + 127.5).clamp(0.0, 255.0).round() as u8;
    }
}

/// Feather: a Gaussian blur, the soft edge reaching about `radius` px either
/// side of the contour.
fn feather(mask: &mut GrayImage, radius: f32) {
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let mut v: Vec<f32> = mask.as_raw().iter().map(|&c| c as f32).collect();
    gaussian(&mut v, w, h, radius / 2.0);
    let out = mask.as_mut();
    par_rows(out, w, |y0, band| {
        let start = y0 * w;
        for (i, o) in band.iter_mut().enumerate() {
            *o = v[start + i].clamp(0.0, 255.0).round() as u8;
        }
    });
}

/// Gaussian blur of standard deviation `sigma` of a `w` × `h` field: an
/// exact kernel for small ones, three box blurs beyond. Edges repeat.
pub fn gaussian(values: &mut [f32], w: usize, h: usize, sigma: f32) {
    if sigma < 0.2 || w == 0 || h == 0 {
        return;
    }
    let pass = Pass::new(sigma);
    rows_pass(values, w, &pass);
    let mut columns = transpose(values, w, h);
    rows_pass(&mut columns, h, &pass);
    values.copy_from_slice(&transpose(&columns, h, w));
}

/// One direction of [`gaussian`].
enum Pass {
    Kernel(Vec<f32>),
    Boxes([usize; 3]),
}

impl Pass {
    fn new(sigma: f32) -> Pass {
        if sigma < 2.0 {
            let r = (3.0 * sigma).ceil() as i32;
            let mut k: Vec<f32> = (-r..=r)
                .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
                .collect();
            let total: f32 = k.iter().sum();
            k.iter_mut().for_each(|v| *v /= total);
            return Pass::Kernel(k);
        }
        // Three boxes whose variances add up to sigma² (W. Jarosz).
        let ideal = (12.0 * sigma * sigma / 3.0 + 1.0).sqrt();
        let mut lower = ideal.floor() as i64;
        if lower % 2 == 0 {
            lower -= 1;
        }
        let upper = lower + 2;
        let (l, s2) = (lower as f32, sigma * sigma);
        let m = ((12.0 * s2 - 3.0 * l * l - 12.0 * l - 9.0) / (-4.0 * l - 4.0)).round() as usize;
        Pass::Boxes([0, 1, 2].map(|i| {
            let width = if i < m { lower } else { upper };
            ((width - 1) / 2).max(0) as usize
        }))
    }

    fn line(&self, line: &mut [f32], tmp: &mut [f32]) {
        let n = line.len();
        let at = |line: &[f32], i: isize| line[i.clamp(0, n as isize - 1) as usize];
        match self {
            Pass::Kernel(k) => {
                let r = (k.len() / 2) as isize;
                for (i, out) in tmp.iter_mut().enumerate().take(n) {
                    *out = k
                        .iter()
                        .enumerate()
                        .map(|(j, w)| w * at(line, i as isize + j as isize - r))
                        .sum();
                }
                line.copy_from_slice(&tmp[..n]);
            }
            Pass::Boxes(radii) => {
                for &r in radii {
                    let r = r as isize;
                    let width = (2 * r + 1) as f32;
                    let mut sum: f32 = (-r..=r).map(|i| at(line, i)).sum();
                    for (i, out) in tmp.iter_mut().enumerate().take(n) {
                        *out = sum / width;
                        let i = i as isize;
                        sum += at(line, i + r + 1) - at(line, i - r);
                    }
                    line.copy_from_slice(&tmp[..n]);
                }
            }
        }
    }
}

fn rows_pass(values: &mut [f32], len: usize, pass: &Pass) {
    par_rows(values, len, |_, band| {
        let mut tmp = vec![0.0f32; len];
        for line in band.chunks_exact_mut(len) {
            pass.line(line, &mut tmp);
        }
    });
}

/// Transpose of a `w` × `h` grid, by blocks (to stay in cache), in parallel.
fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    const BLOCK: usize = 32;
    let mut dst = vec![0.0f32; w * h];
    par_rows(&mut dst, h, |x0, band| {
        let columns = band.len() / h.max(1);
        for by in (0..h).step_by(BLOCK) {
            for bx in (0..columns).step_by(BLOCK) {
                for x in bx..(bx + BLOCK).min(columns) {
                    for y in by..(by + BLOCK).min(h) {
                        band[x * h + y] = src[y * w + x0 + x];
                    }
                }
            }
        }
    });
    dst
}

// ------------------------------------------------------------ Grow, Similar

/// Select › Grow (`contiguous`) and Similar: the pixels of `image` whose
/// color is within `tolerance` of the range of colors selected in `selected`
/// (the same area) are added to it — touching the selection for Grow,
/// anywhere for Similar.
pub fn grow(image: &RgbaImage, selected: &GrayImage, tolerance: u8, contiguous: bool) -> GrayImage {
    let (w, h) = image.dimensions();
    // Range of the selected colors; transparent pixels have none.
    let (mut lo, mut hi) = ([255u8; 4], [0u8; 4]);
    let (mut opaque, mut clear) = (false, false);
    for (p, s) in image.pixels().zip(selected.pixels()) {
        if s.0[0] < 128 {
            continue;
        }
        if p.0[3] == 0 {
            clear = true;
            continue;
        }
        opaque = true;
        for c in 0..4 {
            lo[c] = lo[c].min(p.0[c]);
            hi[c] = hi[c].max(p.0[c]);
        }
    }
    let mut out = selected.clone();
    if !opaque && !clear {
        return out;
    }
    let close = |p: [u8; 4]| {
        if p[3] == 0 {
            return clear || (opaque && lo[3] <= tolerance);
        }
        (clear && p[3] <= tolerance)
            || (opaque
                && (0..4).all(|c| {
                    p[c] >= lo[c].saturating_sub(tolerance)
                        && p[c] <= hi[c].saturating_add(tolerance)
                }))
    };
    if !contiguous {
        let row = w as usize;
        par_rows(out.as_mut(), row, |y0, band| {
            for (i, v) in band.iter_mut().enumerate() {
                let (x, y) = ((i % row) as u32, (y0 + i / row) as u32);
                if close(image.get_pixel(x, y).0) {
                    *v = 255;
                }
            }
        });
        return out;
    }
    let mut stack: Vec<(u32, u32)> = selected
        .enumerate_pixels()
        .filter(|(_, _, s)| s.0[0] >= 128)
        .map(|(x, y, _)| (x, y))
        .collect();
    while let Some((x, y)) = stack.pop() {
        let neighbours = [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ];
        for (nx, ny) in neighbours {
            if nx < w
                && ny < h
                && out.get_pixel(nx, ny).0[0] < 128
                && close(image.get_pixel(nx, ny).0)
            {
                out.put_pixel(nx, ny, image::Luma([255]));
                stack.push((nx, ny));
            }
        }
    }
    out
}

// ------------------------------------------------------------ Color Range

/// CIE L*a*b* (D65) of an sRGB color.
pub fn lab([r, g, b]: [u8; 3]) -> [f32; 3] {
    static LINEAR: OnceLock<[f32; 256]> = OnceLock::new();
    let linear = LINEAR.get_or_init(|| {
        std::array::from_fn(|i| {
            let c = i as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    let (r, g, b) = (linear[r as usize], linear[g as usize], linear[b as usize]);
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f32| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// A color sampled by Color Range's eyedroppers: added to the range, or
/// taken out of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeSample {
    pub lab: [f32; 3],
    pub add: bool,
}

impl RangeSample {
    pub fn new(rgb: [u8; 3], add: bool) -> Self {
        Self { lab: lab(rgb), add }
    }
}

/// How much (0..1) a color belongs to the sampled range: fully within half
/// the fuzziness (in ΔE) of an added sample, then less and less up to the
/// fuzziness; the subtracted samples take their share off the same way.
pub fn range_coverage(lab: [f32; 3], samples: &[RangeSample], fuzziness: f32) -> f32 {
    let near = |s: &RangeSample| {
        let d = (0..3)
            .map(|i| (lab[i] - s.lab[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        if fuzziness <= 0.0 {
            f32::from(u8::from(d < 0.5))
        } else {
            ((fuzziness - d) / (fuzziness * 0.5)).clamp(0.0, 1.0)
        }
    };
    let most = |add: bool| {
        samples
            .iter()
            .filter(|s| s.add == add)
            .map(near)
            .fold(0.0, f32::max)
    };
    most(true) * (1.0 - most(false))
}

/// Select › Color Range over `image`: the coverage of each pixel (its
/// transparency counts).
pub fn color_range(image: &RgbaImage, samples: &[RangeSample], fuzziness: f32) -> GrayImage {
    let (w, h) = image.dimensions();
    let mut out = GrayImage::new(w, h);
    if samples.iter().all(|s| !s.add) {
        return out;
    }
    let row = w as usize;
    par_rows(out.as_mut(), row, |y0, band| {
        for (i, v) in band.iter_mut().enumerate() {
            let [r, g, b, a] = image.get_pixel((i % row) as u32, (y0 + i / row) as u32).0;
            if a > 0 {
                let c = range_coverage(lab([r, g, b]), samples, fuzziness) * a as f32;
                *v = c.round() as u8;
            }
        }
    });
    out
}

// ------------------------------------------------------------ Select and Mask

/// Global refinements of Select and Mask: Smooth (0..100), Feather (px),
/// Contrast (0..100 %), Shift Edge (−100..100 %).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Refine {
    pub smooth: f32,
    pub feather: f32,
    pub contrast: f32,
    pub shift: f32,
}

impl Default for Refine {
    fn default() -> Self {
        Self {
            smooth: 0.0,
            feather: 0.0,
            contrast: 0.0,
            shift: 0.0,
        }
    }
}

impl Refine {
    /// Smoothing radius (px) of Smooth's 0..100.
    fn smooth_radius(&self) -> f32 {
        self.smooth / 10.0
    }

    /// Margin (px) the refinements need around the selection.
    pub fn reach(&self) -> u32 {
        Modify::Smooth(self.smooth_radius())
            .reach()
            .max(Modify::Feather(self.feather).reach())
    }

    /// Contrast then Shift Edge, as a table: Shift Edge moves the contour
    /// within a soft edge (a positive one takes in more of it), Contrast
    /// makes it sharper around there.
    fn levels(&self) -> [u8; 256] {
        let mid = 127.5 * (1.0 - self.shift / 100.0);
        let steep = 1.0 - self.contrast / 100.0;
        std::array::from_fn(|i| {
            let v = i as f32;
            let shifted = if v <= mid {
                if mid <= 0.0 { 127.5 } else { 127.5 * v / mid }
            } else if mid >= 255.0 {
                127.5
            } else {
                127.5 + 127.5 * (v - mid) / (255.0 - mid)
            };
            let c = if steep <= 0.0 {
                if shifted >= 127.5 { 255.0 } else { 0.0 }
            } else {
                (shifted - 127.5) / steep + 127.5
            };
            // A pixel left out stays out, one fully in stays in.
            match i {
                0 => 0,
                255 => 255,
                _ => c.clamp(0.0, 255.0).round() as u8,
            }
        })
    }
}

/// Applies the refinements to `mask`, whose pixels are `scale` canvas
/// pixels wide (a reduced preview).
pub fn refine(mask: &mut GrayImage, r: &Refine, scale: f32) {
    if r.smooth > 0.0 {
        smooth(mask, r.smooth_radius() / scale);
    }
    if r.feather > 0.0 {
        feather(mask, r.feather / scale);
    }
    if r.shift != 0.0 || r.contrast > 0.0 {
        let lut = r.levels();
        let w = mask.width() as usize;
        par_rows(mask.as_mut(), w, |_, band| {
            band.iter_mut().for_each(|v| *v = lut[*v as usize])
        });
    }
}

// ------------------------------------------------------------ shapes

/// Coverage of the ellipse inscribed in a box of `size` (Elliptical
/// Marquee), over `window` of the box: anti-aliased by the distance to its
/// edge, or each pixel by its center.
pub fn ellipse(size: (u32, u32), window: PixelRect, antialias: bool) -> GrayImage {
    let mut out = GrayImage::new(window.w, window.h);
    let (a, b) = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
    let row = window.w as usize;
    par_rows(out.as_mut(), row, |y0, band| {
        for (i, v) in band.iter_mut().enumerate() {
            let dx = (window.x + (i % row) as i64) as f32 + 0.5 - a;
            let dy = (window.y + (y0 + i / row) as i64) as f32 + 0.5 - b;
            let f = (dx / a).powi(2) + (dy / b).powi(2);
            *v = if antialias {
                // Distance to the edge ≈ (f − 1) / |∇f|.
                let grad = 2.0 * ((dx / (a * a)).powi(2) + (dy / (b * b)).powi(2)).sqrt();
                let d = if grad > 1e-6 { (f - 1.0) / grad } else { -1.0 };
                ((0.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8
            } else if f <= 1.0 {
                255
            } else {
                0
            };
        }
    });
    out
}

/// Affine map of the canvas: (x, y) → (a·x + b·y + c, d·x + e·y + f).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [f32; 6]);

impl Affine {
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + b * y + c, d * x + e * y + f)
    }

    pub fn inverse(&self) -> Option<Affine> {
        let [a, b, c, d, e, f] = self.0;
        let det = a * e - b * d;
        if det.abs() < 1e-9 {
            return None;
        }
        let (ia, ib, id, ie) = (e / det, -b / det, -d / det, a / det);
        Some(Affine([
            ia,
            ib,
            -(ia * c + ib * f),
            id,
            ie,
            -(id * c + ie * f),
        ]))
    }
}

/// The coverage `src` (top-left corner at `origin`, canvas coordinates)
/// moved by the map whose inverse is `to_source`, over `area`; bilinear.
pub fn resample(
    src: &GrayImage,
    origin: (i64, i64),
    area: PixelRect,
    to_source: Affine,
) -> GrayImage {
    let mut out = GrayImage::new(area.w, area.h);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let at = |x: i64, y: i64| {
        if x < 0 || y < 0 || x >= sw || y >= sh {
            0.0
        } else {
            src.get_pixel(x as u32, y as u32).0[0] as f32
        }
    };
    let row = area.w as usize;
    par_rows(out.as_mut(), row, |y0, band| {
        for (i, v) in band.iter_mut().enumerate() {
            let x = area.x as f32 + (i % row) as f32 + 0.5;
            let y = area.y as f32 + (y0 + i / row) as f32 + 0.5;
            let (sx, sy) = to_source.apply(x, y);
            let (sx, sy) = (sx - origin.0 as f32 - 0.5, sy - origin.1 as f32 - 0.5);
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
            let bottom = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
            *v = (top * (1.0 - fy) + bottom * fy).round() as u8;
        }
    });
    out
}

// ------------------------------------------------------------ canvas changes

/// A canvas-sized mask after the canvas became the rectangle (x, y, w, h)
/// of the current one: cropped, or extended with unselected pixels.
pub fn recanvas(mask: &GrayImage, x: i64, y: i64, w: u32, h: u32) -> GrayImage {
    let mut out = GrayImage::new(w, h);
    imageops::replace(&mut out, mask, -x, -y);
    out
}

/// A canvas-sized mask after Image Rotation by `degrees` (clockwise) into a
/// canvas of `size`, around the centers.
pub fn rotate_canvas(mask: &GrayImage, degrees: f32, size: (u32, u32)) -> GrayImage {
    let quarter = degrees / 90.0;
    if (quarter - quarter.round()).abs() < 1e-4 {
        return match (quarter.round() as i64).rem_euclid(4) {
            0 => mask.clone(),
            1 => imageops::rotate90(mask),
            2 => imageops::rotate180(mask),
            _ => imageops::rotate270(mask),
        };
    }
    let (s, c) = degrees.to_radians().sin_cos();
    let (cx, cy) = (mask.width() as f32 / 2.0, mask.height() as f32 / 2.0);
    let (nx, ny) = (size.0 as f32 / 2.0, size.1 as f32 / 2.0);
    // New canvas point → old one: the inverse rotation around the centers.
    let back = Affine([c, s, cx - c * nx - s * ny, -s, c, cy + s * nx - c * ny]);
    let area = PixelRect {
        x: 0,
        y: 0,
        w: size.0,
        h: size.1,
    };
    resample(mask, (0, 0), area, back)
}

/// A canvas-sized mask after Flip Canvas.
pub fn flip(mask: &GrayImage, horizontal: bool) -> GrayImage {
    if horizontal {
        imageops::flip_horizontal(mask)
    } else {
        imageops::flip_vertical(mask)
    }
}

/// A canvas-sized mask after Image Size.
pub fn resize(mask: &GrayImage, w: u32, h: u32) -> GrayImage {
    imageops::resize(mask, w, h, imageops::FilterType::Triangle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    /// A `w` × `h` mask selecting the rectangle (x0, y0) – (x1, y1).
    fn rect_mask(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            Luma([if x >= x0 && x < x1 && y >= y0 && y < y1 {
                255
            } else {
                0
            }])
        })
    }

    fn at(m: &GrayImage, x: u32, y: u32) -> u8 {
        m.get_pixel(x, y).0[0]
    }

    fn total(m: &GrayImage) -> f64 {
        m.pixels().map(|p| p.0[0] as f64 / 255.0).sum()
    }

    #[test]
    fn expand_moves_straight_edges_and_rounds_corners() {
        let mut m = rect_mask(100, 100, 40, 40, 60, 60);
        modify(&mut m, Modify::Expand(10.0));
        // Straight edges: exactly 10 px further.
        assert_eq!((at(&m, 30, 50), at(&m, 29, 50)), (255, 0));
        assert_eq!((at(&m, 69, 50), at(&m, 70, 50)), (255, 0));
        assert_eq!((at(&m, 50, 30), at(&m, 50, 70)), (255, 0));
        // Corners: round, of radius 10 around the old corner.
        assert_eq!(at(&m, 31, 31), 0, "no square corner");
        assert_eq!(at(&m, 34, 34), 255);
        let corner = at(&m, 33, 32);
        assert!(corner > 0 && corner < 255, "anti-aliased: {corner}");
    }

    #[test]
    fn contract_keeps_square_corners_and_removes_thin_parts() {
        let mut m = rect_mask(100, 100, 20, 20, 80, 80);
        // A 4 px wide bar sticking out: thinner than the contraction.
        for y in 48..52 {
            for x in 80..95 {
                m.put_pixel(x, y, Luma([255]));
            }
        }
        modify(&mut m, Modify::Contract(5.0));
        assert_eq!((at(&m, 24, 50), at(&m, 25, 50)), (0, 255));
        assert_eq!((at(&m, 25, 25), at(&m, 74, 74)), (255, 255));
        assert_eq!((at(&m, 70, 50), at(&m, 85, 50)), (255, 0));
    }

    #[test]
    fn expand_then_contract_keeps_a_feathered_edge() {
        let mut m = rect_mask(200, 60, 50, 0, 150, 60);
        modify(&mut m, Modify::Feather(10.0));
        let before: Vec<u8> = (30..80).map(|x| at(&m, x, 30)).collect();
        assert!(before.iter().filter(|&&v| v > 10 && v < 245).count() > 8);
        modify(&mut m, Modify::Expand(6.0));
        // The same soft edge, 6 px to the left.
        for (i, x) in (30..74).enumerate() {
            let (was, now) = (before[i + 6] as i32, at(&m, x, 30) as i32);
            assert!((was - now).abs() <= 12, "x {x}: {now}, expected {was}");
        }
        modify(&mut m, Modify::Contract(6.0));
        for (i, x) in (30..80).enumerate() {
            let (was, now) = (before[i] as i32, at(&m, x, 30) as i32);
            assert!((was - now).abs() <= 12, "x {x}: {now}, expected {was}");
        }
    }

    #[test]
    fn feather_is_symmetric_and_keeps_the_area() {
        let mut m = rect_mask(120, 120, 30, 30, 90, 90);
        let area = total(&m);
        modify(&mut m, Modify::Feather(8.0));
        assert!((total(&m) - area).abs() / area < 0.01);
        assert!((128..=150).contains(&at(&m, 30, 60)), "{}", at(&m, 30, 60));
        assert_eq!(at(&m, 60, 60), 255);
        assert_eq!(at(&m, 5, 60), 0);
        assert!(at(&m, 25, 60).abs_diff(255 - at(&m, 34, 60)) <= 1);
        // Small radii use the exact kernel, the same way.
        let mut small = rect_mask(40, 40, 10, 10, 30, 30);
        modify(&mut small, Modify::Feather(1.0));
        assert!(at(&small, 9, 20) > 0 && at(&small, 9, 20) < 128);
    }

    #[test]
    fn border_selects_a_band_around_the_edge() {
        let mut m = rect_mask(100, 100, 20, 20, 80, 80);
        modify(&mut m, Modify::Border(10.0));
        assert_eq!(at(&m, 50, 50), 0, "inside left out");
        assert_eq!(at(&m, 5, 50), 0, "outside left out");
        assert_eq!((at(&m, 20, 50), at(&m, 19, 50)), (255, 255));
        // About 10 px wide in all.
        let width: f64 = (0..50).map(|x| at(&m, x, 50) as f64 / 255.0).sum();
        assert!((width - 10.0).abs() < 1.0, "{width}");
    }

    #[test]
    fn smooth_removes_specks_and_holes() {
        let mut m = rect_mask(100, 100, 20, 20, 80, 80);
        m.put_pixel(50, 50, Luma([0]));
        m.put_pixel(5, 5, Luma([255]));
        modify(&mut m, Modify::Smooth(4.0));
        assert_eq!((at(&m, 50, 50), at(&m, 5, 5)), (255, 0));
        assert_eq!((at(&m, 21, 50), at(&m, 18, 50)), (255, 0));
        assert!(at(&m, 20, 20) < 128, "corner rounded");
    }

    #[test]
    fn grow_follows_close_colors_and_similar_finds_them_anywhere() {
        // Left and right: two red areas apart, a blue band between them.
        let image = RgbaImage::from_fn(30, 10, |x, _| match x {
            0..10 => image::Rgba([200, 10, 10, 255]),
            10..20 => image::Rgba([10, 10, 200, 255]),
            _ => image::Rgba([210, 20, 15, 255]),
        });
        let sel = rect_mask(30, 10, 2, 2, 5, 5);
        let grown = grow(&image, &sel, 32, true);
        assert_eq!((at(&grown, 0, 0), at(&grown, 9, 9)), (255, 255));
        assert_eq!((at(&grown, 12, 5), at(&grown, 25, 5)), (0, 0));
        let similar = grow(&image, &sel, 32, false);
        assert_eq!((at(&similar, 12, 5), at(&similar, 25, 5)), (0, 255));
        // Tolerance 0: only the exact colors.
        assert_eq!(at(&grow(&image, &sel, 0, false), 25, 5), 0);
    }

    #[test]
    fn color_range_follows_fuzziness_and_subtracted_samples() {
        let red = RangeSample::new([220, 30, 30], true);
        let near = lab([200, 40, 40]);
        let far = lab([30, 30, 220]);
        assert_eq!(range_coverage(red.lab, &[red], 40.0), 1.0);
        assert_eq!(range_coverage(far, &[red], 40.0), 0.0);
        let partly = range_coverage(near, &[red], 15.0);
        assert!(partly > 0.0 && partly < 1.0, "{partly}");
        assert_eq!(range_coverage(near, &[red], 200.0), 1.0);
        assert_eq!(range_coverage(near, &[red], 0.0), 0.0);
        // Taken out again by a subtracted sample of that color.
        let out = RangeSample::new([200, 40, 40], false);
        assert_eq!(range_coverage(near, &[red, out], 40.0), 0.0);
        // Lab: white is L 100, black L 0.
        let white = lab([255, 255, 255]);
        assert!((white[0] - 100.0).abs() < 0.1 && white[1].abs() < 0.1 && white[2].abs() < 0.1);
        assert!(lab([0, 0, 0])[0].abs() < 0.01);
        // Over an image: transparent pixels count as unselected.
        let image = RgbaImage::from_fn(4, 1, |x, _| match x {
            0 => image::Rgba([220, 30, 30, 255]),
            1 => image::Rgba([220, 30, 30, 0]),
            2 => image::Rgba([220, 30, 30, 128]),
            _ => image::Rgba([0, 200, 0, 255]),
        });
        let m = color_range(&image, &[red], 40.0);
        assert_eq!(m.as_raw(), &[255, 0, 128, 0]);
    }

    #[test]
    fn contrast_and_shift_edge_reshape_a_soft_edge() {
        let ramp = || GrayImage::from_fn(256, 1, |x, _| Luma([x as u8]));
        let mut m = ramp();
        refine(
            &mut m,
            &Refine {
                contrast: 100.0,
                ..Default::default()
            },
            1.0,
        );
        assert_eq!((at(&m, 127, 0), at(&m, 128, 0)), (0, 255));
        let mut m = ramp();
        refine(
            &mut m,
            &Refine {
                shift: 50.0,
                ..Default::default()
            },
            1.0,
        );
        // The half-way point moved into the softer part.
        assert!(at(&m, 64, 0) >= 127 && at(&m, 60, 0) < 127);
        assert_eq!((at(&m, 0, 0), at(&m, 255, 0)), (0, 255));
        let mut m = ramp();
        refine(
            &mut m,
            &Refine {
                shift: -50.0,
                contrast: 100.0,
                ..Default::default()
            },
            1.0,
        );
        assert_eq!((at(&m, 190, 0), at(&m, 192, 0)), (0, 255));
    }

    #[test]
    fn ellipse_has_the_right_area_and_symmetry() {
        let whole = |w, h| PixelRect { x: 0, y: 0, w, h };
        let e = ellipse((80, 40), whole(80, 40), true);
        let expected = std::f64::consts::PI * 40.0 * 20.0;
        assert!(
            (total(&e) - expected).abs() / expected < 0.01,
            "{}",
            total(&e)
        );
        assert_eq!(at(&e, 40, 20), 255);
        assert_eq!(at(&e, 0, 0), 0);
        assert_eq!(at(&e, 0, 20), at(&e, 79, 20));
        assert_eq!(at(&e, 40, 0), at(&e, 40, 39));
        let hard = ellipse((80, 40), whole(80, 40), false);
        assert!(hard.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255));
        // A window of the box: the same pixels.
        let part = ellipse(
            (80, 40),
            PixelRect {
                x: 30,
                y: 10,
                w: 20,
                h: 5,
            },
            true,
        );
        assert_eq!(part.get_pixel(3, 2), e.get_pixel(33, 12));
    }

    #[test]
    fn resample_maps_through_the_inverse() {
        let src = rect_mask(10, 10, 0, 0, 10, 10);
        // Twice as large, moved to (100, 50).
        let to_source = Affine([2.0, 0.0, 100.0, 0.0, 2.0, 50.0]).inverse().unwrap();
        let area = PixelRect {
            x: 90,
            y: 40,
            w: 40,
            h: 40,
        };
        let out = resample(&src, (0, 0), area, to_source);
        assert_eq!((at(&out, 11, 11), at(&out, 28, 28)), (255, 255));
        assert_eq!((at(&out, 8, 20), at(&out, 31, 20)), (0, 0));
        let back = Affine([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let (x, y) = back
            .inverse()
            .unwrap()
            .apply(back.apply(7.0, -3.0).0, back.apply(7.0, -3.0).1);
        assert!((x - 7.0).abs() < 1e-3 && (y + 3.0).abs() < 1e-3);
    }

    #[test]
    fn channels_follow_the_canvas() {
        let m = rect_mask(40, 20, 0, 0, 10, 20);
        let r = recanvas(&m, -5, 0, 50, 20);
        assert_eq!((at(&r, 4, 5), at(&r, 5, 5), at(&r, 14, 5)), (0, 255, 255));
        let turned = rotate_canvas(&m, 90.0, (20, 40));
        assert_eq!(turned.dimensions(), (20, 40));
        assert_eq!((at(&turned, 5, 5), at(&turned, 5, 35)), (255, 0));
        assert_eq!(at(&flip(&m, true), 35, 5), 255);
        let half = rotate_canvas(&m, 90.0, (20, 40));
        let tilted = rotate_canvas(&m, 89.99, (20, 40));
        assert!(
            half.pixels()
                .zip(tilted.pixels())
                .all(|(a, b)| a.0[0].abs_diff(b.0[0]) < 40)
        );
        assert_eq!(resize(&m, 80, 40).dimensions(), (80, 40));
    }
}
