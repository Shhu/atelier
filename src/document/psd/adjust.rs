//! Adjustment and fill layers: their tagged blocks both ways.

use super::bytes::{Reader, Result, Writer};
use super::descriptor::{self, Descriptor, Value, color_of, enumerated, percent};
use crate::color::{Hsb, hsb_to_rgb, rgb_to_hsb, to_f32, to_u8};
use crate::compose::gradient::GradientFill;
use crate::document::{Fill, LayerKind};
use crate::paint::gradient::{ColorStop, Gradient, OpacityStop, Shape, StopColor};
use crate::pixel_ops::{
    Adjustment, BlackWhite, BrightnessContrast, ColorBalance, Curve, Curves, GradientMap,
    HueSaturation, Levels, LevelsChannel, MAX_CURVE_POINTS, PhotoFilter, Posterize, Threshold,
    Vibrance,
};

/// Keys of the adjustment and fill layers Atelier doesn't have.
const UNSUPPORTED: [&[u8; 4]; 5] = [b"selc", b"mixr", b"expA", b"clrL", b"PtFl"];

/// The kind of an adjustment or fill layer from its blocks: `None` for
/// other layers, `Some(Err(()))` for those Atelier doesn't have.
#[allow(clippy::result_unit_err)]
pub fn read(blocks: &[([u8; 4], &[u8])]) -> Option<std::result::Result<LayerKind, ()>> {
    let get = |key: &[u8; 4]| blocks.iter().find(|(k, _)| k == key).map(|(_, d)| *d);
    let adjustment = |a: Option<Adjustment>| Some(a.map(LayerKind::Adjustment).ok_or(()));
    let fill = |f: Option<Fill>| Some(f.map(LayerKind::Fill).ok_or(()));
    if let Some(d) = get(b"phfl") {
        return adjustment(photo_filter(d).ok());
    }
    if let Some(d) = get(b"CgEd") {
        return adjustment(brightness_contrast(d).ok());
    }
    for (key, data) in blocks {
        let d = *data;
        match key {
            b"levl" => return adjustment(levels(d).ok()),
            b"curv" => return adjustment(curves(d).ok().flatten()),
            b"hue2" | b"hue " => return adjustment(hue_saturation(d).ok()),
            b"blnc" => return adjustment(color_balance(d).ok()),
            b"brit" => return adjustment(legacy_brightness_contrast(d).ok()),
            b"vibA" => return adjustment(vibrance(d).ok()),
            b"blwh" => return adjustment(black_white(d).ok()),
            b"nvrt" => return adjustment(Some(Adjustment::Invert)),
            b"post" => {
                let levels = Reader::new(d).u16().ok().map(|v| v.clamp(2, 255) as u8);
                return adjustment(
                    levels.map(|levels| Adjustment::Posterize(Posterize { levels })),
                );
            }
            b"thrs" => {
                let level = Reader::new(d).u16().ok().map(|v| v.clamp(1, 255) as u8);
                return adjustment(level.map(|level| Adjustment::Threshold(Threshold { level })));
            }
            b"grdm" => return adjustment(gradient_map(d).ok().flatten()),
            b"SoCo" => {
                let color = versioned(d)
                    .ok()
                    .and_then(|d| d.object(b"Clr ").and_then(color_of));
                return fill(color.map(Fill::Solid));
            }
            b"GdFl" => {
                return fill(
                    versioned(d)
                        .ok()
                        .and_then(|d| gradient_fill(&d))
                        .map(Fill::Gradient),
                );
            }
            k if UNSUPPORTED.contains(&k) => return Some(Err(())),
            _ => {}
        }
    }
    None
}

/// A descriptor after its version number.
fn versioned(data: &[u8]) -> Result<Descriptor> {
    let mut r = Reader::new(data);
    r.u32()?;
    Descriptor::read(&mut r)
}

