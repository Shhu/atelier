//! The development, in Camera Raw's order: white balance and exposure,
//! lens corrections and dehaze in linear light; local tone (highlights,
//! shadows, clarity, texture) on the luminance's edge-preserving base layer
//! and details; to the display (a film-like shoulder for RAW files);
//! noise reduction, tone curves, color, sharpening, effects.

use std::sync::atomic::{AtomicU32, Ordering};

use image::RgbaImage;

use super::planes::{Plane, guided};
use super::settings::{Profile, Settings, ToneCurve, VignetteStyle, Wheel};
use super::source::Source;
use super::white;
use crate::filters::par_rows;
use crate::pixel_ops::Curve;

/// Where the pixels developed sit in the image, so that a part (or a
/// reduction) develops like the whole.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Size of the whole image at this scale.
    pub size: (u32, u32),
    /// Position of the developed part in it.
    pub origin: (u32, u32),
    /// Pixels at this scale per full-resolution pixel.
    pub scale: f32,
}

impl Frame {
    pub fn whole(source: &Source) -> Frame {
        Frame {
            size: (source.width, source.height),
            origin: (0, 0),
            scale: 1.0,
        }
    }
}

/// Measurements of the whole image the development depends on.
#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    /// Color of the haze (source values): of the haziest pixels.
    pub airlight: [f32; 3],
    /// White balance of the Auto preset.
    pub auto_balance: (f32, f32),
}

impl Analysis {
    pub fn of(source: &Source) -> Analysis {
        let long = source.width.max(source.height).max(1) as f32;
        let small = source.downscaled((256.0 / long).min(1.0));
        let mut dark: Vec<(f32, usize)> = small
            .rgb
            .iter()
            .enumerate()
            .map(|(i, p)| (p[0].min(p[1]).min(p[2]), i))
            .collect();
        dark.sort_by(|a, b| b.0.total_cmp(&a.0));
        let top = &dark[..(dark.len() / 1000).max(1)];
        let mut airlight = [0.0f32; 3];
        for &(_, i) in top {
            for (a, v) in airlight.iter_mut().zip(small.rgb[i]) {
                *a += v / top.len() as f32;
            }
        }
        // Gray world, without the clipped and the darkest pixels.
        let (mut sum, mut n) = ([0.0f64; 3], 0usize);
        for p in &small.rgb {
            let max = p[0].max(p[1]).max(p[2]);
            if max > 0.01 && max < 0.95 {
                for c in 0..3 {
                    sum[c] += p[c] as f64;
                }
                n += 1;
            }
        }
        let mean = if n == 0 {
            [1.0; 3]
        } else {
            sum.map(|v| (v / n as f64) as f32)
        };
        Analysis {
            airlight: airlight.map(|v| v.max(1e-3)),
            auto_balance: source.neutral_balance(mean),
        }
    }
}

/// What the preview shows instead of the image while Alt is held on a
/// sharpening slider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Visual {
    #[default]
    Image,
    SharpenAmount,
    SharpenRadius,
    SharpenDetail,
    SharpenMasking,
}

/// `settings` applied to 8-bit pixels (Filter › Camera Raw Filter, a Smart
/// Filter): transparency kept.
pub fn apply(image: &RgbaImage, settings: &Settings) -> RgbaImage {
    let source = Source::from_rgba(image);
    let analysis = Analysis::of(&source);
    let frame = Frame::whole(&source);
    render(&source, settings, &frame, &analysis, Visual::Image, None)
}

