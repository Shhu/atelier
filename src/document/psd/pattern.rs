//! Patterns in Photoshop documents: the tiles of the document (the global
//! "Patt" block) and the descriptor of a pattern's use (Pattern Fill layer,
//! Pattern Overlay), which refers to them by identifier.

use std::collections::HashMap;
use std::sync::Arc;

use image::RgbaImage;

use super::bytes::{Reader, Writer, pack_row, unpack_row};
use super::descriptor::{Descriptor, Value, angle, percent};
use crate::compose::pattern::{Pattern, PatternFill, STORED_SCALE};
use crate::document::file_io::Budget;
use crate::document::{FileError, Layer, LayerKind, MAX_DIMENSION, PixelRect};

type Result<T> = std::result::Result<T, FileError>;

/// The patterns of a document, by identifier.
pub type Patterns = HashMap<String, Arc<Pattern>>;

/// Channels of the format's channel list: 24, then the user and sheet masks.
const CHANNELS: u32 = 24;

/// Most channels read from a list.
const MAX_CHANNELS: u32 = 64;

/// What a tile takes while it is read and kept: 4 bytes, a third more for
/// its smaller versions, a byte for the channel being decoded.
const TILE_BYTES_PER_PIXEL: u64 = 7;

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

/// One channel of the list.
struct Channel<'a> {
    /// Its box in the tile: left, top, width, height.
    at: [u32; 4],
    depth: u32,
    compression: u8,
    data: Reader<'a>,
}

/// Most pixels a byte of RLE data can give.
const RLE_PIXELS_PER_BYTE: u64 = 64;

/// The next channel of the list, checked against the tile `rect` (top,
/// left, bottom, right) without decoding it. `None`: not there.
fn read_channel<'a>(r: &mut Reader<'a>, rect: [i32; 4]) -> Result<Option<Channel<'a>>> {
    if r.u32()? == 0 {
        return Ok(None);
    }
    let len = r.u32()? as usize;
    if len == 0 {
        return Ok(None);
    }
    let mut c = r.sub(len)?;
    let first = c.u32()?;
    let [top, left, bottom, right] = [c.i32()?, c.i32()?, c.i32()?, c.i32()?];
    let second = c.u16()? as u32;
    let compression = c.u8()?;
    let depth = if matches!(second, 8 | 16 | 32) {
        second
    } else {
        first
    };
    if !matches!(depth, 8 | 16 | 32)
        || right < left
        || bottom < top
        || left < rect[1]
        || top < rect[0]
        || right > rect[3]
        || bottom > rect[2]
    {
        return Err(FileError::Damaged);
    }
    let (cw, ch) = ((right - left) as u32, (bottom - top) as u32);
    let bytes = cw as u64 * ch as u64 * (depth as u64 / 8);
    let room = c.remaining() as u64;
    let fits = match compression {
        0 => bytes <= room,
        1 => bytes <= room.saturating_mul(RLE_PIXELS_PER_BYTE),
        _ => false,
    };
    if !fits {
        return Err(FileError::Damaged);
    }
    Ok(Some(Channel {
        at: [(left - rect[1]) as u32, (top - rect[0]) as u32, cw, ch],
        depth,
        compression,
        data: c,
    }))
}

/// A channel's values, in 8 bits, row after row.
fn decode_channel(c: Channel) -> Result<Vec<u8>> {
    let [_, _, cw, ch] = c.at;
    let (rows, size) = (ch as usize, (c.depth / 8) as usize);
    let row = cw as usize * size;
    let mut data = c.data;
    let mut plane = vec![0; row * rows];
    match c.compression {
        0 => {
            let raw = data.bytes(plane.len())?;
            plane.copy_from_slice(raw);
        }
        _ => {
            let counts: Vec<usize> = (0..rows)
                .map(|_| data.u16().map(usize::from))
                .collect::<Result<_>>()?;
            if row > 0 {
                for (out, n) in plane.chunks_exact_mut(row).zip(counts) {
                    unpack_row(data.bytes(n)?, out)?;
                }
            }
        }
    }
    Ok(match c.depth {
        16 => plane
            .chunks_exact(2)
            .map(|v| ((u16::from_be_bytes([v[0], v[1]]) as u32 + 128) / 257) as u8)
            .collect(),
        32 => plane
            .chunks_exact(4)
            .map(|v| {
                let f = f32::from_be_bytes([v[0], v[1], v[2], v[3]]);
                (f.clamp(0.0, 1.0) * 255.0).round() as u8
            })
            .collect(),
        _ => plane,
    })
}

