//! Pixel processing: fill, color to transparency, image adjustments (color
//! balance, brightness/contrast, levels, curves, hue/saturation, vibrance,
//! and those of `tones`), histograms, distance fields and blur.

use std::cell::Cell;
use std::sync::Arc;

use image::{GrayImage, Rgba, RgbaImage};

use crate::i18n::t;

mod tones;
pub use tones::{BlackWhite, GradientMap, PhotoFilter, Posterize, Threshold};

/// Distance between two colors: the largest difference on one channel (alpha
/// included). A fully transparent pixel has no color: only alpha counts.
fn distance(a: [u8; 4], b: [u8; 4]) -> u8 {
    if a[3] == 0 || b[3] == 0 {
        return a[3].abs_diff(b[3]);
    }
    (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0)
}

/// `v` × `coverage` / 255.
pub fn scale(v: u8, coverage: u8) -> u8 {
    ((v as u32 * coverage as u32 + 127) / 255) as u8
}

/// Mix of `from` (coverage 0) and `to` (coverage 255), in premultiplied
/// colors: how an edit is applied through a soft selection.
pub fn mix(from: [u8; 4], to: [u8; 4], coverage: u8) -> [u8; 4] {
    match coverage {
        0 => return from,
        255 => return to,
        _ => {}
    }
    let t = coverage as f32 / 255.0;
    let (fa, ta) = (from[3] as f32 * (1.0 - t), to[3] as f32 * t);
    let a = fa + ta;
    if a <= 0.0 {
        return [0, 0, 0, 0];
    }
    let c = |i: usize| ((from[i] as f32 * fa + to[i] as f32 * ta) / a).round() as u8;
    [c(0), c(1), c(2), a.round() as u8]
}

/// Number of worker threads for the pixel loops.
pub fn threads() -> usize {
    static THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()))
}

thread_local! {
    /// Set on threads that already share the work of a larger task
    /// (compositing tiles): the loops below stay on them.
    static SERIAL: Cell<bool> = const { Cell::new(false) };
}

/// From now on, the pixel loops of this thread run on it alone (see
/// [`SERIAL`]).
pub fn run_serially() {
    SERIAL.with(|s| s.set(true));
}

/// Runs `f` on bands of whole rows (`row` elements each) of `data`, in
/// parallel for large data.
pub fn par_bands<T: Send>(data: &mut [T], row: usize, f: impl Fn(&mut [T]) + Sync) {
    let rows = data.len() / row.max(1);
    let n = threads().min(rows);
    if n <= 1 || data.len() < 1 << 16 || SERIAL.with(Cell::get) {
        return f(data);
    }
    let f = &f;
    std::thread::scope(|s| {
        for band in data.chunks_mut(rows.div_ceil(n) * row) {
            s.spawn(move || f(band));
        }
    });
}

/// Runs `f` on each pixel of `image` (RGBA bytes), in parallel.
fn par_pixels(image: &mut RgbaImage, f: impl Fn(&mut [u8]) + Sync) {
    let row = image.width() as usize * 4;
    par_bands(image.as_mut(), row, |band| {
        band.chunks_exact_mut(4).for_each(&f)
    });
}

/// Area of an image where pixels may be modified, with a per-pixel coverage
/// (0..255): the rectangle `rect` (x0, y0, x1, y1 exclusive) to scan,
/// restricted by a shape (selection).
#[derive(Clone, Debug)]
pub struct Clip {
    pub rect: [u32; 4],
    /// Selected shape; without a shape, all of `rect`.
    pub shape: Option<ClipShape>,
    /// Everything but the shape (inverse selection).
    pub inverted: bool,
}

#[derive(Clone, Debug)]
pub enum ClipShape {
    Rect([u32; 4]),
    /// Coverage whose top-left corner is at (x, y), in image coordinates.
    Mask(Arc<GrayImage>, (i64, i64)),
}

impl Clip {
    /// The whole image.
    pub fn all(image: &RgbaImage) -> Self {
        let (w, h) = image.dimensions();
        [0, 0, w, h].into()
    }

    /// Coverage of pixel (x, y): 0 outside the area, 255 fully inside.
    pub fn coverage(&self, x: u32, y: u32) -> u8 {
        let inside = |[x0, y0, x1, y1]: [u32; 4]| x >= x0 && y >= y0 && x < x1 && y < y1;
        if !inside(self.rect) {
            return 0;
        }
        let inner = match &self.shape {
            None => 255,
            Some(ClipShape::Rect(r)) => {
                if inside(*r) {
                    255
                } else {
                    0
                }
            }
            Some(ClipShape::Mask(m, (ox, oy))) => {
                let (mx, my) = (x as i64 - ox, y as i64 - oy);
                if mx >= 0 && my >= 0 && mx < m.width() as i64 && my < m.height() as i64 {
                    m.get_pixel(mx as u32, my as u32).0[0]
                } else {
                    0
                }
            }
        };
        if self.inverted { 255 - inner } else { inner }
    }
}

impl From<[u32; 4]> for Clip {
    fn from(rect: [u32; 4]) -> Self {
        Self {
            rect,
            shape: None,
            inverted: false,
        }
    }
}

/// Paint Bucket: paints `color` over the pixels close to the one at (x, y),
/// within `tolerance` (0..=255) — with `contiguous`, only the connected area
/// (4-connectivity) containing it. `clip` limits the area, its coverage
/// (soft selection) scales the paint.
/// Returns the box (x, y, width, height) of the pixels that changed.
pub fn flood_fill(
    image: &mut RgbaImage,
    x: u32,
    y: u32,
    color: [u8; 4],
    tolerance: u8,
    clip: Option<Clip>,
    contiguous: bool,
) -> Option<[u32; 4]> {
    let (w, h) = image.dimensions();
    let clip = clip.unwrap_or_else(|| Clip::all(image));
    let inside = |x: u32, y: u32| x < w && y < h && clip.coverage(x, y) > 0;
    if !inside(x, y) {
        return None;
    }
    let seed = image.get_pixel(x, y).0;
    let mut changed: Option<[u32; 4]> = None;
    let mut paint = |image: &mut RgbaImage, px: u32, py: u32| {
        let p = image.get_pixel_mut(px, py);
        let [r, g, b, a] = color;
        let new = over(p.0, [r, g, b, scale(a, clip.coverage(px, py))]);
        if new != p.0 {
            p.0 = new;
            changed = Some(match changed {
                Some([x0, y0, x1, y1]) => [x0.min(px), y0.min(py), x1.max(px), y1.max(py)],
                None => [px, py, px, py],
            });
        }
    };
    if contiguous {
        let mut visited = vec![false; (w * h) as usize];
        let mut stack = vec![(x, y)];
        visited[(y * w + x) as usize] = true;
        while let Some((px, py)) = stack.pop() {
            // Unvisited neighbours still have their original color.
            let neighbours = [
                (px.wrapping_sub(1), py),
                (px + 1, py),
                (px, py.wrapping_sub(1)),
                (px, py + 1),
            ];
            for (nx, ny) in neighbours {
                if !inside(nx, ny) {
                    continue;
                }
                let i = (ny * w + nx) as usize;
                if !visited[i] && distance(image.get_pixel(nx, ny).0, seed) <= tolerance {
                    visited[i] = true;
                    stack.push((nx, ny));
                }
            }
            paint(image, px, py);
        }
    } else {
        for py in 0..h {
            for px in 0..w {
                if inside(px, py) && distance(image.get_pixel(px, py).0, seed) <= tolerance {
                    paint(image, px, py);
                }
            }
        }
    }
    changed.map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0 + 1, y1 - y0 + 1])
}

