//! Patterns in Photoshop documents: the tiles of the document (the global
//! "Patt" block) and the descriptor of a pattern's use (Pattern Fill layer,
//! Pattern Overlay), which refers to them by identifier.

use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;

use super::bytes::{Reader, Writer, pack_row, unpack_row};
use super::descriptor::{Descriptor, Value, angle, percent};
use crate::compose::pattern::{Pattern, PatternFill, SCALE_RANGE};
use crate::document::{FileError, Layer, LayerKind, MAX_DIMENSION};

type Result<T> = std::result::Result<T, FileError>;

/// The patterns of a document, by identifier.
pub type Patterns = HashMap<String, Arc<Pattern>>;

/// Channels of the format's channel list: 24, then the user and sheet masks.
const CHANNELS: u32 = 24;

/// A pattern's identifier in the file (the shape of a UUID, from its id).
fn identifier(p: &Pattern) -> String {
    let h = format!("{:016x}{:016x}", p.id, p.id.rotate_left(29) ^ 0x5bd1_e995);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// A channel: its size, left, top, and bytes.
type Channel = ([u32; 2], u32, u32, Vec<u8>);

/// One channel of the list: its box, data and depth checked. `None`: not
/// there; an unknown depth or compression is damage for this pattern alone.
fn read_channel(r: &mut Reader, [w, h]: [u32; 2]) -> Result<Option<Channel>> {
    if r.u32()? == 0 {
        return Ok(None);
    }
    let len = r.u32()? as usize;
    let mut c = r.sub(len)?;
    let depth = c.u32()?;
    let [top, left, bottom, right] = [c.i32()?, c.i32()?, c.i32()?, c.i32()?];
    let second = c.u16()?;
    let compression = c.u8()?;
    if depth != 8 || second != 8 || left < 0 || top < 0 || right < left || bottom < top {
        return Err(FileError::Damaged);
    }
    let (cw, ch) = ((right - left) as u32, (bottom - top) as u32);
    if left as u32 + cw > w || top as u32 + ch > h {
        return Err(FileError::Damaged);
    }
    let mut plane = vec![0; cw as usize * ch as usize];
    match compression {
        0 => {
            let raw = c.bytes(plane.len())?;
            plane.copy_from_slice(raw);
        }
        1 => {
            let counts: Vec<usize> = (0..ch)
                .map(|_| c.u16().map(usize::from))
                .collect::<Result<_>>()?;
            if cw > 0 {
                for (row, n) in plane.chunks_exact_mut(cw as usize).zip(counts) {
                    unpack_row(c.bytes(n)?, row)?;
                }
            }
        }
        _ => return Err(FileError::Damaged),
    }
    Ok(Some(([cw, ch], left as u32, top as u32, plane)))
}

/// One pattern of the block; what Atelier can't read (other modes): `None`.
fn read_pattern(r: &mut Reader) -> Result<Option<(String, Arc<Pattern>)>> {
    if r.u32()? != 1 {
        return Ok(None);
    }
    let mode = r.u32()?;
    r.skip(4)?; // Point.
    let name = r.unicode()?;
    let id = String::from_utf8_lossy(r.pascal(1)?).into_owned();
    let mut palette = None;
    if mode == 2 {
        palette = Some(r.bytes(768)?);
        r.skip(4)?;
    }
    let colors = match mode {
        1 | 2 => 1,
        3 => 3,
        _ => return Ok(None),
    };
    if r.u32()? != 3 {
        return Ok(None);
    }
    let len = r.u32()? as usize;
    let mut list = r.sub(len)?;
    let [top, left, bottom, right] = [list.i32()?, list.i32()?, list.i32()?, list.i32()?];
    let (w, h) = (
        right.checked_sub(left).ok_or(FileError::Damaged)?,
        bottom.checked_sub(top).ok_or(FileError::Damaged)?,
    );
    let max = 1..=MAX_DIMENSION as i32;
    if !max.contains(&w) || !max.contains(&h) {
        return Err(FileError::Damaged);
    }
    let (w, h) = (w as u32, h as u32);
    let channels = list.u32()?;
    // Color channels first, then the transparency when there is one.
    let mut image = RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
    let mut found = 0;
    for _ in 0..channels.min(CHANNELS) + 2 {
        let Some(([cw, _], x0, y0, plane)) = read_channel(&mut list, [w, h])? else {
            continue;
        };
        if found > colors {
            continue;
        }
        let at = if found == colors { 3 } else { found };
        if cw > 0 {
            for (j, row) in plane.chunks_exact(cw as usize).enumerate() {
                for (i, &v) in row.iter().enumerate() {
                    let p = image.get_pixel_mut(x0 + i as u32, y0 + j as u32);
                    p.0[at] = v;
                    if colors == 1 && at == 0 {
                        p.0[1] = v;
                        p.0[2] = v;
                        if let Some(table) = palette {
                            let k = v as usize;
                            p.0[..3].copy_from_slice(&[table[k], table[256 + k], table[512 + k]]);
                        }
                    }
                }
            }
        }
        found += 1;
    }
    if found < colors {
        return Ok(None);
    }
    Ok(Some((id, Pattern::new(name, image))))
}

/// The tiles of a "Patt" block, each under the identifier its uses refer to.
/// A damaged one ends the reading: those uses then count as not understood.
pub fn read_block(data: &[u8]) -> Patterns {
    let mut r = Reader::new(data);
    let mut all = Patterns::new();
    while r.remaining() >= 4 {
        let Ok(n) = r.u32().map(|n| n as usize) else {
            break;
        };
        let Ok(mut one) = r.sub(n) else {
            break;
        };
        match read_pattern(&mut one) {
            Ok(Some((id, p))) => {
                all.insert(id, p);
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if r.skip((4 - n % 4) % 4).is_err() {
            break;
        }
    }
    all
}

fn write_channel(w: &mut Writer, plane: &[u8], width: u32, height: u32) {
    w.u32(1);
    w.block(1, |c| {
        c.u32(8);
        for v in [0, 0, height as i32, width as i32] {
            c.i32(v);
        }
        c.u16(8);
        c.u8(1); // RLE.
        let mut rows = Vec::new();
        let mut packed = Vec::with_capacity(plane.len() / 2);
        for row in plane.chunks_exact(width as usize) {
            let start = packed.len();
            pack_row(row, &mut packed);
            rows.push((packed.len() - start) as u16);
        }
        for n in rows {
            c.u16(n);
        }
        c.bytes(&packed);
    });
}

/// Writes one tile (RGB; its transparency in the sheet mask's place, when it
/// has some).
fn write_pattern(w: &mut Writer, p: &Pattern) {
    w.block(4, |o| {
        o.u32(1);
        o.u32(3);
        o.zeros(4);
        o.unicode(&p.name);
        o.pascal(identifier(p).as_bytes(), 1);
        o.u32(3);
        let (width, height) = p.image.dimensions();
        o.block(1, |l| {
            for v in [0, 0, height as i32, width as i32] {
                l.i32(v);
            }
            l.u32(CHANNELS);
            let plane = |c: usize| -> Vec<u8> { p.image.pixels().map(|px| px.0[c]).collect() };
            for c in 0..3 {
                write_channel(l, &plane(c), width, height);
            }
            for _ in 3..CHANNELS + 1 {
                l.u32(0);
            }
            if p.image.pixels().any(|px| px.0[3] != 255) {
                write_channel(l, &plane(3), width, height);
            } else {
                l.u32(0);
            }
        });
    });
}

/// The tiles the layers use, once each.
pub fn used(layers: &[Layer]) -> Vec<Arc<Pattern>> {
    let mut all: Vec<Arc<Pattern>> = Vec::new();
    for l in layers {
        let kind = match &l.kind {
            LayerKind::Fill(crate::document::Fill::Pattern(f)) => Some(f),
            _ => None,
        };
        let fx = l
            .effects
            .pattern_overlay
            .enabled
            .then_some(&l.effects.pattern_overlay.fill);
        for f in kind.into_iter().chain(fx) {
            if all.iter().all(|p| p.id != f.pattern.id) {
                all.push(f.pattern.clone());
            }
        }
    }
    all
}

/// The global block of `patterns`, if any.
pub fn write_block(w: &mut Writer, patterns: &[Arc<Pattern>]) {
    if patterns.is_empty() {
        return;
    }
    w.tagged(b"Patt", |w| {
        for p in patterns {
            write_pattern(w, p);
        }
    });
}

/// A pattern's use from its descriptor; `None`: its tile isn't in the file.
pub fn fill_of(d: &Descriptor, patterns: &Patterns) -> Option<PatternFill> {
    let reference = d.object(b"Ptrn")?;
    let Some(Value::Text(id)) = reference.get(b"Idnt") else {
        return None;
    };
    let pattern = patterns.get(id)?.clone();
    let (mut x, mut y) = (0.0, 0.0);
    if let Some(p) = d.object(b"phase") {
        x = p.num(b"Hrzn").unwrap_or(0.0) as f32;
        y = p.num(b"Vrtc").unwrap_or(0.0) as f32;
    }
    Some(PatternFill {
        pattern,
        scale: (d.num(b"Scl ").unwrap_or(100.0) as f32 / 100.0)
            .clamp(*SCALE_RANGE.start(), *SCALE_RANGE.end()),
        angle: d.num(b"Angl").unwrap_or(0.0) as f32,
        linked: d.bool(b"Algn").unwrap_or(true),
        phase: (x, y),
    })
}

/// The items of a pattern's use, added to `d`.
pub fn fill_items(f: &PatternFill, d: Descriptor) -> Descriptor {
    let reference = Descriptor::new(b"Ptrn")
        .with(b"Nm  ", Value::Text(f.pattern.name.clone()))
        .with(b"Idnt", Value::Text(identifier(&f.pattern)));
    let phase = Descriptor::new(b"Pnt ")
        .with(b"Hrzn", Value::Double(f.phase.0 as f64))
        .with(b"Vrtc", Value::Double(f.phase.1 as f64));
    d.with(b"Ptrn", Value::Object(reference))
        .with(b"Scl ", percent(f.scale as f64 * 100.0))
        .with(b"Angl", angle(f.angle as f64))
        .with(b"Algn", Value::Bool(f.linked))
        .with(b"phase", Value::Object(phase))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn tile() -> Arc<Pattern> {
        let image = RgbaImage::from_fn(5, 3, |x, y| Rgba([x as u8 * 40, y as u8 * 60, 9, 255]));
        Pattern::new("Tuiles é", image)
    }

    fn back(p: &Arc<Pattern>) -> Arc<Pattern> {
        let mut w = Writer::default();
        write_pattern(&mut w, p);
        read_block(&w.data).remove(&identifier(p)).expect("read")
    }

    #[test]
    fn tiles_round_trip_with_their_transparency() {
        let p = tile();
        let b = back(&p);
        assert_eq!(*b, *p);
        assert_eq!(*b.image, *p.image);
        let mut image = (*p.image).clone();
        image.put_pixel(2, 1, Rgba([1, 2, 3, 40]));
        let p = Pattern::new("alpha", image);
        assert_eq!(*back(&p).image, *p.image);
    }

    /// A gray, raw tile, as the format allows it.
    #[test]
    fn gray_raw_tiles_are_read() {
        let mut w = Writer::default();
        w.block(4, |o| {
            o.u32(1);
            o.u32(1);
            o.zeros(4);
            o.unicode("g");
            o.pascal(b"abc", 1);
            o.u32(3);
            o.block(1, |l| {
                for v in [0, 0, 1, 2] {
                    l.i32(v);
                }
                l.u32(CHANNELS);
                l.u32(1);
                l.block(1, |c| {
                    c.u32(8);
                    for v in [0, 0, 1, 2] {
                        c.i32(v);
                    }
                    c.u16(8);
                    c.u8(0);
                    c.bytes(&[10, 200]);
                });
                for _ in 1..CHANNELS + 2 {
                    l.u32(0);
                }
            });
        });
        let all = read_block(&w.data);
        let g = &all["abc"];
        assert_eq!(g.image.get_pixel(1, 0).0, [200, 200, 200, 255]);
    }

    #[test]
    fn damage_is_not_a_panic() {
        let mut w = Writer::default();
        write_pattern(&mut w, &tile());
        for cut in [0, 3, 10, 40, w.data.len() - 3] {
            assert!(read_block(&w.data[..cut]).is_empty(), "{cut}");
        }
        let mut wrong = w.data.clone();
        for i in 4..wrong.len() {
            wrong[i] ^= 0x55;
            let _ = read_block(&wrong);
            wrong[i] ^= 0x55;
        }
    }

    #[test]
    fn uses_round_trip() {
        let p = tile();
        let f = PatternFill {
            scale: 2.5,
            angle: -30.0,
            linked: false,
            phase: (3.0, 4.0),
            ..PatternFill::of(p.clone())
        };
        let d = fill_items(&f, Descriptor::new(b"null"));
        let all: Patterns = [(identifier(&p), p)].into();
        assert_eq!(fill_of(&d, &all).unwrap(), f);
        assert!(fill_of(&d, &Patterns::new()).is_none());
    }
}
