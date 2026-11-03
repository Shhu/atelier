//! Patterns of Fill…, Pattern Fill layers and Pattern Overlay: a tile
//! repeated over the canvas, at a scale and angle, from an origin.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use image::RgbaImage;

/// A tile: named pixels, identified by their content. Shared, never changed.
pub struct Pattern {
    /// Hash of the pixels: the same tile has the same one everywhere
    /// (library, documents, files).
    pub id: u64,
    pub name: String,
    pub image: Arc<RgbaImage>,
    /// Smaller versions, half the size each, computed when first needed.
    mips: Arc<OnceLock<Vec<RgbaImage>>>,
    /// Its mirror images (horizontal, vertical) while some document holds
    /// them: layers sharing a tile share its mirror, and a flip back finds
    /// the tile it came from.
    mirrors: Arc<Mutex<[Weak<Pattern>; 2]>>,
}

impl std::fmt::Debug for Pattern {
    // Short: the display fingerprints the layers by their debug text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pattern({:x}, {:?})", self.id, self.name)
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.name == other.name
    }
}

/// Hash of the size and pixels (the 64-bit lane of MurmurHash3): every bit
/// of every word reaches every bit of the result.
fn hash_pixels(image: &RgbaImage) -> u64 {
    const C1: u64 = 0x87c3_7b91_1142_53d5;
    const C2: u64 = 0x4cf5_ad43_2745_937f;
    let mix = |h: u64, v: u64| {
        let k = v.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
        (h ^ k)
            .rotate_left(27)
            .wrapping_mul(5)
            .wrapping_add(0x52dc_e729)
    };
    let mut h = 0xcbf2_9ce4_8422_2325;
    h = mix(h, image.width() as u64);
    h = mix(h, image.height() as u64);
    for word in image.as_raw().chunks(8) {
        let mut bytes = [0; 8];
        bytes[..word.len()].copy_from_slice(word);
        h = mix(h, u64::from_le_bytes(bytes));
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    // Never 0: the choice "none yet" of the dialogs.
    h.max(1)
}

impl Pattern {
    /// A pattern of `image`, which must not be empty. A big tile takes its
    /// time here (see the callers), not at its first reduced drawing.
    pub fn new(name: impl Into<String>, image: RgbaImage) -> Arc<Self> {
        assert!(image.width() > 0 && image.height() > 0);
        let p = Self {
            id: hash_pixels(&image),
            name: name.into(),
            image: Arc::new(image),
            mips: Default::default(),
            mirrors: Default::default(),
        };
        if p.image.width() as u64 * p.image.height() as u64 > 512 * 512 {
            p.mip(1);
        }
        Arc::new(p)
    }

    /// This tile under another name.
    pub fn renamed(&self, name: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            id: self.id,
            name: name.into(),
            image: self.image.clone(),
            mips: self.mips.clone(),
            mirrors: self.mirrors.clone(),
        })
    }

    /// This tile mirrored, under the same name: made once while it is held.
    pub fn mirrored(self: &Arc<Self>, horizontal: bool) -> Arc<Self> {
        let k = usize::from(!horizontal);
        let mut mirrors = self.mirrors.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = mirrors[k].upgrade() {
            return match m.name == self.name {
                true => m,
                false => m.renamed(self.name.clone()),
            };
        }
        let image = if horizontal {
            image::imageops::flip_horizontal(&*self.image)
        } else {
            image::imageops::flip_vertical(&*self.image)
        };
        let m = Self::new(self.name.clone(), image);
        mirrors[k] = Arc::downgrade(&m);
        m.mirrors.lock().unwrap_or_else(|e| e.into_inner())[k] = Arc::downgrade(self);
        m
    }

    /// The memory it holds, for the history's budget: (address, bytes).
    pub fn buffers(&self, out: &mut Vec<(usize, usize)>) {
        let mut raw = |r: &[u8]| out.push((r.as_ptr() as usize, r.len()));
        raw(self.image.as_raw());
        for m in self.mips.get().into_iter().flatten() {
            raw(m.as_raw());
        }
    }

    /// The tile reduced `k` times by half (`k` = 0: itself; beyond the
    /// smallest version: that one).
    pub fn mip(&self, k: usize) -> &RgbaImage {
        if k == 0 {
            return &self.image;
        }
        let mips = self.mips.get_or_init(|| {
            let mut all: Vec<RgbaImage> = Vec::new();
            loop {
                let last = all.last().unwrap_or(&self.image);
                if last.width() == 1 && last.height() == 1 {
                    return all;
                }
                let half = halved(last);
                all.push(half);
            }
        });
        match mips.get(k - 1).or(mips.last()) {
            Some(m) => m,
            None => &self.image,
        }
    }

    /// Non-premultiplied pixel of the tile at canvas pixel (x, y), the tile
    /// starting at the canvas origin, as Fill… lays it.
    #[inline]
    pub fn repeated(&self, x: i64, y: i64) -> [u8; 4] {
        let (w, h) = (self.image.width() as i64, self.image.height() as i64);
        let i = (y.rem_euclid(h) * w + x.rem_euclid(w)) as usize * 4;
        let p = &self.image.as_raw()[i..i + 4];
        [p[0], p[1], p[2], p[3]]
    }
}

