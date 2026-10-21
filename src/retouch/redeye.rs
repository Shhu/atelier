//! Red Eye tool: the red pupil next to a click is found (red pixels in one
//! piece) and turned to a dark neutral.

use image::RgbaImage;

use crate::pixel_ops::{Clip, mix};

/// How red a pixel is: 0 (not at all) to 1.
fn redness(p: [u8; 4]) -> f32 {
    let [r, g, b, a] = p.map(|v| v as f32);
    if a < 128.0 || r < 50.0 {
        return 0.0;
    }
    ((r - g.max(b)) / r).max(0.0)
}

/// Red eye at (x, y) within `radius` pixels: `pupil` (Pupil Size, 0..1)
/// takes in less red pixels, `darken` (Darken Amount, 0..1) darkens the
/// pupil. Only where `clip` allows. Returns the box changed (x0, y0, x1,
/// y1), `None` if there is no red there.
pub fn remove_red_eye(
    image: &mut RgbaImage,
    (x, y): (u32, u32),
    radius: u32,
    pupil: f32,
    darken: f32,
    clip: Option<&Clip>,
) -> Option<[u32; 4]> {
    let (w, h) = image.dimensions();
    if x >= w || y >= h {
        return None;
    }
    let [x0, y0] = [x.saturating_sub(radius), y.saturating_sub(radius)];
    let [x1, y1] = [(x + radius + 1).min(w), (y + radius + 1).min(h)];
    let (bw, bh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    let red: Vec<f32> = (y0..y1)
        .flat_map(|py| (x0..x1).map(move |px| (px, py)))
        .map(|(px, py)| redness(image.get_pixel(px, py).0))
        .collect();
    // The reddest pixel near the click starts the pupil.
    let near = (radius / 3).max(2);
    let seed = (0..bw * bh)
        .filter(|&i| {
            let (px, py) = (x0 + (i % bw) as u32, y0 + (i / bw) as u32);
            px.abs_diff(x) <= near && py.abs_diff(y) <= near
        })
        .max_by(|&a, &b| red[a].total_cmp(&red[b]))?;
    let threshold = 0.45 - 0.3 * pupil.clamp(0.0, 1.0);
    if red[seed] < threshold.max(0.2) {
        return None;
    }
    // Red pixels in one piece with it.
    let mut inside = vec![false; bw * bh];
    let mut stack = vec![seed];
    inside[seed] = true;
    while let Some(i) = stack.pop() {
        let (px, py) = (i % bw, i / bw);
        let around = [
            (px > 0).then(|| i - 1),
            (px + 1 < bw).then(|| i + 1),
            (py > 0).then(|| i - bw),
            (py + 1 < bh).then(|| i + bw),
        ];
        for n in around.into_iter().flatten() {
            if !inside[n] && red[n] >= threshold {
                inside[n] = true;
                stack.push(n);
            }
        }
    }
    // Its edge, a pixel wider, fades with the redness.
    let keep = 1.0 - 0.75 * darken.clamp(0.0, 1.0);
    let mut changed = [x1, y1, x0, y0];
    for i in 0..bw * bh {
        let (px, py) = (i % bw, i / bw);
        let edge = !inside[i]
            && [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)]
                .iter()
                .any(|&(dx, dy)| {
                    let (nx, ny) = (px as i32 + dx, py as i32 + dy);
                    nx >= 0
                        && ny >= 0
                        && (nx as usize) < bw
                        && (ny as usize) < bh
                        && inside[ny as usize * bw + nx as usize]
                });
        let weight = if inside[i] {
            1.0
        } else if edge {
            (red[i] / threshold).min(1.0)
        } else {
            0.0
        };
        let (ix, iy) = (x0 + px as u32, y0 + py as u32);
        let coverage = clip.map_or(255, |c| c.coverage(ix, iy));
        let amount = (weight * coverage as f32).round() as u8;
        if amount == 0 {
            continue;
        }
        let p = image.get_pixel_mut(ix, iy);
        let [_, g, b, a] = p.0.map(|v| v as f32);
        let neutral = (g + b) / 2.0;
        let dark = |v: f32| (v * keep).round() as u8;
        p.0 = mix(p.0, [dark(neutral), dark(g), dark(b), a as u8], amount);
        changed = [
            changed[0].min(ix),
            changed[1].min(iy),
            changed[2].max(ix + 1),
            changed[3].max(iy + 1),
        ];
    }
    (changed[0] < changed[2]).then_some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An eye: skin, a white, a red pupil. A click near the pupil makes it
    /// dark and neutral, and leaves the rest alone.
    #[test]
    fn red_pupil_turns_dark() {
        let skin = [224, 172, 140, 255];
        let mut image = RgbaImage::from_fn(80, 60, |x, y| {
            let d = ((x as f32 - 40.0).powi(2) + (y as f32 - 30.0).powi(2)).sqrt();
            image::Rgba(if d < 7.0 {
                [200, 40, 45, 255]
            } else if d < 14.0 {
                [235, 235, 230, 255]
            } else {
                skin
            })
        });
        let before = image.clone();
        let changed = remove_red_eye(&mut image, (42, 31), 20, 0.5, 0.5, None).unwrap();
        assert!(changed[0] >= 32 && changed[2] <= 48, "{changed:?}");
        for (x, y, p) in image.enumerate_pixels() {
            let d = ((x as f32 - 40.0).powi(2) + (y as f32 - 30.0).powi(2)).sqrt();
            if d < 6.0 {
                assert!(redness(p.0) < 0.05 && p.0[0] < 60, "({x}, {y}): {:?}", p.0);
            } else if d > 8.0 {
                assert_eq!(p, before.get_pixel(x, y), "({x}, {y})");
            }
        }
        // Nothing red there: nothing changes.
        assert!(remove_red_eye(&mut image, (5, 5), 20, 0.5, 0.5, None).is_none());
    }
}
