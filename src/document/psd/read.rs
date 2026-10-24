//! Reading: the file's sections, the channels of each layer decoded to 8
//! bits, then the Atelier document built from them.

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::bytes::{Reader, Result, unpack_row};
use super::descriptor::Descriptor;
use super::{Imported, adjust, artboard, blend_of, effects, vector};
use crate::document::file_io::{Budget, check_size, layer_fits};
use crate::document::native::par_map;
use crate::document::{BlendMode, Channel, Document, FileError, LayerKind, Mask, PsdResource};
use crate::guides::{Guide, Orientation};
use crate::i18n::t;

/// Color modes.
const BITMAP: u16 = 0;
const GRAYSCALE: u16 = 1;
const INDEXED: u16 = 2;
const RGB: u16 = 3;
const CMYK: u16 = 4;
const MULTICHANNEL: u16 = 7;
const DUOTONE: u16 = 8;
const LAB: u16 = 9;

struct Header {
    /// PSB (version 2): larger lengths in places.
    big: bool,
    channels: u16,
    width: u32,
    height: u32,
    depth: u16,
    mode: u16,
    /// Color profile (from the resources), the CMYK and gray pixels'.
    icc: Option<Vec<u8>>,
}

fn header(r: &mut Reader) -> Result<Header> {
    if r.bytes(4)? != b"8BPS" {
        return Err(FileError::NotPsd);
    }
    let big = match r.u16()? {
        1 => false,
        2 => true,
        _ => return Err(FileError::Damaged),
    };
    r.skip(6)?;
    let channels = r.u16()?;
    let height = r.u32()?;
    let width = r.u32()?;
    let depth = r.u16()?;
    let mode = r.u16()?;
    // Photoshop's limit is 56 channels.
    if channels == 0 || channels > 56 || ![1, 8, 16, 32].contains(&depth) {
        return Err(FileError::Damaged);
    }
    Ok(Header {
        big,
        channels,
        width,
        height,
        depth,
        mode,
        icc: None,
    })
}

/// Color channels of a mode (before the alpha channels).
fn color_channels(mode: u16) -> usize {
    match mode {
        RGB | LAB => 3,
        CMYK => 4,
        _ => 1,
    }
}

/// Image resources Atelier uses.
#[derive(Default)]
struct Resources {
    icc: Option<Vec<u8>>,
    alpha_names: Vec<String>,
    /// Identifiers of the alpha channels.
    alpha_ids: Vec<u32>,
    /// Index of the active layer's record.
    target_layer: Option<usize>,
    guides: Vec<Guide>,
    global_angle: Option<f64>,
    /// JPEG preview.
    thumbnail: Option<Vec<u8>>,
    /// The Paths panel's saved paths (name, records) and Work Path.
    paths: Vec<(String, Vec<u8>)>,
    work_path: Option<Vec<u8>>,
    /// Resources written back as they are (see [`KEPT`]).
    kept: Vec<PsdResource>,
    /// Layer comps, which name layers by identifiers saving changes.
    comps: bool,
    /// What saving would lose besides (see [`resources`]).
    animation: bool,
    spot: bool,
    variables: bool,
    other: bool,
}

/// Image resources that describe the document rather than its layers,
/// written back unchanged: caption, background color, resolution, print
/// settings, halftone and transfer functions, IPTC, copyright flag and
/// URL, global light, spot halftone, document ids, slices, URL list, EXIF,
/// caption digest, XMP, print scale, pixel aspect ratio, HDR toning,
/// color samplers, measurement scale, Count tool, print style, clipping
/// path, Lightroom workflow, print flags.
const KEPT: [u16; 35] = [
    1005, 1008, 1010, 1011, 1013, 1016, 1028, 1034, 1035, 1037, 1042, 1043, 1044, 1049, 1050, 1054,
    1058, 1059, 1060, 1061, 1062, 1064, 1070, 1071, 1073, 1074, 1080, 1082, 1083, 1084, 1085, 2999,
    3000, 8000, 10000,
];

/// Image resources Atelier writes its own of, or that only describe the
/// file as it was (its thumbnail, version, quick mask, layer and channel
/// bookkeeping): left out without loss.
const REWRITTEN: [u16; 21] = [
    1000, 1001, 1002, 1003, 1004, 1022, 1026, 1033, 1036, 1041, 1046, 1047, 1057, 1066, 1069, 1072,
    1076, 1078, 1086, 1087, 1088,
];

/// The frames of a frame animation (plug-in resource "mani", block
/// "AnDs": its descriptor's list "FrIn"), when it can be read.
fn animation_frames(data: &[u8]) -> Option<usize> {
    let at = data.windows(4).position(|w| w == b"AnDs")? + 4;
    let mut r = Reader::new(data.get(at..)?);
    r.skip(8).ok()?;
    let d = Descriptor::read(&mut r).ok()?;
    Some(d.list(b"FrIn").map_or(1, <[_]>::len))
}

/// DisplayInfo (1077, or 1007 in old files) says a channel is a spot
/// color's (kind 2).
fn has_spot(id: u16, data: &[u8]) -> bool {
    let (start, stride) = if id == 1077 { (4, 13) } else { (0, 14) };
    data.get(start..)
        .is_some_and(|d| d.chunks_exact(stride).any(|c| c[12] == 2))
}

