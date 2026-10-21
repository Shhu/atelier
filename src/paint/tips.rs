//! Brush tips: the computed round tip, and sampled tips (grayscale images)
//! generated procedurally for the natural-media presets.

use std::sync::{Arc, OnceLock};

use image::{GrayImage, Luma};

/// Shape of a brush dab.
#[derive(Clone)]
pub enum Tip {
    /// Disc (ellipse with the roundness), softened by the hardness.
    Round,
    /// Grayscale image stretched to the diameter (its hardness is its own).
    Sampled(Arc<TipImage>),
}

impl PartialEq for Tip {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Tip::Round, Tip::Round) => true,
            (Tip::Sampled(a), Tip::Sampled(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// A sampled tip and its reductions (each half the size of the previous
/// one), so that small dabs keep the tip's look instead of aliasing.
pub struct TipImage {
    levels: Vec<GrayImage>,
}

impl TipImage {
    pub fn new(image: GrayImage) -> Self {
        let mut levels = vec![image];
        while let Some(last) = levels.last()
            && last.width() >= 8
            && last.height() >= 8
        {
            let (w, h) = (last.width() / 2, last.height() / 2);
            let half = GrayImage::from_fn(w, h, |x, y| {
                let sum: u32 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .map(|(dx, dy)| last.get_pixel(2 * x + dx, 2 * y + dy).0[0] as u32)
                    .sum();
                Luma([((sum + 2) / 4) as u8])
            });
            levels.push(half);
        }
        Self { levels }
    }

    /// Height over width.
    pub fn aspect(&self) -> f32 {
        let l = &self.levels[0];
        l.height() as f32 / l.width() as f32
    }

    /// The full-size image (previews).
    pub fn image(&self) -> &GrayImage {
        &self.levels[0]
    }

    /// Coverage (0..1) at (`u`, `v`) in -1..1 across the tip, for a dab
    /// `diameter` px wide: bilinear, in the smallest reduction still at
    /// least that wide.
    pub fn sample(&self, u: f32, v: f32, diameter: f32) -> f32 {
        let level = self
            .levels
            .iter()
            .rev()
            .find(|l| l.width() as f32 >= diameter)
            .unwrap_or(&self.levels[0]);
        let (w, h) = (level.width() as f32, level.height() as f32);
        let x = (u + 1.0) * 0.5 * w - 0.5;
        let y = (v + 1.0) * 0.5 * h - 0.5;
        if x <= -1.0 || y <= -1.0 || x >= w || y >= h {
            return 0.0;
        }
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |px: f32, py: f32| {
            if px < 0.0 || py < 0.0 || px >= w || py >= h {
                0.0
            } else {
                level.get_pixel(px as u32, py as u32).0[0] as f32
            }
        };
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
        let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
        (top * (1.0 - fy) + bottom * fy) / 255.0
    }
}

/// Brushes offered in the preset picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    HardRound,
    SoftRound,
    Chalk,
    Spatter,
    DryBrush,
    Charcoal,
}

/// What a preset sets: tip, size (px), hardness, spacing (fraction of the
/// diameter).
pub struct PresetSettings {
    pub tip: Tip,
    pub size: f32,
    pub hardness: f32,
    pub spacing: f32,
}

impl Preset {
    pub const ALL: [Preset; 6] = [
        Preset::HardRound,
        Preset::SoftRound,
        Preset::Chalk,
        Preset::Spatter,
        Preset::DryBrush,
        Preset::Charcoal,
    ];

    pub fn settings(self) -> PresetSettings {
        let (tip, size, hardness, spacing) = match self {
            Preset::HardRound => (Tip::Round, 30.0, 1.0, 0.25),
            Preset::SoftRound => (Tip::Round, 45.0, 0.0, 0.25),
            Preset::Chalk => (self.sampled(), 60.0, 1.0, 0.25),
            Preset::Spatter => (self.sampled(), 40.0, 1.0, 0.3),
            Preset::DryBrush => (self.sampled(), 50.0, 1.0, 0.05),
            Preset::Charcoal => (self.sampled(), 40.0, 1.0, 0.15),
        };
        PresetSettings {
            tip,
            size,
            hardness,
            spacing,
        }
    }

    /// The preset's tip image, generated once.
    fn sampled(self) -> Tip {
        static TIPS: OnceLock<[Arc<TipImage>; 4]> = OnceLock::new();
        let tips = TIPS.get_or_init(|| {
            [chalk(), spatter(), dry_brush(), charcoal()].map(|i| Arc::new(TipImage::new(i)))
        });
        let i = match self {
            Preset::Chalk => 0,
            Preset::Spatter => 1,
            Preset::DryBrush => 2,
            _ => 3,
        };
        Tip::Sampled(Arc::clone(&tips[i]))
    }

    /// The preset whose tip this is (a round tip: by its hardness).
    pub fn of(tip: &Tip, hardness: f32) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| {
            let s = p.settings();
            s.tip == *tip && (!matches!(tip, Tip::Round) || s.hardness == hardness)
        })
    }
}

/// Small deterministic generator (xorshift), so that tips look the same at
/// every launch.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1 << 24) as f32
    }
}

/// Smooth value noise over a `cells` × `cells` grid, in 0..1.
fn value_noise(size: u32, cells: u32, seed: u32) -> Vec<f32> {
    let mut rng = Noise(seed);
    let n = cells as usize + 1;
    let grid: Vec<f32> = (0..n * n).map(|_| rng.next()).collect();
    let mut out = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let gx = x as f32 / size as f32 * cells as f32;
            let gy = y as f32 / size as f32 * cells as f32;
            let (ix, iy) = (gx as usize, gy as usize);
            let s = |t: f32| t * t * (3.0 - 2.0 * t);
            let (fx, fy) = (s(gx.fract()), s(gy.fract()));
            let g = |i: usize, j: usize| grid[j.min(n - 1) * n + i.min(n - 1)];
            let top = g(ix, iy) * (1.0 - fx) + g(ix + 1, iy) * fx;
            let bottom = g(ix, iy + 1) * (1.0 - fx) + g(ix + 1, iy + 1) * fx;
            out.push(top * (1.0 - fy) + bottom * fy);
        }
    }
    out
}