/// For each of `cells` equal parts of `size` pixels: the pixels it covers
/// with their share, in 1/`cells` of a pixel (each part adds up to `size`).
/// At most 3 pixels, a part being at most 2 pixels wide.
fn coverage(size: u32, cells: u32) -> Vec<[(u32, u64); 3]> {
    (0..cells)
        .map(|c| {
            let (start, end) = (c as u64 * size as u64, (c as u64 + 1) * size as u64);
            let first = start / cells as u64;
            let mut out = [(0, 0); 3];
            for (k, slot) in out.iter_mut().enumerate() {
                let i = first + k as u64;
                let from = start.max(i * cells as u64);
                let to = end.min((i + 1) * cells as u64);
                if to > from {
                    *slot = (i as u32, to - from);
                }
            }
            out
        })
        .collect()
}

/// `image` at half its size (rounded up), each pixel the exact mean of the
/// part of the tile it covers, weighted by opacity: an odd tile keeps its
/// tone however much it is reduced.
fn halved(image: &RgbaImage) -> RgbaImage {
    let (w, h) = image.dimensions();
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let (across, down) = (coverage(w, nw), coverage(h, nh));
    let raw = image.as_raw();
    let total = w as u64 * h as u64;
    RgbaImage::from_fn(nw, nh, |x, y| {
        let mut sum = [0u64; 4];
        for &(py, wy) in &down[y as usize] {
            if wy == 0 {
                continue;
            }
            for &(px, wx) in &across[x as usize] {
                if wx == 0 {
                    continue;
                }
                let i = (py as usize * w as usize + px as usize) * 4;
                let p = &raw[i..i + 4];
                let a = p[3] as u64 * wx * wy;
                sum[0] += p[0] as u64 * a;
                sum[1] += p[1] as u64 * a;
                sum[2] += p[2] as u64 * a;
                sum[3] += a;
            }
        }
        if sum[3] == 0 {
            return image::Rgba([0; 4]);
        }
        let c = |v: u64| ((v + sum[3] / 2) / sum[3]) as u8;
        let alpha = (sum[3] + total / 2) / total;
        image::Rgba([c(sum[0]), c(sum[1]), c(sum[2]), alpha as u8])
    })
}

/// A pattern laid over the canvas.
#[derive(Clone, Debug, PartialEq)]
pub struct PatternFill {
    pub pattern: Arc<Pattern>,
    /// 1 = one tile pixel per canvas pixel.
    pub scale: f32,
    /// Degrees, counterclockwise.
    pub angle: f32,
    /// Moves with the layer ("Link with Layer"), from the layer's top left
    /// corner, or stays at the document's origin.
    pub linked: bool,
    /// Offset of the tile from that origin, in canvas pixels.
    pub phase: (f32, f32),
}

