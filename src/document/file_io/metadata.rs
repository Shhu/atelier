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
    /// TIFF: tags kept by number, BYTE (XMP, IPTC) or ASCII (copyright,
    /// artist, description) values.
    pub tiff: Vec<(u16, TiffValue)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TiffValue {
    Bytes(Vec<u8>),
    Text(String),
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

/// TIFF tags kept: XMP, IPTC, copyright, artist, description.
const TIFF_BYTES: [u16; 2] = [700, 33723];
const TIFF_TEXT: [u16; 3] = [33432, 315, 270];

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
    let data = segment.get(4 + IPTC.len()..)?;
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
    let body = [IPTC, &kept].concat();
    let len = u16::try_from(body.len() + 2).ok()?;
    Some([&[0xFF, 0xED][..], &len.to_be_bytes(), &body].concat())
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
    for tag in TIFF_BYTES {
        match d.find_tag(Tag::from_u16_exhaustive(tag)).ok().flatten() {
            None => {}
            Some(v) => match v.into_u8_vec() {
                Ok(bytes) => meta.tiff.push((tag, TiffValue::Bytes(bytes))),
                Err(_) => unkept.xmp = true,
            },
        }
    }
    for tag in TIFF_TEXT {
        match d.find_tag(Tag::from_u16_exhaustive(tag)).ok().flatten() {
            None => {}
            Some(v) => match v.into_string() {
                Ok(text) => meta.tiff.push((tag, TiffValue::Text(text))),
                Err(_) => unkept.xmp = true,
            },
        }
    }
    let has = |d: &mut tiff::decoder::Decoder<_>, tag: u16| {
        d.find_tag(Tag::from_u16_exhaustive(tag))
            .ok()
            .flatten()
            .is_some()
    };
    unkept.exif = has(&mut d, 34665);
    // Image resources, and the layers some applications store.
    unkept.photoshop = has(&mut d, 34377) || has(&mut d, 37724);
    Some(meta)
}

/// A resolution in whole pixels per inch.
fn density_u16((x, y): (f64, f64)) -> (u16, u16) {
    let side = |v: f64| v.round().clamp(1.0, f64::from(u16::MAX)) as u16;
    (side(x), side(y))
}

impl ImageMetadata {
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
    /// copyright unless a `new_copyright` is written).
    pub fn write_tiff<W: std::io::Write + std::io::Seek, K: tiff::encoder::TiffKind>(
        &self,
        encoder: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
        new_copyright: bool,
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
        for (tag, value) in &self.tiff {
            if new_copyright && *tag == 33432 {
                continue;
            }
            match value {
                TiffValue::Bytes(b) => encoder.write_tag(Tag::from_u16_exhaustive(*tag), &b[..])?,
                TiffValue::Text(t) => {
                    let mut text = t.clone().into_bytes();
                    text.push(0);
                    let entry = encoder.write_entry_bytes(tiff::tags::Type::ASCII, &text)?;
                    let mut tags = tiff::Directory::empty();
                    tags.extend([(Tag::from_u16_exhaustive(*tag), entry)]);
                    encoder.extend_from(&tags);
                }
            }
        }
        Ok(())
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

    fn jpeg_meta(data: &[u8], unkept: &mut Unkept) -> ImageMetadata {
        super::jpeg(data, unkept)
    }
}
