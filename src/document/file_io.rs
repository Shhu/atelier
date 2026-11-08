//! Image files: decoding (content-detected format, EXIF orientation, size
//! limits), Export As (PNG, JPEG, WebP, GIF, TIFF) and writes that never
//! destroy the file they replace.

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};

use image::codecs::gif::GifEncoder;
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::codecs::{jpeg::JpegEncoder, png::PngDecoder, png::PngEncoder};
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError,
    ImageFormat, ImageReader, Limits, RgbaImage,
};

use crate::i18n::t;

mod metadata;
pub use metadata::ImageMetadata;
pub(super) use metadata::{fresh_exif, fresh_xmp};

/// Largest canvas side, in pixels.
pub const MAX_DIMENSION: u32 = 30_000;

/// Why a file could not be read or written.
#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    Image(ImageError),
    /// Wider or taller than [`MAX_DIMENSION`].
    TooLarge(u32, u32),
    /// Layers past the 4 GB a PSD's lengths count (a PSB holds them).
    PsdOver4Gb,
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
    /// Opening the file would take more memory than is free now (what is
    /// free, in bytes): it may be opened anyway ([`MemoryUse::Anyway`]).
    LowMemory(u64),
    /// Smart objects nested deeper than Atelier reads (the limit).
    NestedTooDeep(u32),
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
        let text = match self {
            FileError::Io(e) => crate::i18n::io_error(e),
            FileError::Image(ImageError::Unsupported(_)) => t!("file-error-unsupported"),
            FileError::Image(ImageError::Decoding(_)) => t!("file-error-damaged"),
            FileError::Image(e) => e.to_string(),
            FileError::TooLarge(w, h) => t!(
                "file-error-too-large",
                width = *w,
                height = *h,
                max = MAX_DIMENSION
            ),
            FileError::PsdOver4Gb => t!("psd14-error-psd-over-4gb"),
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
            FileError::LowMemory(free) => t!(
                "files6-error-low-memory",
                free = (*free as f64 / f64::from(1 << 30)).round().max(1.0)
            ),
            FileError::NestedTooDeep(max) => t!("readers14-error-nested", max = *max),
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

/// The memory a document being opened may take while it is read, all its
/// layers, masks, channels and smart objects together, with what decoding
/// them holds at once. Each part proves it can hold its pixels (its data
/// can't be inflated more than deflate allows), and no stored byte is read
/// twice, so only honest files reach these limits: past a share of the
/// memory free now ([`Budget::new`]) opening asks first
/// ([`FileError::LowMemory`]); past all the computer can hold
/// ([`Budget::anyway`]) it is refused rather than aborting the application.
pub(crate) struct Budget {
    left: u64,
    memory: u64,
    /// Out of what is free (asked), not out of all the computer holds.
    free: bool,
}

impl Budget {
    /// Three fifths of the memory free now: the other programs and tabs,
    /// the undo history and the display copies need the rest.
    pub(crate) fn new() -> Self {
        let memory = system_memory();
        match memory.available {
            Some(free) => Self {
                left: (free / 5 * 3).min(memory.total),
                memory: free,
                free: true,
            },
            None => Self::of(memory.total),
        }
    }

    /// All the computer can hold, its memory and swap (opening anyway).
    pub(crate) fn anyway() -> Self {
        Self::of(system_memory().total)
    }

    /// A budget of `memory` bytes.
    pub(crate) fn of(memory: u64) -> Self {
        Self {
            left: memory,
            memory,
            free: false,
        }
    }

    /// What can still be taken.
    pub(crate) fn left(&self) -> u64 {
        self.left
    }

    /// Takes `w` × `h` pixels of `bpp` bytes from the budget.
    pub(crate) fn take(&mut self, w: u32, h: u32, bpp: u64) -> Result<(), FileError> {
        self.take_bytes(
            u64::from(w)
                .saturating_mul(u64::from(h))
                .saturating_mul(bpp),
        )
    }

    pub(crate) fn take_bytes(&mut self, bytes: u64) -> Result<(), FileError> {
        match self.left.checked_sub(bytes) {
            Some(left) => {
                self.left = left;
                Ok(())
            }
            None if self.free => Err(FileError::LowMemory(self.memory)),
            None => Err(FileError::OutOfMemory(self.memory)),
        }
    }
}

/// How much memory a document being opened may take (see [`Budget`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryUse {
    /// A share of what is free now; asked past it.
    Free,
    /// All the computer can hold: the user agreed to open it anyway.
    Anyway,
}

