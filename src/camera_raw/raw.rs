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
use crate::document::{Budget, FileError, check_size};

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

/// Reads a RAW file: its raw data for a DNG, else its embedded preview;
/// the file, its decoding and the pixels it gives taken from `budget`.
pub fn read(path: &Path, budget: &mut Budget) -> Result<RawFile, FileError> {
    let data = std::fs::read(path)?;
    budget.take_bytes(data.len() as u64)?;
    if dng::is_dng(&data) {
        match dng::decode(&data, budget) {
            Ok(source) => {
                return Ok(RawFile {
                    source,
                    preview_only: false,
                });
            }
            Err(dng::DngError::Damaged) => return Err(FileError::Damaged),
            Err(dng::DngError::Memory(e)) => return Err(e),
            Err(dng::DngError::Unsupported) => {}
        }
    }
    let image = preview::largest(&data, budget)?.ok_or_else(unsupported)?;
    check_size(image.width(), image.height())?;
    budget.take(image.width(), image.height(), SOURCE_BYTES_PER_PIXEL)?;
    Ok(RawFile {
        source: Source::from_rgba(&image),
        preview_only: true,
    })
}

/// Bytes per pixel of a [`Source`]: its colors, and opacity.
const SOURCE_BYTES_PER_PIXEL: u64 = 12 + 1;

/// Bytes per pixel developing a source takes beside it: the image made,
/// and what rendering holds while making it.
const DEVELOP_BYTES_PER_PIXEL: u64 = 4 + 12;

fn unsupported() -> FileError {
    FileError::Image(image::ImageError::Unsupported(
        UnsupportedError::from_format_and_kind(
            ImageFormatHint::Name("RAW".into()),
            UnsupportedErrorKind::Format(ImageFormatHint::Name("RAW".into())),
        ),
    ))
}

/// A RAW file developed with its default settings (placed as a layer,
/// opened by a batch), and whether only its preview could be read; within
/// `budget`.
pub fn develop(path: &Path, budget: &mut Budget) -> Result<(RgbaImage, bool), FileError> {
    let file = read(path, budget)?;
    let source = &file.source;
    budget.take(source.width, source.height, DEVELOP_BYTES_PER_PIXEL)?;
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

/// The embedded preview (thumbnails of the recent files), within
/// `budget`.
pub fn thumbnail(path: &Path, budget: &mut Budget) -> Option<RgbaImage> {
    let data = std::fs::read(path).ok()?;
    budget.take_bytes(data.len() as u64).ok()?;
    preview::largest(&data, budget).ok().flatten()
}

#[cfg(test)]
pub use dng::tests::{SRGB_CAMERA, TestDng};
