//! Exposure, Channel Mixer, Selective Color, the Auto commands (Levels
//! settings found in the image), Fade and the stroke of a selection.

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::{
    AUTO_CLIP, Clip, Levels, LevelsChannel, MAX_GAMMA, MIN_GAMMA, distance_field, histogram_within,
    mix, par_pixels,
};
use crate::compose::{BlendMode, blend};
use crate::document::PixelRect;
use crate::filters::par_rows;
use crate::selection::Selection;

/// Exposure: stops, offset and gamma correction, in linear light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exposure {
    /// Stops, -20 to 20.
    pub exposure: f32,
    /// Added in linear light, -0.5 to 0.5.
    pub offset: f32,
    /// 0.01 to 9.99, 1 changing nothing.
    pub gamma: f32,
}

impl Exposure {
    pub const EXPOSURE: f32 = 20.0;
    pub const OFFSET: f32 = 0.5;
    pub const GAMMA: std::ops::RangeInclusive<f32> = 0.01..=9.99;
}

impl Default for Exposure {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            offset: 0.0,
            gamma: 1.0,
        }
    }
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

pub fn exposure(image: &mut RgbaImage, e: &Exposure) {
    let gain = e.exposure.clamp(-Exposure::EXPOSURE, Exposure::EXPOSURE).exp2();
    let inverse = 1.0 / e.gamma.clamp(*Exposure::GAMMA.start(), *Exposure::GAMMA.end());
    let lut: [u8; 256] = std::array::from_fn(|i| {
        let v = to_linear(i as f32 / 255.0) * gain + e.offset;
        let v = v.max(0.0).powf(inverse).min(1.0);
        (to_srgb(v) * 255.0).round() as u8
    });
    par_pixels(image, |p| {
        for c in &mut p[..3] {
            *c = lut[*c as usize];
        }
    });
}

/// Channel Mixer: per output channel, the shares of red, green and blue and
/// a constant (1 = 100%). Monochrome gives the three channels the same mix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelMixer {
    pub rows: [[f32; 4]; 3],
    pub monochrome: bool,
}

impl ChannelMixer {
    /// The limit of a share or constant, either way (200%).
    pub const LIMIT: f32 = 2.0;
    /// What Monochrome starts from.
    pub const GRAY: [f32; 4] = [0.4, 0.4, 0.2, 0.0];
}

impl Default for ChannelMixer {
    fn default() -> Self {
        Self {
            rows: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            monochrome: false,
        }
    }
}

pub fn channel_mixer(image: &mut RgbaImage, m: &ChannelMixer) {
    let rows = m.rows;
    par_pixels(image, |p| {
        let [r, g, b] = [p[0], p[1], p[2]].map(f32::from);
        for (c, row) in p.iter_mut().zip(&rows) {
            let v = r * row[0] + g * row[1] + b * row[2] + row[3] * 255.0;
            *c = v.round().clamp(0.0, 255.0) as u8;
        }
    });
}

/// The ranges of Selective Color, in the order of its menu and of the file
/// formats: reds, yellows, greens, cyans, blues, magentas, whites, neutrals,
/// blacks.
pub const SELECTIVE_RANGES: usize = 9;

/// Selective Color: the cyan, magenta, yellow and black corrections of each
/// range (-1 to 1); Absolute (`true`) or Relative method.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SelectiveColor {
    pub ranges: [[f32; 4]; SELECTIVE_RANGES],
    pub absolute: bool,
}

