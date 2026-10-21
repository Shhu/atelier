//! White balance: temperature and tint of a light (the DNG way, Robertson's
//! isotemperature lines), chromatic adaptation, and the 3 × 3 matrices of
//! the color pipeline.

use std::ops::RangeInclusive;

pub type Mat3 = [[f64; 3]; 3];

/// Temperature of a RAW file's white balance, in kelvins.
pub const TEMPERATURE_RANGE: RangeInclusive<f32> = 2000.0..=50000.0;
/// Tint of a RAW file's white balance.
pub const TINT_RANGE: RangeInclusive<f32> = -150.0..=150.0;

pub const D50: [f64; 2] = [0.3457, 0.3585];
pub const D65: [f64; 2] = [0.3127, 0.3290];

pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Linear sRGB (D65) to XYZ.
pub const SRGB_TO_XYZ: Mat3 = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];

const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

pub fn apply(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

pub fn diagonal(d: [f64; 3]) -> Mat3 {
    [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]]
}

pub fn scale(m: &Mat3, k: f64) -> Mat3 {
    m.map(|row| row.map(|v| v * k))
}

/// Inverse, or `None` for a singular matrix.
pub fn inverse(m: &Mat3) -> Option<Mat3> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-12 {
        return None;
    }
    let k = 1.0 / det;
    Some([
        [
            (e * i - f * h) * k,
            (c * h - b * i) * k,
            (b * f - c * e) * k,
        ],
        [
            (f * g - d * i) * k,
            (a * i - c * g) * k,
            (c * d - a * f) * k,
        ],
        [
            (d * h - e * g) * k,
            (b * g - a * h) * k,
            (a * e - b * d) * k,
        ],
    ])
}

/// XYZ of a chromaticity, Y = 1.
pub fn xy_to_xyz([x, y]: [f64; 2]) -> [f64; 3] {
    let y = y.max(1e-6);
    [x / y, 1.0, (1.0 - x - y) / y]
}

pub fn xyz_to_xy([x, y, z]: [f64; 3]) -> [f64; 2] {
    let sum = x + y + z;
    if sum <= 1e-12 {
        return D50;
    }
    [x / sum, y / sum]
}

/// XYZ to XYZ: what is white under the light `from` becomes white under
/// `to` (Bradford).
pub fn adaptation(from: [f64; 2], to: [f64; 2]) -> Mat3 {
    let cone = |xy| apply(&BRADFORD, xy_to_xyz(xy));
    let (s, d) = (cone(from), cone(to));
    let gain = diagonal(std::array::from_fn(|i| d[i] / s[i]));
    let back = inverse(&BRADFORD).expect("invertible");
    mul(&back, &mul(&gain, &BRADFORD))
}

/// XYZ under D50 (the DNG connection space) to linear sRGB.
pub fn xyz_d50_to_srgb() -> Mat3 {
    let to_srgb = inverse(&SRGB_TO_XYZ).expect("invertible");
    mul(&to_srgb, &adaptation(D50, D65))
}

/// Robertson's isotemperature lines: reciprocal temperature (mired), u, v
/// of the Planckian locus, slope.
const ROBERTSON: [[f64; 4]; 31] = [
    [0.0, 0.18006, 0.26352, -0.24341],
    [10.0, 0.18066, 0.26589, -0.25479],
    [20.0, 0.18133, 0.26846, -0.26876],
    [30.0, 0.18208, 0.27119, -0.28539],
    [40.0, 0.18293, 0.27407, -0.30470],
    [50.0, 0.18388, 0.27709, -0.32675],
    [60.0, 0.18494, 0.28021, -0.35156],
    [70.0, 0.18611, 0.28342, -0.37915],
    [80.0, 0.18740, 0.28668, -0.40955],
    [90.0, 0.18880, 0.28997, -0.44278],
    [100.0, 0.19032, 0.29326, -0.47888],
    [125.0, 0.19462, 0.30141, -0.58204],
    [150.0, 0.19962, 0.30921, -0.70471],
    [175.0, 0.20525, 0.31647, -0.84901],
    [200.0, 0.21142, 0.32312, -1.0182],
    [225.0, 0.21807, 0.32909, -1.2168],
    [250.0, 0.22511, 0.33439, -1.4512],
    [275.0, 0.23247, 0.33904, -1.7298],
    [300.0, 0.24010, 0.34308, -2.0637],
    [325.0, 0.24702, 0.34655, -2.4681],
    [350.0, 0.25591, 0.34951, -2.9641],
    [375.0, 0.26400, 0.35200, -3.5814],
    [400.0, 0.27218, 0.35407, -4.3633],
    [425.0, 0.28039, 0.35577, -5.3762],
    [450.0, 0.28863, 0.35714, -6.7262],
    [475.0, 0.29685, 0.35823, -8.5955],
    [500.0, 0.30505, 0.35907, -11.324],
    [525.0, 0.31320, 0.35968, -15.628],
    [550.0, 0.32129, 0.36011, -23.325],
    [575.0, 0.32931, 0.36038, -40.770],
    [600.0, 0.33724, 0.36051, -116.45],
];

/// Tint units per unit of distance from the locus (the DNG convention:
/// positive tints are magenta).
const TINT_SCALE: f64 = -3000.0;

/// Unit vector along an isotemperature line.
fn slope_vector(t: f64) -> (f64, f64) {
    let len = (1.0 + t * t).sqrt();
    (1.0 / len, t / len)
}