/// Magic Wand: pixels close (within `tolerance`, 0..=255) to the one at
/// (x, y) — with `contiguous`, only the connected area containing it. Mask
/// the size of the image (255: selected).
pub fn magic_wand(image: &RgbaImage, x: u32, y: u32, tolerance: u8, contiguous: bool) -> GrayImage {
    let (w, h) = image.dimensions();
    let mut mask = GrayImage::new(w, h);
    if x >= w || y >= h {
        return mask;
    }
    let seed = image.get_pixel(x, y).0;
    let close = |px: u32, py: u32| distance(image.get_pixel(px, py).0, seed) <= tolerance;
    if !contiguous {
        for (px, py, m) in mask.enumerate_pixels_mut() {
            if close(px, py) {
                m.0[0] = 255;
            }
        }
        return mask;
    }
    let mut stack = vec![(x, y)];
    mask.put_pixel(x, y, image::Luma([255]));
    while let Some((px, py)) = stack.pop() {
        let neighbours = [
            (px.wrapping_sub(1), py),
            (px + 1, py),
            (px, py.wrapping_sub(1)),
            (px, py + 1),
        ];
        for (nx, ny) in neighbours {
            if nx < w && ny < h && mask.get_pixel(nx, ny).0[0] == 0 && close(nx, ny) {
                mask.put_pixel(nx, ny, image::Luma([255]));
                stack.push((nx, ny));
            }
        }
    }
    mask
}

/// Makes the color `key` transparent: distance (0..1) ≤ `tolerance` →
/// transparent; beyond that, over a width of `softness`, gradual
/// transparency, removing the tint of `key` from semi-transparent pixels
/// (no fringe of the old background color).
pub fn color_to_alpha(image: &mut RgbaImage, key: [u8; 3], tolerance: f32, softness: f32) {
    let k = key.map(|c| c as f32 / 255.0);
    for p in image.pixels_mut() {
        let c = [p.0[0], p.0[1], p.0[2]].map(|v| v as f32 / 255.0);
        let d = (0..3).map(|i| (c[i] - k[i]).abs()).fold(0.0, f32::max);
        let keep = if d <= tolerance {
            0.0
        } else if softness <= 0.0 || d >= tolerance + softness {
            1.0
        } else {
            (d - tolerance) / softness
        };
        if keep >= 1.0 {
            continue;
        }
        let alpha = (p.0[3] as f32 * keep).round() as u8;
        if alpha == 0 {
            p.0 = [0, 0, 0, 0];
            continue;
        }
        // The pixel is seen as a mix of its true color (share `keep`) and the
        // removed color: recover the true color.
        for i in 0..3 {
            let v = (c[i] - k[i] * (1.0 - keep)) / keep;
            p.0[i] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        p.0[3] = alpha;
    }
}

/// Color Balance: for each tonal range (shadows, midtones, highlights),
/// three settings from -1 to 1: cyan ↔ red, magenta ↔ green, yellow ↔
/// blue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorBalance {
    pub shadows: [f32; 3],
    pub midtones: [f32; 3],
    pub highlights: [f32; 3],
    /// Keep the luminosity of each pixel (only the hue changes).
    pub preserve_luminosity: bool,
}

impl Default for ColorBalance {
    /// Like Photoshop: "Preserve Luminosity" is checked by default.
    fn default() -> Self {
        Self {
            shadows: [0.0; 3],
            midtones: [0.0; 3],
            highlights: [0.0; 3],
            preserve_luminosity: true,
        }
    }
}

impl ColorBalance {
    pub fn is_neutral(&self) -> bool {
        [self.shadows, self.midtones, self.highlights]
            .iter()
            .all(|r| r.iter().all(|v| *v == 0.0))
    }
}

/// Applies a color balance (GIMP's algorithm, close to Photoshop's). Each
/// correction is weighted by a soft mask based on the pixel's lightness, so
/// that it only affects its tonal range. "Preserve Luminosity" keeps the
/// luminance, like Photoshop.
pub fn color_balance(image: &mut RgbaImage, cb: &ColorBalance) {
    if cb.is_neutral() {
        return;
    }
    const A: f32 = 0.25;
    const B: f32 = 0.333;
    const SCALE: f32 = 0.7;
    par_pixels(image, |p| {
        let rgb = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let lightness = hsl_lightness(rgb);
        let shadows = ((lightness - B) / -A + 0.5).clamp(0.0, 1.0) * SCALE;
        let midtones = ((lightness - B) / A + 0.5).clamp(0.0, 1.0)
            * ((lightness + B - 1.0) / -A + 0.5).clamp(0.0, 1.0)
            * SCALE;
        let highlights = ((lightness + B - 1.0) / A + 0.5).clamp(0.0, 1.0) * SCALE;
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            let v = rgb[i]
                + cb.shadows[i] * shadows
                + cb.midtones[i] * midtones
                + cb.highlights[i] * highlights;
            out[i] = v.clamp(0.0, 1.0);
        }
        if cb.preserve_luminosity {
            out = with_luminance(out, luminance(rgb));
        }
        for (channel, v) in p.iter_mut().zip(out) {
            *channel = (v * 255.0).round() as u8;
        }
    });
}

/// Brightness and contrast, in hundredths (displayed ×100): like in
/// Photoshop, brightness -1.5..1.5 and contrast -0.5..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrightnessContrast {
    pub brightness: f32,
    pub contrast: f32,
}

impl BrightnessContrast {
    pub const BRIGHTNESS: std::ops::RangeInclusive<f32> = -1.5..=1.5;
    pub const CONTRAST: std::ops::RangeInclusive<f32> = -0.5..=1.0;
}

/// Brightness/contrast, like Photoshop: curves that keep black and white —
/// brightness moves the midtones (gamma), contrast stretches them in an S
/// around middle gray. Each channel is processed on its own: a 256-value
/// table is enough.
pub fn brightness_contrast(image: &mut RgbaImage, bc: &BrightnessContrast) {
    let clamp = |v: f32, r: std::ops::RangeInclusive<f32>| v.clamp(*r.start(), *r.end());
    let brightness = clamp(bc.brightness, BrightnessContrast::BRIGHTNESS);
    let contrast = clamp(bc.contrast, BrightnessContrast::CONTRAST);
    // Steepness of the S (contrast -50 is the inverse of +100).
    let steepness = if contrast >= 0.0 { 8.0 } else { 16.0 } * contrast;
    let lut: [u8; 256] = std::array::from_fn(|i| {
        let v = (i as f32 / 255.0).powf(2f32.powf(-brightness));
        (s_curve(v, steepness).clamp(0.0, 1.0) * 255.0).round() as u8
    });
    par_pixels(image, |p| {
        for c in &mut p[..3] {
            *c = lut[*c as usize];
        }
    });
}

/// S curve (normalized sigmoid) that keeps 0 and 1: `steepness` > 0
/// increases contrast, < 0 applies the inverse curve (decreases it).
fn s_curve(t: f32, steepness: f32) -> f32 {
    let a = steepness.abs();
    if a < 1e-3 {
        return t;
    }
    let sigmoid = |x: f32| 1.0 / (1.0 + (-x).exp());
    let lo = sigmoid(-a / 2.0);
    if steepness > 0.0 {
        (sigmoid(a * (t - 0.5)) - lo) / (1.0 - 2.0 * lo)
    } else {
        let u = lo + t * (1.0 - 2.0 * lo);
        0.5 + (u / (1.0 - u)).ln() / a
    }
}

/// Bounds of the levels gamma (like Photoshop).
pub const MIN_GAMMA: f32 = 0.01;
pub const MAX_GAMMA: f32 = 9.99;

/// Levels of one channel: input range (black, white), midtone gamma, output
/// range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelsChannel {
    pub in_black: u8,
    pub in_white: u8,
    /// 1 = neutral; larger lightens the midtones.
    pub gamma: f32,
    pub out_black: u8,
    pub out_white: u8,
}

impl Default for LevelsChannel {
    fn default() -> Self {
        Self {
            in_black: 0,
            in_white: 255,
            gamma: 1.0,
            out_black: 0,
            out_white: 255,
        }
    }
}

