//! Color management, by Little CMS: the working spaces, the documents' and
//! the monitor's profiles, conversions between them (8-bit pixels and
//! colors), CMYK output, and what the canvas shows (the document on the
//! monitor, soft proof, gamut warning) as a 3D table applied to each pixel.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use image::RgbaImage;
use lcms2::{
    CIExyY, CIExyYTRIPLE, DisallowCache, Flags, GlobalContext, InfoType, Locale, MLU, PixelFormat,
    TagSignature, ToneCurve, Transform,
};
use serde::{Deserialize, Serialize};

mod monitor;

pub use monitor::monitor_profile;

/// What a profile's numbers are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    Rgb,
    Cmyk,
    Gray,
    Other,
}

/// An ICC profile: its bytes (embedded in the files), name and color space.
#[derive(Clone, Debug)]
pub struct Profile {
    icc: Arc<Vec<u8>>,
    name: String,
    space: Space,
}

impl PartialEq for Profile {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.icc, &other.icc) || self.icc == other.icc
    }
}

impl Profile {
    /// The profile in `icc`, if Little CMS can use it.
    pub fn from_icc(icc: Arc<Vec<u8>>) -> Option<Self> {
        let p = lcms2::Profile::new_icc(&icc).ok()?;
        let space = match p.color_space() {
            lcms2::ColorSpaceSignature::RgbData => Space::Rgb,
            lcms2::ColorSpaceSignature::CmykData => Space::Cmyk,
            lcms2::ColorSpaceSignature::GrayData => Space::Gray,
            _ => Space::Other,
        };
        let name = p
            .info(InfoType::Description, Locale::none())
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "ICC".to_owned());
        Some(Self { icc, name, space })
    }

    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        Self::from_icc(Arc::new(data))
    }

    pub fn icc(&self) -> &Arc<Vec<u8>> {
        &self.icc
    }

    /// Its description ("Display P3").
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn space(&self) -> Space {
        self.space
    }

    fn lcms(&self) -> lcms2::Profile {
        lcms2::Profile::new_icc(&self.icc).expect("checked by `from_icc`")
    }

    /// A profile made here: `name` written in it, ICC version 2 (read by
    /// every application).
    fn made(mut p: lcms2::Profile, name: &str) -> Self {
        let mut text = |sig, value: &str| {
            let mut mlu = MLU::new(1);
            mlu.set_text_ascii(value, Locale::new("en_US"));
            p.write_tag(sig, lcms2::Tag::MLU(&mlu));
        };
        text(TagSignature::ProfileDescriptionTag, name);
        text(TagSignature::CopyrightTag, "No copyright, use freely");
        p.set_version(2.1);
        let icc = p.icc().expect("profile made here");
        Self::from_icc(Arc::new(icc)).expect("profile made here")
    }
}

/// The RGB working spaces of Edit › Color Settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkingRgb {
    #[default]
    Srgb,
    AdobeRgb,
    DisplayP3,
    ProPhoto,
}

impl WorkingRgb {
    pub const ALL: [WorkingRgb; 4] = [
        WorkingRgb::Srgb,
        WorkingRgb::AdobeRgb,
        WorkingRgb::DisplayP3,
        WorkingRgb::ProPhoto,
    ];

    /// The profile, made once.
    pub fn profile(self) -> &'static Profile {
        static MADE: [OnceLock<Profile>; 4] = [const { OnceLock::new() }; 4];
        MADE[self as usize].get_or_init(|| make_rgb(self))
    }
}

/// sRGB's tone curve: γ 2.4 with a linear foot.
fn srgb_curve() -> ToneCurve {
    let params = [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045];
    ToneCurve::new_parametric(4, &params).expect("valid curve")
}

fn make_rgb(space: WorkingRgb) -> Profile {
    let xy = |x, y| CIExyY { x, y, Y: 1.0 };
    let d65 = xy(0.3127, 0.3290);
    let d50 = xy(0.3457, 0.3585);
    let primaries = |r: (f64, f64), g: (f64, f64), b: (f64, f64)| CIExyYTRIPLE {
        Red: xy(r.0, r.1),
        Green: xy(g.0, g.1),
        Blue: xy(b.0, b.1),
    };
    let rgb = |white: &CIExyY, p: &CIExyYTRIPLE, curve: &ToneCurve| {
        lcms2::Profile::new_rgb(white, p, &[curve, curve, curve]).expect("valid profile")
    };
    match space {
        WorkingRgb::Srgb => Profile::made(lcms2::Profile::new_srgb(), "sRGB IEC61966-2.1"),
        WorkingRgb::AdobeRgb => {
            let p = primaries((0.64, 0.33), (0.21, 0.71), (0.15, 0.06));
            let profile = rgb(&d65, &p, &ToneCurve::new(563.0 / 256.0));
            Profile::made(profile, "Compatible with Adobe RGB (1998)")
        }
        WorkingRgb::DisplayP3 => {
            let p = primaries((0.680, 0.320), (0.265, 0.690), (0.150, 0.060));
            Profile::made(rgb(&d65, &p, &srgb_curve()), "Display P3")
        }
        WorkingRgb::ProPhoto => {
            let p = primaries((0.7347, 0.2653), (0.1596, 0.8404), (0.0366, 0.0001));
            Profile::made(rgb(&d50, &p, &ToneCurve::new(1.8)), "ProPhoto RGB")
        }
    }
}

