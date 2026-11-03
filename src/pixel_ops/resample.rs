//! Resampling in premultiplied alpha, in parallel over bands of rows and
//! cancellable. The arithmetic is that of `image::imageops::resize` (a
//! vertical then a horizontal separable pass, f32 in between, the same
//! weights in the same order), so the results are identical to it.

use std::sync::atomic::{AtomicBool, Ordering};

use image::RgbaImage;
use image::imageops::FilterType;
use rayon::prelude::*;

/// The source pixels one output pixel reads along an axis: the first one and
/// their weights.
struct Taps {
    left: u32,
    weights: Vec<f32>,
}

fn sinc(t: f32) -> f32 {
    let a = t * std::f32::consts::PI;
    if t == 0.0 { 1.0 } else { a.sin() / a }
}

fn lanczos3(x: f32) -> f32 {
    let t = 3.0;
    if x.abs() < t { sinc(x) * sinc(x / t) } else { 0.0 }
}

fn bc_cubic_spline(x: f32, b: f32, c: f32) -> f32 {
    let a = x.abs();
    let k = if a < 1.0 {
        (12.0 - 9.0 * b - 6.0 * c) * a.powi(3)
            + (-18.0 + 12.0 * b + 6.0 * c) * a.powi(2)
            + (6.0 - 2.0 * b)
    } else if a < 2.0 {
        (-b - 6.0 * c) * a.powi(3)
            + (6.0 * b + 30.0 * c) * a.powi(2)
            + (-12.0 * b - 48.0 * c) * a
            + (8.0 * b + 24.0 * c)
    } else {
        0.0
    };
    k / 6.0
}

fn catmull_rom(x: f32) -> f32 {
    bc_cubic_spline(x, 0.0, 0.5)
}

fn gaussian(x: f32) -> f32 {
    let r: f32 = 0.5;
    ((2.0 * std::f32::consts::PI).sqrt() * r).recip() * (-x.powi(2) / (2.0 * r.powi(2))).exp()
}

fn triangle(x: f32) -> f32 {
    if x.abs() < 1.0 { 1.0 - x.abs() } else { 0.0 }
}

fn unit(_: f32) -> f32 {
    1.0
}

/// The kernel of `filter` and its support.
fn kernel(filter: FilterType) -> (fn(f32) -> f32, f32) {
    match filter {
        FilterType::Nearest => (unit, 0.0),
        FilterType::Triangle => (triangle, 1.0),
        FilterType::CatmullRom => (catmull_rom, 2.0),
        FilterType::Gaussian => (gaussian, 3.0),
        FilterType::Lanczos3 => (lanczos3, 3.0),
    }
}

/// The taps of each of the `dst` output pixels along an axis of `src` pixels.
fn taps(src: u32, dst: u32, filter: FilterType) -> Vec<Taps> {
    let (kernel, support) = kernel(filter);
    let ratio = src as f32 / dst as f32;
    let sratio = if ratio < 1.0 { 1.0 } else { ratio };
    let src_support = support * sratio;
    (0..dst)
        .map(|out| {
            let input = (out as f32 + 0.5) * ratio;
            let left = ((input - src_support).floor() as i64).clamp(0, i64::from(src) - 1);
            let right = ((input + src_support).ceil() as i64).clamp(left + 1, i64::from(src));
            let input = input - 0.5;
            let mut weights: Vec<f32> =
                (left..right).map(|i| kernel((i as f32 - input) / sratio)).collect();
            let sum: f32 = weights.iter().fold(0.0, |s, w| s + w);
            weights.iter_mut().for_each(|w| *w /= sum);
            Taps { left: left as u32, weights }
        })
        .collect()
}

/// A premultiplied 16-bit pixel (color × alpha, alpha × 257), as floats.
fn premultiplied(p: &[u8]) -> [f32; 4] {
    let a = u16::from(p[3]);
    [
        f32::from(u16::from(p[0]) * a),
        f32::from(u16::from(p[1]) * a),
        f32::from(u16::from(p[2]) * a),
        f32::from(a * 257),
    ]
}

/// Back from premultiplied 16 bits; an alpha below half a level is clear.
fn straight(p: [u16; 4]) -> [u8; 4] {
    let [r, g, b, a] = p.map(u32::from);
    if a < 129 {
        return [0, 0, 0, 0];
    }
    let c = |v: u32| ((v * 257 + a / 2) / a).min(255) as u8;
    [c(r), c(g), c(b), ((a + 128) / 257) as u8]
}