fn resources(r: &mut Reader) -> Result<Resources> {
    let mut out = Resources::default();
    let mut ascii_names = Vec::new();
    while r.remaining() >= 12 && r.bytes(4)? == b"8BIM" {
        let id = r.u16()?;
        let name = r.pascal(2)?;
        let len = r.u32()? as usize;
        let raw = r.bytes(len)?;
        let mut d = Reader::new(raw);
        r.skip(len % 2)?;
        let kept = || PsdResource {
            id,
            name: name.to_vec(),
            data: raw.to_vec(),
        };
        match id {
            1039 => out.icc = Some(d.bytes(len)?.to_vec()),
            1006 => {
                while d.remaining() > 0 {
                    ascii_names.push(mac_roman(d.pascal(1)?));
                }
            }
            1045 => {
                while d.remaining() >= 4 {
                    out.alpha_names.push(d.unicode()?);
                }
            }
            1053 => {
                while d.remaining() >= 4 {
                    out.alpha_ids.push(d.u32()?);
                }
            }
            1024 => out.target_layer = Some(d.u16()? as usize),
            1032 => {
                d.skip(12)?;
                let n = d.u32()? as usize;
                for _ in 0..n.min(d.remaining() / 5) {
                    let at = d.i32()? as f32 / 32.0;
                    let orientation = match d.u8()? {
                        0 => Orientation::Vertical,
                        _ => Orientation::Horizontal,
                    };
                    out.guides.push(Guide::new(orientation, at));
                }
            }
            1037 => {
                out.global_angle = Some(d.i32()? as f64);
                out.kept.push(kept());
            }
            1025 => out.work_path = Some(d.bytes(len)?.to_vec()),
            2000..=2997 => out.paths.push((mac_roman(name), d.bytes(len)?.to_vec())),
            // Format (1: JPEG), sizes…, then the JFIF data.
            1036 if len > 28 && d.u32()? == 1 => {
                d.skip(24)?;
                out.thumbnail = Some(d.bytes(len - 28)?.to_vec());
            }
            1065 => out.comps = true,
            1007 | 1077 => out.spot |= has_spot(id, raw),
            1067 => out.spot = true,
            // A frame animation (and its timeline) names its layers.
            1075 => out.animation = true,
            // Photoshop's animation data: one frame is just the layers.
            4000..=4999 if raw.windows(4).any(|w| w == b"mani") => {
                out.animation |= animation_frames(raw).is_none_or(|n| n > 1);
            }
            4000..=4999 => out.kept.push(kept()),
            7000 | 7001 | 7003 => out.variables = true,
            _ if KEPT.contains(&id) => out.kept.push(kept()),
            _ if REWRITTEN.contains(&id) => {}
            // Never silently: what isn't known is a loss.
            _ => out.other = true,
        }
    }
    if out.alpha_names.is_empty() {
        out.alpha_names = ascii_names;
    }
    Ok(out)
}

/// Text in the Mac OS Roman encoding (Pascal strings).
pub(super) fn mac_roman(bytes: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø\
                        ¿¡¬√ƒ≈∆«»…\u{a0}ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{f8ff}\
                        ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    let high: Vec<char> = HIGH.chars().collect();
    bytes
        .iter()
        .map(|&b| match b {
            0..=127 => b as char,
            _ => high.get(b as usize - 128).copied().unwrap_or('?'),
        })
        .collect()
}

/// A layer record, with its channels' data.
struct Record<'a> {
    /// Top, left, bottom, right.
    rect: [i32; 4],
    blend: [u8; 4],
    opacity: u8,
    clipped: bool,
    hidden: bool,
    /// Blend If ranges other than the full ones.
    blend_if: bool,
    mask: Option<MaskData>,
    name: String,
    blocks: Vec<([u8; 4], &'a [u8])>,
    /// By channel id, compression first.
    channels: Vec<(i16, &'a [u8])>,
}

impl Record<'_> {
    fn block(&self, key: &[u8; 4]) -> Option<&[u8]> {
        self.blocks.iter().find(|(k, _)| k == key).map(|(_, d)| *d)
    }

    /// The vector mask's block.
    fn vector_mask(&self) -> Option<&[u8]> {
        self.block(b"vmsk").or_else(|| self.block(b"vsms"))
    }

    fn user_mask(&self) -> Option<UserMask> {
        self.mask?.user(self.vector_mask().is_some())
    }
}

/// Refuses a layer or mask box no file can hold (the same limit as the
/// native reader's and both writers', [`layer_fits`]; PSD or PSB).
fn check_rect(rect: [i32; 4]) -> Result<()> {
    let (w, h) = size(rect);
    if !layer_fits(w, h) {
        return Err(FileError::Damaged);
    }
    Ok(())
}

/// Width and height of a top, left, bottom, right rectangle.
fn size([top, left, bottom, right]: [i32; 4]) -> (u32, u32) {
    let side = |a: i32, b: i32| (b as i64 - a as i64).clamp(0, u32::MAX as i64) as u32;
    (side(left, right), side(top, bottom))
}

#[derive(Clone, Copy)]
struct MaskData {
    /// The mask of channel -2: its box (top, left, bottom, right), value
    /// outside it, flags.
    rect: [i32; 4],
    default: u8,
    flags: u8,
    /// Channel -3's, when there is one.
    real: Option<([i32; 4], u8, u8)>,
    density: Option<u8>,
    feather: Option<f64>,
    vector_density: Option<u8>,
    vector_feather: Option<f64>,
}

/// The pixel mask the user made: its channel, box, default value, and
/// whether it is off.
struct UserMask {
    channel: i16,
    rect: [i32; 4],
    default: u8,
    disabled: bool,
}

impl MaskData {
    /// The pixel mask, as Photoshop stores it next to a vector mask: when
    /// channel -2 holds the vector mask's rendering ("came from rendering
    /// other data"), the user's is channel -3, if any.
    fn user(&self, vector: bool) -> Option<UserMask> {
        let disabled = self.flags & 2 != 0;
        match self.real {
            Some((rect, default, flags)) => Some(UserMask {
                channel: -3,
                rect,
                default,
                disabled: disabled || flags & 2 != 0,
            }),
            None if vector && self.flags & 8 != 0 => None,
            None => Some(UserMask {
                channel: -2,
                rect: self.rect,
                default: self.default,
                disabled,
            }),
        }
    }
}