/// Develops `source`, the part of the image `frame` says, in 8 bits.
/// `progress` gets the work done, in thousandths.
pub fn render(
    source: &Source,
    settings: &Settings,
    frame: &Frame,
    analysis: &Analysis,
    visual: Visual,
    progress: Option<&AtomicU32>,
) -> RgbaImage {
    let report = |p: u32| {
        if let Some(progress) = progress {
            progress.store(p, Ordering::Relaxed);
        }
    };
    // A stage done: true when the development was cancelled (its result
    // is then dropped).
    let stage = |p: u32| {
        report(p);
        crate::filters::cancel_requested()
    };
    let s = settings;
    let (w, h) = (source.width as usize, source.height as usize);
    let long = frame.size.0.max(frame.size.1).max(1) as f32;
    let raw = source.is_raw();

    // White balance and exposure.
    let baseline = source.camera.as_ref().map_or(0.0, |c| c.baseline_exposure);
    let gain = 2f64.powf((s.exposure + baseline) as f64);
    let matrix = white::scale(&source.working_matrix(s), gain);
    let mut rgb = if matrix == white::IDENTITY {
        source.rgb.clone()
    } else {
        let m = matrix.map(|row| row.map(|v| v as f32));
        let mut out = source.rgb.clone();
        par_rows(&mut out, w.max(1), |_, band| {
            for p in band {
                *p = mat_apply(&m, *p);
            }
        });
        out
    };
    if stage(100) {
        return RgbaImage::new(source.width, source.height);
    }

    // A distortion moves the opacity with the colors.
    let mut alpha = source.alpha.as_deref().map(std::borrow::Cow::Borrowed);
    if s.lens.distortion != 0.0 || s.lens.vignetting != 0.0 {
        rgb = lens(&rgb, w, h, frame, s);
        if s.lens.distortion != 0.0
            && let Some(a) = &source.alpha
        {
            alpha = Some(std::borrow::Cow::Owned(lens_alpha(a, w, h, frame, s)));
        }
    }
    if s.dehaze != 0.0 {
        let m = matrix.map(|row| row.map(|v| v as f32));
        dehaze(
            &mut rgb,
            w,
            h,
            mat_apply(&m, analysis.airlight),
            s.dehaze / 100.0,
            long,
        );
    }
    if stage(200) {
        return RgbaImage::new(source.width, source.height);
    }
    if s.highlights != 0.0 || s.shadows != 0.0 || s.clarity != 0.0 || s.texture != 0.0 {
        local_tone(&mut rgb, w, h, s, long);
    }
    if stage(450) {
        return RgbaImage::new(source.width, source.height);
    }

    // To the display.
    par_rows(&mut rgb, w.max(1), |_, band| {
        for p in band {
            *p = p.map(|v| {
                let v = if raw { shoulder(v) } else { v.clamp(0.0, 1.0) };
                encode(v)
            });
        }
    });
    if stage(550) {
        return RgbaImage::new(source.width, source.height);
    }

    noise_reduction(&mut rgb, w, h, s, frame.scale);
    if stage(650) {
        return RgbaImage::new(source.width, source.height);
    }
    if let Some(luts) = tone_luts(s, raw) {
        par_rows(&mut rgb, w.max(1), |_, band| {
            for p in band {
                for c in 0..3 {
                    p[c] = lookup(&luts[c], p[c]);
                }
            }
        });
    }
    let color = ColorOps::new(s);
    if !color.neutral {
        par_rows(&mut rgb, w.max(1), |_, band| {
            for p in band {
                *p = color.apply(*p);
            }
        });
    }
    if stage(750) {
        return RgbaImage::new(source.width, source.height);
    }

    let shown = sharpen(&mut rgb, w, h, s, frame.scale, visual);
    if stage(900) {
        return RgbaImage::new(source.width, source.height);
    }
    if visual == Visual::Image {
        effects(&mut rgb, w, s, frame);
    }

    let mut out = RgbaImage::new(source.width, source.height);
    let raw_out: &mut [u8] = &mut out;
    par_rows(raw_out, w.max(1) * 4, |y0, band| {
        for (i, px) in band.chunks_exact_mut(4).enumerate() {
            let k = y0 * w + i;
            let p = match &shown {
                Some(gray) => [gray.data[k]; 3],
                None => rgb[k],
            };
            for c in 0..3 {
                px[c] = (p[c].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            px[3] = alpha.as_ref().map_or(255, |a| a[k]);
        }
    });
    report(1000);
    out
}

fn mat_apply(m: &[[f32; 3]; 3], p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2])
}