impl LevelsChannel {
    /// Lookup table of the 256 values.
    pub fn lut(&self) -> [u8; 256] {
        // At least one level between black and white.
        let lo = self.in_black.min(254);
        let (lo, hi) = (lo as f32, self.in_white.max(lo + 1) as f32);
        let (ob, ow) = (self.out_black as f32, self.out_white as f32);
        let inv_gamma = 1.0 / self.gamma.clamp(MIN_GAMMA, MAX_GAMMA);
        std::array::from_fn(|i| {
            let v = ((i as f32 - lo) / (hi - lo))
                .clamp(0.0, 1.0)
                .powf(inv_gamma);
            (ob + v * (ow - ob)).round().clamp(0.0, 255.0) as u8
        })
    }

    /// Position (0..1, within the input range) of the midtone slider: the
    /// input value that comes out halfway.
    pub fn midpoint(&self) -> f32 {
        0.5f32.powf(self.gamma)
    }

    pub fn set_midpoint(&mut self, t: f32) {
        self.gamma = (t.clamp(0.001, 0.999).ln() / 0.5f32.ln()).clamp(MIN_GAMMA, MAX_GAMMA);
    }

    /// Input range stretched over the histogram: `clip` (0..1) of the pixels
    /// are ignored at each end.
    pub fn auto(histogram: &[u32; 256], clip: f32) -> Self {
        let total: u32 = histogram.iter().sum();
        let limit = (total as f32 * clip) as u32;
        // First level (from each end) where the cumulative sum exceeds the margin.
        let mut sum = 0;
        let lo = (0..256).find(|&i| {
            sum += histogram[i];
            sum > limit
        });
        let mut sum = 0;
        let hi = (0..256).rev().find(|&i| {
            sum += histogram[i];
            sum > limit
        });
        let (lo, hi) = (lo.unwrap_or(0) as u8, hi.unwrap_or(255) as u8);
        // A flat channel (a single level) has nothing to stretch.
        if hi <= lo {
            return Self::default();
        }
        Self {
            in_black: lo,
            in_white: hi,
            ..Default::default()
        }
    }
}

/// Photoshop's auto clipping: 0.1% of the pixels at each end.
pub const AUTO_CLIP: f32 = 0.001;

/// Levels: one setting for the composite (RGB) and one per channel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Levels {
    pub rgb: LevelsChannel,
    pub channels: [LevelsChannel; 3],
}

impl Levels {
    pub fn is_neutral(&self) -> bool {
        let neutral = LevelsChannel::default();
        self.rgb == neutral && self.channels.iter().all(|c| *c == neutral)
    }

    /// Auto (Photoshop's "Enhance Per Channel Contrast"): each channel is
    /// stretched separately, which also corrects a color cast.
    pub fn auto(histograms: &Histograms) -> Self {
        Self {
            rgb: LevelsChannel::default(),
            channels: std::array::from_fn(|c| LevelsChannel::auto(&histograms[c + 1], AUTO_CLIP)),
        }
    }
}

/// Applies levels: first the channel's, then the composite's.
pub fn levels(image: &mut RgbaImage, lv: &Levels) {
    apply_channel_luts(image, lv.rgb.lut(), lv.channels.map(|c| c.lut()));
}

/// Applies each channel's table, then the shared `master` table.
fn apply_channel_luts(image: &mut RgbaImage, master: [u8; 256], own: [[u8; 256]; 3]) {
    let luts = own.map(|own| own.map(|v| master[v as usize]));
    par_pixels(image, |p| {
        for (c, lut) in luts.iter().enumerate() {
            p[c] = lut[p[c] as usize];
        }
    });
}

/// Histograms: [luminosity, red, green, blue], 256 levels each.
pub type Histograms = [[u32; 256]; 4];

/// Histograms of the visible pixels.
pub fn histogram(image: &RgbaImage) -> Histograms {
    let mut h = [[0u32; 256]; 4];
    for p in image.pixels().filter(|p| p.0[3] > 0) {
        let [r, g, b, _] = p.0;
        let luma = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).round() as usize;
        h[0][luma.min(255)] += 1;
        h[1][r as usize] += 1;
        h[2][g as usize] += 1;
        h[3][b as usize] += 1;
    }
    h
}

/// Maximum number of points of a curve (like Photoshop).
pub const MAX_CURVE_POINTS: usize = 14;

/// Tone curve: control points (input, output) in 0..255, sorted by input,
/// joined by a natural cubic spline (like Photoshop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    points: [[f32; 2]; MAX_CURVE_POINTS],
    len: usize,
}

impl Default for Curve {
    fn default() -> Self {
        let mut points = [[0.0; 2]; MAX_CURVE_POINTS];
        points[1] = [255.0, 255.0];
        Self { points, len: 2 }
    }
}

impl Curve {
    pub fn points(&self) -> &[[f32; 2]] {
        &self.points[..self.len]
    }

    /// Curve through `points` (sorted by input, at least two, at most
    /// [`MAX_CURVE_POINTS`]); `None` otherwise.
    pub fn from_points(points: &[[f32; 2]]) -> Option<Curve> {
        let ok = (2..=MAX_CURVE_POINTS).contains(&points.len())
            && points.windows(2).all(|w| w[0][0] < w[1][0])
            && points.iter().flatten().all(|v| (0.0..=255.0).contains(v));
        ok.then(|| {
            let mut c = Curve {
                points: [[0.0; 2]; MAX_CURVE_POINTS],
                len: points.len(),
            };
            c.points[..points.len()].copy_from_slice(points);
            c
        })
    }

    /// Neutral curve: its end points are at the corners and all its points on
    /// the diagonal (the spline is then the diagonal itself).
    pub fn is_identity(&self) -> bool {
        let p = self.points();
        p[0][0] == 0.0 && p[p.len() - 1][0] == 255.0 && p.iter().all(|q| q[0] == q[1])
    }

    /// Adds a point; returns its index, or `None` if the curve is full or a
    /// point already occupies that input.
    pub fn insert(&mut self, x: f32, y: f32) -> Option<usize> {
        let (x, y) = (x.round().clamp(0.0, 255.0), y.round().clamp(0.0, 255.0));
        if self.len == MAX_CURVE_POINTS || self.points().iter().any(|p| (p[0] - x).abs() < 1.0) {
            return None;
        }
        let i = self
            .points()
            .iter()
            .position(|p| p[0] > x)
            .unwrap_or(self.len);
        self.points.copy_within(i..self.len, i + 1);
        self.points[i] = [x, y];
        self.len += 1;
        Some(i)
    }

    /// Moves a point without letting it pass its neighbours.
    pub fn set(&mut self, i: usize, x: f32, y: f32) {
        if i >= self.len {
            return;
        }
        let lo = if i == 0 {
            0.0
        } else {
            self.points[i - 1][0] + 1.0
        };
        let hi = if i + 1 == self.len {
            255.0
        } else {
            self.points[i + 1][0] - 1.0
        };
        self.points[i] = [x.round().clamp(lo, hi), y.round().clamp(0.0, 255.0)];
    }

    /// Removes a point (a curve always keeps at least two points).
    pub fn remove(&mut self, i: usize) -> bool {
        if self.len <= 2 || i >= self.len {
            return false;
        }
        self.points.copy_within(i + 1..self.len, i);
        self.len -= 1;
        true
    }

    /// Value of the curve at `x` (0..255), unclamped.
    pub fn eval(&self, x: f32) -> f32 {
        self.eval_with(x, &self.second_derivatives())
    }

    /// Values of the curve at 0, 1, … 255, unclamped.
    pub fn samples(&self) -> [f32; 256] {
        let m = self.second_derivatives();
        std::array::from_fn(|i| self.eval_with(i as f32, &m))
    }

    /// Value at `x`, with the second derivatives already computed.
    fn eval_with(&self, x: f32, m: &[f32; MAX_CURVE_POINTS]) -> f32 {
        let p = self.points();
        let n = p.len();
        if x <= p[0][0] {
            return p[0][1];
        }
        if x >= p[n - 1][0] {
            return p[n - 1][1];
        }
        let k = p.iter().position(|q| q[0] > x).unwrap_or(n - 1).max(1) - 1;
        let (x0, y0, x1, y1) = (p[k][0], p[k][1], p[k + 1][0], p[k + 1][1]);
        let h = x1 - x0;
        let (a, b) = ((x1 - x) / h, (x - x0) / h);
        a * y0 + b * y1 + ((a * a * a - a) * m[k] + (b * b * b - b) * m[k + 1]) * h * h / 6.0
    }

