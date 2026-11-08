//! Writing: an 8-bit RGB (or CMYK) Photoshop document, every channel RLE
//! compressed, following the specification to the byte (Photoshop refuses
//! files that stray from it).

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::bytes::{Writer, pack_row};
use super::{adjust, artboard, blend_key, effects, pattern};
use crate::cms::Conversion;
use crate::document::native::{check_layer, par_map};
use crate::document::{BlendMode, Document, FileError, Fill, Layer, LayerKind, PixelRect};
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
    /// The mask's image and its origin in the canvas.
    mask: Option<(GrayImage, (i64, i64))>,
}

/// Largest side of a layer or mask box in a PSD (version 1), past which
/// readers refuse the file (ag-psd) or the layer (psd-tools); a PSB holds
/// 300,000, more than Atelier's layers reach.
const PSD_SIDE: u32 = 30_000;

/// What a PSD keeps of box `r` (canvas coordinates): `r`, but along a side
/// longer than [`PSD_SIDE`], the 30,000 px that hold the canvas (never
/// larger) and as much of `r` as they can.
fn psd_box(r: PixelRect, (cw, ch): (u32, u32)) -> PixelRect {
    let axis = |at: i64, len: u32, canvas: u32| -> (i64, u32) {
        if len <= PSD_SIDE {
            return (at, len);
        }
        let side = i64::from(PSD_SIDE);
        let lo = at.clamp((i64::from(canvas) - side).min(0), 0);
        let (a, b) = (at.max(lo), (at + i64::from(len)).min(lo + side));
        (a, (b - a).max(0) as u32)
    };
    let (x, w) = axis(r.x, r.w, cw);
    let (y, h) = axis(r.y, r.h, ch);
    PixelRect { x, y, w, h }
}

/// The box of a layer's mask (canvas coordinates).
fn mask_box(l: &Layer) -> Option<PixelRect> {
    let m = l.mask.as_ref()?;
    let (x, y) = m.origin(l.offset);
    let (w, h) = m.image().dimensions();
    Some(PixelRect { x, y, w, h })
}