/// sRGB transfer function.
fn encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// RAW files: the light above the knee rolls off toward white instead of
/// clipping.
fn shoulder(v: f32) -> f32 {
    const KNEE: f32 = 0.6;
    if v <= KNEE {
        v.max(0.0)
    } else {
        KNEE + (1.0 - KNEE) * (1.0 - (-(v - KNEE) / (1.0 - KNEE)).exp())
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rec. 709 luminance.
fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// Where the distortion of `s` takes the pixel at `x`, `y` of the frame's
/// part from, and its squared distance to the center (1 at the corners).
fn lens_source(frame: &Frame, s: &Settings) -> impl Fn(f32, f32) -> (f32, f32, f32) + Sync {
    let (fw, fh) = (frame.size.0 as f32, frame.size.1 as f32);
    let (cx, cy) = (fw / 2.0, fh / 2.0);
    let half_diagonal = (cx * cx + cy * cy).sqrt().max(1.0);
    let k = s.lens.distortion / 100.0 * 0.15;
    let (ox, oy) = (frame.origin.0 as f32, frame.origin.1 as f32);
    move |x, y| {
        let (dx, dy) = (ox + x + 0.5 - cx, oy + y + 0.5 - cy);
        let r2 = (dx * dx + dy * dy) / (half_diagonal * half_diagonal);
        let f = 1.0 - k * r2;
        (cx + dx * f - ox - 0.5, cy + dy * f - oy - 0.5, r2)
    }
}

/// The opacity distorted as [`lens`] distorts the colors; nothing comes
/// from beyond the image.
fn lens_alpha(alpha: &[u8], w: usize, h: usize, frame: &Frame, s: &Settings) -> Vec<u8> {
    let source = lens_source(frame, s);
    let mut out = vec![0u8; w * h];
    par_rows(&mut out, w.max(1), |y0, band| {
        for (i, a) in band.iter_mut().enumerate() {
            let (sx, sy, _) = source((i % w) as f32, (y0 + i / w) as f32);
            let (left, up) = (sx.floor(), sy.floor());
            let (tx, ty) = (sx - left, sy - up);
            let at = |x: f32, y: f32| {
                if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                    0.0
                } else {
                    alpha[y as usize * w + x as usize] as f32
                }
            };
            let top = at(left, up) + (at(left + 1.0, up) - at(left, up)) * tx;
            let bottom = at(left, up + 1.0) + (at(left + 1.0, up + 1.0) - at(left, up + 1.0)) * tx;
            *a = (top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8;
        }
    });
    out
}

/// Manual lens corrections: distortion (resampling around the center of
/// the image) and vignetting.
fn lens(rgb: &[[f32; 3]], w: usize, h: usize, frame: &Frame, s: &Settings) -> Vec<[f32; 3]> {
    let k = s.lens.distortion;
    let v = s.lens.vignetting / 100.0;
    let power = 1.0 + s.lens.midpoint / 100.0 * 4.0;
    let source = lens_source(frame, s);
    let mut out = vec![[0.0f32; 3]; w * h];
    par_rows(&mut out, w.max(1), |y0, band| {
        for (i, p) in band.iter_mut().enumerate() {
            let (x, y) = ((i % w) as f32, (y0 + i / w) as f32);
            let (sx, sy, r2) = source(x, y);
            let mut q = if k == 0.0 {
                rgb[(y0 * w) + i]
            } else {
                bilinear(rgb, w, h, sx, sy)
            };
            if v != 0.0 {
                let gain = (1.0 + v * r2.sqrt().powf(power)).max(0.0);
                q = q.map(|c| c * gain);
            }
            *p = q;
        }
    });
    out
}

fn bilinear(rgb: &[[f32; 3]], w: usize, h: usize, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x as usize, y as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| rgb[y * w + x];
    std::array::from_fn(|c| {
        let top = at(x0, y0)[c] + (at(x1, y0)[c] - at(x0, y0)[c]) * tx;
        let bottom = at(x0, y1)[c] + (at(x1, y1)[c] - at(x0, y1)[c]) * tx;
        top + (bottom - top) * ty
    })
}

/// Dehaze (dark channel prior): the haze, estimated from the darkest
/// channel around each pixel, is removed (`amount` > 0) or added.
fn dehaze(rgb: &mut [[f32; 3]], w: usize, h: usize, airlight: [f32; 3], amount: f32, long: f32) {
    let a = airlight.map(|v| v.max(1e-3));
    if amount < 0.0 {
        let k = -amount * 0.45;
        par_rows(rgb, w.max(1), |_, band| {
            for p in band {
                *p = std::array::from_fn(|c| p[c] + (a[c] * 0.9 - p[c]) * k);
            }
        });
        return;
    }
    let dark = Plane::from_fn(w, h, |i| {
        let p = rgb[i];
        (p[0] / a[0]).min(p[1] / a[1]).min(p[2] / a[2]).max(0.0)
    });
    let guide = Plane::from_fn(w, h, |i| luma(rgb[i]));
    let r = (0.015 * long).max(2.0) as usize;
    let haze = guided(&guide, &dark.box_blur(r / 2), r, 1e-3, (r / 8).max(1));
    par_rows(rgb, w.max(1), |y0, band| {
        for (i, p) in band.iter_mut().enumerate() {
            let t = (1.0 - 0.95 * amount * haze.data[y0 * w + i]).max(0.15);
            *p = std::array::from_fn(|c| ((p[c] - a[c]) / t + a[c]).max(0.0));
        }
    });
}

/// Highlights, Shadows, Clarity, Texture: on the log luminance, split in
/// an edge-preserving base layer (guided filter) and its details. The
/// tones of the base change, the details stay (or are boosted): local
/// contrast survives a strong recovery, unlike with a global curve.
fn local_tone(rgb: &mut [[f32; 3]], w: usize, h: usize, s: &Settings, long: f32) {
    let log = Plane::from_fn(w, h, |i| luma(rgb[i]).max(1e-5).log2());
    let radius = |fraction: f32| (fraction * long).max(2.0) as usize;
    let rb = radius(0.03);
    let base = guided(&log, &log, rb, 0.25, (rb / 8).max(1));
    let rm = radius(0.008);
    let mid = (s.clarity != 0.0).then(|| guided(&log, &log, rm, 0.15, (rm / 4).max(1)));
    let fine = (s.texture != 0.0).then(|| log.gaussian((0.0012 * long).max(0.6)));
    let (hl, sh) = (s.highlights / 100.0, s.shadows / 100.0);
    let (clarity, texture) = (s.clarity / 100.0, s.texture / 100.0);
    par_rows(rgb, w.max(1), |y0, band| {
        for (i, p) in band.iter_mut().enumerate() {
            let k = y0 * w + i;
            let (l, b) = (log.data[k], base.data[k]);
            let bright = smoothstep(-3.5, 0.0, b);
            let dark = 1.0 - smoothstep(-8.0, -2.0, b);
            let over = if hl < 0.0 { 0.6 * b.max(0.0) } else { 0.0 };
            let mut dl = hl * (bright + over) + sh * 1.5 * dark;
            if let Some(mid) = &mid {
                let midtones = (-((b + 2.5) / 2.5).powi(2)).exp();
                dl += clarity * 0.8 * (l - mid.data[k]) * midtones;
            }
            if let Some(fine) = &fine {
                dl += texture * 0.9 * (l - fine.data[k]);
            }
            let ratio = dl.exp2();
            *p = p.map(|v| v * ratio);
        }
    });
}

/// Luminance noise reduction (guided filter), then color noise reduction
/// (blurred chroma). Their radii are full-resolution pixels: like Camera
/// Raw, they show from 100 %.
fn noise_reduction(rgb: &mut [[f32; 3]], w: usize, h: usize, s: &Settings, scale: f32) {
    let d = &s.detail;
    let amount = d.luminance / 100.0;
    let reach = (1.0 + 2.0 * amount) * scale;
    if amount > 0.0 && reach >= 0.5 {
        let y = Plane::from_fn(w, h, |i| luma(rgb[i]));
        let eps = (0.003 + 0.05 * amount).powi(2);
        let smooth = guided(&y, &y, reach.round().max(1.0) as usize, eps, 1);
        let keep = d.luminance_detail / 100.0 * 0.6;
        par_rows(rgb, w.max(1), |y0, band| {
            for (i, p) in band.iter_mut().enumerate() {
                let k = y0 * w + i;
                let target = smooth.data[k] + (y.data[k] - smooth.data[k]) * keep;
                let delta = target - y.data[k];
                *p = p.map(|v| v + delta);
            }
        });
    }
    let amount = d.color / 100.0;
    let sigma = (0.5 + 4.0 * amount) * scale;
    if amount > 0.0 && sigma >= 0.3 {
        let keep = d.color_detail / 100.0 * 0.3;
        let y = Plane::from_fn(w, h, |i| luma(rgb[i]));
        let cr = Plane::from_fn(w, h, |i| rgb[i][0] - y.data[i]);
        let cb = Plane::from_fn(w, h, |i| rgb[i][2] - y.data[i]);
        let (cr_s, cb_s) = (cr.gaussian(sigma), cb.gaussian(sigma));
        par_rows(rgb, w.max(1), |y0, band| {
            for (i, p) in band.iter_mut().enumerate() {
                let k = y0 * w + i;
                let r = y.data[k] + cr_s.data[k] + (cr.data[k] - cr_s.data[k]) * keep;
                let b = y.data[k] + cb_s.data[k] + (cb.data[k] - cb_s.data[k]) * keep;
                let g = (y.data[k] - 0.2126 * r - 0.0722 * b) / 0.7152;
                *p = [r, g, b];
            }
        });
    }
}

/// Lookup tables of the tone (display values): Whites, Blacks, Contrast,
/// the parametric curve, the point curves. `None` when neutral.
fn tone_luts(s: &Settings, raw: bool) -> Option<[Vec<f32>; 3]> {
    const N: usize = 4096;
    let curve = &s.curve;
    let contrast = s.contrast / 100.0 + if raw { 0.2 } else { 0.0 };
    let parametric = [curve.shadows, curve.darks, curve.lights, curve.highlights];
    let points: Vec<Option<[f32; 256]>> = curve
        .points
        .iter()
        .map(|p| {
            Curve::from_points(p)
                .filter(|c| !c.is_identity())
                .map(|c| c.samples())
        })
        .collect();
    if s.whites == 0.0
        && s.blacks == 0.0
        && contrast == 0.0
        && parametric.iter().all(|&v| v == 0.0)
        && points.iter().all(Option::is_none)
    {
        return None;
    }
    let (whites, blacks) = (s.whites / 100.0, s.blacks / 100.0);
    let gamma = contrast.exp2();
    let master: Vec<f32> = (0..N)
        .map(|i| {
            let mut v = i as f32 / (N - 1) as f32;
            v += whites * 0.3 * v * v * v;
            v += blacks * 0.25 * (1.0 - v).powi(3);
            v = v.clamp(0.0, 1.0);
            if contrast != 0.0 {
                v = if v < 0.5 {
                    0.5 * (v / 0.5).powf(gamma)
                } else {
                    1.0 - 0.5 * ((1.0 - v) / 0.5).powf(gamma)
                };
            }
            v = parametric_curve(v, curve);
            if let Some(samples) = &points[0] {
                v = sample(samples, v);
            }
            v
        })
        .scan(0.0f32, |max, v| {
            *max = max.max(v);
            Some(*max)
        })
        .collect();
    Some(std::array::from_fn(|c| match &points[c + 1] {
        Some(samples) => master.iter().map(|&v| sample(samples, v)).collect(),
        None => master.clone(),
    }))
}

/// The parametric curve at `n` evenly spaced points from 0 to 1 (its
/// graph).
pub fn parametric_samples(curve: &ToneCurve, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| parametric_curve(i as f32 / (n - 1).max(1) as f32, curve))
        .collect()
}

/// The parametric curve: each region's slider raises or lowers it there.
fn parametric_curve(v: f32, c: &ToneCurve) -> f32 {
    let mut s = c.splits.map(|p| (p / 100.0).clamp(0.01, 0.99));
    s[1] = s[1].max(s[0] + 0.01);
    s[2] = s[2].max(s[1] + 0.01);
    let bump = |center: f32, radius: f32| {
        let d = (v - center) / radius.max(0.01);
        if d.abs() < 1.0 {
            0.5 * (1.0 + (std::f32::consts::PI * d).cos())
        } else {
            0.0
        }
    };
    let offset = c.shadows / 100.0 * bump(s[0] / 2.0, s[0])
        + c.darks / 100.0 * bump((s[0] + s[1]) / 2.0, s[1] - s[0])
        + c.lights / 100.0 * bump((s[1] + s[2]) / 2.0, s[2] - s[1])
        + c.highlights / 100.0 * bump((s[2] + 1.0) / 2.0, 1.0 - s[2]);
    let pin = (v / 0.02).min((1.0 - v) / 0.02).clamp(0.0, 1.0);
    (v + 0.25 * offset * pin).clamp(0.0, 1.0)
}

/// A point curve's samples (0 to 255) at `v` (0 to 1).
fn sample(samples: &[f32; 256], v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * 255.0;
    let i = (x as usize).min(254);
    let t = x - i as f32;
    ((samples[i] + (samples[i + 1] - samples[i]) * t) / 255.0).clamp(0.0, 1.0)
}

fn lookup(lut: &[f32], v: f32) -> f32 {
    let x = v.clamp(0.0, 1.0) * (lut.len() - 1) as f32;
    let i = (x as usize).min(lut.len() - 2);
    lut[i] + (lut[i + 1] - lut[i]) * (x - i as f32)
}

/// Centers of the Color Mixer's hue ranges, in degrees.
const HUE_CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];

