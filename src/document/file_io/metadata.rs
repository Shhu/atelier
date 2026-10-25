//! What an image file holds beside its pixels and profile, given back when
//! it is saved over in its format (Save and Close, a linked file's Save):
//! its resolution, XMP, IPTC, comments and text, as they were; its EXIF
//! made true again (size, no stale thumbnail).

use std::path::Path;

use image::ImageFormat;

use super::ExportFormat;

/// The metadata of an image file, kept to be written back in its format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageMetadata {
    /// The format they were read from, the only one they are written to.
    pub format: Option<ExportFormat>,
    /// Resolution, in pixels per inch (horizontal, vertical).
    pub density: Option<(f64, f64)>,
    /// JPEG: whole marker segments (XMP, extended XMP, IPTC, comments).
    /// PNG: whole chunks (text, international text, XMP).
    pub raw: Vec<Vec<u8>>,
    /// TIFF: the first directory's tags kept (XMP, IPTC, image resources,
    /// copyright, artist, description, camera…), byte for byte.
    pub tiff: Vec<TiffTag>,
    /// TIFF: the EXIF and GPS directories, written back as directories of
    /// their own.
    pub tiff_dirs: Vec<TiffDir>,
}

/// A TIFF tag kept: its number, type and value (in this computer's byte
/// order, as the encoder writes).
#[derive(Clone, Debug, PartialEq)]
pub struct TiffTag {
    pub tag: u16,
    pub kind: u16,
    pub data: Vec<u8>,
}

/// A TIFF directory a tag points to (EXIF, GPS, interoperability).
#[derive(Clone, Debug, PartialEq)]
pub struct TiffDir {
    /// The tag pointing to it.
    pub pointer: u16,
    pub tags: Vec<TiffTag>,
    pub dirs: Vec<TiffDir>,
}

/// What reading found that can't be written back.
#[derive(Default, Debug, PartialEq)]
pub struct Unkept {
    /// A JPEG's other images (Multi-Picture: HDR gain map, previews).
    pub images: bool,
    /// XMP or IPTC the format's writer has no room for (WebP, GIF), or
    /// stored in a way not understood.
    pub xmp: bool,
    /// A TIFF's EXIF directory.
    pub exif: bool,
    /// A TIFF's Photoshop data (image resources, layers).
    pub photoshop: bool,
}

/// TIFF tags kept from the first directory, whatever their type, as
/// bytes: XMP, IPTC (often LONG).
const TIFF_BYTES: [u16; 2] = [700, 33723];
/// …as text, byte for byte whatever its encoding (Latin-1 "©" from older
/// software, several NUL-separated parts): copyright, artist,
/// description, document name, make, model, date, host computer.
const TIFF_TEXT: [u16; 8] = [33432, 315, 270, 269, 271, 272, 306, 316];
/// Image resources (Photoshop's), layers some applications store, the
/// EXIF and GPS directories, and the interoperability one inside EXIF.
const TIFF_RESOURCES: u16 = 34377;
const TIFF_LAYERS: u16 = 37724;
const TIFF_EXIF: u16 = 34665;
const TIFF_GPS: u16 = 34853;
const TIFF_INTEROP: u16 = 40965;

/// Reads what `path` (in `format`) holds beside its pixels.
pub fn read(path: &Path, format: Option<ImageFormat>) -> (ImageMetadata, Unkept) {
    let mut unkept = Unkept::default();
    let meta = match format {
        Some(ImageFormat::Jpeg) => std::fs::read(path).ok().map(|d| jpeg(&d, &mut unkept)),
        Some(ImageFormat::Png) => std::fs::read(path).ok().map(|d| png(&d)),
        Some(ImageFormat::Tiff) => tiff(path, &mut unkept),
        _ => None,
    };
    (meta.unwrap_or_default(), unkept)
}

/// The marker segments of a JPEG before its image data: (marker, the
/// whole segment, its data).
fn segments(data: &[u8]) -> Vec<(u8, &[u8], &[u8])> {
    let mut out = Vec::new();
    if !data.starts_with(&[0xFF, 0xD8]) {
        return out;
    }
    let mut at = 2;
    while at + 4 <= data.len() && data[at] == 0xFF {
        let marker = data[at + 1];
        // Fill bytes, then the markers without a length.
        if marker == 0xFF {
            at += 1;
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            at += 2;
            continue;
        }
        // Start of scan, end of image: the metadata are before.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([data[at + 2], data[at + 3]]) as usize;
        let Some(segment) = data.get(at..at + 2 + len).filter(|_| len >= 2) else {
            break;
        };
        out.push((marker, segment, &segment[4..]));
        at += 2 + len;
    }
    out
}

const XMP: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const XMP_EXTENDED: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
const IPTC: &[u8] = b"Photoshop 3.0\0";

fn jpeg(data: &[u8], unkept: &mut Unkept) -> ImageMetadata {
    let mut meta = ImageMetadata {
        format: Some(ExportFormat::Jpeg),
        ..ImageMetadata::default()
    };
    for (marker, segment, body) in segments(data) {
        match marker {
            0xE0 if body.starts_with(b"JFIF\0") && body.len() >= 12 => {
                let x = f64::from(u16::from_be_bytes([body[8], body[9]]));
                let y = f64::from(u16::from_be_bytes([body[10], body[11]]));
                meta.density = match body[7] {
                    1 => Some((x, y)),
                    2 => Some((x * 2.54, y * 2.54)),
                    _ => None,
                }
                .filter(|(x, y)| *x > 0.0 && *y > 0.0);
            }
            0xE1 if body.starts_with(XMP) || body.starts_with(XMP_EXTENDED) => {
                meta.raw.push(segment.to_vec());
            }
            0xED if body.starts_with(IPTC) => {
                meta.raw
                    .push(without_thumbnail(segment).unwrap_or_else(|| segment.to_vec()));
            }
            0xFE => meta.raw.push(segment.to_vec()),
            0xE2 if body.starts_with(b"MPF\0") => unkept.images = true,
            _ => {}
        }
    }
    meta
}