pub fn srgb() -> &'static Profile {
    WorkingRgb::Srgb.profile()
}

/// The CMYK profile that comes with Atelier: coated offset paper
/// (FOGRA39L), free of copyright restrictions (see assets/icc).
pub fn default_cmyk() -> &'static Profile {
    static PROFILE: OnceLock<Profile> = OnceLock::new();
    PROFILE.get_or_init(|| {
        let icc = include_bytes!("../assets/icc/FOGRA39L_coated.icc").to_vec();
        Profile::from_icc(Arc::new(icc)).expect("bundled profile")
    })
}

/// How colors outside the destination's gamut are brought in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Intent {
    Perceptual,
    #[default]
    RelativeColorimetric,
    Saturation,
    AbsoluteColorimetric,
}

impl Intent {
    pub const ALL: [Intent; 4] = [
        Intent::Perceptual,
        Intent::Saturation,
        Intent::RelativeColorimetric,
        Intent::AbsoluteColorimetric,
    ];

    fn lcms(self) -> lcms2::Intent {
        match self {
            Intent::Perceptual => lcms2::Intent::Perceptual,
            Intent::RelativeColorimetric => lcms2::Intent::RelativeColorimetric,
            Intent::Saturation => lcms2::Intent::Saturation,
            Intent::AbsoluteColorimetric => lcms2::Intent::AbsoluteColorimetric,
        }
    }
}

/// A conversion between two profiles.
#[derive(Clone, Copy)]
pub struct Conversion<'a> {
    pub from: &'a Profile,
    pub to: &'a Profile,
    pub intent: Intent,
    /// Black point compensation: the source's black maps to the
    /// destination's (shadow detail kept).
    pub bpc: bool,
}

/// A [`Conversion`] that owns its profiles (to take to another thread).
#[derive(Clone, Debug, PartialEq)]
pub struct Converter {
    pub from: Profile,
    pub to: Profile,
    pub intent: Intent,
    pub bpc: bool,
}

impl Converter {
    pub fn conversion(&self) -> Conversion<'_> {
        Conversion {
            from: &self.from,
            to: &self.to,
            intent: self.intent,
            bpc: self.bpc,
        }
    }
}

type Shared<A, B> = Transform<A, B, GlobalContext, DisallowCache>;

/// A Little CMS transform usable from several threads.
fn transform<A: Copy + lcms2::Pod, B: Copy + lcms2::Pod>(
    (from, to): (&Profile, &Profile),
    (input, output): (PixelFormat, PixelFormat),
    intent: Intent,
    bpc: bool,
    extra: Flags,
) -> Option<Shared<A, B>> {
    let mut flags = Flags::NO_CACHE | extra;
    if bpc {
        flags = flags | Flags::BLACKPOINT_COMPENSATION;
    }
    let (from, to) = (from.lcms(), to.lcms());
    Transform::new_flags_context(
        GlobalContext::new(),
        &from,
        input,
        &to,
        output,
        intent.lcms(),
        flags,
    )
    .ok()
}

/// `src` through a transform (16-bit values, `A` then `B` channels).
fn apply16<const A: usize, const B: usize>(
    src: &[[u16; A]],
    profiles: (&Profile, &Profile),
    intent: Intent,
    bpc: bool,
) -> Option<Vec<[u16; B]>>
where
    [u16; A]: lcms2::Pod,
    [u16; B]: lcms2::Pod,
{
    let format = |n| {
        if n == 4 {
            PixelFormat::CMYK_16
        } else {
            PixelFormat::RGB_16
        }
    };
    let t = transform::<[u16; A], [u16; B]>(
        profiles,
        (format(A), format(B)),
        intent,
        bpc,
        // A few points (a table's): faster without the precalculation.
        Flags::NO_OPTIMIZE,
    )?;
    let mut out = vec![[0u16; B]; src.len()];
    t.transform_pixels(src, &mut out);
    Some(out)
}

