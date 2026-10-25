//! AI Object Selection: SAM 2.1 (Segment Anything, tiny), run locally. The
//! image is analysed once (encoder, the slow part); then each click or
//! rectangle gives the object's mask in a few tens of milliseconds
//! (decoder).

use std::path::PathBuf;

use image::{GrayImage, Luma, RgbaImage, imageops::FilterType};

use super::models::Model;
use super::runtime::{Device, Slot};

/// Side of the image the encoder expects, and its normalization (ImageNet).
const SIZE: u32 = 1024;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

fn encoder_path() -> PathBuf {
    Model::Objects.path("vision_encoder.onnx")
}

fn decoder_path() -> PathBuf {
    Model::Objects.path("prompt_encoder_mask_decoder.onnx")
}

/// The model's two sessions.
pub(super) static ENCODER: Slot = Slot::new(encoder_path);
pub(super) static DECODER: Slot = Slot::new(decoder_path);

/// Where each session last ran (analysis, then masks).
pub(super) fn devices() -> [Option<Device>; 2] {
    [ENCODER.device(), DECODER.device()]
}

/// What the encoder found in an image: three feature maps (shape, values),
/// and the size of the image.
pub struct Features {
    maps: Vec<(Vec<usize>, Vec<f32>)>,
    size: (u32, u32),
}