/// Weights of the eight hue ranges for a hue.
fn hue_weights(hue: f32) -> [f32; 8] {
    let hue = hue.rem_euclid(360.0);
    let mut w = [0.0; 8];
    for i in 0..8 {
        let (a, b) = (
            HUE_CENTERS[i],
            if i == 7 { 360.0 } else { HUE_CENTERS[i + 1] },
        );
        if hue >= a && hue < b {
            let t = smoothstep(0.0, 1.0, (hue - a) / (b - a));
            w[i] = 1.0 - t;
            w[(i + 1) % 8] = t;
        }
    }
    w
}

fn rgb_to_hsv([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m]
}

/// Color: Color Mixer, Vibrance, Saturation, Monochrome, Color Grading.
struct ColorOps<'a> {
    s: &'a Settings,
    mixer: bool,
    monochrome: bool,
    grading: bool,
    neutral: bool,
}

impl<'a> ColorOps<'a> {
    fn new(s: &'a Settings) -> Self {
        let m = &s.mixer;
        let any = |v: &[f32; 8]| v.iter().any(|&x| x != 0.0);
        let mixer = any(&m.hue) || any(&m.saturation) || any(&m.luminance);
        let g = &s.grading;
        let grading = [g.shadows, g.midtones, g.highlights, g.global]
            .iter()
            .any(|w| w.saturation != 0.0 || w.luminance != 0.0);
        let monochrome = s.profile == Profile::Monochrome;
        let neutral = !mixer && !monochrome && !grading && s.vibrance == 0.0 && s.saturation == 0.0;
        Self {
            s,
            mixer,
            monochrome,
            grading,
            neutral,
        }
    }