pub fn selective_color(image: &mut RgbaImage, s: &SelectiveColor) {
    let s = *s;
    // The range tests below stay out of the way when a range is at zero.
    let active: [bool; SELECTIVE_RANGES] = s.ranges.map(|r| r != [0.0; 4]);
    par_pixels(image, |p| {
        let [r, g, b] = [p[0], p[1], p[2]].map(|c| c as f32 / 255.0);
        let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
        let mid = r + g + b - lo - hi;
        let scales = [
            if r == hi { hi - mid } else { 0.0 },
            if b == lo { mid - lo } else { 0.0 },
            if g == hi { hi - mid } else { 0.0 },
            if r == lo { mid - lo } else { 0.0 },
            if b == hi { hi - mid } else { 0.0 },
            if g == lo { mid - lo } else { 0.0 },
            if lo > 0.5 { (lo - 0.5) * 2.0 } else { 0.0 },
            if hi > 0.0 && lo < 1.0 {
                1.0 - ((hi - 0.5).abs() + (lo - 0.5).abs())
            } else {
                0.0
            },
            if hi < 0.5 { (0.5 - hi) * 2.0 } else { 0.0 },
        ];
        let mut out = [r, g, b];
        for (i, &scale) in scales.iter().enumerate() {
            if !active[i] || scale <= 0.0 {
                continue;
            }
            let [c, m, y, k] = s.ranges[i];
            for (o, (value, ink)) in out.iter_mut().zip([(r, c), (g, m), (b, y)]) {
                let mut res = (-1.0 - ink) * k - ink;
                if !s.absolute {
                    res *= 1.0 - value;
                }
                *o += res.clamp(-value, 1.0 - value) * scale;
            }
        }
        for (c, v) in p.iter_mut().zip(out) {
            *c = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    });
}

/// Image › Auto Tone, Auto Contrast, Auto Color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AutoMode {
    /// Each channel stretched on its own.
    Tone,
    /// The channels stretched together: colors keep their hue.
    Contrast,
    /// The darkest and lightest colors become black and white, the nearly
    /// neutral midtones neutral.
    Color,
}

/// Levels `mode` finds in the visible pixels of `image`.
#[cfg(test)]
pub fn auto_levels(image: &RgbaImage, mode: AutoMode) -> Levels {
    auto_levels_within(image, None, mode)
}

/// Levels `mode` finds in the visible pixels of `image` inside `clip`, each
/// counting as much as it is selected.
pub fn auto_levels_within(image: &RgbaImage, clip: Option<&Clip>, mode: AutoMode) -> Levels {
    match mode {
        AutoMode::Tone => Levels::auto(&histogram_within(image, clip)),
        AutoMode::Contrast => {
            let h = histogram_within(image, clip);
            let all: [u32; 256] = std::array::from_fn(|i| h[1][i] + h[2][i] + h[3][i]);
            Levels {
                rgb: LevelsChannel::auto(&all, AUTO_CLIP),
                ..Levels::default()
            }
        }
        AutoMode::Color => auto_color(image, clip),
    }
}

/// Calls `f` with each visible pixel of `image` inside `clip` and its weight.
fn each_selected(image: &RgbaImage, clip: Option<&Clip>, mut f: impl FnMut([u8; 4], u64)) {
    let Some(clip) = clip else {
        for p in image.pixels().filter(|p| p.0[3] > 0) {
            f(p.0, 255);
        }
        return;
    };
    let [x0, y0, x1, y1] = clip.rect;
    for y in y0..y1.min(image.height()) {
        for x in x0..x1.min(image.width()) {
            let p = image.get_pixel(x, y).0;
            let weight = u64::from(clip.coverage(x, y));
            if p[3] > 0 && weight > 0 {
                f(p, weight);
            }
        }
    }
}

fn auto_color(image: &RgbaImage, clip: Option<&Clip>) -> Levels {
    let luma = |p: [u8; 4]| ((77 * p[0] as u32 + 151 * p[1] as u32 + 28 * p[2] as u32) >> 8) as u8;
    let mut levels = [0u64; 256];
    each_selected(image, clip, |p, weight| levels[luma(p) as usize] += weight);
    let total: u64 = levels.iter().sum();
    let limit = (total as f32 * AUTO_CLIP) as u64;
    let mut sum = 0;
    let lo = (0..256).find(|&i| {
        sum += levels[i];
        sum > limit
    });
    let mut sum = 0;
    let hi = (0..256).rev().find(|&i| {
        sum += levels[i];
        sum > limit
    });
    let (Some(lo), Some(hi)) = (lo, hi) else {
        return Levels::default();
    };
    if hi <= lo {
        return Levels::default();
    }
    // The average colors of the darkest and of the lightest pixels.
    let (mut dark, mut light) = (([0u64; 3], 0u64), ([0u64; 3], 0u64));
    each_selected(image, clip, |p, weight| {
        let l = luma(p) as usize;
        for (side, on) in [(&mut dark, l <= lo), (&mut light, l >= hi)] {
            if on {
                side.1 += weight;
                for (sum, v) in side.0.iter_mut().zip(p) {
                    *sum += v as u64 * weight;
                }
            }
        }
    });
    let average = |(s, n): ([u64; 3], u64)| s.map(|v| (v / n.max(1)) as u8);
    let (dark, light) = (average(dark), average(light));
    let mut channels: [LevelsChannel; 3] = std::array::from_fn(|c| {
        if light[c] > dark[c] {
            LevelsChannel {
                in_black: dark[c],
                in_white: light[c],
                ..Default::default()
            }
        } else {
            LevelsChannel::default()
        }
    });
    // The midtones: the average nearly neutral color, once stretched,
    // becomes a gray through each channel's gamma.
    let luts = channels.map(|c| c.lut());
    let (mut mids, mut n) = ([0u64; 3], 0u64);
    each_selected(image, clip, |p, weight| {
        let q = [0, 1, 2].map(|c| luts[c][p[c] as usize]);
        let (lo, hi) = (q[0].min(q[1]).min(q[2]), q[0].max(q[1]).max(q[2]));
        if hi - lo < 40 && lo >= 32 && hi <= 224 {
            n += weight;
            for (sum, v) in mids.iter_mut().zip(q) {
                *sum += v as u64 * weight;
            }
        }
    });
    if n > 0 {
        let mean = mids.map(|v| v as f32 / n as f32 / 255.0);
        let gray = (mean[0] + mean[1] + mean[2]) / 3.0;
        for (channel, m) in channels.iter_mut().zip(mean) {
            channel.gamma = (m.ln() / gray.ln()).clamp(MIN_GAMMA, MAX_GAMMA);
        }
    }
    Levels {
        rgb: LevelsChannel::default(),
        channels,
    }
}

