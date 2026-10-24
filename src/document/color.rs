//! A document's colors: its profile (Edit › Assign Profile, Convert to
//! Profile), the soft proof it is shown with (View › Proof Setup), and its
//! CMYK files.

use std::path::Path;
use std::sync::Arc;

use image::{DynamicImage, ImageFormat};

use super::{Document, ExportFormat, FileError, Fill, Layer, LayerKind, file_io, next_rev, psd};
use crate::cms::{self, Conversion, Grayscale, Intent, Profile, Space};
use crate::compose::gradient::GradientFill;
use crate::i18n::t;
use crate::paint::gradient::StopColor;
use crate::pixel_ops::{Adjustment, ColorBalance};
use crate::vector::Paint;

/// View › Proof Setup: the printing condition the canvas simulates, and
/// whether it does (View › Proof Colors, Gamut Warning).
#[derive(Clone, Debug, PartialEq)]
pub struct ProofSetup {
    /// Device to simulate; `None`: the working CMYK.
    pub custom: Option<Profile>,
    pub intent: Intent,
    pub bpc: bool,
    /// Simulate Paper Color (and so its black too).
    pub paper: bool,
    /// Simulate Black Ink.
    pub ink: bool,
    /// View › Proof Colors (Ctrl+Y).
    pub colors: bool,
    /// View › Gamut Warning (Shift+Ctrl+Y).
    pub gamut: bool,
}

impl Default for ProofSetup {
    fn default() -> Self {
        Self {
            custom: None,
            intent: Intent::RelativeColorimetric,
            bpc: true,
            paper: false,
            ink: false,
            colors: false,
            gamut: false,
        }
    }
}

impl Document {
    /// The profile of the pixels: the document's own if it is an RGB one,
    /// otherwise `working`.
    pub fn profile_or(&self, working: &Profile) -> Profile {
        self.icc_profile
            .as_ref()
            .and_then(|icc| Profile::from_icc(Arc::clone(icc)))
            .filter(|p| p.space() == Space::Rgb)
            .unwrap_or_else(|| working.clone())
    }

    /// Edit › Assign Profile: the same numbers, read as `profile`'s
    /// (`None`: the working RGB's). One step of the history.
    pub fn assign_profile(&mut self, profile: Option<&Profile>) {
        self.checkpoint(t!("color-hist-assign-profile"));
        self.icc_profile = profile.map(|p| Arc::clone(p.icc()));
    }

    /// Edit › Convert to Profile: every layer's colors (pixels, fills,
    /// texts, shapes, styles; masks untouched) from `c.from` to `c.to`,
    /// which becomes the document's profile. One step of the history.
    pub fn convert_to_profile(&mut self, c: &Conversion) {
        self.checkpoint(t!("color-hist-convert-to-profile"));
        self.recolor(c);
        self.icc_profile = Some(Arc::clone(c.to.icc()));
    }

    /// Image › Mode › Grayscale: every layer's colors to grays of the same
    /// lightness (the pixels stay RGB). One step of the history.
    pub fn grayscale(&mut self, g: &Grayscale) {
        self.checkpoint(t!("color-hist-grayscale"));
        self.recolor(g);
    }

    fn recolor(&mut self, r: &dyn Recolor) {
        // Built once: a transform costs milliseconds, and a document has
        // a few colors per layer.
        let c = Prepared {
            run: r.recolorer(),
            profile: r.profile(),
            grays: r.grays(),
        };
        if let Some(real) = &mut self.swapped {
            convert_layer(real, &c);
        }
        for layer in &mut self.layers {
            convert_layer(layer, &c);
        }
    }

    /// Export As in CMYK (JPEG, TIFF, Photoshop document): the image
    /// converted by `cmyk` to its CMYK profile, embedded. The document stays
    /// as it is. Returns what a Photoshop document could not keep.
    pub fn export_cmyk(
        &self,
        path: &Path,
        format: ExportFormat,
        quality: u8,
        cmyk: &Conversion,
    ) -> Result<Vec<String>, FileError> {
        if format == ExportFormat::Psd {
            let mut notes = Vec::new();
            file_io::write_atomic(path, |out| {
                notes = psd::write_cmyk(self, out, cmyk)?;
                Ok(())
            })?;
            return Ok(notes);
        }
        let ink = to_cmyk(&self.flatten(), cmyk)?;
        let size = (self.width(), self.height());
        file_io::write_atomic(path, |out| {
            file_io::encode_cmyk(&ink, size, out, format, quality, Some(cmyk.to.icc()), None)
        })?;
        Ok(Vec::new())
    }
}