impl Conversion<'_> {
    fn transform<A: Copy + lcms2::Pod, B: Copy + lcms2::Pod>(
        &self,
        formats: (PixelFormat, PixelFormat),
        extra: Flags,
    ) -> Option<Shared<A, B>> {
        transform((self.from, self.to), formats, self.intent, self.bpc, extra)
    }

    /// Converts the RGB pixels of `image` (transparency kept), on all the
    /// cores. Returns `false` if the profiles can't be used for that.
    pub fn convert_rgba(&self, image: &mut RgbaImage) -> bool {
        self.recolorer().convert_rgba(image)
    }

    /// The transforms of this conversion, built once for many images and
    /// colors (building one costs milliseconds).
    pub fn recolorer(&self) -> Recolorer {
        let (rgba, rgb) = (PixelFormat::RGBA_8, PixelFormat::RGB_8);
        Recolorer {
            rgba: self
                .transform((rgba, rgba), Flags::COPY_ALPHA)
                .map(|t| vec![t]),
            rgb: self
                .transform((rgb, rgb), Flags::default())
                .map(|t| vec![t]),
            grays: false,
        }
    }

    /// CMYK values (0: no ink, 255: full) of opaque RGB pixels (3 bytes
    /// each), 4 bytes each, on all the cores.
    pub fn rgb_to_cmyk(&self, rgb: &[u8]) -> Option<Vec<u8>> {
        self.bytes((PixelFormat::RGB_8, PixelFormat::CMYK_8), rgb, (3, 4))
    }

    /// RGB pixels (3 bytes each) of CMYK ones (4 bytes, 0: no ink), on all
    /// the cores.
    pub fn cmyk_to_rgb(&self, cmyk: &[u8]) -> Option<Vec<u8>> {
        self.bytes((PixelFormat::CMYK_8, PixelFormat::RGB_8), cmyk, (4, 3))
    }

    /// RGB pixels (3 bytes each) of gray ones, on all the cores.
    pub fn gray_to_rgb(&self, gray: &[u8]) -> Option<Vec<u8>> {
        self.bytes((PixelFormat::GRAY_8, PixelFormat::RGB_8), gray, (1, 3))
    }

    /// `src` (`a` bytes a pixel) converted (`b` bytes a pixel) on all the
    /// cores.
    fn bytes(
        &self,
        formats: (PixelFormat, PixelFormat),
        src: &[u8],
        (a, b): (usize, usize),
    ) -> Option<Vec<u8>> {
        let t = self.transform::<u8, u8>(formats, Flags::default())?;
        let n = src.len() / a;
        let mut out = vec![0u8; n * b];
        let threads = std::thread::available_parallelism().map_or(1, |t| t.get());
        let per = n.div_ceil(threads).max(1 << 14);
        std::thread::scope(|s| {
            for (src, dst) in src.chunks(per * a).zip(out.chunks_mut(per * b)) {
                let t = &t;
                s.spawn(move || t.transform_pixels(src, dst));
            }
        });
        Some(out)
    }

    /// Gray pixels (the red of `image`'s, as decoders give them) to the
    /// RGB destination, transparency kept, on all the cores.
    pub fn gray_to_rgba(&self, image: &mut RgbaImage) -> bool {
        let formats = (PixelFormat::GRAYA_8, PixelFormat::RGBA_8);
        let Some(t) = self.transform::<u8, u8>(formats, Flags::COPY_ALPHA) else {
            return false;
        };
        let row = image.width() as usize * 4;
        crate::pixel_ops::par_bands(image.as_mut(), row, |band| {
            let gray: Vec<u8> = band.chunks_exact(4).flat_map(|p| [p[0], p[3]]).collect();
            t.transform_pixels(&gray, band);
        });
        true
    }

    /// One RGB color in CMYK, ink percentages (0..100).
    pub fn color_to_cmyk(&self, rgb: [u8; 3]) -> Option<[f32; 4]> {
        let ink = apply16::<3, 4>(
            &[rgb.map(|c| c as u16 * 257)],
            (self.from, self.to),
            self.intent,
            self.bpc,
        )?;
        Some(ink[0].map(|v| v as f32 * 100.0 / 65535.0))
    }

    /// One CMYK color (ink percentages) in the RGB destination.
    pub fn cmyk_to_color(&self, cmyk: [f32; 4]) -> Option<[u8; 3]> {
        let ink = cmyk.map(|v| (v.clamp(0.0, 100.0) / 100.0 * 65535.0).round() as u16);
        let rgb = apply16::<4, 3>(&[ink], (self.from, self.to), self.intent, self.bpc)?;
        Some(rgb[0].map(|v| ((v as u32 + 128) / 257) as u8))
    }
}

/// Image › Mode › Grayscale: colors to the grays of the same lightness
/// (through a gray γ 2.2 profile), back in the RGB profile `of`.
pub struct Grayscale<'a> {
    pub of: &'a Profile,
}

fn gray() -> &'static Profile {
    static PROFILE: OnceLock<Profile> = OnceLock::new();
    PROFILE.get_or_init(|| {
        let p = lcms2::Profile::new_gray(&d50(), &ToneCurve::new(2.2)).expect("valid profile");
        Profile::made(p, "Gray Gamma 2.2")
    })
}

