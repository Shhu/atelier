//! What Camera Raw develops: linear floating-point pixels, from an 8-bit
//! layer or from a RAW file (then in the camera's color space, with what
//! it takes to color them for any white balance).

use std::sync::Arc;

use image::RgbaImage;
use image::metadata::Orientation;

use super::settings::Settings;
use super::white::{self, D50, Mat3};
use crate::filters::par_rows;

/// Pixels to develop.
#[derive(Clone, Debug)]
pub struct Source {
    pub width: u32,
    pub height: u32,
    /// Linear values: sRGB primaries, or for a RAW file the camera's,
    /// divided by its white balance as shot (gray is gray) and clipped
    /// highlights made white.
    pub rgb: Vec<[f32; 3]>,
    /// Opacity of a layer's pixels (`None`: opaque).
    pub alpha: Option<Vec<u8>>,
    /// Color of a RAW file.
    pub camera: Option<Arc<Camera>>,
}

/// A camera's color, from its DNG tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    /// One or two calibrations, under lights of different temperatures.
    pub calibrations: Vec<Calibration>,
    pub analog_balance: [f64; 3],
    /// Camera values of a neutral surface under the light of the shot.
    pub as_shot: [f64; 3],
    /// Stops to add for a normal exposure.
    pub baseline_exposure: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Calibration {
    /// Of the calibration light, in kelvins.
    pub temperature: f64,
    /// XYZ to camera.
    pub color_matrix: Mat3,
    /// White-balanced camera to XYZ under D50, if given.
    pub forward_matrix: Option<Mat3>,
    pub camera_calibration: Mat3,
}