/// An APP13 segment (Photoshop's image resources: IPTC, resolution…)
/// without its thumbnail (resources 1033 and 1036), which would show the
/// image as it was; `None` if it can't be read.
fn without_thumbnail(segment: &[u8]) -> Option<Vec<u8>> {
    let kept = without_thumbnails(segment.get(4 + IPTC.len()..)?)?;
    let body = [IPTC, &kept].concat();
    let len = u16::try_from(body.len() + 2).ok()?;
    Some([&[0xFF, 0xED][..], &len.to_be_bytes(), &body].concat())
}

/// Photoshop image resources (one "8BIM" block after another) without
/// their thumbnail (1033 and 1036); `None` if they can't be read.
fn without_thumbnails(data: &[u8]) -> Option<Vec<u8>> {
    let mut kept = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let start = at;
        if data.get(at..at + 4)? != b"8BIM" {
            return None;
        }
        let id = u16::from_be_bytes(data.get(at + 4..at + 6)?.try_into().ok()?);
        // The name: a length byte, then it, padded to an even size.
        let name = (1 + *data.get(at + 6)? as usize).next_multiple_of(2);
        at += 6 + name;
        let len = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
        at = at.checked_add(4 + len.next_multiple_of(2))?;
        let resource = data.get(start..at)?;
        if !matches!(id, 1033 | 1036) {
            kept.extend(resource);
        }
    }
    Some(kept)
}

/// The chunks of a PNG: (type, the whole chunk, its data).
fn chunks(data: &[u8]) -> Vec<([u8; 4], &[u8], &[u8])> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let mut out = Vec::new();
    if !data.starts_with(SIGNATURE) {
        return out;
    }
    let mut at = SIGNATURE.len();
    while at + 12 <= data.len() {
        let len = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        let Some(chunk) = len
            .checked_add(12)
            .and_then(|n| data.get(at..at.checked_add(n)?))
        else {
            break;
        };
        out.push((kind, chunk, &chunk[8..8 + len]));
        if &kind == b"IEND" {
            break;
        }
        at += chunk.len();
    }
    out
}

fn png(data: &[u8]) -> ImageMetadata {
    let mut meta = ImageMetadata {
        format: Some(ExportFormat::Png),
        ..ImageMetadata::default()
    };
    for (kind, chunk, body) in chunks(data) {
        match &kind {
            // Pixels per meter, when the unit is the meter.
            b"pHYs" if body.len() == 9 && body[8] == 1 => {
                let per_meter = |b: &[u8]| f64::from(u32::from_be_bytes(b.try_into().unwrap()));
                let (x, y) = (per_meter(&body[0..4]), per_meter(&body[4..8]));
                meta.density = (x > 0.0 && y > 0.0).then_some((x * 0.0254, y * 0.0254));
            }
            b"tEXt" | b"zTXt" | b"iTXt" => meta.raw.push(chunk.to_vec()),
            _ => {}
        }
    }
    meta
}

fn tiff(path: &Path, unkept: &mut Unkept) -> Option<ImageMetadata> {
    use tiff::decoder::ifd::Value;
    use tiff::tags::Tag;
    let file = std::fs::File::open(path).ok()?;
    let mut d = tiff::decoder::Decoder::new(std::io::BufReader::new(file)).ok()?;
    let mut meta = ImageMetadata {
        format: Some(ExportFormat::Tiff),
        ..ImageMetadata::default()
    };
    let number = |v: Value| match v {
        Value::Rational(n, d) if d > 0 => Some(f64::from(n) / f64::from(d)),
        Value::Short(v) => Some(f64::from(v)),
        Value::Unsigned(v) => Some(f64::from(v)),
        _ => None,
    };
    let x = d.find_tag(Tag::XResolution).ok().flatten().and_then(number);
    let y = d.find_tag(Tag::YResolution).ok().flatten().and_then(number);
    let unit = d
        .find_tag_unsigned::<u16>(Tag::ResolutionUnit)
        .ok()
        .flatten();
    let k = match unit {
        None | Some(2) => Some(1.0),
        Some(3) => Some(2.54),
        _ => None,
    };
    if let (Some(x), Some(y), Some(k)) = (x, y, k)
        && x > 0.0
        && y > 0.0
    {
        meta.density = Some((x * k, y * k));
    }
    drop(d);
    // The tags themselves, read raw: the decoder refuses text that isn't
    // UTF-8 and cuts it at the first NUL.
    let Some(mut file) = RawTiff::open(path) else {
        unkept.xmp = true;
        return Some(meta);
    };
    let Some(ifd0) = file.first_ifd() else {
        unkept.xmp = true;
        return Some(meta);
    };
    let has = |tag: u16| ifd0.iter().any(|e| e.tag == tag);
    for e in &ifd0 {
        let kind = match e.tag {
            t if TIFF_BYTES.contains(&t) => 1,
            t if TIFF_TEXT.contains(&t) => 2,
            // Without the layers they describe, safe to give back (but
            // the thumbnail, which would show the image as it was).
            TIFF_RESOURCES if !has(TIFF_LAYERS) => 1,
            TIFF_EXIF | TIFF_GPS => {
                match file.dir(e, 0) {
                    Some(dir) => meta.tiff_dirs.push(dir),
                    None => unkept.exif = true,
                }
                continue;
            }
            _ => continue,
        };
        let data = match file.data(e) {
            Some(data) if e.tag == TIFF_RESOURCES => without_thumbnails(&data),
            data => data,
        };
        match data {
            Some(mut data) => {
                if kind == 2 && data.last() != Some(&0) {
                    data.push(0);
                }
                meta.tiff.push(TiffTag {
                    tag: e.tag,
                    kind,
                    data,
                });
            }
            None if e.tag == TIFF_RESOURCES => unkept.photoshop = true,
            None => unkept.xmp = true,
        }
    }
    unkept.photoshop |= has(TIFF_LAYERS);
    Some(meta)
}

/// An entry of a TIFF directory: its tag, type, count and value field
/// (the value, or where it is).
struct RawEntry {
    tag: u16,
    kind: u16,
    count: u64,
    field: [u8; 8],
}

/// A TIFF file read by its directories.
struct RawTiff {
    file: std::io::BufReader<std::fs::File>,
    len: u64,
    little: bool,
    /// BigTIFF: 8-byte counts and offsets.
    big: bool,
}

