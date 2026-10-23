//! Writing: an 8-bit RGB (or CMYK) Photoshop document, every channel RLE
//! compressed, following the specification to the byte (Photoshop refuses
//! files that stray from it).

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::bytes::{Writer, pack_row};
use super::{adjust, artboard, blend_key, effects};
use crate::cms::Conversion;
use crate::document::native::par_map;
use crate::document::{BlendMode, Document, FileError, Layer, LayerKind, PixelRect};
use crate::guides::Orientation;
use crate::i18n::t;

/// Most channels a document may have.
const MAX_CHANNELS: usize = 56;

/// What a record's channels are made of, to compress on the other cores:
/// the pixels over their visible box (canvas coordinates) with their
/// layer's position, and the mask.
struct Pixels {
    rect: PixelRect,
    image: Option<(Arc<RgbaImage>, (i64, i64))>,
    mask: Option<GrayImage>,
}

/// Writes `doc`, in CMYK through `cmyk` if given (the layers' pixels and
/// the merged image converted); returns the file and what could not be
/// kept.
pub(super) fn document(
    doc: &Document,
    cmyk: Option<&Conversion>,
) -> Result<(Vec<u8>, Vec<String>), FileError> {
    let layers = doc.render_layers();
    let flat = doc.flatten();
    let (w, h) = (doc.width(), doc.height());
    let transparent = flat.pixels().any(|p| p.0[3] < 255);
    let colors = if cmyk.is_some() { 4 } else { 3 };
    let extra = MAX_CHANNELS - colors - usize::from(transparent);
    let channels = &doc.channels[..doc.channels.len().min(extra)];
    let notes = notes(doc, &layers, channels.len(), cmyk.is_some());

    // Records from the bottom up: a group's divider before its layers.
    let mut contents = Vec::with_capacity(layers.len());
    let mut target = 0;
    for (i, layer) in layers.iter().enumerate() {
        let mut groups: Vec<&Layer> = (i..layers.len())
            .filter(|&g| layers[g].is_group() && block_start(&layers, g) == i)
            .map(|g| &layers[g])
            .collect();
        groups.sort_by_key(|g| g.depth);
        contents.extend(groups.iter().map(|_| None));
        if i == doc.active {
            target = contents.len();
        }
        contents.push(Some(layer));
    }
    let pixels: Vec<Pixels> = contents
        .iter()
        .map(|c| {
            let empty = PixelRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            };
            let layer = c.filter(|l| l.has_pixels());
            let rect = layer.and_then(Layer::content_bounds).unwrap_or(empty);
            Pixels {
                rect,
                image: layer.map(|l| (l.shared_image(), l.offset)),
                mask: c.and_then(|l| l.mask.as_ref()).map(|m| m.image().clone()),
            }
        })
        .collect();
    let (channels_data, ()) = par_map(&pixels, |p| channels_of(p, cmyk), || ());
    let records: Vec<Record> = contents
        .iter()
        .zip(&pixels)
        .zip(channels_data)
        .map(|((c, p), channels)| record(*c, p.rect, channels))
        .collect();

    let mut out = Writer::default();
    out.bytes(b"8BPS");
    out.u16(1);
    out.zeros(6);
    out.u16((colors + usize::from(transparent) + channels.len()) as u16);
    out.u32(h);
    out.u32(w);
    out.u16(8);
    out.u16(if cmyk.is_some() { 4 } else { 3 });
    out.u32(0);
    let icc = match cmyk {
        Some(c) => Some(c.to.icc().as_slice()),
        None => doc.icc_profile.as_deref().map(Vec::as_slice),
    };
    out.block(1, |r| resources(r, doc, target, channels, icc));
    let mut overflow = false;
    out.block(1, |s| {
        s.block(4, |info| {
            let count = records.len() as i16;
            info.i16(if transparent { -count } else { count });
            for rec in &records {
                info.bytes(&rec.head);
            }
            for rec in &records {
                for (_, data) in &rec.channels {
                    info.bytes(data);
                }
            }
            overflow = info.data.len() > u32::MAX as usize;
        });
        // Global layer mask info: none.
        s.u32(0);
    });
    merged(&mut out, &flat, transparent, channels, cmyk);
    if overflow || out.data.len() > u32::MAX as usize {
        return Err(FileError::TooLarge(w, h));
    }
    Ok((out.data, notes))
}