/// Linear value of an 8-bit sRGB level.
pub fn decode_level(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

impl Source {
    /// The pixels of a layer.
    pub fn from_rgba(image: &RgbaImage) -> Source {
        let lut: [f32; 256] = std::array::from_fn(|i| decode_level(i as u8));
        let raw = image.as_raw();
        let opaque = raw.chunks_exact(4).all(|p| p[3] == 255);
        Source {
            width: image.width(),
            height: image.height(),
            rgb: raw
                .chunks_exact(4)
                .map(|p| [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]])
                .collect(),
            alpha: (!opaque).then(|| raw.chunks_exact(4).map(|p| p[3]).collect()),
            camera: None,
        }
    }

    pub fn is_raw(&self) -> bool {
        self.camera.is_some()
    }

    /// Source values to linear sRGB, with the white balance of `settings`.
    pub fn working_matrix(&self, settings: &Settings) -> Mat3 {
        match &self.camera {
            Some(camera) => camera.to_srgb(settings.temperature, settings.tint),
            None => white::relative_balance(settings.temperature, settings.tint),
        }
    }

    /// Temperature and tint that make the source value `rgb` neutral (the
    /// White Balance tool).
    pub fn neutral_balance(&self, rgb: [f32; 3]) -> (f32, f32) {
        let rgb = rgb.map(|v| v.max(1e-6) as f64);
        match &self.camera {
            Some(camera) => camera.neutral_temperature(rgb),
            None => white::relative_neutral(rgb),
        }
    }

    /// Reduced by `factor` (at most 1), each pixel the average of those it
    /// covers.
    pub fn downscaled(&self, factor: f32) -> Source {
        let (w, h) = (self.width as usize, self.height as usize);
        let tw = ((w as f32 * factor).round() as usize).clamp(1, w);
        let th = ((h as f32 * factor).round() as usize).clamp(1, h);
        if (tw, th) == (w, h) {
            return self.clone();
        }
        let alpha = self.alpha.as_ref().map(|a| {
            let plane: Vec<[f32; 1]> = a.iter().map(|&v| [v as f32]).collect();
            resample(&plane, w, h, tw, th)
                .iter()
                .map(|v| v[0].round().clamp(0.0, 255.0) as u8)
                .collect()
        });
        Source {
            width: tw as u32,
            height: th as u32,
            rgb: resample(&self.rgb, w, h, tw, th),
            alpha,
            camera: self.camera.clone(),
        }
    }

    /// The part x0, y0, x1, y1 (exclusive).
    pub fn crop(&self, [x0, y0, x1, y1]: [u32; 4]) -> Source {
        let w = self.width as usize;
        let rows = y0 as usize..y1 as usize;
        let cols = x0 as usize..x1 as usize;
        let take = |y: usize| y * w + cols.start..y * w + cols.end;
        Source {
            width: x1 - x0,
            height: y1 - y0,
            rgb: rows
                .clone()
                .flat_map(|y| self.rgb[take(y)].iter().copied())
                .collect(),
            alpha: self
                .alpha
                .as_ref()
                .map(|a| rows.flat_map(|y| a[take(y)].iter().copied()).collect()),
            camera: self.camera.clone(),
        }
    }

    /// Turned the right way up.
    pub fn oriented(self, orientation: Orientation) -> Source {
        if orientation == Orientation::NoTransforms {
            return self;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (rgb, tw, th) = orient(&self.rgb, w, h, orientation);
        let alpha = self.alpha.map(|a| orient(&a, w, h, orientation).0);
        Source {
            width: tw as u32,
            height: th as u32,
            rgb,
            alpha,
            camera: self.camera,
        }
    }
}

/// `data` (w × h) seen with `orientation` applied, and its new size.
fn orient<T: Copy>(data: &[T], w: usize, h: usize, o: Orientation) -> (Vec<T>, usize, usize) {
    use Orientation::*;
    let turned = matches!(o, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH);
    let (tw, th) = if turned { (h, w) } else { (w, h) };
    let source = |x: usize, y: usize| match o {
        NoTransforms => (x, y),
        Rotate90 => (y, h - 1 - x),
        Rotate180 => (w - 1 - x, h - 1 - y),
        Rotate270 => (w - 1 - y, x),
        FlipHorizontal => (w - 1 - x, y),
        FlipVertical => (x, h - 1 - y),
        Rotate90FlipH => (y, x),
        Rotate270FlipH => (w - 1 - y, h - 1 - x),
    };
    let mut out = Vec::with_capacity(tw * th);
    for y in 0..th {
        for x in 0..tw {
            let (sx, sy) = source(x, y);
            out.push(data[sy * w + sx]);
        }
    }
    (out, tw, th)
}

/// Area weights of a reduction from `n` to `m` samples: for each output,
/// its first input and the inputs' weights (summing to 1).
fn area_weights(n: usize, m: usize) -> Vec<(usize, Vec<f32>)> {
    let step = n as f64 / m as f64;
    (0..m)
        .map(|i| {
            let (a, b) = (i as f64 * step, ((i + 1) as f64 * step).min(n as f64));
            let first = a.floor() as usize;
            let last = (b.ceil() as usize).clamp(first + 1, n);
            let weights = (first..last)
                .map(|j| ((b.min(j as f64 + 1.0) - a.max(j as f64)) / (b - a)) as f32)
                .collect();
            (first, weights)
        })
        .collect()
}

/// Area reduction of `src` (w × h) to tw × th.
pub fn resample<const N: usize>(
    src: &[[f32; N]],
    w: usize,
    h: usize,
    tw: usize,
    th: usize,
) -> Vec<[f32; N]> {
    let (xs, ys) = (area_weights(w, tw), area_weights(h, th));
    let mut wide = vec![[0.0; N]; tw * h];
    par_rows(&mut wide, tw, |y0, band| {
        for (i, line) in band.chunks_exact_mut(tw).enumerate() {
            let row = &src[(y0 + i) * w..][..w];
            for (out, (first, weights)) in line.iter_mut().zip(&xs) {
                for (k, wt) in weights.iter().enumerate() {
                    let v = row[first + k];
                    for c in 0..N {
                        out[c] += v[c] * wt;
                    }
                }
            }
        }
    });
    let mut out = vec![[0.0; N]; tw * th];
    par_rows(&mut out, tw, |y0, band| {
        for (i, line) in band.chunks_exact_mut(tw).enumerate() {
            let (first, weights) = &ys[y0 + i];
            for (k, wt) in weights.iter().enumerate() {
                let row = &wide[(first + k) * tw..][..tw];
                for (o, v) in line.iter_mut().zip(row) {
                    for c in 0..N {
                        o[c] += v[c] * wt;
                    }
                }
            }
        }
    });
    out
}

impl Camera {
    /// XYZ to camera for a white `xy`, the two calibrations weighted by
    /// its temperature; the forward matrix if both calibrations have one;
    /// analog balance × camera calibration.
    fn calibrated(&self, xy: [f64; 2]) -> (Mat3, Option<Mat3>, Mat3) {
        let mut cals: Vec<&Calibration> = self.calibrations.iter().collect();
        cals.sort_by(|a, b| a.temperature.total_cmp(&b.temperature));
        let (first, last) = (cals[0], cals[cals.len() - 1]);
        let g = if cals.len() == 1 || first.temperature >= last.temperature {
            1.0
        } else {
            let (t, _) = white::temperature_of(xy);
            ((1.0 / t - 1.0 / last.temperature)
                / (1.0 / first.temperature - 1.0 / last.temperature))
                .clamp(0.0, 1.0)
        };
        let mix = |a: &Mat3, b: &Mat3| -> Mat3 {
            std::array::from_fn(|i| std::array::from_fn(|j| a[i][j] * g + b[i][j] * (1.0 - g)))
        };
        let cm = mix(&first.color_matrix, &last.color_matrix);
        let cc = mix(&first.camera_calibration, &last.camera_calibration);
        let fm = match (first.forward_matrix, last.forward_matrix) {
            (Some(a), Some(b)) => Some(mix(&a, &b)),
            _ => None,
        };
        let abcc = white::mul(&white::diagonal(self.analog_balance), &cc);
        (white::mul(&abcc, &cm), fm, abcc)
    }

    /// Camera values of a neutral surface under the white `xy`.
    pub fn neutral_of(&self, xy: [f64; 2]) -> [f64; 3] {
        white::apply(&self.calibrated(xy).0, white::xy_to_xyz(xy))
    }

    /// The white under which `neutral` (camera values) is neutral.
    fn white_of_neutral(&self, neutral: [f64; 3]) -> [f64; 2] {
        let mut xy = D50;
        for _ in 0..30 {
            let Some(to_xyz) = white::inverse(&self.calibrated(xy).0) else {
                break;
            };
            let next = white::xyz_to_xy(white::apply(&to_xyz, neutral));
            let converged = (next[0] - xy[0]).abs() + (next[1] - xy[1]).abs() < 1e-7;
            xy = next;
            if converged {
                break;
            }
        }
        xy
    }

    /// Camera to XYZ under D50 for the white `xy` (camera `neutral`),
    /// scaled so that the camera's clipping level of the neutral is Y = 1.
    fn to_pcs(&self, xy: [f64; 2], neutral: [f64; 3]) -> Mat3 {
        let (xyz_to_camera, forward, abcc) = self.calibrated(xy);
        let m = match (forward, white::inverse(&abcc)) {
            (Some(fm), Some(abcc_inv)) => {
                let reference = white::apply(&abcc_inv, neutral);
                let d = white::diagonal(reference.map(|v| 1.0 / v.max(1e-9)));
                white::mul(&fm, &white::mul(&d, &abcc_inv))
            }
            _ => {
                let to_xyz = white::inverse(&xyz_to_camera).unwrap_or(white::IDENTITY);
                white::mul(&white::adaptation(xy, D50), &to_xyz)
            }
        };
        let max = neutral.iter().copied().fold(1e-9, f64::max);
        let y = white::apply(&m, neutral.map(|v| v / max))[1];
        white::scale(&m, 1.0 / y.max(1e-9))
    }

    /// Stored values (see [`Source::rgb`]) to linear sRGB, for a white
    /// balance in kelvins and tint.
    pub fn to_srgb(&self, temperature: f32, tint: f32) -> Mat3 {
        let xy = white::white_of(temperature as f64, tint as f64);
        let neutral = self.neutral_of(xy);
        let max = self.as_shot.iter().copied().fold(1e-9, f64::max);
        let unbalance = white::diagonal(self.as_shot.map(|v| v / max));
        let pcs = self.to_pcs(xy, neutral);
        white::mul(&white::xyz_d50_to_srgb(), &white::mul(&pcs, &unbalance))
    }

    /// The white balance as shot.
    pub fn as_shot_temperature(&self) -> (f32, f32) {
        temperature_in_range(white::temperature_of(self.white_of_neutral(self.as_shot)))
    }

    /// The white balance that makes the stored value `rgb` neutral.
    fn neutral_temperature(&self, rgb: [f64; 3]) -> (f32, f32) {
        let max = self.as_shot.iter().copied().fold(1e-9, f64::max);
        let camera = std::array::from_fn(|i| rgb[i] * self.as_shot[i] / max);
        temperature_in_range(white::temperature_of(self.white_of_neutral(camera)))
    }
}

fn temperature_in_range((t, tint): (f64, f64)) -> (f32, f32) {
    let (tr, nr) = (white::TEMPERATURE_RANGE, white::TINT_RANGE);
    (
        (t as f32).clamp(*tr.start(), *tr.end()).round(),
        (tint as f32).clamp(*nr.start(), *nr.end()).round(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscaling_averages_and_orientation_turns() {
        let src = Source {
            width: 4,
            height: 2,
            rgb: (0..8).map(|i| [i as f32; 3]).collect(),
            alpha: None,
            camera: None,
        };
        let half = src.downscaled(0.5);
        assert_eq!((half.width, half.height), (2, 1));
        // (0 + 1 + 4 + 5) / 4.
        assert!((half.rgb[0][0] - 2.5).abs() < 1e-5);
        let turned = src.clone().oriented(Orientation::Rotate90);
        assert_eq!((turned.width, turned.height), (2, 4));
        // The bottom-left pixel comes to the top left.
        assert_eq!(turned.rgb[0][0], 4.0);
        assert_eq!(turned.rgb[1][0], 0.0);
        let part = src.crop([1, 0, 3, 2]);
        assert_eq!(
            part.rgb.iter().map(|p| p[0]).collect::<Vec<_>>(),
            [1.0, 2.0, 5.0, 6.0]
        );
    }
}