/// The pixels Edit › Fade blends: what the step it fades covered. Outside,
/// the pixels stay; inside, even those it left as they were are blended, as
/// a copy of the result over the original would be.
#[derive(Clone)]
pub enum FadeArea {
    /// The whole frame.
    All,
    /// The pixels that differ.
    Changed,
    /// What it worked in, with its coverage: the selection, or what a
    /// brush, the Paint Bucket or a stroke covered.
    Within(Clip),
    /// The pixels that differ and this area: what a transform moved, and
    /// where it put it.
    ChangedOr(Clip),
}

/// What Edit › Fade brings the last result back toward: the pixels before
/// it, in the frame of the current ones.
#[derive(Clone)]
pub struct Fade {
    pub before: Arc<RgbaImage>,
    pub area: FadeArea,
    /// Canvas position of the frame's corner (Dissolve's pattern).
    pub origin: (i64, i64),
    /// 0..1.
    pub opacity: f32,
    pub mode: BlendMode,
}

// A comparison of the settings: the pixels are the same object or differ,
// and the area goes with them.
impl PartialEq for Fade {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.before, &other.before)
            && self.origin == other.origin
            && self.opacity == other.opacity
            && self.mode == other.mode
    }
}

impl Fade {
    /// `after`, the result, faded: the result blended in `mode` over what
    /// was there, at `opacity`, in the area of the step.
    pub fn apply(&self, after: &mut RgbaImage) {
        if after.dimensions() != self.before.dimensions() {
            return;
        }
        let row = after.width() as usize * 4;
        let before = self.before.as_raw();
        let (origin, opacity, mode) = (self.origin, self.opacity.clamp(0.0, 1.0), self.mode);
        let area = &self.area;
        // The result already carries a soft selection; a pixel it left alone
        // comes out the same. Other modes blend it with itself.
        let plain = matches!(
            mode,
            BlendMode::Normal | BlendMode::PassThrough | BlendMode::Dissolve
        );
        par_rows(after.as_mut(), row, |first, band| {
            let w = row / 4;
            for (i, p) in band.chunks_exact_mut(4).enumerate() {
                let at = first * row + i * 4;
                let b = [before[at], before[at + 1], before[at + 2], before[at + 3]];
                let same = *p == b;
                if same && (plain || b[3] == 0) {
                    continue;
                }
                let (ix, iy) = ((i % w) as u32, (first + i / w) as u32);
                let coverage = match area {
                    FadeArea::All => 255,
                    FadeArea::Changed if same => 0,
                    FadeArea::Changed => 255,
                    FadeArea::Within(clip) => clip.coverage(ix, iy),
                    FadeArea::ChangedOr(clip) if same => clip.coverage(ix, iy),
                    FadeArea::ChangedOr(_) => 255,
                };
                if coverage == 0 {
                    continue;
                }
                let (x, y) = (origin.0 + i64::from(ix), origin.1 + i64::from(iy));
                let out = fade_pixel(b, [p[0], p[1], p[2], p[3]], opacity, mode, x, y);
                let out = if plain { out } else { mix(b, out, coverage) };
                p.copy_from_slice(&out);
            }
        });
    }
}