/// Keys of the tagged blocks whose length takes 8 bytes in a PSB.
const BIG_KEYS: [&[u8; 4]; 13] = [
    b"LMsk", b"Lr16", b"Lr32", b"Layr", b"Mt16", b"Mt32", b"Mtrn", b"Alph", b"FMsk", b"lnk2",
    b"FEid", b"FXid", b"PxSD",
];

fn tagged_blocks<'a>(r: &mut Reader<'a>, big: bool) -> Result<Vec<([u8; 4], &'a [u8])>> {
    let mut blocks = Vec::new();
    while r.remaining() >= 12 {
        let mut peek = r.clone();
        let sig = peek.bytes(4)?;
        if sig != b"8BIM" && sig != b"8B64" {
            break;
        }
        r.skip(4)?;
        let key = r.tag()?;
        let len = r.len(big && BIG_KEYS.contains(&&key))?;
        blocks.push((key, r.bytes(len)?));
    }
    Ok(blocks)
}

/// The records of a layer info section, then their channels' data; and
/// the layer count's sign (negative: the merged image's first alpha
/// channel is its transparency).
fn layer_info<'a>(r: &mut Reader<'a>, big: bool) -> Result<(Vec<Record<'a>>, bool)> {
    let count = r.i16()?;
    let mut records = Vec::new();
    let mut lengths = Vec::new();
    for _ in 0..count.unsigned_abs() {
        let (rec, lens) = record(r, big)?;
        records.push(rec);
        lengths.push(lens);
    }
    for (rec, lens) in records.iter_mut().zip(lengths) {
        for (id, len) in lens {
            rec.channels.push((id, r.bytes(len)?));
        }
    }
    Ok((records, count < 0))
}

fn record<'a>(r: &mut Reader<'a>, big: bool) -> Result<(Record<'a>, Vec<(i16, usize)>)> {
    let rect = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
    check_rect(rect)?;
    let n = r.u16()? as usize;
    if n > 56 {
        return Err(FileError::Damaged);
    }
    let mut lengths = Vec::with_capacity(n);
    for _ in 0..n {
        let id = r.i16()?;
        lengths.push((id, r.len(big)?));
    }
    if r.bytes(4)? != b"8BIM" {
        return Err(FileError::Damaged);
    }
    let blend = r.tag()?;
    let opacity = r.u8()?;
    let clipped = r.u8()? != 0;
    let flags = r.u8()?;
    r.u8()?;
    let extra = r.u32()? as usize;
    let mut x = r.sub(extra)?;
    let mask_len = x.u32()? as usize;
    let has_real = lengths.iter().any(|(id, _)| *id == -3);
    let mask = (mask_len > 0)
        .then(|| mask_data(&mut x.sub(mask_len)?, mask_len, has_real))
        .transpose()?;
    let ranges = x.u32()? as usize;
    let blend_if = x
        .bytes(ranges)?
        .chunks_exact(4)
        .any(|r| r != [0, 0, 255, 255]);
    let mut name = mac_roman(x.pascal(4)?);
    let blocks = tagged_blocks(&mut x, big)?;
    if let Some((_, d)) = blocks.iter().find(|(k, _)| k == b"luni") {
        name = Reader::new(d).unicode()?;
    }
    let rec = Record {
        rect,
        blend,
        opacity,
        clipped,
        hidden: flags & 2 != 0,
        blend_if,
        mask,
        name,
        blocks,
        channels: Vec::with_capacity(n),
    };
    Ok((rec, lengths))
}

fn mask_data(r: &mut Reader, len: usize, has_real: bool) -> Result<MaskData> {
    let rect = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
    check_rect(rect)?;
    let default = r.u8()?;
    let flags = r.u8()?;
    let mut m = MaskData {
        rect,
        default,
        flags,
        real: None,
        density: None,
        feather: None,
        vector_density: None,
        vector_feather: None,
    };
    // As Photoshop writes it: the real mask (only with its channel), then
    // the parameters.
    if has_real && len >= 36 {
        let flags = r.u8()?;
        let default = r.u8()?;
        let rect = [r.i32()?, r.i32()?, r.i32()?, r.i32()?];
        check_rect(rect)?;
        m.real = Some((rect, default, flags));
    }
    if flags & 16 != 0 && r.remaining() > 0 {
        let params = r.u8()?;
        if params & 1 != 0 {
            m.density = Some(r.u8()?);
        }
        if params & 2 != 0 {
            m.feather = Some(r.f64()?);
        }
        if params & 4 != 0 {
            m.vector_density = Some(r.u8()?);
        }
        if params & 8 != 0 {
            m.vector_feather = Some(r.f64()?);
        }
    }
    Ok(m)
}

/// The samples of a channel decoded to 8 bits (`color`: a color channel,
/// which 32 bits store linear).
fn decode(data: &[u8], (w, h): (u32, u32), depth: u16, big: bool, color: bool) -> Result<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(data);
    let compression = r.u16()?;
    let row = (w * depth as usize).div_ceil(8);
    let raw = decompress(&mut r, compression, row, h, depth, big)?;
    Ok(to_8_bits(&raw, w, h, depth, color))
}