    fn apply(&self, mut p: [f32; 3]) -> [f32; 3] {
        let s = self.s;
        let (mut hue, mut sat, value) = rgb_to_hsv(p);
        let weights = hue_weights(hue);
        let dot = |v: &[f32; 8]| (0..8).map(|i| weights[i] * v[i]).sum::<f32>() / 100.0;
        if !self.monochrome && (self.mixer || s.vibrance != 0.0 || s.saturation != 0.0) {
            let mut k = 1.0;
            if self.mixer && sat > 0.0 {
                let shift = dot(&s.mixer.hue) * 30.0;
                if shift != 0.0 {
                    hue += shift;
                    p = hsv_to_rgb(hue, sat, value);
                }
                k *= 1.0 + dot(&s.mixer.saturation);
                let lum = dot(&s.mixer.luminance);
                if lum != 0.0 {
                    let gain = (lum * 0.8 * sat).exp2();
                    p = p.map(|v| v * gain);
                }
            }
            let vibrance = s.vibrance / 100.0;
            if vibrance > 0.0 {
                // Skin tones (orange) are spared.
                let skin = 1.0 - 0.5 * weights[1];
                k *= 1.0 + vibrance * (1.0 - sat) * skin;
            } else if vibrance < 0.0 {
                k *= 1.0 + vibrance * (1.0 - 0.5 * sat);
            }
            k *= 1.0 + s.saturation / 100.0;
            let y = luma(p);
            p = p.map(|v| y + (v - y) * k.max(0.0));
            sat = rgb_to_hsv(p).1;
        }
        if self.monochrome {
            let g = luma(p) + dot(&s.mixer.gray) * sat * 0.5;
            p = [g; 3];
        }
        if self.grading {
            p = self.grade(p);
        }
        p.map(|v| v.clamp(0.0, 1.0))
    }