fn levels(d: &[u8]) -> Result<Adjustment> {
    let mut r = Reader::new(d);
    r.u16()?;
    let mut channel = || -> Result<LevelsChannel> {
        let [in_black, in_white, out_black, out_white, gamma] =
            [r.u16()?, r.u16()?, r.u16()?, r.u16()?, r.u16()?];
        let level = |v: u16| v.min(255) as u8;
        Ok(LevelsChannel {
            in_black: level(in_black),
            in_white: level(in_white),
            gamma: (gamma as f32 / 100.0)
                .clamp(crate::pixel_ops::MIN_GAMMA, crate::pixel_ops::MAX_GAMMA),
            out_black: level(out_black),
            out_white: level(out_white),
        })
    };
    let rgb = channel()?;
    let channels = [channel()?, channel()?, channel()?];
    Ok(Adjustment::Levels(Levels { rgb, channels }))
}

/// Points (input, output) of a curve stored as (output, input) pairs,
/// brought within Atelier's limits.
fn curve(points: &[(u16, u16)]) -> Curve {
    let mut p: Vec<[f32; 2]> = points
        .iter()
        .map(|&(out, input)| [input.min(255) as f32, out.min(255) as f32])
        .collect();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]));
    p.dedup_by(|a, b| a[0] == b[0]);
    // Too many points: evenly chosen ones, the ends kept.
    if p.len() > MAX_CURVE_POINTS {
        let n = p.len();
        p = (0..MAX_CURVE_POINTS)
            .map(|i| p[i * (n - 1) / (MAX_CURVE_POINTS - 1)])
            .collect();
    }
    Curve::from_points(&p).unwrap_or_default()
}

fn curves(d: &[u8]) -> Result<Option<Adjustment>> {
    let mut r = Reader::new(d);
    let is_map = r.u8()? != 0;
    let version = r.u16()?;
    let mask = r.u32()?;
    if is_map {
        return Ok(None);
    }
    let count = if version == 1 {
        mask.count_ones() as usize
    } else {
        mask as usize
    };
    let read_points = |r: &mut Reader| -> Result<Vec<(u16, u16)>> {
        let n = r.u16()? as usize;
        (0..n).map(|_| Ok((r.u16()?, r.u16()?))).collect()
    };
    let mut by_channel: Vec<(u16, Vec<(u16, u16)>)> = Vec::new();
    let bits: Vec<u16> = (0..32).filter(|b| mask & (1 << b) != 0).collect();
    for i in 0..count {
        let points = read_points(&mut r)?;
        by_channel.push((bits.get(i).copied().unwrap_or(i as u16), points));
    }
    // The extra section gives each curve its channel.
    if r.remaining() >= 10 && r.clone().bytes(4)? == b"Crv " {
        r.skip(6)?;
        let n = r.u32()? as usize;
        by_channel.clear();
        for _ in 0..n.min(64) {
            let id = r.u16()?;
            by_channel.push((id, read_points(&mut r)?));
        }
    }
    let mut cv = Curves::default();
    for (id, points) in by_channel {
        match id {
            0 => cv.rgb = curve(&points),
            1..=3 => cv.channels[id as usize - 1] = curve(&points),
            _ => {}
        }
    }
    Ok(Some(Adjustment::Curves(cv)))
}

fn hue_saturation(d: &[u8]) -> Result<Adjustment> {
    let mut r = Reader::new(d);
    r.u16()?;
    let colorize = r.u8()? != 0;
    r.u8()?;
    let colorization = [r.i16()?, r.i16()?, r.i16()?];
    let master = [r.i16()?, r.i16()?, r.i16()?];
    let unit = |v: i16| (v as f32 / 100.0).clamp(-1.0, 1.0);
    Ok(Adjustment::HueSaturation(if colorize {
        let [h, s, l] = colorization;
        HueSaturation {
            hue: (h as f32).rem_euclid(360.0),
            saturation: unit(s).max(0.0),
            lightness: unit(l),
            colorize,
        }
    } else {
        let [h, s, l] = master;
        HueSaturation {
            hue: (h as f32).clamp(-180.0, 180.0),
            saturation: unit(s),
            lightness: unit(l),
            colorize,
        }
    }))
}