impl Grayscale<'_> {
    fn there_and_back(
        &self,
        (a, b): (PixelFormat, PixelFormat),
        extra: Flags,
    ) -> Option<[Shared<u8, u8>; 2]> {
        let intent = Intent::RelativeColorimetric;
        Some([
            transform((self.of, gray()), (a, b), intent, false, extra)?,
            transform((gray(), self.of), (b, a), intent, false, extra)?,
        ])
    }

    /// The transforms there and back, built once for many images and
    /// colors.
    pub fn recolorer(&self) -> Recolorer {
        let rgba = (PixelFormat::RGBA_8, PixelFormat::GRAYA_8);
        let rgb = (PixelFormat::RGB_8, PixelFormat::GRAY_8);
        Recolorer {
            rgba: self.there_and_back(rgba, Flags::COPY_ALPHA).map(Vec::from),
            rgb: self.there_and_back(rgb, Flags::default()).map(Vec::from),
            grays: true,
        }
    }
}

/// A conversion's (or Grayscale's) transforms, built once: one, or there
/// and back through the gray space.
pub struct Recolorer {
    rgba: Option<Vec<Shared<u8, u8>>>,
    rgb: Option<Vec<Shared<u8, u8>>>,
    grays: bool,
}

impl Recolorer {
    /// Converts the RGB pixels of `image` (transparency kept), on all the
    /// cores. Returns `false` if the profiles can't be used for that.
    pub fn convert_rgba(&self, image: &mut RgbaImage) -> bool {
        let Some(ts) = &self.rgba else {
            return false;
        };
        let row = image.width() as usize * 4;
        crate::pixel_ops::par_bands(image.as_mut(), row, |band| match ts.as_slice() {
            [there, back] => {
                let mut gray = vec![0u8; band.len() / 2];
                there.transform_pixels(band, &mut gray);
                back.transform_pixels(&gray, band);
                for p in band.chunks_exact_mut(4) {
                    let v = neutral([p[0], p[1], p[2]]);
                    p[..3].fill(v);
                }
            }
            [t] => t.transform_in_place(band),
            _ => {}
        });
        true
    }

    /// One RGB color converted (unchanged if the profiles can't be used).
    pub fn color(&self, rgb: [u8; 3]) -> [u8; 3] {
        let mut out = [0u8; 3];
        match self.rgb.as_deref() {
            Some([there, back]) => {
                let mut gray = [0u8];
                there.transform_pixels(&rgb, &mut gray);
                back.transform_pixels(&gray, &mut out);
            }
            Some([t]) => t.transform_pixels(&rgb, &mut out),
            _ => return rgb,
        }
        if self.grays { [neutral(out); 3] } else { out }
    }
}

/// A gray back from the gray space, exactly neutral: the round trip can
/// leave its channels a level apart, which later saturation would amplify.
fn neutral([r, g, b]: [u8; 3]) -> u8 {
    ((r as u32 + g as u32 + b as u32 + 1) / 3) as u8
}

/// Out of gamut: farther than this (ΔE) from its round trip through the
/// printing profile.
const GAMUT_DELTA: f64 = 5.0;
/// Out-of-gamut pixels, on the canvas: Photoshop's 50 % gray.
pub const GAMUT_GRAY: [u8; 3] = [128, 128, 128];

/// Whether `rgb` (in `from`) prints with `cmyk`, and the closest color
/// that does (its round trip, relative colorimetric).
pub fn printable(rgb: [u8; 3], from: &Profile, cmyk: &Profile) -> (bool, [u8; 3]) {
    let there = Conversion {
        from,
        to: cmyk,
        intent: Intent::RelativeColorimetric,
        bpc: false,
    };
    let back = Conversion {
        from: cmyk,
        to: from,
        ..there
    };
    let node = [rgb.map(|c| c as u16 * 257)];
    let Some(delta) = round_trip(&node, from, cmyk) else {
        return (true, rgb);
    };
    let ink = there.color_to_cmyk(rgb).unwrap_or_default();
    let near = back.cmyk_to_color(ink).unwrap_or(rgb);
    (delta[0] <= GAMUT_DELTA as f32, near)
}

/// ΔE between colors (`nodes`, 16-bit, in `from`) and their round trip
/// through `cmyk` (relative colorimetric, no compensation).
fn round_trip(nodes: &[[u16; 3]], from: &Profile, cmyk: &Profile) -> Option<Vec<f32>> {
    let lab = lcms2::Profile::new_lab4_context(GlobalContext::new(), &d50()).ok()?;
    let flags = Flags::NO_CACHE | Flags::NO_OPTIMIZE;
    let intent = lcms2::Intent::RelativeColorimetric;
    let to_lab = |profile: &Profile, format, src: &[u16]| -> Option<Vec<[f64; 3]>> {
        let p = profile.lcms();
        let t: Shared<u8, [f64; 3]> = Transform::new_flags_context(
            GlobalContext::new(),
            &p,
            format,
            &lab,
            PixelFormat::Lab_DBL,
            intent,
            flags,
        )
        .ok()?;
        let channels = if format == PixelFormat::CMYK_16 { 4 } else { 3 };
        let mut out = vec![[0.0; 3]; src.len() / channels];
        // `u16` values: as bytes, the transform's special case.
        t.transform_pixels(u8_slice(src), &mut out);
        Some(out)
    };
    let flat: Vec<u16> = nodes.iter().flatten().copied().collect();
    let direct = to_lab(from, PixelFormat::RGB_16, &flat)?;
    let inks = apply16::<3, 4>(nodes, (from, cmyk), Intent::RelativeColorimetric, false)?;
    let flat: Vec<u16> = inks.iter().flatten().copied().collect();
    let back = to_lab(cmyk, PixelFormat::CMYK_16, &flat)?;
    Some(
        direct
            .iter()
            .zip(&back)
            .map(|(a, b)| {
                let d2: f64 = (0..3).map(|i| (a[i] - b[i]).powi(2)).sum();
                d2.sqrt() as f32
            })
            .collect(),
    )
}