    /// Color Grading: each wheel tints (and lightens or darkens) its range
    /// of luminance.
    fn grade(&self, p: [f32; 3]) -> [f32; 3] {
        let g = &self.s.grading;
        let l = luma(p).clamp(0.0, 1.0);
        let center = 0.5 - 0.25 * g.balance / 100.0;
        let width = 0.1 + 0.5 * g.blending / 100.0;
        let shadows = 1.0 - smoothstep(center - width, center, l);
        let highlights = smoothstep(center, center + width, l);
        let midtones = (1.0 - shadows - highlights).max(0.0);
        let mut out = p;
        for (wheel, weight) in [
            (g.shadows, shadows),
            (g.midtones, midtones),
            (g.highlights, highlights),
            (g.global, 1.0),
        ] {
            let Wheel {
                hue,
                saturation,
                luminance,
            } = wheel;
            if weight <= 0.0 || (saturation == 0.0 && luminance == 0.0) {
                continue;
            }
            let tint = hsv_to_rgb(hue, 1.0, 1.0);
            let y = luma(tint);
            for c in 0..3 {
                out[c] +=
                    weight * ((tint[c] - y) * saturation / 100.0 * 0.3 + luminance / 100.0 * 0.25);
            }
        }
        out
    }
}

/// Sharpening of the luminance (unsharp mask: Radius, halos limited by
/// Detail, edges only with Masking). Returns the grayscale view asked by
/// `visual`.
fn sharpen(
    rgb: &mut [[f32; 3]],
    w: usize,
    h: usize,
    s: &Settings,
    scale: f32,
    visual: Visual,
) -> Option<Plane> {
    let d = &s.detail;
    if d.sharpen_amount <= 0.0 && visual == Visual::Image {
        return None;
    }
    let y = Plane::from_fn(w, h, |i| luma(rgb[i]));
    let sigma = d.sharpen_radius * scale;
    let blur = y.gaussian(sigma);
    let k = (1.0 - d.sharpen_detail / 100.0) * 30.0;
    let amount = d.sharpen_amount / 100.0 * 1.6;
    let mask = (d.sharpen_masking > 0.0).then(|| {
        let threshold = d.sharpen_masking / 100.0 * 0.08;
        let edges = y.gaussian(sigma.max(1.0));
        Plane::from_fn(w, h, |i| {
            let (x, yy) = (i % w, i / w);
            let at = |x: usize, y: usize| edges.data[y * w + x];
            let gx = at((x + 1).min(w - 1), yy) - at(x.saturating_sub(1), yy);
            let gy = at(x, (yy + 1).min(h - 1)) - at(x, yy.saturating_sub(1));
            let gradient = (gx * gx + gy * gy).sqrt() / 2.0 / scale.max(1e-3);
            smoothstep(threshold * 0.5, threshold * 1.5 + 1e-4, gradient)
        })
    });
    let detail = |i: usize| {
        let diff = y.data[i] - blur.data[i];
        diff / (1.0 + diff.abs() * k)
    };
    let weight = |i: usize| mask.as_ref().map_or(1.0, |m| m.data[i]);
    match visual {
        Visual::Image => {
            par_rows(rgb, w.max(1), |y0, band| {
                for (i, p) in band.iter_mut().enumerate() {
                    let k = y0 * w + i;
                    let delta = amount * detail(k) * weight(k);
                    *p = p.map(|v| v + delta);
                }
            });
            None
        }
        Visual::SharpenAmount => Some(Plane::from_fn(w, h, |i| {
            y.data[i] + amount * detail(i) * weight(i)
        })),
        Visual::SharpenRadius => Some(Plane::from_fn(w, h, |i| {
            0.5 + (y.data[i] - blur.data[i]) * 4.0
        })),
        Visual::SharpenDetail => Some(Plane::from_fn(w, h, |i| 0.5 + detail(i) * 4.0)),
        Visual::SharpenMasking => Some(Plane::from_fn(w, h, weight)),
    }
}

