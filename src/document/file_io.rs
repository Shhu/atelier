//! Image files: decoding (content-detected format, EXIF orientation, size
//! limits), Export As (PNG, JPEG, WebP, GIF, TIFF) and writes that never
//! destroy the file they replace.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use image::codecs::gif::{GifDecoder, GifEncoder};
use image::codecs::tiff::TiffEncoder;
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::codecs::{jpeg::JpegEncoder, png::PngDecoder, png::PngEncoder};
use image::{
    AnimationDecoder, DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError,
    ImageFormat, ImageReader, Limits, RgbaImage,
};

use crate::i18n::t;

/// Largest canvas side, in pixels.
pub const MAX_DIMENSION: u32 = 30_000;

/// Why a file could not be read or written.
#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    Image(ImageError),
    /// Wider or taller than [`MAX_DIMENSION`].
    TooLarge(u32, u32),
    /// No pixels (0 × 0).
    Empty,
    /// Not an Atelier document, or too damaged to tell.
    NotNative,
    /// Not a Photoshop document, or one Atelier can't read.
    NotPsd,
    /// An Atelier document whose content is inconsistent.
    Damaged,
    /// Made by a newer Atelier (format version).
    NewerVersion(u32),
    /// The export format cannot hold an image this large (maximum side).
    FormatTooSmall(ExportFormat, u32),
}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        FileError::Io(e)
    }
}

impl From<ImageError> for FileError {
    fn from(e: ImageError) -> Self {
        match e {
            ImageError::IoError(e) => FileError::Io(e),
            e => FileError::Image(e),
        }
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::io::ErrorKind;
        let text = match self {
            FileError::Io(e) => match e.kind() {
                ErrorKind::NotFound => t!("file-error-not-found"),
                ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem => {
                    t!("file-error-permission")
                }
                ErrorKind::StorageFull | ErrorKind::QuotaExceeded => t!("file-error-disk-full"),
                ErrorKind::UnexpectedEof | ErrorKind::InvalidData => t!("file-error-damaged"),
                _ => e.to_string(),
            },
            FileError::Image(ImageError::Unsupported(_)) => t!("file-error-unsupported"),
            FileError::Image(ImageError::Decoding(_)) => t!("file-error-damaged"),
            FileError::Image(e) => e.to_string(),
            FileError::TooLarge(w, h) => t!(
                "file-error-too-large",
                width = *w,
                height = *h,
                max = MAX_DIMENSION
            ),
            FileError::Empty => t!("file-error-empty"),
            FileError::NotNative => t!("file-error-not-atelier"),
            FileError::NotPsd => t!("xchg-file-error-not-psd"),
            FileError::Damaged => t!("file-error-damaged"),
            FileError::NewerVersion(v) => t!("file-error-newer-version", version = *v),
            FileError::FormatTooSmall(format, max) => {
                t!(
                    "file-error-format-limit",
                    format = format.name(),
                    max = *max
                )
            }
        };
        f.write_str(&text)
    }
}

/// Refuses images Atelier cannot hold: empty, or a side over [`MAX_DIMENSION`].
pub fn check_size(width: u32, height: u32) -> Result<(), FileError> {
    if width == 0 || height == 0 {
        Err(FileError::Empty)
    } else if width > MAX_DIMENSION || height > MAX_DIMENSION {
        Err(FileError::TooLarge(width, height))
    } else {
        Ok(())
    }
}

/// Decoder memory limit: what the largest accepted image needs in 16-bit RGBA.
fn limits() -> Limits {
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(MAX_DIMENSION as u64 * MAX_DIMENSION as u64 * 8);
    limits
}

/// A document just opened.
pub struct Opened {
    pub doc: super::Document,
    /// The file is an animation: only its first frame was read.
    pub animated: bool,
    /// What had to be simplified to open it (translated).
    pub notes: Vec<String>,
}

/// A decoded image file.
pub struct Decoded {
    /// Pixels, the right way up (EXIF orientation applied).
    pub image: RgbaImage,
    /// Embedded color profile, kept for the exports.
    pub icc_profile: Option<Vec<u8>>,
    /// The file is an animation: only its first frame was read.
    pub animated: bool,
}