/// [`super::resize`], or `None` once `cancel` is set (checked on every row).
pub fn resize_unless(
    image: &RgbaImage,
    w: u32,
    h: u32,
    filter: FilterType,
    cancel: &AtomicBool,
) -> Option<RgbaImage> {
    let (sw, sh) = image.dimensions();
    if sw == 0 || sh == 0 || w == 0 || h == 0 {
        return Some(RgbaImage::new(w, h));
    }
    if (w, h) == (sw, sh) {
        // A copy, through the same conversions (only a clear pixel changes).
        let mut out = image.clone();
        for p in out.pixels_mut().filter(|p| p.0[3] == 0) {
            p.0 = [0; 4];
        }
        return Some(out);
    }
    let rows = taps(sh, h, filter);
    let columns = taps(sw, w, filter);
    let src = image.as_raw();
    let src_row = sw as usize * 4;
    let mut out = RgbaImage::new(w, h);
    let out_row = w as usize * 4;
    // Bands of about 64k output pixels: enough work each to share it out.
    let band = (65536 / w as usize).clamp(1, h as usize);
    out.par_chunks_mut(band * out_row).enumerate().for_each(|(b, chunk)| {
        let mut column = vec![[0f32; 4]; sw as usize];
        for (r, row) in chunk.chunks_exact_mut(out_row).enumerate() {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            // Vertical pass: this output row from the source rows it reads.
            let taps = &rows[b * band + r];
            column.iter_mut().for_each(|t| *t = [0.0; 4]);
            for (i, &wt) in taps.weights.iter().enumerate() {
                let y = (taps.left as usize + i) * src_row;
                let line = &src[y..y + src_row];
                for (t, p) in column.iter_mut().zip(line.chunks_exact(4)) {
                    let v = premultiplied(p);
                    for c in 0..4 {
                        t[c] += v[c] * wt;
                    }
                }
            }
            // Horizontal pass, then back to 8 bits.
            for (taps, px) in columns.iter().zip(row.chunks_exact_mut(4)) {
                let mut t = [0f32; 4];
                let from = &column[taps.left as usize..];
                for (v, &wt) in from.iter().zip(&taps.weights) {
                    for c in 0..4 {
                        t[c] += v[c] * wt;
                    }
                }
                let q = t.map(|v| v.clamp(0.0, 65535.0).round() as u16);
                px.copy_from_slice(&straight(q));
            }
        }
    });
    (!cancel.load(Ordering::Relaxed)).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The former implementation: premultiplied 16 bits through
    /// `image::imageops::resize`.
    fn reference(image: &RgbaImage, w: u32, h: u32, filter: FilterType) -> RgbaImage {
        let premultiplied = image::ImageBuffer::<image::Rgba<u16>, Vec<u16>>::from_fn(
            image.width(),
            image.height(),
            |x, y| {
                let [r, g, b, a] = image.get_pixel(x, y).0.map(u16::from);
                image::Rgba([r * a, g * a, b * a, a * 257])
            },
        );
        let resized = image::imageops::resize(&premultiplied, w, h, filter);
        RgbaImage::from_fn(w, h, |x, y| image::Rgba(straight(resized.get_pixel(x, y).0)))
    }

    fn noise(w: u32, h: u32, seed: u32) -> RgbaImage {
        let mut s = seed.wrapping_mul(2654435761) | 1;
        RgbaImage::from_fn(w, h, |_, _| {
            let mut next = || {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s as u8
            };
            // Some clear and some opaque pixels among the partial ones.
            let a = match next() % 4 {
                0 => 0,
                1 => 255,
                _ => next(),
            };
            image::Rgba([next(), next(), next(), a])
        })
    }

    #[test]
    fn identical_to_the_former_resize() {
        let filters = [
            FilterType::Nearest,
            FilterType::Triangle,
            FilterType::CatmullRom,
            FilterType::Gaussian,
            FilterType::Lanczos3,
        ];
        let sizes = [
            (97, 61, 250, 30),
            (300, 200, 41, 333),
            (5, 7, 5, 19),
            (64, 64, 64, 64),
            (1, 1, 3, 2),
            (120, 1, 7, 9),
        ];
        for (n, &(sw, sh, w, h)) in sizes.iter().enumerate() {
            let src = noise(sw, sh, n as u32 + 1);
            for filter in filters {
                let ours = resize_unless(&src, w, h, filter, &AtomicBool::new(false)).unwrap();
                let theirs = reference(&src, w, h, filter);
                assert!(ours == theirs, "{sw}×{sh} → {w}×{h}, {filter:?}");
            }
        }
    }

    #[test]
    fn a_set_cancel_stops_it() {
        let src = noise(400, 300, 9);
        let cancel = AtomicBool::new(true);
        assert!(resize_unless(&src, 800, 600, FilterType::Lanczos3, &cancel).is_none());
    }

    /// Release timing of one 6000×4000 layer to 9000×6000 (run by hand:
    /// `cargo test --release -- --ignored resample_timing --nocapture`).
    #[test]
    #[ignore]
    fn resample_timing() {
        let src = noise(6000, 4000, 3);
        for filter in [FilterType::Triangle, FilterType::CatmullRom, FilterType::Lanczos3] {
            for (w, h) in [(9000, 6000), (3000, 2000)] {
                let t = std::time::Instant::now();
                let theirs = reference(&src, w, h, filter);
                let before = t.elapsed();
                let t = std::time::Instant::now();
                let ours = resize_unless(&src, w, h, filter, &AtomicBool::new(false)).unwrap();
                let after = t.elapsed();
                assert!(ours == theirs);
                println!("{filter:?} → {w}×{h}: before {before:?}, after {after:?}");
            }
        }
        // Cancelled 2 ms in: how long until it gives up.
        let cancel = AtomicBool::new(false);
        let t = std::time::Instant::now();
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(2));
                cancel.store(true, Ordering::Relaxed);
            });
            assert!(resize_unless(&src, 9000, 6000, FilterType::Lanczos3, &cancel).is_none());
        });
        println!("cancelled after {:?}", t.elapsed());
    }
}
