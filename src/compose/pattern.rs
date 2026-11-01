//! Patterns of Fill…, Pattern Fill layers and Pattern Overlay: a tile
//! repeated over the canvas, at a scale and angle, from an origin.

use std::sync::{Arc, OnceLock};

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

/// FNV-1a over 64-bit words, of the size and pixels.
fn hash_pixels(image: &RgbaImage) -> u64 {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mix = |h: u64, v: u64| (h ^ v).wrapping_mul(PRIME);
    let mut h = 0xcbf2_9ce4_8422_2325;
    h = mix(h, image.width() as u64);
    h = mix(h, image.height() as u64);
    for word in image.as_raw().chunks(8) {
        let mut bytes = [0; 8];
        bytes[..word.len()].copy_from_slice(word);
        h = mix(h, u64::from_le_bytes(bytes));
    }
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
        })
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

/// `image` at half its size (rounded up), each pixel the mean of those it
/// covers, tile edges wrapping around; weighted by opacity.
fn halved(image: &RgbaImage) -> RgbaImage {
    let (w, h) = image.dimensions();
    let (nw, nh) = (w.div_ceil(2), h.div_ceil(2));
    let raw = image.as_raw();
    let at = |x: u32, y: u32| {
        let i = ((y % h) as usize * w as usize + (x % w) as usize) * 4;
        &raw[i..i + 4]
    };
    RgbaImage::from_fn(nw, nh, |x, y| {
        let mut sum = [0u32; 4];
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let p = at(x * 2 + dx, y * 2 + dy);
            let a = p[3] as u32;
            sum[0] += p[0] as u32 * a;
            sum[1] += p[1] as u32 * a;
            sum[2] += p[2] as u32 * a;
            sum[3] += a;
        }
        if sum[3] == 0 {
            return image::Rgba([0; 4]);
        }
        let c = |v: u32| ((v + sum[3] / 2) / sum[3]) as u8;
        image::Rgba([c(sum[0]), c(sum[1]), c(sum[2]), ((sum[3] + 2) / 4) as u8])
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
    /// Moves with the layer ("Link with Layer"), from its top left corner,
    /// or stays at the document's origin.
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
            fill.scale.clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end())
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
        let (sin, cos) = angle.to_radians().sin_cos();
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
}
