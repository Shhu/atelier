//! Toning tools: Dodge and Burn (by tonal range) and Sponge, on colors in
//! 0..1, with a strength `k` (0..1) given by the stroke's coverage.

/// Tones a dodge or burn works on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneRange {
    Shadows,
    Midtones,
    Highlights,
}

impl ToneRange {
    pub const ALL: [ToneRange; 3] = [
        ToneRange::Shadows,
        ToneRange::Midtones,
        ToneRange::Highlights,
    ];
}

fn luma([r, g, b]: [f32; 3]) -> f32 {
    0.299 * r + 0.587 * g + 0.114 * b
}

/// Tone curve of a dodge (lighten) or burn (darken) of `range`, strength `k`.
/// Highlights dodged and shadows burned clip, like an exposure change; the
/// other ranges keep black and white.
fn curve(v: f32, k: f32, burn: bool, range: ToneRange) -> f32 {
    let v = v.clamp(0.0, 1.0);
    let out = match (burn, range) {
        (false, ToneRange::Shadows) => v + 0.5 * k * (1.0 - v) * (1.0 - v),
        (false, ToneRange::Midtones) => v.powf(1.0 / (1.0 + k)),
        (false, ToneRange::Highlights) => v * (1.0 + k),
        (true, ToneRange::Shadows) => v - k * (1.0 - v),
        (true, ToneRange::Midtones) => v.powf(1.0 + k),
        (true, ToneRange::Highlights) => v - 0.5 * k * v * v,
    };
    out.clamp(0.0, 1.0)
}

/// Dodge or burn. With `protect` (Protect Tones), the curve applies to the
/// luminance and the color is scaled to it without clipping: hue and
/// saturation are kept. Otherwise each channel follows the curve.
pub fn dodge_burn(rgb: [f32; 3], k: f32, burn: bool, range: ToneRange, protect: bool) -> [f32; 3] {
    if !protect {
        return rgb.map(|v| curve(v, k, burn, range));
    }
    let l = luma(rgb);
    let target = curve(l, k, burn, range);
    if l <= 1e-6 {
        return [target; 3];
    }
    let max = rgb.iter().fold(0.0f32, |m, &v| m.max(v));
    let scale = (target / l).min(if max > 0.0 { 1.0 / max } else { 1.0 });
    rgb.map(|v| (v * scale).clamp(0.0, 1.0))
}

/// Sponge: saturates (or desaturates) by `k` around the luminance. With
/// `vibrance`, colors already saturated change less and none clips.
pub fn sponge(rgb: [f32; 3], k: f32, saturate: bool, vibrance: bool) -> [f32; 3] {
    let l = luma(rgb);
    let (min, max) = rgb
        .iter()
        .fold((1.0f32, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    let mut factor = if saturate {
        let k = if vibrance { k * (1.0 - (max - min)) } else { k };
        1.0 + k
    } else {
        1.0 - k
    };
    if saturate && vibrance {
        // Largest factor keeping every channel within 0..1.
        for v in rgb {
            let d = v - l;
            if d > 1e-6 {
                factor = factor.min((1.0 - l) / d);
            } else if d < -1e-6 {
                factor = factor.min(l / -d);
            }
        }
    }
    rgb.map(|v| (l + (v - l) * factor).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saturation(c: [f32; 3]) -> f32 {
        c.iter().cloned().fold(0.0, f32::max) - c.iter().cloned().fold(1.0, f32::min)
    }

    #[test]
    fn each_range_acts_on_its_tones() {
        for burn in [false, true] {
            let change = |range, v: f32| (curve(v, 0.5, burn, range) - v).abs();
            assert!(change(ToneRange::Shadows, 0.15) > change(ToneRange::Shadows, 0.85));
            assert!(change(ToneRange::Highlights, 0.85) > change(ToneRange::Highlights, 0.15));
            assert!(change(ToneRange::Midtones, 0.5) > change(ToneRange::Midtones, 0.05));
        }
    }

    #[test]
    fn dodge_lightens_and_burn_darkens() {
        for range in ToneRange::ALL {
            for v in [0.2, 0.5, 0.8] {
                assert!(curve(v, 0.5, false, range) >= v, "{range:?} {v}");
                assert!(curve(v, 0.5, true, range) <= v, "{range:?} {v}");
            }
            // No strength, no change.
            assert_eq!(curve(0.4, 0.0, false, range), 0.4);
        }
    }

    #[test]
    fn protect_tones_keeps_the_hue_and_does_not_clip() {
        let orange = [0.9, 0.5, 0.1];
        let plain = dodge_burn(orange, 1.0, false, ToneRange::Highlights, false);
        let protected = dodge_burn(orange, 1.0, false, ToneRange::Highlights, true);
        assert_eq!(plain[0], 1.0, "clipped without protection");
        assert!(protected.iter().all(|&v| v <= 1.0));
        // Same proportions between the channels.
        let ratio = |c: [f32; 3]| c[1] / c[0];
        assert!((ratio(protected) - ratio(orange)).abs() < 1e-4);
        assert!(luma(protected) > luma(orange));
    }

    #[test]
    fn sponge_changes_the_saturation_only() {
        let c = [0.6, 0.4, 0.3];
        let less = sponge(c, 0.5, false, false);
        let more = sponge(c, 0.5, true, false);
        assert!(saturation(less) < saturation(c));
        assert!(saturation(more) > saturation(c));
        assert!((luma(less) - luma(c)).abs() < 1e-4);
        assert!((luma(more) - luma(c)).abs() < 1e-4);
        // Fully desaturated: a grey.
        let grey = sponge(c, 1.0, false, false);
        assert!(saturation(grey) < 1e-5);
    }

    #[test]
    fn vibrance_spares_saturated_colors_and_never_clips() {
        let pale = [0.6, 0.5, 0.45];
        let vivid = [0.9, 0.2, 0.1];
        let gain = |c: [f32; 3]| saturation(sponge(c, 0.5, true, true)) / saturation(c);
        assert!(gain(pale) > gain(vivid));
        let out = sponge(vivid, 1.0, true, true);
        assert!(out.iter().all(|&v| (0.0..=1.0).contains(&v)));
        assert!(out[1] > 0.0 && out[2] > 0.0, "no channel clipped: {out:?}");
    }
}