/// Bytes of an element of TIFF type `kind`, and of the parts its byte
/// order applies to; `None` for an unknown type.
fn type_size(kind: u16) -> Option<(u64, usize)> {
    Some(match kind {
        1 | 2 | 6 | 7 => (1, 1),
        3 | 8 => (2, 2),
        4 | 9 | 11 | 13 => (4, 4),
        5 | 10 => (8, 4),
        12 | 16..=18 => (8, 8),
        _ => return None,
    })
}

/// A tag's value at most (larger is refused).
const MAX_TIFF_VALUE: u64 = 64 << 20;

impl RawTiff {
    fn open(path: &Path) -> Option<Self> {
        use std::io::Read;
        let file = std::fs::File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        let mut file = std::io::BufReader::new(file);
        let mut head = [0u8; 4];
        file.read_exact(&mut head).ok()?;
        let little = match &head[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let version = if little {
            u16::from_le_bytes([head[2], head[3]])
        } else {
            u16::from_be_bytes([head[2], head[3]])
        };
        let big = match version {
            42 => false,
            43 => true,
            _ => return None,
        };
        Some(Self {
            file,
            len,
            little,
            big,
        })
    }

    fn read(&mut self, at: u64, n: u64) -> Option<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        if n > MAX_TIFF_VALUE || at.checked_add(n)? > self.len {
            return None;
        }
        self.file.seek(SeekFrom::Start(at)).ok()?;
        let mut out = vec![0; n as usize];
        self.file.read_exact(&mut out).ok()?;
        Some(out)
    }

    fn uint(&self, b: &[u8]) -> u64 {
        let mut v = 0u64;
        for i in 0..b.len() {
            let byte = if self.little {
                b[b.len() - 1 - i]
            } else {
                b[i]
            };
            v = v << 8 | u64::from(byte);
        }
        v
    }

    fn first_ifd(&mut self) -> Option<Vec<RawEntry>> {
        let at = match self.big {
            false => self.read(4, 4)?,
            true => self.read(8, 8)?,
        };
        let at = self.uint(&at);
        self.ifd(at)
    }

    /// The entries of the directory at `at`.
    fn ifd(&mut self, at: u64) -> Option<Vec<RawEntry>> {
        let (count_size, entry_size, field) = if self.big { (8, 20, 8) } else { (2, 12, 4) };
        let count = self.read(at, count_size)?;
        let count = self.uint(&count);
        if count > 4096 {
            return None;
        }
        let data = self.read(at + count_size, count * entry_size)?;
        Some(
            data.chunks_exact(entry_size as usize)
                .map(|e| {
                    let mut value = [0; 8];
                    value[..field].copy_from_slice(&e[e.len() - field..]);
                    RawEntry {
                        tag: self.uint(&e[0..2]) as u16,
                        kind: self.uint(&e[2..4]) as u16,
                        count: self.uint(&e[4..e.len() - field]),
                        field: value,
                    }
                })
                .collect(),
        )
    }

    /// An entry's value, in the file's byte order.
    fn data(&mut self, e: &RawEntry) -> Option<Vec<u8>> {
        let size = type_size(e.kind)?.0.checked_mul(e.count)?;
        let field = if self.big { 8 } else { 4 };
        if size <= field {
            return Some(e.field[..size as usize].to_vec());
        }
        let at = self.uint(&e.field[..field as usize]);
        self.read(at, size)
    }

    /// The directory entry `e` points to, its values in this computer's
    /// byte order; `None` if it can't be kept whole (`depth`: how deep
    /// in directories it is).
    fn dir(&mut self, e: &RawEntry, depth: u32) -> Option<TiffDir> {
        let at = self.data(e)?;
        if depth > 1 || !matches!(e.kind, 4 | 13 | 16 | 18) || e.count != 1 {
            return None;
        }
        let at = self.uint(&at);
        let mut dir = TiffDir {
            pointer: e.tag,
            tags: Vec::new(),
            dirs: Vec::new(),
        };
        for e in self.ifd(at)? {
            if e.tag == TIFF_INTEROP {
                dir.dirs.push(self.dir(&e, depth + 1)?);
                continue;
            }
            // Other directories, or BigTIFF values a TIFF can't hold.
            if matches!(e.kind, 13 | 16..=18) {
                return None;
            }
            let (_, unit) = type_size(e.kind)?;
            let mut data = self.data(&e)?;
            if self.little != cfg!(target_endian = "little") {
                for part in data.chunks_exact_mut(unit) {
                    part.reverse();
                }
            }
            dir.tags.push(TiffTag {
                tag: e.tag,
                kind: e.kind,
                data,
            });
        }
        Some(dir)
    }
}

/// A resolution in whole pixels per inch.
fn density_u16((x, y): (f64, f64)) -> (u16, u16) {
    let side = |v: f64| v.round().clamp(1.0, f64::from(u16::MAX)) as u16;
    (side(x), side(y))
}

impl ImageMetadata {
    /// These metadata for the image of `size` being saved, upright: their
    /// XMP's orientation, pixel dimensions and thumbnail made true again
    /// (as [`fresh_exif`] does the EXIF's), or the XMP left as it was where
    /// it can't be edited.
    pub fn for_size(&self, size: (u32, u32)) -> ImageMetadata {
        let mut out = self.clone();
        for part in &mut out.raw {
            if let Some(fresh) = fresh_xmp_part(part, size) {
                *part = fresh;
            }
        }
        for t in &mut out.tiff {
            if t.tag == 700 {
                t.data = fresh_xmp(&t.data, size);
            }
        }
        out
    }

    /// A JPEG's resolution for its encoder.
    pub fn jpeg_density(&self) -> Option<image::codecs::jpeg::PixelDensity> {
        let (x, y) = density_u16(self.density?);
        Some(image::codecs::jpeg::PixelDensity {
            density: (x, y),
            unit: image::codecs::jpeg::PixelDensityUnit::Inches,
        })
    }

