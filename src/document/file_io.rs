//! Image files: decoding (content-detected format, EXIF orientation, size
//! limits), Export As (PNG, JPEG, WebP, GIF, TIFF) and writes that never
//! destroy the file they replace.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

use image::codecs::gif::{GifDecoder, GifEncoder};
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::codecs::{jpeg::JpegEncoder, png::PngDecoder, png::PngEncoder};
use image::{
    AnimationDecoder, DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError,
    ImageFormat, ImageReader, Limits, RgbaImage,
};

use crate::i18n::t;

mod metadata;
pub use metadata::ImageMetadata;

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
    /// A layer (its name and size) larger than any file may hold (see
    /// [`layer_fits`]): saving it would write a file no reader takes.
    LayerTooLarge(String, u32, u32),
    /// Opening the file would take more memory than the computer has (its
    /// memory, in bytes).
    OutOfMemory(u64),
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
            FileError::LayerTooLarge(name, w, h) => t!(
                "files5-error-layer-too-large",
                name = name.as_str(),
                width = *w,
                height = *h
            ),
            FileError::OutOfMemory(memory) => t!(
                "files5-error-out-of-memory",
                memory = (*memory as f64 / f64::from(1 << 30)).round().max(1.0)
            ),
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

/// Largest side of a layer, mask or channel in a file (it may reach far
/// beyond the canvas).
pub const MAX_LAYER_SIDE: u32 = 4 * MAX_DIMENSION;

/// A layer, mask or channel of `w` × `h` pixels can be written and read
/// back: sides up to [`MAX_LAYER_SIDE`], the area of the largest canvas.
/// The same rule for the readers and the writers of both formats, so that
/// Atelier never writes a file it refuses to open.
pub fn layer_fits(w: u32, h: u32) -> bool {
    w <= MAX_LAYER_SIDE
        && h <= MAX_LAYER_SIDE
        && u64::from(w) * u64::from(h) <= u64::from(MAX_DIMENSION).pow(2)
}

/// The memory the pixels of a document being opened may take, all its
/// layers, masks, channels and smart objects together: what the computer
/// has. Each part proves it can hold its pixels (its data can't be
/// inflated more than deflate allows), and no part is read twice, so only
/// honest files of that size reach this limit; they are refused rather
/// than aborting the application when memory runs out.
pub(super) struct Budget {
    left: u64,
    memory: u64,
}

impl Budget {
    pub(super) fn new() -> Self {
        Self::of(physical_memory().unwrap_or(16 << 30))
    }

    /// A budget of `memory` bytes.
    pub(super) fn of(memory: u64) -> Self {
        Self {
            left: memory,
            memory,
        }
    }

    /// Takes `w` × `h` pixels of `bpp` bytes from the budget.
    pub(super) fn take(&mut self, w: u32, h: u32, bpp: u64) -> Result<(), FileError> {
        self.take_bytes(
            u64::from(w)
                .saturating_mul(u64::from(h))
                .saturating_mul(bpp),
        )
    }

    pub(super) fn take_bytes(&mut self, bytes: u64) -> Result<(), FileError> {
        match self.left.checked_sub(bytes) {
            Some(left) => {
                self.left = left;
                Ok(())
            }
            None => Err(FileError::OutOfMemory(self.memory)),
        }
    }

    /// What is left, in bytes.
    pub(super) fn left(&self) -> u64 {
        self.left
    }
}

/// The computer's memory, in bytes.
#[cfg(target_os = "linux")]
fn physical_memory() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kb: u64 = line["MemTotal:".len()..]
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[cfg(windows)]
fn physical_memory() -> Option<u64> {
    /// MEMORYSTATUSEX.
    #[repr(C)]
    #[derive(Default)]
    struct Status {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut Status) -> i32;
    }
    let mut status = Status {
        length: size_of::<Status>() as u32,
        ..Default::default()
    };
    // SAFETY: the call fills the structure whose size it is given.
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some(status.total_phys)
}

#[cfg(target_os = "macos")]
fn physical_memory() -> Option<u64> {
    let mut memory = 0u64;
    let mut size = size_of::<u64>();
    // SAFETY: "hw.memsize" is a 64-bit integer, written into `memory`
    // whose size is given.
    let ok = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut memory).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (ok == 0).then_some(memory)
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn physical_memory() -> Option<u64> {
    None
}

