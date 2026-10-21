//! Gradients: color and opacity stops with midpoints, the five shapes
//! (linear, radial, angle, reflected, diamond), dithering, and their
//! rendering into a layer.

use image::RgbaImage;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::i18n::t;
use crate::pixel_ops::{Clip, mix, par_bands};

/// Color of a stop: fixed, or the current foreground / background color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopColor {
    Foreground,
    Background,
    Rgb([u8; 3]),
}

impl StopColor {
    pub fn resolve(self, fg: [u8; 3], bg: [u8; 3]) -> [u8; 3] {
        match self {
            StopColor::Foreground => fg,
            StopColor::Background => bg,
            StopColor::Rgb(c) => c,
        }
    }
}

/// A color stop. `location` 0..1 along the gradient; `midpoint` 0..1: where,
/// between this stop and the next, the two colors are mixed half and half.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorStop {
    pub location: f32,
    pub midpoint: f32,
    pub color: StopColor,
}

/// An opacity stop (opacity 0..1), with its midpoint like a color stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpacityStop {
    pub location: f32,
    pub midpoint: f32,
    pub opacity: f32,
}

/// Smallest and largest midpoint, like Photoshop (5 % to 95 %).
pub const MIDPOINT_RANGE: std::ops::RangeInclusive<f32> = 0.05..=0.95;

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub colors: Vec<ColorStop>,
    pub opacities: Vec<OpacityStop>,
}

/// Gradients offered in the picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    ForegroundToBackground,
    ForegroundToTransparent,
    BlackToWhite,
    Spectrum,
    VioletOrange,
    Copper,
}

impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::ForegroundToBackground,
        Preset::ForegroundToTransparent,
        Preset::BlackToWhite,
        Preset::Spectrum,
        Preset::VioletOrange,
        Preset::Copper,
    ];

    pub fn gradient(self) -> Gradient {
        use StopColor::*;
        let rgb = |c: [u8; 3]| Rgb(c);
        let (colors, opacities): (Vec<StopColor>, Vec<f32>) = match self {
            Preset::ForegroundToBackground => (vec![Foreground, Background], vec![1.0, 1.0]),
            Preset::ForegroundToTransparent => (vec![Foreground, Foreground], vec![1.0, 0.0]),
            Preset::BlackToWhite => (vec![rgb([0, 0, 0]), rgb([255, 255, 255])], vec![1.0, 1.0]),
            Preset::Spectrum => (
                [
                    [255, 0, 0],
                    [255, 255, 0],
                    [0, 255, 0],
                    [0, 255, 255],
                    [0, 0, 255],
                    [255, 0, 255],
                    [255, 0, 0],
                ]
                .map(rgb)
                .to_vec(),
                vec![1.0, 1.0],
            ),
            Preset::VioletOrange => (vec![rgb([41, 10, 89]), rgb([255, 124, 0])], vec![1.0, 1.0]),
            Preset::Copper => (
                vec![
                    rgb([151, 70, 26]),
                    rgb([251, 216, 197]),
                    rgb([108, 46, 22]),
                    rgb([239, 219, 205]),
                ],
                vec![1.0, 1.0],
            ),
        };
        let at = |i: usize, n: usize| i as f32 / (n - 1) as f32;
        Gradient {
            colors: colors
                .iter()
                .enumerate()
                .map(|(i, &color)| ColorStop {
                    location: at(i, colors.len()),
                    midpoint: 0.5,
                    color,
                })
                .collect(),
            opacities: opacities
                .iter()
                .enumerate()
                .map(|(i, &opacity)| OpacityStop {
                    location: at(i, opacities.len()),
                    midpoint: 0.5,
                    opacity,
                })
                .collect(),
        }
    }
}

/// Position between two stops (`f` 0..1) bent so that the midpoint `m` is
/// reached at half: two linear pieces.
pub fn bend(f: f32, m: f32) -> f32 {
    let m = m.clamp(*MIDPOINT_RANGE.start(), *MIDPOINT_RANGE.end());
    if f < m {
        0.5 * f / m
    } else {
        0.5 + 0.5 * (f - m) / (1.0 - m)
    }
}