    /// The file's bytes as its encoder wrote them, with these metadata
    /// put back (JPEG and PNG; the others are written by their encoder).
    pub fn put_back(&self, format: ExportFormat, data: Vec<u8>) -> Vec<u8> {
        if self.format != Some(format) {
            return data;
        }
        match format {
            ExportFormat::Jpeg if !self.raw.is_empty() => {
                // After the encoder's own application segments (JFIF,
                // EXIF, profile), before the tables.
                let end = segments(&data)
                    .into_iter()
                    .take_while(|(m, ..)| (0xE0..=0xEF).contains(m))
                    .map(|(_, s, _)| s.len())
                    .sum::<usize>()
                    + 2;
                splice(data, end, &self.raw)
            }
            ExportFormat::Png => {
                let mut extra = Vec::new();
                if let Some((x, y)) = self.density {
                    let per_meter = |v: f64| (v / 0.0254).round() as u32;
                    let mut body = Vec::with_capacity(9);
                    body.extend(per_meter(x).to_be_bytes());
                    body.extend(per_meter(y).to_be_bytes());
                    body.push(1);
                    extra.push(png_chunk(b"pHYs", &body));
                }
                let written: Vec<[u8; 4]> = chunks(&data).iter().map(|(k, ..)| *k).collect();
                if written.contains(b"pHYs") {
                    extra.clear();
                }
                extra.extend(self.raw.iter().cloned());
                if extra.is_empty() {
                    return data;
                }
                // Right after the header (before the pixels, as pHYs must).
                let end = 8 + chunks(&data).first().map_or(0, |(_, c, _)| c.len());
                splice(data, end, &extra)
            }
            _ => data,
        }
    }

    /// A TIFF's kept tags and resolution, written by `encoder` (its
    /// copyright unless a `new_copyright` is written), with `dirs` the
    /// directories [`Self::write_tiff_dirs`] wrote.
    pub fn write_tiff<W: std::io::Write + std::io::Seek, K: tiff::encoder::TiffKind>(
        &self,
        encoder: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
        new_copyright: bool,
        dirs: &[(u16, u32)],
    ) -> tiff::TiffResult<()> {
        use tiff::encoder::Rational;
        use tiff::tags::Tag;
        if self.format != Some(ExportFormat::Tiff) {
            return Ok(());
        }
        if let Some((x, y)) = self.density {
            let rational = |v: f64| Rational {
                n: (v * 100.0).round().clamp(1.0, f64::from(u32::MAX)) as u32,
                d: 100,
            };
            encoder.write_tag(Tag::ResolutionUnit, 2u16)?;
            encoder.write_tag(Tag::XResolution, rational(x))?;
            encoder.write_tag(Tag::YResolution, rational(y))?;
        }
        for t in &self.tiff {
            if new_copyright && t.tag == 33432 {
                continue;
            }
            write_raw(encoder, t.tag, t.kind, &t.data)?;
        }
        for &(tag, at) in dirs {
            encoder.write_tag(Tag::from_u16_exhaustive(tag), at)?;
        }
        Ok(())
    }

    /// The kept EXIF and GPS directories, written by `encoder` before the
    /// image's (the EXIF's pixel dimensions made `size`): their pointer
    /// tags and offsets, for [`Self::write_tiff`].
    pub fn write_tiff_dirs<W: std::io::Write + std::io::Seek>(
        &self,
        encoder: &mut tiff::encoder::TiffEncoder<W>,
        size: (u32, u32),
    ) -> tiff::TiffResult<Vec<(u16, u32)>> {
        if self.format != Some(ExportFormat::Tiff) {
            return Ok(Vec::new());
        }
        self.tiff_dirs
            .iter()
            .map(|d| Ok((d.pointer, write_dir(encoder, d, size)?)))
            .collect()
    }
}

/// A tag of `kind` whose `data` is in this computer's byte order.
fn write_raw<W: std::io::Write + std::io::Seek, K: tiff::encoder::TiffKind>(
    encoder: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
    tag: u16,
    kind: u16,
    data: &[u8],
) -> tiff::TiffResult<()> {
    let Some(kind) = tiff::tags::Type::from_u16(kind) else {
        return Ok(());
    };
    let entry = encoder.write_entry_bytes(kind, data)?;
    let mut tags = tiff::Directory::empty();
    tags.extend([(tiff::tags::Tag::from_u16_exhaustive(tag), entry)]);
    encoder.extend_from(&tags);
    Ok(())
}

/// Directory `dir` (and those inside it), written; where it is.
fn write_dir<W: std::io::Write + std::io::Seek>(
    encoder: &mut tiff::encoder::TiffEncoder<W>,
    dir: &TiffDir,
    (w, h): (u32, u32),
) -> tiff::TiffResult<u32> {
    let inner = (dir.dirs.iter())
        .map(|d| Ok((d.pointer, write_dir(encoder, d, (w, h))?)))
        .collect::<tiff::TiffResult<Vec<_>>>()?;
    let mut out = encoder.extra_directory()?;
    for t in &dir.tags {
        match (dir.pointer, t.tag) {
            (TIFF_EXIF, 40962) => out.write_tag(tiff::tags::Tag::from_u16_exhaustive(t.tag), w)?,
            (TIFF_EXIF, 40963) => out.write_tag(tiff::tags::Tag::from_u16_exhaustive(t.tag), h)?,
            _ => write_raw(&mut out, t.tag, t.kind, &t.data)?,
        }
    }
    for (tag, at) in inner {
        out.write_tag(tiff::tags::Tag::from_u16_exhaustive(tag), at)?;
    }
    Ok(out.finish_with_offsets()?.offset)
}

/// A JPEG segment or PNG chunk holding XMP, made true for `size`
/// ([`ImageMetadata::for_size`]); `None` when it holds none, compressed
/// XMP, or would grow past what a segment holds.
fn fresh_xmp_part(part: &[u8], size: (u32, u32)) -> Option<Vec<u8>> {
    const PNG_XMP: &[u8] = b"XML:com.adobe.xmp\0";
    if part.starts_with(&[0xFF, 0xE1]) && part.get(4..)?.starts_with(XMP) {
        let packet = fresh_xmp(&part[4 + XMP.len()..], size);
        let len = u16::try_from(2 + XMP.len() + packet.len()).ok()?;
        return Some([&[0xFF, 0xE1][..], &len.to_be_bytes(), XMP, &packet].concat());
    }
    let body = part.get(8..part.len().checked_sub(4)?)?;
    if part.get(4..8)? == b"iTXt" && body.starts_with(PNG_XMP) {
        let rest = &body[PNG_XMP.len()..];
        // Compression flag and method, then language and translated
        // keyword (each ended by a NUL), then the text.
        if rest.first() != Some(&0) {
            return None;
        }
        let mut at = 2;
        for _ in 0..2 {
            at += rest.get(at..)?.iter().position(|&b| b == 0)? + 1;
        }
        let text = fresh_xmp(&rest[at..], size);
        let body = [PNG_XMP, &rest[..at], &text].concat();
        return Some(png_chunk(b"iTXt", &body));
    }
    None
}