/// What a Photoshop document of `doc` can't keep, `kept` of its alpha
/// channels written.
fn notes(doc: &Document, layers: &[Layer], kept: usize, cmyk: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if kept < doc.channels.len() {
        notes.push(t!(
            "xchg-psd-note-channels",
            count = doc.channels.len() - kept
        ));
    }
    let texts: Vec<&str> = layers
        .iter()
        .filter(|l| l.text.is_some())
        .map(|l| l.name.as_str())
        .collect();
    if !texts.is_empty() {
        notes.push(t!("xchg-psd-note-text-saved", count = texts.len()));
    }
    // Smart objects as their rendering: their contents, transform and
    // smart filters stay in Atelier documents.
    let smart: Vec<&str> = layers
        .iter()
        .filter(|l| l.is_smart())
        .map(|l| l.name.as_str())
        .collect();
    if !smart.is_empty() {
        notes.push(t!("xchg-psd-note-smart", names = smart.join(", ")));
    }
    // Shape layers as their rendering.
    let shapes: Vec<&str> = layers
        .iter()
        .filter(|l| l.shape.is_some())
        .map(|l| l.name.as_str())
        .collect();
    if !shapes.is_empty() {
        notes.push(t!("files3-psd-note-shapes", names = shapes.join(", ")));
    }
    if cmyk && layers.len() > 1 {
        notes.push(t!("color-psd-note-cmyk-layers"));
    }
    notes
}

/// What saving `doc` as a Photoshop document would lose (as [`document`]
/// says it, without writing).
pub(super) fn losses(doc: &Document) -> Vec<String> {
    // At most the channels left with a transparent merged image.
    notes(doc, &doc.render_layers(), MAX_CHANNELS - 4, false)
}

/// First index of the block of group `g` (itself and the layers below it
/// that are deeper).
fn block_start(layers: &[Layer], g: usize) -> usize {
    let d = layers[g].depth;
    let mut j = g;
    while j > 0 && layers[j - 1].depth > d {
        j -= 1;
    }
    j
}

fn resources(
    r: &mut Writer,
    doc: &Document,
    target: usize,
    channels: &[crate::document::Channel],
    icc: Option<&[u8]>,
) {
    // Its size, then its data padded to an even length (not counted).
    let resource = |r: &mut Writer, id: u16, f: &dyn Fn(&mut Writer)| {
        let mut data = Writer::default();
        f(&mut data);
        r.bytes(b"8BIM");
        r.u16(id);
        r.u16(0);
        r.u32(data.data.len() as u32);
        r.bytes(&data.data);
        r.zeros(data.data.len() % 2);
    };
    // 72 pixels per inch.
    resource(r, 1005, &|r| {
        for _ in 0..2 {
            r.u32(72 << 16);
            r.u16(1);
            r.u16(1);
        }
    });
    resource(r, 1024, &|r| r.u16(target as u16));
    if !doc.guides.is_empty() {
        resource(r, 1032, &|r| {
            r.u32(1);
            r.u32(576);
            r.u32(576);
            r.u32(doc.guides.len() as u32);
            for g in &doc.guides {
                r.i32((g.position * 32.0).round() as i32);
                r.u8(match g.orientation {
                    Orientation::Vertical => 0,
                    Orientation::Horizontal => 1,
                });
            }
        });
    }
    if !channels.is_empty() {
        resource(r, 1006, &|r| {
            for c in channels {
                r.pascal(&mac_roman(&c.name), 1);
            }
        });
        resource(r, 1045, &|r| {
            for c in channels {
                r.unicode(&c.name);
            }
        });
    }
    if let Some(icc) = icc {
        resource(r, 1039, &|r| r.bytes(icc));
    }
    // The Paths panel: the saved paths, named, then the Work Path.
    let size = (doc.width(), doc.height());
    for (i, p) in doc.paths.iter().take(SAVED_PATHS).enumerate() {
        let data = super::vector::records(&p.path, size);
        r.bytes(b"8BIM");
        r.u16(2000 + i as u16);
        r.pascal(&mac_roman(&p.name), 2);
        r.u32(data.len() as u32);
        r.bytes(&data);
        r.zeros(data.len() % 2);
    }
    if let Some(p) = &doc.work_path {
        resource(r, 1025, &|r| r.bytes(&super::vector::records(p, size)));
    }
}

/// Saved paths a file holds (resources 2000 to 2997).
pub(super) const SAVED_PATHS: usize = 998;

/// `text` in Mac OS Roman, what it can't hold as "?" (the record's name;
/// the "luni" block has the real one).
fn mac_roman(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| match c {
            ' '..='~' => c as u8,
            _ => {
                let mut buf = [0u8; 1];
                (128..=255u8)
                    .find(|&b| {
                        buf[0] = b;
                        super::read::mac_roman(&buf).starts_with(c)
                    })
                    .unwrap_or(b'?')
            }
        })
        .collect()
}

/// A record: its fixed part (with its blocks), and its channels' data.
struct Record {
    head: Vec<u8>,
    channels: Vec<(i16, Vec<u8>)>,
}