fn color_balance(d: &[u8]) -> Result<Adjustment> {
    let mut r = Reader::new(d);
    let mut three = || -> Result<[f32; 3]> {
        Ok([r.i16()?, r.i16()?, r.i16()?].map(|v| (v as f32 / 100.0).clamp(-1.0, 1.0)))
    };
    let (shadows, midtones, highlights) = (three()?, three()?, three()?);
    Ok(Adjustment::ColorBalance(ColorBalance {
        shadows,
        midtones,
        highlights,
        preserve_luminosity: r.u8()? != 0,
    }))
}

fn bc(brightness: f64, contrast: f64) -> Adjustment {
    let clamp =
        |v: f64, r: std::ops::RangeInclusive<f32>| (v as f32 / 100.0).clamp(*r.start(), *r.end());
    Adjustment::BrightnessContrast(BrightnessContrast {
        brightness: clamp(brightness, BrightnessContrast::BRIGHTNESS),
        contrast: clamp(contrast, BrightnessContrast::CONTRAST),
    })
}

fn brightness_contrast(d: &[u8]) -> Result<Adjustment> {
    let d = versioned(d)?;
    Ok(bc(
        d.num(b"Brgh").unwrap_or(0.0),
        d.num(b"Cntr").unwrap_or(0.0),
    ))
}

fn legacy_brightness_contrast(d: &[u8]) -> Result<Adjustment> {
    let mut r = Reader::new(d);
    Ok(bc(r.i16()? as f64, r.i16()? as f64))
}

fn vibrance(d: &[u8]) -> Result<Adjustment> {
    let d = versioned(d)?;
    let unit = |k: &[u8]| (d.num(k).unwrap_or(0.0) as f32 / 100.0).clamp(-1.0, 1.0);
    Ok(Adjustment::Vibrance(Vibrance {
        vibrance: unit(b"vibrance"),
        saturation: unit(b"Strt"),
    }))
}

/// Keys of the six weights of Black & White, in Atelier's order.
const BW_KEYS: [&[u8; 4]; 6] = [b"Rd  ", b"Yllw", b"Grn ", b"Cyn ", b"Bl  ", b"Mgnt"];

fn black_white(d: &[u8]) -> Result<Adjustment> {
    let d = versioned(d)?;
    let default = BlackWhite::default();
    let weights = std::array::from_fn(|i| {
        d.num(BW_KEYS[i])
            .map_or(default.weights[i], |v| (v as f32).clamp(-200.0, 300.0))
    });
    let (hue, saturation) = d
        .object(b"tintColor")
        .and_then(color_of)
        .map_or((default.hue, default.saturation), hue_and_saturation);
    Ok(Adjustment::BlackWhite(BlackWhite {
        weights,
        tint: d.bool(b"useTint").unwrap_or(false),
        hue,
        saturation,
    }))
}

/// HSL hue and saturation of a color (Black & White's tint).
fn hue_and_saturation(rgb: [u8; 3]) -> (f32, f32) {
    let c = to_f32(rgb);
    let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
    let l = (max + min) / 2.0;
    let s = if max == min {
        0.0
    } else {
        (max - min) / (1.0 - (2.0 * l - 1.0).abs())
    };
    (rgb_to_hsb(c).h, s.clamp(0.0, 1.0))
}

/// The color of a hue and HSL saturation at mid lightness.
fn tint_color(hue: f32, saturation: f32) -> [u8; 3] {
    let max = 0.5 + saturation / 2.0;
    to_u8(hsb_to_rgb(Hsb {
        h: hue,
        s: (saturation / max).clamp(0.0, 1.0),
        b: max,
    }))
}

fn photo_filter(d: &[u8]) -> Result<Adjustment> {
    let mut r = Reader::new(d);
    let color = match r.u16()? {
        3 => {
            // XYZ, 0..? as 16.16 fixed point: through Lab is not worth it,
            // keep the default color.
            r.skip(12)?;
            PhotoFilter::default().color
        }
        _ => {
            let space = r.u16()?;
            let c = [r.u16()?, r.u16()?, r.u16()?, r.u16()?];
            ps_color(space, c)
        }
    };
    let density = (r.u32()? as f32 / 100.0).clamp(0.0, 1.0);
    let preserve_luminosity = r.u8()? != 0;
    Ok(Adjustment::PhotoFilter(PhotoFilter {
        color,
        density,
        preserve_luminosity,
    }))
}