    /// Second derivatives of the natural spline (Thomas algorithm).
    fn second_derivatives(&self) -> [f32; MAX_CURVE_POINTS] {
        let p = self.points();
        let n = p.len();
        let mut m = [0.0f32; MAX_CURVE_POINTS];
        if n < 3 {
            return m;
        }
        let (mut c, mut d) = ([0.0f32; MAX_CURVE_POINTS], [0.0f32; MAX_CURVE_POINTS]);
        for i in 1..n - 1 {
            let (h0, h1) = (p[i][0] - p[i - 1][0], p[i + 1][0] - p[i][0]);
            let rhs = 6.0 * ((p[i + 1][1] - p[i][1]) / h1 - (p[i][1] - p[i - 1][1]) / h0);
            let diag = 2.0 * (h0 + h1) - h0 * c[i - 1];
            c[i] = h1 / diag;
            d[i] = (rhs - h0 * d[i - 1]) / diag;
        }
        for i in (1..n - 1).rev() {
            m[i] = d[i] - c[i] * m[i + 1];
        }
        m
    }

    /// Lookup table of the 256 values.
    pub fn lut(&self) -> [u8; 256] {
        self.samples().map(|v| v.round().clamp(0.0, 255.0) as u8)
    }
}

/// Curves: one for the composite (RGB) and one per channel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Curves {
    pub rgb: Curve,
    pub channels: [Curve; 3],
}

impl Curves {
    pub fn is_neutral(&self) -> bool {
        self.rgb.is_identity() && self.channels.iter().all(Curve::is_identity)
    }
}

/// Applies curves: first the channel's, then the composite's.
pub fn curves(image: &mut RgbaImage, cv: &Curves) {
    apply_channel_luts(image, cv.rgb.lut(), cv.channels.map(|c| c.lut()));
}

/// Hue / saturation / lightness.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HueSaturation {
    /// Hue rotation in degrees (-180..180); in colorize mode, the hue (0..360).
    pub hue: f32,
    /// -1..1; in colorize mode, the saturation (0..1).
    pub saturation: f32,
    /// -1..1 (towards black or white).
    pub lightness: f32,
    /// A single hue for the whole layer (tinted monochrome image).
    pub colorize: bool,
}

/// Initial saturation of colorize mode (25, like Photoshop).
pub const COLORIZE_SATURATION: f32 = 0.25;

impl HueSaturation {
    pub fn is_neutral(&self) -> bool {
        !self.colorize && self.hue == 0.0 && self.saturation == 0.0 && self.lightness == 0.0
    }
}

/// Hue/saturation (close to Photoshop and GIMP): hue rotation, saturation
/// multiplied (or pushed towards the maximum), lightness pushed towards black
/// or white. In colorize mode, each pixel takes the given hue and
/// saturation, keeping its lightness.
pub fn hue_saturation(image: &mut RgbaImage, hs: &HueSaturation) {
    if hs.is_neutral() {
        return;
    }
    let sat = hs.saturation.clamp(-1.0, 1.0);
    let light = hs.lightness.clamp(-1.0, 1.0);
    par_pixels(image, |p| {
        let rgb = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let (h, s, l) = rgb_to_hsl(rgb);
        let (h, s) = if hs.colorize {
            (hs.hue.rem_euclid(360.0), sat.clamp(0.0, 1.0))
        } else {
            // Multiplicative: a gray (zero saturation) stays gray.
            let s = s * (1.0 + sat);
            ((h + hs.hue).rem_euclid(360.0), s.clamp(0.0, 1.0))
        };
        let mut out = hsl_to_rgb(h, s, l);
        if light != 0.0 {
            out = out.map(|v| {
                if light < 0.0 {
                    v * (1.0 + light)
                } else {
                    v + (1.0 - v) * light
                }
            });
        }
        for (channel, v) in p.iter_mut().zip(out) {
            *channel = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
}

/// Vibrance and saturation, from -1 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vibrance {
    pub vibrance: f32,
    pub saturation: f32,
}

/// Vibrance (like Photoshop): each pixel's saturation changes all the more
/// as it is low, which revives dull colors without pushing those already
/// vivid; skin tones (orange hues) are spared when increasing it. Saturation,
/// on the other hand, acts uniformly. Grays stay gray.
pub fn vibrance(image: &mut RgbaImage, v: &Vibrance) {
    let (vib, sat) = (v.vibrance.clamp(-1.0, 1.0), v.saturation.clamp(-1.0, 1.0));
    if vib == 0.0 && sat == 0.0 {
        return;
    }
    par_pixels(image, |p| {
        let rgb = [p[0], p[1], p[2]].map(|c| c as f32 / 255.0);
        let (h, s, l) = rgb_to_hsl(rgb);
        // Skin tones: around 25° (orange), effect reduced by half at most.
        let skin = if vib > 0.0 {
            1.0 - 0.5 * (1.0 - (h - 25.0).abs() / 25.0).max(0.0)
        } else {
            1.0
        };
        let s = (s * (1.0 + vib * skin * (1.0 - s)) * (1.0 + sat)).clamp(0.0, 1.0);
        for (channel, c) in p.iter_mut().zip(hsl_to_rgb(h, s, l)) {
            *channel = (c * 255.0).round() as u8;
        }
    });
}

/// Hue (degrees) of a color, or `None` for a gray (black and white included).
pub fn hue_of(rgb: [u8; 3]) -> Option<f32> {
    let (h, s, _) = rgb_to_hsl(rgb.map(|c| c as f32 / 255.0));
    (s > 0.0).then_some(h)
}

/// RGB (0..1) → hue (degrees), saturation, lightness (HSL).
fn rgb_to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

/// Hue (degrees), saturation, lightness → RGB (0..1).
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r + m, g + m, b + m].map(|v| v.clamp(0.0, 1.0))
}

/// Kind of image adjustment, in the order of Photoshop's Adjustments menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdjustKind {
    BrightnessContrast,
    Levels,
    Curves,
    Vibrance,
    HueSaturation,
    ColorBalance,
    BlackWhite,
    PhotoFilter,
    Invert,
    Posterize,
    Threshold,
    GradientMap,
}

impl AdjustKind {
    pub fn title(self) -> String {
        match self {
            AdjustKind::BrightnessContrast => t!("adjust-brightness-contrast"),
            AdjustKind::Levels => t!("adjust-levels"),
            AdjustKind::Curves => t!("adjust-curves"),
            AdjustKind::Vibrance => t!("adjust-vibrance"),
            AdjustKind::HueSaturation => t!("adjust-hue-saturation"),
            AdjustKind::ColorBalance => t!("adjust-color-balance"),
            AdjustKind::BlackWhite => t!("core-adjust-black-white"),
            AdjustKind::PhotoFilter => t!("core-adjust-photo-filter"),
            AdjustKind::Invert => t!("core-adjust-invert"),
            AdjustKind::Posterize => t!("core-adjust-posterize"),
            AdjustKind::Threshold => t!("core-adjust-threshold"),
            AdjustKind::GradientMap => t!("core-adjust-gradient-map"),
        }
    }

    /// Neutral adjustment of this kind (Photoshop's defaults for those
    /// that change the image as soon as they are created).
    pub fn initial(self) -> Adjustment {
        match self {
            AdjustKind::ColorBalance => Adjustment::ColorBalance(Default::default()),
            AdjustKind::BrightnessContrast => Adjustment::BrightnessContrast(Default::default()),
            AdjustKind::Levels => Adjustment::Levels(Default::default()),
            AdjustKind::Curves => Adjustment::Curves(Default::default()),
            AdjustKind::HueSaturation => Adjustment::HueSaturation(Default::default()),
            AdjustKind::Vibrance => Adjustment::Vibrance(Default::default()),
            AdjustKind::BlackWhite => Adjustment::BlackWhite(Default::default()),
            AdjustKind::PhotoFilter => Adjustment::PhotoFilter(Default::default()),
            AdjustKind::Invert => Adjustment::Invert,
            AdjustKind::Posterize => Adjustment::Posterize(Default::default()),
            AdjustKind::Threshold => Adjustment::Threshold(Default::default()),
            AdjustKind::GradientMap => Adjustment::GradientMap(Default::default()),
        }
    }
}