/// An image's CMYK pixels (flattened onto white: no transparency), 4
/// bytes each.
pub fn to_cmyk(image: &image::RgbaImage, cmyk: &Conversion) -> Result<Vec<u8>, FileError> {
    let rgb = file_io::flatten_on_white(image);
    cmyk.rgb_to_cmyk(rgb.as_raw()).ok_or(FileError::Damaged)
}

/// What changes every color of a document.
trait Recolor {
    fn recolorer(&self) -> cms::Recolorer;
    /// The profile the colors are then in, if it changes.
    fn profile(&self) -> Option<&Arc<Vec<u8>>> {
        None
    }
    /// The colors become grays: adjustments that only add color are
    /// neutralized.
    fn grays(&self) -> bool {
        false
    }
}

impl Recolor for Conversion<'_> {
    fn recolorer(&self) -> cms::Recolorer {
        Conversion::recolorer(self)
    }

    fn profile(&self) -> Option<&Arc<Vec<u8>>> {
        Some(self.to.icc())
    }
}

impl Recolor for Grayscale<'_> {
    fn recolorer(&self) -> cms::Recolorer {
        Grayscale::recolorer(self)
    }

    fn grays(&self) -> bool {
        true
    }
}

/// A [`Recolor`] with its transforms built.
struct Prepared<'a> {
    run: cms::Recolorer,
    profile: Option<&'a Arc<Vec<u8>>>,
    grays: bool,
}

impl Prepared<'_> {
    fn pixels(&self, image: &mut image::RgbaImage) {
        self.run.convert_rgba(image);
    }

    fn color(&self, rgb: [u8; 3]) -> [u8; 3] {
        self.run.color(rgb)
    }

    fn profile(&self) -> Option<&Arc<Vec<u8>>> {
        self.profile
    }

    fn grays(&self) -> bool {
        self.grays
    }
}

/// A layer's colors changed: its pixels as they are (a text, a shape or a
/// smart object stays one) and the colors its contents are drawn with.
fn convert_layer(layer: &mut Layer, c: &Prepared) {
    if let Some(s) = layer.smart.as_deref_mut() {
        // Its contents converted, then rendered again from them.
        s.recolor(&mut |l| convert_layer(l, c), c.profile());
        let limit = layer.rect();
        layer.render_smart(limit);
    } else if layer.image.width() * layer.image.height() > 0 {
        c.pixels(Arc::make_mut(&mut layer.image));
        layer.rev = next_rev();
    }
    let rgb = |v: &mut [u8; 3]| *v = c.color(*v);
    let rgba = |v: &mut [u8; 4]| {
        let [r, g, b] = c.color([v[0], v[1], v[2]]);
        *v = [r, g, b, v[3]];
    };
    let gradient = |g: &mut GradientFill| {
        for stop in &mut g.gradient.colors {
            if let StopColor::Rgb(v) = &mut stop.color {
                rgb(v);
            }
        }
    };
    let paint = |p: &mut Paint| match p {
        Paint::Color(v) => rgb(v),
        Paint::Gradient(g) => gradient(g),
        Paint::None => {}
    };
    let stops = |g: &mut crate::paint::gradient::Gradient| {
        for stop in &mut g.colors {
            if let StopColor::Rgb(v) = &mut stop.color {
                rgb(v);
            }
        }
    };
    match &mut layer.kind {
        LayerKind::Fill(Fill::Solid(v)) => rgb(v),
        LayerKind::Fill(Fill::Gradient(g)) => gradient(g),
        LayerKind::Adjustment(a) => match a {
            Adjustment::PhotoFilter(f) => rgb(&mut f.color),
            Adjustment::GradientMap(g) => stops(&mut g.gradient),
            // In grays, what only adds color is dropped (a gray image
            // stays gray).
            Adjustment::ColorBalance(b) if c.grays() => {
                *b = ColorBalance {
                    preserve_luminosity: b.preserve_luminosity,
                    ..ColorBalance::default()
                };
            }
            Adjustment::HueSaturation(h) if c.grays() && h.colorize => {
                h.colorize = false;
                (h.hue, h.saturation) = (0.0, 0.0);
            }
            Adjustment::BlackWhite(b) if c.grays() => b.tint = false,
            // Per-channel corrections tint: only the composite one stays.
            Adjustment::Curves(curves) if c.grays() => {
                curves.channels = Default::default();
            }
            Adjustment::Levels(levels) if c.grays() => {
                levels.channels = Default::default();
            }
            _ => {}
        },
        _ => {}
    }
    if let Some(text) = &mut layer.text {
        for run in &mut text.runs {
            rgba(&mut run.style.color);
        }
    }
    if let Some(shape) = &mut layer.shape {
        paint(&mut shape.fill);
        paint(&mut shape.stroke.paint);
    }
    let fx = &mut layer.effects;
    for color in [
        &mut fx.stroke.color,
        &mut fx.shadow.color,
        &mut fx.inner_shadow.color,
        &mut fx.outer_glow.color,
        &mut fx.inner_glow.color,
        &mut fx.color_overlay.color,
        &mut fx.bevel.highlight,
        &mut fx.bevel.shadow,
    ] {
        rgba(color);
    }
    gradient(&mut fx.gradient_overlay.fill);
    if let Some(board) = &mut layer.artboard
        && let super::ArtboardBackground::Custom(v) = &mut board.background
    {
        rgb(v);
    }
}

