//! Retouching: the healing blend (Healing Brush, Spot Healing Brush, Patch
//! tool), patch-based inpainting (Content-Aware Fill) and red eye removal.
//! Pixels are worked on premultiplied, in 0..255 floats.

pub mod heal;
pub mod patchmatch;
pub mod redeye;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use image::RgbaImage;

use crate::paint::DirtyRect;

/// Premultiplied RGBA, 0..255.
pub type Px = [f32; 4];

/// Progress of a long computation (0..1), shared with the UI, which may
/// cancel it.
#[derive(Default)]
pub struct Progress {
    /// Thousandths done.
    done: AtomicU32,
    cancel: AtomicBool,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        self.done.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn set(&self, fraction: f32) {
        let v = (fraction.clamp(0.0, 1.0) * 1000.0) as u32;
        self.done.store(v, Ordering::Relaxed);
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub fn premultiply(p: [u8; 4]) -> Px {
    let a = p[3] as f32 / 255.0;
    [
        p[0] as f32 * a,
        p[1] as f32 * a,
        p[2] as f32 * a,
        p[3] as f32,
    ]
}

/// Back to 8-bit straight colors, clamped to what is valid.
pub fn unpremultiply(p: Px) -> [u8; 4] {
    let a = p[3].clamp(0.0, 255.0);
    if a < 0.5 {
        return [0; 4];
    }
    let c = |v: f32| (v.clamp(0.0, a) * 255.0 / a).round() as u8;
    [c(p[0]), c(p[1]), c(p[2]), a.round() as u8]
}

/// `top` over `under` (premultiplied).
fn over(top: Px, under: Px) -> Px {
    let k = 1.0 - top[3] / 255.0;
    std::array::from_fn(|c| top[c] + under[c] * k)
}

/// The pixels of `image` in `[x0, y0, x1, y1]`, premultiplied, row by row.
pub fn region_px(image: &RgbaImage, [x0, y0, x1, y1]: [u32; 4]) -> Vec<Px> {
    let mut out = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
    for y in y0..y1 {
        for x in x0..x1 {
            out.push(premultiply(image.get_pixel(x, y).0));
        }
    }
    out
}

/// Source of the Clone Stamp and the Healing Brush: the point set with
/// Alt + click (canvas coordinates), and with Aligned, the offset that the
/// first stroke after it fixed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloneSource {
    pub point: (f32, f32),
    offset: Option<(i64, i64)>,
}

impl CloneSource {
    pub fn new(point: (f32, f32)) -> Self {
        Self {
            point,
            offset: None,
        }
    }

    /// Offset from the pixels of a stroke starting at `start` to those it
    /// copies. Aligned: the same for every stroke, the source moving with
    /// the brush; otherwise each stroke starts again from the source point.
    pub fn stroke_offset(&mut self, start: (f32, f32), aligned: bool) -> (i64, i64) {
        let fresh = self.offset_at(start);
        if aligned {
            *self.offset.get_or_insert(fresh)
        } else {
            fresh
        }
    }

    /// The offset a stroke starting at `at` would take (where the source
    /// shows under the brush before painting).
    pub fn preview_offset(&self, at: (f32, f32), aligned: bool) -> (i64, i64) {
        match self.offset {
            Some(offset) if aligned => offset,
            _ => self.offset_at(at),
        }
    }