/// Decodes an image file whatever its extension says (a PNG named .jpg,
/// a .jfif JPEG…).
pub fn read_image(path: &Path) -> Result<Decoded, FileError> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    // Dimensions first (header only): too large is refused with its size.
    reader.no_limits();
    let mut decoder = reader.into_decoder()?;
    let (w, h) = decoder.dimensions();
    check_size(w, h)?;
    decoder.set_limits(limits())?;
    let orientation = decoder.orientation()?;
    let icc_profile = decoder.icc_profile().ok().flatten();
    let mut image = DynamicImage::from_decoder(decoder)?;
    let icc_profile = super::color::rgb_pixels(path, format, &mut image, icc_profile);
    image.apply_orientation(orientation);
    Ok(Decoded {
        image: image.into_rgba8(),
        icc_profile,
        animated: format.is_some_and(|f| is_animated(path, f)),
    })
}

/// Size of an image file as it opens (EXIF orientation applied), from its
/// header only.
fn image_size(path: &Path) -> Option<(u32, u32)> {
    let mut decoder = ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (w, h) = decoder.dimensions();
    use image::metadata::Orientation::{Rotate90, Rotate90FlipH, Rotate270, Rotate270FlipH};
    match decoder.orientation() {
        Ok(Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH) => Some((h, w)),
        _ => Some((w, h)),
    }
}

/// Size of a file Atelier opens (Atelier or Photoshop document, image),
/// read quickly.
pub fn file_size(path: &Path) -> Option<(u32, u32)> {
    if super::is_native(path) {
        super::native::canvas_size(path)
    } else if super::is_psd(path) {
        super::psd::canvas_size(path)
    } else if crate::camera_raw::raw::is_raw(path) {
        crate::camera_raw::raw::size(path)
    } else {
        image_size(path)
    }
}

/// Preview of a file Atelier opens, fitting in `width` × `height`.
pub fn file_thumbnail(path: &Path, width: u32, height: u32) -> Option<RgbaImage> {
    let image = if super::is_native(path) {
        super::native::thumbnail_of(path)?
    } else if super::is_psd(path) {
        super::psd::thumbnail_of(path)?
    } else if crate::camera_raw::raw::is_raw(path) {
        crate::camera_raw::raw::thumbnail(path)?
    } else {
        read_image(path).ok()?.image
    };
    Some(
        DynamicImage::ImageRgba8(image)
            .thumbnail(width, height)
            .into_rgba8(),
    )
}

/// More than one frame (GIF, APNG, animated WebP).
fn is_animated(path: &Path, format: ImageFormat) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let reader = BufReader::new(file);
    match format {
        ImageFormat::Gif => GifDecoder::new(reader).is_ok_and(|d| d.into_frames().nth(1).is_some()),
        ImageFormat::Png => PngDecoder::new(reader).is_ok_and(|d| d.is_apng().unwrap_or(false)),
        ImageFormat::WebP => WebPDecoder::new(reader).is_ok_and(|d| d.has_animation()),
        _ => false,
    }
}

/// Formats of Export As. PNG, WebP and TIFF are lossless; GIF has 256
/// colors; PSD keeps the layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Png,
    Jpeg,
    WebP,
    Gif,
    Tiff,
    Psd,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 6] = [
        ExportFormat::Png,
        ExportFormat::Jpeg,
        ExportFormat::WebP,
        ExportFormat::Gif,
        ExportFormat::Tiff,
        ExportFormat::Psd,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ExportFormat::Png => "PNG",
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::WebP => "WebP",
            ExportFormat::Gif => "GIF",
            ExportFormat::Tiff => "TIFF",
            ExportFormat::Psd => "PSD",
        }
    }

    /// File extensions, the usual one first.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            ExportFormat::Png => &["png"],
            ExportFormat::Jpeg => &["jpg", "jpeg", "jpe", "jfif"],
            ExportFormat::WebP => &["webp"],
            ExportFormat::Gif => &["gif"],
            ExportFormat::Tiff => &["tif", "tiff"],
            ExportFormat::Psd => &["psd"],
        }
    }

    /// Largest side the format can store.
    fn max_side(self) -> u32 {
        match self {
            // What Atelier itself opens.
            ExportFormat::Png | ExportFormat::Tiff => MAX_DIMENSION,
            ExportFormat::Jpeg | ExportFormat::Gif => u16::MAX as u32,
            ExportFormat::WebP => 16_383,
            ExportFormat::Psd => 30_000,
        }
    }
}

