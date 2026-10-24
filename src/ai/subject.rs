//! AI "Select Subject": BiRefNet model (dichotomous image segmentation,
//! lite version), run locally with ONNX Runtime.

use std::path::PathBuf;

use image::{GrayImage, RgbaImage, imageops::FilterType};

use super::models::Model;
use super::runtime::Slot;
use crate::i18n::t;

/// Side of the image the model expects, and its normalization (ImageNet).
const SIZE: u32 = 1024;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

fn model_path() -> PathBuf {
    Model::Subject.path(Model::Subject.files()[0].name)
}

/// The model's session (it weighs over 200 MB).
pub(super) static SESSION: Slot = Slot::new(model_path);

/// Coverage (0..255) of the subject of `image`, at its size. The model only
/// sees the bounding box of the visible pixels, with transparency laid over
/// white (otherwise it would take the whole cut-out shape for the subject).
pub fn select_subject(image: &RgbaImage) -> Result<GrayImage, String> {
    select_subject_with(&SESSION, image)
}

fn select_subject_with(slot: &'static Slot, image: &RgbaImage) -> Result<GrayImage, String> {
    let (w, h) = image.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in image.enumerate_pixels() {
        if p.0[3] > 0 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
        }
    }
    let mut full = GrayImage::new(w, h);
    if x0 >= x1 {
        return Ok(full);
    }
    let visible = RgbaImage::from_fn(x1 - x0, y1 - y0, |x, y| {
        let [r, g, b, a] = image.get_pixel(x0 + x, y0 + y).0;
        let on_white = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        image::Rgba([on_white(r), on_white(g), on_white(b), a])
    });
    let mask = run(slot, &visible)?;
    image::imageops::replace(&mut full, &mask, x0 as i64, y0 as i64);
    Ok(full)
}

/// The model on an image: the coverage of its subject.
fn run(slot: &'static Slot, image: &RgbaImage) -> Result<GrayImage, String> {
    let resized = image::imageops::resize(image, SIZE, SIZE, FilterType::Triangle);
    let n = (SIZE * SIZE) as usize;
    let mut data = vec![0.0f32; 3 * n];
    for (i, p) in resized.pixels().enumerate() {
        for c in 0..3 {
            data[c * n + i] = (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let shape = [1, 3, SIZE as usize, SIZE as usize];
    let logits = slot.run(|session, ort| {
        let input = ort.tensor(&shape, &data)?;
        let outputs = session.run(ort::inputs![input])?;
        // First output: the final subject map (logits).
        let (_, logits) = outputs[0].try_extract_tensor::<f32>()?;
        Ok(logits.to_vec())
    })?;
    if logits.len() < (SIZE * SIZE) as usize {
        return Err(t!("ai-error-model-corrupt"));
    }
    let mask = GrayImage::from_fn(SIZE, SIZE, |x, y| {
        let v = 1.0 / (1.0 + (-logits[(y * SIZE + x) as usize]).exp());
        image::Luma([(v * 255.0).round() as u8])
    });
    // Not limited by the pixels' opacity: Remove Background's mask comes on
    // top of it (Select Subject limits it, see `select_tools`).
    Ok(image::imageops::resize(
        &mask,
        image.width(),
        image.height(),
        FilterType::Triangle,
    ))
}

#[cfg(test)]
mod tests {
    /// A disc on a plain background is the subject. On Windows, this also
    /// loads the ONNX Runtime embedded in the .exe, before any other call
    /// to it. Needs the downloaded model.
    #[test]
    #[ignore = "needs the BiRefNet model"]
    fn finds_a_disc() {
        let inside = |x: u32, y: u32| {
            (x as f32 - 300.0).powi(2) + (y as f32 - 200.0).powi(2) < 120.0f32.powi(2)
        };
        let image = image::RgbaImage::from_fn(600, 400, |x, y| {
            if inside(x, y) {
                image::Rgba([210, 50, 40, 255])
            } else {
                image::Rgba([235, 235, 228, 255])
            }
        });
        let mask = super::select_subject(&image).unwrap();
        let wrong = mask
            .enumerate_pixels()
            .filter(|(x, y, m)| (m.0[0] > 127) != inside(*x, *y))
            .count();
        eprintln!(
            "{wrong} pixels wrong, on {:?}",
            super::super::models::Model::Subject.devices()
        );
        assert!(wrong < 2000, "{wrong} pixels wrong");
    }

    /// `MODEL`: model path, `IMG`: image, `OUT`: result on a magenta
    /// background; `CIRCLE=cx,cy,r` makes everything outside the disc transparent.
    #[test]
    #[ignore = "trial with the model"]
    fn subject_on_file() {
        static TRIAL: super::Slot = super::Slot::new(|| std::env::var_os("MODEL").unwrap().into());
        let mut img = image::open(std::env::var("IMG").unwrap())
            .unwrap()
            .to_rgba8();
        if let Ok(c) = std::env::var("CIRCLE") {
            let v: Vec<f32> = c.split(',').map(|s| s.parse().unwrap()).collect();
            for (x, y, p) in img.enumerate_pixels_mut() {
                if (x as f32 - v[0]).powi(2) + (y as f32 - v[1]).powi(2) > v[2] * v[2] {
                    p.0 = [0, 0, 0, 0];
                }
            }
        }
        for _ in 0..2 {
            let t = std::time::Instant::now();
            let mask = super::select_subject_with(&TRIAL, &img).unwrap();
            eprintln!("{:?} in {:?}", img.dimensions(), t.elapsed());
            let mut cut = img.clone();
            for (p, m) in cut.pixels_mut().zip(mask.pixels()) {
                p.0[3] = (p.0[3] as u32 * m.0[0] as u32 / 255) as u8;
            }
            let mut bg = image::RgbaImage::from_pixel(
                cut.width(),
                cut.height(),
                image::Rgba([255, 0, 255, 255]),
            );
            image::imageops::overlay(&mut bg, &cut, 0, 0);
            bg.save(std::env::var("OUT").unwrap()).unwrap();
        }
    }
}