/// A color of Photoshop's color structure (space, four components).
fn ps_color(space: u16, c: [u16; 4]) -> [u8; 3] {
    let to8 = |v: u16| ((v as u32 + 128) / 257) as u8;
    match space {
        0 => [to8(c[0]), to8(c[1]), to8(c[2])],
        1 => to_u8(hsb_to_rgb(Hsb {
            h: c[0] as f32 / 65535.0 * 360.0,
            s: c[1] as f32 / 65535.0,
            b: c[2] as f32 / 65535.0,
        })),
        2 => {
            let ink = |v: u16| 1.0 - v as f32 / 65535.0;
            let k = ink(c[3]);
            [c[0], c[1], c[2]].map(|v| ((1.0 - ink(v)) * (1.0 - k) * 255.0).round() as u8)
        }
        7 => {
            let l = c[0] as f32 / 100.0;
            let (a, b) = (c[1] as i16 as f32 / 100.0, c[2] as i16 as f32 / 100.0);
            to_u8(crate::color::lab_to_rgb([l, a, b]))
        }
        8 => [255 - ((c[0] as u32 * 255 / 10000).min(255) as u8); 3],
        _ => [0; 3],
    }
}

fn gradient_map(d: &[u8]) -> Result<Option<Adjustment>> {
    let mut r = Reader::new(d);
    let version = r.u16()?;
    let reverse = r.u8()? != 0;
    r.u8()?;
    if version == 3 {
        r.skip(4)?;
    }
    r.unicode()?;
    let n = r.u16()? as usize;
    let mut colors = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let location = r.u32()?;
        let midpoint = r.u32()?;
        let space = r.u16()?;
        let c = [r.u16()?, r.u16()?, r.u16()?, r.u16()?];
        r.skip(2)?;
        colors.push(ColorStop {
            location: (location as f32 / 4096.0).clamp(0.0, 1.0),
            midpoint: (midpoint as f32 / 100.0).clamp(0.0, 1.0),
            color: StopColor::Rgb(ps_color(space, c)),
        });
    }
    let n = r.u16()? as usize;
    let mut opacities = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        let location = r.u32()?;
        let midpoint = r.u32()?;
        let opacity = r.u16()?;
        opacities.push(OpacityStop {
            location: (location as f32 / 4096.0).clamp(0.0, 1.0),
            midpoint: (midpoint as f32 / 100.0).clamp(0.0, 1.0),
            opacity: (opacity as f32 / 255.0).clamp(0.0, 1.0),
        });
    }
    // Noise gradients (color model) are not stops: left out.
    if colors.len() < 2 {
        return Ok(None);
    }
    if opacities.len() < 2 {
        opacities = Gradient::default().opacities;
    }
    Ok(Some(Adjustment::GradientMap(GradientMap {
        gradient: Gradient { colors, opacities },
        reverse,
    })))
}

/// A gradient of a descriptor ("Grdn" class).
pub fn gradient_of(d: &Descriptor) -> Option<Gradient> {
    if d.enumerated(b"GrdF").is_some_and(|f| f == b"ClNs") {
        return None;
    }
    let location = |d: &Descriptor| (d.num(b"Lctn").unwrap_or(0.0) as f32 / 4096.0).clamp(0.0, 1.0);
    let midpoint = |d: &Descriptor| (d.num(b"Mdpn").unwrap_or(50.0) as f32 / 100.0).clamp(0.0, 1.0);
    let objects = |key: &[u8]| -> Vec<&Descriptor> {
        d.list(key)
            .unwrap_or_default()
            .iter()
            .filter_map(|v| match v {
                Value::Object(o) => Some(o),
                _ => None,
            })
            .collect()
    };
    let colors: Vec<ColorStop> = objects(b"Clrs")
        .into_iter()
        .map(|s| {
            let fixed = s.object(b"Clr ").and_then(color_of);
            let color = match s.enumerated(b"Type") {
                Some(b"BckC") => fixed.unwrap_or([255; 3]),
                _ => fixed.unwrap_or([0; 3]),
            };
            ColorStop {
                location: location(s),
                midpoint: midpoint(s),
                color: StopColor::Rgb(color),
            }
        })
        .collect();
    let mut opacities: Vec<OpacityStop> = objects(b"Trns")
        .into_iter()
        .map(|s| OpacityStop {
            location: location(s),
            midpoint: midpoint(s),
            opacity: (s.num(b"Opct").unwrap_or(100.0) as f32 / 100.0).clamp(0.0, 1.0),
        })
        .collect();
    if colors.len() < 2 {
        return None;
    }
    if opacities.len() < 2 {
        opacities = Gradient::default().opacities;
    }
    Some(Gradient { colors, opacities })
}