// Cloned around (windows, adjustment layers, history): the size of the
// curves costs little.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum Adjustment {
    ColorBalance(ColorBalance),
    BrightnessContrast(BrightnessContrast),
    Levels(Levels),
    Curves(Curves),
    HueSaturation(HueSaturation),
    Vibrance(Vibrance),
    BlackWhite(BlackWhite),
    PhotoFilter(PhotoFilter),
    Invert,
    Posterize(Posterize),
    Threshold(Threshold),
    GradientMap(GradientMap),
}

impl Adjustment {
    pub fn kind(&self) -> AdjustKind {
        match self {
            Adjustment::ColorBalance(_) => AdjustKind::ColorBalance,
            Adjustment::BrightnessContrast(_) => AdjustKind::BrightnessContrast,
            Adjustment::Levels(_) => AdjustKind::Levels,
            Adjustment::Curves(_) => AdjustKind::Curves,
            Adjustment::HueSaturation(_) => AdjustKind::HueSaturation,
            Adjustment::Vibrance(_) => AdjustKind::Vibrance,
            Adjustment::BlackWhite(_) => AdjustKind::BlackWhite,
            Adjustment::PhotoFilter(_) => AdjustKind::PhotoFilter,
            Adjustment::Invert => AdjustKind::Invert,
            Adjustment::Posterize(_) => AdjustKind::Posterize,
            Adjustment::Threshold(_) => AdjustKind::Threshold,
            Adjustment::GradientMap(_) => AdjustKind::GradientMap,
        }
    }

    pub fn title(&self) -> String {
        self.kind().title()
    }

    /// Changes nothing (applying it would be an empty step). Adjustments
    /// that always change the image (invert, threshold…) never are.
    pub fn is_neutral(&self) -> bool {
        match self {
            Adjustment::ColorBalance(cb) => cb.is_neutral(),
            Adjustment::BrightnessContrast(bc) => bc.brightness == 0.0 && bc.contrast == 0.0,
            Adjustment::Levels(lv) => lv.is_neutral(),
            Adjustment::Curves(cv) => cv.is_neutral(),
            Adjustment::HueSaturation(hs) => hs.is_neutral(),
            Adjustment::Vibrance(v) => v.vibrance == 0.0 && v.saturation == 0.0,
            Adjustment::PhotoFilter(f) => f.density == 0.0,
            Adjustment::BlackWhite(_)
            | Adjustment::Invert
            | Adjustment::Posterize(_)
            | Adjustment::Threshold(_)
            | Adjustment::GradientMap(_) => false,
        }
    }

    /// The same adjustment reset to zero (Photoshop's "Reset").
    pub fn reset(&self) -> Adjustment {
        self.kind().initial()
    }

    pub fn apply(&self, image: &mut RgbaImage) {
        match self {
            Adjustment::ColorBalance(cb) => color_balance(image, cb),
            Adjustment::BrightnessContrast(bc) => brightness_contrast(image, bc),
            Adjustment::Levels(lv) => levels(image, lv),
            Adjustment::Curves(cv) => curves(image, cv),
            Adjustment::HueSaturation(hs) => hue_saturation(image, hs),
            Adjustment::Vibrance(v) => vibrance(image, v),
            Adjustment::BlackWhite(bw) => tones::black_white(image, bw),
            Adjustment::PhotoFilter(f) => tones::photo_filter(image, f),
            Adjustment::Invert => tones::invert(image),
            Adjustment::Posterize(p) => tones::posterize(image, p.levels),
            Adjustment::Threshold(t) => tones::threshold(image, t.level),
            Adjustment::GradientMap(g) => tones::gradient_map(image, g),
        }
    }
}

/// HSL lightness: average of the largest and smallest channel.
fn hsl_lightness([r, g, b]: [f32; 3]) -> f32 {
    (r.max(g).max(b) + r.min(g).min(b)) / 2.0
}

/// Luminance (0..1), with the weights of Photoshop's Luminosity mode.
fn luminance([r, g, b]: [f32; 3]) -> f32 {
    0.3 * r + 0.59 * g + 0.11 * b
}

/// Same hue, with luminance `y`, colors kept in gamut (W3C SetLum and
/// ClipColor, as in Photoshop's Luminosity mode).
fn with_luminance(rgb: [f32; 3], y: f32) -> [f32; 3] {
    let d = y - luminance(rgb);
    let c = rgb.map(|v| v + d);
    let l = luminance(c);
    let (lo, hi) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    c.map(|v| {
        let v = if lo < 0.0 {
            l + (v - l) * l / (l - lo)
        } else {
            v
        };
        let v = if hi > 1.0 {
            l + (v - l) * (1.0 - l) / (hi - l)
        } else {
            v
        };
        v.clamp(0.0, 1.0)
    })
}

/// Distance (in pixels) from each pixel to the nearest "filled" pixel
/// (exact Euclidean distance transform, Felzenszwalb & Huttenlocher).
pub fn distance_field(inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    const FAR: f32 = 1e12;
    let mut grid: Vec<f32> = inside.iter().map(|&i| if i { 0.0 } else { FAR }).collect();
    // Rows, then columns (turned into rows by transposition, to read memory
    // sequentially), on squared distances.
    let rows = |grid: &mut [f32], len: usize| {
        par_bands(grid, len, |band| {
            let mut out = vec![0.0f32; len];
            let mut scratch = (vec![0usize; len], vec![0.0f32; len + 1]);
            for line in band.chunks_exact_mut(len) {
                distance_1d(line, &mut out, &mut scratch);
                line.copy_from_slice(&out);
            }
        });
    };
    rows(&mut grid, w);
    let mut grid = transpose(&grid, w, h);
    rows(&mut grid, h);
    let mut grid = transpose(&grid, h, w);
    par_bands(&mut grid, w, |band| {
        band.iter_mut().for_each(|d| *d = d.sqrt())
    });
    grid
}

/// Transpose of a `w` × `h` grid (by blocks, to stay in cache).
fn transpose(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    const BLOCK: usize = 32;
    let mut dst = vec![0.0f32; w * h];
    for by in (0..h).step_by(BLOCK) {
        for bx in (0..w).step_by(BLOCK) {
            for y in by..(by + BLOCK).min(h) {
                for x in bx..(bx + BLOCK).min(w) {
                    dst[x * h + y] = src[y * w + x];
                }
            }
        }
    }
    dst
}

/// 1D transform: out[q] = min_p (f[p] + (q - p)²). `scratch`: work buffers
/// reused from one line to the next.
fn distance_1d(f: &[f32], out: &mut [f32], scratch: &mut (Vec<usize>, Vec<f32>)) {
    let n = f.len();
    let (v, z) = (&mut scratch.0, &mut scratch.1);
    let mut k = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let s = |p: usize| {
            ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32))
        };
        let mut cut = s(v[k]);
        while cut <= z[k] {
            k -= 1;
            cut = s(v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = cut;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let d = q as f32 - v[k] as f32;
        *o = d * d + f[v[k]];
    }
}

/// Fast blur (3 box-filter passes ≈ Gaussian blur) of a field.
pub fn blur(values: &mut [f32], w: usize, h: usize, radius: f32) {
    let r = (radius / 1.7).round() as usize; // 3 passes of radius r ≈ Gaussian of radius `radius`
    if r == 0 {
        return;
    }
    // Horizontal and vertical passes commute: three of each, the vertical
    // ones on the transpose (memory read sequentially).
    let passes = |grid: &mut [f32], len: usize| {
        par_bands(grid, len, |band| {
            let mut tmp = vec![0.0f32; len];
            for line in band.chunks_exact_mut(len) {
                for _ in 0..3 {
                    box_1d(line, r, &mut tmp);
                }
            }
        });
    };
    passes(values, w);
    let mut columns = transpose(values, w, h);
    passes(&mut columns, h);
    values.copy_from_slice(&transpose(&columns, h, w));
}