impl MemoryUse {
    pub(crate) fn budget(self) -> Budget {
        match self {
            MemoryUse::Free => Budget::new(),
            MemoryUse::Anyway => Budget::anyway(),
        }
    }
}

/// The computer's memory, in bytes.
struct SystemMemory {
    /// Memory and swap (or page file): all it can hold.
    total: u64,
    /// Memory free now, other programs' caches included.
    available: Option<u64>,
}

/// [`SystemMemory`]; 16 GB when it can't be told.
fn system_memory() -> SystemMemory {
    #[cfg_attr(not(test), allow(unused_mut))]
    let mut memory = memory_info().unwrap_or(SystemMemory {
        total: 16 << 30,
        available: None,
    });
    #[cfg(test)]
    if let Some(free) = TEST_FREE_MEMORY.get() {
        memory.available = Some(free);
    }
    memory
}

#[cfg(test)]
thread_local! {
    static TEST_FREE_MEMORY: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Tests: the memory said to be free on this thread (`None`: the real).
#[cfg(test)]
pub fn set_test_free_memory(free: Option<u64>) {
    TEST_FREE_MEMORY.set(free);
}

#[cfg(target_os = "linux")]
fn memory_info() -> Option<SystemMemory> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = info.lines().find(|l| l.starts_with(name))?;
        let kb: u64 = line[name.len()..]
            .trim()
            .trim_end_matches("kB")
            .trim()
            .parse()
            .ok()?;
        Some(kb * 1024)
    };
    let total = field("MemTotal:")?;
    Some(SystemMemory {
        total: total + field("SwapTotal:").unwrap_or(0),
        available: field("MemAvailable:"),
    })
}

#[cfg(windows)]
fn memory_info() -> Option<SystemMemory> {
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
    // The commit limit: memory and page file.
    (ok != 0).then_some(SystemMemory {
        total: status.total_page_file.max(status.total_phys),
        available: Some(status.avail_phys),
    })
}