/// Decoder memory limit: what the largest accepted image needs in 16-bit RGBA.
fn limits() -> Limits {
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(MAX_DIMENSION as u64 * MAX_DIMENSION as u64 * 8);
    limits
}

/// What a document's file holds beside the pixels and layers, given back
/// when the document is saved in that format again.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FileMetadata {
    /// An image's EXIF (camera, date, copyright…), orientation reset.
    pub exif: Option<std::sync::Arc<Vec<u8>>>,
    /// A Photoshop document's image resources Atelier doesn't use
    /// (resolution, XMP, IPTC, EXIF, print settings…).
    pub psd_resources: std::sync::Arc<Vec<PsdResource>>,
    /// An image's resolution, XMP, IPTC, comments… in its format.
    pub image: Option<std::sync::Arc<ImageMetadata>>,
}

/// An image resource of a Photoshop document, kept as it was.
#[derive(Clone, Debug, PartialEq)]
pub struct PsdResource {
    pub id: u16,
    /// Its name, Mac OS Roman (usually empty).
    pub name: Vec<u8>,
    pub data: Vec<u8>,
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
    /// Its EXIF, orientation reset (the pixels are the right way up).
    pub exif: Option<Vec<u8>>,
    /// What else it holds that saving in its format writes back.
    pub metadata: ImageMetadata,
    /// What the image lost on opening, that saving over the file in its
    /// format would lose (translated notes).
    pub losses: Vec<String>,
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
    let original = decoder.original_color_type();
    let mut exif = decoder.exif_metadata().ok().flatten();
    if let Some(exif) = &mut exif {
        let _ = image::metadata::Orientation::remove_from_exif_chunk(exif);
    }
    let xmp = decoder.xmp_metadata().ok().flatten().is_some()
        || decoder.iptc_metadata().ok().flatten().is_some();
    let mut image = DynamicImage::from_decoder(decoder)?;
    let color = image.color();
    let cmyk = matches!(
        original,
        ExtendedColorType::Cmyk8 | ExtendedColorType::Cmyk16
    ) || format == Some(ImageFormat::Jpeg) && jpeg_is_cmyk(path)
        || (icc_profile.as_ref())
            .and_then(|icc| crate::cms::Profile::from_icc(std::sync::Arc::new(icc.clone())))
            .is_some_and(|p| p.space() == crate::cms::Space::Cmyk);
    let icc_profile = super::color::rgb_pixels(path, format, &mut image, icc_profile);
    image.apply_orientation(orientation);
    let animated = format.is_some_and(|f| is_animated(path, f));
    let mut losses = Vec::new();
    if animated {
        losses.push(t!("files4-note-frames"));
    }
    if format == Some(ImageFormat::Tiff) && tiff_has_more(path) {
        losses.push(t!("files4-note-pages"));
    }
    let bits = color.bits_per_pixel() / u16::from(color.channel_count().max(1));
    if bits > 8 {
        losses.push(t!("xchg-psd-note-depth", bits = bits));
    }
    if cmyk {
        let mode = t!("xchg-psd-mode-cmyk");
        losses.push(t!("xchg-psd-note-mode", mode = mode));
    }
    // What saving in the format writes back: XMP and IPTC as they were
    // in a JPEG, PNG or TIFF; not a TIFF's EXIF (a directory of its own).
    let (metadata, unkept) = metadata::read(path, format);
    let carried = match format {
        Some(ImageFormat::Jpeg | ImageFormat::Png) => !metadata.raw.is_empty(),
        Some(ImageFormat::Tiff) => !metadata.tiff.is_empty(),
        _ => false,
    };
    if format == Some(ImageFormat::Tiff) {
        exif = None;
    }
    if unkept.images {
        losses.push(t!("files5-note-images"));
    }
    if xmp && !carried || unkept.xmp {
        losses.push(t!("files5-note-xmp"));
    }
    if unkept.exif {
        losses.push(t!("files5-note-exif"));
    }
    if unkept.photoshop {
        losses.push(t!("files5-note-other-apps"));
    }
    Ok(Decoded {
        image: image.into_rgba8(),
        icc_profile,
        animated,
        exif,
        metadata,
        losses,
    })
}