/// Moving average of radius `r` (edges: divided by the full width, which
/// preserves the total mass). `tmp`: buffer of at least `line.len()`.
fn box_1d(line: &mut [f32], r: usize, tmp: &mut [f32]) {
    let n = line.len();
    let width = (2 * r + 1) as f32;
    let mut sum: f32 = line[..(r + 1).min(n)].iter().sum();
    for i in 0..n {
        tmp[i] = sum / width;
        if i + r + 1 < n {
            sum += line[i + r + 1];
        }
        if i >= r {
            sum -= line[i - r];
        }
    }
    line.copy_from_slice(&tmp[..n]);
}

/// Box (x, y, width, height) of the non-transparent pixels of an image.
pub fn content_box(image: &RgbaImage) -> Option<[u32; 4]> {
    let w = image.width() as usize;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (y, row) in image.as_raw().chunks_exact(w.max(1) * 4).enumerate() {
        let visible = |p: &[u8]| p[3] > 0;
        let Some(left) = row.chunks_exact(4).position(visible) else {
            continue;
        };
        let right = row.chunks_exact(4).rposition(visible).unwrap_or(left);
        (x0, x1) = (x0.min(left), x1.max(right));
        (y0, y1) = (y0.min(y), y);
    }
    (x0 <= x1).then(|| [x0, y0, x1 - x0 + 1, y1 - y0 + 1].map(|v| v as u32))
}

/// Resizes an image in premultiplied alpha, so that the colors hidden under
/// transparent pixels do not bleed into the edges (no dark fringes).
pub fn resize(image: &RgbaImage, w: u32, h: u32, filter: image::imageops::FilterType) -> RgbaImage {
    // 16 bits: color × alpha (≤ 255 × 255) and alpha × 257 are exact.
    let premultiplied = image::ImageBuffer::<Rgba<u16>, Vec<u16>>::from_fn(
        image.width(),
        image.height(),
        |x, y| {
            let [r, g, b, a] = image.get_pixel(x, y).0.map(u16::from);
            Rgba([r * a, g * a, b * a, a * 257])
        },
    );
    let resized = image::imageops::resize(&premultiplied, w, h, filter);
    RgbaImage::from_fn(w, h, |x, y| {
        let [r, g, b, a] = resized.get_pixel(x, y).0.map(u32::from);
        if a < 129 {
            return Rgba([0, 0, 0, 0]);
        }
        let c = |v: u32| ((v * 257 + a / 2) / a).min(255) as u8;
        Rgba([c(r), c(g), c(b), ((a + 128) / 257) as u8])
    })
}

