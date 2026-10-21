//! The development as a whole: neutral at the defaults, each slider's
//! direction, white balance, highlight recovery, RAW files.

use image::{Rgba, RgbaImage};

use super::raw::{SRGB_CAMERA, TestDng};
use super::settings::Wheel;
use super::*;

/// A test chart: a gray ramp on top, saturated colors below, a fine
/// texture at the bottom.
fn chart() -> RgbaImage {
    RgbaImage::from_fn(96, 64, |x, y| {
        if y < 24 {
            let v = (x * 255 / 95) as u8;
            Rgba([v, v, v, 255])
        } else if y < 44 {
            let colors = [
                [200, 40, 40],
                [220, 140, 40],
                [210, 200, 50],
                [50, 170, 60],
                [40, 160, 190],
                [50, 70, 200],
                [140, 60, 190],
                [200, 50, 160],
            ];
            let [r, g, b] = colors[(x / 12) as usize];
            Rgba([r, g, b, 255])
        } else {
            let v = if (x + y) % 2 == 0 { 110 } else { 150 };
            Rgba([v, v, v, 255])
        }
    })
}

fn mean(image: &RgbaImage, area: [u32; 4], f: impl Fn([f32; 3]) -> f32) -> f32 {
    let [x0, y0, x1, y1] = area;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = image.get_pixel(x, y).0;
            sum += f([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    sum / ((x1 - x0) * (y1 - y0)) as f32
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn chroma(p: [f32; 3]) -> f32 {
    p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2])
}

/// Mean absolute difference between neighbors (local contrast).
fn texture(image: &RgbaImage, area: [u32; 4]) -> f32 {
    let [x0, y0, x1, y1] = area;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 - 1 {
            sum += (luma(px(image, x, y)) - luma(px(image, x + 1, y))).abs();
        }
    }
    sum / ((x1 - x0 - 1) * (y1 - y0)) as f32
}

fn px(image: &RgbaImage, x: u32, y: u32) -> [f32; 3] {
    let p = image.get_pixel(x, y).0;
    [p[0] as f32, p[1] as f32, p[2] as f32]
}

const SHADOWS: [u32; 4] = [4, 0, 24, 24];
const HIGHLIGHTS: [u32; 4] = [72, 0, 92, 24];
const MIDTONES: [u32; 4] = [38, 0, 58, 24];
const ALL_GRAYS: [u32; 4] = [0, 0, 96, 24];
const COLORS: [u32; 4] = [0, 24, 96, 44];
const REDS: [u32; 4] = [0, 24, 12, 44];
const TEXTURE: [u32; 4] = [0, 44, 96, 64];

#[test]
fn the_defaults_change_nothing() {
    let image = chart();
    assert_eq!(apply(&image, &Settings::default()), image);
    // Transparency is kept.
    let mut clear = image.clone();
    clear.put_pixel(3, 3, Rgba([10, 20, 30, 40]));
    assert_eq!(apply(&clear, &Settings::default()), clear);
}