/// Writes `image` to `path` in `format` (JPEG `quality` 1–100), with the
/// color profile if there is one. JPEG has no transparency: the image is
/// flattened onto white.
pub fn export(
    image: &RgbaImage,
    path: &Path,
    format: ExportFormat,
    quality: u8,
    icc_profile: Option<&[u8]>,
) -> Result<(), FileError> {
    write_atomic(path, |out| {
        encode_image(image, out, format, quality, icc_profile, None)
    })
}

/// Encodes `image` in `format` into `out` (see [`export`]), with a
/// copyright notice where the format has room for it (not GIF).
pub fn encode_image(
    image: &RgbaImage,
    out: &mut (impl Write + Seek),
    format: ExportFormat,
    quality: u8,
    icc_profile: Option<&[u8]>,
    copyright: Option<&str>,
) -> Result<(), FileError> {
    let (w, h) = image.dimensions();
    if w.max(h) > format.max_side() {
        return Err(FileError::FormatTooSmall(format, format.max_side()));
    }
    // TIFF: its own Copyright tag (its encoder takes no EXIF).
    if let (ExportFormat::Tiff, Some(text)) = (format, copyright) {
        return tiff_with_copyright(image, out, icc_profile, text);
    }
    let exif = copyright.map(crate::export::copyright_exif);
    let meta = (icc_profile, exif.as_deref());
    match format {
        ExportFormat::Png => encode(PngEncoder::new(out), image, meta),
        ExportFormat::Jpeg => {
            let encoder = JpegEncoder::new_with_quality(out, quality.clamp(1, 100));
            let rgb = flatten_on_white(image);
            encode_with(encoder, rgb.as_raw(), (w, h), ExtendedColorType::Rgb8, meta)
        }
        ExportFormat::WebP => encode(WebPEncoder::new_lossless(out), image, meta),
        ExportFormat::Tiff => encode(TiffEncoder::new(out), image, meta),
        ExportFormat::Gif => {
            // Speed 10: the quantizer's usual compromise.
            let mut encoder = GifEncoder::new_with_speed(out, 10);
            encoder.encode(image.as_raw(), w, h, ExtendedColorType::Rgba8)?;
            Ok(())
        }
        ExportFormat::Psd => unreachable!("written with its layers by `Document::export`"),
    }
}

fn encode(
    encoder: impl ImageEncoder,
    image: &RgbaImage,
    meta: (Option<&[u8]>, Option<&[u8]>),
) -> Result<(), FileError> {
    let size = image.dimensions();
    encode_with(
        encoder,
        image.as_raw(),
        size,
        ExtendedColorType::Rgba8,
        meta,
    )
}

fn encode_with(
    mut encoder: impl ImageEncoder,
    pixels: &[u8],
    (w, h): (u32, u32),
    color: ExtendedColorType,
    (icc, exif): (Option<&[u8]>, Option<&[u8]>),
) -> Result<(), FileError> {
    // Formats without room for them just go without.
    if let Some(icc) = icc {
        let _ = encoder.set_icc_profile(icc.to_vec());
    }
    if let Some(exif) = exif {
        let _ = encoder.set_exif_metadata(exif.to_vec());
    }
    encoder.write_image(pixels, w, h, color)?;
    Ok(())
}