/// The bytes of 16-bit values (native order, as Little CMS reads them).
fn u8_slice(values: &[u16]) -> &[u8] {
    // SAFETY: any u16 is two valid bytes; the length covers them exactly.
    unsafe { std::slice::from_raw_parts(values.as_ptr().cast(), values.len() * 2) }
}

fn d50() -> CIExyY {
    CIExyY {
        x: 0.3457,
        y: 0.3585,
        Y: 1.0,
    }
}

/// Points per side of the 3D tables.
const GRID: usize = 33;

/// What the canvas shows of 8-bit RGB pixels: their colors through a 3D
/// table (tetrahedral interpolation), and the ones out of the printing
/// gamut in gray.
pub struct Lut {
    table: Vec<[u16; 3]>,
    /// ΔE of each point's round trip through the printing profile.
    gamut: Option<Vec<f32>>,
}

/// The table's points: 16-bit RGB, blue fastest.
fn grid() -> Vec<[u16; 3]> {
    let level = |k: usize| ((k * 65535 + (GRID - 1) / 2) / (GRID - 1)) as u16;
    let mut nodes = Vec::with_capacity(GRID * GRID * GRID);
    for r in 0..GRID {
        for g in 0..GRID {
            for b in 0..GRID {
                nodes.push([level(r), level(g), level(b)]);
            }
        }
    }
    nodes
}

/// Soft proof (View › Proof Setup): the printing condition simulated.
#[derive(Clone, Debug, PartialEq)]
pub struct Proof {
    pub profile: Profile,
    pub intent: Intent,
    pub bpc: bool,
    /// Simulate Paper Color: the paper's white (absolute colorimetric),
    /// its black too.
    pub paper: bool,
    /// Simulate Black Ink: the ink's black as it is (no compensation).
    pub ink: bool,
}

impl Lut {
    /// The document (`doc`) on the monitor; `None` when that changes
    /// nothing (same profile).
    pub fn display(doc: &Profile, monitor: &Profile) -> Option<Lut> {
        if doc == monitor {
            return None;
        }
        let nodes = grid();
        let table = apply16::<3, 3>(&nodes, (doc, monitor), Intent::RelativeColorimetric, true)?;
        let lut = Lut { table, gamut: None };
        (!lut.is_identity(&nodes)).then_some(lut)
    }

    /// The document on the monitor as `proof` prints it (`colors`: View ›
    /// Proof Colors), its unprintable colors in gray (`gamut`: View ›
    /// Gamut Warning). `None` when that changes nothing.
    pub fn proof(
        doc: &Profile,
        monitor: &Profile,
        proof: &Proof,
        colors: bool,
        gamut: bool,
    ) -> Option<Lut> {
        if !colors && !gamut {
            return Self::display(doc, monitor);
        }
        let nodes = grid();
        let table = if colors {
            let (shown, bpc) = if proof.paper {
                (Intent::AbsoluteColorimetric, false)
            } else {
                (Intent::RelativeColorimetric, !proof.ink)
            };
            let ink = &proof.profile;
            let inks = apply16::<3, 4>(&nodes, (doc, ink), proof.intent, proof.bpc)?;
            apply16::<4, 3>(&inks, (ink, monitor), shown, bpc)?
        } else if doc == monitor {
            nodes.clone()
        } else {
            apply16::<3, 3>(&nodes, (doc, monitor), Intent::RelativeColorimetric, true)?
        };
        let gamut = if gamut {
            Some(round_trip(&nodes, doc, &proof.profile)?)
        } else {
            None
        };
        Some(Lut { table, gamut })
    }

    /// The document on the monitor as converted to `to` (Edit › Convert
    /// to Profile's preview).
    pub fn converted(doc: &Profile, monitor: &Profile, c: &Conversion) -> Option<Lut> {
        let nodes = grid();
        let there = apply16::<3, 3>(&nodes, (doc, c.to), c.intent, c.bpc)?;
        let table = apply16::<3, 3>(&there, (c.to, monitor), Intent::RelativeColorimetric, true)?;
        Some(Lut { table, gamut: None })
    }

    fn is_identity(&self, nodes: &[[u16; 3]]) -> bool {
        self.table
            .iter()
            .zip(nodes)
            .all(|(a, b)| (0..3).all(|i| a[i].abs_diff(b[i]) <= 128))
    }