const GRAY: u32 = 1;
const INDEXED: u32 = 2;
const RGB: u32 = 3;
const CMYK: u32 = 4;

/// Memory running out ends the reading; another tile may be good after a
/// damaged one.
fn memory_error(e: &FileError) -> bool {
    matches!(e, FileError::LowMemory(_) | FileError::OutOfMemory(_))
}

/// One pattern of the block, if `wanted` and readable; its pixels are taken
/// from `budget` before they are made. What Atelier can't read (other modes):
/// `None`.
fn read_pattern(
    r: &mut Reader,
    wanted: &dyn Fn(&str) -> bool,
    budget: &mut Budget,
) -> Result<Option<(String, Arc<Pattern>)>> {
    if r.u32()? != 1 {
        return Ok(None);
    }
    let mode = r.u32()?;
    r.skip(4)?; // Point.
    let name = r.unicode()?;
    let id = String::from_utf8_lossy(r.pascal(1)?).into_owned();
    if !wanted(&id) {
        return Ok(None);
    }
    let mut palette = None;
    if mode == INDEXED {
        palette = Some(r.bytes(768)?);
        // Some writers put 4 more bytes here, others don't: the version of
        // the list comes next.
        if r.clone().u32()? != 3 {
            r.skip(4)?;
        }
    }
    let colors = match mode {
        GRAY | INDEXED => 1,
        RGB => 3,
        CMYK => 4,
        _ => return Ok(None),
    };
    if r.u32()? != 3 {
        return Ok(None);
    }
    let len = r.u32()? as usize;
    let mut list = r.sub(len)?;
    let rect = [list.i32()?, list.i32()?, list.i32()?, list.i32()?];
    let [top, left, bottom, right] = rect;
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
    // All the channels checked before any pixel is made: the size comes from
    // the rectangle alone, and nothing else ties it to the data.
    if w as u64 * h as u64 > (list.remaining() as u64).saturating_mul(RLE_PIXELS_PER_BYTE) {
        return Err(FileError::Damaged);
    }
    // Color channels first, then the transparency when there is one.
    let mut kept = Vec::new();
    for _ in 0..channels.min(MAX_CHANNELS) + 2 {
        if list.remaining() < 4 {
            break;
        }
        let Some(channel) = read_channel(&mut list, rect)? else {
            continue;
        };
        if kept.len() <= colors {
            kept.push(channel);
        }
    }
    if kept.len() < colors {
        return Ok(None);
    }
    // The tile, its smaller versions, and a channel while it is decoded (a
    // deeper one first takes its own bytes); CMYK also keeps its inks.
    let deepest = kept.iter().map(|c| (c.depth / 8).max(1)).max().unwrap_or(1) as u64;
    let inks = if mode == CMYK { 4 } else { 0 };
    budget.take(w, h, TILE_BYTES_PER_PIXEL + deepest - 1 + inks)?;
    let mut planes = vec![
        [0u8, 0, 0, 255];
        if mode == CMYK {
            w as usize * h as usize
        } else {
            0
        }
    ];
    let mut image = RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 255]));
    for (i, channel) in kept.into_iter().enumerate() {
        let [x0, y0, cw, _] = channel.at;
        let plane = decode_channel(channel)?;
        if cw == 0 {
            continue;
        }
        for (j, row) in plane.chunks_exact(cw as usize).enumerate() {
            for (k, &v) in row.iter().enumerate() {
                let (x, y) = (x0 + k as u32, y0 + j as u32);
                let p = image.get_pixel_mut(x, y);
                match mode {
                    _ if i == colors => p.0[3] = v,
                    GRAY => p.0[..3].fill(v),
                    INDEXED => {
                        if let Some(table) = palette {
                            let c = v as usize * 3;
                            p.0[..3].copy_from_slice(&table[c..c + 3]);
                        }
                    }
                    CMYK => planes[y as usize * w as usize + x as usize][i] = v,
                    _ => p.0[i] = v,
                }
            }
        }
    }
    if mode == CMYK {
        // Stored inverted: 255 is no ink.
        for (p, inks) in image.pixels_mut().zip(&planes) {
            let k = inks[3] as u32;
            for (out, &ink) in p.0.iter_mut().zip(&inks[..3]) {
                *out = ((ink as u32 * k + 127) / 255) as u8;
            }
        }
    }
    Ok(Some((id, Pattern::new(name, image))))
}