/// Pixels decoded with a profile that isn't an RGB one, made RGB through
/// it: gray images, CMYK JPEG and TIFF files (read again as their inks).
/// Returns the profile the pixels now have: sRGB for those, the same for
/// RGB ones, none for other spaces (it doesn't describe RGB pixels).
pub(super) fn rgb_pixels(
    path: &Path,
    format: Option<ImageFormat>,
    image: &mut DynamicImage,
    icc: Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    // `image` doesn't give TIFFs' profiles.
    let icc = Arc::new(icc.or_else(|| tiff_profile(path, format))?);
    // One Little CMS can't read goes along as it is.
    let Some(profile) = Profile::from_icc(Arc::clone(&icc)) else {
        return Some(icc.to_vec());
    };
    let srgb = cms::srgb();
    let c = Conversion {
        from: &profile,
        to: srgb,
        intent: Intent::RelativeColorimetric,
        bpc: true,
    };
    match profile.space() {
        Space::Rgb => Some(profile.icc().to_vec()),
        Space::Gray => {
            let mut rgba = image.to_rgba8();
            c.gray_to_rgba(&mut rgba).then(|| {
                *image = DynamicImage::ImageRgba8(rgba);
                srgb.icc().to_vec()
            })
        }
        Space::Cmyk => {
            let (w, h) = (image.width(), image.height());
            let ink = read_inks(path, format, (w, h))?;
            let rgb = image::RgbImage::from_raw(w, h, c.cmyk_to_rgb(&ink)?)?;
            *image = DynamicImage::ImageRgb8(rgb);
            Some(srgb.icc().to_vec())
        }
        Space::Other => None,
    }
}

/// CMYK pixels (4 bytes, 0: no ink; `cmyk`) or gray ones (1 byte) in
/// sRGB (3 bytes), read through `icc`: CMYK without one through Atelier's
/// default CMYK. `None` if the profile isn't of that kind (or gray
/// without one: kept as it is).
pub(super) fn to_srgb(pixels: &[u8], cmyk: bool, icc: Option<&[u8]>) -> Option<Vec<u8>> {
    let profile = match icc {
        Some(icc) => Profile::from_icc(Arc::new(icc.to_vec()))?,
        None if cmyk => cms::default_cmyk().clone(),
        None => return None,
    };
    let c = Conversion {
        from: &profile,
        to: cms::srgb(),
        intent: Intent::RelativeColorimetric,
        bpc: true,
    };
    match profile.space() {
        Space::Cmyk if cmyk => c.cmyk_to_rgb(pixels),
        Space::Gray if !cmyk => c.gray_to_rgb(pixels),
        _ => None,
    }
}

/// The profile of pixels read with `icc`: itself if they are RGB (`rgb`)
/// and it is an RGB profile (or one Little CMS can't read), sRGB for gray
/// ones [`to_srgb`] converted, none otherwise.
pub(super) fn rgb_profile(icc: Option<Vec<u8>>, rgb: bool) -> Option<Vec<u8>> {
    let space = icc
        .as_ref()
        .map(|icc| Profile::from_icc(Arc::new(icc.clone())).map(|p| p.space()));
    match (rgb, space) {
        (true, Some(Some(Space::Rgb) | None)) => icc,
        (true, _) => None,
        (false, Some(Some(Space::Gray))) => Some(cms::srgb().icc().to_vec()),
        (false, _) => None,
    }
}