/// Writes `doc`, in CMYK through `cmyk` if given (the layers' pixels and
/// the merged image converted), as a PSB if `big`; returns the file and
/// what could not be kept.
pub(super) fn document(
    doc: &Document,
    cmyk: Option<&Conversion>,
    big: bool,
) -> Result<(Vec<u8>, Vec<String>), FileError> {
    let layers = crate::document::group_overlays_for_files(doc.render_layers());
    let flat = doc.flatten();
    let (w, h) = (doc.width(), doc.height());
    let transparent = flat.pixels().any(|p| p.0[3] < 255);
    let colors = if cmyk.is_some() { 4 } else { 3 };
    let extra = MAX_CHANNELS - colors - usize::from(transparent);
    let channels = &doc.channels[..doc.channels.len().min(extra)];
    let notes = notes(doc, &layers, channels.len(), cmyk.is_some(), big);
    let canvas = (w, h);
    let fit = |r: PixelRect| if big { r } else { psd_box(r, canvas) };

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
        // Clipped to the layer below in its group, as readers keep it (at
        // the bottom of a group, clipping holds nothing).
        let clipped = layer.clipped && i > 0 && layers[i - 1].depth >= layer.depth;
        contents.push(Some((layer, clipped)));
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
            let layer = c.map(|(l, _)| l).filter(|l| l.has_pixels());
            let rect = layer.and_then(Layer::content_bounds).unwrap_or(empty);
            let mask = c.and_then(|(l, _)| {
                let (m, whole) = (l.mask.as_ref()?, mask_box(l)?);
                let kept = fit(whole);
                let image = if kept == whole {
                    m.image().clone()
                } else {
                    let (x, y) = ((kept.x - whole.x) as u32, (kept.y - whole.y) as u32);
                    image::imageops::crop_imm(m.image(), x, y, kept.w, kept.h).to_image()
                };
                Some((image, (kept.x, kept.y)))
            });
            Pixels {
                rect: fit(rect),
                image: layer.map(|l| (l.shared_image(), l.offset)),
                mask,
            }
        })
        .collect();
    for (c, p) in contents.iter().zip(&pixels) {
        if let Some((layer, _)) = c {
            check_layer(layer, p.rect.w, p.rect.h)?;
            if let Some((m, _)) = &p.mask {
                check_layer(layer, m.width(), m.height())?;
            }
        }
    }
    let (channels_data, ()) = par_map(&pixels, |p| channels_of(p, cmyk, big), || ());
    let records: Vec<Record> = contents
        .iter()
        .zip(&pixels)
        .zip(channels_data)
        .map(|((c, p), channels)| record(*c, p, channels, big))
        .collect();

    let mut out = Writer::default();
    out.bytes(b"8BPS");
    out.u16(if big { 2 } else { 1 });
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
    // Linked layers: a group id per record (dividers and unlinked layers 0).
    let mut groups: Vec<u64> = Vec::new();
    let links: Vec<u16> = contents
        .iter()
        .map(|c| match c.and_then(|(l, _)| l.link) {
            Some(link) => {
                let at = groups.iter().position(|&g| g == link).unwrap_or_else(|| {
                    groups.push(link);
                    groups.len() - 1
                });
                (at + 1).min(u16::MAX as usize) as u16
            }
            None => 0,
        })
        .collect();
    out.block(1, |r| resources(r, doc, target, &links, channels, icc));
    let at = out.data.len();
    out.sized(big, 1, |s| {
        s.sized(big, 4, |info| {
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
        });
        // Global layer mask info: none.
        s.u32(0);
        pattern::write_block(s, &pattern::used(&layers));
        unicode_path_names(s, doc, big);
    });
    // A PSD's lengths take 4 bytes: past 4 GB, only a PSB holds the layers.
    if !big && out.data.len() - at - 4 > u32::MAX as usize {
        return Err(FileError::PsdOver4Gb);
    }
    merged(&mut out, &flat, transparent, channels, cmyk, big);
    Ok((out.data, notes))
}

/// What a Photoshop document of `doc` can't keep, `kept` of its alpha
/// channels written.
fn notes(doc: &Document, layers: &[Layer], kept: usize, cmyk: bool, big: bool) -> Vec<String> {
    let mut notes = Vec::new();
    // Layers and masks past a PSD's sides, cropped.
    let canvas = (doc.width(), doc.height());
    let cropped: Vec<&str> = layers
        .iter()
        .filter(|_| !big)
        .filter(|l| {
            let pixels = l.has_pixels().then(|| l.content_bounds()).flatten();
            pixels
                .into_iter()
                .chain(mask_box(l))
                .any(|r| psd_box(r, canvas) != r)
        })
        .map(|l| l.name.as_str())
        .collect();
    if !cropped.is_empty() {
        notes.push(t!(
            "psd14-psd-note-cropped",
            names = cropped.join(", "),
            max = PSD_SIDE
        ));
    }
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
    if placed_lost(doc) {
        notes.push(t!("files6-psd-note-placed"));
    }
    notes
}