fn fade_pixel(b: [u8; 4], s: [u8; 4], opacity: f32, mode: BlendMode, x: i64, y: i64) -> [u8; 4] {
    let (t, mode) = if mode == BlendMode::Dissolve {
        (blend::dissolve(opacity, x, y), BlendMode::Normal)
    } else {
        (opacity, mode)
    };
    let (ba, sa) = (b[3] as f32 / 255.0, s[3] as f32 / 255.0);
    let bc = [0, 1, 2].map(|i| b[i] as f32 / 255.0);
    let sc = [0, 1, 2].map(|i| s[i] as f32 / 255.0);
    let mixed = if ba > 0.0 && !matches!(mode, BlendMode::Normal | BlendMode::PassThrough) {
        blend::blend_color(mode, bc, sc)
    } else {
        sc
    };
    let a = ba + t * (sa - ba);
    if a <= 0.0 {
        return [0; 4];
    }
    let to_byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let c = |i: usize| {
        let target = (1.0 - ba) * sc[i] + ba * mixed[i];
        let pre = bc[i] * ba + t * (target * sa - bc[i] * ba);
        to_byte(pre / a)
    };
    [c(0), c(1), c(2), to_byte(a)]
}

/// Where a stroke lies around the edge of the selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StrokeLocation {
    Inside,
    #[default]
    Center,
    Outside,
}

/// Settings of Edit › Stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeSettings {
    /// In pixels.
    pub width: u32,
    pub color: [u8; 3],
    pub location: StrokeLocation,
    pub mode: BlendMode,
    /// 0..1.
    pub opacity: f32,
    pub preserve_transparency: bool,
}

impl StrokeSettings {
    pub const MAX_WIDTH: u32 = 250;
}

/// `image` (its corner at the canvas position `origin`) with the edge of
/// `selection` stroked; without one, the edge of its visible pixels.
#[cfg(test)]
fn stroke(
    image: &RgbaImage,
    origin: (i64, i64),
    selection: Option<&Selection>,
    canvas: PixelRect,
    s: &StrokeSettings,
) -> RgbaImage {
    stroke_covering(image, origin, selection, canvas, s).0
}