/// Post-crop vignetting and grain, placed in the whole image.
fn effects(rgb: &mut [[f32; 3]], w: usize, s: &Settings, frame: &Frame) {
    let e = &s.effects;
    if e.vignette == 0.0 && e.grain == 0.0 {
        return;
    }
    let (fw, fh) = (frame.size.0 as f32, frame.size.1 as f32);
    // Roundness: from the image's proportions (−100) to a circle (+100).
    let t = (e.roundness / 100.0 + 1.0) / 2.0;
    let short = fw.min(fh) / 2.0;
    let sx = 1.0 / (fw / 2.0 * (1.0 - t) + short * t).max(1.0);
    let sy = 1.0 / (fh / 2.0 * (1.0 - t) + short * t).max(1.0);
    let start = e.midpoint / 100.0 * 1.2;
    let spread = 0.05 + e.feather / 100.0;
    let amount = e.vignette / 100.0 * 0.8;
    let protect = e.vignette_highlights / 100.0;
    let grain = e.grain / 100.0 * 0.12;
    // Grain size, in full-resolution pixels; seen reduced, it averages out.
    let size = 1.0 + e.grain_size / 100.0 * 3.0;
    let fade = (size * frame.scale).min(1.0).sqrt();
    let rough = e.grain_roughness / 100.0;
    let (ox, oy) = frame.origin;
    par_rows(rgb, w.max(1), |y0, band| {
        for (i, p) in band.iter_mut().enumerate() {
            let x = ox as f32 + (i % w) as f32 + 0.5;
            let y = oy as f32 + (y0 + i / w) as f32 + 0.5;
            if amount != 0.0 {
                let (dx, dy) = ((x - fw / 2.0) * sx, (y - fh / 2.0) * sy);
                let r = (dx * dx + dy * dy).sqrt();
                let f = smoothstep(start, start + spread, r) * amount;
                *p = vignette(*p, f, e.vignette_style, protect);
            }
            if grain > 0.0 {
                let (gx, gy) = (x / frame.scale / size, y / frame.scale / size);
                let n = value_noise(gx, gy) * (1.0 - rough)
                    + value_noise(gx * 2.0 + 17.3, gy * 2.0 + 5.1) * rough;
                let l = luma(*p).clamp(0.0, 1.0);
                let delta = n * grain * fade * (0.35 + 2.6 * l * (1.0 - l));
                *p = p.map(|v| (v + delta).clamp(0.0, 1.0));
            }
        }
    });
}