/// Temperature (K) and tint of a white.
pub fn temperature_of(xy: [f64; 2]) -> (f64, f64) {
    let [x, y] = xy;
    let d = 1.5 - x + 6.0 * y;
    let (u, v) = (2.0 * x / d, 3.0 * y / d);
    let (mut last_dt, mut last_du, mut last_dv) = (0.0, 0.0, 0.0);
    for i in 1..ROBERTSON.len() {
        let (du, dv) = slope_vector(ROBERTSON[i][3]);
        let (uu, vv) = (u - ROBERTSON[i][1], v - ROBERTSON[i][2]);
        let mut dt = -uu * dv + vv * du;
        if dt <= 0.0 || i == ROBERTSON.len() - 1 {
            dt = (-dt).max(0.0);
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            let (a, b) = (&ROBERTSON[i - 1], &ROBERTSON[i]);
            let temperature = 1.0e6 / (a[0] * f + b[0] * (1.0 - f)).max(1e-6);
            let uu = u - (a[1] * f + b[1] * (1.0 - f));
            let vv = v - (a[2] * f + b[2] * (1.0 - f));
            let (du, dv) = (du * (1.0 - f) + last_du * f, dv * (1.0 - f) + last_dv * f);
            let len = (du * du + dv * dv).sqrt();
            let tint = (uu * du + vv * dv) / len * TINT_SCALE;
            return (temperature, tint);
        }
        (last_dt, last_du, last_dv) = (dt, du, dv);
    }
    unreachable!("the last line ends the search")
}

/// White of a temperature (K) and a tint.
pub fn white_of(temperature: f64, tint: f64) -> [f64; 2] {
    let r = 1.0e6 / temperature.max(1.0);
    let offset = tint / TINT_SCALE;
    let last = ROBERTSON.len() - 2;
    for i in 0..=last {
        if r < ROBERTSON[i + 1][0] || i == last {
            let (a, b) = (&ROBERTSON[i], &ROBERTSON[i + 1]);
            let f = (b[0] - r) / (b[0] - a[0]);
            let mut u = a[1] * f + b[1] * (1.0 - f);
            let mut v = a[2] * f + b[2] * (1.0 - f);
            let (u1, v1) = slope_vector(a[3]);
            let (u2, v2) = slope_vector(b[3]);
            let (u3, v3) = (u1 * f + u2 * (1.0 - f), v1 * f + v2 * (1.0 - f));
            let len = (u3 * u3 + v3 * v3).sqrt();
            u += u3 / len * offset;
            v += v3 / len * offset;
            let d = u - 4.0 * v + 2.0;
            return [1.5 * u / d, v / d];
        }
    }
    unreachable!("the last line ends the search")
}

/// Mireds per unit of the relative temperature slider (images that are not
/// RAW files).
const MIRED_PER_UNIT: f64 = 1.2;

/// The light a relative white balance assumes (for images that are not RAW
/// files: 0, 0 is D65, the white of sRGB).
fn assumed_white(temperature: f32, tint: f32) -> [f64; 2] {
    let (t0, tint0) = temperature_of(D65);
    let mired = 1.0e6 / t0 - temperature as f64 * MIRED_PER_UNIT;
    white_of(1.0e6 / mired.max(1.0), tint0 + tint as f64)
}

/// Linear sRGB to linear sRGB: the relative white balance of an image that
/// is not a RAW file.
pub fn relative_balance(temperature: f32, tint: f32) -> Mat3 {
    if temperature == 0.0 && tint == 0.0 {
        return IDENTITY;
    }
    let to_srgb = inverse(&SRGB_TO_XYZ).expect("invertible");
    let adapt = adaptation(assumed_white(temperature, tint), D65);
    mul(&to_srgb, &mul(&adapt, &SRGB_TO_XYZ))
}

/// Relative temperature and tint that make `rgb` (linear sRGB, before the
/// white balance) neutral.
pub fn relative_neutral(rgb: [f64; 3]) -> (f32, f32) {
    let (t0, tint0) = temperature_of(D65);
    let (t, tint) = temperature_of(xyz_to_xy(apply(&SRGB_TO_XYZ, rgb)));
    let temperature = (1.0e6 / t0 - 1.0e6 / t) / MIRED_PER_UNIT;
    (
        (temperature as f32).clamp(-100.0, 100.0),
        ((tint - tint0) as f32).clamp(-100.0, 100.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temperature_and_tint_round_trip() {
        for (t, tint) in [
            (2850.0, 0.0),
            (5000.0, 10.0),
            (6500.0, -20.0),
            (12000.0, 40.0),
        ] {
            let (t2, tint2) = temperature_of(white_of(t, tint));
            assert!((t2 - t).abs() / t < 0.002, "{t} → {t2}");
            assert!((tint2 - tint).abs() < 0.2, "{tint} → {tint2}");
        }
        // Standard lights.
        let (d65, _) = temperature_of(D65);
        assert!((d65 - 6504.0).abs() < 30.0, "{d65}");
        let (d50, _) = temperature_of(D50);
        assert!((d50 - 5003.0).abs() < 30.0, "{d50}");
    }

    #[test]
    fn positive_tint_is_magenta_and_higher_temperature_is_warmer() {
        let gray = [0.2, 0.2, 0.2];
        let warm = apply(&relative_balance(40.0, 0.0), gray);
        assert!(warm[0] > warm[2] * 1.1, "{warm:?}");
        let magenta = apply(&relative_balance(0.0, 40.0), gray);
        assert!(
            magenta[1] < magenta[0] && magenta[1] < magenta[2],
            "{magenta:?}"
        );
        // Neutral at 0, 0.
        assert_eq!(relative_balance(0.0, 0.0), IDENTITY);
    }

    #[test]
    fn relative_neutral_makes_a_color_gray() {
        let card = [0.22, 0.18, 0.14];
        let (t, tint) = relative_neutral(card);
        let out = apply(&relative_balance(t, tint), card);
        assert!(
            (out[0] - out[1]).abs() < 0.003 && (out[2] - out[1]).abs() < 0.003,
            "{out:?}"
        );
    }
}