    fn offset_at(&self, (x, y): (f32, f32)) -> (i64, i64) {
        (
            (self.point.0 - x).round() as i64,
            (self.point.1 - y).round() as i64,
        )
    }
}

/// The healing blend over area `r` of `image` (being healed from `base`):
/// where `coverage` (0..1) is not zero, the source pixels (`sample` at
/// `offset`) take on the colors of what surrounds them, and are laid over
/// `base` by the coverage. The healed pixels on the edge of `r` (if inside
/// the image) keep their current value and the area is healed to meet
/// them: a part of a longer stroke. With `seen` (what was visible there
/// before the stroke, the layer included, the same size as it), the colors
/// met are those seen: an empty layer is healed opaque, like Photoshop's
/// Current & Below.
pub fn heal_area(
    image: &mut RgbaImage,
    base: &RgbaImage,
    coverage: impl Fn(u32, u32) -> f32,
    (sample, offset): (&RgbaImage, (i64, i64)),
    r: DirtyRect,
    seen: Option<&RgbaImage>,
) {
    let (iw, ih) = image.dimensions();
    let (w, h) = (r.w as usize, r.h as usize);
    let source_at = |x: u32, y: u32| {
        let (sx, sy) = (x as i64 + offset.0, y as i64 + offset.1);
        let inside =
            sx >= 0 && sy >= 0 && sx < sample.width() as i64 && sy < sample.height() as i64;
        let (cx, cy) = (
            sx.clamp(0, sample.width() as i64 - 1) as u32,
            sy.clamp(0, sample.height() as i64 - 1) as u32,
        );
        (premultiply(sample.get_pixel(cx, cy).0), inside)
    };
    let mut dest = Vec::with_capacity(w * h);
    let mut src = Vec::with_capacity(w * h);
    let mut free = Vec::with_capacity(w * h);
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let covered = coverage(x, y) > 0.0;
            let (s, inside) = source_at(x, y);
            // On an edge of the area that is not the image's: kept.
            let edge = (x == r.x && x > 0)
                || (y == r.y && y > 0)
                || (x + 1 == r.x + r.w && x + 1 < iw)
                || (y + 1 == r.y + r.h && y + 1 < ih);
            let healed = covered && inside;
            let kept = healed && edge;
            let layer = premultiply(if kept {
                image.get_pixel(x, y).0
            } else {
                base.get_pixel(x, y).0
            });
            dest.push(match seen {
                // What is healed already, over what was seen.
                Some(v) if kept => over(layer, premultiply(v.get_pixel(x, y).0)),
                Some(v) => premultiply(v.get_pixel(x, y).0),
                None => layer,
            });
            src.push(s);
            free.push(healed && !edge);
        }
    }
    let out = heal::heal(&dest, &src, &free, w, h);
    for (i, healed) in out.iter().enumerate() {
        if !free[i] {
            continue;
        }
        let (x, y) = (r.x + (i % w) as u32, r.y + (i / w) as u32);
        let k = coverage(x, y);
        let b = premultiply(base.get_pixel(x, y).0);
        let cover = k * healed[3].clamp(0.0, 255.0) / 255.0;
        let px: Px = std::array::from_fn(|c| healed[c] * k + b[c] * (1.0 - cover));
        image.get_pixel_mut(x, y).0 = unpremultiply(px);
    }
}

/// A fill laid over the layer: `filled` (a region of it, its hole painted
/// in) over `base` (the same region as it is) by `coverage` (0..1, row by
/// row) times `opacity`. With `adapt` (Color Adaptation), the fill first
/// takes on the colors around the hole (healing blend): those of `seen`
/// if given (what is visible there, when sampled from all layers: an
/// empty layer then gets opaque pixels), otherwise the layer's.
/// `keep_alpha`: Preserve Transparency.
pub fn blend_fill(
    base: &RgbaImage,
    filled: &RgbaImage,
    coverage: &[f32],
    adapt: bool,
    opacity: f32,
    keep_alpha: bool,
    seen: Option<&RgbaImage>,
) -> RgbaImage {
    let (w, h) = base.dimensions();
    let inside: Vec<bool> = coverage.iter().map(|&c| c > 0.0).collect();
    let dest = region_px(base, [0, 0, w, h]);
    let src = region_px(filled, [0, 0, w, h]);
    let result = if adapt {
        let target = seen.map(|s| region_px(s, [0, 0, w, h]));
        heal::heal(
            target.as_ref().unwrap_or(&dest),
            &src,
            &inside,
            w as usize,
            h as usize,
        )
    } else {
        src
    };
    let mut out = base.clone();
    for (i, p) in out.pixels_mut().enumerate() {
        if !inside[i] {
            continue;
        }
        let k = coverage[i].min(1.0) * opacity;
        let (r, b) = (result[i], dest[i]);
        let cover = k * r[3].clamp(0.0, 255.0) / 255.0;
        let mut px = unpremultiply(std::array::from_fn(|c| r[c] * k + b[c] * (1.0 - cover)));
        if keep_alpha {
            px[3] = p.0[3];
        }
        p.0 = px;
    }
    out
}