/// The properties of an XMP packet that describe the pixels: they are
/// set to the saved image's (upright, `size`), the thumbnails left out.
const XMP_SIZED: [&str; 5] = [
    "tiff:Orientation",
    "tiff:ImageWidth",
    "tiff:ImageLength",
    "exif:PixelXDimension",
    "exif:PixelYDimension",
];

/// An XMP packet made true for the image of `size` being saved, upright:
/// [`XMP_SIZED`] set, as attributes or elements, and its thumbnails
/// (`xmp:Thumbnails`) left out; the packet's padding takes up the
/// difference in length, so that it stays the same size when it can.
pub fn fresh_xmp(xmp: &[u8], (w, h): (u32, u32)) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(xmp) else {
        return xmp.to_vec();
    };
    let values = [1, w, h, w, h];
    let mut out = text.to_owned();
    for (name, value) in XMP_SIZED.iter().zip(values) {
        set_xmp(&mut out, name, &value.to_string());
    }
    // The thumbnails: an element, removed whole.
    while let Some(start) = out.find("<xmp:Thumbnails") {
        let Some(end) = out[start..].find("</xmp:Thumbnails>") else {
            break;
        };
        out.replace_range(start..start + end + "</xmp:Thumbnails>".len(), "");
    }
    while let Some(start) = find_attribute(&out, "xmp:Thumbnails") {
        let Some(end) = attribute_value(&out, start).map(|(_, end)| end + 1) else {
            break;
        };
        out.replace_range(start..end, "");
    }
    // The padding before the trailer absorbs the change.
    let grown = out.len() as isize - text.len() as isize;
    if let Some(trailer) = out.rfind("<?xpacket end") {
        let pad = out[..trailer]
            .bytes()
            .rev()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        if grown > 0 {
            let cut = (grown as usize).min(pad.saturating_sub(1));
            out.replace_range(trailer - cut..trailer, "");
        } else if grown < 0 {
            out.insert_str(trailer, &" ".repeat(grown.unsigned_abs()));
        }
    }
    out.into_bytes()
}

/// Where attribute `name` starts (as a whole name, followed by `=`).
fn find_attribute(text: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find(name) {
        let at = from + i;
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].trim_start();
        if before.is_some_and(char::is_whitespace) && after.starts_with('=') {
            return Some(at);
        }
        from = at + name.len();
    }
    None
}

/// The value of the attribute starting at `at`: its first byte and its
/// closing quote's.
fn attribute_value(text: &str, at: usize) -> Option<(usize, usize)> {
    let eq = at + text[at..].find('=')?;
    let rest = &text[eq + 1..];
    let skip = rest.len() - rest.trim_start().len();
    let open = eq + 1 + skip;
    let quote = text[open..]
        .chars()
        .next()
        .filter(|c| *c == '"' || *c == '\'')?;
    let close = open + 1 + text[open + 1..].find(quote)?;
    Some((open + 1, close))
}

/// Property `name` set to `value`, wherever it is written as an attribute
/// or as a simple element.
fn set_xmp(text: &mut String, name: &str, value: &str) {
    let mut from = 0;
    while let Some(at) = find_attribute(&text[from..], name).map(|i| from + i) {
        let Some((start, end)) = attribute_value(text, at) else {
            break;
        };
        text.replace_range(start..end, value);
        from = start + value.len();
    }
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut from = 0;
    while let Some(start) = text[from..].find(&open).map(|i| from + i + open.len()) {
        let Some(end) = text[start..].find(&close).map(|i| start + i) else {
            break;
        };
        text.replace_range(start..end, value);
        from = start + value.len();
    }
}

fn splice(data: Vec<u8>, at: usize, parts: &[Vec<u8>]) -> Vec<u8> {
    let at = at.min(data.len());
    let mut out = Vec::with_capacity(data.len() + parts.iter().map(Vec::len).sum::<usize>());
    out.extend(&data[..at]);
    for p in parts {
        out.extend(p);
    }
    out.extend(&data[at..]);
    out
}

#[cfg(test)]
pub(super) fn tests_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    png_chunk(kind, body)
}

fn png_chunk(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(body);
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(body);
    out.extend(crc.sum().to_be_bytes());
    out
}

/// `exif` made true for an image of `size`: its pixel dimensions (tags
/// 40962, 40963) set to it, its thumbnail (the second directory) left
/// out, since it shows the image as it was. Left as it is if it can't
/// be read.
pub fn fresh_exif(exif: &[u8], (w, h): (u32, u32)) -> Vec<u8> {
    let mut out = exif.to_vec();
    let _ = refresh(&mut out, (w, h));
    out
}