/// Shapes and their keys.
const SHAPES: [(Shape, &[u8; 4]); 5] = [
    (Shape::Linear, b"Lnr "),
    (Shape::Radial, b"Rdl "),
    (Shape::Angle, b"Angl"),
    (Shape::Reflected, b"Rflc"),
    (Shape::Diamond, b"Dmnd"),
];

/// A gradient fill (fill layer, Gradient Overlay) from its descriptor.
pub fn gradient_fill(d: &Descriptor) -> Option<GradientFill> {
    let style = d
        .enumerated(b"Type")
        .and_then(|t| SHAPES.iter().find(|s| s.1 == t))
        .map_or(Shape::Linear, |s| s.0);
    Some(GradientFill {
        gradient: gradient_of(d.object(b"Grad")?)?,
        style,
        angle: d.num(b"Angl").unwrap_or(90.0) as f32,
        scale: (d.num(b"Scl ").unwrap_or(100.0) as f32 / 100.0).clamp(0.1, 10.0),
        reverse: d.bool(b"Rvrs").unwrap_or(false),
    })
}

/// The descriptor of a gradient ("Grdn"), its colors fixed.
pub fn gradient_descriptor(g: &Gradient) -> Descriptor {
    let colors = g
        .colors
        .iter()
        .map(|s| {
            Value::Object(
                Descriptor::new(b"Clrt")
                    .with(b"Clr ", descriptor::rgb(s.color.resolve([0; 3], [255; 3])))
                    .with(b"Type", enumerated(b"Clry", b"UsrS"))
                    .with(
                        b"Lctn",
                        Value::Integer((s.location * 4096.0).round() as i32),
                    )
                    .with(b"Mdpn", Value::Integer((s.midpoint * 100.0).round() as i32)),
            )
        })
        .collect();
    let opacities = g
        .opacities
        .iter()
        .map(|s| {
            Value::Object(
                Descriptor::new(b"TrnS")
                    .with(b"Opct", percent((s.opacity * 100.0) as f64))
                    .with(
                        b"Lctn",
                        Value::Integer((s.location * 4096.0).round() as i32),
                    )
                    .with(b"Mdpn", Value::Integer((s.midpoint * 100.0).round() as i32)),
            )
        })
        .collect();
    Descriptor::new(b"Grdn")
        .with(b"Nm  ", Value::Text("Custom".into()))
        .with(b"GrdF", enumerated(b"GrdF", b"CstS"))
        .with(b"Intr", Value::Double(4096.0))
        .with(b"Clrs", Value::List(colors))
        .with(b"Trns", Value::List(opacities))
}

/// The items of a gradient fill's descriptor (fill layer, overlay).
pub fn gradient_fill_items(f: &GradientFill, d: Descriptor) -> Descriptor {
    let shape = SHAPES
        .iter()
        .find(|s| s.0 == f.style)
        .map_or(b"Lnr ", |s| s.1);
    d.with(b"Grad", Value::Object(gradient_descriptor(&f.gradient)))
        .with(b"Type", enumerated(b"GrdT", shape))
        .with(b"Angl", descriptor::angle(f.angle as f64))
        .with(b"Rvrs", Value::Bool(f.reverse))
        .with(b"Dthr", Value::Bool(false))
        .with(b"Scl ", percent(f.scale as f64 * 100.0))
        .with(b"Algn", Value::Bool(true))
        .with(
            b"Ofst",
            Value::Object(
                Descriptor::new(b"Pnt ")
                    .with(b"Hrzn", percent(0.0))
                    .with(b"Vrtc", percent(0.0)),
            ),
        )
}

