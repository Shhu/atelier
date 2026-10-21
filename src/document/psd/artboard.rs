//! Artboards: the "artb" block of a group (Photoshop CC 2015 and later),
//! a version (16) then a descriptor with the artboard's rectangle, its
//! background type (1 white, 2 black, 3 transparent, 4 other) and color.
//! Older files may name the block "artd" or "abdd".

use super::bytes::{Reader, Writer};
use super::descriptor::{Descriptor, Value, color_of, rgb};
use crate::document::{Artboard, ArtboardBackground, Layer, PixelRect};

/// The artboard block of `layer`, if it is an artboard.
pub fn write(layer: &Layer, x: &mut Writer) {
    let (Some(a), Some(r)) = (layer.artboard, layer.artboard_rect()) else {
        return;
    };
    let (kind, color) = match a.background {
        ArtboardBackground::White => (1, [255; 3]),
        ArtboardBackground::Black => (2, [0; 3]),
        ArtboardBackground::Transparent => (3, [255; 3]),
        ArtboardBackground::Custom(c) => (4, c),
    };
    let rect = Descriptor::new(b"classFloatRect")
        .with(b"Top ", Value::Double(r.y as f64))
        .with(b"Left", Value::Double(r.x as f64))
        .with(b"Btom", Value::Double(r.bottom() as f64))
        .with(b"Rght", Value::Double(r.right() as f64));
    let d = Descriptor::new(b"artboard")
        .with(b"artboardRect", Value::Object(rect))
        .with(b"guideIndeces", Value::List(Vec::new()))
        .with(b"artboardPresetName", Value::Text(String::new()))
        .with(b"Clr ", rgb(color))
        .with(b"artboardBackgroundType", Value::Integer(kind));
    x.tagged(b"artb", |x| {
        x.u32(16);
        d.write(x);
    });
}

/// The artboard of a group's block: its rectangle and settings.
pub fn read(data: &[u8]) -> Option<(PixelRect, Artboard)> {
    let mut r = Reader::new(data);
    if r.u32().ok()? != 16 {
        return None;
    }
    let d = Descriptor::read(&mut r).ok()?;
    let rect = d.object(b"artboardRect")?;
    let side = |k: &[u8]| {
        rect.num(k)
            .filter(|v| v.is_finite())
            .map(|v| v.round() as i64)
    };
    let (top, left) = (side(b"Top ")?, side(b"Left")?);
    let (bottom, right) = (side(b"Btom")?, side(b"Rght")?);
    let (w, h) = (right - left, bottom - top);
    let max = i64::from(crate::document::MAX_DIMENSION);
    if !(1..=max).contains(&w) || !(1..=max).contains(&h) {
        return None;
    }
    let background = match d.num(b"artboardBackgroundType").map(|v| v as i32) {
        Some(2) => ArtboardBackground::Black,
        Some(3) => ArtboardBackground::Transparent,
        Some(4) => d
            .object(b"Clr ")
            .and_then(color_of)
            .map_or(ArtboardBackground::White, ArtboardBackground::Custom),
        _ => ArtboardBackground::White,
    };
    let rect = PixelRect {
        x: left,
        y: top,
        w: w as u32,
        h: h as u32,
    };
    let artboard = Artboard {
        size: (rect.w, rect.h),
        background,
    };
    Some((rect, artboard))
}
