//! Blend modes: how a layer's colors combine with what is below it, with
//! Photoshop's formulas, and source-over compositing in premultiplied
//! colors (W3C compositing model).

use crate::i18n::t;

/// How a layer blends with what is below it, in the order of Photoshop's
/// list. `PassThrough` is for groups only: their layers blend with what is
/// under the group, as if they were not grouped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BlendMode {
    PassThrough,
    #[default]
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl BlendMode {
    /// The modes of a layer, in the order of the list (groups add Pass
    /// Through first). Each slice is one block of the menu.
    pub const GROUPS: [&'static [BlendMode]; 6] = [
        &[BlendMode::Normal, BlendMode::Dissolve],
        &[
            BlendMode::Darken,
            BlendMode::Multiply,
            BlendMode::ColorBurn,
            BlendMode::LinearBurn,
            BlendMode::DarkerColor,
        ],
        &[
            BlendMode::Lighten,
            BlendMode::Screen,
            BlendMode::ColorDodge,
            BlendMode::LinearDodge,
            BlendMode::LighterColor,
        ],
        &[
            BlendMode::Overlay,
            BlendMode::SoftLight,
            BlendMode::HardLight,
            BlendMode::VividLight,
            BlendMode::LinearLight,
            BlendMode::PinLight,
            BlendMode::HardMix,
        ],
        &[
            BlendMode::Difference,
            BlendMode::Exclusion,
            BlendMode::Subtract,
            BlendMode::Divide,
        ],
        &[
            BlendMode::Hue,
            BlendMode::Saturation,
            BlendMode::Color,
            BlendMode::Luminosity,
        ],
    ];

    /// Every mode a layer can have, in the order of the list.
    pub fn all() -> impl Iterator<Item = BlendMode> {
        Self::GROUPS.into_iter().flatten().copied()
    }

    /// The modes Shift++ and Shift+- go through: Pass Through too for a group.
    pub fn cycle(group: bool) -> Vec<BlendMode> {
        group
            .then_some(BlendMode::PassThrough)
            .into_iter()
            .chain(Self::all())
            .collect()
    }

    pub fn label(self) -> String {
        match self {
            BlendMode::PassThrough => t!("core-blend-pass-through"),
            BlendMode::Normal => t!("blend-normal"),
            BlendMode::Dissolve => t!("core-blend-dissolve"),
            BlendMode::Darken => t!("core-blend-darken"),
            BlendMode::Multiply => t!("blend-multiply"),
            BlendMode::ColorBurn => t!("core-blend-color-burn"),
            BlendMode::LinearBurn => t!("core-blend-linear-burn"),
            BlendMode::DarkerColor => t!("core-blend-darker-color"),
            BlendMode::Lighten => t!("core-blend-lighten"),
            BlendMode::Screen => t!("blend-screen"),
            BlendMode::ColorDodge => t!("core-blend-color-dodge"),
            BlendMode::LinearDodge => t!("core-blend-linear-dodge"),
            BlendMode::LighterColor => t!("core-blend-lighter-color"),
            BlendMode::Overlay => t!("core-blend-overlay"),
            BlendMode::SoftLight => t!("core-blend-soft-light"),
            BlendMode::HardLight => t!("core-blend-hard-light"),
            BlendMode::VividLight => t!("core-blend-vivid-light"),
            BlendMode::LinearLight => t!("core-blend-linear-light"),
            BlendMode::PinLight => t!("core-blend-pin-light"),
            BlendMode::HardMix => t!("core-blend-hard-mix"),
            BlendMode::Difference => t!("core-blend-difference"),
            BlendMode::Exclusion => t!("core-blend-exclusion"),
            BlendMode::Subtract => t!("core-blend-subtract"),
            BlendMode::Divide => t!("core-blend-divide"),
            BlendMode::Hue => t!("core-blend-hue"),
            BlendMode::Saturation => t!("core-blend-saturation"),
            BlendMode::Color => t!("core-blend-color"),
            BlendMode::Luminosity => t!("core-blend-luminosity"),
        }
    }

    /// Letter of its Alt+Shift shortcut, like Photoshop's (some modes have
    /// none).
    pub fn letter(self) -> Option<char> {
        Some(match self {
            BlendMode::PassThrough => 'P',
            BlendMode::Normal => 'N',
            BlendMode::Dissolve => 'I',
            BlendMode::Darken => 'K',
            BlendMode::Multiply => 'M',
            BlendMode::ColorBurn => 'B',
            BlendMode::LinearBurn => 'A',
            BlendMode::Lighten => 'G',
            BlendMode::Screen => 'S',
            BlendMode::ColorDodge => 'D',
            BlendMode::LinearDodge => 'W',
            BlendMode::Overlay => 'O',
            BlendMode::SoftLight => 'F',
            BlendMode::HardLight => 'H',
            BlendMode::VividLight => 'V',
            BlendMode::LinearLight => 'J',
            BlendMode::PinLight => 'Z',
            BlendMode::HardMix => 'L',
            BlendMode::Difference => 'E',
            BlendMode::Exclusion => 'X',
            BlendMode::Hue => 'U',
            BlendMode::Saturation => 'T',
            BlendMode::Color => 'C',
            BlendMode::Luminosity => 'Y',
            BlendMode::DarkerColor
            | BlendMode::LighterColor
            | BlendMode::Subtract
            | BlendMode::Divide => return None,
        })
    }

    /// Whether the mode works on each channel on its own.
    fn separable(self) -> bool {
        !matches!(
            self,
            BlendMode::DarkerColor
                | BlendMode::LighterColor
                | BlendMode::Hue
                | BlendMode::Saturation
                | BlendMode::Color
                | BlendMode::Luminosity
        )
    }
}

