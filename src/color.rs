//! Color models of the Color Picker: HSB and Lab from sRGB values, and hex
//! codes. RGB components are 0..1 floats (0..255 once rounded).

/// Hue (degrees, 0..360), saturation and brightness (0..1), computed on the
/// sRGB values, like Photoshop's HSB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hsb {
    pub h: f32,
    pub s: f32,
    pub b: f32,
}

pub fn rgb_to_hsb([r, g, b]: [f32; 3]) -> Hsb {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let s = if max > 0.0 { delta / max } else { 0.0 };
    Hsb {
        h: h % 360.0,
        s,
        b: max,
    }
}

pub fn hsb_to_rgb(Hsb { h, s, b }: Hsb) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = b * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, bl) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = b - c;
    [r + m, g + m, bl + m]
}

pub fn to_u8(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

pub fn to_f32(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|c| f32::from(c) / 255.0)
}

/// D65 white point (XYZ).
const WHITE: [f32; 3] = [0.950_47, 1.0, 1.088_83];

fn to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// CIE L*a*b* (L 0..100, a and b about -128..127) of an sRGB color, D65.
/// Photoshop works in D50: its values differ by a few units.
pub fn rgb_to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(to_linear);
    let xyz = [
        0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b,
        0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b,
        0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b,
    ];
    let f = |t: f32| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let [fx, fy, fz] = [0, 1, 2].map(|i| f(xyz[i] / WHITE[i]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// sRGB color of a Lab one, brought into the sRGB gamut.
pub fn lab_to_rgb([l, a, b]: [f32; 3]) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let (fx, fz) = (fy + a / 500.0, fy - b / 200.0);
    let inv = |t: f32| {
        if t.powi(3) > 216.0 / 24389.0 {
            t.powi(3)
        } else {
            (116.0 * t - 16.0) * 27.0 / 24389.0
        }
    };
    let [x, y, z] = [inv(fx), inv(fy), inv(fz)];
    let [x, y, z] = [x * WHITE[0], y * WHITE[1], z * WHITE[2]];
    let rgb = [
        3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
        -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z,
        0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
    ];
    rgb.map(|c| from_linear(c.clamp(0.0, 1.0)))
}

/// Six lowercase hex digits, without "#": "ff8800".
pub fn hex([r, g, b]: [u8; 3]) -> String {
    format!("{r:02x}{g:02x}{b:02x}")
}

/// A typed or pasted hex code: 3 or 6 digits, any case, with or without
/// "#" ("f80", "#FF8800", " ff8800 ").
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.trim().trim_start_matches('#');
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = |s: &str| u8::from_str_radix(s, 16).ok();
    match digits.len() {
        3 => {
            let mut rgb = [0; 3];
            for (c, d) in rgb.iter_mut().zip(digits.chars()) {
                *c = value(&d.to_string())? * 17;
            }
            Some(rgb)
        }
        6 => Some([
            value(&digits[0..2])?,
            value(&digits[2..4])?,
            value(&digits[4..6])?,
        ]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsb_of_known_colors() {
        let hsb = |rgb| {
            let c = rgb_to_hsb(to_f32(rgb));
            [c.h.round(), (c.s * 100.0).round(), (c.b * 100.0).round()]
        };
        assert_eq!(hsb([255, 0, 0]), [0.0, 100.0, 100.0]);
        assert_eq!(hsb([0, 255, 0]), [120.0, 100.0, 100.0]);
        assert_eq!(hsb([0, 0, 255]), [240.0, 100.0, 100.0]);
        assert_eq!(hsb([255, 0, 255]), [300.0, 100.0, 100.0]);
        assert_eq!(hsb([128, 128, 128]), [0.0, 0.0, 50.0]);
        assert_eq!(hsb([255, 128, 0]), [30.0, 100.0, 100.0]);
        assert_eq!(hsb([0, 0, 0]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn rgb_hsb_round_trip_is_exact_on_bytes() {
        for r in (0..=255).step_by(15) {
            for g in (0..=255).step_by(17) {
                for b in (0..=255).step_by(51) {
                    let rgb = [r as u8, g as u8, b as u8];
                    let back = to_u8(hsb_to_rgb(rgb_to_hsb(to_f32(rgb))));
                    assert_eq!(back, rgb);
                }
            }
        }
    }

    #[test]
    fn hue_wraps() {
        let red = Hsb {
            h: 360.0,
            s: 1.0,
            b: 1.0,
        };
        assert_eq!(to_u8(hsb_to_rgb(red)), [255, 0, 0]);
        let h = Hsb { h: -60.0, ..red };
        assert_eq!(to_u8(hsb_to_rgb(h)), [255, 0, 255]);
    }

    #[test]
    fn lab_of_known_colors() {
        let lab = |rgb| rgb_to_lab(to_f32(rgb)).map(f32::round);
        assert_eq!(lab([255, 255, 255]), [100.0, 0.0, 0.0]);
        assert_eq!(lab([0, 0, 0]), [0.0, 0.0, 0.0]);
        assert_eq!(lab([255, 0, 0]), [53.0, 80.0, 67.0]);
        assert_eq!(lab([0, 0, 255]), [32.0, 79.0, -108.0]);
        assert_eq!(lab([128, 128, 128])[0], 54.0);
    }

    #[test]
    fn rgb_lab_round_trip() {
        for rgb in [[255, 0, 0], [12, 200, 90], [128, 128, 128], [3, 7, 250]] {
            let back = to_u8(lab_to_rgb(rgb_to_lab(to_f32(rgb))));
            for (a, b) in back.iter().zip(rgb) {
                assert!(a.abs_diff(b) <= 1, "{rgb:?} → {back:?}");
            }
        }
        // Out of gamut: brought back into it.
        let rgb = lab_to_rgb([50.0, 127.0, -128.0]);
        assert!(rgb.iter().all(|c| (0.0..=1.0).contains(c)));
    }

    #[test]
    fn hex_codes() {
        assert_eq!(hex([255, 136, 0]), "ff8800");
        assert_eq!(parse_hex("ff8800"), Some([255, 136, 0]));
        assert_eq!(parse_hex("#FF8800"), Some([255, 136, 0]));
        assert_eq!(parse_hex("  #f80 "), Some([255, 136, 0]));
        assert_eq!(parse_hex("F80"), Some([255, 136, 0]));
        for bad in ["", "#", "ff88", "ff88001", "gg8800", "#ff 880"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
        for rgb in [[0, 0, 0], [1, 2, 3], [255, 255, 255]] {
            assert_eq!(parse_hex(&hex(rgb)), Some(rgb));
        }
    }
}