fn tiff_profile(path: &Path, format: Option<ImageFormat>) -> Option<Vec<u8>> {
    (format? == ImageFormat::Tiff).then_some(())?;
    let file = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let mut decoder = tiff::decoder::Decoder::new(file).ok()?;
    decoder.get_tag_u8_vec(tiff::tags::Tag::IccProfile).ok()
}

/// Inks (0: none) of a YCCK pixel: Y, Cb, Cr encode the inverted cyan,
/// magenta and yellow as Adobe stores them, and K the inverted black.
fn ycck_inks(p: &[u8]) -> [u8; 4] {
    let (y, cb, cr) = (p[0] as f32, p[1] as f32 - 128.0, p[2] as f32 - 128.0);
    let level = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    [
        level(y + 1.402 * cr),
        level(y - 0.344_136 * cb - 0.714_136 * cr),
        level(y + 1.772 * cb),
        255 - p[3],
    ]
}

/// The inks (0: none) of a CMYK JPEG or TIFF of `size`.
fn read_inks(path: &Path, format: Option<ImageFormat>, (w, h): (u32, u32)) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let inks = match format? {
        ImageFormat::Jpeg => {
            use zune_core::colorspace::ColorSpace;
            let cursor = || zune_core::bytestream::ZCursor::new(&data);
            let mut headers = zune_jpeg::JpegDecoder::new(cursor());
            headers.decode_headers().ok()?;
            // Plain CMYK, or YCCK (Adobe's transform 2, how Photoshop
            // usually writes them): decoded as stored.
            let space = headers.input_colorspace()?;
            if !matches!(space, ColorSpace::CMYK | ColorSpace::YCCK) {
                return None;
            }
            let options =
                zune_core::options::DecoderOptions::default().jpeg_set_out_colorspace(space);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(cursor(), options);
            let raw = decoder.decode().ok()?;
            (decoder.input_colorspace()? == space).then_some(())?;
            if space == ColorSpace::YCCK {
                raw.chunks_exact(4).flat_map(ycck_inks).collect()
            } else {
                // Stored inverted, as Photoshop writes them (Adobe's marker).
                raw.into_iter().map(|v| 255 - v).collect()
            }
        }
        ImageFormat::Tiff => {
            let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(&data)).ok()?;
            (decoder.colortype().ok()? == tiff::ColorType::CMYK(8)).then_some(())?;
            match decoder.read_image().ok()? {
                tiff::decoder::DecodingResult::U8(px) => px,
                _ => return None,
            }
        }
        _ => return None,
    };
    (inks.len() == w as usize * h as usize * 4).then_some(inks)
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;
    use crate::cms::{self, WorkingRgb};

    fn adobe(intent: Intent) -> Conversion<'static> {
        Conversion {
            from: cms::srgb(),
            to: WorkingRgb::AdobeRgb.profile(),
            intent,
            bpc: true,
        }
    }

    #[test]
    fn convert_to_profile_is_one_step_and_keeps_masks() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, Rgba([255, 0, 0, 255])), None);
        doc.add_mask(crate::document::NewMask::HideAll, None);
        let mask = doc.active_layer().mask.as_ref().map(|m| m.image().clone());
        doc.add_fill_layer(Fill::Solid([0, 255, 0]), None);
        doc.convert_to_profile(&adobe(Intent::RelativeColorimetric));
        let px = doc.layers[0].pixel_at(1, 1).unwrap();
        assert!(px[0] < 230 && px[0] > 200, "{px:?}");
        assert_eq!(doc.layers[0].mask.as_ref().map(|m| m.image().clone()), mask);
        assert!(matches!(doc.layers[1].kind, LayerKind::Fill(Fill::Solid([r, 255, _])) if r > 100));
        assert_eq!(
            doc.icc_profile.as_deref(),
            Some(WorkingRgb::AdobeRgb.profile().icc().as_ref())
        );
        doc.undo();
        assert_eq!(doc.layers[0].pixel_at(1, 1), Some([255, 0, 0, 255]));
        assert_eq!(doc.icc_profile, None);
        assert!(matches!(
            doc.layers[1].kind,
            LayerKind::Fill(Fill::Solid([0, 255, 0]))
        ));
    }

    #[test]
    fn assign_profile_keeps_the_numbers() {
        let mut doc = Document::new(RgbaImage::from_pixel(2, 2, Rgba([10, 20, 30, 255])), None);
        doc.assign_profile(Some(WorkingRgb::DisplayP3.profile()));
        assert_eq!(doc.layers[0].pixel_at(0, 0), Some([10, 20, 30, 255]));
        assert_eq!(doc.profile_or(cms::srgb()).name(), "Display P3");
        doc.undo();
        assert_eq!(doc.profile_or(cms::srgb()).name(), cms::srgb().name());
    }

    #[test]
    fn profiles_and_proof_setup_are_saved() {
        let dir = std::env::temp_dir().join(format!("atelier-color-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("proof.atelier");
        let mut doc = Document::new(RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255])), None);
        doc.assign_profile(Some(WorkingRgb::ProPhoto.profile()));
        doc.proof = ProofSetup {
            custom: Some(WorkingRgb::AdobeRgb.profile().clone()),
            intent: Intent::Perceptual,
            bpc: false,
            paper: true,
            ink: true,
            colors: true,
            gamut: true,
        };
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        // Proof Colors and Gamut Warning are views, off when opened.
        let setup = ProofSetup {
            colors: false,
            gamut: false,
            ..doc.proof.clone()
        };
        assert_eq!(back.proof, setup);
        assert_eq!(back.icc_profile, doc.icc_profile);
        // The defaults are not written, and come back.
        doc.proof = ProofSetup::default();
        doc.save(&path).unwrap();
        assert_eq!(
            Document::open(&path).unwrap().doc.proof,
            ProofSetup::default()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmyk_jpeg_and_tiff_read_back() {
        let dir = std::env::temp_dir().join(format!("atelier-cmyk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut image = RgbaImage::from_pixel(16, 8, Rgba([255, 255, 255, 255]));
        for x in 0..8 {
            for y in 0..8 {
                image.put_pixel(x, y, Rgba([0, 160, 230, 255]));
            }
        }
        let doc = Document::new(image, None);
        let c = Conversion {
            from: cms::srgb(),
            to: cms::default_cmyk(),
            intent: Intent::RelativeColorimetric,
            bpc: true,
        };
        let jpeg = dir.join("cmyk.jpg");
        doc.export_cmyk(&jpeg, ExportFormat::Jpeg, 95, &c).unwrap();
        let mut decoder = jpeg_decoder::Decoder::new(std::fs::File::open(&jpeg).unwrap());
        let pixels = decoder.decode().unwrap();
        let info = decoder.info().unwrap();
        assert_eq!(info.pixel_format, jpeg_decoder::PixelFormat::CMYK32);
        assert_eq!(
            decoder.icc_profile().as_deref(),
            Some(cms::default_cmyk().icc().as_slice())
        );
        // Stored inverted (Adobe's marker), read back as inks.
        let ink = |i: usize| &pixels[i * 4..i * 4 + 4];
        let cyan = ink(2 * 16 + 2);
        assert!(cyan[0] > 170 && cyan[2] < 60, "cyan: {cyan:?}");
        let paper = ink(4 * 16 + 12);
        assert!(paper.iter().all(|&v| v < 8), "paper: {paper:?}");

        let tiff = dir.join("cmyk.tif");
        doc.export_cmyk(&tiff, ExportFormat::Tiff, 90, &c).unwrap();
        let mut decoder = tiff::decoder::Decoder::new(std::fs::File::open(&tiff).unwrap()).unwrap();
        assert_eq!(decoder.colortype().unwrap(), tiff::ColorType::CMYK(8));
        let icc = decoder.get_tag_u8_vec(tiff::tags::Tag::IccProfile).unwrap();
        assert_eq!(icc, cms::default_cmyk().icc().as_slice());
        let tiff::decoder::DecodingResult::U8(px) = decoder.read_image().unwrap() else {
            panic!("8-bit");
        };
        assert!(px[(2 * 16 + 2) * 4] > 170 && px[(4 * 16 + 12) * 4 + 3] < 8);

        let psd = dir.join("cmyk.psd");
        doc.export_cmyk(&psd, ExportFormat::Psd, 90, &c).unwrap();
        let data = std::fs::read(&psd).unwrap();
        // Header: 4 channels, CMYK mode.
        assert_eq!(&data[12..14], &[0, 4]);
        assert_eq!(&data[24..26], &[0, 4]);
        // ImageMagick, when installed, agrees.
        for file in [&jpeg, &tiff, &psd] {
            let Ok(out) = std::process::Command::new("identify")
                .args(["-format", "%[colorspace] %[profile:icc]\n"])
                .arg(file)
                .output()
            else {
                continue;
            };
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.starts_with("CMYK"), "{file:?}: {text}");
            assert!(text.contains("FOGRA39"), "{file:?}: {text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cmyk_files_open_converted_with_their_profile() {
        let dir = std::env::temp_dir().join(format!("atelier-cmyk-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let steel = [70, 130, 180];
        let doc = Document::new(
            RgbaImage::from_pixel(8, 8, Rgba([steel[0], steel[1], steel[2], 255])),
            None,
        );
        let c = Conversion {
            from: cms::srgb(),
            to: cms::default_cmyk(),
            intent: Intent::RelativeColorimetric,
            bpc: true,
        };
        let formats = [
            (ExportFormat::Jpeg, "open.jpg"),
            (ExportFormat::Tiff, "open.tif"),
            (ExportFormat::Psd, "open.psd"),
        ];
        for (format, name) in formats {
            let path = dir.join(name);
            doc.export_cmyk(&path, format, 100, &c).unwrap();
            let back = Document::open(&path).unwrap().doc;
            let px = back.layers[0].pixel_at(4, 4).unwrap();
            assert!(
                (0..3).all(|i| px[i].abs_diff(steel[i]) <= 6),
                "{format:?}: {px:?}"
            );
            let profile = back.icc_profile.as_deref().map(Vec::as_slice);
            assert_eq!(profile, Some(cms::srgb().icc().as_slice()), "{format:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grayscale_leaves_no_color() {
        let photo = RgbaImage::from_fn(64, 48, |x, y| {
            Rgba([(x * 4) as u8, (y * 5) as u8, 200, 255])
        });
        let colorize = crate::pixel_ops::HueSaturation {
            hue: 30.0,
            saturation: 0.5,
            lightness: 0.0,
            colorize: true,
        };
        for adjustment in [
            Adjustment::PhotoFilter(Default::default()),
            Adjustment::HueSaturation(colorize),
            Adjustment::ColorBalance(ColorBalance {
                midtones: [0.5, -0.3, 0.2],
                ..Default::default()
            }),
            Adjustment::BlackWhite(crate::pixel_ops::BlackWhite {
                tint: true,
                ..Default::default()
            }),
        ] {
            let mut doc = Document::new(photo.clone(), None);
            doc.add_adjustment_layer(adjustment.clone(), None);
            for of in [cms::srgb(), cms::WorkingRgb::AdobeRgb.profile()] {
                doc.grayscale(&Grayscale { of });
                let flat = doc.flatten();
                let spread = |p: &Rgba<u8>| p[0].abs_diff(p[1]).max(p[0].abs_diff(p[2]));
                let worst = flat.pixels().map(spread).max().unwrap();
                assert!(worst <= 1, "{adjustment:?}: {worst}");
            }
        }
    }

    #[test]
    fn ycck_jpegs_open_with_their_profile() {
        // Photoshop's CMYK JPEGs are usually YCCK (Adobe's transform 2).
        let dir = std::env::temp_dir().join(format!("atelier-ycck-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let c = Conversion {
            from: cms::srgb(),
            to: cms::default_cmyk(),
            intent: Intent::RelativeColorimetric,
            bpc: true,
        };
        for steel in [[70u8, 130, 180], [200, 40, 30], [250, 250, 245]] {
            let rgb: Vec<u8> = (0..16 * 16).flat_map(|_| steel).collect();
            let ink = c.rgb_to_cmyk(&rgb).unwrap();
            let path = dir.join("ycck.jpg");
            let mut encoder = jpeg_encoder::Encoder::new_file(&path, 98).unwrap();
            encoder.add_icc_profile(cms::default_cmyk().icc()).unwrap();
            encoder
                .encode(&ink, 16, 16, jpeg_encoder::ColorType::CmykAsYcck)
                .unwrap();
            let back = Document::open(&path).unwrap().doc;
            let px = back.layers[0].pixel_at(8, 8).unwrap();
            assert!(
                (0..3).all(|i| px[i].abs_diff(steel[i]) <= 6),
                "{steel:?}: {px:?}"
            );
            let profile = back.icc_profile.as_deref().map(Vec::as_slice);
            assert_eq!(profile, Some(cms::srgb().icc().as_slice()));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn cmyk_jpegs_keep_one_pixel_detail_at_low_quality() {
        let dir = std::env::temp_dir().join(format!("atelier-cmyk-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 1-px red / white columns, and 1-px black / white columns.
        let photo = RgbaImage::from_fn(96, 64, |x, y| match (x % 2 == 0, y < 32) {
            (true, true) => Rgba([230, 20, 30, 255]),
            (true, false) => Rgba([0, 0, 0, 255]),
            _ => Rgba([255, 255, 255, 255]),
        });
        let doc = Document::new(photo.clone(), None);
        let c = Conversion {
            from: cms::srgb(),
            to: cms::default_cmyk(),
            intent: Intent::RelativeColorimetric,
            bpc: true,
        };
        let path = dir.join("q80.jpg");
        doc.export_cmyk(&path, ExportFormat::Jpeg, 80, &c).unwrap();
        let back = Document::open(&path).unwrap().doc;
        let (mut error, mut n) = (0.0f64, 0.0f64);
        for y in 4..60 {
            for x in 4..92 {
                let a = photo.get_pixel(x, y).0;
                let b = back.layers[0].pixel_at(x as i64, y as i64).unwrap();
                error += (0..3).map(|c| a[c].abs_diff(b[c]) as f64).sum::<f64>() / 3.0;
                n += 1.0;
            }
        }
        // Half-resolution inks: about 100; whole: a few levels.
        assert!(error / n < 20.0, "mean error {}", error / n);
        let white = back.layers[0].pixel_at(11, 10).unwrap();
        assert!(
            white[..3].iter().all(|&v| v > 200),
            "white column {white:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grayscale_neutralizes_per_channel_curves_levels_and_artboards() {
        let photo = RgbaImage::from_fn(64, 48, |x, y| {
            Rgba([(x * 4) as u8, (y * 5) as u8, 200, 255])
        });
        let mut curves = crate::pixel_ops::Curves::default();
        curves.channels[0] =
            crate::pixel_ops::Curve::from_points(&[[0.0, 0.0], [128.0, 190.0], [255.0, 255.0]])
                .unwrap();
        let mut levels = crate::pixel_ops::Levels::default();
        levels.channels[2].gamma = 0.5;
        for adjustment in [Adjustment::Curves(curves), Adjustment::Levels(levels)] {
            let mut doc = Document::new(photo.clone(), None);
            doc.add_adjustment_layer(adjustment, None);
            doc.grayscale(&Grayscale { of: cms::srgb() });
            let spread = |p: &Rgba<u8>| p[0].abs_diff(p[1]).max(p[0].abs_diff(p[2]));
            let worst = doc.flatten().pixels().map(spread).max().unwrap();
            assert!(worst <= 1, "spread {worst}");
        }
        let mut doc = Document::new(RgbaImage::new(32, 32), None);
        let rect = crate::document::PixelRect {
            x: 0,
            y: 0,
            w: 16,
            h: 16,
        };
        let background = super::super::ArtboardBackground::Custom([200, 40, 40]);
        doc.add_artboard("a".into(), "A".into(), rect, background);
        doc.grayscale(&Grayscale { of: cms::srgb() });
        let board = doc
            .layers
            .iter()
            .find_map(|l| l.artboard)
            .expect("artboard");
        assert!(
            matches!(board.background, super::super::ArtboardBackground::Custom([r, g, b]) if r == g && g == b),
            "{:?}",
            board.background
        );
    }

    #[test]
    fn grayscale_of_many_layers_builds_its_transforms_once() {
        let mut doc = Document::new(RgbaImage::new(16, 16), None);
        for i in 0..200 {
            doc.add_layer(format!("l{i}"), RgbaImage::new(4, 4), (0, 0));
        }
        let t = std::time::Instant::now();
        doc.grayscale(&Grayscale { of: cms::srgb() });
        // Two transforms per color: about 26 s.
        assert!(t.elapsed().as_secs_f32() < 5.0, "{:?}", t.elapsed());
    }
}