/// The samples of a channel, uncompressed (`row` bytes per row).
fn decompress(
    r: &mut Reader,
    compression: u16,
    row: usize,
    h: usize,
    depth: u16,
    big: bool,
) -> Result<Vec<u8>> {
    let size = row.checked_mul(h).ok_or(FileError::Damaged)?;
    match compression {
        0 => Ok(r.bytes(size)?.to_vec()),
        1 => {
            let counts = rle_counts(r, h, big)?;
            if !rle_fits(&counts, row) || counts.iter().sum::<usize>() > r.remaining() {
                return Err(FileError::Damaged);
            }
            let mut out = vec![0; size];
            for (y, n) in counts.into_iter().enumerate() {
                unpack_row(r.bytes(n)?, &mut out[y * row..(y + 1) * row])?;
            }
            Ok(out)
        }
        2 | 3 => {
            // Grows with what the data really holds, which deflate
            // shrinks at most about 1032 times.
            if r.remaining().saturating_mul(1100) + (1 << 16) < size {
                return Err(FileError::Damaged);
            }
            let mut out = Vec::new();
            flate2::read::ZlibDecoder::new(r.bytes(r.remaining())?)
                .take(size as u64)
                .read_to_end(&mut out)
                .map_err(|_| FileError::Damaged)?;
            if out.len() != size {
                return Err(FileError::Damaged);
            }
            if compression == 3 {
                unpredict(&mut out, row, depth);
            }
            Ok(out)
        }
        _ => Err(FileError::Damaged),
    }
}

/// The byte counts of `n` PackBits rows, refused when the data is too
/// short to hold them.
fn rle_counts(r: &mut Reader, n: usize, big: bool) -> Result<Vec<usize>> {
    if r.remaining() < n.saturating_mul(if big { 4 } else { 2 }) {
        return Err(FileError::Damaged);
    }
    (0..n)
        .map(|_| {
            Ok(if big {
                r.u32()? as usize
            } else {
                r.u16()? as usize
            })
        })
        .collect()
}

/// Whether PackBits rows of these sizes can unpack to `row` bytes each:
/// two bytes give at most 128 (checked before allocating the output).
fn rle_fits(counts: &[usize], row: usize) -> bool {
    counts.iter().all(|&n| n.saturating_mul(64) >= row)
}

/// Undoes the prediction of "ZIP with prediction": deltas along each row
/// (of 16-bit values for 16 bits; of bytes split in four planes for 32).
fn unpredict(data: &mut [u8], row: usize, depth: u16) {
    for line in data.chunks_exact_mut(row) {
        match depth {
            16 => {
                let mut prev = 0u16;
                for v in line.chunks_exact_mut(2) {
                    let x = u16::from_be_bytes([v[0], v[1]]).wrapping_add(prev);
                    v.copy_from_slice(&x.to_be_bytes());
                    prev = x;
                }
            }
            _ => {
                for i in 1..line.len() {
                    line[i] = line[i].wrapping_add(line[i - 1]);
                }
                if depth == 32 {
                    let n = line.len() / 4;
                    let planes = line.to_vec();
                    for i in 0..n {
                        for b in 0..4 {
                            line[i * 4 + b] = planes[b * n + i];
                        }
                    }
                }
            }
        }
    }
}

fn to_8_bits(raw: &[u8], w: usize, h: usize, depth: u16, color: bool) -> Vec<u8> {
    match depth {
        8 => raw.to_vec(),
        16 => raw
            .chunks_exact(2)
            .map(|c| ((u16::from_be_bytes([c[0], c[1]]) as u32 + 128) / 257) as u8)
            .collect(),
        32 => raw
            .chunks_exact(4)
            .map(|c| {
                let v = f32::from_be_bytes([c[0], c[1], c[2], c[3]]);
                let v = if v.is_finite() {
                    v.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let v = if color { srgb_of_linear(v) } else { v };
                (v * 255.0).round() as u8
            })
            .collect(),
        // Bitmap: 1 is black.
        _ => {
            let row = w.div_ceil(8);
            let mut out = Vec::with_capacity(w * h);
            for y in 0..h {
                for x in 0..w {
                    let bit = (raw[y * row + x / 8] >> (7 - x % 8)) & 1;
                    out.push(if bit == 1 { 0 } else { 255 });
                }
            }
            out
        }
    }
}

fn srgb_of_linear(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// A mode's color planes (and alpha, opaque if `None`) to RGBA; CMYK and
/// gray through the file's profile (see `color::to_srgb`).
fn to_rgba(
    header: &Header,
    (w, h): (u32, u32),
    color: &[&[u8]],
    alpha: Option<&[u8]>,
    palette: &[u8],
) -> RgbaImage {
    let mode = header.mode;
    // Sides are bounded by the readers (`check_rect`, `check_size`).
    let n = (w as usize).checked_mul(h as usize).expect("bounded size");
    let at = |plane: usize, i: usize| {
        color
            .get(plane)
            .map_or(0, |p| p.get(i).copied().unwrap_or(0))
    };
    let managed = match mode {
        // Stored inverted: 255 is no ink.
        CMYK => {
            let inks: Vec<u8> = (0..n)
                .flat_map(|i| (0..4).map(move |c| 255 - at(c, i)))
                .collect();
            crate::document::color::to_srgb(&inks, true, header.icc.as_deref())
        }
        GRAYSCALE => {
            let gray: Vec<u8> = (0..n).map(|i| at(0, i)).collect();
            crate::document::color::to_srgb(&gray, false, header.icc.as_deref())
        }
        _ => None,
    };
    let mut out = vec![0u8; n * 4];
    for (i, px) in out.chunks_exact_mut(4).enumerate() {
        let [r, g, b] = match mode {
            _ if let Some(rgb) = &managed => [rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]],
            RGB => [at(0, i), at(1, i), at(2, i)],
            CMYK => {
                // Stored inverted: 255 is no ink.
                let k = at(3, i) as u32;
                let f = |c: u8| ((c as u32 * k + 127) / 255) as u8;
                [f(at(0, i)), f(at(1, i)), f(at(2, i))]
            }
            LAB => {
                let l = at(0, i) as f32 * 100.0 / 255.0;
                let (a, b) = (at(1, i) as f32 - 128.0, at(2, i) as f32 - 128.0);
                crate::color::to_u8(crate::color::lab_to_rgb([l, a, b]))
            }
            INDEXED => {
                let k = at(0, i) as usize;
                let c = |j: usize| palette.get(j * 256 + k).copied().unwrap_or(0);
                [c(0), c(1), c(2)]
            }
            _ => [at(0, i); 3],
        };
        let a = alpha.map_or(255, |p| p.get(i).copied().unwrap_or(0));
        px.copy_from_slice(&[r, g, b, a]);
    }
    RgbaImage::from_raw(w, h, out).expect("matching size")
}

/// The decoded image and mask of a record.
struct Decoded {
    image: RgbaImage,
    /// Pixel mask: (image, origin) in the canvas.
    mask: Option<(GrayImage, (i64, i64))>,
}

fn decode_record(rec: &Record, h: &Header, palette: &[u8]) -> Result<Decoded> {
    let (w, ht) = size(rec.rect);
    let n = color_channels(h.mode);
    let plane = |id: i16, rect: [i32; 4], color: bool| -> Result<Option<Vec<u8>>> {
        rec.channels
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, data)| decode(data, size(rect), h.depth, h.big, color))
            .transpose()
    };
    let mut colors = Vec::with_capacity(n);
    for c in 0..n as i16 {
        colors.push(plane(c, rec.rect, true)?.unwrap_or_default());
    }
    let alpha = plane(-1, rec.rect, false)?;
    // A box without any channel data holds nothing (damaged files).
    let pixels = colors.iter().chain(&alpha).any(|p| !p.is_empty());
    let (w, ht) = if pixels { (w, ht) } else { (0, 0) };
    let refs: Vec<&[u8]> = colors.iter().map(Vec::as_slice).collect();
    let image = to_rgba(h, (w, ht), &refs, alpha.as_deref(), palette);
    let mask = match rec.user_mask() {
        Some(m) => {
            let (mw, mh) = size(m.rect);
            plane(m.channel, m.rect, false)?.map(|p| {
                let image = GrayImage::from_raw(mw, mh, p).filter(|_| mw * mh > 0);
                let origin = (m.rect[1] as i64, m.rect[0] as i64);
                (image.unwrap_or_else(|| GrayImage::new(0, 0)), origin)
            })
        }
        None => None,
    };
    Ok(Decoded { image, mask })
}