/// Proximity Match: the hole filled with the pixels beside it, from the
/// side whose surroundings match those of the hole best. `None` if no side
/// has room.
pub fn proximity_fill(image: &RgbaImage, hole: &[bool]) -> Option<RgbaImage> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let at = |x: i64, y: i64| (y * w + x) as usize;
    let mut b = [w, h, 0, 0];
    for (i, _) in hole.iter().enumerate().filter(|(_, h)| **h) {
        let (x, y) = (i as i64 % w, i as i64 / w);
        b = [b[0].min(x), b[1].min(y), b[2].max(x + 1), b[3].max(y + 1)];
    }
    if b[0] >= b[2] {
        return Some(image.clone());
    }
    let (dx, dy) = (b[2] - b[0] + 2, b[3] - b[1] + 2);
    // The ring of pixels around the hole, three wide.
    const RING: i64 = 3;
    let mut ring = Vec::new();
    for y in (b[1] - RING).max(0)..(b[3] + RING).min(h) {
        for x in (b[0] - RING).max(0)..(b[2] + RING).min(w) {
            let near = !hole[at(x, y)]
                && ((y - RING).max(0)..(y + RING + 1).min(h)).any(|ny| {
                    ((x - RING).max(0)..(x + RING + 1).min(w)).any(|nx| hole[at(nx, ny)])
                });
            if near {
                ring.push((x, y));
            }
        }
    }
    let px = |x: i64, y: i64| image.get_pixel(x as u32, y as u32).0;
    let usable = |x: i64, y: i64| x >= 0 && y >= 0 && x < w && y < h && !hole[at(x, y)];
    let mut best: Option<(f64, (i64, i64))> = None;
    for (ox, oy) in [
        (-dx, 0),
        (dx, 0),
        (0, -dy),
        (0, dy),
        (-dx, -dy),
        (dx, -dy),
        (-dx, dy),
        (dx, dy),
    ] {
        let fits = hole
            .iter()
            .enumerate()
            .filter(|(_, h)| **h)
            .all(|(i, _)| usable(i as i64 % w + ox, i as i64 / w + oy));
        if !fits {
            continue;
        }
        let (mut sum, mut n) = (0.0, 0.0);
        for &(x, y) in &ring {
            if usable(x + ox, y + oy) {
                let (p, q) = (px(x, y), px(x + ox, y + oy));
                sum += (0..4)
                    .map(|c| (p[c] as f64 - q[c] as f64).powi(2))
                    .sum::<f64>();
                n += 1.0;
            }
        }
        let score = if n > 0.0 { sum / n } else { f64::MAX };
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, (ox, oy)));
        }
    }
    let (_, (ox, oy)) = best?;
    let mut out = image.clone();
    for (i, _) in hole.iter().enumerate().filter(|(_, h)| **h) {
        let (x, y) = (i as i64 % w, i as i64 / w);
        out.put_pixel(x as u32, y as u32, image::Rgba(px(x + ox, y + oy)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Aligned: every stroke keeps the offset of the first one after the
    /// source was set. Not aligned: each stroke starts from the source.
    #[test]
    fn clone_alignment() {
        let mut source = CloneSource::new((10.0, 20.0));
        assert_eq!(source.preview_offset((50.0, 20.0), true), (-40, 0));
        assert_eq!(source.stroke_offset((50.0, 20.0), true), (-40, 0));
        assert_eq!(source.stroke_offset((80.0, 60.0), true), (-40, 0));
        assert_eq!(source.preview_offset((0.0, 0.0), true), (-40, 0));
        assert_eq!(source.stroke_offset((80.0, 60.0), false), (-70, -40));
        assert_eq!(source.preview_offset((80.0, 60.0), false), (-70, -40));
        // A new source point: a new offset.
        let mut source = CloneSource::new((100.0, 100.0));
        assert_eq!(source.stroke_offset((80.0, 60.0), true), (20, 40));
    }

    /// The Healing Brush over a smooth area, from a textured source of
    /// other colors: the texture comes, the colors stay.
    #[test]
    fn healing_keeps_the_destination_colors() {
        let (w, h) = (120u32, 80u32);
        let base = RgbaImage::from_fn(w, h, |x, _| {
            if x < 60 {
                image::Rgba([40 + (x % 7 * 20) as u8, 200, 60, 255])
            } else {
                image::Rgba([120, 120, 200, 255])
            }
        });
        let mut image = base.clone();
        // A disc of the stroke on the right half, copying the left half.
        let coverage: Vec<u16> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                if (x - 90.0).powi(2) + (y - 40.0).powi(2) < 15.0f32.powi(2) {
                    65535
                } else {
                    0
                }
            })
            .collect();
        let r = DirtyRect {
            x: 70,
            y: 20,
            w: 41,
            h: 41,
        };
        let cover = |x: u32, y: u32| coverage[(y * w + x) as usize] as f32 / 65535.0;
        heal_area(&mut image, &base, cover, (&base, (-60, 0)), r, None);
        let center = image.get_pixel(90, 40).0;
        // Blue around: blue it stays, not the source's green.
        assert!(center[2] > 150 && center[1] < 160, "{center:?}");
        // The texture (columns of 7) shows.
        let row: Vec<i32> = (84..97)
            .map(|x| image.get_pixel(x, 40).0[0] as i32)
            .collect();
        let spread = row.iter().max().unwrap() - row.iter().min().unwrap();
        assert!(spread > 60, "{row:?}");
        // Outside the stroke: untouched.
        assert_eq!(image.get_pixel(72, 22), base.get_pixel(72, 22));
    }

    /// Proximity Match fills a hole in horizontal stripes from beside it,
    /// in phase; with Color Adaptation the fill keeps them.
    #[test]
    fn proximity_match_takes_the_matching_side() {
        let (w, h) = (100u32, 60u32);
        let image = RgbaImage::from_fn(w, h, |x, y| {
            let v = if y % 6 < 3 { 30 } else { 220 };
            image::Rgba([v, v, (x * 2) as u8, 255])
        });
        let hole: Vec<bool> = (0..w * h)
            .map(|i| (40..52).contains(&(i % w)) && (20..33).contains(&(i / w)))
            .collect();
        let mut damaged = image.clone();
        for (p, &h) in damaged.pixels_mut().zip(&hole) {
            if h {
                p.0 = [255, 0, 0, 255];
            }
        }
        let filled = proximity_fill(&damaged, &hole).unwrap();
        let coverage: Vec<f32> = hole.iter().map(|&h| f32::from(u8::from(h))).collect();
        let out = blend_fill(&damaged, &filled, &coverage, true, 1.0, false, None);
        for (i, &h) in hole.iter().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            let (p, q) = (out.get_pixel(x, y).0, image.get_pixel(x, y).0);
            if h {
                // The stripes in phase (the blue ramp comes from the side).
                assert!(p[0].abs_diff(q[0]) < 12, "({x}, {y}): {p:?} for {q:?}");
            } else {
                assert_eq!(p, damaged.get_pixel(x, y).0);
            }
        }
    }
}