    /// What the canvas shows of `rgb`.
    #[inline]
    pub fn apply(&self, rgb: [u8; 3]) -> [u8; 3] {
        let (base, [fr, fg, fb]) = cell(rgb);
        if let Some(gamut) = &self.gamut {
            let d = trilinear(gamut, base, [fr, fg, fb]);
            if d > GAMUT_DELTA as f32 {
                return GAMUT_GRAY;
            }
        }
        tetrahedral(&self.table, base, [fr, fg, fb])
    }
}

/// Index of the table point at or below `rgb`, and the position in the
/// cell (0..255 on each axis).
#[inline]
fn cell(rgb: [u8; 3]) -> (usize, [i32; 3]) {
    let mut base = 0;
    let mut frac = [0; 3];
    for (c, &v) in rgb.iter().enumerate() {
        let p = v as usize * (GRID - 1);
        let (i, f) = (p / 255, p % 255);
        // The last point has no cell above it.
        let (i, f) = if i == GRID - 1 { (i - 1, 255) } else { (i, f) };
        base = base * GRID + i;
        frac[c] = f as i32;
    }
    (base, frac)
}

const STEP: [usize; 3] = [GRID * GRID, GRID, 1];

#[inline]
fn tetrahedral(t: &[[u16; 3]], base: usize, [fr, fg, fb]: [i32; 3]) -> [u8; 3] {
    let at = |dr: usize, dg: usize, db: usize| t[base + dr * STEP[0] + dg * STEP[1] + db * STEP[2]];
    let c000 = at(0, 0, 0);
    let c111 = at(1, 1, 1);
    // The tetrahedron the point is in: from c000 to c111 along the axes
    // in decreasing order of their fraction.
    let (f1, f2, f3, c1, c2) = if fr >= fg {
        if fg >= fb {
            (fr, fg, fb, at(1, 0, 0), at(1, 1, 0))
        } else if fr >= fb {
            (fr, fb, fg, at(1, 0, 0), at(1, 0, 1))
        } else {
            (fb, fr, fg, at(0, 0, 1), at(1, 0, 1))
        }
    } else if fb > fg {
        (fb, fg, fr, at(0, 0, 1), at(0, 1, 1))
    } else if fb > fr {
        (fg, fb, fr, at(0, 1, 0), at(0, 1, 1))
    } else {
        (fg, fr, fb, at(0, 1, 0), at(1, 1, 0))
    };
    let mut out = [0u8; 3];
    for i in 0..3 {
        let (a, b, c, d) = (c000[i] as i32, c1[i] as i32, c2[i] as i32, c111[i] as i32);
        let v = a * 255 + f1 * (b - a) + f2 * (c - b) + f3 * (d - c);
        // 16-bit × 255 → 8-bit.
        out[i] = ((v + 255 * 257 / 2) / (255 * 257)).clamp(0, 255) as u8;
    }
    out
}

#[inline]
fn trilinear(t: &[f32], base: usize, f: [i32; 3]) -> f32 {
    let [fr, fg, fb] = f.map(|v| v as f32 / 255.0);
    let at = |dr: usize, dg: usize, db: usize| t[base + dr * STEP[0] + dg * STEP[1] + db * STEP[2]];
    let lerp = |a: f32, b: f32, f: f32| a + (b - a) * f;
    let c00 = lerp(at(0, 0, 0), at(0, 0, 1), fb);
    let c01 = lerp(at(0, 1, 0), at(0, 1, 1), fb);
    let c10 = lerp(at(1, 0, 0), at(1, 0, 1), fb);
    let c11 = lerp(at(1, 1, 0), at(1, 1, 1), fb);
    lerp(lerp(c00, c01, fg), lerp(c10, c11, fg), fr)
}

/// A profile found among the system's.
#[derive(Clone, Debug)]
pub struct Installed {
    pub path: PathBuf,
    pub name: String,
    pub space: Space,
}

/// Folders where the system and the user keep color profiles.
fn profile_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(windows) {
        let root =
            std::env::var_os("SystemRoot").map_or_else(|| "C:\\Windows".into(), PathBuf::from);
        vec![root.join("System32\\spool\\drivers\\color")]
    } else if cfg!(target_os = "macos") {
        let mut dirs = vec![
            PathBuf::from("/Library/ColorSync/Profiles"),
            PathBuf::from("/System/Library/ColorSync/Profiles"),
        ];
        dirs.extend(home.map(|h| h.join("Library/ColorSync/Profiles")));
        dirs
    } else {
        let mut dirs = vec![
            PathBuf::from("/usr/share/color/icc"),
            PathBuf::from("/usr/local/share/color/icc"),
        ];
        if let Some(h) = home {
            dirs.push(h.join(".local/share/icc"));
            dirs.push(h.join(".color/icc"));
        }
        dirs
    }
}