/// The tiles of a "Patt" block that are `wanted`, each under the identifier
/// its uses refer to. A damaged one is left out alone: those uses then count
/// as not understood. Only memory running out is an error.
pub fn read_block(
    data: &[u8],
    wanted: &dyn Fn(&str) -> bool,
    budget: &mut Budget,
) -> Result<Patterns> {
    let mut r = Reader::new(data);
    let mut all = Patterns::new();
    while r.remaining() >= 4 {
        let Ok(n) = r.u32().map(|n| n as usize) else {
            break;
        };
        let Ok(mut one) = r.sub(n) else {
            break;
        };
        match read_pattern(&mut one, wanted, budget) {
            Ok(Some((id, p))) => {
                all.insert(id, p);
            }
            Ok(None) => {}
            Err(e) if memory_error(&e) => return Err(e),
            Err(_) => {}
        }
        if r.skip((4 - n % 4) % 4).is_err() {
            break;
        }
    }
    Ok(all)
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
            .in_use()
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

/// The identifier of the tile a pattern's use refers to.
pub fn id_of(d: &Descriptor) -> Option<&str> {
    match d.object(b"Ptrn")?.get(b"Idnt") {
        Some(Value::Text(id)) => Some(id),
        _ => None,
    }
}

/// A number of a file: `default` unless it can be used.
fn finite(v: Option<f64>, default: f32) -> f32 {
    let v = v.map_or(default, |v| v as f32);
    if v.is_finite() { v } else { default }
}

/// A pattern's use from its descriptor; `None`: its tile isn't in the file.
/// A Pattern Fill says "Lnkd", a Pattern Overlay "Algn".
pub fn fill_of(d: &Descriptor, patterns: &Patterns) -> Option<PatternFill> {
    let reference = d.object(b"Ptrn")?;
    let mut pattern = patterns.get(id_of(d)?)?.clone();
    // A tile has one entry in the file, whatever its uses call it.
    if let Some(Value::Text(name)) = reference.get(b"Nm  ")
        && *name != pattern.name
    {
        pattern = pattern.renamed(name.clone());
    }
    let (mut x, mut y) = (0.0, 0.0);
    if let Some(p) = d.object(b"phase") {
        x = finite(p.num(b"Hrzn"), 0.0);
        y = finite(p.num(b"Vrtc"), 0.0);
    }
    Some(PatternFill {
        pattern,
        scale: (finite(d.num(b"Scl "), 100.0) / 100.0)
            .clamp(*STORED_SCALE.start(), *STORED_SCALE.end()),
        angle: finite(d.num(b"Angl"), 0.0),
        linked: d.bool(b"Lnkd").or(d.bool(b"Algn")).unwrap_or(true),
        phase: (x, y),
    })
}

/// A use of a tile as the file says it: the file's layer starts at `rect`
/// (trimmed to its content; none for a layer without pixels), and a linked
/// tile starts from the layer's corner.
pub fn rebased(f: &PatternFill, offset: (i64, i64), rect: PixelRect) -> PatternFill {
    let mut out = f.clone();
    if f.linked {
        out.phase.0 += (offset.0 - rect.x) as f32;
        out.phase.1 += (offset.1 - rect.y) as f32;
    }
    out
}

/// The items of a pattern's use, added to `d`; `link`: the key of Link with
/// Layer ("Lnkd" in a Pattern Fill, "Algn" in a Pattern Overlay).
pub fn fill_items(f: &PatternFill, d: Descriptor, link: &[u8; 4]) -> Descriptor {
    let reference = Descriptor::new(b"Ptrn")
        .with(b"Nm  ", Value::Text(f.pattern.name.clone()))
        .with(b"Idnt", Value::Text(identifier(&f.pattern)));
    let phase = Descriptor::new(b"Pnt ")
        .with(b"Hrzn", Value::Double(f.phase.0 as f64))
        .with(b"Vrtc", Value::Double(f.phase.1 as f64));
    d.with(b"Ptrn", Value::Object(reference))
        .with(b"Scl ", percent(f.scale as f64 * 100.0))
        .with(b"Angl", angle(f.angle as f64))
        .with(link, Value::Bool(f.linked))
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

    fn read_all(data: &[u8]) -> Patterns {
        read_block(data, &|_| true, &mut Budget::of(1 << 30)).expect("memory")
    }

    fn back(p: &Arc<Pattern>) -> Arc<Pattern> {
        let mut w = Writer::default();
        write_pattern(&mut w, p);
        read_all(&w.data).remove(&identifier(p)).expect("read")
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
        let all = read_all(&w.data);
        let g = &all["abc"];
        assert_eq!(g.image.get_pixel(1, 0).0, [200, 200, 200, 255]);
    }

    #[test]
    fn damage_is_not_a_panic() {
        let mut w = Writer::default();
        write_pattern(&mut w, &tile());
        for cut in [0, 3, 10, 40, w.data.len() - 3] {
            assert!(read_all(&w.data[..cut]).is_empty(), "{cut}");
        }
        let mut wrong = w.data.clone();
        for i in 4..wrong.len() {
            wrong[i] ^= 0x55;
            let _ = read_all(&wrong);
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
        let d = fill_items(&f, Descriptor::new(b"null"), b"Lnkd");
        let all: Patterns = [(identifier(&p), p)].into();
        assert_eq!(fill_of(&d, &all).unwrap(), f);
        assert!(fill_of(&d, &Patterns::new()).is_none());
    }

    fn raw_channel(l: &mut Writer, depth: u32, rect: [i32; 4], data: &[u8]) {
        l.u32(1);
        l.block(1, |c| {
            c.u32(depth);
            for v in rect {
                c.i32(v);
            }
            c.u16(depth as u16);
            c.u8(0);
            c.bytes(data);
        });
    }

    /// A tile with its channels written by `f`, after `extra` (a palette).
    fn tile_block(
        w: &mut Writer,
        mode: u32,
        id: &[u8],
        rect: [i32; 4],
        extra: &[u8],
        f: impl FnOnce(&mut Writer),
    ) {
        w.block(4, |o| {
            o.u32(1);
            o.u32(mode);
            o.zeros(4);
            o.unicode("t");
            o.pascal(id, 1);
            o.bytes(extra);
            o.u32(3);
            o.block(1, |l| {
                for v in rect {
                    l.i32(v);
                }
                l.u32(24);
                f(l);
            });
        });
    }

    fn empty(l: &mut Writer, n: usize) {
        for _ in 0..n {
            l.u32(0);
        }
    }

    const ONE: [i32; 4] = [0, 0, 1, 1];

    fn rgb_tile(w: &mut Writer, id: &[u8], depth: u32, values: [&[u8]; 3]) {
        tile_block(w, RGB, id, ONE, &[], |l| {
            for v in values {
                raw_channel(l, depth, ONE, v);
            }
            empty(l, 23);
        });
    }

    #[test]
    fn a_tile_without_its_data_takes_no_memory() {
        // Three channels of no size in a tile said to be 30000 a side: about
        // 120 bytes that would make 3.6 GB.
        let mut w = Writer::default();
        let rect = [0, 0, 30000, 30000];
        tile_block(&mut w, RGB, b"bomb", rect, &[], |l| {
            for _ in 0..3 {
                raw_channel(l, 8, [0, 0, 0, 0], &[]);
            }
            empty(l, 23);
        });
        let mut budget = Budget::of(100);
        let all = read_block(&w.data, &|_| true, &mut budget).expect("no memory asked");
        assert!(all.is_empty());
        // A channel saying more than its bytes can hold is refused as well.
        for compression in [0u8, 1] {
            let mut w = Writer::default();
            tile_block(&mut w, GRAY, b"c", rect, &[], |l| {
                l.u32(1);
                l.block(1, |c| {
                    c.u32(8);
                    for v in rect {
                        c.i32(v);
                    }
                    c.u16(8);
                    c.u8(compression);
                    c.zeros(4000);
                });
                empty(l, 25);
            });
            assert!(read_all(&w.data).is_empty(), "{compression}");
        }
    }

    #[test]
    fn tiles_are_paid_from_the_budget() {
        let mut w = Writer::default();
        write_pattern(&mut w, &tile());
        // 5 x 3 tile.
        let small = read_block(&w.data, &|_| true, &mut Budget::of(14));
        assert!(matches!(small, Err(FileError::OutOfMemory(_))), "{small:?}");
        let mut enough = Budget::of(5 * 3 * TILE_BYTES_PER_PIXEL);
        assert_eq!(
            read_block(&w.data, &|_| true, &mut enough).unwrap().len(),
            1
        );
    }

    #[test]
    fn only_the_wanted_tiles_are_read() {
        let mut w = Writer::default();
        rgb_tile(&mut w, b"a", 8, [&[1], &[2], &[3]]);
        rgb_tile(&mut w, b"b", 8, [&[4], &[5], &[6]]);
        let mut budget = Budget::of(TILE_BYTES_PER_PIXEL);
        let all = read_block(&w.data, &|id| id == "b", &mut budget).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all["b"].image.get_pixel(0, 0).0, [4, 5, 6, 255]);
    }

    #[test]
    fn a_bad_tile_leaves_the_next_ones_and_deep_ones_are_read() {
        let mut w = Writer::default();
        // Depth 7 doesn't exist.
        rgb_tile(&mut w, b"bad", 7, [&[1], &[2], &[3]]);
        rgb_tile(&mut w, b"deep", 16, [&[0x80, 0x80], &[0xff, 0xff], &[0, 0]]);
        let one = 0.5f32.to_be_bytes();
        rgb_tile(&mut w, b"float", 32, [&one, &one, &one]);
        rgb_tile(&mut w, b"good", 8, [&[7], &[8], &[9]]);
        let all = read_all(&w.data);
        assert!(!all.contains_key("bad"));
        assert_eq!(all["deep"].image.get_pixel(0, 0).0, [128, 255, 0, 255]);
        assert_eq!(all["float"].image.get_pixel(0, 0).0, [128, 128, 128, 255]);
        assert_eq!(all["good"].image.get_pixel(0, 0).0, [7, 8, 9, 255]);
    }

    #[test]
    fn written_with_no_length_is_a_missing_channel() {
        let mut w = Writer::default();
        tile_block(&mut w, RGB, b"z", ONE, &[], |l| {
            for v in [7, 8, 9] {
                raw_channel(l, 8, ONE, &[v]);
            }
            l.u32(1);
            l.u32(0);
            empty(l, 22);
        });
        assert_eq!(
            read_all(&w.data)["z"].image.get_pixel(0, 0).0,
            [7, 8, 9, 255]
        );
    }

    #[test]
    fn the_rectangle_need_not_start_at_zero() {
        let rect = [10, 20, 12, 23];
        let inner = [11, 21, 12, 22];
        let mut w = Writer::default();
        tile_block(&mut w, GRAY, b"o", rect, &[], |l| {
            raw_channel(l, 8, inner, &[99]);
            empty(l, 25);
        });
        let all = read_all(&w.data);
        let image = &all["o"].image;
        assert_eq!(image.dimensions(), (3, 2));
        assert_eq!(image.get_pixel(1, 1).0, [99, 99, 99, 255]);
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn the_palette_is_read_as_triples_with_or_without_its_extra_bytes() {
        let mut table = vec![0u8; 768];
        table[3..6].copy_from_slice(&[10, 20, 30]);
        let mut with = table.clone();
        with.extend_from_slice(&[0; 4]);
        for (id, extra) in [(&b"with"[..], &with), (&b"bare"[..], &table)] {
            let mut w = Writer::default();
            tile_block(&mut w, INDEXED, id, ONE, extra, |l| {
                raw_channel(l, 8, ONE, &[1]);
                empty(l, 25);
            });
            let all = read_all(&w.data);
            let key = std::str::from_utf8(id).unwrap();
            assert_eq!(all[key].image.get_pixel(0, 0).0, [10, 20, 30, 255]);
        }
    }

    #[test]
    fn cmyk_tiles_become_rgb() {
        let mut w = Writer::default();
        tile_block(&mut w, CMYK, b"k", ONE, &[], |l| {
            // Stored inverted: no cyan, full magenta, half yellow, no black.
            for v in [255, 0, 128, 255] {
                raw_channel(l, 8, ONE, &[v]);
            }
            raw_channel(l, 8, ONE, &[200]);
            empty(l, 21);
        });
        assert_eq!(
            read_all(&w.data)["k"].image.get_pixel(0, 0).0,
            [255, 0, 128, 200]
        );
    }

    #[test]
    fn link_with_layer_has_its_key_for_each_use() {
        let p = tile();
        let all: Patterns = [(identifier(&p), p.clone())].into();
        let f = PatternFill {
            linked: false,
            ..PatternFill::of(p.clone())
        };
        for key in [b"Lnkd", b"Algn"] {
            let d = fill_items(&f, Descriptor::new(b"null"), key);
            assert!(d.bool(key) == Some(false), "written under its key");
            assert!(!fill_of(&d, &all).unwrap().linked);
        }
    }

    #[test]
    fn values_that_are_not_numbers_are_not_kept() {
        let p = tile();
        let all: Patterns = [(identifier(&p), p.clone())].into();
        let f = PatternFill {
            scale: f32::NAN,
            angle: f32::INFINITY,
            phase: (f32::NAN, f32::NEG_INFINITY),
            ..PatternFill::of(p)
        };
        let back = fill_of(&fill_items(&f, Descriptor::new(b"null"), b"Lnkd"), &all).unwrap();
        assert_eq!(back.scale, 1.0);
        assert_eq!(back.angle, 0.0);
        assert_eq!(back.phase, (0.0, 0.0));
    }

    #[test]
    fn each_use_keeps_the_name_it_gave_the_tile() {
        let p = tile();
        let all: Patterns = [(identifier(&p), p.clone())].into();
        let f = PatternFill::of(p.renamed("Autre"));
        let back = fill_of(&fill_items(&f, Descriptor::new(b"null"), b"Lnkd"), &all).unwrap();
        assert_eq!(back.pattern.name, "Autre");
    }

    /// Random changes in a block: never a panic, whatever is read.
    #[test]
    fn random_damage_is_not_a_panic() {
        let mut w = Writer::default();
        let mut image = RgbaImage::from_fn(9, 7, |x, y| {
            Rgba([(x * 20) as u8, (y * 30) as u8, 7, (x * y * 3) as u8])
        });
        image.put_pixel(0, 0, Rgba([1, 2, 3, 0]));
        write_pattern(&mut w, &Pattern::new("fuzz é", image));
        write_pattern(&mut w, &tile());
        let base = w.data;
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let mut d = base.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() % d.len() as u64) as usize;
                d[i] = [0u8, 1, 0x7f, 0x80, 0xff, next() as u8][(next() % 6) as usize];
            }
            let _ = read_block(&d, &|_| true, &mut Budget::of(1 << 24));
        }
    }
}