/// "Source over" compositing in non-premultiplied colors: places `src` over
/// `dst`. The project's only implementation (text, brush, effects).
pub fn over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let sa = src[3] as f32 / 255.0;
    if sa <= 0.0 {
        return dst;
    }
    let da = dst[3] as f32 / 255.0;
    let oa = sa + da * (1.0 - sa);
    let mix = |s: u8, d: u8| ((s as f32 * sa + d as f32 * da * (1.0 - sa)) / oa).round() as u8;
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        (oa * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);
    const BLACK: Rgba<u8> = Rgba([0, 0, 0, 255]);

    #[test]
    fn flood_fill_stops_at_borders() {
        // Vertical black line at x = 2: only the left half is filled.
        let mut img = RgbaImage::from_pixel(5, 3, WHITE);
        for y in 0..3 {
            img.put_pixel(2, y, BLACK);
        }
        let r = flood_fill(&mut img, 0, 0, [255, 0, 0, 255], 0, None, true).unwrap();
        assert_eq!(r, [0, 0, 2, 3]);
        assert_eq!(img.get_pixel(1, 2).0, [255, 0, 0, 255]);
        assert_eq!(*img.get_pixel(2, 1), BLACK);
        assert_eq!(*img.get_pixel(4, 1), WHITE);
    }

    #[test]
    fn tolerance_includes_close_colors() {
        let mut img = RgbaImage::from_pixel(3, 1, WHITE);
        img.put_pixel(1, 0, Rgba([250, 250, 250, 255]));
        assert!(
            flood_fill(&mut img.clone(), 0, 0, [0, 0, 255, 255], 0, None, true)
                .is_some_and(|r| r[2] == 1)
        );
        flood_fill(&mut img, 0, 0, [0, 0, 255, 255], 10, None, true);
        assert!(img.pixels().all(|p| p.0 == [0, 0, 255, 255]));
    }

    #[test]
    fn color_to_alpha_removes_key_and_softens_edges() {
        let mut img = RgbaImage::new(3, 1);
        img.put_pixel(0, 0, WHITE);
        img.put_pixel(1, 0, Rgba([128, 128, 255, 255])); // light blue: blue/white mix
        img.put_pixel(2, 0, Rgba([0, 0, 255, 255]));
        color_to_alpha(&mut img, [255, 255, 255], 0.0, 1.0);
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        // 50% mix: half transparent, and back to pure blue.
        let mid = img.get_pixel(1, 0).0;
        assert!((126..=130).contains(&mid[3]), "{mid:?}");
        assert!(mid[0] <= 2 && mid[1] <= 2 && mid[2] == 255, "{mid:?}");
        assert_eq!(img.get_pixel(2, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn color_to_alpha_without_softness_is_binary() {
        let mut img = RgbaImage::from_pixel(1, 2, WHITE);
        img.put_pixel(0, 1, Rgba([240, 240, 240, 255]));
        color_to_alpha(&mut img, [255, 255, 255], 0.1, 0.0);
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_eq!(img.get_pixel(0, 1).0[3], 0);
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([200, 200, 200, 255]));
        color_to_alpha(&mut img, [255, 255, 255], 0.1, 0.0);
        assert_eq!(img.get_pixel(0, 0).0, [200, 200, 200, 255]);
    }

    #[test]
    fn over_on_transparent_keeps_source() {
        assert_eq!(over([0, 0, 0, 0], [10, 20, 30, 128]), [10, 20, 30, 128]);
        assert_eq!(over([255, 255, 255, 255], [0, 0, 0, 255]), [0, 0, 0, 255]);
        assert_eq!(over([1, 2, 3, 4], [9, 9, 9, 0]), [1, 2, 3, 4]);
    }

    #[test]
    fn color_balance_targets_its_tonal_range() {
        let dark = Rgba([40, 40, 40, 255]);
        let light = Rgba([215, 215, 215, 128]);
        let mut img = RgbaImage::from_pixel(2, 1, dark);
        img.put_pixel(1, 0, light);
        // Shadows towards red: the dark gray turns red, the light gray does not move.
        let cb = ColorBalance {
            shadows: [1.0, 0.0, 0.0],
            preserve_luminosity: false,
            ..Default::default()
        };
        color_balance(&mut img, &cb);
        let d = img.get_pixel(0, 0).0;
        assert!(d[0] > 150 && d[1] == 40 && d[2] == 40, "{d:?}");
        assert_eq!(
            *img.get_pixel(1, 0),
            light,
            "highlights untouched, alpha included"
        );
        // Neutral setting: nothing changes.
        let before = img.clone();
        color_balance(&mut img, &ColorBalance::default());
        assert_eq!(img, before);
    }

    #[test]
    fn color_balance_can_preserve_luminosity() {
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([128, 128, 128, 255]));
        let cb = ColorBalance {
            midtones: [0.0, 0.0, 1.0],
            preserve_luminosity: true,
            ..Default::default()
        };
        color_balance(&mut img, &cb);
        let p = img.get_pixel(0, 0).0;
        assert!(p[2] > p[0] && p[2] > p[1], "bluer: {p:?}");
        let y = 0.3 * p[0] as f32 + 0.59 * p[1] as f32 + 0.11 * p[2] as f32;
        assert!((y - 128.0).abs() <= 1.0, "luminosity kept: {y}");
    }

    #[test]
    fn brightness_contrast_behaves() {
        let grays = |values: &[u8]| {
            let mut img = RgbaImage::new(values.len() as u32, 1);
            for (x, v) in values.iter().enumerate() {
                img.put_pixel(x as u32, 0, Rgba([*v, *v, *v, 200]));
            }
            img
        };
        let row = |img: &RgbaImage| img.pixels().map(|p| p.0[0]).collect::<Vec<_>>();
        // Neutral: identical.
        let mut img = grays(&[0, 64, 128, 200, 255]);
        brightness_contrast(&mut img, &BrightnessContrast::default());
        assert_eq!(row(&img), [0, 64, 128, 200, 255]);
        let bc = |brightness, contrast| BrightnessContrast {
            brightness,
            contrast,
        };
        // Brighter: the midtones rise, black and white stay, alpha does not
        // move.
        let mut img = grays(&[0, 128, 255]);
        brightness_contrast(&mut img, &bc(1.0, 0.0));
        let v = row(&img);
        assert!(v[0] == 0 && v[1] > 160 && v[2] == 255, "{v:?}");
        assert_eq!(img.get_pixel(0, 0).0[3], 200);
        // More contrast: darks go down, lights go up.
        let mut img = grays(&[0, 64, 128, 192, 255]);
        brightness_contrast(&mut img, &bc(0.0, 0.5));
        let v = row(&img);
        assert!(
            v[0] == 0 && v[1] < 64 && (127..=129).contains(&v[2]) && v[3] > 192 && v[4] == 255,
            "{v:?}"
        );
        // Minimum contrast (-50): tones compressed, ends kept.
        let mut img = grays(&[0, 64, 192, 255]);
        brightness_contrast(&mut img, &bc(0.0, -0.5));
        let v = row(&img);
        assert!(v[0] == 0 && v[1] > 80 && v[2] < 176 && v[3] == 255, "{v:?}");
    }

    #[test]
    fn levels_map_input_range_and_gamma() {
        let neutral = LevelsChannel::default().lut();
        assert!(neutral.iter().enumerate().all(|(i, v)| *v as usize == i));
        // Input 50..200 stretched over 0..255.
        let lut = LevelsChannel {
            in_black: 50,
            in_white: 200,
            ..Default::default()
        }
        .lut();
        assert_eq!((lut[0], lut[50], lut[200], lut[255]), (0, 0, 255, 255));
        assert_eq!(lut[125], 128);
        // Gamma > 1 lightens the midtones; output clamped.
        let lut = LevelsChannel {
            gamma: 2.0,
            out_black: 20,
            out_white: 230,
            ..Default::default()
        }
        .lut();
        assert!(lut[128] > 150 && lut[0] == 20 && lut[255] == 230);
        // Midtone slider ↔ gamma.
        let mut ch = LevelsChannel::default();
        assert!((ch.midpoint() - 0.5).abs() < 1e-6);
        ch.set_midpoint(0.25);
        assert!((ch.gamma - 2.0).abs() < 1e-4);
    }

    #[test]
    fn levels_apply_channel_then_master() {
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([100, 100, 100, 77]));
        let mut lv = Levels::default();
        lv.channels[0].out_white = 0; // red turned off
        lv.rgb.out_black = 10;
        levels(&mut img, &lv);
        let p = img.get_pixel(0, 0).0;
        assert_eq!(p[0], 10);
        assert!(p[1] > 100 && p[3] == 77);
    }

    #[test]
    fn magic_wand_selects_close_colors() {
        let mut img = RgbaImage::from_pixel(5, 1, Rgba([100, 0, 0, 255]));
        img.put_pixel(1, 0, Rgba([110, 0, 0, 255]));
        img.put_pixel(2, 0, Rgba([0, 0, 255, 255]));
        let row = |m: &GrayImage| m.pixels().map(|p| p.0[0]).collect::<Vec<_>>();
        assert_eq!(row(&magic_wand(&img, 0, 0, 32, true)), [255, 255, 0, 0, 0]);
        assert_eq!(row(&magic_wand(&img, 0, 0, 5, true)), [255, 0, 0, 0, 0]);
        assert_eq!(
            row(&magic_wand(&img, 0, 0, 32, false)),
            [255, 255, 0, 255, 255]
        );
    }

    #[test]
    fn curves_interpolate_through_points() {
        let mut c = Curve::default();
        assert!(c.is_identity());
        // S curve: goes through its points, monotonic here, clamped.
        assert_eq!(c.insert(64.0, 40.0), Some(1));
        assert_eq!(c.insert(192.0, 215.0), Some(2));
        assert_eq!(c.insert(64.4, 0.0), None); // input already taken
        let lut = c.lut();
        assert_eq!((lut[0], lut[64], lut[192], lut[255]), (0, 40, 215, 255));
        assert!(lut.windows(2).all(|w| w[0] <= w[1]));
        assert!(!c.is_identity());
        // A point does not pass its neighbours; the end points stay.
        c.set(1, 500.0, 300.0);
        assert_eq!(c.points()[1], [191.0, 255.0]);
        assert!(c.remove(1) && c.remove(1));
        assert!(!c.remove(0));
        assert!(c.is_identity());
        // End points moved: the curve is flat beyond them.
        c.set(0, 50.0, 30.0);
        let lut = c.lut();
        assert_eq!((lut[0], lut[50]), (30, 30));
    }

    #[test]
    fn curves_apply_channel_then_master() {
        let mut img = RgbaImage::from_pixel(1, 1, Rgba([100, 100, 100, 77]));
        let mut cv = Curves::default();
        cv.channels[2].set(1, 255.0, 0.0); // blue inverted… towards 0
        cv.rgb.insert(100.0, 150.0);
        curves(&mut img, &cv);
        let p = img.get_pixel(0, 0).0;
        assert_eq!(p[0], 150);
        assert_eq!(p[3], 77);
        assert!(p[2] < p[0]);
    }

    #[test]
    fn histogram_and_auto_levels() {
        let mut img = RgbaImage::from_pixel(10, 1, Rgba([60, 60, 60, 255]));
        img.put_pixel(9, 0, Rgba([180, 180, 180, 255]));
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0])); // transparent: ignored
        let h = histogram(&img);
        assert_eq!(h[1][60], 8);
        assert_eq!(h[1][180], 1);
        assert_eq!(h[0].iter().sum::<u32>(), 9);
        let auto = LevelsChannel::auto(&h[0], 0.0);
        assert_eq!((auto.in_black, auto.in_white), (60, 180));
        // Per-channel auto: each channel stretched over its own values.
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([20, 60, 100, 255]));
        img.put_pixel(1, 0, Rgba([220, 200, 180, 255]));
        let lv = Levels::auto(&histogram(&img));
        assert_eq!(lv.rgb, LevelsChannel::default());
        let ends = lv.channels.map(|c| (c.in_black, c.in_white));
        assert_eq!(ends, [(20, 220), (60, 200), (100, 180)]);
    }

    #[test]
    fn hue_saturation_behaves() {
        let px = |rgb: [u8; 3], hs: HueSaturation| {
            let mut img = RgbaImage::from_pixel(1, 1, Rgba([rgb[0], rgb[1], rgb[2], 90]));
            hue_saturation(&mut img, &hs);
            img.get_pixel(0, 0).0
        };
        // 120° rotation: red becomes green.
        let p = px(
            [255, 0, 0],
            HueSaturation {
                hue: 120.0,
                ..Default::default()
            },
        );
        assert_eq!(p, [0, 255, 0, 90]);
        // Saturation -100: gray of the same lightness.
        let p = px(
            [200, 100, 0],
            HueSaturation {
                saturation: -1.0,
                ..Default::default()
            },
        );
        assert!(
            p[0] == p[1] && p[1] == p[2] && (p[0] as i32 - 100).abs() <= 1,
            "{p:?}"
        );
        // Lightness +100: white; -100: black.
        assert_eq!(
            px(
                [30, 60, 90],
                HueSaturation {
                    lightness: 1.0,
                    ..Default::default()
                }
            ),
            [255, 255, 255, 90]
        );
        assert_eq!(
            px(
                [30, 60, 90],
                HueSaturation {
                    lightness: -1.0,
                    ..Default::default()
                }
            ),
            [0, 0, 0, 90]
        );
        // Colorize in blue: a gray turns bluish.
        let p = px(
            [128, 128, 128],
            HueSaturation {
                hue: 240.0,
                saturation: COLORIZE_SATURATION,
                colorize: true,
                ..Default::default()
            },
        );
        assert!(p[2] > p[0] && p[0] == p[1], "{p:?}");
        // Colorize with zero saturation: grayscale.
        let p = px(
            [200, 100, 0],
            HueSaturation {
                hue: 240.0,
                colorize: true,
                ..Default::default()
            },
        );
        assert!(p[0] == p[1] && p[1] == p[2], "{p:?}");
        // Neutral: nothing changes.
        assert_eq!(px([12, 34, 56], HueSaturation::default()), [12, 34, 56, 90]);
    }

    #[test]
    fn vibrance_favours_dull_colors() {
        let sat_of = |p: [u8; 4]| rgb_to_hsl([p[0], p[1], p[2]].map(|c| c as f32 / 255.0)).1;
        let mut img = RgbaImage::new(4, 1);
        img.put_pixel(0, 0, Rgba([110, 120, 140, 255])); // dull
        img.put_pixel(1, 0, Rgba([240, 30, 20, 255])); // vivid
        img.put_pixel(2, 0, Rgba([90, 90, 90, 40])); // gray
        img.put_pixel(3, 0, Rgba([140, 120, 110, 255])); // skin tone, also dull
        let before: Vec<f32> = img.pixels().map(|p| sat_of(p.0)).collect();
        vibrance(
            &mut img,
            &Vibrance {
                vibrance: 0.8,
                saturation: 0.0,
            },
        );
        let after: Vec<f32> = img.pixels().map(|p| sat_of(p.0)).collect();
        let gain = |i: usize| after[i] / before[i];
        assert!(
            gain(0) > gain(1),
            "the dull one gains more than the vivid one: {before:?} → {after:?}"
        );
        assert!(
            gain(3) > 1.0 && gain(3) < gain(0),
            "skin tone spared: {before:?} → {after:?}"
        );
        assert_eq!(
            img.get_pixel(2, 0).0,
            [90, 90, 90, 40],
            "gray untouched, alpha included"
        );
    }

    #[test]
    fn with_luminance_keeps_luminance_in_gamut() {
        assert_eq!(with_luminance([0.3; 3], 0.7), [0.7; 3]);
        // Pure blue brought to a high luminance: clipped, still blue-ish,
        // luminance reached.
        let c = with_luminance([0.0, 0.0, 1.0], 0.5);
        assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?}");
        assert!(c[2] >= c[0] && c[2] >= c[1], "{c:?}");
        assert!((luminance(c) - 0.5).abs() < 1e-4, "{c:?}");
    }

    #[test]
    fn distance_field_is_euclidean() {
        // A single filled pixel at the center of a 7 × 7 grid.
        let mut inside = vec![false; 49];
        inside[3 * 7 + 3] = true;
        let d = distance_field(&inside, 7, 7);
        assert_eq!(d[3 * 7 + 3], 0.0);
        assert_eq!(d[3 * 7 + 6], 3.0);
        assert!((d[0] - 18f32.sqrt()).abs() < 1e-4);
    }

    #[test]
    fn blur_spreads_and_keeps_mass() {
        let mut v = vec![0.0f32; 21 * 21];
        v[10 * 21 + 10] = 1.0;
        blur(&mut v, 21, 21, 4.0);
        let total: f32 = v.iter().sum();
        assert!((total - 1.0).abs() < 1e-3, "{total}");
        assert!(v[10 * 21 + 10] < 0.2 && v[10 * 21 + 12] > 0.0);
    }

    #[test]
    fn auto_levels_leave_flat_channels_alone() {
        // Red logo on transparency: red is 255 everywhere, green and blue 0.
        let mut img = RgbaImage::new(4, 4);
        img.put_pixel(1, 1, Rgba([255, 0, 0, 255]));
        img.put_pixel(2, 1, Rgba([255, 0, 0, 255]));
        let lv = Levels::auto(&histogram(&img));
        assert!(lv.is_neutral(), "{lv:?}");
        // A solid gray layer is not turned black.
        let mut gray = RgbaImage::from_pixel(3, 3, Rgba([128, 128, 128, 255]));
        let auto = Levels::auto(&histogram(&gray));
        levels(&mut gray, &auto);
        assert_eq!(gray.get_pixel(0, 0).0, [128, 128, 128, 255]);
        // Extreme settings never divide by zero.
        let ch = LevelsChannel {
            in_black: 255,
            in_white: 255,
            ..Default::default()
        };
        let lut = ch.lut();
        assert_eq!((lut[0], lut[254], lut[255]), (0, 0, 255));
    }

    #[test]
    fn transparent_pixels_match_whatever_their_hidden_color() {
        // Fully transparent strip with arbitrary RGB underneath (eraser, other tools).
        let mut img = RgbaImage::from_fn(100, 2, |x, _| Rgba([(2 * x) as u8, 0, 255, 0]));
        img.put_pixel(50, 0, Rgba([9, 9, 9, 255]));
        let mask = magic_wand(&img, 0, 1, 32, true);
        assert_eq!(mask.pixels().filter(|m| m.0[0] == 255).count(), 199);
        let r = flood_fill(&mut img, 99, 1, [255, 0, 0, 255], 32, None, true);
        assert_eq!(r, Some([0, 0, 100, 2]));
        assert_eq!(img.get_pixel(50, 0).0, [9, 9, 9, 255]);
    }

    #[test]
    fn bucket_follows_selection_coverage() {
        let mut img = RgbaImage::from_pixel(4, 1, WHITE);
        let mask = Arc::new(GrayImage::from_raw(4, 1, vec![255, 200, 127, 0]).unwrap());
        let clip = Clip {
            rect: [0, 0, 4, 1],
            shape: Some(ClipShape::Mask(mask, (0, 0))),
            inverted: false,
        };
        flood_fill(&mut img, 0, 0, [0, 0, 0, 255], 0, Some(clip), true);
        let v: Vec<u8> = img.pixels().map(|p| p.0[0]).collect();
        assert_eq!(v[0], 0);
        assert!(
            (54..=56).contains(&v[1]) && (127..=129).contains(&v[2]),
            "{v:?}"
        );
        assert_eq!(v[3], 255);
    }

    #[test]
    fn bucket_on_the_same_color_fills_close_colors() {
        let mut img = RgbaImage::from_pixel(3, 1, Rgba([250, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        assert!(flood_fill(&mut img, 0, 0, [255, 0, 0, 255], 32, None, true).is_some());
        assert!(img.pixels().all(|p| p.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn parallel_bands_cover_every_row_once() {
        let mut data = vec![0u32; 1000 * 333];
        par_bands(&mut data, 1000, |band| {
            band.iter_mut().for_each(|v| *v += 1)
        });
        assert!(data.iter().all(|&v| v == 1));
    }

    #[test]
    fn premultiplied_resize_has_no_dark_fringe() {
        let mut img = RgbaImage::new(4, 4);
        img.put_pixel(1, 1, Rgba([255, 255, 255, 255]));
        // Hidden color under a transparent pixel must not bleed in.
        img.put_pixel(2, 1, Rgba([0, 0, 255, 0]));
        let out = resize(&img, 9, 9, image::imageops::FilterType::Triangle);
        for p in out.pixels().filter(|p| p.0[3] > 0) {
            assert!(p.0[..3].iter().all(|&c| c >= 250), "{p:?}");
        }
    }

    #[test]
    fn filling_with_same_color_does_nothing() {
        let mut img = RgbaImage::from_pixel(2, 2, WHITE);
        assert_eq!(flood_fill(&mut img, 0, 0, WHITE.0, 0, None, true), None);
    }
}