#[cfg(target_os = "macos")]
fn memory_info() -> Option<SystemMemory> {
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
    // Swap grows as needed: the memory alone bounds it.
    (ok == 0).then_some(SystemMemory {
        total: memory,
        available: None,
    })
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn memory_info() -> Option<SystemMemory> {
    None
}

/// Decoder memory limit: `room`, what was taken from the budget for the
/// image (a frame larger than the image it is in can't take more).
fn limits(room: u64) -> Limits {
    let mut limits = Limits::no_limits();
    limits.max_alloc = Some(room);
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
    /// The canvas and alpha channels `psd_resources` describe.
    pub psd_layout: Option<PsdLayout>,
}

/// What a Photoshop document's kept resources were made for: those placed
/// on the canvas (slices, color samplers, Count marks) go back only on a
/// canvas of the same size, the channels' display settings (color,
/// opacity, what they show) to the channels of the same name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PsdLayout {
    pub canvas: (u32, u32),
    /// The alpha channels, in the order of the display settings (1077).
    pub channels: Vec<String>,
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
/// a .jfif JPEG…), its pixels taken from `budget`.
pub fn read_image(path: &Path, budget: &mut Budget) -> Result<Decoded, FileError> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    // Dimensions first (header only): too large is refused with its size,
    // too large for the memory asked about, before anything is decoded.
    reader.no_limits();
    let mut decoder = reader.into_decoder()?;
    let (w, h) = decoder.dimensions();
    check_size(w, h)?;
    // The decoded pixels, a copy of them converted or turned, and the
    // 8-bit RGBA image made of them.
    let decoded = u64::from(decoder.color_type().bytes_per_pixel());
    budget.take(w, h, 2 * decoded + 4)?;
    decoder.set_limits(limits(u64::from(w) * u64::from(h) * (2 * decoded + 4)))?;
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

/// Preview of a file Atelier opens, fitting in `width` × `height`; none
/// when making it would take more memory than is free ([`Budget::new`]).
pub fn file_thumbnail(path: &Path, width: u32, height: u32) -> Option<RgbaImage> {
    // A decoder panicking on a damaged file: no thumbnail.
    std::panic::catch_unwind(|| thumbnail_unguarded(path, width, height)).ok()?
}

fn thumbnail_unguarded(path: &Path, width: u32, height: u32) -> Option<RgbaImage> {
    let budget = &mut Budget::new();
    let image = if super::is_native(path) {
        super::native::thumbnail_of(path, budget)?
    } else if super::is_psd(path) {
        super::psd::thumbnail_of(path, budget)?
    } else if crate::camera_raw::raw::is_raw(path) {
        crate::camera_raw::raw::thumbnail(path, budget)?
    } else {
        read_image(path, budget).ok()?.image
    };
    Some(
        DynamicImage::ImageRgba8(image)
            .thumbnail(width, height)
            .into_rgba8(),
    )
}

/// An image held in memory (a document's thumbnail), decoded if its
/// pixels fit in `budget`.
pub(super) fn decode_within(
    data: &[u8],
    format: ImageFormat,
    budget: &mut Budget,
) -> Option<RgbaImage> {
    let reader = ImageReader::with_format(std::io::Cursor::new(data), format);
    let mut decoder = reader.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    check_size(w, h).ok()?;
    let decoded = u64::from(decoder.color_type().bytes_per_pixel());
    budget.take(w, h, decoded + 4).ok()?;
    (decoder.set_limits(limits(u64::from(w) * u64::from(h) * (decoded + 4)))).ok()?;
    Some(DynamicImage::from_decoder(decoder).ok()?.into_rgba8())
}

/// More than one frame (GIF, APNG, animated WebP), from the headers only.
fn is_animated(path: &Path, format: ImageFormat) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    match format {
        ImageFormat::Gif => gif_frames(&mut reader) > 1,
        ImageFormat::Png => PngDecoder::new(reader).is_ok_and(|d| d.is_apng().unwrap_or(false)),
        ImageFormat::WebP => WebPDecoder::new(reader).is_ok_and(|d| d.has_animation()),
        _ => false,
    }
}

/// Images in a GIF, up to 2, counted from its blocks without decoding
/// them (a frame's declared size could ask for gigabytes).
fn gif_frames(r: &mut impl Read) -> u32 {
    let mut images = 0;
    let _ = gif_count(r, &mut images);
    images
}

fn gif_count(r: &mut impl Read, images: &mut u32) -> std::io::Result<()> {
    // A color table: 3 bytes per entry, 2^(size + 1) entries.
    let table = |flags: u8| if flags & 0x80 != 0 { 3u64 << ((flags & 7) + 1) } else { 0 };
    let mut header = [0u8; 13];
    r.read_exact(&mut header)?;
    skip(r, table(header[10]))?;
    while *images < 2 {
        let mut kind = [0u8];
        r.read_exact(&mut kind)?;
        match kind[0] {
            // Image descriptor: position, size, flags; then its color
            // table, the LZW code size and the data.
            0x2C => {
                *images += 1;
                let mut descriptor = [0u8; 9];
                r.read_exact(&mut descriptor)?;
                skip(r, table(descriptor[8]) + 1)?;
                skip_blocks(r)?;
            }
            // Extension: its label, then its blocks.
            0x21 => {
                skip(r, 1)?;
                skip_blocks(r)?;
            }
            // The trailer, or what can't be read further.
            _ => break,
        }
    }
    Ok(())
}

