//! Filter › Distort: every result pixel takes its color at a computed place
//! of the source, read with bilinear interpolation, premultiplied by alpha.
//! They work on the whole processed area (the selection's box, or the layer):
//! the radial ones fit an ellipse in it, and pixels outside the ellipse don't
//! change.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use super::{Filter, Undefined, cancel_flag, cancelled, par_rows};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RippleSize {
    Small,
    Medium,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WaveType {
    Sine,
    Triangle,
    Square,
}

/// Wave. Lengths in pixels, scales in %.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Wave {
    pub generators: f32,
    pub wavelength_min: f32,
    pub wavelength_max: f32,
    pub amplitude_min: f32,
    pub amplitude_max: f32,
    pub scale_horizontal: f32,
    pub scale_vertical: f32,
    pub wave_type: WaveType,
    pub undefined: Undefined,
    pub seed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpherizeMode {
    Normal,
    HorizontalOnly,
    VerticalOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PolarConversion {
    RectangularToPolar,
    PolarToRectangular,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ZigZagStyle {
    AroundCenter,
    OutFromCenter,
    PondRipples,
}

/// Most points of a Shear curve.
pub const SHEAR_POINTS: usize = 8;

/// Shear: a curve from the top of the image to its bottom, by which rows
/// shift. The array keeps [`Filter`](super::Filter) `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shear {
    /// [shift, height], the first `count` in use, from the top down: the
    /// shift from −1 to 1 (half the width to the left or to the right), the
    /// height from 0 (top) to 1 (bottom).
    pub points: [[f32; 2]; SHEAR_POINTS],
    pub count: u8,
    pub undefined: Undefined,
}

impl Shear {
    /// A straight line: nothing shifts.
    pub fn straight(undefined: Undefined) -> Self {
        let mut points = [[0.0; 2]; SHEAR_POINTS];
        points[1] = [0.0, 1.0];
        Self {
            points,
            count: 2,
            undefined,
        }
    }

    /// The points in use.
    pub fn used(&self) -> &[[f32; 2]] {
        &self.points[..(self.count as usize).clamp(2, SHEAR_POINTS)]
    }

    /// A new point, its index; `None` if there are as many as can be, or if
    /// it would not lie strictly between two others.
    pub fn insert(&mut self, shift: f32, height: f32) -> Option<usize> {
        let n = self.used().len();
        if n >= SHEAR_POINTS {
            return None;
        }
        let at = self.used().iter().position(|p| p[1] >= height)?;
        if at == 0 || self.points[at][1] == height {
            return None;
        }
        self.points.copy_within(at..n, at + 1);
        self.points[at] = [shift.clamp(-1.0, 1.0), height];
        self.count = n as u8 + 1;
        Some(at)
    }

    /// Moves point `i`: the two ends stay at the top and the bottom, the
    /// others between their neighbors.
    pub fn place(&mut self, i: usize, shift: f32, height: f32) {
        let n = self.used().len();
        if i >= n {
            return;
        }
        self.points[i][0] = shift.clamp(-1.0, 1.0);
        if i > 0 && i + 1 < n {
            let gap = 0.01;
            let (low, high) = (self.points[i - 1][1] + gap, self.points[i + 1][1] - gap);
            self.points[i][1] = if low < high {
                height.clamp(low, high)
            } else {
                (low + high) / 2.0
            };
        }
    }

    /// Removes point `i`, unless it is one of the two ends.
    pub fn remove(&mut self, i: usize) {
        let n = self.used().len();
        if i > 0 && i + 1 < n {
            self.points.copy_within(i + 1..n, i);
            self.count = n as u8 - 1;
        }
    }

    /// The shift at `height` (0 to 1), a smooth curve through the points.
    pub fn shift_at(&self, height: f32) -> f32 {
        let p = self.used();
        let height = height.clamp(0.0, 1.0);
        let i = p
            .windows(2)
            .position(|s| height <= s[1][1])
            .unwrap_or(p.len() - 2);
        let slope = |a: [f32; 2], b: [f32; 2]| (b[0] - a[0]) / (b[1] - a[1]).max(1e-4);
        let (a, b) = (p[i], p[i + 1]);
        let before = slope(p[i.saturating_sub(1)], b);
        let after = slope(a, p[(i + 2).min(p.len() - 1)]);
        let span = (b[1] - a[1]).max(1e-4);
        let t = ((height - a[1]) / span).clamp(0.0, 1.0);
        let (t2, t3) = (t * t, t * t * t);
        let shift = (2.0 * t3 - 3.0 * t2 + 1.0) * a[0]
            + (t3 - 2.0 * t2 + t) * span * before
            + (-2.0 * t3 + 3.0 * t2) * b[0]
            + (t3 - t2) * span * after;
        shift.clamp(-1.0, 1.0)
    }
}

/// Most samples of a result pixel along each axis.
const MOST_SAMPLES: usize = 4;

/// Samples along an axis of a pixel covering `span` source pixels: more than
/// one only where the source is compressed (thin lines would break up).
#[inline]
fn samples(span: f32) -> usize {
    if span <= 1.5 {
        1
    } else {
        (span.ceil() as usize).min(MOST_SAMPLES)
    }
}

/// `region` (x0, y0, x1, y1) of the result: every pixel takes its color at
/// the place (in pixels, the top-left corner of the image being 0, 0) that
/// `place` gives for the middle of the pixel; `None`: it stays as it is. Only
/// the region is computed, and each of its pixels is what it is in the whole
/// result. Stops early once cancelled.
fn remap(
    src: &RgbaImage,
    region: [u32; 4],
    edges: (Undefined, Undefined),
    place: impl Fn(f32, f32) -> Option<(f32, f32)> + Sync,
) -> RgbaImage {
    let [x0, y0, x1, y1] = region;
    let mut out = image::imageops::crop_imm(src, x0, y0, x1 - x0, y1 - y0).to_image();
    let (w, h) = src.dimensions();
    let (raw, size) = (src.as_raw().as_slice(), (w as i64, h as i64));
    let row = (x1 - x0) as usize * 4;
    let count = (x1 - x0) as usize;
    if row == 0 {
        return out;
    }
    // Where a row of pixels reads, one more on the right, to tell how much
    // of the source each pixel covers.
    let read = |y: u32, into: &mut Vec<Option<(f32, f32)>>| {
        into.clear();
        if y >= h {
            into.resize(count + 1, None);
            return;
        }
        let cy = y as f32 + 0.5;
        into.extend((x0..x1).map(|x| place(x as f32 + 0.5, cy)));
        into.push((x1 < w).then(|| place(x1 as f32 + 0.5, cy)).flatten());
    };
    par_rows(out.as_mut(), row, |first, band| {
        let cancel = cancel_flag();
        let (mut here, mut below) = (Vec::with_capacity(count + 1), Vec::with_capacity(count + 1));
        read(y0 + first as u32, &mut here);
        for (i, line) in band.chunks_exact_mut(row).enumerate() {
            if cancelled(&cancel) {
                return;
            }
            let y = y0 + (first + i) as u32;
            read(y + 1, &mut below);
            for (j, d) in line.chunks_exact_mut(4).enumerate() {
                let Some((sx, sy)) = here[j] else { continue };
                let span = |next: Option<(f32, f32)>| {
                    next.map_or(0.0, |(nx, ny): (f32, f32)| (nx - sx).hypot(ny - sy))
                };
                let (across, down) = (samples(span(here[j + 1])), samples(span(below[j])));
                let c = if across == 1 && down == 1 {
                    bilinear(raw, size, edges, sx - 0.5, sy - 0.5)
                } else {
                    let (cx, cy) = ((x0 as usize + j) as f32, y as f32);
                    averaged(raw, size, edges, (across, down), |a, b| {
                        let (px, py) = (cx + a, cy + b);
                        place(px, py).unwrap_or((px, py))
                    })
                };
                d.copy_from_slice(&c);
            }
            std::mem::swap(&mut here, &mut below);
        }
    });
    out
}

/// The average, premultiplied, of `across` × `down` samples spread over a
/// pixel: `place` gets the place inside it (0 to 1 on each axis).
fn averaged(
    raw: &[u8],
    size: (i64, i64),
    edges: (Undefined, Undefined),
    (across, down): (usize, usize),
    place: impl Fn(f32, f32) -> (f32, f32),
) -> [u8; 4] {
    let mut sum = [0.0f32; 4];
    for b in 0..down {
        let v = (b as f32 + 0.5) / down as f32;
        for a in 0..across {
            let (sx, sy) = place((a as f32 + 0.5) / across as f32, v);
            let c = bilinear(raw, size, edges, sx - 0.5, sy - 0.5);
            let alpha = f32::from(c[3]);
            sum[0] += f32::from(c[0]) * alpha;
            sum[1] += f32::from(c[1]) * alpha;
            sum[2] += f32::from(c[2]) * alpha;
            sum[3] += alpha;
        }
    }
    if sum[3] <= 0.0 {
        return [0; 4];
    }
    let c = |v: f32| (v / sum[3] + 0.5).min(255.0) as u8;
    let alpha = sum[3] / (across * down) as f32 + 0.5;
    [c(sum[0]), c(sum[1]), c(sum[2]), alpha.min(255.0) as u8]
}

/// A setting within its dialog's range (an action's file could hold anything,
/// and 1e39 reads as infinity).
#[inline]
fn limited(v: f32, low: f32, high: f32) -> f32 {
    if v.is_nan() {
        low.max(0.0).min(high)
    } else {
        v.clamp(low, high)
    }
}

/// A value per row or column, between two whole places: the middle of a pixel
/// reads its own.
#[inline]
fn between(table: &[f32], first: u32, at: f32) -> f32 {
    let place = (at - 0.5 - first as f32).clamp(0.0, (table.len() - 1) as f32);
    let i = place as usize;
    let next = table[(i + 1).min(table.len() - 1)];
    table[i] + (next - table[i]) * (place - i as f32)
}

/// `region` of what `filter` gives for all of `src` (at `origin` in the
/// canvas), if it is one of these filters.
pub(super) fn render(
    filter: &Filter,
    src: &RgbaImage,
    origin: (i64, i64),
    region: [u32; 4],
) -> Option<RgbaImage> {
    Some(match *filter {
        Filter::Ripple { amount, size } => ripple(src, region, amount, size, origin),
        Filter::Wave(w) => wave(src, region, &w, origin),
        Filter::Twirl { angle } => twirl(src, region, angle),
        Filter::Spherize { amount, mode } => spherize(src, region, amount, mode),
        Filter::PolarCoordinates { conversion } => polar(src, region, conversion),
        Filter::Pinch { amount } => pinch(src, region, amount),
        Filter::ZigZag {
            amount,
            ridges,
            style,
        } => zigzag(src, region, amount, ridges, style),
        Filter::Shear(s) => shear(src, region, &s),
        _ => return None,
    })
}

/// Index along an axis of `n` pixels of position `i`; `None`: transparent.
#[inline]
fn along(i: i64, n: i64, edge: Undefined) -> Option<i64> {
    if (0..n).contains(&i) {
        return Some(i);
    }
    match edge {
        Undefined::RepeatEdges => Some(i.clamp(0, n - 1)),
        Undefined::WrapAround => Some(i.rem_euclid(n)),
        Undefined::Transparent => None,
    }
}

/// The color at (x, y), pixel coordinates; averaged premultiplied.
#[inline]
fn bilinear(
    raw: &[u8],
    (w, h): (i64, i64),
    (edge_x, edge_y): (Undefined, Undefined),
    x: f32,
    y: f32,
) -> [u8; 4] {
    if !x.is_finite() || !y.is_finite() {
        return [0; 4];
    }
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (x0, y0) = (fx as i64, fy as i64);
    let mut sum = [0.0f32; 4];
    for (dy, wy) in [(0, 1.0 - ty), (1, ty)] {
        if wy == 0.0 {
            continue;
        }
        let Some(sy) = along(y0 + dy, h, edge_y) else {
            continue;
        };
        for (dx, wx) in [(0, 1.0 - tx), (1, tx)] {
            if wx == 0.0 {
                continue;
            }
            let Some(sx) = along(x0 + dx, w, edge_x) else {
                continue;
            };
            let at = ((sy * w + sx) * 4) as usize;
            let weight = wx * wy;
            let alpha = f32::from(raw[at + 3]) * weight;
            sum[0] += f32::from(raw[at]) * alpha;
            sum[1] += f32::from(raw[at + 1]) * alpha;
            sum[2] += f32::from(raw[at + 2]) * alpha;
            sum[3] += alpha;
        }
    }
    if sum[3] <= 0.0 {
        return [0; 4];
    }
    let c = |v: f32| (v / sum[3] + 0.5).min(255.0) as u8;
    [
        c(sum[0]),
        c(sum[1]),
        c(sum[2]),
        (sum[3] + 0.5).min(255.0) as u8,
    ]
}

/// [`remap`] inside the ellipse fit in the image: `f` gets the place in the
/// ellipse (from −1 to 1 on each axis) and its distance to the center (below
/// 1), and gives the place to read in the same units.
fn in_ellipse(
    src: &RgbaImage,
    region: [u32; 4],
    f: impl Fn(f32, f32, f32) -> (f32, f32) + Sync,
) -> RgbaImage {
    let (hw, hh) = (src.width() as f32 / 2.0, src.height() as f32 / 2.0);
    let edge = Undefined::RepeatEdges;
    remap(src, region, (edge, edge), |x, y| {
        let (u, v) = ((x - hw) / hw, (y - hh) / hh);
        let r = (u * u + v * v).sqrt();
        (r < 1.0).then(|| {
            let (su, sv) = f(u, v, r);
            (hw + su * hw, hh + sv * hh)
        })
    })
}

/// Turned by `a` radians, clockwise on the screen (y points down).
#[inline]
fn turned(u: f32, v: f32, a: f32) -> (f32, f32) {
    let (s, c) = a.sin_cos();
    (u * c - v * s, u * s + v * c)
}

/// Twirl: a positive angle turns clockwise, the most at the center, not at
/// the rim.
fn twirl(src: &RgbaImage, region: [u32; 4], angle: f32) -> RgbaImage {
    let angle = limited(angle, -999.0, 999.0).to_radians();
    in_ellipse(src, region, |u, v, r| turned(u, v, -angle * (1.0 - r)))
}

/// Pinch: a positive amount squeezes toward the center, a negative one
/// pushes out from it.
fn pinch(src: &RgbaImage, region: [u32; 4], amount: f32) -> RgbaImage {
    let k = -limited(amount / 100.0, -1.0, 1.0) / 2.0;
    in_ellipse(src, region, |u, v, r| {
        if r <= 0.0 {
            return (u, v);
        }
        let factor = (r * FRAC_PI_2).sin().powf(k);
        (u * factor, v * factor)
    })
}

/// Spherize: wraps the image around a sphere (a cylinder in the two
/// one-axis modes); a negative amount, inside it.
fn spherize(src: &RgbaImage, region: [u32; 4], amount: f32, mode: SpherizeMode) -> RgbaImage {
    let k = limited(amount / 100.0, -1.0, 1.0);
    // Distance to the center read for a distance (0 to 1).
    let bend = move |r: f32| {
        let to = if k >= 0.0 {
            r.min(1.0).asin() / FRAC_PI_2
        } else {
            (r * FRAC_PI_2).sin()
        };
        r + (to - r) * k.abs()
    };
    let signed = move |c: f32| bend(c.abs()).copysign(c);
    match mode {
        SpherizeMode::Normal => in_ellipse(src, region, |u, v, r| {
            if r <= 0.0 {
                return (u, v);
            }
            let scale = bend(r) / r;
            (u * scale, v * scale)
        }),
        SpherizeMode::HorizontalOnly | SpherizeMode::VerticalOnly => {
            let (hw, hh) = (src.width() as f32 / 2.0, src.height() as f32 / 2.0);
            let edge = Undefined::RepeatEdges;
            let horizontal = mode == SpherizeMode::HorizontalOnly;
            remap(src, region, (edge, edge), |x, y| {
                Some(if horizontal {
                    (hw + signed((x - hw) / hw) * hw, y)
                } else {
                    (x, hh + signed((y - hh) / hh) * hh)
                })
            })
        }
    }
}

/// Polar Coordinates. Rectangular to Polar: the top row becomes the center,
/// the bottom one the rim, the left and right edges meeting at the top.
fn polar(src: &RgbaImage, region: [u32; 4], conversion: PolarConversion) -> RgbaImage {
    let (w, h) = (src.width() as f32, src.height() as f32);
    let (hw, hh) = (w / 2.0, h / 2.0);
    match conversion {
        PolarConversion::RectangularToPolar => {
            let edges = (Undefined::WrapAround, Undefined::RepeatEdges);
            remap(src, region, edges, |x, y| {
                let (u, v) = ((x - hw) / hw, (y - hh) / hh);
                let turn = u.atan2(-v).rem_euclid(TAU) / TAU;
                Some((turn * w, (u * u + v * v).sqrt() * h))
            })
        }
        PolarConversion::PolarToRectangular => {
            let edge = Undefined::RepeatEdges;
            remap(src, region, (edge, edge), |x, y| {
                let (s, c) = (x / w * TAU).sin_cos();
                let r = y / h;
                Some((hw + r * s * hw, hh - r * c * hh))
            })
        }
    }
}

/// ZigZag: rings around the center, fading to nothing at the rim.
fn zigzag(
    src: &RgbaImage,
    region: [u32; 4],
    amount: f32,
    ridges: f32,
    style: ZigZagStyle,
) -> RgbaImage {
    let k = limited(amount / 100.0, -1.0, 1.0);
    let ridges = limited(ridges, 1.0, 20.0).round();
    let (turn, out) = match style {
        ZigZagStyle::AroundCenter => (PI, 0.0),
        ZigZagStyle::OutFromCenter => (0.0, 0.25),
        ZigZagStyle::PondRipples => (FRAC_PI_2, 0.25),
    };
    in_ellipse(src, region, move |u, v, r| {
        if r <= 0.0 {
            return (u, v);
        }
        let wave = k * (TAU * ridges * r).sin() * (1.0 - r);
        let (u, v) = turned(u, v, turn * wave);
        let scale = (r + out * wave).max(0.0) / r;
        (u * scale, v * scale)
    })
}

/// Ripple: small irregular waves, the same each time.
fn ripple(
    src: &RgbaImage,
    region: [u32; 4],
    amount: f32,
    size: RippleSize,
    origin: (i64, i64),
) -> RgbaImage {
    let length = match size {
        RippleSize::Small => 6.0,
        RippleSize::Medium => 16.0,
        RippleSize::Large => 40.0,
    };
    let amplitude = limited(amount, -999.0, 999.0) / 100.0 * length * 0.2;
    let cells = ripple_noise();
    let (ox, oy) = (origin.0 as f32, origin.1 as f32);
    let edge = Undefined::RepeatEdges;
    remap(src, region, (edge, edge), |x, y| {
        let (cx, cy) = (x + ox, y + oy);
        let (nx, ny) = (cx / (length * 4.0), cy / (length * 4.0));
        let a = TAU * cy / length + 3.0 * smooth_noise(&cells, nx, ny);
        let b = TAU * cx / length + 3.0 * smooth_noise(&cells, nx + 17.0, ny + 31.0);
        Some((x + amplitude * a.sin(), y + amplitude * b.sin()))
    })
}

const NOISE_SIDE: usize = 64;

/// Fixed pseudo-random values from −1 to 1.
fn ripple_noise() -> Vec<f32> {
    let mut rng = Rng::new(0x5EED);
    (0..NOISE_SIDE * NOISE_SIDE)
        .map(|_| rng.next() * 2.0 - 1.0)
        .collect()
}

/// [`ripple_noise`], smooth between its values, repeating.
#[inline]
fn smooth_noise(cells: &[f32], x: f32, y: f32) -> f32 {
    let (fx, fy) = (x.floor(), y.floor());
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (tx, ty) = (smooth(x - fx), smooth(y - fy));
    let side = NOISE_SIDE as i64;
    let at = |i: i64, j: i64| cells[(j.rem_euclid(side) * side + i.rem_euclid(side)) as usize];
    let (i, j) = (fx as i64, fy as i64);
    let top = at(i, j) + (at(i + 1, j) - at(i, j)) * tx;
    let bottom = at(i, j + 1) + (at(i + 1, j + 1) - at(i, j + 1)) * tx;
    top + (bottom - top) * ty
}

/// Small deterministic generator of values from 0 to 1.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // Small seeds (0 is Wave's first) mixed, or the first values would
        // all be tiny.
        let mut x = seed.wrapping_add(0x9E37_79B9);
        x = (x ^ (x >> 16)).wrapping_mul(0x85EB_CA6B);
        x = (x ^ (x >> 13)).wrapping_mul(0xC2B2_AE35);
        Self((x ^ (x >> 16)) | 1)
    }

    fn next(&mut self) -> f32 {
        // xorshift32.
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Wave: the sum of the generators' waves shifts rows sideways and columns
/// up and down; computed per row and per column, so that many generators
/// stay fast. The sum is reduced by the root of their number: more generators
/// make the wave less regular, not wider.
fn wave(src: &RgbaImage, region: [u32; 4], wave: &Wave, origin: (i64, i64)) -> RgbaImage {
    let (w, h) = src.dimensions();
    let [x0, y0, x1, y1] = region;
    let generators = limited(wave.generators, 1.0, 999.0).round() as u32;
    let mut rng = Rng::new(wave.seed);
    let scale = (generators as f32).sqrt();
    let cancel = cancel_flag();
    let lengths = ordered(
        limited(wave.wavelength_min, 1.0, 999.0),
        limited(wave.wavelength_max, 1.0, 999.0),
    );
    let amplitudes = ordered(
        limited(wave.amplitude_min, 0.0, 999.0),
        limited(wave.amplitude_max, 0.0, 999.0),
    );
    // The places `first..end` along an axis. All the random values are drawn
    // whatever the part: a part gets what the whole gets.
    let mut table = |first: u32, end: u32, at: i64, percent: f32| -> Vec<f32> {
        let mut out = vec![0.0f32; (end - first) as usize];
        for _ in 0..generators {
            let length = lengths.0 + (lengths.1 - lengths.0) * rng.next();
            let amplitude = amplitudes.0 + (amplitudes.1 - amplitudes.0) * rng.next();
            let phase = rng.next();
            if cancelled(&cancel) {
                continue;
            }
            for (i, v) in out.iter_mut().enumerate() {
                let t = (i as i64 + first as i64 + at) as f32 / length + phase;
                *v += amplitude * wave_shape(wave.wave_type, t);
            }
        }
        let k = limited(percent, 0.0, 100.0) / 100.0 / scale;
        out.iter_mut().for_each(|v| *v *= k);
        out
    };
    // One more on each side: between rows, a pixel's samples read two.
    let (top, left) = (y0.saturating_sub(1), x0.saturating_sub(1));
    let sideways = table(
        top,
        (y1 + 1).min(h).max(top + 1),
        origin.1,
        wave.scale_horizontal,
    );
    let upward = table(
        left,
        (x1 + 1).min(w).max(left + 1),
        origin.0,
        wave.scale_vertical,
    );
    remap(src, region, (wave.undefined, wave.undefined), |x, y| {
        Some((
            x + between(&sideways, top, y),
            y + between(&upward, left, x),
        ))
    })
}

fn ordered(a: f32, b: f32) -> (f32, f32) {
    (a.min(b), a.max(b))
}

/// One period per unit of `t`, from −1 to 1.
#[inline]
fn wave_shape(kind: WaveType, t: f32) -> f32 {
    let t = t - t.floor();
    match kind {
        WaveType::Sine => (t * TAU).sin(),
        WaveType::Triangle => {
            if t < 0.25 {
                4.0 * t
            } else if t < 0.75 {
                2.0 - 4.0 * t
            } else {
                4.0 * t - 4.0
            }
        }
        WaveType::Square => {
            if t < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// Shear: each row shifts by what the curve gives for its height.
fn shear(src: &RgbaImage, region: [u32; 4], shear: &Shear) -> RgbaImage {
    let (w, h) = src.dimensions();
    let half = w as f32 / 2.0;
    let mut curve = *shear;
    for p in &mut curve.points {
        *p = [limited(p[0], -1.0, 1.0), limited(p[1], 0.0, 1.0)];
    }
    let shifts: Vec<f32> = (0..h)
        .map(|y| curve.shift_at((y as f32 + 0.5) / h as f32) * half)
        .collect();
    let edges = (curve.undefined, Undefined::RepeatEdges);
    remap(src, region, edges, |x, y| {
        Some((x - between(&shifts, 0, y), y))
    })
}

#[cfg(test)]
mod tests {
    use super::super::{FilterKind, Target, cancellable};
    use super::*;
    use image::Rgba;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    const KINDS: [FilterKind; 8] = [
        FilterKind::Pinch,
        FilterKind::PolarCoordinates,
        FilterKind::Ripple,
        FilterKind::Shear,
        FilterKind::Spherize,
        FilterKind::Twirl,
        FilterKind::Wave,
        FilterKind::ZigZag,
    ];

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 90, 255])
        })
    }

    fn applied(filter: Filter, image: &RgbaImage) -> RgbaImage {
        let mut out = image.clone();
        filter.apply(&mut out, (0, 0));
        out
    }

    /// Black image with a white square of `side` centered at (x, y).
    fn spot(w: u32, h: u32, (x, y): (u32, u32), side: u32) -> RgbaImage {
        let inside = |a: u32, c: u32| a + side / 2 >= c && a < c + side / 2;
        RgbaImage::from_fn(w, h, |i, j| {
            if inside(i, x) && inside(j, y) {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 255])
            }
        })
    }

    /// Center of the white in an image.
    fn centroid(image: &RgbaImage) -> (f32, f32) {
        let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for (x, y, p) in image.enumerate_pixels() {
            let v = p.0[0] as f32;
            sum += v;
            sx += v * (x as f32 + 0.5);
            sy += v * (y as f32 + 0.5);
        }
        (sx / sum, sy / sum)
    }

    #[test]
    fn settings_survive_serialization_and_reach_the_whole_area() {
        for kind in KINDS {
            let filter = kind.initial();
            assert_eq!(filter.kind(), kind);
            assert!(kind.has_settings());
            assert_eq!(filter.reach(), None, "{kind:?} needs the whole area");
            let text = serde_json::to_string(&filter).unwrap();
            assert_eq!(serde_json::from_str::<Filter>(&text).unwrap(), filter);
        }
        let text = serde_json::to_string(&FilterKind::ZigZag.initial()).unwrap();
        assert!(text.contains(r#""filter":"zig-zag""#), "{text}");
    }

    #[test]
    fn neutral_settings_change_nothing() {
        let image = gradient(40, 30);
        for filter in [
            Filter::Twirl { angle: 0.0 },
            Filter::Pinch { amount: 0.0 },
            Filter::Spherize {
                amount: 0.0,
                mode: SpherizeMode::Normal,
            },
            Filter::Spherize {
                amount: 0.0,
                mode: SpherizeMode::VerticalOnly,
            },
            Filter::Ripple {
                amount: 0.0,
                size: RippleSize::Large,
            },
            Filter::ZigZag {
                amount: 0.0,
                ridges: 5.0,
                style: ZigZagStyle::PondRipples,
            },
            FilterKind::Shear.initial(),
        ] {
            let out = applied(filter, &image);
            for (a, b) in out.pixels().zip(image.pixels()) {
                let d = (0..4).map(|c| a[c].abs_diff(b[c])).max().unwrap();
                assert!(d <= 1, "{filter:?}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn defaults_are_the_reference_ones_and_all_change_the_image() {
        assert_eq!(FilterKind::Twirl.initial(), Filter::Twirl { angle: 50.0 });
        assert_eq!(FilterKind::Pinch.initial(), Filter::Pinch { amount: 50.0 });
        assert_eq!(
            FilterKind::ZigZag.initial(),
            Filter::ZigZag {
                amount: 10.0,
                ridges: 5.0,
                style: ZigZagStyle::PondRipples,
            }
        );
        let image = gradient(80, 60);
        for kind in KINDS {
            if kind == FilterKind::Shear {
                continue; // A straight line at first.
            }
            assert_ne!(applied(kind.initial(), &image), image, "{kind:?}");
        }
    }

    #[test]
    fn twirl_turns_clockwise_and_keeps_outside_the_ellipse() {
        // Right of the center: a positive angle brings it below.
        let image = spot(201, 201, (130, 100), 12);
        let out = applied(Filter::Twirl { angle: 90.0 }, &image);
        let (x, y) = centroid(&out);
        assert!(y > 110.0 && x > 100.0, "({x}, {y})");
        let back = applied(Filter::Twirl { angle: -90.0 }, &image);
        assert!(centroid(&back).1 < 90.0, "the other way");
        // The corners are out of the ellipse.
        let image = gradient(40, 40);
        let out = applied(Filter::Twirl { angle: 500.0 }, &image);
        assert_eq!(out.get_pixel(0, 0), image.get_pixel(0, 0));
        assert_eq!(out.get_pixel(39, 39), image.get_pixel(39, 39));
    }

    #[test]
    fn pinch_and_spherize_move_along_the_radius() {
        let image = spot(201, 201, (150, 100), 10);
        let inward = applied(Filter::Pinch { amount: 100.0 }, &image);
        assert!(centroid(&inward).0 < 148.0, "{:?}", centroid(&inward));
        let outward = applied(Filter::Pinch { amount: -100.0 }, &image);
        assert!(centroid(&outward).0 > 152.0, "{:?}", centroid(&outward));
        let on_sphere =
            |amount, mode| centroid(&applied(Filter::Spherize { amount, mode }, &image)).0;
        assert!(on_sphere(100.0, SpherizeMode::Normal) > 152.0, "magnified");
        assert!(on_sphere(-100.0, SpherizeMode::Normal) < 148.0, "shrunk");
        // Vertical only leaves columns where they are.
        let vertical = on_sphere(100.0, SpherizeMode::VerticalOnly);
        assert!((vertical - 150.0).abs() < 1.0, "{vertical}");
        assert!(on_sphere(100.0, SpherizeMode::HorizontalOnly) > 152.0);
    }

    #[test]
    fn polar_coordinates_map_top_to_center_and_convert_back() {
        // A white top row: a spot at the center.
        let image = RgbaImage::from_fn(101, 101, |_, y| {
            if y < 5 {
                Rgba([255; 4])
            } else {
                Rgba([0, 0, 0, 255])
            }
        });
        let conversion = PolarConversion::RectangularToPolar;
        let out = applied(Filter::PolarCoordinates { conversion }, &image);
        assert_eq!(out.get_pixel(50, 50).0[0], 255);
        assert_eq!(out.get_pixel(50, 5).0[0], 0, "the rim is the bottom row");
        // Back: close to the original away from the center and the seams.
        let smooth = RgbaImage::from_fn(200, 200, |_, y| Rgba([y as u8, 40, 90, 255]));
        let there = applied(Filter::PolarCoordinates { conversion }, &smooth);
        let back = applied(
            Filter::PolarCoordinates {
                conversion: PolarConversion::PolarToRectangular,
            },
            &there,
        );
        for y in 20..180 {
            let d = back.get_pixel(100, y).0[0].abs_diff(smooth.get_pixel(100, y).0[0]);
            assert!(d <= 3, "row {y}: {d}");
        }
    }

    #[test]
    fn wave_keeps_to_its_seed_and_amplitude() {
        let Filter::Wave(initial) = FilterKind::Wave.initial() else {
            unreachable!()
        };
        let image = gradient(80, 60);
        let run = |w: Wave| applied(Filter::Wave(w), &image);
        assert_eq!(run(initial), run(initial), "the same seed, the same waves");
        assert_ne!(run(initial), run(Wave { seed: 9, ..initial }));
        // Scales at 1 % of 1 px: as good as nothing.
        let still = run(Wave {
            amplitude_min: 1.0,
            amplitude_max: 1.0,
            scale_horizontal: 1.0,
            scale_vertical: 1.0,
            generators: 1.0,
            ..initial
        });
        let d = still
            .pixels()
            .zip(image.pixels())
            .map(|(a, b)| a[0].abs_diff(b[0]).max(a[1].abs_diff(b[1])))
            .max()
            .unwrap();
        assert!(d <= 4, "{d}");
        // One generator of a fixed amplitude, sideways only: a white column
        // moves by at most the amplitude.
        let column = RgbaImage::from_fn(101, 60, |x, _| {
            if x == 50 {
                Rgba([255; 4])
            } else {
                Rgba([0, 0, 0, 255])
            }
        });
        let mut out = column.clone();
        Filter::Wave(Wave {
            generators: 1.0,
            wavelength_min: 20.0,
            wavelength_max: 20.0,
            amplitude_min: 10.0,
            amplitude_max: 10.0,
            scale_horizontal: 100.0,
            scale_vertical: 1.0,
            ..initial
        })
        .apply(&mut out, (0, 0));
        let mut moved = 0;
        for y in 0..60 {
            let x = (0..101).max_by_key(|&x| out.get_pixel(x, y).0[0]).unwrap() as i32;
            assert!((x - 50).abs() <= 11, "row {y}: {x}");
            moved = moved.max((x - 50).abs());
        }
        assert!(moved >= 7, "{moved}");
    }

    #[test]
    fn wave_shapes_span_their_range() {
        for kind in [WaveType::Sine, WaveType::Triangle, WaveType::Square] {
            assert!(wave_shape(kind, 0.75).abs() >= 0.99 || wave_shape(kind, 0.8) == -1.0);
            for i in 0..100 {
                assert!(wave_shape(kind, i as f32 / 37.0 - 1.0).abs() <= 1.0);
            }
        }
        assert_eq!(wave_shape(WaveType::Triangle, 0.25), 1.0);
        assert_eq!(wave_shape(WaveType::Square, 0.75), -1.0);
    }

    #[test]
    fn undefined_areas_wrap_repeat_or_clear() {
        let image = RgbaImage::from_fn(4, 1, |x, _| Rgba([x as u8 * 10, 0, 0, 255]));
        let shifted = |undefined| {
            let mut s = Shear::straight(undefined);
            // Half the width to the right all along: 2 px.
            s.points[0][0] = 1.0;
            s.points[1][0] = 1.0;
            let out = applied(Filter::Shear(s), &image);
            out.pixels().map(|p| (p.0[0], p.0[3])).collect::<Vec<_>>()
        };
        assert_eq!(
            shifted(Undefined::WrapAround),
            [(20, 255), (30, 255), (0, 255), (10, 255)]
        );
        assert_eq!(
            shifted(Undefined::RepeatEdges),
            [(0, 255), (0, 255), (0, 255), (10, 255)]
        );
        assert_eq!(
            shifted(Undefined::Transparent),
            [(0, 0), (0, 0), (0, 255), (10, 255)]
        );
    }

    #[test]
    fn shear_curve_goes_through_its_points_and_edits() {
        let mut s = Shear::straight(Undefined::RepeatEdges);
        assert_eq!(s.shift_at(0.3), 0.0);
        let i = s.insert(0.5, 0.5).unwrap();
        assert_eq!((i, s.used().len()), (1, 3));
        assert!((s.shift_at(0.5) - 0.5).abs() < 1e-5);
        assert!(s.shift_at(0.25) > 0.1 && s.shift_at(0.25) < 0.5);
        // The ends keep their height; the others between their neighbors.
        s.place(0, -0.25, 0.4);
        assert_eq!(s.points[0], [-0.25, 0.0]);
        s.place(1, 2.0, -1.0);
        assert_eq!(s.points[1][0], 1.0);
        assert!(s.points[1][1] > 0.0);
        // At most SHEAR_POINTS, the ends not removed.
        let mut height = 0.9;
        while s.used().len() < SHEAR_POINTS {
            assert!(s.insert(0.0, height).is_some());
            height += 0.01;
        }
        assert_eq!(s.insert(0.0, 0.2), None);
        s.remove(0);
        s.remove(SHEAR_POINTS - 1);
        assert_eq!(s.used().len(), SHEAR_POINTS);
        s.remove(1);
        assert_eq!(s.used().len(), SHEAR_POINTS - 1);
        assert!(s.used().windows(2).all(|p| p[0][1] < p[1][1]));
        assert_eq!(s.used()[SHEAR_POINTS - 2], [0.0, 1.0]);
    }

    #[test]
    fn hidden_colors_of_transparent_pixels_stay_hidden() {
        let disc = |hidden: u8| {
            RgbaImage::from_fn(64, 64, |x, y| {
                let (dx, dy) = (x as f32 - 31.5, y as f32 - 31.5);
                if dx * dx + dy * dy < 15.0 * 15.0 {
                    Rgba([200, 120, 40, 255])
                } else {
                    Rgba([hidden, hidden, hidden, 0])
                }
            })
        };
        for kind in KINDS {
            let filter = match kind.initial() {
                Filter::Shear(mut s) => {
                    s.insert(0.3, 0.5);
                    Filter::Shear(s)
                }
                f => f,
            };
            let (a, b) = (applied(filter, &disc(0)), applied(filter, &disc(255)));
            let edge = a.pixels().any(|p| p[3] > 0 && p[3] < 255);
            assert!(edge, "{kind:?}: interpolated");
            for (p, q) in a.pixels().zip(b.pixels()) {
                if p[3] > 0 || q[3] > 0 {
                    assert_eq!(p, q, "{kind:?}");
                    assert_eq!(p[0], 200, "{kind:?}: the color alone");
                }
            }
        }
    }

    #[test]
    fn a_selection_box_is_the_area_distorted() {
        // The center of Twirl is the middle of the box; outside it nothing
        // changes, and a preview's region gives the same pixels.
        let image = Arc::new(gradient(60, 40));
        let target = super::super::Target {
            alpha_locked: false,
            whole_source: false,
            image: image.clone(),
            clip: Some(crate::pixel_ops::Clip {
                rect: [10, 5, 50, 35],
                shape: None,
                inverted: false,
            }),
            offset: (3, 4),
        };
        let filter = Filter::Twirl { angle: 200.0 };
        let reach = filter.reach().unwrap_or(0);
        let out = target.process(reach, |i, o| filter.apply(i, o));
        assert_eq!(out.get_pixel(5, 20), image.get_pixel(5, 20));
        let mut area = image::imageops::crop_imm(&*image, 10, 5, 40, 30).to_image();
        filter.apply(&mut area, (13, 9));
        assert_eq!(out.get_pixel(30, 20), area.get_pixel(20, 15));
        let part =
            target.process_region([20, 10, 40, 30], filter.reach(), |i, o| filter.apply(i, o));
        assert_eq!(
            part,
            image::imageops::crop_imm(&out, 20, 10, 20, 20).to_image()
        );
    }

    #[test]
    fn tiny_images_and_extreme_settings_are_safe() {
        for (w, h) in [(1, 1), (1, 7), (7, 1), (2, 2)] {
            let image = gradient(w, h);
            for kind in KINDS {
                applied(kind.initial(), &image);
            }
            for filter in [
                Filter::Twirl { angle: 999.0 },
                Filter::Ripple {
                    amount: -999.0,
                    size: RippleSize::Large,
                },
                Filter::ZigZag {
                    amount: -100.0,
                    ridges: 20.0,
                    style: ZigZagStyle::OutFromCenter,
                },
                Filter::Pinch { amount: 100.0 },
                Filter::Spherize {
                    amount: -100.0,
                    mode: SpherizeMode::HorizontalOnly,
                },
            ] {
                applied(filter, &image);
            }
        }
    }

    #[test]
    fn cancelled_work_stops() {
        let image = gradient(200, 200);
        let mut out = image.clone();
        let flag = Arc::new(AtomicBool::new(true));
        cancellable(Some(flag), || {
            Filter::Twirl { angle: 90.0 }.apply(&mut out, (0, 0));
        });
        assert_eq!(out, image);
    }

    fn strong(kind: FilterKind) -> Filter {
        match kind.initial() {
            Filter::Twirl { .. } => Filter::Twirl { angle: 999.0 },
            Filter::Shear(mut s) => {
                s.insert(0.6, 0.5);
                Filter::Shear(s)
            }
            f => f,
        }
    }

    #[test]
    fn a_part_is_what_the_whole_gives_and_only_it_is_computed() {
        let image = Arc::new(gradient(200, 150));
        for kind in KINDS {
            let filter = strong(kind);
            let target = Target::of(image.clone(), (13, -4));
            let mut whole = (*image).clone();
            filter.apply(&mut whole, (13, -4));
            let region = [31, 57, 94, 121];
            let part = target.filter_region(region, &filter);
            let expected = image::imageops::crop_imm(&whole, 31, 57, 63, 64).to_image();
            assert_eq!(part, expected, "{kind:?}");
            // The same with a selection's box, the geometry fit in it.
            let clipped = Target {
                clip: Some([20, 30, 180, 140].into()),
                ..target.clone()
            };
            let all = clipped.process(0, |i, o| filter.apply(i, o));
            let part = clipped.filter_region(region, &filter);
            let expected = image::imageops::crop_imm(&all, 31, 57, 63, 64).to_image();
            assert_eq!(part, expected, "{kind:?} in a box");
        }
    }

    #[test]
    fn compressed_detail_is_averaged_not_skipped() {
        // Stripes of one pixel read four to one: one sample per pixel would
        // see only one of the two colors.
        let stripes = RgbaImage::from_fn(200, 8, |x, _| {
            let v = if x % 2 == 0 { 255 } else { 0 };
            Rgba([v, v, v, 255])
        });
        let e = Undefined::RepeatEdges;
        let out = remap(&stripes, [0, 0, 50, 8], (e, e), |x, y| Some((x * 4.0, y)));
        for p in out.pixels() {
            assert!((90..=165).contains(&p[0]), "{p:?}");
        }
        // A thin line stays in one piece where a twirl compresses it.
        let line = RgbaImage::from_fn(400, 400, |x, y| {
            let v = if x.abs_diff(200) < 1 && y < 200 {
                255
            } else {
                0
            };
            Rgba([v, v, v, 255])
        });
        let out = applied(Filter::Twirl { angle: 999.0 }, &line);
        let total = |i: &RgbaImage| i.pixels().map(|p| u64::from(p[0])).sum::<u64>();
        let (before, after) = (total(&line), total(&out));
        assert!(after * 3 > before, "{before} -> {after}");
    }

    #[test]
    fn neutral_rows_read_their_own_value() {
        let t = [1.0, 3.0, 7.0];
        assert_eq!(between(&t, 10, 10.5), 1.0);
        assert_eq!(between(&t, 10, 11.5), 3.0);
        assert_eq!(between(&t, 10, 12.0), 5.0);
        assert_eq!(between(&t, 10, 99.0), 7.0);
        assert_eq!(between(&t, 10, 0.0), 1.0);
    }

    #[test]
    fn settings_out_of_range_from_a_file_do_not_erase_the_layer() {
        let image = gradient(64, 48);
        for text in [
            r#"{"filter":"ripple","amount":1e39,"size":"medium"}"#,
            r#"{"filter":"twirl","angle":1e39}"#,
            r#"{"filter":"twirl","angle":-1e39}"#,
            r#"{"filter":"pinch","amount":1e39}"#,
            r#"{"filter":"zig-zag","amount":1e39,"ridges":1e39,"style":"pond-ripples"}"#,
        ] {
            let filter: Filter = serde_json::from_str(text).expect(text);
            let out = applied(filter, &image);
            assert!(out.pixels().all(|p| p[3] == 255), "{text}");
        }
        let Filter::Wave(mut w) = FilterKind::Wave.initial() else {
            panic!()
        };
        w.amplitude_max = f32::INFINITY;
        w.wavelength_max = f32::INFINITY;
        w.scale_horizontal = f32::INFINITY;
        w.undefined = Undefined::RepeatEdges;
        assert!(
            applied(Filter::Wave(w), &image)
                .pixels()
                .all(|p| p[3] == 255)
        );
        let Filter::Shear(mut s) = FilterKind::Shear.initial() else {
            panic!()
        };
        s.points[0][0] = f32::INFINITY;
        s.points[1][1] = f32::NAN;
        s.undefined = Undefined::RepeatEdges;
        assert!(
            applied(Filter::Shear(s), &image)
                .pixels()
                .all(|p| p[3] == 255)
        );
    }

    #[test]
    fn the_first_values_of_small_seeds_are_not_tiny() {
        for seed in 0..64 {
            let mut rng = Rng::new(seed);
            let first: Vec<f32> = (0..4).map(|_| rng.next()).collect();
            assert!(first.iter().any(|v| *v > 0.1), "{seed}: {first:?}");
        }
        let (a, b) = (Rng::new(0).next(), Rng::new(1).next());
        assert!((a - b).abs() > 1e-3, "{a} {b}");
    }

    #[test]
    fn a_locked_target_keeps_the_opacity_and_the_old_color() {
        let half = RgbaImage::from_fn(100, 100, |x, _| {
            if x < 50 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let target = Target {
            alpha_locked: true,
            ..Target::of(Arc::new(half.clone()), (0, 0))
        };
        let filter = FilterKind::Wave.initial();
        let out = target.process(0, |i, o| filter.apply(i, o));
        for (a, b) in out.pixels().zip(half.pixels()) {
            assert_eq!(a[3], b[3]);
            if b[3] == 255 {
                assert!(a[0] > 200 && a[1] == 0 && a[2] == 0, "{a:?}");
            }
        }
    }

    #[test]
    fn keeping_the_opacity_can_be_done_again() {
        let old = [10u8, 20, 30, 128];
        // Less opaque than it was: half of what is missing is the old color.
        let mut new = [200u8, 200, 200, 64];
        crate::pixel_ops::keep_opacity(&mut new, &old);
        assert_eq!(new, [105, 110, 115, 128]);
        let again = new;
        crate::pixel_ops::keep_opacity(&mut new, &old);
        assert_eq!(new, again);
        // As opaque or more (an adjustment, a brush): its color stays.
        let mut new = [200u8, 100, 50, 128];
        crate::pixel_ops::keep_opacity(&mut new, &old);
        assert_eq!(new, [200, 100, 50, 128]);
        let mut new = [200u8, 100, 50, 255];
        crate::pixel_ops::keep_opacity(&mut new, &old);
        assert_eq!(new, [200, 100, 50, 128]);
        // Nothing left: the old pixel.
        let mut new = [0u8; 4];
        crate::pixel_ops::keep_opacity(&mut new, &old);
        assert_eq!(new, old);
    }

    #[test]
    fn huge_settings_of_any_filter_are_safe() {
        let image = gradient(32, 32);
        let mut kinds = KINDS.to_vec();
        kinds.extend([
            FilterKind::BoxBlur,
            FilterKind::GaussianBlur,
            FilterKind::MotionBlur,
            FilterKind::SurfaceBlur,
            FilterKind::AddNoise,
            FilterKind::DustAndScratches,
            FilterKind::Median,
            FilterKind::ReduceNoise,
            FilterKind::Mosaic,
            FilterKind::SmartSharpen,
            FilterKind::UnsharpMask,
            FilterKind::Emboss,
            FilterKind::HighPass,
            FilterKind::Maximum,
            FilterKind::Minimum,
            FilterKind::Offset,
        ]);
        // Every number of its settings written as it would be in a file.
        fn huge(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Number(n) if n.is_f64() => {
                    *v = serde_json::Value::from(1e300);
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(huge),
                serde_json::Value::Object(o) => o.values_mut().for_each(huge),
                _ => {}
            }
        }
        for kind in kinds {
            let mut value = serde_json::to_value(kind.initial()).unwrap();
            huge(&mut value);
            let Ok(filter) = serde_json::from_value::<Filter>(value.clone()) else {
                continue;
            };
            let _ = filter.reach();
            let _ = applied(filter, &image);
            let target = Target::of(Arc::new(image.clone()), (0, 0));
            let _ = target.filter_region([4, 4, 20, 20], &filter);
        }
    }

    /// `cargo test --release distort_speed -- --ignored --nocapture`
    #[test]
    #[ignore = "timing, in release"]
    fn distort_speed() {
        let mut image = gradient(6000, 4000);
        for kind in KINDS {
            let start = std::time::Instant::now();
            kind.initial().apply(&mut image, (0, 0));
            println!("{kind:?}: {:?}", start.elapsed());
        }
    }
}
