//! AI Content-Aware Fill: LaMa (large mask inpainting, Fourier
//! convolutions), run locally. The model sees the hole and what surrounds
//! it at 512 × 512 and paints the hole in.

use std::path::PathBuf;

use image::{GrayImage, Luma, RgbImage, RgbaImage, imageops::FilterType};

use super::models::Model;
use super::runtime::Slot;
use crate::i18n::t;

/// Side of the image the model works on.
const SIZE: u32 = 512;

fn model_path() -> PathBuf {
    Model::Inpaint.path(Model::Inpaint.files()[0].name)
}

/// The model's session.
pub(super) static SESSION: Slot = Slot::new(model_path);

/// `image` (opaque) with the pixels where `hole` painted in by the model.
/// It sees a square around the hole, twice its size where the image
/// allows.
pub fn inpaint(image: &RgbaImage, hole: &[bool]) -> Result<RgbaImage, String> {
    let (w, h) = image.dimensions();
    let mut b = [w, h, 0, 0];
    for (i, _) in hole.iter().enumerate().filter(|(_, h)| **h) {
        let (x, y) = (i as u32 % w, i as u32 / w);
        b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
    }
    if b[0] >= b[2] {
        return Ok(image.clone());
    }
    let side = ((b[2] - b[0]).max(b[3] - b[1]) * 2).max(SIZE / 2);
    let span = |lo: u32, hi: u32, len: u32| {
        let size = side.min(len);
        let start = ((lo + hi) / 2).saturating_sub(size / 2).min(len - size);
        (start, size)
    };
    let (x0, cw) = span(b[0], b[2], w);
    let (y0, ch) = span(b[1], b[3], h);
    let crop = image::imageops::crop_imm(image, x0, y0, cw, ch).to_image();
    let input = image::imageops::resize(&crop, SIZE, SIZE, FilterType::Triangle);
    // Any part of the hole in a pixel makes it a hole, and a pixel more
    // around: the model must not see the edge of what is removed.
    let local = GrayImage::from_fn(cw, ch, |x, y| {
        Luma([if hole[((y0 + y) * w + x0 + x) as usize] {
            255
        } else {
            0
        }])
    });
    let mask = grow(&image::imageops::resize(
        &local,
        SIZE,
        SIZE,
        FilterType::Triangle,
    ));

    let n = (SIZE * SIZE) as usize;
    let mut pixels = vec![0.0f32; 3 * n];
    for (i, p) in input.pixels().enumerate() {
        for c in 0..3 {
            pixels[c * n + i] = p.0[c] as f32 / 255.0;
        }
    }
    let holes: Vec<f32> = mask.pixels().map(|p| f32::from(p.0[0] > 0)).collect();
    let shape = |channels: usize| [1, channels, SIZE as usize, SIZE as usize];
    let painted = SESSION.run(|session, ort| {
        let outputs = session.run(ort::inputs![
            "image" => ort.tensor(&shape(3), &pixels)?,
            "mask" => ort.tensor(&shape(1), &holes)?,
        ])?;
        let (_, values) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(values.to_vec())
    })?;
    if painted.len() < 3 * n {
        return Err(t!("ai-error-model-corrupt"));
    }
    // Output in 0..255.
    let out = RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let i = (y * SIZE + x) as usize;
        image::Rgb(std::array::from_fn(|c| {
            painted[c * n + i].round().clamp(0.0, 255.0) as u8
        }))
    });
    let out = image::imageops::resize(&out, cw, ch, FilterType::CatmullRom);
    let mut result = image.clone();
    for (x, y, p) in out.enumerate_pixels() {
        let (ix, iy) = (x0 + x, y0 + y);
        if hole[(iy * w + ix) as usize] {
            let [r, g, b] = p.0;
            result.put_pixel(ix, iy, image::Rgba([r, g, b, 255]));
        }
    }
    Ok(result)
}

/// The mask's holes grown by two pixels.
fn grow(mask: &GrayImage) -> GrayImage {
    let (w, h) = mask.dimensions();
    GrayImage::from_fn(w, h, |x, y| {
        let near = (y.saturating_sub(2)..(y + 3).min(h)).any(|ny| {
            (x.saturating_sub(2)..(x + 3).min(w)).any(|nx| mask.get_pixel(nx, ny).0[0] > 0)
        });
        Luma([if near { 255 } else { 0 }])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disc removed from a striped image is painted with the stripes,
    /// not with the disc's color. Needs the downloaded model.
    #[test]
    #[ignore = "needs the LaMa model"]
    fn fills_the_hole_with_its_surroundings() {
        let (w, h) = (700u32, 500u32);
        let disc = |x: u32, y: u32| {
            (x as f32 - 350.0).powi(2) + (y as f32 - 250.0).powi(2) < 80.0f32.powi(2)
        };
        let stripes = |y: u32| {
            if y % 40 < 20 {
                [40, 90, 200, 255]
            } else {
                [230, 230, 220, 255]
            }
        };
        let image = RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba(if disc(x, y) {
                [250, 20, 20, 255]
            } else {
                stripes(y)
            })
        });
        let hole: Vec<bool> = (0..w * h).map(|i| disc(i % w, i / w)).collect();
        let t = std::time::Instant::now();
        let out = inpaint(&image, &hole).unwrap();
        eprintln!("inpainted in {:?}", t.elapsed());
        let (mut error, mut n) = (0.0, 0.0);
        for (x, y, p) in out.enumerate_pixels() {
            if disc(x, y) {
                assert!(p.0[0] < 245 || p.0[1] > 60, "red left at ({x}, {y})");
                error += (0..3)
                    .map(|c| p.0[c].abs_diff(stripes(y)[c]) as f32)
                    .sum::<f32>()
                    / 3.0;
                n += 1.0;
            } else {
                assert_eq!(p, image.get_pixel(x, y));
            }
        }
        assert!(error / n < 40.0, "mean error {}", error / n);
    }
}