/// Passes `n` bytes.
fn skip(r: &mut impl Read, n: u64) -> std::io::Result<()> {
    match std::io::copy(&mut r.by_ref().take(n), &mut std::io::sink())? == n {
        true => Ok(()),
        false => Err(std::io::ErrorKind::UnexpectedEof.into()),
    }
}

/// Passes a GIF's data sub-blocks: a length, that many bytes; 0 ends them.
fn skip_blocks(r: &mut impl Read) -> std::io::Result<()> {
    loop {
        let mut len = [0u8];
        r.read_exact(&mut len)?;
        if len[0] == 0 {
            return Ok(());
        }
        skip(r, u64::from(len[0]))?;
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
    // Its XMP made true for the image saved.
    let kept = kept
        .filter(|k| k.format == Some(format))
        .map(|k| k.for_size((w, h)));
    let kept = kept.as_ref();
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
    // The EXIF and GPS directories first: the image's points to them.
    let dirs = match kept {
        Some(kept) => kept.write_tiff_dirs(&mut encoder, (w, h)).map_err(failed)?,
        None => Vec::new(),
    };
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
                kept.write_tiff(file.encoder(), copyright.is_some(), &dirs)
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
    let existing = std::fs::metadata(&target).ok();
    // A read-only file stays as it is (the rename would replace it).
    if existing.as_ref().is_some_and(|m| m.permissions().readonly()) {
        return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into());
    }
    remove_stale_temporaries(&target);
    let (file, tmp) = create_temporary(&target)?;
    let result = (|| {
        // The file's permissions before its contents: never readable by
        // more people than the one it replaces, even for a moment.
        #[cfg(unix)]
        if let Some(meta) = &existing {
            file.set_permissions(meta.permissions())?;
        }
        // Held while written: no other window takes it for one a crash left.
        let _ = file.try_lock();
        let mut out = BufWriter::new(file);
        write(&mut out)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        rename_retrying(&tmp, &target)?;
        // The rename itself survives a power cut.
        #[cfg(unix)]
        if let Some(dir) = target.parent().and_then(|d| File::open(d).ok()) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// `std::fs::rename`, tried again for a moment on Windows: an antivirus or
/// an indexer often holds a file that was just written.
pub fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if cfg!(windows) && tries < 8 && e.kind() != std::io::ErrorKind::NotFound => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(50 * tries));
            }
            result => return result.map_err(|e| held_open(e, to)),
        }
    }
}

/// On Windows, replacing a file another program holds open fails as
/// "access denied", like a read-only one: said as "in use" when the file
/// isn't read-only.
fn held_open(e: std::io::Error, to: &Path) -> std::io::Error {
    let writable = || std::fs::metadata(to).is_ok_and(|m| !m.permissions().readonly());
    if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied && writable() {
        std::io::Error::new(std::io::ErrorKind::ResourceBusy, e)
    } else {
        e
    }
}

/// Prefix of the temporary files of [`write_atomic`] whose target's name
/// is too long to be part of theirs.
const SHORT_TEMPORARY: &str = ".atelier-";

/// A new temporary file next to `target`: ".name.1234-5.tmp" (1234: this
/// process, 5: this write, for writers on several threads), or
/// ".atelier-1234-5.tmp" when the target's name is long (a name of 255
/// bytes can be saved). Never one that exists.
fn create_temporary(target: &Path) -> std::io::Result<(File, PathBuf)> {
    loop {
        let tmp = temporary_path(target);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        match options.open(&tmp) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            result => return result.map(|file| (file, tmp)),
        }
    }
}

fn temporary_path(target: &Path) -> PathBuf {
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let suffix = format!(".{}-{n}.tmp", std::process::id());
    let name = target.file_name().unwrap_or_default();
    // 255 bytes at most on most file systems (UTF-16 units on Windows).
    let mut tmp = OsString::from(".");
    if name.len() + suffix.len() < 200 {
        tmp.push(name);
        tmp.push(suffix);
    } else {
        tmp = OsString::from(format!("{SHORT_TEMPORARY}{}", &suffix[1..]));
    }
    target.with_file_name(tmp)
}