/// What had to be simplified, by kind, with the layers concerned.
#[derive(Default)]
struct Notes {
    lines: Vec<String>,
    text: Vec<String>,
    smart: Vec<String>,
    vector: Vec<String>,
    shapes: Vec<String>,
    dropped: Vec<String>,
    styles: Vec<String>,
}

impl Notes {
    fn finish(mut self) -> Vec<String> {
        // A layer is named once per kind.
        self.styles.dedup();
        let list = |names: &[String]| {
            const SHOWN: usize = 3;
            let mut s = names
                .iter()
                .take(SHOWN)
                .map(|n| t!("files3-quoted-name", name = n.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            if names.len() > SHOWN {
                s.push('…');
            }
            s
        };
        let groups = [
            (
                &self.text,
                t!("xchg-psd-note-text", names = list(&self.text)),
            ),
            (
                &self.smart,
                t!("xchg-psd-note-smart", names = list(&self.smart)),
            ),
            (
                &self.vector,
                t!("xchg-psd-note-vector", names = list(&self.vector)),
            ),
            (
                &self.shapes,
                t!("xchg-psd-note-shapes", names = list(&self.shapes)),
            ),
            (
                &self.styles,
                t!("xchg-psd-note-styles", names = list(&self.styles)),
            ),
            (
                &self.dropped,
                t!("xchg-psd-note-dropped", names = list(&self.dropped)),
            ),
        ];
        for (names, line) in groups {
            if !names.is_empty() {
                self.lines.push(line);
            }
        }
        self.lines
    }
}

/// Everything read from the file.
struct File<'a> {
    header: Header,
    resources: Resources,
    palette: Vec<u8>,
    records: Vec<Record<'a>>,
    /// The merged image's first alpha channel is its transparency.
    merged_alpha: bool,
    /// The merged image's data (compression first).
    image_data: &'a [u8],
}

fn parse(data: &[u8]) -> Result<File<'_>> {
    let mut r = Reader::new(data);
    let mut header = header(&mut r)?;
    let n = r.u32()? as usize;
    let palette = r.bytes(n)?.to_vec();
    let n = r.u32()? as usize;
    let resources = resources(&mut r.sub(n)?)?;
    header.icc = resources.icc.clone();
    let big = header.big;
    let n = r.len(big)?;
    let mut lm = r.sub(n)?;
    let image_data = r.bytes(r.remaining())?;
    let (mut records, mut merged_alpha) = (Vec::new(), false);
    let mut global = Vec::new();
    if lm.remaining() > 0 {
        let n = lm.len(big)?;
        let mut info = lm.sub(n)?;
        if n > 0 {
            (records, merged_alpha) = layer_info(&mut info, big)?;
        }
        if lm.remaining() >= 4 {
            let n = lm.u32()? as usize;
            lm.skip(n)?;
        }
        // Global tagged blocks, aligned to 4 bytes.
        while lm.remaining() >= 12 {
            let mut blocks = tagged_blocks(&mut lm, big)?;
            if blocks.is_empty() {
                lm.skip(1)?;
            }
            for (_, data) in &blocks {
                lm.skip((4 - data.len() % 4) % 4).ok();
            }
            global.append(&mut blocks);
        }
    }
    // 16 and 32 bits: the layers are in a global block.
    if records.is_empty()
        && let Some((_, block)) = global.iter().find(|(k, _)| k == b"Lr16" || k == b"Lr32")
    {
        (records, merged_alpha) = layer_info(&mut Reader::new(block), big)?;
    }
    merged_alpha |= global
        .iter()
        .any(|(k, _)| matches!(k, b"Mtrn" | b"Mt16" | b"Mt32"));
    Ok(File {
        header,
        resources,
        palette,
        records,
        merged_alpha,
        image_data,
    })
}

