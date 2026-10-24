//! Export As settings and encoding, shared by File › Export › Export As,
//! Quick Export, the actions, Batch and the Image Processor: the flattened
//! image at a scale, in a format and quality, with or without its color
//! profile and a copyright notice.

use std::io::{Cursor, Write};
use std::path::Path;

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::actions::Resample;
use crate::document::{ExportFormat, FileError};

/// Export As settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportOptions {
    pub format: ExportFormat,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    /// Size of the exported image, in % of the canvas (100: as it is).
    pub scale: f32,
    pub resample: Resample,
    /// Copyright written into the file's metadata (`None`: no metadata).
    pub copyright: Option<String>,
    /// Convert to sRGB: the colors converted from the document's profile
    /// (when it isn't sRGB), sRGB's embedded instead.
    pub srgb: bool,
    /// The document's color profile embedded in the file.
    pub embed_profile: bool,
    /// Use Proof Setup: converted to the proof profile (View › Proof
    /// Setup), CMYK in JPEG, TIFF and PSD.
    pub use_proof: bool,
    /// EXIF written as it is when there is no copyright (Save and Close
    /// gives a file its own back).
    #[serde(skip)]
    pub exif: Option<std::sync::Arc<Vec<u8>>>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Png,
            quality: 90,
            scale: 100.0,
            resample: Resample::Bicubic,
            copyright: None,
            srgb: true,
            embed_profile: true,
            use_proof: false,
            exif: None,
        }
    }
}

impl ExportOptions {
    /// For the web: JPEG at 80, no metadata.
    pub fn web() -> Self {
        Self {
            format: ExportFormat::Jpeg,
            quality: 80,
            ..Self::default()
        }
    }

    /// Size of the exported image for a canvas of `size`: never larger
    /// than Atelier opens, nor than the format holds.
    pub fn size(&self, (w, h): (u32, u32)) -> (u32, u32) {
        let max = self.format.max_side().min(crate::document::MAX_DIMENSION);
        let largest = f64::from(max) / f64::from(w.max(h).max(1));
        let k = (f64::from(self.scale.max(0.0)) / 100.0).min(largest);
        let side = |v: u32| ((f64::from(v) * k).round() as u32).max(1);
        (side(w), side(h))
    }

    /// The flattened image at the export size.
    pub fn render(&self, flat: RgbaImage) -> RgbaImage {
        let (w, h) = self.size(flat.dimensions());
        if (w, h) == flat.dimensions() {
            return flat;
        }
        image::imageops::resize(&flat, w, h, self.resample.filter())
    }

    /// The file's bytes, the image already at its export size; `icc`: the
    /// document's color profile.
    pub fn encode(&self, image: &RgbaImage, icc: Option<&[u8]>) -> Result<Vec<u8>, FileError> {
        let icc = icc.filter(|_| self.embed_profile);
        let mut out = Cursor::new(Vec::new());
        crate::document::encode_image(
            image,
            &mut out,
            self.format,
            self.quality,
            icc,
            self.copyright.as_deref(),
            self.exif.as_deref().map(Vec::as_slice),
        )?;
        Ok(out.into_inner())
    }
}

impl ExportOptions {
    /// [`Self::encode`] in CMYK, converted by `cmyk` (Use Proof Setup).
    pub fn encode_cmyk(
        &self,
        image: &RgbaImage,
        cmyk: &crate::cms::Conversion,
    ) -> Result<Vec<u8>, FileError> {
        let ink = crate::document::to_cmyk(image, cmyk)?;
        let icc = self.embed_profile.then(|| cmyk.to.icc().as_slice());
        let mut out = Cursor::new(Vec::new());
        let (format, quality) = (self.format, self.quality);
        let copyright = self.copyright.as_deref();
        let size = image.dimensions();
        crate::document::encode_cmyk(&ink, size, &mut out, format, quality, icc, copyright)?;
        Ok(out.into_inner())
    }
}

impl Resample {
    pub fn filter(self) -> image::imageops::FilterType {
        use image::imageops::FilterType;
        match self {
            Resample::Nearest => FilterType::Nearest,
            Resample::Bilinear => FilterType::Triangle,
            Resample::Bicubic => FilterType::CatmullRom,
            Resample::Lanczos => FilterType::Lanczos3,
        }
    }
}

