//! Adjustments that remap the tones: Black & White, Photo Filter, Invert,
//! Posterize, Threshold, Gradient Map (Photoshop's algorithms or close to
//! them).

use image::RgbaImage;

use super::{hsl_to_rgb, luminance, par_pixels, with_luminance};
use crate::paint::gradient::Gradient;

/// Black & White: how bright each color range becomes (percent, -200..300),
/// in Photoshop's order (reds, yellows, greens, cyans, blues, magentas),
/// and an optional tint (hue in degrees, saturation 0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlackWhite {
    pub weights: [f32; 6],
    pub tint: bool,
    pub hue: f32,
    pub saturation: f32,
}

impl Default for BlackWhite {
    /// Photoshop's default mix.
    fn default() -> Self {
        Self {
            weights: [40.0, 60.0, 40.0, 60.0, 20.0, 80.0],
            tint: false,
            hue: 35.0,
            saturation: 0.25,
        }
    }
}

/// Black & White: a pixel is its smallest channel, plus the share of its
/// secondary color (yellow, cyan or magenta) and of its primary color (red,
/// green or blue), each weighted by its slider.
pub fn black_white(image: &mut RgbaImage, bw: &BlackWhite) {
    let w = bw.weights.map(|v| v / 100.0);
    let tint = bw
        .tint
        .then(|| hsl_to_rgb(bw.hue, bw.saturation.clamp(0.0, 1.0), 0.5));
    par_pixels(image, |p| {
        let c = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        // Channels from the largest down.
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| c[b].total_cmp(&c[a]));
        let [hi, mid, lo] = order;
        // Primary: red 0, green 2, blue 4; secondary between two of them.
        let primary = [0, 2, 4][hi];
        let secondary = match (hi.min(mid), hi.max(mid)) {
            (0, 1) => 1, // yellow
            (1, 2) => 3, // cyan
            _ => 5,      // magenta
        };
        let gray = c[lo] + (c[mid] - c[lo]) * w[secondary] + (c[hi] - c[mid]) * w[primary];
        let gray = gray.clamp(0.0, 1.0);
        let out = match tint {
            Some(t) => with_luminance(t, gray),
            None => [gray; 3],
        };
        for (ch, v) in p.iter_mut().zip(out) {
            *ch = (v * 255.0).round() as u8;
        }
    });
}

/// Photo Filter: the colors seen through a colored filter of `density`
/// (0..1), keeping their luminosity if asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhotoFilter {
    pub color: [u8; 3],
    pub density: f32,
    pub preserve_luminosity: bool,
}

impl Default for PhotoFilter {
    /// Photoshop's Warming Filter (85), 25 %.
    fn default() -> Self {
        Self {
            color: [236, 138, 0],
            density: 0.25,
            preserve_luminosity: true,
        }
    }
}

pub fn photo_filter(image: &mut RgbaImage, f: &PhotoFilter) {
    let filter = f.color.map(|v| v as f32 / 255.0);
    let d = f.density.clamp(0.0, 1.0);
    par_pixels(image, |p| {
        let c = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let mut out: [f32; 3] = std::array::from_fn(|i| c[i] + (c[i] * filter[i] - c[i]) * d);
        if f.preserve_luminosity {
            out = with_luminance(out, luminance(c));
        }
        for (ch, v) in p.iter_mut().zip(out) {
            *ch = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    });
}

pub fn invert(image: &mut RgbaImage) {
    par_pixels(image, |p| {
        for c in &mut p[..3] {
            *c = 255 - *c;
        }
    });
}

/// Posterize: number of tones per channel (2..255).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Posterize {
    pub levels: u8,
}

impl Default for Posterize {
    fn default() -> Self {
        Self { levels: 4 }
    }
}

/// Photoshop's posterize: `levels` equal bands per channel, spread from
/// black to white.
pub fn posterize(image: &mut RgbaImage, levels: u8) {
    let n = levels.max(2) as u32;
    let lut: [u8; 256] =
        std::array::from_fn(|v| ((v as u32 * n / 256) * 255 / (n - 1)).min(255) as u8);
    par_pixels(image, |p| {
        for c in &mut p[..3] {
            *c = lut[*c as usize];
        }
    });
}

/// Threshold: pixels at least as bright as `level` become white, the others
/// black.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Threshold {
    pub level: u8,
}

impl Default for Threshold {
    fn default() -> Self {
        Self { level: 128 }
    }
}

pub fn threshold(image: &mut RgbaImage, level: u8) {
    par_pixels(image, |p| {
        let c = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let v = if (luminance(c) * 255.0).round() >= level as f32 {
            255
        } else {
            0
        };
        p[..3].fill(v);
    });
}

/// Gradient Map: each pixel's luminosity picks a color of the gradient
/// (fixed colors, see [`Gradient::resolved`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GradientMap {
    pub gradient: Gradient,
    pub reverse: bool,
}

pub fn gradient_map(image: &mut RgbaImage, g: &GradientMap) {
    let lut: Vec<[u8; 4]> = g
        .gradient
        .ramp(256)
        .iter()
        .map(|c| c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8))
        .collect();
    par_pixels(image, |p| {
        let c = [p[0], p[1], p[2]].map(|v| v as f32 / 255.0);
        let mut i = (luminance(c) * 255.0).round().clamp(0.0, 255.0) as usize;
        if g.reverse {
            i = 255 - i;
        }
        p[..3].copy_from_slice(&lut[i][..3]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paint::gradient::Preset;
    use image::Rgba;

    fn one(c: [u8; 3]) -> RgbaImage {
        RgbaImage::from_pixel(1, 1, Rgba([c[0], c[1], c[2], 255]))
    }

    #[test]
    fn black_white_weights_the_color_ranges() {
        let bw = BlackWhite::default();
        let gray = |c| {
            let mut img = one(c);
            black_white(&mut img, &bw);
            let p = img.get_pixel(0, 0).0;
            assert!(p[0] == p[1] && p[1] == p[2]);
            p[0]
        };
        assert_eq!(gray([255, 0, 0]), 102); // reds 40 %
        assert_eq!(gray([255, 255, 0]), 153); // yellows 60 %
        assert_eq!(gray([0, 0, 255]), 51); // blues 20 %
        assert_eq!(gray([255, 255, 255]), 255);
        assert_eq!(gray([90, 90, 90]), 90);
    }

    #[test]
    fn posterize_threshold_invert_and_map() {
        let mut img = RgbaImage::from_fn(256, 1, |x, _| Rgba([x as u8, x as u8, x as u8, 255]));
        posterize(&mut img, 4);
        let tones: std::collections::BTreeSet<u8> = img.pixels().map(|p| p.0[0]).collect();
        assert_eq!(tones.into_iter().collect::<Vec<_>>(), [0, 85, 170, 255]);
        let mut img = one([200, 200, 200]);
        threshold(&mut img, 128);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 255]);
        invert(&mut img);
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 255]);
        let map = GradientMap {
            gradient: Preset::ForegroundToBackground
                .gradient()
                .resolved([255, 0, 0], [0, 0, 255]),
            reverse: false,
        };
        gradient_map(&mut img, &map);
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        let mut warm = one([100, 100, 100]);
        photo_filter(&mut warm, &PhotoFilter::default());
        let p = warm.get_pixel(0, 0).0;
        assert!(p[0] > p[2], "{p:?}");
    }
}