const SIZE: u32 = 128;

fn from_fn(f: impl Fn(f32, f32, usize) -> f32) -> GrayImage {
    let mut image = GrayImage::new(SIZE, SIZE);
    for (i, (x, y, p)) in image.enumerate_pixels_mut().enumerate() {
        // Centered coordinates, -1..1.
        let u = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
        let v = (y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
        p.0[0] = (f(u, v, i).clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    image
}

fn soft_disc(d: f32, edge: f32) -> f32 {
    ((1.0 - d) / edge).clamp(0.0, 1.0)
}

/// Chalk: a disc with a rough edge and a grain that lets the paper show.
fn chalk() -> GrayImage {
    let coarse = value_noise(SIZE, 8, 0x9e37_79b9);
    let fine = value_noise(SIZE, 40, 0x85eb_ca6b);
    from_fn(|u, v, i| {
        let d = (u * u + v * v).sqrt() + (coarse[i] - 0.5) * 0.35;
        let grain = ((fine[i] - 0.25) * 2.2).clamp(0.0, 1.0);
        soft_disc(d / 0.75, 0.15) * grain
    })
}

/// Spatter: droplets of various sizes, denser toward the center.
fn spatter() -> GrayImage {
    let mut rng = Noise(0xc2b2_ae35);
    let drops: Vec<(f32, f32, f32)> = (0..46)
        .map(|_| {
            let angle = rng.next() * std::f32::consts::TAU;
            let dist = rng.next().sqrt() * 0.78;
            let radius = 0.025 + rng.next().powi(3) * 0.15;
            (dist * angle.cos(), dist * angle.sin(), radius)
        })
        .collect();
    from_fn(|u, v, _| {
        drops
            .iter()
            .map(|&(x, y, r)| {
                let d = ((u - x).powi(2) + (v - y).powi(2)).sqrt() / r;
                soft_disc(d, 0.3)
            })
            .fold(0.0, f32::max)
    })
}

/// Dry brush: the tips of a flat brush's bristles, in a band across the
/// tip, unevenly loaded.
fn dry_brush() -> GrayImage {
    let mut rng = Noise(0x27d4_eb2f);
    let bristles: Vec<(f32, f32, f32, f32)> = (0..34)
        .map(|_| {
            let x = rng.next() * 1.8 - 0.9;
            let y = (rng.next() - 0.5) * 0.36;
            let r = 0.03 + rng.next() * 0.05;
            let load = 0.35 + rng.next() * 0.65;
            (x, y, r, load)
        })
        .collect();
    from_fn(|u, v, _| {
        bristles
            .iter()
            .map(|&(x, y, r, load)| {
                // Slightly elongated across the band.
                let d = ((u - x).powi(2) + ((v - y) * 0.7).powi(2)).sqrt() / r;
                soft_disc(d, 0.5) * load
            })
            .fold(0.0, f32::max)
    })
}

/// Charcoal: a grainy, slanted stick with a broken edge.
fn charcoal() -> GrayImage {
    let coarse = value_noise(SIZE, 6, 0x1656_67b1);
    let fine = value_noise(SIZE, 64, 0xd3a2_646c);
    from_fn(|u, v, i| {
        let (a, b) = (u * 0.94 + v * 0.34, -u * 0.34 + v * 0.94);
        let d = ((a / 0.75).powi(2) + (b / 0.45).powi(2)).sqrt() + (coarse[i] - 0.5) * 0.4;
        let grain = ((fine[i] - 0.35) * 2.6).clamp(0.0, 1.0);
        soft_disc(d, 0.25) * (0.3 + 0.7 * grain)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampled_tips_are_centered_and_not_empty() {
        for preset in [
            Preset::Chalk,
            Preset::Spatter,
            Preset::DryBrush,
            Preset::Charcoal,
        ] {
            let Tip::Sampled(tip) = preset.settings().tip else {
                panic!("{preset:?} is sampled");
            };
            let image = tip.image();
            let covered = image.pixels().filter(|p| p.0[0] > 64).count();
            assert!(covered > 200, "{preset:?}: {covered} px");
            // Nothing on the border: dabs don't show a square edge.
            let w = image.width() - 1;
            let edge = (0..=w).all(|i| {
                [(i, 0), (i, w), (0, i), (w, i)]
                    .iter()
                    .all(|&(x, y)| image.get_pixel(x, y).0[0] < 8)
            });
            assert!(edge, "{preset:?}");
        }
    }

    #[test]
    fn small_dabs_use_a_reduction() {
        let mut image = GrayImage::new(64, 64);
        // Checkerboard of single pixels: its reductions are uniform grey.
        for (x, y, p) in image.enumerate_pixels_mut() {
            p.0[0] = if (x + y) % 2 == 0 { 255 } else { 0 };
        }
        let tip = TipImage::new(image);
        let v = tip.sample(0.1, 0.1, 8.0);
        assert!((v - 0.5).abs() < 0.05, "{v}");
        assert_eq!(tip.sample(1.5, 0.0, 8.0), 0.0, "outside the tip");
    }

    #[test]
    fn preset_is_found_from_its_tip() {
        for preset in Preset::ALL {
            let s = preset.settings();
            assert_eq!(Preset::of(&s.tip, s.hardness), Some(preset));
        }
        assert_eq!(Preset::of(&Tip::Round, 0.5), None);
    }
}