/// The merged image's channels `wanted` (by index), decoded.
fn merged_planes(f: &File, wanted: &[usize]) -> Result<Vec<Vec<u8>>> {
    let h = &f.header;
    check_size(h.width, h.height)?;
    let (w, ht) = (h.width as usize, h.height as usize);
    let row = (w * h.depth as usize).div_ceil(8);
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let mut r = Reader::new(f.image_data);
    let compression = r.u16()?;
    let channels = h.channels as usize;
    let planes: Vec<Vec<u8>> = match compression {
        1 => {
            let counts = rle_counts(&mut r, channels * ht, h.big)?;
            if !rle_fits(&counts, row) {
                return Err(FileError::Damaged);
            }
            let mut planes = Vec::new();
            for c in 0..channels {
                let rows = &counts[c * ht..(c + 1) * ht];
                let total: usize = rows.iter().sum();
                let data = r.bytes(total)?;
                if !wanted.contains(&c) {
                    planes.push(Vec::new());
                    continue;
                }
                let mut raw = vec![0; row * ht];
                let mut at = 0;
                for (y, &n) in rows.iter().enumerate() {
                    unpack_row(&data[at..at + n], &mut raw[y * row..(y + 1) * row])?;
                    at += n;
                }
                planes.push(raw);
            }
            planes
        }
        _ => {
            let all = decompress(&mut r, compression, row, ht * channels, h.depth, h.big)?;
            all.chunks(row * ht).map(<[u8]>::to_vec).collect()
        }
    };
    let color = color_channels(h.mode);
    Ok(wanted
        .iter()
        .map(|&c| {
            planes
                .get(c)
                .map_or_else(Vec::new, |p| to_8_bits(p, w, ht, h.depth, c < color))
        })
        .collect())
}

/// Reads a Photoshop document from its bytes.
pub(super) fn document(data: &[u8]) -> Result<Imported> {
    document_within(data, Budget::new())
}