/// An empty channel: raw, no data.
fn empty_channel() -> Vec<u8> {
    vec![0, 0]
}

/// A plane of `w` × `h` bytes, RLE compressed (row lengths, then rows).
fn rle(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    if w == 0 || h == 0 {
        return empty_channel();
    }
    let mut rows = Vec::with_capacity(plane.len() / 2);
    let mut lengths = Vec::with_capacity(h);
    for row in plane.chunks_exact(w) {
        let at = rows.len();
        pack_row(row, &mut rows);
        lengths.push((rows.len() - at) as u16);
    }
    let mut out = Vec::with_capacity(2 + 2 * h + rows.len());
    out.extend_from_slice(&1u16.to_be_bytes());
    for n in lengths {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend_from_slice(&rows);
    out
}

/// One channel of `image` over `r` (canvas coordinates; the layer at
/// `offset`): `c` 0..3, 3 being alpha.
fn plane(image: &RgbaImage, offset: (i64, i64), r: PixelRect, c: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(r.w as usize * r.h as usize);
    for y in 0..r.h as i64 {
        let ly = (r.y + y - offset.1) as u32;
        let row = &image.as_raw()[(ly as usize * image.width() as usize) * 4..];
        let lx0 = (r.x - offset.0) as usize;
        out.extend(
            row[lx0 * 4..(lx0 + r.w as usize) * 4]
                .chunks_exact(4)
                .map(|p| p[c]),
        );
    }
    out
}

/// Photoshop's CMYK planes (stored inverted: 255 is no ink) of RGB
/// pixels (3 bytes each).
fn cmyk_planes(rgb: &[u8], cmyk: &Conversion) -> [Vec<u8>; 4] {
    let ink = cmyk
        .rgb_to_cmyk(rgb)
        .unwrap_or_else(|| vec![0; rgb.len() / 3 * 4]);
    std::array::from_fn(|c| ink.chunks_exact(4).map(|p| 255 - p[c]).collect())
}

/// The compressed channels of a record: transparency, red, green, blue
/// (or cyan, magenta, yellow, black), then the mask.
fn channels_of(p: &Pixels, cmyk: Option<&Conversion>) -> Vec<(i16, Vec<u8>)> {
    let r = p.rect;
    let (w, h) = (r.w as usize, r.h as usize);
    let mut channels: Vec<(i16, Vec<u8>)> = match (&p.image, cmyk) {
        (Some((image, offset)), Some(cmyk)) if w * h > 0 => {
            let planes: Vec<Vec<u8>> = (0..4).map(|c| plane(image, *offset, r, c)).collect();
            let rgb: Vec<u8> = (0..w * h)
                .flat_map(|i| [planes[0][i], planes[1][i], planes[2][i]])
                .collect();
            let alpha = (-1, rle(&planes[3], w, h));
            let inks = cmyk_planes(&rgb, cmyk).into_iter().enumerate();
            [alpha]
                .into_iter()
                .chain(inks.map(|(c, ink)| (c as i16, rle(&ink, w, h))))
                .collect()
        }
        _ => {
            let ids: &[(i16, usize)] = if cmyk.is_some() {
                &[(-1, 3), (0, 0), (1, 1), (2, 2), (3, 0)]
            } else {
                &[(-1, 3), (0, 0), (1, 1), (2, 2)]
            };
            ids.iter()
                .map(|&(id, c)| {
                    let data = match &p.image {
                        Some((image, offset)) if w * h > 0 => {
                            rle(&plane(image, *offset, r, c), w, h)
                        }
                        _ => empty_channel(),
                    };
                    (id, data)
                })
                .collect()
        }
    };
    if let Some(m) = &p.mask {
        let (mw, mh) = m.dimensions();
        channels.push((-2, rle(m.as_raw(), mw as usize, mh as usize)));
    }
    channels
}

fn record(layer: Option<&Layer>, rect: PixelRect, channels: Vec<(i16, Vec<u8>)>) -> Record {
    let mask = layer.and_then(|l| l.mask.as_ref().map(|m| (m, m.origin(l.offset))));

    let mut w = Writer::default();
    w.i32(rect.y as i32);
    w.i32(rect.x as i32);
    w.i32(rect.bottom() as i32);
    w.i32(rect.right() as i32);
    w.u16(channels.len() as u16);
    for (id, data) in &channels {
        w.i16(*id);
        w.u32(data.len() as u32);
    }
    w.bytes(b"8BIM");
    let blend = layer.map_or(BlendMode::Normal, |l| l.blend);
    w.bytes(blend_key(blend));
    w.u8(layer.map_or(255, |l| (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8));
    w.u8(u8::from(layer.is_some_and(|l| l.clipped)));
    let pixels = layer.is_some_and(Layer::has_pixels);
    let hidden = layer.is_some_and(|l| !l.visible);
    w.u8(8 | if pixels { 0 } else { 16 } | if hidden { 2 } else { 0 });
    w.u8(0);
    w.block(1, |x| {
        match mask {
            Some((m, (ox, oy))) => x.block(1, |x| mask_data(x, m, (ox, oy))),
            None => x.u32(0),
        }
        // Blending ranges: none.
        x.u32(0);
        let name = layer.map_or("</Layer group>", |l| l.name.as_str());
        x.pascal(&mac_roman(name), 4);
        x.tagged(b"luni", |x| x.unicode(name));
        let Some(l) = layer else {
            x.tagged(b"lsct", |x| x.u32(3));
            return;
        };
        x.tagged(b"lyid", |x| x.u32(l.id as u32));
        if l.fill < 1.0 {
            x.tagged(b"iOpa", |x| {
                x.u8((l.fill.clamp(0.0, 1.0) * 255.0).round() as u8)
            });
        }
        if l.locked {
            x.tagged(b"lspf", |x| x.u32(0x8000_0000));
        }
        match &l.kind {
            LayerKind::Group { expanded } => {
                x.tagged(b"lsct", |x| {
                    x.u32(if *expanded { 1 } else { 2 });
                    x.bytes(b"8BIM");
                    x.bytes(blend_key(l.blend));
                });
                artboard::write(l, x);
            }
            kind => adjust::write(kind, x),
        }
        if l.has_pixels()
            && let Some(fx) = effects::write(&l.effects)
        {
            x.tagged(b"lfx2", |x| {
                x.u32(0);
                x.u32(16);
                fx.write(x);
            });
        }
    });
    Record {
        head: w.data,
        channels,
    }
}

/// A mask's data: its box, default color, flags and, if not neutral, its
/// density and feather.
fn mask_data(x: &mut Writer, m: &crate::document::Mask, (ox, oy): (i64, i64)) {
    let (mw, mh) = m.image().dimensions();
    x.i32(oy as i32);
    x.i32(ox as i32);
    x.i32((oy + mh as i64) as i32);
    x.i32((ox + mw as i64) as i32);
    x.u8(m.default);
    let params = m.density < 1.0 || m.feather > 0.0;
    x.u8(if m.enabled { 0 } else { 2 } | if params { 16 } else { 0 });
    if params {
        x.u8(3);
        x.u8((m.density.clamp(0.0, 1.0) * 255.0).round() as u8);
        x.f64(m.feather as f64);
    }
    // Padded to 20 bytes, or to a multiple of 4.
    let len = 18 + if params { 10 } else { 0 };
    x.zeros((4 - len % 4) % 4);
}

/// The merged image: the colors (matted against white where transparent,
/// as Photoshop stores them), the transparency, then the alpha channels.
fn merged(
    out: &mut Writer,
    flat: &RgbaImage,
    transparent: bool,
    channels: &[crate::document::Channel],
    cmyk: Option<&Conversion>,
) {
    let (w, h) = (flat.width() as usize, flat.height() as usize);
    let matted = |c: usize| -> Vec<u8> {
        flat.pixels()
            .map(|p| {
                let a = p.0[3] as u32;
                ((p.0[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8
            })
            .collect()
    };
    let mut planes: Vec<Vec<u8>> = (0..3).map(matted).collect();
    if let Some(cmyk) = cmyk {
        let rgb: Vec<u8> = (0..w * h)
            .flat_map(|i| [planes[0][i], planes[1][i], planes[2][i]])
            .collect();
        planes = cmyk_planes(&rgb, cmyk).into();
    }
    if transparent {
        planes.push(flat.pixels().map(|p| p.0[3]).collect());
    }
    planes.extend(channels.iter().map(|c| fit(&c.mask, w, h)));
    let (packed, ()) = par_map(&planes, |p| rle(p, w, h), || ());
    out.u16(1);
    for p in &packed {
        out.bytes(&p[2..2 + 2 * h]);
    }
    for p in &packed {
        out.bytes(&p[2 + 2 * h..]);
    }
}

/// A saved selection's coverage over the canvas.
fn fit(mask: &GrayImage, w: usize, h: usize) -> Vec<u8> {
    if mask.dimensions() == (w as u32, h as u32) {
        return mask.as_raw().clone();
    }
    let mut out = vec![0; w * h];
    for (x, y, p) in mask.enumerate_pixels() {
        if (x as usize) < w && (y as usize) < h {
            out[y as usize * w + x as usize] = p.0[0];
        }
    }
    out
}