impl Default for PatternFill {
    fn default() -> Self {
        Self::of(crate::patterns::default_pattern())
    }
}

/// Range of the Scale fields, as a factor.
pub const SCALE_RANGE: std::ops::RangeInclusive<f32> = 0.01..=10.0;

/// Range of a stored scale: Image Size and transforms take it beyond what
/// the fields offer, so that undoing them by another gives the same look.
pub const STORED_SCALE: std::ops::RangeInclusive<f32> = 1e-4..=1e4;

/// `scale` multiplied by `k`, within [`STORED_SCALE`].
pub fn scaled(scale: f32, k: f32) -> f32 {
    let s = scale * k;
    if s.is_finite() {
        s.clamp(*STORED_SCALE.start(), *STORED_SCALE.end())
    } else {
        scale
    }
}

impl PatternFill {
    pub fn of(pattern: Arc<Pattern>) -> Self {
        Self {
            pattern,
            scale: 1.0,
            angle: 0.0,
            linked: true,
            phase: (0.0, 0.0),
        }
    }

    /// The document's geometry changed by `map`: the tile's origin stays on
    /// the same point of the image. `anchor` is the point a linked tile
    /// starts from, before and after (the layer's top left corner).
    pub fn map_origin(
        &mut self,
        map: impl Fn(f32, f32) -> (f32, f32),
        anchor: ((f32, f32), (f32, f32)),
    ) {
        let (before, after) = match self.linked {
            true => anchor,
            false => ((0.0, 0.0), (0.0, 0.0)),
        };
        let (x, y) = map(before.0 + self.phase.0, before.1 + self.phase.1);
        self.phase = (x - after.0, y - after.1);
    }

    /// The tile under the affine map `m` of the canvas (x' = m0 x + m1 y +
    /// m2, y' = m3 x + m4 y + m5), `anchor` as in [`Self::map_origin`]: it
    /// takes the map's turn and mean scale, mirrored if the map mirrors (a
    /// tile can't be stretched nor slanted).
    pub fn transformed(&mut self, m: [f32; 6], anchor: ((f32, f32), (f32, f32))) {
        let [a, b, c, d, e, f] = m;
        self.map_origin(|x, y| (a * x + b * y + c, d * x + e * y + f), anchor);
        let det = a * e - b * d;
        if !det.is_finite() || det == 0.0 {
            return;
        }
        // m = (m · mirror) · mirror, the mirror being x → -x.
        let (a, d) = match det < 0.0 {
            true => {
                self.mirrored(true);
                (-a, -d)
            }
            false => (a, d),
        };
        self.scale = scaled(self.scale, det.abs().sqrt());
        // Clockwise on screen: its first column is (cos, sin).
        let degrees = d.atan2(a).to_degrees();
        if degrees != 0.0 {
            self.turned(degrees);
        }
    }

    /// The tile turned by `degrees` clockwise.
    pub fn turned(&mut self, degrees: f32) {
        self.angle = (self.angle - degrees + 180.0).rem_euclid(360.0) - 180.0;
    }

    /// The tile seen in a mirror (a flip of the canvas): the same picture
    /// needs the mirrored tile, and the opposite angle.
    pub fn mirrored(&mut self, horizontal: bool) {
        self.pattern = self.pattern.mirrored(horizontal);
        self.angle = -self.angle;
    }
}

/// Sine and cosine of `degrees`, exact on quarter turns (turned images
/// stay exact).
fn sin_cos(degrees: f32) -> (f32, f32) {
    let turn = degrees.rem_euclid(360.0);
    if turn == 0.0 {
        (0.0, 1.0)
    } else if turn == 90.0 {
        (1.0, 0.0)
    } else if turn == 180.0 {
        (0.0, -1.0)
    } else if turn == 270.0 {
        (-1.0, 0.0)
    } else {
        degrees.to_radians().sin_cos()
    }
}

