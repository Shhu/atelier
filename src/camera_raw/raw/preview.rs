//! The full-size JPEG preview most RAW files carry (CR2, CR3, NEF, ARW,
//! RAF, ORF, RW2, PEF…): found by its markers wherever the format keeps it,
//! the largest one that decodes, turned like the photo.

use std::io::Cursor;

use image::metadata::Orientation;
use image::{ImageDecoder, ImageFormat, ImageReader, RgbaImage};

use super::tiff::Tiff;

/// Where a JPEG may start (SOI, then a marker), and its size.
fn candidates(data: &[u8]) -> Vec<(usize, (u32, u32))> {
    let mut out: Vec<(usize, (u32, u32))> = data
        .windows(4)
        .enumerate()
        .filter(|(_, w)| w[0] == 0xFF && w[1] == 0xD8 && w[2] == 0xFF && w[3] >= 0xC0)
        .filter_map(|(at, _)| {
            let reader = ImageReader::with_format(Cursor::new(&data[at..]), ImageFormat::Jpeg);
            let size = reader.into_dimensions().ok()?;
            (size.0 >= 16 && size.1 >= 16).then_some((at, size))
        })
        .collect();
    out.sort_by_key(|&(_, (w, h))| std::cmp::Reverse(w as u64 * h as u64));
    out
}

/// Size of the largest preview.
pub fn size(data: &[u8]) -> Option<(u32, u32)> {
    let (at, size) = *candidates(data).first()?;
    let turned = matches!(
        orientation_at(data, at),
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    );
    Some(if turned { (size.1, size.0) } else { size })
}

/// The largest preview, the right way up.
pub fn largest(data: &[u8]) -> Option<RgbaImage> {
    for (at, _) in candidates(data) {
        let reader = ImageReader::with_format(Cursor::new(&data[at..]), ImageFormat::Jpeg);
        let Ok(decoder) = reader.into_decoder() else {
            continue;
        };
        let Ok(mut image) = image::DynamicImage::from_decoder(decoder) else {
            continue;
        };
        image.apply_orientation(orientation_at(data, at));
        return Some(image.into_rgba8());
    }
    None
}

/// Orientation of the preview at `at`: its own EXIF's, else the RAW
/// file's.
fn orientation_at(data: &[u8], at: usize) -> Orientation {
    let reader = ImageReader::with_format(Cursor::new(&data[at..]), ImageFormat::Jpeg);
    let own = reader
        .into_decoder()
        .ok()
        .and_then(|mut d| d.orientation().ok())
        .filter(|&o| o != Orientation::NoTransforms);
    own.or_else(|| container_orientation(data))
        .unwrap_or(Orientation::NoTransforms)
}

/// Orientation recorded by the RAW file: in its first TIFF directory, or
/// for CR3 in the TIFF of its CMT1 box.
fn container_orientation(data: &[u8]) -> Option<Orientation> {
    let tiff_at = if Tiff::new(data).is_some() {
        0
    } else {
        let head = &data[..data.len().min(1 << 20)];
        head.windows(4).position(|w| w == b"CMT1")? + 4
    };
    let (tiff, first) = Tiff::new(&data[tiff_at..])?;
    let (ifd0, _) = tiff.ifd(first)?;
    Orientation::from_exif(tiff.number(&ifd0, 274)? as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32, color: [u8; 3]) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(w, h, image::Rgb(color));
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
            .encode_image(&image)
            .unwrap();
        out
    }

    #[test]
    fn the_largest_embedded_jpeg_is_found_and_turned() {
        // A TIFF-like RAW: header and IFD 0 (orientation 6: turned right),
        // then a thumbnail, raw-looking bytes, and the full-size preview.
        let mut file = b"II*\0\x08\0\0\0".to_vec();
        file.extend(1u16.to_le_bytes());
        file.extend(274u16.to_le_bytes());
        file.extend(3u16.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        file.extend(6u32.to_le_bytes());
        file.extend(0u32.to_le_bytes());
        file.extend(jpeg(32, 24, [200, 0, 0]));
        file.extend((0..5000u32).map(|i| (i * 31 % 251) as u8));
        file.extend(jpeg(64, 48, [0, 0, 200]));
        let image = largest(&file).expect("preview");
        assert_eq!(image.dimensions(), (48, 64));
        let p = image.get_pixel(10, 10).0;
        assert!(p[2] > 150 && p[0] < 50, "{p:?}");
        assert_eq!(size(&file), Some((48, 64)));
        assert!(largest(&file[..100]).is_none());
    }
}