/// A descriptor block: its version (16), then it.
fn versioned_block(w: &mut Writer, d: &Descriptor) {
    w.u32(16);
    d.write(w);
}

/// Writes the blocks of an adjustment or fill layer.
pub fn write(kind: &LayerKind, w: &mut Writer) {
    match kind {
        LayerKind::Fill(Fill::Solid(c)) => w.tagged(b"SoCo", |w| {
            versioned_block(
                w,
                &Descriptor::new(b"null").with(b"Clr ", descriptor::rgb(*c)),
            );
        }),
        LayerKind::Fill(Fill::Gradient(g)) => w.tagged(b"GdFl", |w| {
            versioned_block(w, &gradient_fill_items(g, Descriptor::new(b"null")));
        }),
        LayerKind::Adjustment(a) => write_adjustment(a, w),
        LayerKind::Pixels | LayerKind::Group { .. } => {}
    }
}

fn write_adjustment(a: &Adjustment, w: &mut Writer) {
    let pct = |v: f32| (v * 100.0).round() as i16;
    match a {
        Adjustment::Levels(lv) => w.tagged(b"levl", |w| {
            w.u16(2);
            let neutral = LevelsChannel::default();
            let all = [lv.rgb, lv.channels[0], lv.channels[1], lv.channels[2]];
            for i in 0..29 {
                let c = all.get(i).copied().unwrap_or(neutral);
                w.u16(c.in_black as u16);
                w.u16(c.in_white as u16);
                w.u16(c.out_black as u16);
                w.u16(c.out_white as u16);
                w.u16((c.gamma * 100.0).round().clamp(10.0, 999.0) as u16);
            }
        }),
        Adjustment::Curves(cv) => w.tagged(b"curv", |w| {
            let all = [&cv.rgb, &cv.channels[0], &cv.channels[1], &cv.channels[2]];
            let ids: Vec<u16> = (0..4)
                .filter(|&i| i == 0 || !all[i].is_identity())
                .map(|i| i as u16)
                .collect();
            let points = |w: &mut Writer, c: &Curve| {
                w.u16(c.points().len() as u16);
                for p in c.points() {
                    w.u16(p[1].round() as u16);
                    w.u16(p[0].round() as u16);
                }
            };
            w.u8(0);
            w.u16(1);
            w.u32(ids.iter().map(|&i| 1u32 << i).sum());
            for &i in &ids {
                points(w, all[i as usize]);
            }
            w.bytes(b"Crv ");
            w.u16(4);
            w.u32(ids.len() as u32);
            for &i in &ids {
                w.u16(i);
                points(w, all[i as usize]);
            }
        }),
        Adjustment::HueSaturation(hs) => w.tagged(b"hue2", |w| {
            w.u16(2);
            w.u8(u8::from(hs.colorize));
            w.u8(0);
            if hs.colorize {
                let hue = hs.hue.rem_euclid(360.0);
                let hue = if hue > 180.0 { hue - 360.0 } else { hue };
                for v in [hue.round() as i16, pct(hs.saturation), pct(hs.lightness)] {
                    w.i16(v);
                }
                w.zeros(6);
            } else {
                for v in [0, 25, 0] {
                    w.i16(v);
                }
                for v in [hs.hue.round() as i16, pct(hs.saturation), pct(hs.lightness)] {
                    w.i16(v);
                }
            }
            // The six hue ranges, unchanged.
            for k in 0..6i16 {
                let start = (315 + 60 * k) % 360;
                for v in [
                    start,
                    (start + 30) % 360,
                    (start + 60) % 360,
                    (start + 90) % 360,
                ] {
                    w.i16(v);
                }
                w.zeros(6);
            }
        }),
        Adjustment::ColorBalance(cb) => w.tagged(b"blnc", |w| {
            for v in cb.shadows.iter().chain(&cb.midtones).chain(&cb.highlights) {
                w.i16(pct(*v));
            }
            w.u8(u8::from(cb.preserve_luminosity));
        }),
        Adjustment::BrightnessContrast(bc) => {
            w.tagged(b"brit", |w| {
                w.zeros(4);
                w.u16(127);
                w.zeros(2);
            });
            w.tagged(b"CgEd", |w| {
                let d = Descriptor::new(b"null")
                    .with(b"Vrsn", Value::Integer(1))
                    .with(b"Brgh", Value::Integer(pct(bc.brightness) as i32))
                    .with(b"Cntr", Value::Integer(pct(bc.contrast) as i32))
                    .with(b"means", Value::Integer(127))
                    .with(b"Lab ", Value::Bool(false))
                    .with(b"useLegacy", Value::Bool(false))
                    .with(b"Auto", Value::Bool(false));
                versioned_block(w, &d);
            });
        }
        Adjustment::Vibrance(v) => w.tagged(b"vibA", |w| {
            let d = Descriptor::new(b"null")
                .with(b"vibrance", Value::Integer(pct(v.vibrance) as i32))
                .with(b"Strt", Value::Integer(pct(v.saturation) as i32));
            versioned_block(w, &d);
        }),
        Adjustment::BlackWhite(bw) => w.tagged(b"blwh", |w| {
            let mut d = Descriptor::new(b"null");
            for (key, weight) in BW_KEYS.iter().zip(bw.weights) {
                d = d.with(*key, Value::Integer(weight.round() as i32));
            }
            let d = d
                .with(b"useTint", Value::Bool(bw.tint))
                .with(
                    b"tintColor",
                    descriptor::rgb(tint_color(bw.hue, bw.saturation)),
                )
                .with(b"bwPresetKind", Value::Integer(1))
                .with(b"blackAndWhitePresetFileName", Value::Text(String::new()));
            versioned_block(w, &d);
        }),
        Adjustment::PhotoFilter(f) => w.tagged(b"phfl", |w| {
            w.u16(2);
            w.u16(0);
            for c in f.color {
                w.u16(c as u16 * 257);
            }
            w.u16(0);
            w.u32((f.density * 100.0).round() as u32);
            w.u8(u8::from(f.preserve_luminosity));
        }),
        Adjustment::Invert => w.tagged(b"nvrt", |_| {}),
        Adjustment::Posterize(p) => w.tagged(b"post", |w| {
            w.u16(p.levels as u16);
            w.u16(0);
        }),
        Adjustment::Threshold(t) => w.tagged(b"thrs", |w| {
            w.u16(t.level as u16);
            w.u16(0);
        }),
        Adjustment::GradientMap(g) => w.tagged(b"grdm", |w| {
            w.u16(1);
            w.u8(u8::from(g.reverse));
            w.u8(0);
            w.unicode("Custom");
            w.u16(g.gradient.colors.len() as u16);
            for s in &g.gradient.colors {
                w.u32((s.location * 4096.0).round() as u32);
                w.u32((s.midpoint * 100.0).round() as u32);
                w.u16(0);
                for c in s.color.resolve([0; 3], [255; 3]) {
                    w.u16(c as u16 * 257);
                }
                w.zeros(4);
            }
            w.u16(g.gradient.opacities.len() as u16);
            for s in &g.gradient.opacities {
                w.u32((s.location * 4096.0).round() as u32);
                w.u32((s.midpoint * 100.0).round() as u32);
                w.u16((s.opacity * 255.0).round() as u16);
            }
            // Expansion, smoothness, length, mode, random seed, show
            // transparency, vector color, roughness, color model, and the
            // noise range, as Photoshop writes them for stops.
            for v in [2, 4096, 32, 0] {
                w.u16(v);
            }
            w.u32(0);
            w.u16(0);
            w.u16(1);
            w.u32(2048);
            w.u16(3);
            w.zeros(8);
            for _ in 0..4 {
                w.u16(32768);
            }
            w.zeros(2);
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(kind: LayerKind) -> LayerKind {
        let mut w = Writer::default();
        write(&kind, &mut w);
        let mut r = Reader::new(&w.data);
        let mut blocks = Vec::new();
        while r.remaining() > 0 {
            assert_eq!(r.bytes(4).unwrap(), b"8BIM");
            let key = r.tag().unwrap();
            let n = r.u32().unwrap() as usize;
            assert_eq!(n % 4, 0, "padded to 4");
            blocks.push((key, r.bytes(n).unwrap()));
        }
        read(&blocks).unwrap().unwrap()
    }

    #[test]
    fn adjustments_round_trip() {
        let mut curves = Curves::default();
        curves.rgb.insert(64.0, 90.0);
        curves.channels[2].insert(128.0, 100.0);
        let mut levels = Levels::default();
        levels.rgb.in_black = 12;
        levels.channels[1].gamma = 1.5;
        levels.channels[2].out_white = 200;
        let all = [
            Adjustment::Levels(levels),
            Adjustment::Curves(curves),
            Adjustment::HueSaturation(HueSaturation {
                hue: 30.0,
                saturation: -0.5,
                lightness: 0.25,
                colorize: false,
            }),
            Adjustment::HueSaturation(HueSaturation {
                hue: 250.0,
                saturation: 0.4,
                lightness: -0.1,
                colorize: true,
            }),
            Adjustment::ColorBalance(ColorBalance {
                shadows: [0.1, -0.2, 0.3],
                midtones: [0.0, 0.5, -1.0],
                highlights: [0.25, 0.0, 0.0],
                preserve_luminosity: false,
            }),
            Adjustment::BrightnessContrast(BrightnessContrast {
                brightness: 0.4,
                contrast: -0.3,
            }),
            Adjustment::Vibrance(Vibrance {
                vibrance: 0.5,
                saturation: -0.2,
            }),
            Adjustment::BlackWhite(BlackWhite {
                weights: [10.0, 20.0, 30.0, 40.0, 50.0, -60.0],
                tint: true,
                hue: 200.0,
                saturation: 0.5,
            }),
            Adjustment::PhotoFilter(PhotoFilter {
                color: [255, 0, 128],
                density: 0.6,
                preserve_luminosity: false,
            }),
            Adjustment::Invert,
            Adjustment::Posterize(Posterize { levels: 7 }),
            Adjustment::Threshold(Threshold { level: 99 }),
            Adjustment::GradientMap(GradientMap {
                gradient: crate::paint::gradient::Preset::Copper.gradient(),
                reverse: true,
            }),
        ];
        for a in all {
            let back = round_trip(LayerKind::Adjustment(a.clone()));
            let LayerKind::Adjustment(b) = back else {
                panic!("{a:?}");
            };
            match (&a, &b) {
                (Adjustment::BlackWhite(x), Adjustment::BlackWhite(y)) => {
                    assert_eq!((x.weights, x.tint), (y.weights, y.tint));
                    assert!(
                        (x.hue - y.hue).abs() < 2.0 && (x.saturation - y.saturation).abs() < 0.02
                    );
                }
                (Adjustment::GradientMap(x), Adjustment::GradientMap(y)) => {
                    assert_eq!(x.reverse, y.reverse);
                    for t in [0.0, 0.3, 0.7, 1.0] {
                        let (p, q) = (
                            x.gradient.sample(t, [0; 3], [0; 3], true),
                            y.gradient.sample(t, [0; 3], [0; 3], true),
                        );
                        assert!(
                            p.iter().zip(q).all(|(a, b)| (a - b).abs() < 1.0),
                            "{p:?} {q:?}"
                        );
                    }
                }
                _ => assert_eq!(a, b),
            }
        }
    }

    #[test]
    fn fills_round_trip() {
        let solid = LayerKind::Fill(Fill::Solid([12, 34, 56]));
        assert_eq!(round_trip(solid.clone()), solid);
        let fill = GradientFill {
            gradient: crate::paint::gradient::Preset::Spectrum.gradient(),
            style: Shape::Diamond,
            angle: 33.0,
            scale: 1.5,
            reverse: true,
        };
        let LayerKind::Fill(Fill::Gradient(back)) =
            round_trip(LayerKind::Fill(Fill::Gradient(fill.clone())))
        else {
            panic!("a gradient fill");
        };
        assert_eq!(
            (back.style, back.angle, back.scale, back.reverse),
            (fill.style, fill.angle, fill.scale, fill.reverse)
        );
        assert_eq!(back.gradient.colors.len(), 7);
    }
}