/// [`document`], its pixels taken from `budget` (all the layers decode at
/// once).
pub(super) fn document_within(data: &[u8], mut budget: Budget) -> Result<Imported> {
    let f = parse(data)?;
    let h = &f.header;
    check_size(h.width, h.height)?;
    for rec in &f.records {
        let (w, ht) = size(rec.rect);
        budget.take(w, ht, 4)?;
        if let Some(m) = &rec.mask {
            let (w, ht) = size(m.rect);
            budget.take(w, ht, 1)?;
            if let Some((rect, ..)) = m.real {
                let (w, ht) = size(rect);
                budget.take(w, ht, 1)?;
            }
        }
        // Rasterized over the canvas.
        if rec.vector_mask().is_some() {
            budget.take(h.width, h.height, 1)?;
        }
    }
    // The merged image's channels (the alpha channels, or the flat image).
    budget.take(h.width, h.height, u64::from(h.channels).max(4))?;
    if h.mode == MULTICHANNEL && h.channels < 3 {
        return Err(FileError::NotPsd);
    }
    let mut notes = Notes::default();
    if h.depth != 8 {
        notes.lines.push(t!("xchg-psd-note-depth", bits = h.depth));
    }
    let mode_name = match h.mode {
        BITMAP => Some(t!("xchg-psd-mode-bitmap")),
        GRAYSCALE => Some(t!("xchg-psd-mode-grayscale")),
        INDEXED => Some(t!("xchg-psd-mode-indexed")),
        CMYK => Some(t!("xchg-psd-mode-cmyk")),
        MULTICHANNEL => Some(t!("xchg-psd-mode-multichannel")),
        DUOTONE => Some(t!("xchg-psd-mode-duotone")),
        LAB => Some(t!("xchg-psd-mode-lab")),
        _ => None,
    };
    if let Some(mode) = mode_name {
        notes.lines.push(t!("xchg-psd-note-mode", mode = mode));
    }
    let mut doc = Document::new(RgbaImage::new(0, 0), None);
    doc.layers.clear();
    doc.canvas = (h.width, h.height);
    let canvas = (h.width, h.height);

    // Layers, decoded on all the cores.
    let (decoded, ()) = par_map(&f.records, |rec| decode_record(rec, h, &f.palette), || ());
    let mut target = None;
    let mut depth = 0u32;
    for (i, (rec, decoded)) in f.records.iter().zip(decoded).enumerate() {
        let divider = rec.block(b"lsct").or_else(|| rec.block(b"lsdk"));
        let section = divider.map_or(Ok(0), |d| Reader::new(d).u32())?;
        if section == 3 {
            depth += 1;
            continue;
        }
        let Decoded { image, mask } = decoded?;
        let mut layer = doc.make_layer(
            rec.name.clone(),
            image,
            (rec.rect[1] as i64, rec.rect[0] as i64),
        );
        layer.blend = blend_of(&rec.blend);
        if section == 1 || section == 2 {
            depth = depth.saturating_sub(1);
            layer.kind = LayerKind::Group {
                expanded: section == 1,
            };
            layer.replace_pixels(RgbaImage::new(0, 0));
            layer.offset = (0, 0);
            if let Some(d) = divider.filter(|d| d.len() >= 12) {
                layer.blend = blend_of(&d[8..12]);
            }
            let board = [b"artb", b"artd", b"abdd"]
                .iter()
                .find_map(|k| rec.block(k))
                .filter(|_| depth == 0)
                .and_then(artboard::read);
            if let Some((r, a)) = board {
                layer.offset = (r.x, r.y);
                layer.artboard = Some(a);
            }
        } else {
            match adjust::read(&rec.blocks) {
                Some(Ok(kind)) => {
                    layer.kind = kind;
                    layer.replace_pixels(RgbaImage::new(0, 0));
                    layer.offset = (0, 0);
                }
                Some(Err(())) => {
                    notes.dropped.push(rec.name.clone());
                    continue;
                }
                None => {}
            }
        }
        if layer.blend == BlendMode::PassThrough && !layer.is_group() {
            layer.blend = BlendMode::Normal;
        }
        layer.depth = depth;
        layer.visible = !rec.hidden;
        layer.opacity = rec.opacity as f32 / 255.0;
        if let Some(mut d) = rec.block(b"iOpa").map(Reader::new) {
            layer.fill = d.u8()? as f32 / 255.0;
        }
        layer.clipped = rec.clipped && !layer.is_group();
        // A group's fill opacity acts on what it holds, as its opacity
        // does (without knockout): Atelier's groups only have the latter.
        if layer.is_group() {
            layer.opacity *= layer.fill;
            layer.fill = 1.0;
        }
        if let Some(mut d) = rec.block(b"lspf").map(Reader::new) {
            let lock = d.u32()?;
            layer.locked = lock & 0x8000_0000 != 0 || lock & 6 == 6;
        }
        // Shape strokes; knockout and Blend If (Blending Options).
        if rec.block(b"vstk").is_some() {
            notes.shapes.push(rec.name.clone());
        }
        let knockout = rec
            .block(b"knko")
            .is_some_and(|d| d.first().is_some_and(|&k| k != 0));
        if knockout || rec.blend_if {
            notes.styles.push(rec.name.clone());
        }
        if layer.has_pixels() {
            if rec.block(b"TySh").is_some() {
                notes.text.push(rec.name.clone());
            }
            if [b"SoLd", b"PlLd", b"SoLE"]
                .iter()
                .any(|k| rec.block(k).is_some())
            {
                notes.smart.push(rec.name.clone());
            }
        }
        // Layer styles: those Atelier has, on the pixel layers.
        let fx = rec
            .block(b"lmfx")
            .or_else(|| rec.block(b"lfx2"))
            .or_else(|| rec.block(b"lfxs"))
            .and_then(|d| {
                let mut r = Reader::new(d);
                r.skip(8).ok()?;
                Descriptor::read(&mut r).ok()
            });
        if let Some(fx) = fx {
            let angle = f.resources.global_angle.unwrap_or(120.0);
            let (effects, simplified) = effects::read(&fx, angle);
            // Atelier has styles on pixel layers only.
            if simplified || (!layer.has_pixels() && effects.any()) {
                notes.styles.push(rec.name.clone());
            }
            if layer.has_pixels() {
                layer.effects = effects;
            }
        }
        // Masks: the pixel mask and the vector mask rasterized, multiplied.
        let pixel = mask
            .zip(rec.user_mask())
            .zip(rec.mask)
            .map(|(((image, origin), u), md)| {
                let mut m = Mask::new(image, origin, u.default, layer.offset);
                m.enabled = !u.disabled;
                m.density = md.density.map_or(1.0, |d| d as f32 / 255.0);
                m.feather = md.feather.map_or(0.0, |f| f.clamp(0.0, 1000.0) as f32);
                m
            });
        let vector = match rec.vector_mask() {
            Some(v) => vector::rasterize(v, canvas)?,
            None => None,
        };
        layer.mask = match (pixel, vector) {
            (pixel, None) => pixel,
            (pixel, Some(coverage)) => {
                if layer.has_pixels() || pixel.is_some() {
                    notes.vector.push(rec.name.clone());
                }
                let md = rec.mask;
                let mut v = Mask::new(coverage, (0, 0), 0, layer.offset);
                v.density = md
                    .and_then(|m| m.vector_density)
                    .map_or(1.0, |d| d as f32 / 255.0);
                v.feather = md
                    .and_then(|m| m.vector_feather)
                    .map_or(0.0, |f| f.clamp(0.0, 1000.0) as f32);
                Some(combine(
                    &v,
                    pixel.as_ref().filter(|m| m.enabled),
                    canvas,
                    layer.offset,
                ))
            }
        };
        if f.resources.target_layer == Some(i) {
            target = Some(doc.layers.len());
        }
        doc.layers.push(layer);
    }
    doc.fix_depths();
    // A clipped layer needs one below it, in its group.
    for i in 0..doc.layers.len() {
        let below = i.checked_sub(1).map(|j| doc.layers[j].depth);
        if below.is_none_or(|d| d < doc.layers[i].depth) {
            doc.layers[i].clipped = false;
        }
    }

    // Alpha channels: after the color channels (and the merged
    // transparency).
    let color = color_channels(h.mode);
    let first_alpha =
        color + usize::from(f.merged_alpha || f.records.is_empty() && has_transparency(&f));
    let flat = doc.layers.is_empty();
    let mut wanted: Vec<usize> = (first_alpha..h.channels as usize).collect();
    if flat {
        wanted.extend(0..first_alpha.min(h.channels as usize));
    }
    // A damaged merged image only costs the alpha channels when there are
    // layers.
    let planes = match merged_planes(&f, &wanted) {
        Ok(planes) => planes,
        Err(_) if !flat => Vec::new(),
        Err(e) => return Err(e),
    };
    let alpha_count = h.channels as usize - first_alpha.min(h.channels as usize);
    for (k, plane) in planes.iter().take(alpha_count).enumerate() {
        let name = f
            .resources
            .alpha_names
            .get(k)
            .cloned()
            .unwrap_or_else(|| t!("xchg-psd-alpha-name", number = k + 1));
        if let Some(mask) = GrayImage::from_raw(h.width, h.height, plane.clone()) {
            doc.channels.push(Channel {
                name,
                mask: Arc::new(mask),
            });
        }
    }
    if flat {
        let p = &planes[alpha_count..];
        let refs: Vec<&[u8]> = p[..color.min(p.len())].iter().map(Vec::as_slice).collect();
        let alpha = (first_alpha > color)
            .then(|| p.get(color).map(Vec::as_slice))
            .flatten();
        let mut image = to_rgba(h, canvas, &refs, alpha, &f.palette);
        if alpha.is_some() {
            unmatte(&mut image);
        }
        let layer = doc.make_layer(t!("doc-background-layer-name"), image, (0, 0));
        doc.layers.push(layer);
    }
    doc.active = target.unwrap_or(doc.layers.len() - 1);
    doc.guides = f.resources.guides;
    let path = |data: &[u8]| super::vector::path_of(data, canvas).ok();
    doc.paths = (f.resources.paths.iter())
        .filter_map(|(name, data)| {
            let path = path(data)?;
            Some(crate::document::SavedPath {
                name: name.clone(),
                path,
            })
        })
        .collect();
    doc.work_path = f.resources.work_path.as_deref().and_then(path);
    // With artboards, the canvas is their box (Photoshop's also reaches
    // the origin).
    doc.fit_artboards();
    // CMYK and Lab pixels are read in sRGB.
    doc.icc_profile = match h.mode {
        CMYK | LAB => Some(crate::cms::srgb().icc().to_vec()),
        mode => crate::document::color::rgb_profile(f.resources.icc, matches!(mode, RGB | INDEXED)),
    }
    .map(Arc::new);
    doc.file_metadata.psd_resources = Arc::new(f.resources.kept.clone());
    if f.resources.comps {
        notes.lines.push(t!("files4-psd-note-comps"));
    }
    let lost = [
        (f.resources.animation, t!("files5-psd-note-animation")),
        (f.resources.spot, t!("files5-psd-note-spot")),
        (f.resources.variables, t!("files5-psd-note-variables")),
        (f.resources.other, t!("files5-psd-note-other")),
    ];
    notes.lines.extend(
        lost.into_iter()
            .filter(|(lost, _)| *lost)
            .map(|(_, note)| note),
    );
    doc.mark_saved();
    Ok(Imported {
        doc,
        notes: notes.finish(),
    })
}