/// Writes the bytes of an export, never destroying the file it replaces
/// before they are all written.
pub fn write(path: &Path, bytes: &[u8]) -> Result<(), FileError> {
    crate::document::write_atomic(path, |out| Ok(out.write_all(bytes)?))
}

/// The copyright notice as metadata text: UTF-8 (what readers expect
/// today), without control characters.
pub(crate) fn copyright_text(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// EXIF holding a copyright notice: a little-endian TIFF header and one
/// directory with the Copyright tag (0x8298), its text in UTF-8.
pub(crate) fn copyright_exif(text: &str) -> Vec<u8> {
    let mut ascii = copyright_text(text).into_bytes();
    ascii.push(0);
    let count = ascii.len() as u32;
    let mut exif = b"II*\0".to_vec();
    exif.extend(8u32.to_le_bytes());
    // One entry, then the offset of the next directory (none).
    exif.extend(1u16.to_le_bytes());
    exif.extend(0x8298u16.to_le_bytes());
    exif.extend(2u16.to_le_bytes());
    exif.extend(count.to_le_bytes());
    if count <= 4 {
        ascii.resize(4, 0);
        exif.extend(&ascii);
        exif.extend(0u32.to_le_bytes());
    } else {
        // The value follows the directory: header 8 + count 2 + entry 12 + next 4.
        exif.extend(26u32.to_le_bytes());
        exif.extend(0u32.to_le_bytes());
        exif.extend(&ascii);
    }
    exif
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_stay_within_what_atelier_opens() {
        let options = ExportOptions {
            scale: 1000.0,
            ..ExportOptions::default()
        };
        assert_eq!(options.size((6000, 4000)), (30_000, 20_000));
        // Nor than the format holds.
        let webp = ExportOptions {
            format: ExportFormat::WebP,
            ..options
        };
        assert_eq!(webp.size((6000, 4000)), (16_383, 10_922));
        let image = RgbaImage::new(webp.size((6000, 4000)).0, 1);
        assert!(webp.encode(&image, None).is_ok());
    }

    #[test]
    fn the_copyright_keeps_its_accents() {
        let image = RgbaImage::from_pixel(8, 8, image::Rgba([200, 10, 10, 255]));
        let text = "© 2026 Élodie Atelier";
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::WebP] {
            let options = ExportOptions {
                format,
                copyright: Some(text.into()),
                ..ExportOptions::default()
            };
            let file = options.encode(&image, None).unwrap();
            assert!(
                file.windows(text.len()).any(|w| w == text.as_bytes()),
                "{format:?}"
            );
        }
    }

    #[test]
    fn scaled_exports_and_their_size() {
        let options = ExportOptions {
            scale: 50.0,
            ..ExportOptions::default()
        };
        assert_eq!(options.size((101, 40)), (51, 20));
        let image = options.render(RgbaImage::from_pixel(100, 40, image::Rgba([9, 9, 9, 255])));
        assert_eq!(image.dimensions(), (50, 20));
        let png = options.encode(&image, None).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let gif = ExportOptions {
            format: ExportFormat::Gif,
            ..ExportOptions::default()
        };
        assert_eq!(&gif.encode(&image, None).unwrap()[..3], b"GIF");
    }

    #[test]
    fn jpeg_carries_the_copyright() {
        let image = RgbaImage::from_pixel(8, 8, image::Rgba([200, 10, 10, 255]));
        let options = ExportOptions {
            copyright: Some("© 2026 Atelier".into()),
            ..ExportOptions::web()
        };
        let jpeg = options.encode(&image, None).unwrap();
        let marker = jpeg.windows(6).position(|w| w == b"Exif\0\0").unwrap();
        let exif = &jpeg[marker + 6..];
        assert_eq!(&exif[..4], b"II*\0");
        let text = "© 2026 Atelier\0".as_bytes();
        assert_eq!(&exif[26..26 + text.len()], text);
        // Short notices sit in the entry itself.
        let short = copyright_exif("ab");
        assert_eq!(&short[18..22], b"ab\0\0");
        let plain = ExportOptions::web().encode(&image, None).unwrap();
        assert!(!plain.windows(6).any(|w| w == b"Exif\0\0"));
    }
}