/// The installed profiles of `space` (printers' and displays' alike), by
/// name; read once.
pub fn installed(space: Space) -> Vec<Installed> {
    static FOUND: OnceLock<Vec<Installed>> = OnceLock::new();
    let all = FOUND.get_or_init(|| {
        if cfg!(test) {
            return Vec::new();
        }
        let mut found: Vec<Installed> = Vec::new();
        let mut todo = profile_dirs();
        while let Some(dir) = todo.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    todo.push(path);
                    continue;
                }
                let icc = path.extension().and_then(|x| x.to_str()).is_some_and(|x| {
                    x.eq_ignore_ascii_case("icc") || x.eq_ignore_ascii_case("icm")
                });
                if let Some(p) = icc.then(|| Profile::load(&path)).flatten()
                    && !found.iter().any(|f| f.name == p.name)
                {
                    found.push(Installed {
                        path,
                        name: p.name,
                        space: p.space,
                    });
                }
            }
        }
        found.sort_by_key(|f| f.name.to_lowercase());
        found
    });
    all.iter().filter(|p| p.space == space).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv<'a>(from: &'a Profile, to: &'a Profile) -> Conversion<'a> {
        Conversion {
            from,
            to,
            intent: Intent::RelativeColorimetric,
            bpc: true,
        }
    }

    #[test]
    fn working_spaces_are_named_rgb_profiles() {
        for space in WorkingRgb::ALL {
            let p = space.profile();
            assert_eq!(p.space(), Space::Rgb);
            // Read back from its own bytes, as a file embeds it.
            let back = Profile::from_icc(Arc::clone(p.icc())).unwrap();
            assert_eq!(back.name(), p.name());
        }
        let adobe = WorkingRgb::AdobeRgb.profile();
        assert_eq!(adobe.name(), "Compatible with Adobe RGB (1998)");
        assert_eq!(default_cmyk().space(), Space::Cmyk);
    }

    #[test]
    fn srgb_primaries_in_adobe_rgb() {
        // Known values (Little CMS, relative colorimetric): sRGB red is
        // about (219, 0, 0) in Adobe RGB, green (144, 255, 60), blue (0, 0, 250).
        let c = conv(srgb(), WorkingRgb::AdobeRgb.profile());
        let near = |a: [u8; 3], b: [u8; 3]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 2);
        assert!(
            near(c.recolorer().color([255, 0, 0]), [219, 0, 0]),
            "{:?}",
            c.recolorer().color([255, 0, 0])
        );
        assert!(
            near(c.recolorer().color([0, 255, 0]), [144, 255, 60]),
            "{:?}",
            c.recolorer().color([0, 255, 0])
        );
        assert!(
            near(c.recolorer().color([0, 0, 255]), [0, 0, 250]),
            "{:?}",
            c.recolorer().color([0, 0, 255])
        );
        // White and black stay.
        assert_eq!(c.recolorer().color([255; 3]), [255; 3]);
        assert_eq!(c.recolorer().color([0; 3]), [0; 3]);
    }

    #[test]
    fn rgb_round_trips_within_a_unit() {
        let srgb = srgb();
        for space in [
            WorkingRgb::AdobeRgb,
            WorkingRgb::DisplayP3,
            WorkingRgb::ProPhoto,
        ] {
            let wide = space.profile();
            let mut image = RgbaImage::from_fn(64, 64, |x, y| {
                image::Rgba([
                    (x * 4) as u8,
                    (y * 4) as u8,
                    ((x + y) * 2) as u8,
                    (x * y) as u8,
                ])
            });
            let original = image.clone();
            assert!(conv(srgb, wide).convert_rgba(&mut image));
            assert_ne!(image, original);
            assert!(conv(wide, srgb).convert_rgba(&mut image));
            // ProPhoto's gamut is the widest: its 8-bit steps too.
            let tolerance = if space == WorkingRgb::ProPhoto { 5 } else { 3 };
            let mut total = 0;
            for (a, b) in image.pixels().zip(original.pixels()) {
                let diff = (0..3).map(|i| a.0[i].abs_diff(b.0[i])).max().unwrap();
                total += diff as u32;
                // 8 bits in a wider space lose a little; most near sRGB's
                // dark toe, where a step there is several here.
                if b.0[..3].iter().all(|&c| c >= 64) {
                    assert!(diff <= tolerance, "{space:?}: {a:?} {b:?}");
                }
                assert_eq!(a.0[3], b.0[3], "transparency kept");
            }
            // About a unit on average at most.
            assert!(total < 64 * 64 * tolerance as u32 / 3, "{space:?}: {total}");
        }
    }

    #[test]
    fn cmyk_of_known_colors() {
        let c = conv(srgb(), default_cmyk());
        let white = c.color_to_cmyk([255; 3]).unwrap();
        assert!(white.iter().all(|&v| v < 1.0), "{white:?}");
        let black = c.color_to_cmyk([0; 3]).unwrap();
        assert!(black[3] > 80.0, "rich black: {black:?}");
        let cyan = c.color_to_cmyk([0, 160, 230]).unwrap();
        assert!(cyan[0] > 70.0 && cyan[2] < 20.0, "{cyan:?}");
        // And back (a color the press prints).
        let steel = [70, 130, 180];
        let ink = c.color_to_cmyk(steel).unwrap();
        let back = conv(default_cmyk(), srgb()).cmyk_to_color(ink).unwrap();
        assert!((0..3).all(|i| back[i].abs_diff(steel[i]) <= 4), "{back:?}");
        let pixels = c.rgb_to_cmyk(&[255, 255, 255, 0, 0, 0]).unwrap();
        assert_eq!(pixels.len(), 8);
        assert!(pixels[..4].iter().all(|&v| v <= 2));
        assert!(pixels[7] > 200);
    }

    #[test]
    fn saturated_colors_are_out_of_the_printing_gamut() {
        let cmyk = default_cmyk();
        let (ok, near) = printable([0, 0, 255], srgb(), cmyk);
        assert!(!ok);
        assert_ne!(near, [0, 0, 255]);
        assert!(printable(near, srgb(), cmyk).0, "the color offered prints");
        assert!(printable([128, 128, 128], srgb(), cmyk).0);
        assert!(printable([200, 150, 120], srgb(), cmyk).0);
        // On the canvas too.
        let proof = Proof {
            profile: cmyk.clone(),
            intent: Intent::RelativeColorimetric,
            bpc: true,
            paper: false,
            ink: false,
        };
        let lut = Lut::proof(srgb(), srgb(), &proof, false, true).unwrap();
        assert_eq!(lut.apply([0, 255, 0]), GAMUT_GRAY);
        assert_eq!(lut.apply([0, 0, 255]), GAMUT_GRAY);
        assert_eq!(lut.apply([128, 128, 128]), [128, 128, 128]);
        assert_eq!(lut.apply([200, 150, 120]), [200, 150, 120]);
    }

    #[test]
    fn display_table_matches_the_exact_conversion() {
        let (doc, monitor) = (WorkingRgb::AdobeRgb.profile(), srgb());
        assert!(Lut::display(srgb(), srgb()).is_none(), "nothing to do");
        let lut = Lut::display(doc, monitor).unwrap();
        let c = conv(doc, monitor);
        for rgb in [
            [255, 0, 0],
            [12, 200, 77],
            [128, 128, 128],
            [250, 251, 3],
            [0, 0, 0],
        ] {
            let (a, b) = (lut.apply(rgb), c.recolorer().color(rgb));
            assert!(
                (0..3).all(|i| a[i].abs_diff(b[i]) <= 2),
                "{rgb:?}: {a:?} {b:?}"
            );
        }
    }

    #[test]
    fn soft_proof_dulls_what_does_not_print() {
        let proof = Proof {
            profile: default_cmyk().clone(),
            intent: Intent::RelativeColorimetric,
            bpc: true,
            paper: false,
            ink: false,
        };
        let lut = Lut::proof(srgb(), srgb(), &proof, true, false).unwrap();
        let blue = lut.apply([0, 0, 255]);
        assert!(blue[0] > 20 || blue[1] > 20, "{blue:?}");
        let gray = lut.apply([128, 128, 128]);
        assert!(gray.iter().all(|&c| c.abs_diff(128) <= 6), "{gray:?}");
        // The paper's white is darker than the monitor's.
        let paper = Lut::proof(
            srgb(),
            srgb(),
            &Proof {
                paper: true,
                ..proof
            },
            true,
            false,
        );
        let white = paper.unwrap().apply([255; 3]);
        assert!(white.iter().any(|&c| c < 250), "{white:?}");
    }

    #[test]
    fn grayscale_keeps_the_lightness() {
        let g = Grayscale { of: srgb() };
        let gray = g.recolorer().color([200, 40, 40]);
        assert!(gray[0] == gray[1] && gray[1] == gray[2], "{gray:?}");
        // Red is darker than its value: its lightness (L* about 47).
        assert!((100..=125).contains(&gray[0]), "{gray:?}");
        assert_eq!(g.recolorer().color([255; 3]), [255; 3]);
        assert_eq!(g.recolorer().color([0; 3]), [0; 3]);
        let mut image = RgbaImage::from_pixel(3, 1, image::Rgba([200, 40, 40, 77]));
        assert!(g.recolorer().convert_rgba(&mut image));
        let p = image.get_pixel(1, 0).0;
        assert_eq!(p, [gray[0], gray[1], gray[2], 77]);
    }

    #[test]
    fn gray_pixels_to_rgb() {
        let c = conv(gray(), srgb());
        let mut image = RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 128, 200]));
        assert!(c.gray_to_rgba(&mut image));
        let p = image.get_pixel(0, 0).0;
        // γ 2.2 against sRGB's curve: nearly the same gray.
        assert!(
            p[0] == p[1] && p[1] == p[2] && p[0].abs_diff(128) <= 3,
            "{p:?}"
        );
        assert_eq!(p[3], 200);
    }
}