/// A pattern fill ready to be drawn at a level of reduction, its tile's top
/// left corner at `origin` (canvas pixels).
pub struct Sampler {
    pattern: Arc<Pattern>,
    mip: usize,
    /// Canvas pixels → pixels of the reduced tile.
    matrix: [f32; 4],
    origin: (f32, f32),
}

impl Sampler {
    pub fn new(fill: &PatternFill, origin: (f32, f32), level: u32) -> Self {
        let scale = if fill.scale.is_finite() {
            fill.scale.clamp(*STORED_SCALE.start(), *STORED_SCALE.end())
        } else {
            1.0
        };
        // Tile pixels under a pixel of the level.
        let footprint = (1u32 << level.min(30)) as f32 / scale;
        let mip = footprint.log2().floor().max(0.0) as usize;
        let tile = &fill.pattern.image;
        let reduced = fill.pattern.mip(mip);
        let (sx, sy) = (
            reduced.width() as f32 / tile.width() as f32 / scale,
            reduced.height() as f32 / tile.height() as f32 / scale,
        );
        let angle = if fill.angle.is_finite() {
            fill.angle
        } else {
            0.0
        };
        let (sin, cos) = sin_cos(angle);
        let (px, py) = (
            if fill.phase.0.is_finite() {
                fill.phase.0
            } else {
                0.0
            },
            if fill.phase.1.is_finite() {
                fill.phase.1
            } else {
                0.0
            },
        );
        Self {
            pattern: fill.pattern.clone(),
            mip,
            matrix: [cos * sx, -sin * sx, sin * sy, cos * sy],
            origin: (origin.0 + px, origin.1 + py),
        }
    }

