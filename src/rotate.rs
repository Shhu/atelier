//! Rotating an image around its center.

use image::{Rgba, RgbaImage, imageops};

/// Size of the bounding box of a `w × h` rectangle rotated by `degrees`.
pub fn rotated_size(w: f32, h: f32, degrees: f32) -> (f32, f32) {
    let (s, c) = degrees.to_radians().sin_cos();
    (w * c.abs() + h * s.abs(), w * s.abs() + h * c.abs())
}

/// Rotates `image` by `degrees` (clockwise on screen) around its center.
/// The resulting image contains all the rotated content; the added corners are
/// transparent. Quarter turns are exact (no resampling);
/// otherwise it interpolates, nearest neighbor if `nearest`.
pub fn rotate(image: &RgbaImage, degrees: f32, nearest: bool) -> RgbaImage {
    let quarter = degrees / 90.0;
    if (quarter - quarter.round()).abs() < 1e-4 {
        return match (quarter.round() as i64).rem_euclid(4) {
            0 => image.clone(),
            1 => imageops::rotate90(image),
            2 => imageops::rotate180(image),
            _ => imageops::rotate270(image),
        };
    }

    let (w, h) = (image.width() as f32, image.height() as f32);
    let (rw, rh) = rotated_size(w, h, degrees);
    let (out_w, out_h) = (rw.ceil().max(1.0) as u32, rh.ceil().max(1.0) as u32);
    let (s, c) = degrees.to_radians().sin_cos();
    let (src_cx, src_cy) = (w / 2.0, h / 2.0);
    let (dst_cx, dst_cy) = (out_w as f32 / 2.0, out_h as f32 / 2.0);

    RgbaImage::from_fn(out_w, out_h, |x, y| {
        // Inverse rotation from the destination pixel center to the source.
        let (dx, dy) = (x as f32 + 0.5 - dst_cx, y as f32 + 0.5 - dst_cy);
        let sx = c * dx + s * dy + src_cx;
        let sy = -s * dx + c * dy + src_cy;
        if nearest {
            sample_nearest(image, sx, sy)
        } else {
            sample_bilinear(image, sx, sy)
        }
    })
}

fn sample_nearest(image: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (ix, iy) = (x.floor(), y.floor());
    if ix < 0.0 || iy < 0.0 || ix >= image.width() as f32 || iy >= image.height() as f32 {
        return Rgba([0, 0, 0, 0]);
    }
    *image.get_pixel(ix as u32, iy as u32)
}

/// Bilinear interpolation in premultiplied alpha (avoids dark fringes
/// at the edge of transparent areas). Outside the image: transparent.
fn sample_bilinear(image: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let (x, y) = (x - 0.5, y - 0.5);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (w, h) = (image.width() as i64, image.height() as i64);

    let mut acc = [0.0f32; 4];
    for (ox, oy, weight) in [
        (0, 0, (1.0 - fx) * (1.0 - fy)),
        (1, 0, fx * (1.0 - fy)),
        (0, 1, (1.0 - fx) * fy),
        (1, 1, fx * fy),
    ] {
        let (px, py) = (x0 as i64 + ox, y0 as i64 + oy);
        if weight <= 0.0 || px < 0 || py < 0 || px >= w || py >= h {
            continue;
        }
        let p = image.get_pixel(px as u32, py as u32).0;
        let a = p[3] as f32 / 255.0 * weight;
        acc[0] += p[0] as f32 * a;
        acc[1] += p[1] as f32 * a;
        acc[2] += p[2] as f32 * a;
        acc[3] += a;
    }
    if acc[3] <= 1e-6 {
        return Rgba([0, 0, 0, 0]);
    }
    let un = |v: f32| (v / acc[3]).round().clamp(0.0, 255.0) as u8;
    Rgba([
        un(acc[0]),
        un(acc[1]),
        un(acc[2]),
        (acc[3] * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

    #[test]
    fn quarter_turn_is_exact_and_clockwise() {
        // Red pixel at the top left of a 3 × 2 image.
        let mut img = RgbaImage::new(3, 2);
        img.put_pixel(0, 0, RED);
        let r = rotate(&img, 90.0, false);
        assert_eq!(r.dimensions(), (2, 3));
        // Clockwise: the top-left corner goes to the top right.
        assert_eq!(*r.get_pixel(1, 0), RED);
        assert_eq!(rotate(&img, -270.0, false), r);
        assert_eq!(rotate(&img, 360.0, false), img);
    }

    #[test]
    fn rotation_grows_canvas_to_fit() {
        let img = RgbaImage::from_pixel(10, 10, RED);
        let r = rotate(&img, 45.0, false);
        // Diagonal of 10: ≈ 14.14 → 15 px.
        assert_eq!(r.dimensions(), (15, 15));
        // Solid center, transparent corners.
        assert_eq!(*r.get_pixel(7, 7), RED);
        assert_eq!(r.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn bilinear_keeps_color_at_transparent_edges() {
        let img = RgbaImage::from_pixel(10, 10, RED);
        let r = rotate(&img, 30.0, false);
        // Partially covered pixels keep the pure red color.
        for p in r.pixels().filter(|p| p.0[3] > 0) {
            assert_eq!(&p.0[..3], &[255, 0, 0]);
        }
    }

    #[test]
    fn rotated_size_of_square_at_45() {
        let (w, h) = rotated_size(10.0, 10.0, 45.0);
        assert!((w - 14.142).abs() < 0.01 && (h - 14.142).abs() < 0.01);
    }
}