/// Each slider, moved one way: what it must increase.
#[test]
fn each_slider_moves_the_right_way() {
    let image = chart();
    let lum = |a| move |i: &RgbaImage| mean(i, a, luma);
    let tex = |a| move |i: &RgbaImage| texture(i, a);
    let chroma_of = |a| move |i: &RgbaImage| mean(i, a, chroma);
    let spread = |i: &RgbaImage| mean(i, HIGHLIGHTS, luma) - mean(i, SHADOWS, luma);
    let warmth = |i: &RgbaImage| mean(i, ALL_GRAYS, |p| p[0] - p[2]);
    let magenta = |i: &RgbaImage| mean(i, ALL_GRAYS, |p| (p[0] + p[2]) / 2.0 - p[1]);
    let neg = |f: Box<dyn Fn(&RgbaImage) -> f32>| move |i: &RgbaImage| -f(i);
    type Metric = Box<dyn Fn(&RgbaImage) -> f32>;
    type Case = (&'static str, Box<dyn Fn(&mut Settings)>, Metric);
    let cases: Vec<Case> = vec![
        (
            "exposure",
            Box::new(|s| s.exposure = 1.0),
            Box::new(lum(ALL_GRAYS)),
        ),
        (
            "contrast",
            Box::new(|s| s.contrast = 50.0),
            Box::new(spread),
        ),
        (
            "highlights",
            Box::new(|s| s.highlights = 60.0),
            Box::new(lum(HIGHLIGHTS)),
        ),
        (
            "shadows",
            Box::new(|s| s.shadows = 60.0),
            Box::new(lum(SHADOWS)),
        ),
        (
            "whites",
            Box::new(|s| s.whites = 60.0),
            Box::new(lum(HIGHLIGHTS)),
        ),
        (
            "blacks",
            Box::new(|s| s.blacks = 60.0),
            Box::new(lum(SHADOWS)),
        ),
        (
            "texture",
            Box::new(|s| s.texture = 80.0),
            Box::new(tex(TEXTURE)),
        ),
        (
            "clarity",
            Box::new(|s| s.clarity = 80.0),
            Box::new(tex(TEXTURE)),
        ),
        (
            "dehaze",
            Box::new(|s| s.dehaze = 60.0),
            Box::new(neg(Box::new(lum(SHADOWS)))),
        ),
        (
            "vibrance",
            Box::new(|s| s.vibrance = 60.0),
            Box::new(chroma_of(COLORS)),
        ),
        (
            "saturation",
            Box::new(|s| s.saturation = 60.0),
            Box::new(chroma_of(COLORS)),
        ),
        (
            "temperature",
            Box::new(|s| s.temperature = 40.0),
            Box::new(warmth),
        ),
        ("tint", Box::new(|s| s.tint = 40.0), Box::new(magenta)),
        (
            "curve highlights",
            Box::new(|s| s.curve.highlights = 60.0),
            Box::new(lum(HIGHLIGHTS)),
        ),
        (
            "curve lights",
            Box::new(|s| s.curve.lights = 60.0),
            Box::new(lum([58, 0, 76, 24])),
        ),
        (
            "curve darks",
            Box::new(|s| s.curve.darks = 60.0),
            Box::new(lum([24, 0, 44, 24])),
        ),
        (
            "curve shadows",
            Box::new(|s| s.curve.shadows = 60.0),
            Box::new(lum(SHADOWS)),
        ),
        (
            "point curve",
            Box::new(|s| s.curve.points[0] = vec![[0.0, 0.0], [128.0, 170.0], [255.0, 255.0]]),
            Box::new(lum(MIDTONES)),
        ),
        (
            "red curve",
            Box::new(|s| s.curve.points[1] = vec![[0.0, 0.0], [128.0, 170.0], [255.0, 255.0]]),
            Box::new(warmth),
        ),
        (
            "red saturation",
            Box::new(|s| s.mixer.saturation[0] = -80.0),
            Box::new(neg(Box::new(chroma_of(REDS)))),
        ),
        (
            "red luminance",
            Box::new(|s| s.mixer.luminance[0] = 80.0),
            Box::new(lum(REDS)),
        ),
        (
            "red hue",
            Box::new(|s| s.mixer.hue[0] = 100.0),
            Box::new(|i: &RgbaImage| mean(i, REDS, |p| p[1] - p[2])),
        ),
        (
            "grading shadows",
            Box::new(|s| {
                s.grading.shadows = Wheel {
                    hue: 240.0,
                    saturation: 60.0,
                    luminance: 0.0,
                }
            }),
            Box::new(|i: &RgbaImage| mean(i, SHADOWS, |p| p[2] - p[0])),
        ),
        (
            "grading highlights",
            Box::new(|s| {
                s.grading.highlights = Wheel {
                    hue: 30.0,
                    saturation: 60.0,
                    luminance: 0.0,
                }
            }),
            Box::new(|i: &RgbaImage| mean(i, HIGHLIGHTS, |p| p[0] - p[2])),
        ),
        (
            "sharpening",
            Box::new(|s| s.detail.sharpen_amount = 100.0),
            Box::new(tex(TEXTURE)),
        ),
        (
            "luminance noise reduction",
            Box::new(|s| s.detail.luminance = 100.0),
            Box::new(neg(Box::new(tex(TEXTURE)))),
        ),
        (
            "vignetting",
            Box::new(|s| s.effects.vignette = -80.0),
            Box::new(neg(Box::new(lum([0, 0, 8, 8])))),
        ),
        (
            "grain",
            Box::new(|s| s.effects.grain = 80.0),
            Box::new(tex([10, 0, 30, 20])),
        ),
        (
            "lens vignetting",
            Box::new(|s| s.lens.vignetting = 80.0),
            Box::new(lum([0, 0, 8, 8])),
        ),
    ];
    let before = apply(&image, &Settings::default());
    for (name, change, metric) in cases {
        let mut s = Settings::default();
        change(&mut s);
        let after = apply(&image, &s);
        let (m0, m1) = (metric(&before), metric(&after));
        assert!(m1 > m0 + 0.5, "{name}: {m0} → {m1}");
    }
}

#[test]
fn negative_values_go_the_other_way() {
    let image = chart();
    for (name, change, area) in [
        (
            "exposure",
            (|s: &mut Settings| s.exposure = -1.0) as fn(&mut Settings),
            ALL_GRAYS,
        ),
        ("highlights", |s| s.highlights = -60.0, HIGHLIGHTS),
        ("shadows", |s| s.shadows = -60.0, SHADOWS),
        ("whites", |s| s.whites = -60.0, HIGHLIGHTS),
        ("blacks", |s| s.blacks = -60.0, [8, 0, 20, 24]),
        ("dehaze", |s| s.dehaze = -60.0, SHADOWS),
    ] {
        let mut s = Settings::default();
        change(&mut s);
        let (m0, m1) = (
            mean(&image, area, luma),
            mean(&apply(&image, &s), area, luma),
        );
        // Negative dehaze adds haze: the shadows lighten.
        if name == "dehaze" {
            assert!(m1 > m0 + 0.5, "{name}: {m0} → {m1}");
        } else {
            assert!(m1 < m0 - 0.5, "{name}: {m0} → {m1}");
        }
    }
    let s = Settings {
        saturation: -100.0,
        ..Settings::default()
    };
    let gray = apply(&image, &s);
    assert!(mean(&gray, COLORS, chroma) < 1.5);
}

#[test]
fn monochrome_is_gray_and_its_mix_follows_the_hues() {
    let image = chart();
    let mut s = Settings {
        profile: Profile::Monochrome,
        ..Settings::default()
    };
    let gray = apply(&image, &s);
    assert!(mean(&gray, COLORS, chroma) < 0.5);
    let reds = mean(&gray, REDS, luma);
    s.mixer.gray[0] = 80.0;
    assert!(mean(&apply(&image, &s), REDS, luma) > reds + 5.0);
}

#[test]
fn the_white_balance_tool_makes_a_gray_card_neutral() {
    // A warm-lit gray card: it is the white balance tool's target.
    let card = RgbaImage::from_pixel(32, 32, Rgba([150, 120, 90, 255]));
    let source = Source::from_rgba(&card);
    let (temperature, tint) = source.neutral_balance(source.rgb[0]);
    assert!(temperature < 0.0, "a warm card asks for a cooler balance");
    let s = Settings {
        white_balance: WhiteBalance::Custom,
        temperature,
        tint,
        ..Settings::default()
    };
    let out = apply(&card, &s).get_pixel(10, 10).0;
    let spread = out[0].max(out[1]).max(out[2]) - out[0].min(out[1]).min(out[2]);
    assert!(spread <= 2, "{out:?}");
    // Auto finds the same light on an image of that card.
    let analysis = Analysis::of(&source);
    assert!((analysis.auto_balance.0 - temperature).abs() < 1.0);
}

/// Highlight recovery is local: the details of the highlights keep their
/// contrast (a global curve pinned at white would flatten them), and on a
/// RAW file the light above white comes back.
#[test]
fn highlights_are_recovered_with_their_details() {
    // Bright texture (levels 225 / 245) on the left, dark on the right.
    let image = RgbaImage::from_fn(120, 80, |x, y| {
        let v = if x < 60 {
            if (x + y) % 2 == 0 { 225 } else { 245 }
        } else {
            40
        };
        Rgba([v, v, v, 255])
    });
    let s = Settings {
        highlights: -100.0,
        ..Settings::default()
    };
    let out = apply(&image, &s);
    let area = [10, 10, 50, 70];
    let (m0, m1) = (mean(&image, area, luma), mean(&out, area, luma));
    assert!(m1 < m0 - 25.0, "highlights darker: {m0} → {m1}");
    // In linear light, the ratio between the texture's two levels holds.
    let lin = |v: f32| source::decode_level(v.round().clamp(0.0, 255.0) as u8);
    let ratio = |i: &RgbaImage| {
        let (a, b) = (px(i, 20, 20)[0], px(i, 21, 20)[0]);
        (lin(a.max(b)) / lin(a.min(b))).log2()
    };
    let (r0, r1) = (ratio(&image), ratio(&out));
    assert!(r1 > r0 * 0.85, "details kept: {r0} → {r1}");
    // The dark side barely changes.
    assert!((mean(&out, [80, 10, 110, 70], luma) - 40.0).abs() < 3.0);

    // Over-exposed RAW data: values above white roll off by default, and
    // Highlights −100 brings their detail back down into the visible range.
    let source = Source {
        width: 64,
        height: 32,
        rgb: (0..64 * 32)
            .map(|i| {
                let v = if i % 2 == 0 { 2.0 } else { 3.0 };
                [v; 3]
            })
            .collect(),
        alpha: None,
        camera: Some(std::sync::Arc::new(srgb_camera([1.0, 1.0, 1.0]))),
    };
    let analysis = Analysis::of(&source);
    let frame = Frame::whole(&source);
    let mut s = Settings::for_source(&source);
    s.detail = Default::default();
    let shown = render(&source, &s, &frame, &analysis, Visual::Image, None);
    s.highlights = -100.0;
    s.exposure = -0.5;
    let recovered = render(&source, &s, &frame, &analysis, Visual::Image, None);
    let all = [0, 0, 64, 32];
    assert!(texture(&recovered, all) > texture(&shown, all) * 3.0);
    assert!(mean(&recovered, all, luma) < 235.0);
}

/// A camera that sees linear sRGB, white balance `as_shot`.
fn srgb_camera(as_shot: [f64; 3]) -> source::Camera {
    let m = SRGB_CAMERA;
    source::Camera {
        calibrations: vec![source::Calibration {
            temperature: 6504.0,
            color_matrix: std::array::from_fn(|i| std::array::from_fn(|j| m[i * 3 + j])),
            forward_matrix: None,
            camera_calibration: white::IDENTITY,
        }],
        analog_balance: [1.0; 3],
        as_shot,
        baseline_exposure: 0.0,
    }
}

#[test]
fn raw_white_balance_follows_kelvins_and_the_tool() {
    // A gray card under a warm light: the camera recorded it warm, and the
    // white balance as shot is of a neutral (here D65-like) light.
    let camera = srgb_camera([1.0, 1.0, 1.0]);
    let (t, tint) = camera.as_shot_temperature();
    // D65: 6504 K, a little above the Planckian locus (+10, like daylight).
    assert!(
        (t - 6504.0).abs() < 60.0 && (tint - 10.0).abs() < 3.0,
        "{t} {tint}"
    );
    let card = [0.5, 0.38, 0.25];
    let source = Source {
        width: 8,
        height: 8,
        rgb: vec![card; 64],
        alpha: None,
        camera: Some(std::sync::Arc::new(camera)),
    };
    let analysis = Analysis::of(&source);
    let frame = Frame::whole(&source);
    let mut s = Settings::for_source(&source);
    s.detail = Default::default();
    let at = |s: &Settings| {
        render(&source, s, &frame, &analysis, Visual::Image, None)
            .get_pixel(3, 3)
            .0
    };
    let shot = at(&s);
    assert!(shot[0] > shot[2] + 20, "warm as shot: {shot:?}");
    // A lower temperature (tungsten) cools the image.
    s.temperature = 3000.0;
    let cool = at(&s);
    assert!(cool[2] as i32 - cool[0] as i32 > shot[2] as i32 - shot[0] as i32 + 20);
    // The tool on the card: neutral.
    (s.temperature, s.tint) = source.neutral_balance(card);
    let neutral = at(&s);
    let spread = neutral.iter().take(3).max().unwrap() - neutral.iter().take(3).min().unwrap();
    assert!(spread <= 2, "{neutral:?} at {} K", s.temperature);
}

#[test]
fn a_dng_opens_neutral_and_with_its_highlight_data() {
    // Gray card mosaic, camera white balance as shot (0.5, 1, 0.8).
    let (w, h) = (24u32, 16u32);
    let as_shot = [0.5, 1.0, 0.8];
    let pattern = [0u8, 1, 1, 2];
    let level =
        |c: usize, gain: f64| (512.0 + (as_shot[c] * 0.18 * gain * 15000.0).min(15000.0)) as u16;
    let samples = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            // Left: an 18 % gray; right: twice as bright, the green clipped.
            let gain = if x < w / 2 { 1.0 } else { 8.0 };
            level(pattern[((y % 2) * 2 + x % 2) as usize] as usize, gain)
        })
        .collect();
    let dng = TestDng {
        width: w,
        height: h,
        samples,
        linear: false,
        lossless_jpeg: true,
        black: 512,
        white: 15512,
        pattern,
        color_matrix: SRGB_CAMERA,
        as_shot,
        orientation: 1,
    };
    let dir = std::env::temp_dir().join("atelier-raw-dng");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("card.dng");
    std::fs::write(&path, dng.bytes()).unwrap();
    assert!(raw::is_raw(&path));
    let file = raw::read(&path).expect("read");
    assert!(!file.preview_only);
    let source = file.source;
    let analysis = Analysis::of(&source);
    let mut s = Settings::for_source(&source);
    s.detail = Default::default();
    let image = render(
        &source,
        &s,
        &Frame::whole(&source),
        &analysis,
        Visual::Image,
        None,
    );
    let gray = image.get_pixel(5, 8).0;
    let spread = gray.iter().take(3).max().unwrap() - gray.iter().take(3).min().unwrap();
    assert!(
        spread <= 3 && gray[1] > 60 && gray[1] < 180,
        "neutral gray: {gray:?}"
    );
    // The clipped part is white, not magenta.
    let bright = image.get_pixel(18, 8).0;
    assert!(bright[1] >= bright[0].min(bright[2]) - 3, "{bright:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A reduced preview looks like the full-resolution result reduced.
#[test]
fn previews_match_the_full_resolution() {
    let image = RgbaImage::from_fn(240, 160, |x, y| {
        let v =
            ((x as f32 / 240.0) * 200.0 + if (x / 6 + y / 6) % 2 == 0 { 30.0 } else { 0.0 }) as u8;
        Rgba([v, v / 2 + 40, 255 - v, 255])
    });
    let s = Settings {
        highlights: -60.0,
        shadows: 50.0,
        clarity: 40.0,
        dehaze: 20.0,
        effects: settings::Effects {
            vignette: -50.0,
            ..Default::default()
        },
        ..Settings::default()
    };
    let source = Source::from_rgba(&image);
    let analysis = Analysis::of(&source);
    let full = render(
        &source,
        &s,
        &Frame::whole(&source),
        &analysis,
        Visual::Image,
        None,
    );
    let small_source = source.downscaled(0.25);
    let frame = Frame {
        size: (small_source.width, small_source.height),
        origin: (0, 0),
        scale: 0.25,
    };
    let small = render(&small_source, &s, &frame, &analysis, Visual::Image, None);
    let reduced = image::imageops::resize(&full, 60, 40, image::imageops::FilterType::Triangle);
    let diff = mean_abs_diff(&small, &reduced);
    assert!(diff < 6.0, "{diff}");
    // A part with its surroundings develops like the whole.
    let part = source.crop([60, 40, 180, 120]);
    let frame = Frame {
        size: (240, 160),
        origin: (60, 40),
        scale: 1.0,
    };
    let developed = render(&part, &s, &frame, &analysis, Visual::Image, None);
    let same = image::imageops::crop_imm(&full, 60, 40, 120, 80).to_image();
    let inner = |i: &RgbaImage| image::imageops::crop_imm(i, 20, 20, 80, 40).to_image();
    assert!(mean_abs_diff(&inner(&developed), &inner(&same)) < 3.0);
}

fn mean_abs_diff(a: &RgbaImage, b: &RgbaImage) -> f32 {
    let sum: f32 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| (*x as f32 - *y as f32).abs())
        .sum();
    sum / a.as_raw().len() as f32
}

/// Alt on a sharpening slider: the image in gray, showing what it does.
#[test]
fn sharpening_views_are_gray() {
    let image = chart();
    let source = Source::from_rgba(&image);
    let analysis = Analysis::of(&source);
    let mut s = Settings::default();
    s.detail.sharpen_amount = 80.0;
    s.detail.sharpen_masking = 60.0;
    for visual in [
        Visual::SharpenAmount,
        Visual::SharpenRadius,
        Visual::SharpenDetail,
        Visual::SharpenMasking,
    ] {
        let out = render(&source, &s, &Frame::whole(&source), &analysis, visual, None);
        assert!(out.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]));
    }
    // The mask: the edges of the color bars, not the flat ramp.
    let mask = render(
        &source,
        &s,
        &Frame::whole(&source),
        &analysis,
        Visual::SharpenMasking,
        None,
    );
    assert!(mask.get_pixel(12, 30).0[0] > mask.get_pixel(40, 10).0[0]);
}