/// An RGBA TIFF with its profile and a Copyright tag.
fn tiff_with_copyright(
    image: &RgbaImage,
    out: &mut (impl Write + Seek),
    icc: Option<&[u8]>,
    copyright: &str,
) -> Result<(), FileError> {
    use tiff::encoder::{Compression, TiffEncoder, colortype::RGBA8};
    let tiff = |e: tiff::TiffError| FileError::Io(std::io::Error::other(e.to_string()));
    let mut encoder = TiffEncoder::new(out)
        .map_err(tiff)?
        .with_compression(Compression::Lzw);
    let (w, h) = image.dimensions();
    let mut file = encoder.new_image::<RGBA8>(w, h).map_err(tiff)?;
    // In UTF-8, as readers expect today (the crate only writes ASCII).
    let mut text = crate::export::copyright_text(copyright).into_bytes();
    text.push(0);
    let entry = (file.encoder())
        .write_entry_bytes(tiff::tags::Type::ASCII, &text)
        .map_err(tiff)?;
    let mut tags = tiff::Directory::empty();
    tags.extend([(tiff::tags::Tag::Copyright, entry)]);
    file.encoder().extend_from(&tags);
    if let Some(icc) = icc {
        file.encoder()
            .write_tag(tiff::tags::Tag::IccProfile, icc)
            .map_err(tiff)?;
    }
    file.write_data(image.as_raw()).map_err(tiff)
}

/// Encodes CMYK pixels (4 bytes each, 0 = no ink) of `size` into `out`
/// as a JPEG (Adobe's marker, the inks inverted as Photoshop writes them)
/// or an LZW-compressed TIFF, with their profile if given.
pub fn encode_cmyk(
    cmyk: &[u8],
    (w, h): (u32, u32),
    out: &mut (impl Write + Seek),
    format: ExportFormat,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<(), FileError> {
    if w.max(h) > format.max_side() {
        return Err(FileError::FormatTooSmall(format, format.max_side()));
    }
    let invalid = |e: String| FileError::Io(std::io::Error::other(e));
    match format {
        ExportFormat::Jpeg => {
            let mut encoder = jpeg_encoder::Encoder::new(out, quality.clamp(1, 100));
            if let Some(icc) = icc {
                encoder
                    .add_icc_profile(icc)
                    .map_err(|e| invalid(e.to_string()))?;
            }
            encoder
                .encode(cmyk, w as u16, h as u16, jpeg_encoder::ColorType::Cmyk)
                .map_err(|e| invalid(e.to_string()))
        }
        ExportFormat::Tiff => {
            use tiff::encoder::{Compression, TiffEncoder, colortype::CMYK8};
            let tiff = |e: tiff::TiffError| invalid(e.to_string());
            let mut encoder = TiffEncoder::new(out)
                .map_err(tiff)?
                .with_compression(Compression::Lzw);
            let mut image = encoder.new_image::<CMYK8>(w, h).map_err(tiff)?;
            if let Some(icc) = icc {
                image
                    .encoder()
                    .write_tag(tiff::tags::Tag::IccProfile, icc)
                    .map_err(tiff)?;
            }
            image.write_data(cmyk).map_err(tiff)
        }
        _ => unreachable!("no CMYK {format:?}"),
    }
}

/// Whether `format` can hold CMYK (see [`encode_cmyk`]; a Photoshop
/// document too).
pub fn has_cmyk(format: ExportFormat) -> bool {
    matches!(
        format,
        ExportFormat::Jpeg | ExportFormat::Tiff | ExportFormat::Psd
    )
}

pub(super) fn flatten_on_white(image: &RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])])
    })
}