/// A flat file's extra channel is its transparency unless the alpha
/// channels are named as such.
fn has_transparency(f: &File) -> bool {
    let color = color_channels(f.header.mode);
    f.header.channels as usize > color
        && f.resources.alpha_names.len() < f.header.channels as usize - color
}

/// The merged image with transparency is matted against white: its colors
/// get theirs back.
fn unmatte(image: &mut RgbaImage) {
    for p in image.pixels_mut() {
        let a = p.0[3] as u32;
        if a == 0 {
            p.0 = [0; 4];
        } else if a < 255 {
            for c in &mut p.0[..3] {
                let v = (*c as i32 - (255 - a) as i32).max(0) as u32;
                *c = ((v * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// One mask over the canvas: the vector mask `v` times the pixel mask, each
/// with its density and feather.
fn combine(v: &Mask, pixel: Option<&Mask>, (w, h): (u32, u32), offset: (i64, i64)) -> Mask {
    let image = GrayImage::from_fn(w, h, |x, y| {
        let (x, y) = (x as i64, y as i64);
        let p = pixel.map_or(1.0, |m| m.value_at(x, y, offset));
        image::Luma([(v.value_at(x, y, offset) * p * 255.0).round() as u8])
    });
    Mask::new(image, (0, 0), 0, offset)
}

/// Canvas size of a Photoshop document, from its header.
pub fn canvas_size(path: &Path) -> Option<(u32, u32)> {
    let mut head = [0u8; 26];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    let h = header(&mut Reader::new(&head)).ok()?;
    Some((h.width, h.height))
}

/// Preview of a Photoshop document: its thumbnail resource, else its
/// merged image.
pub fn thumbnail_of(path: &Path) -> Option<RgbaImage> {
    let data = std::fs::read(path).ok()?;
    let f = parse(&data).ok()?;
    if let Some(jpeg) = &f.resources.thumbnail
        && let Ok(image) = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
    {
        return Some(image.into_rgba8());
    }
    let color = color_channels(f.header.mode);
    let wanted: Vec<usize> = (0..color).collect();
    let planes = merged_planes(&f, &wanted).ok()?;
    let refs: Vec<&[u8]> = planes.iter().map(Vec::as_slice).collect();
    let size = (f.header.width, f.header.height);
    Some(to_rgba(&f.header, size, &refs, None, &f.palette))
}

/// The merged image of a file (tests).
#[cfg(test)]
pub(super) fn merged_image(data: &[u8]) -> Option<RgbaImage> {
    let f = parse(data).ok()?;
    let color = color_channels(f.header.mode);
    let alpha = f.merged_alpha || f.records.is_empty() && has_transparency(&f);
    let mut wanted: Vec<usize> = (0..color).collect();
    if alpha {
        wanted.push(color);
    }
    let planes = merged_planes(&f, &wanted).ok()?;
    let refs: Vec<&[u8]> = planes[..color].iter().map(Vec::as_slice).collect();
    let size = (f.header.width, f.header.height);
    let transparency = alpha.then(|| planes[color].as_slice());
    let mut image = to_rgba(&f.header, size, &refs, transparency, &f.palette);
    if alpha {
        unmatte(&mut image);
    }
    Some(image)
}