fn refresh(data: &mut [u8], (w, h): (u32, u32)) -> Option<()> {
    let little = match data.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |d: &[u8], at: usize| {
        let b: [u8; 2] = d.get(at..at + 2)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |d: &[u8], at: usize| {
        let b: [u8; 4] = d.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let put = |d: &mut [u8], at: usize, bytes: &[u8]| -> Option<()> {
        d.get_mut(at..at + bytes.len())?.copy_from_slice(bytes);
        Some(())
    };
    let bytes32 = |v: u32| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let bytes16 = |v: u16| {
        if little {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        }
    };
    let ifd0 = u32_at(data, 4)? as usize;
    let count = u16_at(data, ifd0)? as usize;
    let entry = |i: usize| ifd0 + 2 + i * 12;
    // No next directory: the thumbnail is gone.
    put(data, entry(count), &[0; 4])?;
    let exif_ifd = (0..count)
        .find_map(|i| (u16_at(data, entry(i))? == 34665).then(|| u32_at(data, entry(i) + 8))?)?
        as usize;
    let n = u16_at(data, exif_ifd)? as usize;
    for i in 0..n {
        let at = exif_ifd + 2 + i * 12;
        let tag = u16_at(data, at)?;
        let value = match tag {
            40962 => w,
            40963 => h,
            _ => continue,
        };
        match u16_at(data, at + 2)? {
            3 => {
                put(data, at + 8, &bytes16(value.min(u16::MAX as u32) as u16))?;
                put(data, at + 10, &[0, 0])?;
            }
            4 => put(data, at + 8, &bytes32(value))?,
            _ => {}
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A little-endian EXIF: IFD0 with the EXIF pointer and a next
    /// directory (the thumbnail), the EXIF directory with the dimensions.
    fn camera_exif() -> Vec<u8> {
        let mut d = b"II*\0".to_vec();
        d.extend(8u32.to_le_bytes());
        // IFD0 at 8: one entry, then the next IFD at 50.
        d.extend(1u16.to_le_bytes());
        d.extend(34665u16.to_le_bytes());
        d.extend(4u16.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend(26u32.to_le_bytes());
        d.extend(56u32.to_le_bytes());
        // EXIF IFD at 26: width (SHORT), height (LONG).
        d.extend(2u16.to_le_bytes());
        d.extend(40962u16.to_le_bytes());
        d.extend(3u16.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend([0xA0, 0x0F, 0, 0]);
        d.extend(40963u16.to_le_bytes());
        d.extend(4u16.to_le_bytes());
        d.extend(1u32.to_le_bytes());
        d.extend(3000u32.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        // IFD1 at 56 (a stand-in).
        d.extend(0u16.to_le_bytes());
        d.extend(0u32.to_le_bytes());
        d
    }

    #[test]
    fn exif_gets_the_new_size_and_loses_its_thumbnail() {
        let exif = fresh_exif(&camera_exif(), (640, 480));
        let u32_at = |at: usize| u32::from_le_bytes(exif[at..at + 4].try_into().unwrap());
        // IFD0's next directory: none.
        assert_eq!(u32_at(8 + 2 + 12), 0);
        assert_eq!(
            u16::from_le_bytes([exif[26 + 2 + 8], exif[26 + 2 + 9]]),
            640
        );
        assert_eq!(u32_at(26 + 2 + 12 + 8), 480);
        // Garbage stays as it is.
        assert_eq!(fresh_exif(b"nonsense", (1, 1)), b"nonsense");
        assert_eq!(
            fresh_exif(b"II*\0\xff\xff\xff\xff", (1, 1)),
            b"II*\0\xff\xff\xff\xff"
        );
    }

    #[test]
    fn jpeg_and_png_segments_are_found_and_put_back() {
        // A JPEG: JFIF at 300 dpi, an XMP packet, a comment.
        let mut jpeg = vec![0xFF, 0xD8];
        let jfif = [
            b"JFIF\0".as_slice(),
            &[1, 2, 1, 0x01, 0x2C, 0x01, 0x2C, 0, 0],
        ]
        .concat();
        let segment = |marker: u8, body: &[u8]| {
            [
                &[0xFF, marker][..],
                &((body.len() + 2) as u16).to_be_bytes(),
                body,
            ]
            .concat()
        };
        jpeg.extend(segment(0xE0, &jfif));
        let xmp = segment(0xE1, &[XMP, b"<x:xmpmeta/>"].concat());
        jpeg.extend(&xmp);
        jpeg.extend(segment(0xFE, b"hello"));
        jpeg.extend([0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        let mut unkept = Unkept::default();
        let meta = jpeg_meta(&jpeg, &mut unkept);
        assert_eq!(meta.density, Some((300.0, 300.0)));
        assert_eq!(meta.raw.len(), 2);
        assert_eq!(unkept, Unkept::default());
        let written = [
            &[0xFF, 0xD8][..],
            &segment(0xE0, &jfif),
            &[0xFF, 0xDB, 0, 2],
        ]
        .concat();
        let back = meta.put_back(ExportFormat::Jpeg, written);
        assert_eq!(jpeg_meta(&back, &mut unkept).raw, meta.raw);
        // Before the tables.
        assert!(back.windows(2).position(|w| w == [0xFF, 0xDB]).unwrap() > xmp.len());
        // Another format: nothing put.
        assert_eq!(meta.put_back(ExportFormat::Png, vec![1, 2]), vec![1, 2]);

        let mut png = image::RgbaImage::new(2, 2);
        png.put_pixel(0, 0, image::Rgba([1, 2, 3, 4]));
        let mut data = Vec::new();
        png.write_to(&mut std::io::Cursor::new(&mut data), ImageFormat::Png)
            .unwrap();
        let meta = ImageMetadata {
            format: Some(ExportFormat::Png),
            density: Some((300.0, 300.0)),
            raw: vec![png_chunk(b"iTXt", b"XML:com.adobe.xmp\0\0\0\0\0<x/>")],
            ..ImageMetadata::default()
        };
        let back = meta.put_back(ExportFormat::Png, data);
        let read = super::png(&back);
        assert_eq!(read.raw, meta.raw);
        let (x, _) = read.density.unwrap();
        assert!((x - 300.0).abs() < 0.1, "{x}");
        // Still a valid PNG.
        assert_eq!(image::load_from_memory(&back).unwrap().to_rgba8(), png);
    }

    #[test]
    fn photoshop_jpeg_resources_lose_their_thumbnail() {
        let resource = |id: u16, data: &[u8]| {
            let mut r = b"8BIM".to_vec();
            r.extend(id.to_be_bytes());
            r.extend([0, 0]);
            r.extend((data.len() as u32).to_be_bytes());
            r.extend(data);
            if data.len() % 2 == 1 {
                r.push(0);
            }
            r
        };
        let iptc = resource(1028, b"\x1c\x02\x74\x00\x02(c)");
        let body = [IPTC, &iptc, &resource(1036, b"old thumbnail")].concat();
        let segment = [
            &[0xFF, 0xED][..],
            &((body.len() + 2) as u16).to_be_bytes(),
            &body,
        ]
        .concat();
        let kept = without_thumbnail(&segment).unwrap();
        assert_eq!(
            kept,
            [
                &[0xFF, 0xED][..],
                &((IPTC.len() + iptc.len() + 2) as u16).to_be_bytes(),
                IPTC,
                &iptc
            ]
            .concat()
        );
        assert_eq!(without_thumbnail(&segment[..segment.len() - 3]), None);
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-metadata-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn resource(id: u16, data: &[u8]) -> Vec<u8> {
        let mut r = b"8BIM".to_vec();
        r.extend(id.to_be_bytes());
        r.extend([0, 0]);
        r.extend((data.len() as u32).to_be_bytes());
        r.extend(data);
        if data.len() % 2 == 1 {
            r.push(0);
        }
        r
    }

    /// A camera or Photoshop TIFF's tags go back byte for byte: text that
    /// isn't UTF-8 or holds several parts, IPTC stored as LONG, image
    /// resources (without their thumbnail), the EXIF directory (its pixel
    /// dimensions made the image's) with the one inside it, the GPS one;
    /// its XMP is made true for the image saved.
    #[test]
    fn tiff_tags_and_directories_are_kept_byte_for_byte() {
        use tiff::encoder::{Rational, TiffEncoder, colortype};
        use tiff::tags::{Tag, Type};
        let dir = temp("tiff-kept");
        let path = dir.join("camera.tif");
        let xmp = b"<x:xmpmeta><rdf:Description tiff:Orientation=\"6\" \
                    exif:PixelXDimension=\"4000\"/></x:xmpmeta>";
        let resources = [resource(1005, &[0; 16]), resource(1036, b"old thumbnail")].concat();
        {
            let mut enc = TiffEncoder::new(std::fs::File::create(&path).unwrap()).unwrap();
            let raw =
                |e: &mut tiff::encoder::DirectoryEncoder<'_, _, _>, tag: u16, ty, data: &[u8]| {
                    let entry = e.write_entry_bytes(ty, data).unwrap();
                    let mut tags = tiff::Directory::empty();
                    tags.extend([(Tag::from_u16_exhaustive(tag), entry)]);
                    e.extend_from(&tags);
                };
            let mut interop = enc.extra_directory().unwrap();
            raw(&mut interop, 1, Type::ASCII, b"R98\0");
            let interop = interop.finish_with_offsets().unwrap().offset;
            let mut exif = enc.extra_directory().unwrap();
            exif.write_tag(Tag::from_u16_exhaustive(40962), 4000u16)
                .unwrap();
            exif.write_tag(Tag::from_u16_exhaustive(40963), 3000u32)
                .unwrap();
            exif.write_tag(Tag::from_u16_exhaustive(33434), Rational { n: 1, d: 250 })
                .unwrap();
            raw(&mut exif, 37500, Type::UNDEFINED, b"Maker notes");
            exif.write_tag(Tag::from_u16_exhaustive(TIFF_INTEROP), interop)
                .unwrap();
            let exif = exif.finish_with_offsets().unwrap().offset;
            let mut gps = enc.extra_directory().unwrap();
            gps.write_tag(Tag::from_u16_exhaustive(0), &[2u8, 3, 0, 0][..])
                .unwrap();
            let gps = gps.finish_with_offsets().unwrap().offset;
            let mut im = enc.new_image::<colortype::RGB8>(2, 2).unwrap();
            let e = im.encoder();
            raw(e, 33432, Type::ASCII, b"\xa9 2003 Studio Lumi\xe8re\0");
            raw(e, 315, Type::ASCII, b"Photographer QA\0Editor QA\0");
            let iptc: [u32; 2] = [0x1c027400, 0x0951412d];
            e.write_tag(Tag::from_u16_exhaustive(33723), &iptc[..])
                .unwrap();
            e.write_tag(Tag::from_u16_exhaustive(700), &xmp[..])
                .unwrap();
            e.write_tag(Tag::from_u16_exhaustive(TIFF_RESOURCES), &resources[..])
                .unwrap();
            e.write_tag(Tag::from_u16_exhaustive(TIFF_EXIF), exif)
                .unwrap();
            e.write_tag(Tag::from_u16_exhaustive(TIFF_GPS), gps)
                .unwrap();
            im.write_data(&[7; 12]).unwrap();
        }
        let (meta, unkept) = read(&path, Some(ImageFormat::Tiff));
        assert_eq!(unkept, Unkept::default());
        let tag =
            |m: &ImageMetadata, n: u16| m.tiff.iter().find(|t| t.tag == n).map(|t| t.data.clone());
        assert_eq!(tag(&meta, 33432).unwrap(), b"\xa9 2003 Studio Lumi\xe8re\0");
        assert_eq!(tag(&meta, 315).unwrap(), b"Photographer QA\0Editor QA\0");
        assert_eq!(
            tag(&meta, TIFF_RESOURCES).unwrap(),
            resource(1005, &[0; 16])
        );
        assert_eq!(meta.tiff_dirs.len(), 2);

        // Saved over, as Save and Close does.
        let image = image::RgbaImage::from_pixel(8, 6, image::Rgba([1, 2, 3, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        super::super::encode_image(
            &image,
            &mut out,
            ExportFormat::Tiff,
            90,
            None,
            None,
            None,
            Some(&meta),
        )
        .unwrap();
        let saved = dir.join("saved.tif");
        std::fs::write(&saved, out.into_inner()).unwrap();
        let (back, unkept) = read(&saved, Some(ImageFormat::Tiff));
        assert_eq!(unkept, Unkept::default());
        for n in [33432, 315, 33723, TIFF_RESOURCES] {
            assert_eq!(tag(&back, n), tag(&meta, n), "{n}");
        }
        let xmp = String::from_utf8(tag(&back, 700).unwrap()).unwrap();
        assert!(xmp.contains("tiff:Orientation=\"1\""), "{xmp}");
        assert!(xmp.contains("exif:PixelXDimension=\"8\""), "{xmp}");
        let exif = back
            .tiff_dirs
            .iter()
            .find(|d| d.pointer == TIFF_EXIF)
            .unwrap();
        let value = |n: u16| exif.tags.iter().find(|t| t.tag == n).unwrap();
        assert_eq!(value(40962).data, 8u32.to_ne_bytes());
        assert_eq!(value(40963).data, 6u32.to_ne_bytes());
        assert_eq!(value(37500).data, b"Maker notes");
        assert_eq!(
            value(33434).data,
            [1u32.to_ne_bytes(), 250u32.to_ne_bytes()].concat()
        );
        assert_eq!(exif.dirs[0].tags[0].data, b"R98\0");
        let gps = back
            .tiff_dirs
            .iter()
            .find(|d| d.pointer == TIFF_GPS)
            .unwrap();
        assert_eq!(gps.tags[0].data, [2, 3, 0, 0]);
        // Still a TIFF the decoder reads.
        assert_eq!(image::open(&saved).unwrap().width(), 8);
    }

    /// A big-endian EXIF directory's values come in this computer's byte
    /// order (the encoder writes in it).
    #[test]
    fn big_endian_tiff_directories_are_read_in_native_order() {
        let mut d = b"MM\0*".to_vec();
        d.extend(8u32.to_be_bytes());
        // IFD0 at 8: the EXIF pointer.
        d.extend(1u16.to_be_bytes());
        d.extend(TIFF_EXIF.to_be_bytes());
        d.extend(4u16.to_be_bytes());
        d.extend(1u32.to_be_bytes());
        d.extend(26u32.to_be_bytes());
        d.extend(0u32.to_be_bytes());
        // EXIF at 26: a LONG and a RATIONAL (at 26 + 2 + 24 + 4 = 56).
        d.extend(2u16.to_be_bytes());
        d.extend(40963u16.to_be_bytes());
        d.extend(4u16.to_be_bytes());
        d.extend(1u32.to_be_bytes());
        d.extend(3000u32.to_be_bytes());
        d.extend(33434u16.to_be_bytes());
        d.extend(5u16.to_be_bytes());
        d.extend(1u32.to_be_bytes());
        d.extend(56u32.to_be_bytes());
        d.extend(0u32.to_be_bytes());
        d.extend(1u32.to_be_bytes());
        d.extend(250u32.to_be_bytes());
        let path = temp("tiff-be").join("be.tif");
        std::fs::write(&path, &d).unwrap();
        let mut file = RawTiff::open(&path).unwrap();
        let ifd0 = file.first_ifd().unwrap();
        let exif = file.dir(&ifd0[0], 0).unwrap();
        assert_eq!(exif.tags[0].data, 3000u32.to_ne_bytes());
        assert_eq!(
            exif.tags[1].data,
            [1u32.to_ne_bytes(), 250u32.to_ne_bytes()].concat()
        );
        // Damaged: a directory past the end.
        let mut cut = d.clone();
        cut.truncate(30);
        std::fs::write(&path, &cut).unwrap();
        let mut file = RawTiff::open(&path).unwrap();
        let ifd0 = file.first_ifd().unwrap();
        assert!(file.dir(&ifd0[0], 0).is_none());
    }

    /// The XMP's orientation, pixel dimensions and thumbnail describe the
    /// image saved, as attributes or elements, the packet's padding taking
    /// up the difference; JPEG segments and PNG chunks are rebuilt.
    #[test]
    fn xmp_describes_the_image_saved() {
        let packet = "<?xpacket begin=\"\"?><x:xmpmeta><rdf:Description \
                      tiff:Orientation=\"6\" tiff:ImageWidth='4000'>\
                      <exif:PixelXDimension>4000</exif:PixelXDimension>\
                      <exif:PixelYDimension>3000</exif:PixelYDimension>\
                      <xmp:Thumbnails><rdf:Alt>old</rdf:Alt></xmp:Thumbnails>\
                      <dc:rights>QA</dc:rights></rdf:Description></x:xmpmeta>\
                      <?xpacket end=\"w\"?>";
        let padded = packet.replace("<?xpacket end", &format!("{}<?xpacket end", " ".repeat(50)));
        let fresh = String::from_utf8(fresh_xmp(padded.as_bytes(), (64, 48))).unwrap();
        assert!(fresh.contains("tiff:Orientation=\"1\""), "{fresh}");
        assert!(fresh.contains("tiff:ImageWidth='64'"), "{fresh}");
        assert!(fresh.contains("<exif:PixelXDimension>64</exif:PixelXDimension>"));
        assert!(fresh.contains("<exif:PixelYDimension>48</exif:PixelYDimension>"));
        assert!(!fresh.contains("Thumbnails") && fresh.contains("<dc:rights>QA"));
        // The padding took the difference: the same length.
        assert_eq!(fresh.len(), padded.len());
        // Nothing to change, or not text: as it was.
        assert_eq!(fresh_xmp(b"<x:xmpmeta/>", (1, 1)), b"<x:xmpmeta/>");
        assert_eq!(fresh_xmp(&[0xFF, 0xFE, 0], (1, 1)), [0xFF, 0xFE, 0]);

        let segment = [
            &[0xFF, 0xE1][..],
            &((2 + XMP.len() + packet.len()) as u16).to_be_bytes(),
            XMP,
            packet.as_bytes(),
        ]
        .concat();
        let chunk = png_chunk(
            b"iTXt",
            &[b"XML:com.adobe.xmp\0\0\0\0\0".as_slice(), packet.as_bytes()].concat(),
        );
        let meta = ImageMetadata {
            raw: vec![segment, chunk, png_chunk(b"tEXt", b"Comment\0hi")],
            ..ImageMetadata::default()
        };
        let sized = meta.for_size((64, 48));
        let jpeg = &sized.raw[0];
        assert_eq!(
            u16::from_be_bytes([jpeg[2], jpeg[3]]) as usize,
            jpeg.len() - 2
        );
        assert!(jpeg.windows(20).any(|w| w == b"tiff:Orientation=\"1\""));
        let png = &sized.raw[1];
        let found = chunks(&[b"\x89PNG\r\n\x1a\n".as_slice(), png].concat())
            .first()
            .map(|(k, _, body)| (*k, body.to_vec()))
            .unwrap();
        assert_eq!(&found.0, b"iTXt");
        assert!(found.1.windows(20).any(|w| w == b"tiff:Orientation=\"1\""));
        assert_eq!(sized.raw[2], meta.raw[2]);
    }

    fn jpeg_meta(data: &[u8], unkept: &mut Unkept) -> ImageMetadata {
        super::jpeg(data, unkept)
    }
}