/// Writes a file without ever destroying the one it replaces: into a
/// temporary file in the same folder, renamed over the target only once
/// completely written. On error the target is untouched.
pub fn write_atomic(
    path: &Path,
    write: impl FnOnce(&mut BufWriter<File>) -> Result<(), FileError>,
) -> Result<(), FileError> {
    // Through a symbolic link, the file it points to is replaced (or
    // created), not the link.
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).or_else(|_| {
            let to = std::fs::read_link(path)?;
            Ok::<_, std::io::Error>(path.parent().map_or(to.clone(), |dir| dir.join(&to)))
        })?,
        _ => path.to_path_buf(),
    };
    // A read-only file stays as it is (the rename would replace it).
    if std::fs::metadata(&target).is_ok_and(|m| m.permissions().readonly()) {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into());
    }
    let tmp = temporary_path(&target);
    let result = (|| {
        let mut out = BufWriter::new(File::create(&tmp)?);
        write(&mut out)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        if let Ok(meta) = std::fs::metadata(&target) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// ".name.1234-5.tmp" next to `target` (1234: this process, 5: this
/// write, for writers on several threads).
fn temporary_path(target: &Path) -> PathBuf {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = OsString::from(".");
    name.push(target.file_name().unwrap_or_default());
    name.push(format!(".{}-{n}.tmp", std::process::id()));
    target.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-file-io-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_failed_write_keeps_the_original() {
        let dir = dir("failed-write");
        let path = dir.join("photo.png");
        std::fs::write(&path, b"original").unwrap();
        let err = write_atomic(&path, |out| {
            out.write_all(b"half")?;
            Err(FileError::Damaged)
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        // No temporary file left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn export_too_large_for_webp_keeps_the_original() {
        let dir = dir("webp-limit");
        let path = dir.join("wide.webp");
        std::fs::write(&path, b"original").unwrap();
        let image = RgbaImage::new(16_384, 1);
        let err = export(&image, &path, ExportFormat::WebP, 90, None).unwrap_err();
        assert!(matches!(
            err,
            FileError::FormatTooSmall(ExportFormat::WebP, 16_383)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn read_only_files_and_dangling_links() {
        let dir = dir("read-only");
        let locked = dir.join("locked.png");
        std::fs::write(&locked, b"original").unwrap();
        let mut p = std::fs::metadata(&locked).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&locked, p).unwrap();
        let err = write_atomic(&locked, |out| Ok(out.write_all(b"new")?)).unwrap_err();
        assert!(
            matches!(&err, FileError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied)
        );
        assert_eq!(std::fs::read(&locked).unwrap(), b"original");
        // Through a link to a file not there yet: that file is created.
        let link = dir.join("link.png");
        std::os::unix::fs::symlink("target.png", &link).unwrap();
        write_atomic(&link, |out| Ok(out.write_all(b"made")?)).unwrap();
        assert_eq!(std::fs::read(dir.join("target.png")).unwrap(), b"made");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn temporary_names_differ_between_writes() {
        let target = Path::new("/somewhere/photo.png");
        assert_ne!(temporary_path(target), temporary_path(target));
    }

    #[test]
    fn tiff_holds_the_copyright() {
        let image = RgbaImage::from_pixel(4, 4, image::Rgba([9, 9, 9, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        let text = "© 2026 Élodie";
        encode_image(&image, &mut out, ExportFormat::Tiff, 90, None, Some(text)).unwrap();
        let data = out.into_inner();
        assert!(data.windows(text.len()).any(|w| w == text.as_bytes()));
        let back = image::load_from_memory(&data).unwrap().into_rgba8();
        assert_eq!(back, image);
    }

    #[test]
    fn jpeg_export_uses_the_quality_and_flattens_on_white() {
        let dir = dir("jpeg");
        let mut image = RgbaImage::from_pixel(64, 64, image::Rgba([0, 0, 0, 0]));
        // Noise, for the quality to make a difference.
        for (x, y, p) in image
            .enumerate_pixels_mut()
            .filter(|(x, y, _)| (x * 7 + y * 13) % 5 == 0)
        {
            *p = image::Rgba([(x * 4) as u8, (y * 4) as u8, 128, 255]);
        }
        let (low, high) = (dir.join("low.jpg"), dir.join("high.jpg"));
        export(&image, &low, ExportFormat::Jpeg, 30, None).unwrap();
        export(&image, &high, ExportFormat::Jpeg, 95, None).unwrap();
        let size = |p: &Path| std::fs::metadata(p).unwrap().len();
        assert!(size(&high) > size(&low));
        let back = image::open(&high).unwrap().to_rgb8();
        // A transparent pixel came out white.
        assert!(back.get_pixel(1, 0).0.iter().all(|&c| c > 240));
    }

    #[test]
    fn png_and_webp_exports_are_lossless_and_keep_the_profile() {
        let dir = dir("lossless");
        let image = RgbaImage::from_fn(33, 17, |x, y| {
            image::Rgba([x as u8 * 7, y as u8 * 13, 99, (x * y) as u8])
        });
        let icc = vec![7u8; 200];
        for format in [ExportFormat::Png, ExportFormat::WebP] {
            let path = dir.join(format!("out.{}", format.extensions()[0]));
            export(&image, &path, format, 90, Some(&icc)).unwrap();
            let decoded = read_image(&path).unwrap();
            assert_eq!(decoded.image, image, "{format:?}");
            assert_eq!(decoded.icc_profile.as_deref(), Some(&icc[..]), "{format:?}");
        }
    }

    #[test]
    fn format_is_read_from_the_content() {
        let dir = dir("guess");
        // A PNG named .jpg, and a JPEG named .jfif.
        let png = dir.join("photo.jpg");
        RgbaImage::from_pixel(5, 4, image::Rgba([1, 2, 3, 255]))
            .save_with_format(&png, ImageFormat::Png)
            .unwrap();
        assert_eq!(read_image(&png).unwrap().image.dimensions(), (5, 4));
        let jfif = dir.join("photo.jfif");
        image::RgbImage::new(6, 3)
            .save_with_format(&jfif, ImageFormat::Jpeg)
            .unwrap();
        assert_eq!(read_image(&jfif).unwrap().image.dimensions(), (6, 3));
        assert_eq!(image_size(&jfif), Some((6, 3)));
    }

    #[test]
    fn empty_and_oversized_images_are_refused() {
        let dir = dir("sizes");
        let empty = dir.join("empty.pnm");
        std::fs::write(&empty, b"P6\n0 0\n255\n").unwrap();
        assert!(matches!(read_image(&empty), Err(FileError::Empty)));
        let wide = dir.join("wide.png");
        image::GrayImage::new(MAX_DIMENSION + 1, 1)
            .save(&wide)
            .unwrap();
        assert!(matches!(
            read_image(&wide),
            Err(FileError::TooLarge(30_001, 1))
        ));
        // Beyond the image crate's default 512 MiB limit, but allowed.
        assert!(check_size(12_000, 12_000).is_ok());
    }

    #[test]
    fn exif_orientation_is_applied() {
        // 2 × 1 JPEG (red, blue) with Orientation = 6 (rotate 90° clockwise).
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 100);
        let exif: Vec<u8> = [
            b"MM\0\x2a\0\0\0\x08".as_slice(),
            &[0, 1],
            &[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0],
            &[0, 0, 0, 0],
        ]
        .concat();
        encoder.set_exif_metadata(exif).unwrap();
        let mut img = image::RgbImage::new(16, 8);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 8 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            };
        }
        encoder
            .write_image(img.as_raw(), 16, 8, ExtendedColorType::Rgb8)
            .unwrap();
        let dir = dir("exif");
        let path = dir.join("phone.jpg");
        std::fs::write(&path, jpeg).unwrap();
        let decoded = read_image(&path).unwrap();
        assert_eq!(decoded.image.dimensions(), (8, 16));
        assert_eq!(image_size(&path), Some((8, 16)));
        // Red, on the left, is now at the top.
        assert!(decoded.image.get_pixel(4, 2).0[0] > 200);
        assert!(decoded.image.get_pixel(4, 13).0[2] > 200);
    }

    #[test]
    fn animated_gif_opens_its_first_frame() {
        use image::codecs::gif::GifEncoder;
        let dir = dir("gif");
        let path = dir.join("anim.gif");
        let mut encoder = GifEncoder::new(File::create(&path).unwrap());
        for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            let frame = image::Frame::new(RgbaImage::from_pixel(4, 4, image::Rgba(color)));
            encoder.encode_frame(frame).unwrap();
        }
        drop(encoder);
        let decoded = read_image(&path).unwrap();
        assert!(decoded.animated);
        assert_eq!(decoded.image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        let still = dir.join("still.png");
        RgbaImage::new(2, 2).save(&still).unwrap();
        assert!(!read_image(&still).unwrap().animated);
    }
}