/// What saving `doc` as a Photoshop document would lose (as [`document`]
/// says it, without writing).
pub(super) fn losses(doc: &Document, big: bool) -> Vec<String> {
    // The channels [`document`] keeps; only that many need the merged
    // image's transparency to tell.
    let mut kept = doc.channels.len();
    if kept > MAX_CHANNELS - 4 {
        let transparent = doc.flatten().pixels().any(|p| p.0[3] < 255);
        kept = kept.min(MAX_CHANNELS - 3 - usize::from(transparent));
    }
    notes(doc, &doc.render_layers(), kept, false, big)
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

/// Resources placed on the canvas: slices, color samplers, Count marks.
const PLACED: [u16; 3] = [1050, 1073, 1080];

/// The canvas is the one the kept resources were placed on (or isn't
/// known: as they were).
fn placed_fit(doc: &Document) -> bool {
    let layout = doc.file_metadata.psd_layout.as_ref();
    layout.is_none_or(|l| l.canvas == (doc.width(), doc.height()))
}

/// The placed resources that would be left out, but slices that are only
/// the one of the whole image (Photoshop makes it again).
fn placed_lost(doc: &Document) -> bool {
    !placed_fit(doc)
        && doc.file_metadata.psd_resources.iter().any(|r| match r.id {
            1050 => slices(&r.data).is_none_or(|n| n > 1),
            id => PLACED.contains(&id),
        })
}

/// The slices of a Slices resource (version 6, or a descriptor's list).
fn slices(data: &[u8]) -> Option<usize> {
    let mut r = super::bytes::Reader::new(data);
    match r.u32().ok()? {
        6 => {
            r.skip(16).ok()?;
            r.unicode().ok()?;
            Some(r.u32().ok()? as usize)
        }
        7 | 8 => {
            r.skip(4).ok()?;
            let d = super::descriptor::Descriptor::read(&mut r).ok()?;
            Some(d.list(b"slices").map_or(0, <[_]>::len))
        }
        _ => None,
    }
}

/// The display settings (1077) of the `channels` written, each taken
/// from the file's channel of its name (a new one gets the default), spot
/// colors as the alpha channels they became; `None` without any.
fn display_info(
    data: &[u8],
    doc: &Document,
    channels: &[crate::document::Channel],
) -> Option<Vec<u8>> {
    const ENTRY: usize = 13;
    let entries: Vec<&[u8]> = data.get(4..)?.chunks_exact(ENTRY).collect();
    let names = match &doc.file_metadata.psd_layout {
        Some(l) => l.channels.clone(),
        None => channels.iter().map(|c| c.name.clone()).collect(),
    };
    if channels.is_empty() || entries.len() != names.len() {
        return None;
    }
    let mut used = vec![false; names.len()];
    let mut out = data[..4].to_vec();
    for c in channels {
        let found = (0..names.len()).find(|&i| !used[i] && names[i] == c.name);
        match found {
            Some(i) => {
                used[i] = true;
                out.extend(&entries[i][..12]);
                out.push(match entries[i][12] {
                    2 => 0,
                    kind => kind,
                });
            }
            // Red at 50 %, showing the selected areas.
            None => out.extend([0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 50, 0]),
        }
    }
    Some(out)
}

fn resources(
    r: &mut Writer,
    doc: &Document,
    target: usize,
    links: &[u16],
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
    // What the file opened held beside the layers, as it was: its
    // resolution first (72 pixels per inch without one).
    let kept = &doc.file_metadata.psd_resources;
    // Its XMP and EXIF made true for the image saved (upright, its size).
    let canvas = (doc.width(), doc.height());
    let raw = |r: &mut Writer, res: &crate::document::PsdResource| {
        use std::borrow::Cow;
        let data = match res.id {
            1060 => Cow::Owned(crate::document::file_io::fresh_xmp(&res.data, canvas)),
            1058 => Cow::Owned(crate::document::file_io::fresh_exif(&res.data, canvas)),
            1077 => match display_info(&res.data, doc, channels) {
                Some(data) => Cow::Owned(data),
                None => return,
            },
            // Placed for another canvas: left out (said by `notes`).
            id if PLACED.contains(&id) && !placed_fit(doc) => return,
            // The clipping path names a saved path: gone with it.
            2999 if !clipping_path_kept(&res.data, doc) => return,
            _ => Cow::Borrowed(&res.data[..]),
        };
        r.bytes(b"8BIM");
        r.u16(res.id);
        r.pascal(&res.name, 2);
        r.u32(data.len() as u32);
        r.bytes(&data);
        r.zeros(data.len() % 2);
    };
    // Crop's Resolution: the document's value over what the file said.
    let density = doc.file_metadata.image.as_ref().and_then(|m| m.density);
    // Signed 16.16 (Fixed).
    let fixed = |ppi: f64| (ppi.clamp(1.0, 32767.0) * 65536.0).round() as u32;
    match (kept.iter().find(|k| k.id == 1005), density) {
        (Some(res), None) => raw(r, res),
        // Its units stay, the values in pixels per inch.
        (Some(res), Some((x, y))) if res.data.len() >= 16 => {
            let mut data = res.data.to_vec();
            data[..4].copy_from_slice(&fixed(x).to_be_bytes());
            data[8..12].copy_from_slice(&fixed(y).to_be_bytes());
            resource(r, 1005, &|r| r.bytes(&data));
        }
        (_, density) => {
            let (x, y) = density.unwrap_or((72.0, 72.0));
            resource(r, 1005, &|r| {
                for ppi in [x, y] {
                    r.u32(fixed(ppi));
                    r.u16(1);
                    r.u16(1);
                }
            });
        }
    }
    for res in kept.iter().filter(|k| k.id != 1005) {
        raw(r, res);
    }
    resource(r, 1024, &|r| r.u16(target as u16));
    if links.iter().any(|&l| l != 0) {
        resource(r, 1026, &|r| links.iter().for_each(|&l| r.u16(l)));
        // Each link group on (none is turned off for a while).
        resource(r, 1072, &|r| links.iter().for_each(|_| r.u8(1)));
    }
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
    if !doc.samplers.is_empty() {
        // The current block, in the one form the specification and ag-psd
        // read alike: version 2, floats, a depth. (Vertical first: see the
        // reader.)
        resource(r, 1073, &|r| {
            r.u32(2);
            r.u32(doc.samplers.len() as u32);
            for s in &doc.samplers {
                r.u32(s.y.to_bits());
                r.u32(s.x.to_bits());
                // The color space shown: as the document.
                r.i16(-1);
                r.u16(1);
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

/// Whether the clipping path (resource 2999: a path's name first) names
/// one of the saved paths written.
fn clipping_path_kept(data: &[u8], doc: &Document) -> bool {
    let Some(name) = data.first().and_then(|&n| data.get(1..=n as usize)) else {
        return false;
    };
    doc.paths
        .iter()
        .take(SAVED_PATHS)
        .any(|p| mac_roman(&p.name) == name)
}

/// The saved paths' names in full ("pths", global: resources only hold
/// them in Mac OS Roman), when one needs it.
fn unicode_path_names(w: &mut Writer, doc: &Document, big: bool) {
    use super::descriptor::{Descriptor, Value};
    let paths = &doc.paths[..doc.paths.len().min(SAVED_PATHS)];
    let roman = |n: &str| super::read::mac_roman(&mac_roman(n)) == n;
    if paths.iter().all(|p| roman(&p.name)) {
        return;
    }
    let list = paths
        .iter()
        .map(|p| {
            let info = Descriptor::new(b"pathInfoClass")
                .with(b"pathUnicodeName", Value::Text(p.name.clone()));
            Value::Object(info)
        })
        .collect();
    let d = Descriptor::new(b"pathsDataClass").with(b"pathList", Value::List(list));
    w.tagged_sized(b"pths", big, |w| {
        w.u32(16);
        d.write(w);
    });
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

/// A plane of `w` × `h` bytes, RLE compressed (row lengths, then rows):
/// lengths of 2 bytes, 4 in a PSB (`big`). A PSD's rows are at most
/// [`PSD_SIDE`] bytes, which pack within 2 bytes.
fn rle(plane: &[u8], w: usize, h: usize, big: bool) -> Vec<u8> {
    if w == 0 || h == 0 {
        return empty_channel();
    }
    debug_assert!(big || w <= PSD_SIDE as usize);
    let mut rows = Vec::with_capacity(plane.len() / 2);
    let mut lengths = Vec::with_capacity(h);
    for row in plane.chunks_exact(w) {
        let at = rows.len();
        pack_row(row, &mut rows);
        lengths.push(rows.len() - at);
    }
    let count = if big { 4 } else { 2 };
    let mut out = Vec::with_capacity(2 + count * h + rows.len());
    out.extend_from_slice(&1u16.to_be_bytes());
    for n in lengths {
        if big {
            out.extend_from_slice(&(n as u32).to_be_bytes());
        } else {
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
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
fn channels_of(p: &Pixels, cmyk: Option<&Conversion>, big: bool) -> Vec<(i16, Vec<u8>)> {
    let rle = |plane: &[u8], w, h| rle(plane, w, h, big);
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
    if let Some((m, _)) = &p.mask {
        let (mw, mh) = m.dimensions();
        channels.push((-2, rle(m.as_raw(), mw as usize, mh as usize)));
    }
    channels
}

fn record(
    content: Option<(&Layer, bool)>,
    p: &Pixels,
    channels: Vec<(i16, Vec<u8>)>,
    big: bool,
) -> Record {
    let (layer, rect) = (content.map(|(l, _)| l), p.rect);
    let mask = layer
        .and_then(|l| l.mask.as_ref())
        .zip(p.mask.as_ref().map(|(image, at)| (image.dimensions(), *at)));

    let mut w = Writer::default();
    w.i32(rect.y as i32);
    w.i32(rect.x as i32);
    w.i32(rect.bottom() as i32);
    w.i32(rect.right() as i32);
    w.u16(channels.len() as u16);
    for (id, data) in &channels {
        w.i16(*id);
        if big {
            w.u64(data.len() as u64);
        } else {
            w.u32(data.len() as u32);
        }
    }
    w.bytes(b"8BIM");
    let blend = layer.map_or(BlendMode::Normal, |l| l.blend);
    w.bytes(blend_key(blend));
    w.u8(layer.map_or(255, |l| (l.opacity.clamp(0.0, 1.0) * 255.0).round() as u8));
    w.u8(u8::from(content.is_some_and(|(_, clipped)| clipped)));
    let pixels = layer.is_some_and(Layer::has_pixels);
    let hidden = layer.is_some_and(|l| !l.visible);
    w.u8(8 | if pixels { 0 } else { 16 } | if hidden { 2 } else { 0 });
    w.u8(0);
    w.block(1, |x| {
        match mask {
            Some((m, (size, at))) => x.block(1, |x| mask_data(x, m, size, at)),
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
        if l.locked || l.locks.any() {
            let bits = u32::from(l.locks.transparent)
                | u32::from(l.locks.pixels) << 1
                | u32::from(l.locks.position) << 2
                | u32::from(l.locked) << 31;
            x.tagged(b"lspf", |x| x.u32(bits));
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
            LayerKind::Fill(Fill::Pattern(p)) => adjust::write(
                &LayerKind::Fill(Fill::Pattern(pattern::rebased(p, l.offset, rect))),
                x,
            ),
            kind => adjust::write(kind, x),
        }
        // The overlay's phase counts from the record's box once it is kept:
        // the default one, unused, stays out of the file.
        let mut styles = l.effects.clone();
        if !l.is_group() && styles.pattern_overlay.in_use() {
            let overlay = &mut styles.pattern_overlay.fill;
            *overlay = pattern::rebased(overlay, l.offset, rect);
        }
        if l.styleable()
            && let Some(fx) = effects::write(&styles)
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
fn mask_data(x: &mut Writer, m: &crate::document::Mask, (mw, mh): (u32, u32), (ox, oy): (i64, i64)) {
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
    big: bool,
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
    let (packed, ()) = par_map(&planes, |p| rle(p, w, h, big), || ());
    let counts = 2 + if big { 4 } else { 2 } * h;
    out.u16(1);
    for p in &packed {
        out.bytes(&p[2..counts]);
    }
    for p in &packed {
        out.bytes(&p[counts..]);
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