/// Value at `t` of stops given as (location, midpoint, value), sorted by
/// location: the first value before the first stop, the last after the last.
fn interpolate<const N: usize>(stops: &[(f32, f32, [f32; N])], t: f32) -> [f32; N] {
    let Some(first) = stops.first() else {
        return [0.0; N];
    };
    if t <= first.0 {
        return first.2;
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.0 {
            let span = b.0 - a.0;
            if span <= 1e-6 {
                return b.2;
            }
            let f = bend((t - a.0) / span, a.1);
            return std::array::from_fn(|i| a.2[i] + (b.2[i] - a.2[i]) * f);
        }
    }
    stops[stops.len() - 1].2
}

/// Entries of a gradient's color table.
pub const LUT_SIZE: usize = 1024;

impl Default for Gradient {
    /// Black to white, the default of the layers' gradients.
    fn default() -> Self {
        Preset::BlackToWhite.gradient()
    }
}

/// Stops of a gradient sorted by location, as (location, midpoint, value).
struct Sorted {
    colors: Vec<(f32, f32, [f32; 3])>,
    opacities: Vec<(f32, f32, [f32; 1])>,
}

impl Sorted {
    fn at(&self, t: f32, transparency: bool) -> [f32; 4] {
        let [r, g, b] = interpolate(&self.colors, t);
        let a = if transparency {
            interpolate(&self.opacities, t)[0]
        } else {
            1.0
        };
        [r, g, b, a]
    }
}

impl Gradient {
    fn sorted(&self, fg: [u8; 3], bg: [u8; 3]) -> Sorted {
        let mut colors: Vec<(f32, f32, [f32; 3])> = self
            .colors
            .iter()
            .map(|s| {
                let rgb = s.color.resolve(fg, bg).map(f32::from);
                (s.location, s.midpoint, rgb)
            })
            .collect();
        colors.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut opacities: Vec<(f32, f32, [f32; 1])> = self
            .opacities
            .iter()
            .map(|s| (s.location, s.midpoint, [s.opacity]))
            .collect();
        opacities.sort_by(|a, b| a.0.total_cmp(&b.0));
        Sorted { colors, opacities }
    }

    /// Color (0..255) and opacity (0..1) at `t`, 0..1. Without
    /// `transparency`, the opacity stops are ignored.
    pub fn sample(&self, t: f32, fg: [u8; 3], bg: [u8; 3], transparency: bool) -> [f32; 4] {
        self.sorted(fg, bg).at(t, transparency)
    }

    /// The gradient sampled at `LUT_SIZE` evenly spaced points.
    pub fn lut(&self, fg: [u8; 3], bg: [u8; 3], transparency: bool) -> Vec<[f32; 4]> {
        self.table(LUT_SIZE, fg, bg, transparency)
    }

    /// The gradient sampled at `n` evenly spaced points.
    fn table(&self, n: usize, fg: [u8; 3], bg: [u8; 3], transparency: bool) -> Vec<[f32; 4]> {
        let sorted = self.sorted(fg, bg);
        let last = n.saturating_sub(1).max(1) as f32;
        (0..n)
            .map(|i| sorted.at(i as f32 / last, transparency))
            .collect()
    }

    /// The same gradient with fixed colors: the foreground and background
    /// stops take `fg` and `bg`. Layers keep their gradients this way, like
    /// Photoshop's (a later change of the colors doesn't change them).
    pub fn resolved(&self, fg: [u8; 3], bg: [u8; 3]) -> Gradient {
        let mut g = self.clone();
        for s in &mut g.colors {
            s.color = StopColor::Rgb(s.color.resolve(fg, bg));
        }
        g
    }

