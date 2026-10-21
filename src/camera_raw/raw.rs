//! Camera RAW files. The Rust RAW decoders (rawler, rawloader, quickraw)
//! and LibRaw are LGPL, `dng` is AGPL, and the permissive rawkit only
//! reads Sony files: Atelier decodes DNG itself (see [`dng`]), and opens the
//! other formats through the full-size JPEG preview they embed.

mod demosaic;
mod dng;
mod ljpeg;
mod preview;
mod tiff;

use std::path::Path;

use image::RgbaImage;
use image::error::{ImageFormatHint, UnsupportedError, UnsupportedErrorKind};

use super::source::Source;
use crate::document::{FileError, check_size};

/// Extensions of camera RAW files.
pub const EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "rwl",
    "pef", "srw", "3fr", "iiq", "erf", "mef", "mos", "kdc", "dcr", "x3f",
];

/// Whether `path` names a camera RAW file (by its extension).
pub fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// A RAW file read.
pub struct RawFile {
    pub source: Source,
    /// Only the embedded preview could be read (8 bits, the camera's
    /// rendering), not the raw data.
    pub preview_only: bool,
}

/// Reads a RAW file: its raw data for a DNG, else its embedded preview.
pub fn read(path: &Path) -> Result<RawFile, FileError> {
    let data = std::fs::read(path)?;
    if dng::is_dng(&data) {
        match dng::decode(&data) {
            Ok(source) => {
                return Ok(RawFile {
                    source,
                    preview_only: false,
                });
            }
            Err(dng::DngError::Damaged) => return Err(FileError::Damaged),
            Err(dng::DngError::Unsupported) => {}
        }
    }
    let image = preview::largest(&data).ok_or_else(unsupported)?;
    check_size(image.width(), image.height())?;
    Ok(RawFile {
        source: Source::from_rgba(&image),
        preview_only: true,
    })
}

fn unsupported() -> FileError {
    FileError::Image(image::ImageError::Unsupported(
        UnsupportedError::from_format_and_kind(
            ImageFormatHint::Name("RAW".into()),
            UnsupportedErrorKind::Format(ImageFormatHint::Name("RAW".into())),
        ),
    ))
}

/// A RAW file developed with its default settings (placed as a layer,
/// opened by a batch), and whether only its preview could be read.
pub fn develop(path: &Path) -> Result<(RgbaImage, bool), FileError> {
    let file = read(path)?;
    let source = &file.source;
    let settings = super::Settings::for_source(source);
    let analysis = super::Analysis::of(source);
    let frame = super::Frame::whole(source);
    let image = super::render(
        source,
        &settings,
        &frame,
        &analysis,
        super::Visual::Image,
        None,
    );
    Ok((image, file.preview_only))
}

/// Size of the image a RAW file opens to, quickly (for the recent files).
pub fn size(path: &Path) -> Option<(u32, u32)> {
    preview::size(&std::fs::read(path).ok()?)
}

/// The embedded preview (thumbnails of the recent files).
pub fn thumbnail(path: &Path) -> Option<RgbaImage> {
    preview::largest(&std::fs::read(path).ok()?)
}

#[cfg(test)]
pub use dng::tests::{SRGB_CAMERA, TestDng};