/// Temporary files that writes to `target` (or long-named ones) left when
/// cut short (a crash, a kill, a power cut): removed once no one holds them
/// and they are a minute old, so that a write going on in another window
/// is never touched.
fn remove_stale_temporaries(target: &Path) {
    let (Some(dir), Some(name)) = (target.parent(), target.file_name()) else {
        return;
    };
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let own = format!(".{}.", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        let left = file
            .strip_suffix(".tmp")
            .and_then(|f| f.strip_prefix(&own).or_else(|| f.strip_prefix(SHORT_TEMPORARY)))
            .is_some_and(is_write_id);
        if left && is_stale(&entry.path()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// "1234-5": a process and a write.
fn is_write_id(id: &str) -> bool {
    id.split_once('-').is_some_and(|(pid, n)| {
        [pid, n]
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// A temporary file no one writes: unlocked, and untouched for a minute.
fn is_stale(path: &Path) -> bool {
    let old = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age.as_secs() >= 60));
    old && File::open(path).is_ok_and(|f| f.try_lock().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn dir(name: &str) -> PathBuf {
        crate::testing::test_dir(&format!("file-io-{name}"))
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

    /// Saving over a file another program holds open (without letting it
    /// be deleted, as image viewers do) says so, and keeps it.
    #[cfg(windows)]
    #[test]
    fn saving_over_a_file_held_open_says_it_is_in_use() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        let dir = dir("held-open");
        let path = dir.join("held.png");
        std::fs::write(&path, b"original").unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&path)
            .unwrap();
        let err = write_atomic(&path, |out| Ok(out.write_all(b"new")?)).unwrap_err();
        let FileError::Io(e) = &err else { panic!("{err:?}") };
        assert_eq!(e.kind(), std::io::ErrorKind::ResourceBusy);
        assert_eq!(err.to_string(), crate::i18n::t!("platforms14-file-error-in-use"));
        drop(held);
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn a_busy_file_is_said_in_use() {
        let busy = FileError::Io(std::io::Error::from(std::io::ErrorKind::ResourceBusy));
        assert_eq!(busy.to_string(), crate::i18n::t!("platforms14-file-error-in-use"));
        // Elsewhere, and for a read-only file, "access denied" stays.
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let missing = Path::new("/no/such/file");
        assert_eq!(held_open(denied, missing).kind(), std::io::ErrorKind::PermissionDenied);
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
    fn long_names_save_and_temporaries_never_replace_a_file() {
        let dir = dir("long-name");
        let long = dir.join(format!("{}.atelier", "a".repeat(240)));
        write_atomic(&long, |out| Ok(out.write_all(b"long")?)).unwrap();
        assert_eq!(std::fs::read(&long).unwrap(), b"long");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // A file already named as the next temporary is left as it is.
        let target = dir.join("photo.png");
        let next = temporary_path(&target);
        let taken = |n: u64| {
            let name = next.file_name().unwrap().to_string_lossy().into_owned();
            let (head, _) = name.rsplit_once('-').unwrap();
            dir.join(format!("{head}-{n}.tmp"))
        };
        let n: u64 = next.to_string_lossy().rsplit_once('-').unwrap().1
            .trim_end_matches(".tmp").parse().unwrap();
        for k in n + 1..n + 4 {
            std::fs::write(taken(k), b"someone's").unwrap();
        }
        write_atomic(&target, |out| Ok(out.write_all(b"new")?)).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        for k in n + 1..n + 4 {
            assert_eq!(std::fs::read(taken(k)).unwrap(), b"someone's");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn temporaries_left_by_a_crash_are_removed() {
        let dir = dir("stale-temporaries");
        let target = dir.join("photo.atelier");
        std::fs::write(&target, b"original").unwrap();
        let aged = |name: &str| {
            let path = dir.join(name);
            let file = File::create(&path).unwrap();
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
            file.set_modified(old).unwrap();
            path
        };
        let stale = aged(".photo.atelier.99999-3.tmp");
        let short = aged(".atelier-99999-4.tmp");
        // Another window's write going on: locked, kept.
        let busy = aged(".photo.atelier.99998-1.tmp");
        let held = File::options().write(true).open(&busy).unwrap();
        held.lock().unwrap();
        // Just begun (not a minute old), or not ours: kept.
        let fresh = dir.join(".photo.atelier.99997-2.tmp");
        std::fs::write(&fresh, b"").unwrap();
        let other = aged(".notes.txt.tmp");
        write_atomic(&target, |out| Ok(out.write_all(b"new")?)).unwrap();
        assert!(!stale.exists() && !short.exists());
        assert!(busy.exists() && fresh.exists() && other.exists());
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn permissions_come_before_the_contents() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dir("permissions-first");
        let target = dir.join("private.atelier");
        std::fs::write(&target, b"original").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&target, |out| {
            let mode = out.get_ref().metadata()?.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "readable by others while written");
            Ok(out.write_all(b"secret")?)
        })
        .unwrap();
        let mode = std::fs::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
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
            let decoded = read_image(&path, &mut Budget::new()).unwrap();
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
        assert_eq!(read_image(&png, &mut Budget::new()).unwrap().image.dimensions(), (5, 4));
        let jfif = dir.join("photo.jfif");
        image::RgbImage::new(6, 3)
            .save_with_format(&jfif, ImageFormat::Jpeg)
            .unwrap();
        assert_eq!(read_image(&jfif, &mut Budget::new()).unwrap().image.dimensions(), (6, 3));
        assert_eq!(image_size(&jfif), Some((6, 3)));
    }

    #[test]
    fn empty_and_oversized_images_are_refused() {
        let dir = dir("sizes");
        let empty = dir.join("empty.pnm");
        std::fs::write(&empty, b"P6\n0 0\n255\n").unwrap();
        assert!(matches!(read_image(&empty, &mut Budget::new()), Err(FileError::Empty)));
        let wide = dir.join("wide.png");
        image::GrayImage::new(MAX_DIMENSION + 1, 1)
            .save(&wide)
            .unwrap();
        assert!(matches!(
            read_image(&wide, &mut Budget::new()),
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
        let decoded = read_image(&path, &mut Budget::new()).unwrap();
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
        let decoded = read_image(&path, &mut Budget::new()).unwrap();
        assert!(decoded.animated);
        assert_eq!(decoded.image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        let still = dir.join("still.png");
        RgbaImage::new(2, 2).save(&still).unwrap();
        assert!(!read_image(&still, &mut Budget::new()).unwrap().animated);
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
        let decoded = read_image(&path, &mut Budget::new()).unwrap();
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
        let losses = |name: &str| read_image(&dir.join(name), &mut Budget::new()).unwrap().losses;
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
        // Said on opening too, as for a Photoshop document.
        assert_eq!(
            crate::document::Document::open(&dir.join("deep.png"))
                .unwrap()
                .notes,
            [t!("xchg-psd-note-depth", bits = 16)]
        );
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
            let opened = read_image(&path, &mut Budget::new()).unwrap();
            assert!(opened.losses.is_empty(), "{name}: {:?}", opened.losses);
            let mut out = std::io::Cursor::new(Vec::new());
            let kept = Some(&opened.metadata);
            encode_image(&image, &mut out, format, 90, None, None, None, kept).unwrap();
            let saved = dir.join(format!("saved-{name}"));
            std::fs::write(&saved, out.into_inner()).unwrap();
            let back = read_image(&saved, &mut Budget::new()).unwrap();
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

    /// Past three fifths of the memory free, opening asks first; past all
    /// the computer holds, it is refused.
    #[test]
    fn the_budget_asks_past_the_free_memory() {
        set_test_free_memory(Some(1000));
        let mut budget = Budget::new();
        assert!(budget.take_bytes(600).is_ok());
        assert!(matches!(
            budget.take_bytes(1),
            Err(FileError::LowMemory(1000))
        ));
        let mut anyway = MemoryUse::Anyway.budget();
        assert!(anyway.take_bytes(1 << 20).is_ok());
        set_test_free_memory(None);
        let mut small = Budget::of(10);
        assert!(matches!(
            small.take_bytes(11),
            Err(FileError::OutOfMemory(10))
        ));
    }

    /// Damaged JPEGs on which the JPEG decoder panicked (found by fuzzing):
    /// opening, placing or dropping one says it is damaged, the
    /// application stays.
    #[test]
    fn a_decoder_panic_is_a_damaged_file() {
        let dir = dir("decoder-panic");
        let files: [(&str, &[u8]); 3] = [
            ("avx2-246.jpg", include_bytes!("../../tests/fixtures/damaged/zune-avx2-246.jpg")),
            ("avx2-378.jpg", include_bytes!("../../tests/fixtures/damaged/zune-avx2-378.jpg")),
            ("scalar-37.jpg", include_bytes!("../../tests/fixtures/damaged/zune-scalar-37.jpg")),
        ];
        for (name, bytes) in files {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let opened = crate::document::Document::open(&path);
            assert!(opened.is_err(), "{name}");
            // The recent files' thumbnail and size neither.
            let _ = file_thumbnail(&path, 64, 64);
            let _ = file_size(&path);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// An image's pixels come from the memory budget, as a document's
    /// layers do: past what is free, opening asks (Open Anyway); its
    /// thumbnail is not made.
    #[test]
    fn images_ask_past_the_free_memory() {
        let dir = dir("image-budget");
        let path = dir.join("big.png");
        RgbaImage::from_pixel(100, 100, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        set_test_free_memory(Some(100_000));
        let open = |memory| crate::document::Document::open_within(&path, memory);
        assert!(matches!(open(MemoryUse::Free), Err(FileError::LowMemory(_))));
        assert!(file_thumbnail(&path, 64, 64).is_none());
        assert!(open(MemoryUse::Anyway).is_ok());
        set_test_free_memory(None);
        assert!(file_thumbnail(&path, 64, 64).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// GIFs whose headers claim more than they hold (fuzzing): a 15-byte
    /// file's 18000 × 14000 screen is asked about before anything is
    /// reserved; a second frame claiming gigabytes is counted without
    /// being decoded.
    #[test]
    fn gif_headers_are_not_believed() {
        let dir = dir("gif-claims");
        let tiny = dir.join("tiny.gif");
        std::fs::write(&tiny, b"GIF89aIF89a\x10\x10\x00\x0c").unwrap();
        set_test_free_memory(Some(512 << 20));
        assert!(matches!(
            crate::document::Document::open(&tiny),
            Err(FileError::LowMemory(_))
        ));
        set_test_free_memory(None);
        let second = dir.join("second.gif");
        let bytes: [u8; 79] = [
            0x47, 0x49, 0x46, 0x38, 0x39, 0x61, 0x10, 0x00, 0x0c, 0x00, 0x80, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x21, 0xf9, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x0c, 0x00, 0x80, 0x0a, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x02, 0x0c, 0x84, 0x8f, 0xa9, 0xcb, 0xed, 0x0f, 0xa3, 0x9c, 0xb4, 0xda, 0x6b,
            0x0a, 0x00, 0x21, 0xf9, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00, 0x2c, 0xa9, 0xcb, 0xed,
            0x0f, 0xa3, 0x9c, 0xb4, 0xda, 0x6b, 0x0a, 0x00, 0x3b,
        ];
        std::fs::write(&second, bytes).unwrap();
        let opened = read_image(&second, &mut Budget::new()).unwrap();
        assert!(opened.animated);
        // Counted from the blocks: one image, two, a damaged one.
        let still = dir.join("still.gif");
        RgbaImage::new(3, 2).save(&still).unwrap();
        let count = |data: &[u8]| gif_frames(&mut std::io::Cursor::new(data));
        assert_eq!(count(&std::fs::read(&still).unwrap()), 1);
        assert_eq!(count(&bytes), 2);
        assert_eq!(count(&bytes[..40]), 1);
        assert_eq!(count(b"GIF"), 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}