impl Features {
    /// Size of the analysed image.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}

/// What designates the object, in image pixels: points (`true`: on the
/// object, `false`: not on it) and/or a rectangle `[x0, y0, x1, y1]`.
#[derive(Clone, Default, Debug)]
pub struct Prompt {
    pub points: Vec<(f32, f32, bool)>,
    pub rect: Option<[f32; 4]>,
}

/// Coverage (0..255) of an object over a part of the image, whose top-left
/// corner is at `offset`.
pub struct Mask {
    pub coverage: GrayImage,
    pub offset: (i64, i64),
}

/// The decoder's answer: object scores (logits, > 0 on the object) over a
/// low-resolution grid stretched over the whole image.
struct Logits {
    values: Vec<f32>,
    size: (usize, usize),
}

/// Analyses `image` (transparency over white): to do once per image.
pub fn encode(image: &RgbaImage) -> Result<Features, super::Error> {
    let resized = image::imageops::resize(image, SIZE, SIZE, FilterType::Triangle);
    let n = (SIZE * SIZE) as usize;
    let mut data = vec![0.0f32; 3 * n];
    for (i, p) in resized.pixels().enumerate() {
        let a = p.0[3] as f32 / 255.0;
        for c in 0..3 {
            let v = (p.0[c] as f32 / 255.0) * a + (1.0 - a);
            data[c * n + i] = (v - MEAN[c]) / STD[c];
        }
    }
    let shape = [1, 3, SIZE as usize, SIZE as usize];
    let maps = ENCODER.run(|session, ort| {
        let outputs = session.run(ort::inputs!["pixel_values" => ort.tensor(&shape, &data)?])?;
        (0..3)
            .map(|k| {
                let (shape, values) = outputs[format!("image_embeddings.{k}").as_str()]
                    .try_extract_tensor::<f32>()?;
                let shape = shape.iter().map(|&d| d as usize).collect();
                Ok((shape, values.to_vec()))
            })
            .collect()
    })?;
    Ok(Features {
        maps,
        size: image.dimensions(),
    })
}

/// Coverage of the object designated by `prompt`, at the size of the
/// encoded image, cropped around the object; `None` if the prompt is empty
/// or nothing is found.
pub fn segment(features: &Features, prompt: &Prompt) -> Result<Option<Mask>, super::Error> {
    let Some(logits) = decode(features, prompt)? else {
        return Ok(None);
    };
    let Some(region) = object_region(&logits, features.size) else {
        return Ok(None);
    };
    Ok(Some(Mask {
        coverage: coverage(&logits, features.size, region),
        offset: (region[0] as i64, region[1] as i64),
    }))
}

/// Region `[x0, y0, x1, y1]` of an image of `(w, h)` where the object of
/// `logits` lies: the grid cells where it is, and one cell around (the
/// contour runs within half a cell of them); `None` if it is nowhere.
fn object_region(logits: &Logits, (w, h): (u32, u32)) -> Option<[u32; 4]> {
    let (mw, mh) = logits.size;
    let (mut c0, mut r0, mut c1, mut r1) = (mw, mh, 0, 0);
    for (i, _) in logits.values.iter().enumerate().filter(|(_, v)| **v > 0.0) {
        let (c, r) = (i % mw, i / mw);
        (c0, r0, c1, r1) = (c0.min(c), r0.min(r), c1.max(c + 1), r1.max(r + 1));
    }
    if c0 >= c1 {
        return None;
    }
    let to_image = |cell: usize, cells: usize, side: u32| {
        ((cell as f64 * side as f64 / cells as f64) as u32).min(side)
    };
    let x0 = to_image(c0.saturating_sub(1), mw, w);
    let y0 = to_image(r0.saturating_sub(1), mh, h);
    let x1 = to_image((c1 + 1).min(mw), mw, w).max(x0 + 1);
    let y1 = to_image((r1 + 1).min(mh), mh, h).max(y0 + 1);
    Some([x0, y0, x1, y1])
}

/// Coverage of the object designated by `prompt` over the whole image,
/// computed directly at `size` (a preview); `None` if the prompt is empty.
pub fn segment_preview(
    features: &Features,
    prompt: &Prompt,
    size: (u32, u32),
) -> Result<Option<GrayImage>, super::Error> {
    Ok(decode(features, prompt)?.map(|logits| coverage(&logits, size, [0, 0, size.0, size.1])))
}

/// The decoder on `prompt`: the best of its candidate masks.
fn decode(features: &Features, prompt: &Prompt) -> Result<Option<Logits>, super::Error> {
    if prompt.points.is_empty() && prompt.rect.is_none() {
        return Ok(None);
    }
    let (w, h) = features.size;
    // The encoder saw the image stretched to 1024 × 1024.
    let (sx, sy) = (SIZE as f32 / w as f32, SIZE as f32 / h as f32);
    let n = prompt.points.len();
    let coords: Vec<f32> = prompt
        .points
        .iter()
        .flat_map(|&(x, y, _)| [x * sx, y * sy])
        .collect();
    let labels: Vec<i64> = prompt
        .points
        .iter()
        .map(|&(.., on)| i64::from(on))
        .collect();
    let rect: Vec<f32> = prompt
        .rect
        .map(|[x0, y0, x1, y1]| vec![x0 * sx, y0 * sy, x1 * sx, y1 * sy])
        .unwrap_or_default();
    let boxes = usize::from(prompt.rect.is_some());
    DECODER
        .run(|s, ort| {
            let [e0, e1, e2] = [0, 1, 2].map(|k| {
                let (shape, values) = &features.maps[k];
                ort.tensor(shape, values)
            });
            let outputs = s.run(ort::inputs![
                "input_points" => ort.tensor(&[1, 1, n, 2], &coords)?,
                "input_labels" => ort.tensor(&[1, 1, n], &labels)?,
                "input_boxes" => ort.tensor(&[1, boxes, 4], &rect)?,
                "image_embeddings.0" => e0?,
                "image_embeddings.1" => e1?,
                "image_embeddings.2" => e2?,
            ])?;
            let (_, scores) = outputs["iou_scores"].try_extract_tensor::<f32>()?;
            let (shape, masks) = outputs["pred_masks"].try_extract_tensor::<f32>()?;
            // Three candidate masks: the one the model rates best.
            let best = (0..scores.len().min(3))
                .max_by(|&a, &b| scores[a].total_cmp(&scores[b]))
                .unwrap_or(0);
            let (mh, mw) = (shape[3] as usize, shape[4] as usize);
            let start = best * mw * mh;
            Ok(Logits {
                values: masks[start..start + mw * mh].to_vec(),
                size: (mw, mh),
            })
        })
        .map(Some)
}

/// Coverage (0..255) of `logits` stretched to an image of `size`, over its
/// region `[x0, y0, x1, y1]`. The scores are interpolated (bilinear, like
/// SAM's own post-processing), then the contour (score 0) is drawn with an
/// edge of one pixel, whatever the enlargement.
fn coverage(logits: &Logits, size: (u32, u32), [x0, y0, x1, y1]: [u32; 4]) -> GrayImage {
    let (mw, mh) = logits.size;
    // Source cells and weight of each output column and row (pixel centers
    // aligned), one more around the region for the gradient.
    let taps = |from: u32, to: u32, side: u32, cells: usize| -> Vec<(usize, usize, f32)> {
        (from.saturating_sub(1)..(to + 1).min(side))
            .map(|i| {
                let s = ((i as f32 + 0.5) * cells as f32 / side as f32 - 0.5)
                    .clamp(0.0, (cells - 1) as f32);
                let a = s as usize;
                (a, (a + 1).min(cells - 1), s - a as f32)
            })
            .collect()
    };
    let (cols, rows) = (taps(x0, x1, size.0, mw), taps(y0, y1, size.1, mh));
    let v = &logits.values;
    let field: Vec<f32> = rows
        .iter()
        .flat_map(|&(r0, r1, ty)| {
            cols.iter().map(move |&(c0, c1, tx)| {
                let top = v[r0 * mw + c0] * (1.0 - tx) + v[r0 * mw + c1] * tx;
                let bottom = v[r1 * mw + c0] * (1.0 - tx) + v[r1 * mw + c1] * tx;
                top * (1.0 - ty) + bottom * ty
            })
        })
        .collect();
    let (fw, fh) = (cols.len(), rows.len());
    let (dx, dy) = ((x0 > 0) as usize, (y0 > 0) as usize);
    GrayImage::from_fn(x1 - x0, y1 - y0, |x, y| {
        let (i, j) = (x as usize + dx, y as usize + dy);
        let at = |i: usize, j: usize| field[j * fw + i];
        let l = at(i, j);
        // Distance to the contour, in pixels: score over its slope.
        let slope_along = |a: usize, b: usize, va: f32, vb: f32| {
            if b > a {
                (vb - va) / (b - a) as f32
            } else {
                0.0
            }
        };
        let (left, right) = (i.saturating_sub(1), (i + 1).min(fw - 1));
        let (up, down) = (j.saturating_sub(1), (j + 1).min(fh - 1));
        let gx = slope_along(left, right, at(left, j), at(right, j));
        let gy = slope_along(up, down, at(i, up), at(i, down));
        let slope = (gx * gx + gy * gy).sqrt();
        let c = if slope > 1e-6 {
            (0.5 + l / slope).clamp(0.0, 1.0)
        } else {
            f32::from(l > 0.0)
        };
        Luma([(c * 255.0).round() as u8])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scores of a disc (centre and radius in pixels of an image of
    /// `size`), like SAM's: a few units per grid cell from the contour.
    fn disc_logits(size: (u32, u32), (cx, cy, r): (f32, f32, f32)) -> Logits {
        let n = 256;
        let cell = size.0 as f32 / n as f32;
        let values = (0..n * n)
            .map(|i| {
                let x = ((i % n) as f32 + 0.5) * size.0 as f32 / n as f32;
                let y = ((i / n) as f32 + 0.5) * size.1 as f32 / n as f32;
                (r - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt()) / cell * 4.0
            })
            .collect();
        Logits {
            values,
            size: (n, n),
        }
    }

    /// Enlarged 23 times (a 6000 px photo), the contour stays sharp (an
    /// edge of a pixel) and in place: the scores are interpolated, not the
    /// coverage.
    #[test]
    fn mask_edges_stay_sharp_on_large_images() {
        let (size, disc) = ((6000, 4000), (2500.0, 2000.0, 1200.0));
        let logits = disc_logits(size, disc);
        let region = object_region(&logits, size).unwrap();
        let mask = coverage(&logits, size, region);
        let at = |x: u32, y: u32| {
            let (mx, my) = (x.wrapping_sub(region[0]), y.wrapping_sub(region[1]));
            mask.get_pixel_checked(mx, my).map_or(0, |p| p.0[0])
        };
        let soft = (0..size.0)
            .filter(|&x| (6..250).contains(&at(x, 2000)))
            .count();
        assert!(soft <= 4, "{soft} soft pixels on the centre row");
        let (cx, cy, r) = disc;
        let wrong = (1000..3000)
            .flat_map(|y| (1000..4000).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let inside = (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2) < r * r;
                (at(x, y) > 127) != inside
            })
            .count();
        // Perimeter: 7540 px.
        assert!(wrong < 600, "{wrong} pixels wrong");
    }

    /// The mask of a click is computed around the object only, and is the
    /// same there as over the whole image.
    #[test]
    fn object_region_holds_the_whole_object() {
        let (size, disc) = ((900, 600), (300.0, 250.0, 80.0));
        let logits = disc_logits(size, disc);
        let [x0, y0, x1, y1] = object_region(&logits, size).unwrap();
        assert!(
            x0 > 150 && y0 > 100 && x1 < 450 && y1 < 400,
            "{:?}",
            [x0, y0, x1, y1]
        );
        let part = coverage(&logits, size, [x0, y0, x1, y1]);
        let whole = coverage(&logits, size, [0, 0, size.0, size.1]);
        for (x, y, p) in whole.enumerate_pixels() {
            let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            let expected = if inside {
                part.get_pixel(x - x0, y - y0).0[0]
            } else {
                0
            };
            assert_eq!(p.0[0], expected, "({x}, {y})");
        }
    }

    /// Two discs on a gradient: a click on each, then a rectangle around
    /// one, select exactly that disc. Needs the downloaded model.
    #[test]
    #[ignore = "needs the SAM model"]
    fn clicks_and_rectangle_select_the_object() {
        let disc = |x: u32, y: u32, cx: f32, cy: f32, r: f32| {
            (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2) < r * r
        };
        let image = RgbaImage::from_fn(800, 500, |x, y| {
            if disc(x, y, 250.0, 250.0, 120.0) {
                image::Rgba([220, 40, 40, 255])
            } else if disc(x, y, 580.0, 260.0, 90.0) {
                image::Rgba([40, 70, 220, 255])
            } else {
                image::Rgba([(x / 4) as u8, 200, (y / 3) as u8, 255])
            }
        });
        let features = encode(&image).unwrap();
        let red = Prompt {
            points: vec![(250.0, 250.0, true)],
            rect: None,
        };
        let blue = Prompt {
            points: Vec::new(),
            rect: Some([480.0, 160.0, 680.0, 360.0]),
        };
        for (prompt, (cx, cy, r)) in [(red, (250.0, 250.0, 120.0)), (blue, (580.0, 260.0, 90.0))] {
            let mask = segment(&features, &prompt).unwrap().unwrap();
            let (ox, oy) = mask.offset;
            let wrong = (0..500)
                .flat_map(|y| (0..800).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let (mx, my) = (x as i64 - ox, y as i64 - oy);
                    let inside = mx >= 0
                        && my >= 0
                        && mask
                            .coverage
                            .get_pixel_checked(mx as u32, my as u32)
                            .is_some_and(|m| m.0[0] > 127);
                    inside != disc(x, y, cx, cy, r)
                })
                .count();
            assert!(wrong < 800, "{wrong} pixels wrong");
        }
    }

    /// A 6000 × 4000 photo: the contour of a disc is sharp and in place,
    /// a preview is computed at its own size, quickly, and the analysis
    /// gives its memory back. Needs the downloaded model.
    #[test]
    #[ignore = "needs the SAM model"]
    fn large_image_sharp_edges_and_quick_previews() {
        let (w, h) = (6000u32, 4000u32);
        let (cx, cy, r) = (2500.0f32, 2000.0f32, 1200.0f32);
        let inside = |x: u32, y: u32| (x as f32 - cx).powi(2) + (y as f32 - cy).powi(2) < r * r;
        let image = RgbaImage::from_fn(w, h, |x, y| {
            if inside(x, y) {
                image::Rgba([220, 40, 40, 255])
            } else {
                image::Rgba([(x / 24) as u8, 200, (y / 16) as u8, 255])
            }
        });
        let features = encode(&image).unwrap();
        drop(image);
        assert!(ENCODER.loaded());
        let prompt = Prompt {
            points: vec![(cx, cy, true)],
            rect: None,
        };
        let t = std::time::Instant::now();
        let preview = segment_preview(&features, &prompt, (1024, 683))
            .unwrap()
            .unwrap();
        eprintln!("preview in {:?}", t.elapsed());
        assert_eq!(preview.dimensions(), (1024, 683));
        let t = std::time::Instant::now();
        let mask = segment(&features, &prompt).unwrap().unwrap();
        eprintln!(
            "mask in {:?}, {:?} at {:?}",
            t.elapsed(),
            mask.coverage.dimensions(),
            mask.offset
        );
        let at = |x: u32, y: u32| {
            let (mx, my) = (x as i64 - mask.offset.0, y as i64 - mask.offset.1);
            if mx < 0 || my < 0 {
                return 0;
            }
            mask.coverage
                .get_pixel_checked(mx as u32, my as u32)
                .map_or(0, |p| p.0[0])
        };
        let soft = (0..w)
            .filter(|&x| (6..250).contains(&at(x, cy as u32)))
            .count();
        let wrong = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| (at(x, y) > 127) != inside(x, y))
            .count();
        eprintln!("soft pixels on the centre row: {soft}; wrong pixels: {wrong}");
        assert!(soft <= 8, "{soft} soft pixels on the centre row");
        // Perimeter: 7540 px; SAM itself is off by a pixel or two.
        assert!(wrong < 20_000, "{wrong} pixels wrong");
        Model::Objects.unload();
        assert!(!ENCODER.loaded() && !DECODER.loaded());
    }
}