/// Vignette of strength `f` (− darkens) in one of the three styles.
fn vignette(p: [f32; 3], f: f32, style: VignetteStyle, protect: f32) -> [f32; 3] {
    if f == 0.0 {
        return p;
    }
    match style {
        VignetteStyle::PaintOverlay => {
            let target = if f < 0.0 { 0.0 } else { 1.0 };
            p.map(|v| v + (target - v) * f.abs())
        }
        VignetteStyle::HighlightPriority | VignetteStyle::ColorPriority => {
            let max = p[0].max(p[1]).max(p[2]);
            let f = if f < 0.0 {
                f * (1.0 - protect * smoothstep(0.6, 1.0, max))
            } else {
                f
            };
            if style == VignetteStyle::HighlightPriority {
                if f < 0.0 {
                    p.map(|v| v * (1.0 + f))
                } else {
                    p.map(|v| 1.0 - (1.0 - v) * (1.0 - f))
                }
            } else {
                // The luminance changes, the color stays.
                let y = luma(p);
                let target = if f < 0.0 {
                    y * (1.0 + f)
                } else {
                    1.0 - (1.0 - y) * (1.0 - f)
                };
                p.map(|v| (v + target - y).clamp(0.0, 1.0))
            }
        }
    }
}

/// Smooth noise in −1..1 with features of about 1 unit.
fn value_noise(x: f32, y: f32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let (tx, ty) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let n = |dx: i32, dy: i32| {
        let mut v = (ix.wrapping_add(dx) as u32).wrapping_mul(0x9E37_79B1)
            ^ (iy.wrapping_add(dy) as u32).wrapping_mul(0x85EB_CA77);
        v ^= v >> 15;
        v = v.wrapping_mul(0x2C1B_3C6D);
        v ^= v >> 12;
        v as f32 / u32::MAX as f32 * 2.0 - 1.0
    };
    let top = n(0, 0) + (n(1, 0) - n(0, 0)) * tx;
    let bottom = n(0, 1) + (n(1, 1) - n(0, 1)) * tx;
    (top + (bottom - top) * ty) * 1.7
}