/// Blended color B(backdrop, source) of one channel (0..1), Photoshop's
/// formulas.
fn channel(mode: BlendMode, b: f32, s: f32) -> f32 {
    match mode {
        BlendMode::Darken => b.min(s),
        BlendMode::Multiply => b * s,
        BlendMode::ColorBurn => color_burn(b, s),
        BlendMode::LinearBurn => (b + s - 1.0).max(0.0),
        BlendMode::Lighten => b.max(s),
        BlendMode::Screen => b + s - b * s,
        BlendMode::ColorDodge => color_dodge(b, s),
        BlendMode::LinearDodge => (b + s).min(1.0),
        BlendMode::Overlay => hard_light(s, b),
        // Photoshop's soft light (not the W3C one).
        BlendMode::SoftLight => {
            if s < 0.5 {
                2.0 * b * s + b * b * (1.0 - 2.0 * s)
            } else {
                2.0 * b * (1.0 - s) + b.sqrt() * (2.0 * s - 1.0)
            }
        }
        BlendMode::HardLight => hard_light(b, s),
        BlendMode::VividLight => {
            if s <= 0.5 {
                color_burn(b, 2.0 * s)
            } else {
                color_dodge(b, 2.0 * s - 1.0)
            }
        }
        BlendMode::LinearLight => (b + 2.0 * s - 1.0).clamp(0.0, 1.0),
        BlendMode::PinLight => {
            if s <= 0.5 {
                b.min(2.0 * s)
            } else {
                b.max(2.0 * s - 1.0)
            }
        }
        // 8-bit values adding up to 255 or more give white.
        BlendMode::HardMix => {
            if b + s >= 1.0 - 0.5 / 255.0 {
                1.0
            } else {
                0.0
            }
        }
        BlendMode::Difference => (b - s).abs(),
        BlendMode::Exclusion => b + s - 2.0 * b * s,
        BlendMode::Subtract => (b - s).max(0.0),
        BlendMode::Divide => {
            if s <= 0.0 {
                if b <= 0.0 { 0.0 } else { 1.0 }
            } else {
                (b / s).min(1.0)
            }
        }
        _ => s,
    }
}

fn color_burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

fn color_dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        let s = 2.0 * s - 1.0;
        b + s - b * s
    }
}

/// Luminosity (Photoshop's weights).
pub fn lum([r, g, b]: [f32; 3]) -> f32 {
    0.3 * r + 0.59 * g + 0.11 * b
}