/// A JPEG stores inks (four components: CMYK or YCCK), with a profile or
/// not (`image` gives them as RGB).
fn jpeg_is_cmyk(path: &Path) -> bool {
    use zune_core::colorspace::ColorSpace;
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    let mut headers = zune_jpeg::JpegDecoder::new(zune_core::bytestream::ZCursor::new(&data));
    headers.decode_headers().is_ok()
        && matches!(
            headers.input_colorspace(),
            Some(ColorSpace::CMYK | ColorSpace::YCCK)
        )
}

/// A TIFF holds pages after its first.
fn tiff_has_more(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    tiff::decoder::Decoder::new(BufReader::new(file)).is_ok_and(|d| d.more_images())
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
    pub fn max_side(self) -> u32 {
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
        encode_image(image, out, format, quality, icc_profile, None, None, None)
    })
}

/// Encodes `image` in `format` into `out` (see [`export`]), with a
/// copyright notice where the format has room for it (not GIF), else
/// with `exif` (PNG, JPEG, WebP), its size made the image's; and `kept`,
/// the metadata of the file it replaces when it is of that format.
#[allow(clippy::too_many_arguments)]
pub fn encode_image(
    image: &RgbaImage,
    out: &mut (impl Write + Seek),
    format: ExportFormat,
    quality: u8,
    icc_profile: Option<&[u8]>,
    copyright: Option<&str>,
    exif: Option<&[u8]>,
    kept: Option<&ImageMetadata>,
) -> Result<(), FileError> {
    let (w, h) = image.dimensions();
    if w.max(h) > format.max_side() {
        return Err(FileError::FormatTooSmall(format, format.max_side()));
    }
    let kept = kept.filter(|k| k.format == Some(format));
    // TIFF: its own Copyright tag (its encoder takes no EXIF).
    if format == ExportFormat::Tiff {
        return tiff(image, out, icc_profile, copyright, kept);
    }
    let exif = exif.map(|e| metadata::fresh_exif(e, (w, h)));
    let copyright = copyright.map(crate::export::copyright_exif);
    let meta = (icc_profile, copyright.as_deref().or(exif.as_deref()));
    // Written whole, then the kept metadata put in.
    if let Some(kept) = kept.filter(|_| matches!(format, ExportFormat::Jpeg | ExportFormat::Png)) {
        let mut data = std::io::Cursor::new(Vec::new());
        if format == ExportFormat::Jpeg {
            let mut encoder = JpegEncoder::new_with_quality(&mut data, quality.clamp(1, 100));
            if let Some(density) = kept.jpeg_density() {
                encoder.set_pixel_density(density);
            }
            let rgb = flatten_on_white(image);
            encode_with(encoder, rgb.as_raw(), (w, h), ExtendedColorType::Rgb8, meta)?;
        } else {
            encode(PngEncoder::new(&mut data), image, meta)?;
        }
        out.write_all(&kept.put_back(format, data.into_inner()))?;
        return Ok(());
    }
    match format {
        ExportFormat::Png => encode(PngEncoder::new(out), image, meta),
        ExportFormat::Jpeg => {
            let encoder = JpegEncoder::new_with_quality(out, quality.clamp(1, 100));
            let rgb = flatten_on_white(image);
            encode_with(encoder, rgb.as_raw(), (w, h), ExtendedColorType::Rgb8, meta)
        }
        ExportFormat::WebP => encode(WebPEncoder::new_lossless(out), image, meta),
        ExportFormat::Gif => {
            // Speed 10: the quantizer's usual compromise.
            let mut encoder = GifEncoder::new_with_speed(out, 10);
            encoder.encode(image.as_raw(), w, h, ExtendedColorType::Rgba8)?;
            Ok(())
        }
        ExportFormat::Tiff => unreachable!("written by `tiff`"),
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

/// An LZW-compressed TIFF with its profile and a Copyright tag if given:
/// RGB when opaque, else RGBA with its fourth sample said to be the
/// transparency (other applications would take it for an extra channel).
fn tiff(
    image: &RgbaImage,
    out: &mut (impl Write + Seek),
    icc: Option<&[u8]>,
    copyright: Option<&str>,
    kept: Option<&ImageMetadata>,
) -> Result<(), FileError> {
    use tiff::encoder::{Compression, TiffEncoder, colortype};
    use tiff::tags::{ExtraSamples, Tag};
    let failed = |e: tiff::TiffError| FileError::Io(std::io::Error::other(e.to_string()));
    let mut encoder = TiffEncoder::new(out)
        .map_err(failed)?
        .with_compression(Compression::Lzw);
    let (w, h) = image.dimensions();
    macro_rules! write {
        ($kind:ty, $data:expr, $extra:expr) => {{
            let mut file = encoder.new_image::<$kind>(w, h).map_err(failed)?;
            if let Some(extra) = $extra {
                let extra: &[u16] = &[ExtraSamples::to_u16(&extra)];
                file.encoder()
                    .write_tag(Tag::ExtraSamples, extra)
                    .map_err(failed)?;
            }
            if let Some(text) = copyright {
                tiff_copyright(file.encoder(), text).map_err(failed)?;
            }
            if let Some(kept) = kept {
                kept.write_tiff(file.encoder(), copyright.is_some())
                    .map_err(failed)?;
            }
            if let Some(icc) = icc {
                file.encoder()
                    .write_tag(Tag::IccProfile, icc)
                    .map_err(failed)?;
            }
            file.write_data($data).map_err(failed)
        }};
    }
    if image.pixels().all(|p| p.0[3] == 255) {
        let rgb = DynamicImage::ImageRgba8(image.clone()).into_rgb8();
        write!(colortype::RGB8, rgb.as_raw(), None::<ExtraSamples>)
    } else {
        write!(
            colortype::RGBA8,
            image.as_raw(),
            Some(ExtraSamples::UnassociatedAlpha)
        )
    }
}

/// A TIFF's Copyright tag, in UTF-8 as readers expect today (the crate
/// only writes ASCII).
fn tiff_copyright<W: Write + Seek, K: tiff::encoder::TiffKind>(
    encoder: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
    copyright: &str,
) -> tiff::TiffResult<()> {
    let mut text = crate::export::copyright_text(copyright).into_bytes();
    text.push(0);
    let entry = encoder.write_entry_bytes(tiff::tags::Type::ASCII, &text)?;
    let mut tags = tiff::Directory::empty();
    tags.extend([(tiff::tags::Tag::Copyright, entry)]);
    encoder.extend_from(&tags);
    Ok(())
}

/// The JPEG quantization table of Annex K (luminance) at `quality`, scaled
/// as libjpeg does, for the four inks of a CMYK JPEG.
fn cmyk_quantization(quality: u8) -> [u16; 64] {
    const ANNEX_K: [u16; 64] = [
        16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69,
        56, 14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81,
        104, 113, 92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
    ];
    let quality = u32::from(quality.clamp(1, 100));
    let scale = if quality < 50 {
        5000 / quality
    } else {
        200 - quality * 2
    };
    ANNEX_K.map(|v| ((u32::from(v) * scale + 50) / 100).clamp(1, 255) as u16)
}

/// Encodes CMYK pixels (4 bytes each, 0 = no ink) of `size` into `out`
/// as a JPEG (Adobe's marker, the inks inverted as Photoshop writes them)
/// or an LZW-compressed TIFF, with their profile and a copyright notice
/// if given.
pub fn encode_cmyk(
    cmyk: &[u8],
    (w, h): (u32, u32),
    out: &mut (impl Write + Seek),
    format: ExportFormat,
    quality: u8,
    icc: Option<&[u8]>,
    copyright: Option<&str>,
) -> Result<(), FileError> {
    if w.max(h) > format.max_side() {
        return Err(FileError::FormatTooSmall(format, format.max_side()));
    }
    let invalid = |e: String| FileError::Io(std::io::Error::other(e));
    match format {
        ExportFormat::Jpeg => {
            let quality = quality.clamp(1, 100);
            let mut encoder = jpeg_encoder::Encoder::new(out, quality);
            // The encoder treats cyan, magenta and yellow as chroma: below
            // quality 90 they would be stored at half resolution (one pixel
            // of each 2×2 block kept) and quantized more coarsely. Every ink
            // carries detail, so all four are kept whole, with one table.
            encoder.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
            let table = cmyk_quantization(quality);
            encoder.set_quantization_tables(
                jpeg_encoder::QuantizationTableType::Custom(Box::new(table)),
                jpeg_encoder::QuantizationTableType::Custom(Box::new(table)),
            );
            if let Some(icc) = icc {
                encoder
                    .add_icc_profile(icc)
                    .map_err(|e| invalid(e.to_string()))?;
            }
            if let Some(text) = copyright {
                let exif = [b"Exif\0\0".as_slice(), &crate::export::copyright_exif(text)].concat();
                encoder
                    .add_app_segment(1, exif)
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
            if let Some(text) = copyright {
                tiff_copyright(image.encoder(), text).map_err(tiff)?;
            }
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
        encode_image(
            &image,
            &mut out,
            ExportFormat::Tiff,
            90,
            None,
            Some(text),
            None,
            None,
        )
        .unwrap();
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

    /// The EXIF kept for saving over the file has its orientation reset
    /// (the pixels are already the right way up).
    #[test]
    fn exif_is_kept_without_its_orientation() {
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 90);
        let exif: Vec<u8> = [
            b"MM\0\x2a\0\0\0\x08".as_slice(),
            &[0, 1],
            &[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0],
            &[0, 0, 0, 0],
        ]
        .concat();
        encoder.set_exif_metadata(exif.clone()).unwrap();
        encoder
            .write_image(&[200; 4 * 2 * 3], 4, 2, ExtendedColorType::Rgb8)
            .unwrap();
        let path = dir("exif-kept").join("phone.jpg");
        std::fs::write(&path, jpeg).unwrap();
        let decoded = read_image(&path).unwrap();
        let kept = decoded.exif.expect("kept");
        assert_eq!(kept.len(), exif.len());
        assert_eq!(
            image::metadata::Orientation::from_exif_chunk(&kept),
            Some(image::metadata::Orientation::NoTransforms)
        );
        assert!(decoded.losses.is_empty(), "{:?}", decoded.losses);
    }

    /// What saving an image over its file in its format would lose is
    /// known on opening: other frames or pages, depth, inks, metadata.
    #[test]
    fn what_an_image_loses_is_known_on_opening() {
        let dir = dir("losses");
        let losses = |name: &str| read_image(&dir.join(name)).unwrap().losses;
        // Plain: nothing.
        RgbaImage::new(2, 2).save(dir.join("plain.png")).unwrap();
        assert!(losses("plain.png").is_empty());
        // Animated GIF.
        let mut encoder =
            image::codecs::gif::GifEncoder::new(File::create(dir.join("a.gif")).unwrap());
        for v in [10, 20, 30] {
            let frame = image::Frame::new(RgbaImage::from_pixel(4, 4, image::Rgba([v, v, v, 255])));
            encoder.encode_frame(frame).unwrap();
        }
        drop(encoder);
        assert_eq!(losses("a.gif"), [t!("files4-note-frames")]);
        // 16 bits.
        image::ImageBuffer::<image::Rgba<u16>, _>::from_pixel(2, 2, image::Rgba([9000; 4]))
            .save(dir.join("deep.png"))
            .unwrap();
        assert_eq!(losses("deep.png"), [t!("xchg-psd-note-depth", bits = 16)]);
        // A TIFF of two pages, with a copyright.
        {
            use tiff::encoder::{TiffEncoder, colortype::RGB8};
            let mut out = TiffEncoder::new(File::create(dir.join("pages.tif")).unwrap()).unwrap();
            let mut first = out.new_image::<RGB8>(2, 2).unwrap();
            tiff_copyright(first.encoder(), "QA").unwrap();
            first.write_data(&[5; 12]).unwrap();
            out.write_image::<RGB8>(2, 2, &[6; 12]).unwrap();
        }
        // Its copyright is written back: only the pages are lost.
        assert_eq!(losses("pages.tif"), [t!("files4-note-pages")]);
        // An untagged CMYK JPEG (`image` reads it as RGB).
        let mut cmyk = Vec::new();
        jpeg_encoder::Encoder::new(&mut cmyk, 90)
            .encode(&[30; 4 * 8 * 8], 8, 8, jpeg_encoder::ColorType::Cmyk)
            .unwrap();
        std::fs::write(dir.join("cmyk.jpg"), cmyk).unwrap();
        let mode = t!("xchg-psd-mode-cmyk");
        assert_eq!(losses("cmyk.jpg"), [t!("xchg-psd-note-mode", mode = mode)]);
    }

    /// Saved over in its format (Save and Close), an image keeps its
    /// resolution, XMP, IPTC and copyright; its EXIF says its new size.
    #[test]
    fn saving_in_place_keeps_the_metadata() {
        let dir = dir("in-place");
        let image = RgbaImage::from_pixel(6, 4, image::Rgba([90, 120, 150, 255]));
        // JPEG at 300 dpi, with XMP.
        let mut jpeg = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut jpeg, 90);
        encoder.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(300));
        encoder
            .write_image(&[50; 3 * 6 * 4], 6, 4, ExtendedColorType::Rgb8)
            .unwrap();
        let xmp = b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta>QA rights</x:xmpmeta>";
        let segment = [
            &[0xFF, 0xE1][..],
            &((xmp.len() + 2) as u16).to_be_bytes(),
            xmp,
        ]
        .concat();
        jpeg.splice(2..2, segment);
        // PNG at 300 ppi with an XMP chunk; TIFF at 300 dpi, copyright.
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let png = ImageMetadata {
            format: Some(ExportFormat::Png),
            density: Some((300.0, 300.0)),
            raw: vec![metadata::tests_chunk(
                b"iTXt",
                b"XML:com.adobe.xmp\0\0\0\0\0QA rights",
            )],
            ..ImageMetadata::default()
        }
        .put_back(ExportFormat::Png, png);
        let tiff_meta = ImageMetadata {
            format: Some(ExportFormat::Tiff),
            density: Some((300.0, 300.0)),
            ..ImageMetadata::default()
        };
        let mut tif = std::io::Cursor::new(Vec::new());
        tiff(&image, &mut tif, None, Some("QA rights"), Some(&tiff_meta)).unwrap();
        for (name, data, format) in [
            ("d.jpg", jpeg, ExportFormat::Jpeg),
            ("d.png", png, ExportFormat::Png),
            ("d.tif", tif.into_inner(), ExportFormat::Tiff),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, data).unwrap();
            let opened = read_image(&path).unwrap();
            assert!(opened.losses.is_empty(), "{name}: {:?}", opened.losses);
            let mut out = std::io::Cursor::new(Vec::new());
            let kept = Some(&opened.metadata);
            encode_image(&image, &mut out, format, 90, None, None, None, kept).unwrap();
            let saved = dir.join(format!("saved-{name}"));
            std::fs::write(&saved, out.into_inner()).unwrap();
            let back = read_image(&saved).unwrap();
            let (x, y) = back.metadata.density.expect(name);
            assert!(
                (x - 300.0).abs() < 0.5 && (y - 300.0).abs() < 0.5,
                "{name}: {x} {y}"
            );
            let bytes = std::fs::read(&saved).unwrap();
            assert!(bytes.windows(9).any(|w| w == b"QA rights"), "{name}");
            assert_eq!(back.image.dimensions(), (6, 4));
        }
    }

    /// A TIFF says what its fourth sample is: the transparency (other
    /// applications would show an extra channel); opaque, it has three.
    #[test]
    fn tiff_transparency_is_said_to_be_one() {
        use tiff::tags::Tag;
        let samples = |image: &RgbaImage, copyright: Option<&str>| {
            let mut out = std::io::Cursor::new(Vec::new());
            encode_image(
                image,
                &mut out,
                ExportFormat::Tiff,
                90,
                None,
                copyright,
                None,
                None,
            )
            .unwrap();
            let bytes = out.into_inner();
            let mut d = tiff::decoder::Decoder::new(std::io::Cursor::new(&bytes)).unwrap();
            let n = d.get_tag_u32(Tag::SamplesPerPixel).unwrap();
            let extra = d
                .find_tag(Tag::ExtraSamples)
                .unwrap()
                .map(|v| v.into_u32().unwrap());
            let back = image::load_from_memory(&bytes).unwrap().into_rgba8();
            assert_eq!(back, *image);
            (n, extra)
        };
        let opaque = RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]));
        let clear = RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 100]));
        for copyright in [None, Some("QA")] {
            assert_eq!(samples(&opaque, copyright), (3, None));
            assert_eq!(samples(&clear, copyright), (4, Some(2)));
        }
    }

    /// CMYK files carry the copyright too.
    #[test]
    fn cmyk_files_carry_the_copyright() {
        for format in [ExportFormat::Jpeg, ExportFormat::Tiff] {
            let mut out = std::io::Cursor::new(Vec::new());
            let ink = [10u8; 4 * 8 * 8];
            encode_cmyk(
                &ink,
                (8, 8),
                &mut out,
                format,
                80,
                None,
                Some("QA-Copyright-2026"),
            )
            .unwrap();
            let bytes = out.into_inner();
            assert!(
                bytes.windows(17).any(|w| w == b"QA-Copyright-2026"),
                "{format:?}"
            );
        }
    }
}
