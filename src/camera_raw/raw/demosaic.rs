//! Demosaicking of a Bayer mosaic: Malvar, He and Cutler's gradient-
//! corrected bilinear interpolation (5 × 5 kernels), sharp and free of the
//! zipper artifacts of plain bilinear interpolation.

use crate::filters::par_rows;

/// RGB of every pixel of `mosaic` (w × h, one value per pixel), whose
/// colors repeat `pattern` (0 red, 1 green, 2 blue; rows of two).
pub fn malvar(mosaic: &[f32], w: usize, h: usize, pattern: [u8; 4]) -> Vec<[f32; 3]> {
    let color = |x: usize, y: usize| pattern[(y % 2) * 2 + x % 2];
    // Mirrored beyond the edges, keeping the colors' parity.
    let reflect = |i: isize, n: usize| -> usize {
        let n = n as isize;
        let i = if i < 0 {
            -i
        } else if i >= n {
            2 * n - 2 - i
        } else {
            i
        };
        i.clamp(0, n - 1) as usize
    };
    let mut out = vec![[0.0f32; 3]; w * h];
    par_rows(&mut out, w.max(1), |y0, band| {
        for (i, p) in band.iter_mut().enumerate() {
            let (x, y) = (i % w, y0 + i / w);
            let at = |dx: isize, dy: isize| {
                let sx = reflect(x as isize + dx, w);
                let sy = reflect(y as isize + dy, h);
                mosaic[sy * w + sx]
            };
            let c = at(0, 0);
            let cross = at(-1, 0) + at(1, 0) + at(0, -1) + at(0, 1);
            let far = at(-2, 0) + at(2, 0) + at(0, -2) + at(0, 2);
            let diagonal = at(-1, -1) + at(1, -1) + at(-1, 1) + at(1, 1);
            let (row_far, col_far) = (at(-2, 0) + at(2, 0), at(0, -2) + at(0, 2));
            // Green at a red or blue site.
            let green = (4.0 * c + 2.0 * cross - far) / 8.0;
            // The other of red and blue at a red or blue site.
            let opposite = (6.0 * c + 2.0 * diagonal - 1.5 * far) / 8.0;
            // Red or blue at a green site, from the row's or the column's
            // neighbors.
            let from_row =
                (5.0 * c + 4.0 * (at(-1, 0) + at(1, 0)) - diagonal - row_far + 0.5 * col_far) / 8.0;
            let from_col =
                (5.0 * c + 4.0 * (at(0, -1) + at(0, 1)) - diagonal - col_far + 0.5 * row_far) / 8.0;
            let rgb = match color(x, y) {
                0 => [c, green, opposite],
                2 => [opposite, green, c],
                _ => {
                    if color(x + 1, y) == 0 {
                        [from_row, c, from_col]
                    } else {
                        [from_col, c, from_row]
                    }
                }
            };
            *p = rgb.map(|v| v.max(0.0));
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mosaic a camera would record of `image`.
    fn mosaic_of(image: &[[f32; 3]], w: usize, pattern: [u8; 4]) -> Vec<f32> {
        image
            .iter()
            .enumerate()
            .map(|(i, p)| p[pattern[((i / w) % 2) * 2 + (i % w) % 2] as usize])
            .collect()
    }

    #[test]
    fn flat_colors_and_smooth_gradients_come_back() {
        let (w, h) = (24, 18);
        for pattern in [[0, 1, 1, 2], [2, 1, 1, 0], [1, 0, 2, 1], [1, 2, 0, 1]] {
            let flat = vec![[0.6, 0.3, 0.1]; w * h];
            for p in malvar(&mosaic_of(&flat, w, pattern), w, h, pattern) {
                for c in 0..3 {
                    assert!((p[c] - flat[0][c]).abs() < 1e-5, "{pattern:?} {p:?}");
                }
            }
            // A linear ramp in each channel: exact away from the edges.
            let ramp: Vec<[f32; 3]> = (0..w * h)
                .map(|i| {
                    let (x, y) = ((i % w) as f32, (i / w) as f32);
                    [0.02 * x, 0.01 * y + 0.1, 0.5 - 0.01 * x]
                })
                .collect();
            let out = malvar(&mosaic_of(&ramp, w, pattern), w, h, pattern);
            for y in 3..h - 3 {
                for x in 3..w - 3 {
                    let (a, b) = (out[y * w + x], ramp[y * w + x]);
                    for c in 0..3 {
                        assert!(
                            (a[c] - b[c]).abs() < 1e-4,
                            "{pattern:?} at {x},{y}: {a:?} vs {b:?}"
                        );
                    }
                }
            }
        }
    }
}