/// Same hue and saturation, with luminosity `l`, kept in gamut (W3C SetLum
/// and ClipColor).
pub fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    let c = c.map(|v| v + d);
    let l = lum(c);
    let (lo, hi) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    c.map(|v| {
        let mut v = v;
        if lo < 0.0 && l - lo > 1e-6 {
            v = l + (v - l) * l / (l - lo);
        }
        if hi > 1.0 && hi - l > 1e-6 {
            v = l + (v - l) * (1.0 - l) / (hi - l);
        }
        v.clamp(0.0, 1.0)
    })
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// Same hue, saturation `s` (W3C SetSat).
fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let (lo, hi) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
    if hi - lo <= 1e-6 {
        return [0.0; 3];
    }
    c.map(|v| (v - lo) * s / (hi - lo))
}

/// Blended color B(backdrop, source) of a mode, non-premultiplied (0..1).
pub fn blend_color(mode: BlendMode, b: [f32; 3], s: [f32; 3]) -> [f32; 3] {
    if mode.separable() {
        return [0, 1, 2].map(|i| channel(mode, b[i], s[i]));
    }
    match mode {
        // "Compares the total of all channel values" of both colors.
        BlendMode::DarkerColor => {
            if s.iter().sum::<f32>() < b.iter().sum::<f32>() {
                s
            } else {
                b
            }
        }
        BlendMode::LighterColor => {
            if s.iter().sum::<f32>() > b.iter().sum::<f32>() {
                s
            } else {
                b
            }
        }
        BlendMode::Hue => set_lum(set_sat(s, sat(b)), lum(b)),
        BlendMode::Saturation => set_lum(set_sat(b, sat(s)), lum(b)),
        BlendMode::Color => set_lum(s, lum(b)),
        _ => set_lum(b, lum(s)),
    }
}

/// Premultiplied color (0..1): red, green, blue, alpha.
pub type Px = [f32; 4];

/// Places color `s` (non-premultiplied) of opacity `a` over `d`, in mode
/// `mode` (not Dissolve: see [`dissolve`]).
#[inline]
pub fn over(d: &mut Px, s: [f32; 3], a: f32, mode: BlendMode) {
    if a <= 0.0 {
        return;
    }
    let da = d[3];
    if da <= 0.0 || matches!(mode, BlendMode::Normal | BlendMode::PassThrough) {
        for i in 0..3 {
            d[i] = s[i] * a + d[i] * (1.0 - a);
        }
    } else {
        let b = [d[0] / da, d[1] / da, d[2] / da];
        let m = blend_color(mode, b, s);
        for i in 0..3 {
            d[i] = a * (1.0 - da) * s[i] + a * da * m[i] + (1.0 - a) * d[i];
        }
    }
    d[3] = a + da * (1.0 - a);
}

/// Like [`over`], but the result keeps the opacity of `d` (a layer clipped
/// to the one below shows only where that one does).
#[inline]
pub fn atop(d: &mut Px, s: [f32; 3], a: f32, mode: BlendMode) {
    let da = d[3];
    if a <= 0.0 || da <= 0.0 {
        return;
    }
    let m = match mode {
        BlendMode::Normal | BlendMode::PassThrough => s,
        _ => blend_color(mode, [d[0] / da, d[1] / da, d[2] / da], s),
    };
    for i in 0..3 {
        d[i] += a * (da * m[i] - d[i]);
    }
}