/// `image` (its corner at the canvas position `origin`) with the edge of
/// `selection` stroked (without one, the edge of its visible pixels), and
/// the coverage of the band (what Fade blends) with its canvas position.
pub fn stroke_covering(
    image: &RgbaImage,
    origin: (i64, i64),
    selection: Option<&Selection>,
    canvas: PixelRect,
    s: &StrokeSettings,
) -> (RgbaImage, GrayImage, (i64, i64)) {
    let mut out = image.clone();
    let width = s.width.clamp(1, StrokeSettings::MAX_WIDTH) as f32;
    let (band_lo, band_hi) = match s.location {
        StrokeLocation::Inside => (-width, 0.0),
        // An odd width on whole pixels: the odd one inside, so a straight
        // edge stays crisp.
        StrokeLocation::Center => {
            let inner = (width / 2.0).ceil();
            (-inner, width - inner)
        }
        StrokeLocation::Outside => (0.0, width),
    };
    let frame = PixelRect {
        x: origin.0,
        y: origin.1,
        w: image.width(),
        h: image.height(),
    };
    // What is outlined, with room around it for the distances.
    let inner = match selection {
        Some(sel) if sel.inverted => canvas,
        Some(sel) => sel.rect,
        None => frame,
    };
    let pad = width.ceil() as i64 + 2;
    let area = PixelRect {
        x: inner.x - pad,
        y: inner.y - pad,
        w: inner.w + 2 * pad as u32,
        h: inner.h + 2 * pad as u32,
    };
    let (w, h) = (area.w as usize, area.h as usize);
    let inside: Vec<bool> = match selection {
        Some(sel) => sel.to_mask(area).as_raw().iter().map(|&v| v >= 128).collect(),
        None => (0..w * h)
            .map(|i| {
                let (x, y) = (area.x + (i % w) as i64, area.y + (i / w) as i64);
                frame.contains(x, y)
                    && image
                        .get_pixel((x - frame.x) as u32, (y - frame.y) as u32)
                        .0[3]
                        >= 128
            })
            .collect(),
    };
    let outside: Vec<bool> = inside.iter().map(|v| !v).collect();
    let to_inside = distance_field(&inside, w, h);
    let to_outside = distance_field(&outside, w, h);
    // Coverage of the band at `at` in `area`.
    let band_at = |at: usize| {
        // Signed distance to the edge, between the pixels.
        let distance = if inside[at] {
            0.5 - to_outside[at]
        } else {
            to_inside[at] - 0.5
        };
        (0.5 + (distance - band_lo).min(band_hi - distance)).clamp(0.0, 1.0)
    };
    let mut covered = GrayImage::new(area.w, area.h);
    par_rows(covered.as_mut(), w, |first, rows| {
        for (i, v) in rows.iter_mut().enumerate() {
            *v = (band_at(first * w + i) * 255.0).round() as u8;
        }
    });
    let (s, row) = (*s, frame.w as usize * 4);
    let color = s.color.map(|c| c as f32 / 255.0);
    let opacity = s.opacity.clamp(0.0, 1.0);
    par_rows(out.as_mut(), row, |first, band| {
        for (i, p) in band.chunks_exact_mut(4).enumerate() {
            let (x, y) = (
                frame.x + (i % frame.w as usize) as i64,
                frame.y + (first + i / frame.w as usize) as i64,
            );
            if !area.contains(x, y) {
                continue;
            }
            let at = (y - area.y) as usize * w + (x - area.x) as usize;
            let coverage = band_at(at);
            if coverage <= 0.0 {
                continue;
            }
            let mut a = coverage * opacity;
            let mut mode = s.mode;
            if mode == BlendMode::Dissolve {
                a = blend::dissolve(a, x, y);
                mode = BlendMode::Normal;
            }
            let da = p[3] as f32 / 255.0;
            let c = |i: usize| p[i] as f32 / 255.0 * da;
            let mut px = [c(0), c(1), c(2), da];
            if s.preserve_transparency {
                blend::atop(&mut px, color, a, mode);
            } else {
                blend::over(&mut px, color, a, mode);
            }
            if px[3] <= 0.0 {
                continue;
            }
            let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
            let out = [
                byte(px[0] / px[3]),
                byte(px[1] / px[3]),
                byte(px[2] / px[3]),
                byte(px[3]),
            ];
            p.copy_from_slice(&out);
        }
    });
    (out, covered, (area.x, area.y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn one(rgb: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(1, 1, Rgba([rgb[0], rgb[1], rgb[2], 200]))
    }

    #[test]
    fn exposure_works_in_linear_light() {
        let mut i = one([100, 50, 0]);
        exposure(&mut i, &Exposure::default());
        assert_eq!(i.get_pixel(0, 0).0, [100, 50, 0, 200], "neutral");
        let mut i = one([128, 128, 128]);
        exposure(
            &mut i,
            &Exposure {
                exposure: 1.0,
                ..Default::default()
            },
        );
        // One stop: linear light doubles (not the sRGB value), alpha stays.
        let p = i.get_pixel(0, 0).0;
        assert!((p[0] as i32 - 175).abs() <= 1 && p[3] == 200, "{p:?}");
        let mut i = one([128, 128, 128]);
        exposure(
            &mut i,
            &Exposure {
                gamma: 2.0,
                ..Default::default()
            },
        );
        assert!(i.get_pixel(0, 0).0[0] > 170, "gamma above 1 lightens");
        let mut i = one([255, 255, 255]);
        exposure(
            &mut i,
            &Exposure {
                offset: -0.5,
                ..Default::default()
            },
        );
        assert!((i.get_pixel(0, 0).0[0] as i32 - 188).abs() <= 1);
    }

    #[test]
    fn channel_mixer_mixes_and_defaults_change_nothing() {
        let mut i = one([100, 50, 200]);
        channel_mixer(&mut i, &ChannelMixer::default());
        assert_eq!(i.get_pixel(0, 0).0, [100, 50, 200, 200]);
        channel_mixer(
            &mut i,
            &ChannelMixer {
                rows: [
                    [0.0, 1.0, 0.0, 0.0],
                    [0.5, 0.0, 0.0, 0.1],
                    [2.0, 0.0, 0.0, 0.0],
                ],
                monochrome: false,
            },
        );
        // Red gets green; green half of red plus 10%; blue twice red.
        assert_eq!(i.get_pixel(0, 0).0, [50, 76, 200, 200]);
        let mut i = one([100, 100, 100]);
        channel_mixer(
            &mut i,
            &ChannelMixer {
                rows: [ChannelMixer::GRAY; 3],
                monochrome: true,
            },
        );
        assert_eq!(i.get_pixel(0, 0).0, [100, 100, 100, 200]);
    }

    #[test]
    fn selective_color_acts_on_its_range_only() {
        let mut reds = SelectiveColor::default();
        reds.ranges[0] = [1.0, 0.0, 0.0, 0.0];
        // A green pixel isn't touched by a correction of the reds, and grays
        // neither (their "red" scale is zero).
        for rgb in [[0, 200, 0], [128, 128, 128]] {
            let mut i = one(rgb);
            selective_color(&mut i, &reds);
            assert_eq!(i.get_pixel(0, 0).0, [rgb[0], rgb[1], rgb[2], 200]);
        }
        // More cyan in a red removes red, in proportion to how red it is.
        let mut i = one([200, 100, 100]);
        selective_color(&mut i, &reds);
        let p = i.get_pixel(0, 0).0;
        assert!(p[0] < 200 && p[1] == 100 && p[2] == 100, "{p:?}");
        let relative = p[0];
        let mut i = one([200, 100, 100]);
        reds.absolute = true;
        selective_color(&mut i, &reds);
        assert!(i.get_pixel(0, 0).0[0] < relative, "absolute goes further");
        // More black in the blacks darkens them; whites keep out of it.
        let mut blacks = SelectiveColor::default();
        blacks.ranges[8] = [0.0, 0.0, 0.0, 1.0];
        let mut i = one([50, 50, 50]);
        selective_color(&mut i, &blacks);
        assert!(i.get_pixel(0, 0).0[0] < 50);
        let mut i = one([250, 250, 250]);
        selective_color(&mut i, &blacks);
        assert_eq!(i.get_pixel(0, 0).0[0], 250);
    }

    /// A horizontal ramp with a cast: red lifted, blue lowered.
    fn cast_ramp() -> RgbaImage {
        RgbaImage::from_fn(256, 8, |x, _| {
            let v = 40 + x * 160 / 255;
            Rgba([(v + 30) as u8, v as u8, (v - 20) as u8, 255])
        })
    }

    #[test]
    fn auto_tone_stretches_each_channel_and_auto_contrast_all_together() {
        let tone = auto_levels(&cast_ramp(), AutoMode::Tone);
        let mut i = cast_ramp();
        super::super::levels(&mut i, &tone);
        // Every channel reaches both ends: the cast at the ends is gone.
        assert_eq!(i.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(i.get_pixel(255, 0).0, [255, 255, 255, 255]);

        let contrast = auto_levels(&cast_ramp(), AutoMode::Contrast);
        let mut i = cast_ramp();
        super::super::levels(&mut i, &contrast);
        let (dark, light) = (i.get_pixel(0, 0).0, i.get_pixel(255, 0).0);
        // Only the extreme channels reach the ends: the cast stays.
        assert_eq!((dark[2], light[0]), (0, 255));
        assert!(dark[0] > 30 && light[2] < 225, "{dark:?} {light:?}");
    }

    #[test]
    fn auto_color_removes_a_cast_and_leaves_flat_images() {
        let levels = auto_levels(&cast_ramp(), AutoMode::Color);
        let mut i = cast_ramp();
        super::super::levels(&mut i, &levels);
        let mid = i.get_pixel(128, 0).0;
        let spread = mid[0].max(mid[1]).max(mid[2]) - mid[0].min(mid[1]).min(mid[2]);
        assert!(spread <= 8, "neutral midtones: {mid:?}");
        assert_eq!(i.get_pixel(255, 0).0, [255, 255, 255, 255]);
        let flat = RgbaImage::from_pixel(4, 4, Rgba([90, 60, 30, 255]));
        assert_eq!(auto_levels(&flat, AutoMode::Color), Levels::default());
    }

    fn fade(before: [u8; 4], after: [u8; 4], opacity: f32, mode: BlendMode) -> [u8; 4] {
        let f = Fade {
            before: Arc::new(RgbaImage::from_pixel(1, 1, Rgba(before))),
            area: FadeArea::All,
            origin: (0, 0),
            opacity,
            mode,
        };
        let mut i = RgbaImage::from_pixel(1, 1, Rgba(after));
        f.apply(&mut i);
        i.get_pixel(0, 0).0
    }

    #[test]
    fn fade_goes_from_before_to_after() {
        let (b, a) = ([200, 100, 0, 255], [0, 100, 200, 255]);
        assert_eq!(fade(b, a, 1.0, BlendMode::Normal), a);
        assert_eq!(fade(b, a, 0.0, BlendMode::Normal), b);
        assert_eq!(fade(b, a, 0.5, BlendMode::Normal), [100, 100, 100, 255]);
        // Multiply: result times what was there.
        assert_eq!(fade(b, a, 1.0, BlendMode::Multiply), [0, 39, 0, 255]);
        // A lower alpha (a clear, an eraser) comes back too.
        assert_eq!(fade([10, 20, 30, 255], [10, 20, 30, 0], 1.0, BlendMode::Normal)[3], 0);
        assert_eq!(fade([10, 20, 30, 255], [10, 20, 30, 0], 0.5, BlendMode::Normal)[3], 128);
        // On transparency, no backdrop for the mode.
        assert_eq!(fade([0; 4], a, 1.0, BlendMode::Multiply), a);
        // What the step covered is blended even where it changed nothing.
        assert_eq!(fade(b, b, 1.0, BlendMode::Multiply), [157, 39, 0, 255]);
        assert_eq!(fade(b, b, 1.0, BlendMode::Normal), b);
    }

    fn settings(width: u32, location: StrokeLocation) -> StrokeSettings {
        StrokeSettings {
            width,
            color: [255, 0, 0],
            location,
            mode: BlendMode::Normal,
            opacity: 1.0,
            preserve_transparency: false,
        }
    }

    #[test]
    fn fade_blends_its_whole_area_without_seams() {
        // Neighbours one level apart stay close, whichever the step changed.
        let before = RgbaImage::from_fn(2, 1, |x, _| Rgba([200 + x as u8, 200, 200, 255]));
        let mut after = before.clone();
        after.get_pixel_mut(1, 0).0[0] = 202;
        let mut f = Fade {
            before: Arc::new(before),
            area: FadeArea::All,
            origin: (0, 0),
            opacity: 1.0,
            mode: BlendMode::Multiply,
        };
        let mut all = after.clone();
        f.apply(&mut all);
        let (a, b) = (all.get_pixel(0, 0).0[0], all.get_pixel(1, 0).0[0]);
        assert!(a.abs_diff(b) <= 2, "{a} {b}");
        assert_eq!(a, 157);
        // Outside the selection nothing moves; half selected, half way.
        f.area = FadeArea::Within(Clip {
            rect: [0, 0, 2, 1],
            shape: Some(super::super::ClipShape::Mask(
                Arc::new(image::GrayImage::from_fn(2, 1, |x, _| image::Luma([x as u8 * 128]))),
                (0, 0),
            )),
            inverted: false,
        });
        let mut within = after.clone();
        f.apply(&mut within);
        assert_eq!(within.get_pixel(0, 0).0[0], 200);
        let half = within.get_pixel(1, 0).0[0];
        assert!((175..=185).contains(&half), "{half}");
        // A painting step: only what it painted.
        f.area = FadeArea::Changed;
        let mut changed = after.clone();
        f.apply(&mut changed);
        assert_eq!(changed.get_pixel(0, 0).0[0], 200);
        assert!(changed.get_pixel(1, 0).0[0] < 170);
    }

    #[test]
    fn center_strokes_stay_crisp_at_odd_widths() {
        let canvas = PixelRect { x: 0, y: 0, w: 20, h: 20 };
        let base = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let sel: Selection = PixelRect { x: 5, y: 5, w: 10, h: 10 }.into();
        for (width, red) in [(1u32, 5..6), (3, 4..7), (5, 3..8), (4, 3..7)] {
            let i = stroke(&base, (0, 0), Some(&sel), canvas, &settings(width, StrokeLocation::Center));
            for x in 0..10 {
                let want = if red.contains(&x) { 0 } else { 255 };
                assert_eq!(i.get_pixel(x, 10).0[1], want, "width {width}, x {x}");
            }
        }
    }

    #[test]
    fn auto_measures_only_what_is_selected() {
        // Mid grays selected; pure black and white in the unselected heart.
        let image = RgbaImage::from_fn(40, 1, |x, _| match x {
            18 => Rgba([0, 0, 0, 255]),
            19 => Rgba([255, 255, 255, 255]),
            _ => Rgba([100 + x as u8, 100 + x as u8, 100 + x as u8, 255]),
        });
        let mask = image::GrayImage::from_fn(40, 1, |x, _| {
            image::Luma([if x == 18 || x == 19 { 0 } else { 255 }])
        });
        let clip = Clip {
            rect: [0, 0, 40, 1],
            shape: Some(super::super::ClipShape::Mask(Arc::new(mask), (0, 0))),
            inverted: false,
        };
        for mode in [AutoMode::Tone, AutoMode::Contrast, AutoMode::Color] {
            let levels = auto_levels_within(&image, Some(&clip), mode);
            assert_ne!(levels, auto_levels(&image, mode), "{mode:?}");
            let mut i = image.clone();
            super::super::levels(&mut i, &levels);
            assert!(i.get_pixel(0, 0).0[0] < 10, "{mode:?}: {:?}", i.get_pixel(0, 0));
            assert!(i.get_pixel(39, 0).0[0] > 245, "{mode:?}: {:?}", i.get_pixel(39, 0));
        }
    }

    #[test]
    fn stroke_lies_where_asked() {
        let canvas = PixelRect { x: 0, y: 0, w: 20, h: 20 };
        let base = RgbaImage::from_pixel(20, 20, Rgba([255, 255, 255, 255]));
        let sel: Selection = PixelRect { x: 5, y: 5, w: 10, h: 10 }.into();
        let red = |i: &RgbaImage, x, y| i.get_pixel(x, y).0 == [255, 0, 0, 255];
        let white = |i: &RgbaImage, x, y| i.get_pixel(x, y).0 == [255, 255, 255, 255];

        let i = stroke(&base, (0, 0), Some(&sel), canvas, &settings(2, StrokeLocation::Inside));
        assert!(red(&i, 5, 10) && red(&i, 6, 10) && white(&i, 7, 10) && white(&i, 4, 10));
        let i = stroke(&base, (0, 0), Some(&sel), canvas, &settings(2, StrokeLocation::Outside));
        assert!(red(&i, 4, 10) && red(&i, 3, 10) && white(&i, 2, 10) && white(&i, 5, 10));
        let i = stroke(&base, (0, 0), Some(&sel), canvas, &settings(2, StrokeLocation::Center));
        assert!(red(&i, 4, 10) && red(&i, 5, 10) && white(&i, 3, 10) && white(&i, 6, 10));
        assert!(white(&i, 10, 10), "the middle stays");

        // Opacity, a mode.
        let half = StrokeSettings {
            opacity: 0.5,
            ..settings(2, StrokeLocation::Inside)
        };
        let i = stroke(&base, (0, 0), Some(&sel), canvas, &half);
        assert_eq!(i.get_pixel(5, 10).0, [255, 128, 128, 255]);
        let multiply = StrokeSettings {
            mode: BlendMode::Multiply,
            ..settings(2, StrokeLocation::Inside)
        };
        let i = stroke(&base, (0, 0), Some(&sel), canvas, &multiply);
        assert_eq!(i.get_pixel(5, 10).0, [255, 0, 0, 255]);

        // The whole canvas selected: its edge is stroked; inverted, the
        // canvas's edge isn't an edge.
        let all: Selection = canvas.into();
        let i = stroke(&base, (0, 0), Some(&all), canvas, &settings(1, StrokeLocation::Inside));
        assert!(red(&i, 0, 7) && white(&i, 1, 7));
        let inverted = Selection {
            inverted: true,
            ..sel.clone()
        };
        let i = stroke(&base, (0, 0), Some(&inverted), canvas, &settings(1, StrokeLocation::Inside));
        assert!(white(&i, 0, 7) && red(&i, 4, 10) && white(&i, 5, 10));

        // Preserve Transparency: transparent pixels stay so.
        let clear = RgbaImage::new(20, 20);
        let keep = StrokeSettings {
            preserve_transparency: true,
            ..settings(2, StrokeLocation::Inside)
        };
        assert_eq!(stroke(&clear, (0, 0), Some(&sel), canvas, &keep).get_pixel(5, 10).0[3], 0);
        assert_eq!(
            stroke(&clear, (0, 0), Some(&sel), canvas, &settings(2, StrokeLocation::Inside))
                .get_pixel(5, 10)
                .0,
            [255, 0, 0, 255]
        );
    }

    #[test]
    fn stroke_without_a_selection_outlines_the_visible_pixels() {
        let canvas = PixelRect { x: 0, y: 0, w: 20, h: 20 };
        let mut layer = RgbaImage::new(20, 20);
        for y in 8..12 {
            for x in 8..12 {
                layer.put_pixel(x, y, Rgba([0, 0, 255, 255]));
            }
        }
        let i = stroke(&layer, (0, 0), None, canvas, &settings(1, StrokeLocation::Outside));
        assert_eq!(i.get_pixel(7, 9).0, [255, 0, 0, 255]);
        assert_eq!(i.get_pixel(8, 9).0, [0, 0, 255, 255]);
        assert_eq!(i.get_pixel(6, 9).0[3], 0);
    }
}