    /// `n` colors of a gradient with fixed colors (see [`Self::resolved`]),
    /// evenly spaced, non-premultiplied 0..1, opacity stops included.
    pub fn ramp(&self, n: usize) -> Vec<[f32; 4]> {
        let mut table = self.table(n, [0; 3], [255; 3], true);
        for c in &mut table {
            for v in &mut c[..3] {
                *v /= 255.0;
            }
        }
        table
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

impl Shape {
    pub const ALL: [Shape; 5] = [
        Shape::Linear,
        Shape::Radial,
        Shape::Angle,
        Shape::Reflected,
        Shape::Diamond,
    ];

    pub fn label(self) -> String {
        match self {
            Shape::Linear => t!("paint-gradient-linear"),
            Shape::Radial => t!("paint-gradient-radial"),
            Shape::Angle => t!("paint-gradient-angle"),
            Shape::Reflected => t!("paint-gradient-reflected"),
            Shape::Diamond => t!("paint-gradient-diamond"),
        }
    }

    /// Position (0..1) in the gradient of point `p`, for a gradient drawn
    /// from `start` to `end`.
    pub fn position(self, start: [f32; 2], end: [f32; 2], p: [f32; 2]) -> f32 {
        let d = [end[0] - start[0], end[1] - start[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        if len2 <= 1e-12 {
            return 1.0;
        }
        let q = [p[0] - start[0], p[1] - start[1]];
        // Along the line and across it, in lengths of the line.
        let along = (q[0] * d[0] + q[1] * d[1]) / len2;
        let across = (q[1] * d[0] - q[0] * d[1]) / len2;
        let t = match self {
            Shape::Linear => along,
            Shape::Reflected => along.abs(),
            Shape::Radial => (q[0] * q[0] + q[1] * q[1]).sqrt() / len2.sqrt(),
            Shape::Diamond => along.abs() + across.abs(),
            // Counterclockwise on screen from the line (y points down).
            Shape::Angle => {
                let turn = (-across).atan2(along);
                turn.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
            }
        };
        t.clamp(0.0, 1.0)
    }
}

/// How a gradient is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub shape: Shape,
    pub reverse: bool,
    pub dither: bool,
    /// Opacity of the whole gradient, 0..1.
    pub opacity: f32,
}

/// Dither noise for pixel (x, y), channel `c`: triangular, -1..1 (in levels).
fn noise(x: u32, y: u32, c: u32) -> f32 {
    let hash = |mut h: u32| {
        h ^= h >> 16;
        h = h.wrapping_mul(0x7feb_352d);
        h ^= h >> 15;
        h = h.wrapping_mul(0x846c_a68b);
        h ^ (h >> 16)
    };
    let h = hash(x.wrapping_mul(0x9e37_79b9) ^ hash(y.wrapping_add(c.wrapping_mul(0x632b_e5ab))));
    let a = (h & 0xffff) as f32 / 65535.0;
    let b = (h >> 16) as f32 / 65535.0;
    a + b - 1.0
}

/// The gradient's pixel at canvas pixel (x, y) (`lut`, then the opacity),
/// before blending.
fn pixel(
    lut: &[[f32; 4]],
    params: &Params,
    start: [f32; 2],
    end: [f32; 2],
    x: i64,
    y: i64,
) -> [u8; 4] {
    let p = [x as f32 + 0.5, y as f32 + 0.5];
    let mut t = params.shape.position(start, end, p);
    if params.reverse {
        t = 1.0 - t;
    }
    let i = (t * (lut.len() - 1) as f32).round() as usize;
    let [r, g, b, a] = lut[i.min(lut.len() - 1)];
    let a = a * 255.0 * params.opacity.clamp(0.0, 1.0);
    let q = |v: f32, c: u32| {
        // Black, white, opaque and transparent stay exact.
        let n = if params.dither && v > 0.0 && v < 255.0 {
            noise(x as u32, y as u32, c)
        } else {
            0.0
        };
        (v + n).round().clamp(0.0, 255.0) as u8
    };
    [q(r, 0), q(g, 1), q(b, 2), q(a, 3)]
}

/// Draws the gradient from `start` to `end` (canvas coordinates) into
/// `image`, a layer placed at `offset`, within `clip` (layer coordinates).
/// `blend` puts a gradient pixel over a layer pixel (mode), at a canvas
/// pixel. Rows are drawn in parallel. Returns whether a pixel changed.
#[allow(clippy::too_many_arguments)]
pub fn render(
    image: &mut RgbaImage,
    offset: (i64, i64),
    clip: &Clip,
    lut: &[[f32; 4]],
    params: &Params,
    start: [f32; 2],
    end: [f32; 2],
    blend: &(dyn Fn([u8; 4], [u8; 4], i64, i64) -> [u8; 4] + Sync),
) -> bool {
    let w = image.width() as usize;
    let [x0, y0, x1, y1] = clip.rect;
    if x0 >= x1 || y0 >= y1 || lut.is_empty() {
        return false;
    }
    let shaped = clip.shape.is_some() || clip.inverted;
    let rows = &mut image.as_mut()[y0 as usize * w * 4..y1 as usize * w * 4];
    // Bands know their first row from where they start in `rows`.
    let base = rows.as_ptr() as usize;
    let changed = AtomicBool::new(false);
    par_bands(rows, w * 4, |band| {
        let first = y0 as usize + (band.as_ptr() as usize - base) / (w * 4);
        let mut any = false;
        for (j, row) in band.chunks_exact_mut(w * 4).enumerate() {
            let y = (first + j) as u32;
            for x in x0..x1 {
                let coverage = if shaped { clip.coverage(x, y) } else { 255 };
                if coverage == 0 {
                    continue;
                }
                let (cx, cy) = (x as i64 + offset.0, y as i64 + offset.1);
                let src = pixel(lut, params, start, end, cx, cy);
                let d = &mut row[x as usize * 4..x as usize * 4 + 4];
                let dst = [d[0], d[1], d[2], d[3]];
                let out = mix(dst, blend(dst, src, cx, cy), coverage);
                any |= out != dst;
                d.copy_from_slice(&out);
            }
        }
        if any {
            changed.store(true, Ordering::Relaxed);
        }
    });
    changed.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_ops::over;

    const FG: [u8; 3] = [255, 0, 0];
    const BG: [u8; 3] = [0, 0, 255];

    fn two(m: f32) -> Gradient {
        let mut g = Preset::BlackToWhite.gradient();
        g.colors[0].midpoint = m;
        g
    }

    #[test]
    fn stops_set_the_colors_and_the_ends_extend() {
        let g = Preset::ForegroundToBackground.gradient();
        assert_eq!(g.sample(0.0, FG, BG, true), [255.0, 0.0, 0.0, 1.0]);
        assert_eq!(g.sample(1.0, FG, BG, true), [0.0, 0.0, 255.0, 1.0]);
        assert_eq!(g.sample(0.5, FG, BG, true), [127.5, 0.0, 127.5, 1.0]);
        // Stops inside the gradient: before the first, its color.
        let mut inside = g.clone();
        inside.colors[0].location = 0.25;
        inside.colors[1].location = 0.75;
        assert_eq!(inside.sample(0.1, FG, BG, true)[0], 255.0);
        assert_eq!(inside.sample(0.9, FG, BG, true)[2], 255.0);
        assert_eq!(inside.sample(0.5, FG, BG, true)[0], 127.5);
    }

    #[test]
    fn stops_out_of_order_are_sorted() {
        let mut g = Preset::BlackToWhite.gradient();
        g.colors.swap(0, 1);
        assert_eq!(g.sample(0.0, FG, BG, true)[0], 0.0);
        assert_eq!(g.sample(1.0, FG, BG, true)[0], 255.0);
    }

    #[test]
    fn midpoint_is_where_the_colors_are_half_mixed() {
        for m in [0.2, 0.5, 0.8] {
            let v = two(m).sample(m, FG, BG, true)[0];
            assert!((v - 127.5).abs() < 0.01, "{m}: {v}");
        }
        // Moved toward the start: the second color comes sooner.
        assert!(two(0.2).sample(0.4, FG, BG, true)[0] > two(0.5).sample(0.4, FG, BG, true)[0]);
        // Out of range: kept within 5 %..95 %.
        assert_eq!(bend(0.5, 0.0), bend(0.5, 0.05));
    }

    #[test]
    fn opacity_stops_and_transparency_option() {
        let g = Preset::ForegroundToTransparent.gradient();
        assert_eq!(g.sample(0.0, FG, BG, true)[3], 1.0);
        assert_eq!(g.sample(1.0, FG, BG, true)[3], 0.0);
        assert_eq!(g.sample(0.5, FG, BG, true)[3], 0.5);
        assert_eq!(g.sample(1.0, FG, BG, false)[3], 1.0, "Transparency off");
        // The color stays the foreground's all along.
        assert_eq!(g.sample(1.0, FG, BG, true)[0], 255.0);
    }

    #[test]
    fn shapes() {
        let (s, e) = ([0.0, 0.0], [10.0, 0.0]);
        let at = |shape: Shape, p: [f32; 2]| shape.position(s, e, p);
        assert_eq!(at(Shape::Linear, [5.0, 7.0]), 0.5);
        assert_eq!(at(Shape::Linear, [-5.0, 0.0]), 0.0);
        assert_eq!(at(Shape::Linear, [20.0, 0.0]), 1.0);
        assert_eq!(at(Shape::Reflected, [-5.0, 3.0]), 0.5);
        assert_eq!(at(Shape::Radial, [0.0, 5.0]), 0.5);
        assert_eq!(at(Shape::Radial, [6.0, 8.0]), 1.0);
        // Diamond: the end is a corner.
        assert_eq!(at(Shape::Diamond, [2.5, 2.5]), 0.5);
        assert_eq!(at(Shape::Diamond, [0.0, -10.0]), 1.0);
        // Angle: a turn counterclockwise from the line (screen: y down).
        assert_eq!(at(Shape::Angle, [5.0, 0.0]), 0.0);
        assert!((at(Shape::Angle, [0.0, -5.0]) - 0.25).abs() < 1e-6);
        assert!((at(Shape::Angle, [-5.0, 0.0]) - 0.5).abs() < 1e-6);
        assert!((at(Shape::Angle, [0.0, 5.0]) - 0.75).abs() < 1e-6);
        // Lines in other directions: only the relative position counts.
        let diagonal = Shape::Linear.position([2.0, 2.0], [12.0, 12.0], [7.0, 7.0]);
        assert!((diagonal - 0.5).abs() < 1e-6);
    }

    fn ramp(dither: bool) -> RgbaImage {
        let mut image = RgbaImage::new(256, 4);
        let lut = Preset::BlackToWhite.gradient().lut(FG, BG, true);
        let params = Params {
            shape: Shape::Linear,
            reverse: false,
            dither,
            opacity: 1.0,
        };
        let clip = Clip::all(&image);
        render(
            &mut image,
            (0, 0),
            &clip,
            &lut,
            &params,
            [0.0, 0.0],
            [256.0, 0.0],
            &|d, s, _, _| over(d, s),
        );
        image
    }

    #[test]
    fn dither_stays_within_one_level() {
        let (plain, dithered) = (ramp(false), ramp(true));
        let mut differ = 0;
        for (a, b) in plain.pixels().zip(dithered.pixels()) {
            for c in 0..3 {
                assert!(a.0[c].abs_diff(b.0[c]) <= 1, "{:?} {:?}", a.0, b.0);
            }
            differ += (a.0 != b.0) as usize;
            assert_eq!(b.0[3], 255, "opaque stays opaque");
        }
        assert!(differ > 100, "the dither changes many pixels: {differ}");
        for c in [0.0f32, 255.0] {
            for x in 0..64 {
                for y in 0..64 {
                    let v = c + noise(x, y, 0);
                    assert!((-1.0..=256.0).contains(&v));
                }
            }
        }
    }

    #[test]
    fn render_respects_the_clip_and_the_reverse() {
        let mut image = RgbaImage::from_pixel(10, 10, image::Rgba([9, 9, 9, 255]));
        let lut = Preset::BlackToWhite.gradient().lut(FG, BG, true);
        let params = Params {
            shape: Shape::Linear,
            reverse: true,
            dither: false,
            opacity: 1.0,
        };
        // A layer placed at (100, 0) of the canvas; the clip keeps x < 5.
        let clip: Clip = [0, 0, 5, 10].into();
        let changed = render(
            &mut image,
            (100, 0),
            &clip,
            &lut,
            &params,
            [100.0, 0.0],
            [110.0, 0.0],
            &|d, s, _, _| over(d, s),
        );
        assert!(changed);
        assert_eq!(image.get_pixel(0, 0).0[0], 242, "reversed: white first");
        assert_eq!(image.get_pixel(5, 0).0, [9, 9, 9, 255]);
    }
}