/// Dissolve: the pixel shows whole or not at all, with a probability of its
/// opacity (fixed noise per pixel, so it doesn't flicker).
#[inline]
pub fn dissolve(a: f32, x: i64, y: i64) -> f32 {
    let h = (x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
    let h = (h ^ (h >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let noise = ((h ^ (h >> 32)) & 0xFFFF) as f32 / 65536.0;
    if noise < a { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(mode: BlendMode, base: f32, blend: f32) -> f32 {
        blend_color(mode, [base; 3], [blend; 3])[0]
    }

    #[test]
    fn separable_modes_follow_photoshop() {
        let close = |a: f32, e: f32| (a - e).abs() < 1e-5;
        let (lo, mid, hi) = (0.25, 0.5, 0.75);
        assert!(close(b(BlendMode::Multiply, mid, mid), 0.25));
        assert!(close(b(BlendMode::Screen, mid, mid), 0.75));
        assert!(close(b(BlendMode::Darken, lo, hi), lo));
        assert!(close(b(BlendMode::Lighten, lo, hi), hi));
        assert!(close(b(BlendMode::ColorBurn, mid, mid), 0.0));
        assert!(close(b(BlendMode::ColorBurn, 1.0, 0.0), 1.0));
        assert!(close(b(BlendMode::ColorDodge, lo, mid), 0.5));
        assert!(close(b(BlendMode::ColorDodge, 0.0, 1.0), 0.0));
        assert!(close(b(BlendMode::LinearBurn, hi, mid), 0.25));
        assert!(close(b(BlendMode::LinearDodge, hi, mid), 1.0));
        // Overlay: hard light with the layers swapped.
        assert!(close(
            b(BlendMode::Overlay, lo, hi),
            b(BlendMode::HardLight, hi, lo)
        ));
        assert!(close(b(BlendMode::HardLight, mid, lo), 0.25));
        assert!(close(b(BlendMode::HardLight, mid, hi), 0.75));
        // Soft light and overlay: a middle gray changes nothing.
        for base in [0.0, lo, mid, hi, 1.0] {
            assert!(close(b(BlendMode::SoftLight, base, 0.5), base));
            assert!(close(b(BlendMode::Overlay, base, 0.5), base));
        }
        assert!(close(b(BlendMode::SoftLight, lo, 1.0), 0.5));
        assert!(close(b(BlendMode::VividLight, mid, lo), 0.0));
        assert!(close(b(BlendMode::VividLight, lo, hi), 0.5));
        assert!(close(b(BlendMode::LinearLight, mid, hi), 1.0));
        assert!(close(b(BlendMode::PinLight, hi, lo), 0.5));
        assert!(close(b(BlendMode::PinLight, lo, hi), 0.5));
        assert!(close(
            b(BlendMode::HardMix, 128.0 / 255.0, 127.0 / 255.0),
            1.0
        ));
        assert!(close(
            b(BlendMode::HardMix, 128.0 / 255.0, 126.0 / 255.0),
            0.0
        ));
        assert!(close(b(BlendMode::Difference, lo, hi), 0.5));
        assert!(close(b(BlendMode::Exclusion, mid, mid), 0.5));
        assert!(close(b(BlendMode::Subtract, lo, hi), 0.0));
        assert!(close(b(BlendMode::Divide, lo, mid), 0.5));
        assert!(close(b(BlendMode::Divide, lo, 0.0), 1.0));
    }

    #[test]
    fn non_separable_modes_trade_hue_and_luminosity() {
        let red = [1.0, 0.0, 0.0];
        let gray = [0.5, 0.5, 0.5];
        // Color: the hue of red at the luminosity of the gray.
        let c = blend_color(BlendMode::Color, gray, red);
        assert!((lum(c) - 0.5).abs() < 1e-4 && c[0] > c[1], "{c:?}");
        // Luminosity: the gray at red's luminosity.
        let l = blend_color(BlendMode::Luminosity, gray, red);
        assert!(l.iter().all(|&v| (v - 0.3).abs() < 1e-4), "{l:?}");
        // Saturation of a gray: nothing to saturate.
        let s = blend_color(BlendMode::Saturation, gray, red);
        assert!(s.iter().all(|&v| (v - 0.5).abs() < 1e-4), "{s:?}");
        assert_eq!(blend_color(BlendMode::DarkerColor, gray, red), red);
        assert_eq!(blend_color(BlendMode::LighterColor, gray, red), gray);
    }

    #[test]
    fn compositing_keeps_alpha_rules() {
        // Over transparency, every mode puts the color as is.
        for mode in BlendMode::all().filter(|m| *m != BlendMode::Dissolve) {
            let mut d = [0.0; 4];
            over(&mut d, [0.2, 0.4, 0.6], 0.5, mode);
            assert!(
                (d[3] - 0.5).abs() < 1e-6 && (d[0] - 0.1).abs() < 1e-6,
                "{mode:?}"
            );
        }
        // Atop keeps the backdrop's opacity.
        let mut d = [0.25, 0.25, 0.25, 0.5];
        atop(&mut d, [1.0, 1.0, 1.0], 1.0, BlendMode::Normal);
        assert_eq!(d, [0.5, 0.5, 0.5, 0.5]);
        // Dissolve: about the given share of pixels.
        let shown = (0..10_000).filter(|i| dissolve(0.3, i % 100, i / 100) > 0.0);
        let n = shown.count();
        assert!((2_700..3_300).contains(&n), "{n}");
    }
}