    /// Non-premultiplied color (0..1) at canvas point (x, y), a pixel's
    /// center (bilinear; a tile pixel exactly at scale 1 and angle 0).
    #[inline]
    pub fn at(&self, x: f32, y: f32) -> [f32; 4] {
        let image = self.pattern.mip(self.mip);
        let (dx, dy) = (x - self.origin.0, y - self.origin.1);
        let [a, b, c, d] = self.matrix;
        let u = dx * a + dy * b - 0.5;
        let v = dx * c + dy * d - 0.5;
        let (w, h) = (image.width() as i64, image.height() as i64);
        let (fu, fv) = (u.floor(), v.floor());
        let (tx, ty) = (u - fu, v - fv);
        let (x0, y0) = ((fu as i64).rem_euclid(w), (fv as i64).rem_euclid(h));
        let (x1, y1) = ((x0 + 1) % w, (y0 + 1) % h);
        let raw = image.as_raw();
        let mut out = [0.0f32; 4];
        let mut add = |px: i64, py: i64, k: f32| {
            if k > 0.0 {
                let i = (py * w + px) as usize * 4;
                let p = &raw[i..i + 4];
                let alpha = p[3] as f32 * k;
                out[0] += p[0] as f32 * alpha;
                out[1] += p[1] as f32 * alpha;
                out[2] += p[2] as f32 * alpha;
                out[3] += alpha;
            }
        };
        add(x0, y0, (1.0 - tx) * (1.0 - ty));
        add(x1, y0, tx * (1.0 - ty));
        add(x0, y1, (1.0 - tx) * ty);
        add(x1, y1, tx * ty);
        if out[3] <= 0.0 {
            return [0.0; 4];
        }
        let k = 1.0 / (out[3] * 255.0);
        [out[0] * k, out[1] * k, out[2] * k, out[3] / 255.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn tile() -> Arc<Pattern> {
        // 2 × 1: red then blue.
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([0, 0, 255, 255]));
        Pattern::new("tile", image)
    }

    #[test]
    fn identity_is_by_content() {
        let t = tile();
        assert_eq!(t.id, Pattern::new("other", (*t.image).clone()).id);
        assert_eq!(t.id, t.renamed("x").id);
        let blank = Pattern::new("blank", RgbaImage::new(2, 1));
        assert_ne!(t.id, blank.id);
        assert_ne!(*t, *t.renamed("x"));
    }

    #[test]
    fn repeats_from_the_origin_exactly() {
        let f = PatternFill {
            linked: false,
            ..PatternFill::of(tile())
        };
        let s = Sampler::new(&f, (0.0, 0.0), 0);
        assert_eq!(s.at(0.5, 0.5), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.at(1.5, 0.5), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(s.at(2.5, 7.5), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.at(-0.5, 0.5), [0.0, 0.0, 1.0, 1.0], "wraps backwards");
        // From another origin and phase.
        let f = PatternFill {
            phase: (1.0, 0.0),
            ..f
        };
        let s = Sampler::new(&f, (0.0, 0.0), 0);
        assert_eq!(s.at(0.5, 0.5), [0.0, 0.0, 1.0, 1.0]);
        let s = Sampler::new(&f, (1.0, 0.0), 0);
        assert_eq!(s.at(0.5, 0.5), [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn scale_and_angle_move_the_tile() {
        let mut f = PatternFill::of(tile());
        f.scale = 2.0;
        let s = Sampler::new(&f, (0.0, 0.0), 0);
        assert_eq!(s.at(1.0, 0.5), [1.0, 0.0, 0.0, 1.0], "center of the first");
        assert_eq!(s.at(3.0, 0.5), [0.0, 0.0, 1.0, 1.0]);
        assert!(s.at(1.5, 0.5)[0] > s.at(2.5, 0.5)[0], "interpolated");
        // 90° counterclockwise: the row now runs upwards.
        f.scale = 1.0;
        f.angle = 90.0;
        let s = Sampler::new(&f, (0.0, 0.0), 0);
        let (a, b) = (s.at(0.5, -0.5), s.at(0.5, -1.5));
        assert!(a[0] > 0.9 && b[2] > 0.9, "{a:?} {b:?}");
    }

    #[test]
    fn reduced_views_show_the_mean_color() {
        let f = PatternFill::of(tile());
        // Level 1: each pixel covers both colors.
        let s = Sampler::new(&f, (0.0, 0.0), 1);
        let c = s.at(1.0, 1.0);
        assert!(
            (c[0] - 0.5).abs() < 0.02 && (c[2] - 0.5).abs() < 0.02,
            "{c:?}"
        );
        // Beyond the smallest version.
        let s = Sampler::new(&f, (0.0, 0.0), 12);
        assert_eq!(s.at(100.0, 100.0)[3], 1.0);
    }

    #[test]
    fn transparency_does_not_bleed_its_color() {
        let mut image = RgbaImage::new(2, 1);
        image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        image.put_pixel(1, 0, Rgba([0, 255, 0, 0]));
        let f = PatternFill {
            scale: 4.0,
            ..PatternFill::of(Pattern::new("t", image))
        };
        let c = Sampler::new(&f, (0.0, 0.0), 0).at(4.0, 0.5);
        assert!(c[1] == 0.0 && c[3] > 0.1 && c[3] < 0.9, "{c:?}");
        // Damaged values stay drawable.
        let f = PatternFill {
            scale: f32::NAN,
            angle: f32::INFINITY,
            phase: (f32::NAN, f32::NAN),
            ..f
        };
        assert!(Sampler::new(&f, (0.0, 0.0), 0).at(0.5, 0.5)[3] > 0.99);
    }

    #[test]
    fn two_top_bits_do_not_cancel_out() {
        // Two alphas differing by 128 at odd pixels: the top bit of two words.
        let a = RgbaImage::from_pixel(4, 1, Rgba([10, 20, 30, 200]));
        let mut b = a.clone();
        b.put_pixel(1, 0, Rgba([10, 20, 30, 72]));
        b.put_pixel(3, 0, Rgba([10, 20, 30, 72]));
        assert_ne!(Pattern::new("a", a).id, Pattern::new("b", b).id);
    }

    /// The mean color of a tile, over a whole number of periods.
    fn mean_at(s: &Sampler, w: u32, h: u32, level: u32) -> f32 {
        let step = (1u32 << level) as f32;
        // Enough pixels of the level for every tile pixel to count alike.
        let (nx, ny) = (w * 8, h * 8);
        let mut total = 0.0;
        for y in 0..ny {
            for x in 0..nx {
                total += s.at((x as f32 + 0.5) * step, (y as f32 + 0.5) * step)[0];
            }
        }
        total / (nx * ny) as f32
    }

    #[test]
    fn odd_tiles_keep_their_tone_when_reduced() {
        for (w, h) in [
            (3, 3),
            (5, 5),
            (7, 7),
            (15, 15),
            (16, 16),
            (17, 1),
            (33, 33),
        ] {
            let image = RgbaImage::from_fn(w, h, |x, y| {
                let v = if x == 0 || y == 0 { 0 } else { 255 };
                Rgba([v, v, v, 255])
            });
            let truth = image.pixels().map(|p| p.0[0] as f32 / 255.0).sum::<f32>() / (w * h) as f32;
            let f = PatternFill::of(Pattern::new("t", image));
            for level in 1..6 {
                let mean = mean_at(&Sampler::new(&f, (0.0, 0.0), level), w, h, level);
                // One rounding per reduction.
                assert!(
                    (mean - truth).abs() < 0.02,
                    "{w}x{h} level {level}: {mean} for {truth}"
                );
            }
        }
    }

    #[test]
    fn every_reduced_pixel_is_covered_exactly() {
        for (size, cells) in [(1, 1), (2, 1), (3, 2), (5, 3), (7, 4), (16, 8), (33, 17)] {
            let parts = coverage(size, cells);
            let mut each = vec![0; size as usize];
            for part in &parts {
                let sum: u64 = part.iter().map(|&(_, k)| k).sum();
                assert_eq!(sum, size as u64);
                for &(i, k) in part {
                    if k > 0 {
                        each[i as usize] += k;
                    }
                }
            }
            assert!(each.iter().all(|&k| k == cells as u64), "{size}/{cells}");
        }
    }

    #[test]
    fn mirrored_and_turned_tiles_draw_the_mirrored_and_turned_picture() {
        let tile = Pattern::new(
            "t",
            RgbaImage::from_fn(5, 3, |x, y| Rgba([x as u8 * 50, y as u8 * 100, 7, 255])),
        );
        // A point of the canvas, and where it goes.
        for (angle, phase) in [(0.0, (0.0, 0.0)), (30.0, (2.0, 1.0)), (-75.0, (-3.0, 4.0))] {
            let f = PatternFill {
                angle,
                phase,
                linked: false,
                ..PatternFill::of(tile.clone())
            };
            let before = Sampler::new(&f, (0.0, 0.0), 0);
            let mut flipped = f.clone();
            flipped.map_origin(|x, y| (40.0 - x, y), Default::default());
            flipped.mirrored(true);
            let flipped = Sampler::new(&flipped, (0.0, 0.0), 0);
            let mut quarter = f.clone();
            quarter.map_origin(|x, y| (30.0 - y, x), Default::default());
            quarter.turned(90.0);
            let quarter = Sampler::new(&quarter, (0.0, 0.0), 0);
            for (x, y) in [(3.5, 2.5), (11.5, 7.5), (21.5, 13.5)] {
                let (a, b, c) = (
                    before.at(x, y),
                    flipped.at(40.0 - x, y),
                    quarter.at(30.0 - y, x),
                );
                for i in 0..4 {
                    assert!((a[i] - b[i]).abs() < 0.02, "flip {angle} {a:?} {b:?}");
                    assert!((a[i] - c[i]).abs() < 0.02, "turn {angle} {a:?} {c:?}");
                }
            }
        }
    }
}
