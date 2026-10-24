//! Atelier documents (.atelier): the layers and all their settings, as
//! Photoshop keeps them in a PSD.
//!
//! The file is a zip archive (any zip tool can open it) holding:
//! - `mimetype`: `image/x-atelier`, first and uncompressed, to recognize
//!   the file;
//! - `document.json`: the manifest ([`Manifest`]): format version, canvas
//!   size, active layer, then the layers from the bottom up with their
//!   settings (kind, name, position, visibility, opacity, fill, blend mode,
//!   lock, nesting in groups, clipping, link, mask, styles, text, shape,
//!   the settings of adjustment and fill layers), the ruler guides, the
//!   names of the saved selections and the paths of the Paths panel;
//! - `layers/<n>.png`: pixels of the n-th layer from the bottom, lossless
//!   8-bit RGBA PNG (stored as is: PNG is already compressed);
//! - `masks/<n>.png`: its mask, 8-bit grayscale PNG;
//! - `channels/<n>.png`: the n-th saved selection, 8-bit grayscale PNG;
//! - `thumbnail.png`: the flattened image, at most 256 px on a side
//!   (recent files);
//! - `color-profile.icc`: color profile of the pixels, if they have one;
//! - `proof-profile.icc`: the device View › Proof Setup simulates, when it
//!   isn't the working CMYK.
//!
//! `version` goes up whenever older versions of Atelier could not ignore a
//! change; they then refuse the file with a clear message. Additions they
//! can ignore (unknown fields are skipped) keep the version. Version 2
//! added groups, masks, clipping, adjustment and fill layers, fill opacity,
//! every blend mode and the new styles; version 1 files still open (the
//! missing fields take the look version 1 gave them). Text layers keep
//! their rendered pixels: they look the same where the font is missing,
//! and stay editable. Shape layers too (an Atelier that doesn't know them
//! opens them as pixels). Version 3 added artboards (older versions would
//! open them as plain groups, their layout lost) and smart objects (see
//! `native/smart.rs`; older versions would open them as pixels, their
//! contents, transform and smart filters lost).

use std::fs::File;
use std::io::{BufReader, Cursor, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;

use image::codecs::png::{PngDecoder, PngEncoder};
use image::{DynamicImage, ExtendedColorType, GrayImage, ImageDecoder, ImageEncoder, RgbaImage};
use serde::{Deserialize, Serialize};
use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use super::file_io::{MAX_DIMENSION, check_size};
use super::{
    Artboard, ArtboardBackground, BlendMode, Channel, Document, FileError, Fill, LayerKind, Mask,
};
use crate::compose::gradient::GradientFill;
use crate::compose::styles::{BevelFx, Effects, GlowFx, GradientFx, OverlayFx, ShadowFx, StrokeFx};
use crate::guides::{Guide, Orientation};
use crate::paint::gradient::{ColorStop, Gradient, OpacityStop, Shape, StopColor};
use crate::pixel_ops::{
    Adjustment, BlackWhite, BrightnessContrast, ColorBalance, Curve, Curves, GradientMap,
    HueSaturation, Levels, LevelsChannel, PhotoFilter, Posterize, Threshold, Vibrance,
};
use crate::text::{self, AntiAlias, CharStyle, Kerning, ParagraphStyle, TextAlign, TextData};

mod smart;
mod vectors;

pub(super) use smart::remote;

/// File extension of Atelier documents.
pub const EXTENSION: &str = "atelier";

const FORMAT: &str = "atelier";
/// Current format version, the newest this Atelier reads.
const VERSION: u32 = 3;
const MIMETYPE: &str = "image/x-atelier";
const MANIFEST: &str = "document.json";
const THUMBNAIL: &str = "thumbnail.png";
const PROFILE: &str = "color-profile.icc";
const PROOF_PROFILE: &str = "proof-profile.icc";
const EXIF: &str = "metadata/exif.bin";
const PSD_RESOURCES: &str = "metadata/photoshop-resources.bin";
const THUMBNAIL_SIZE: u32 = 256;
/// Read limit of the manifest and color profile (damaged files).
const MAX_SMALL_ENTRY: u64 = 64 << 20;

/// Whether `path` names an Atelier document (by its extension).
pub fn is_native(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// What is read first: whether the file is an Atelier document, and a
/// version this Atelier can read.
#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    /// Always "atelier".
    format: String,
    version: u32,
    /// Canvas size.
    width: u32,
    height: u32,
    /// Index of the active layer.
    active: usize,
    /// From the bottom up.
    layers: Vec<LayerEntry>,
    /// Ruler guides.
    #[serde(default)]
    guides: Vec<GuideEntry>,
    /// Saved selections (alpha channels), canvas-sized.
    #[serde(default)]
    channels: Vec<ChannelEntry>,
    /// Paths panel: the saved paths, the Work Path (canvas coordinates).
    #[serde(default)]
    paths: Vec<vectors::SavedPathEntry>,
    #[serde(default)]
    work_path: Option<vectors::PathEntry>,
    /// View › Proof Setup (Proof Colors and Gamut Warning are views of a
    /// session, not saved).
    #[serde(default)]
    proof: Option<ProofEntry>,
    /// What the document's first file held beside its layers, given back
    /// when saved in that format: archive paths of the EXIF and of the
    /// Photoshop image resources (see [`resources_bytes`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exif: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    photoshop_resources: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ProofEntry {
    intent: crate::cms::Intent,
    black_point_compensation: bool,
    paper_color: bool,
    black_ink: bool,
}

#[derive(Serialize, Deserialize)]
struct GuideEntry {
    /// "horizontal" (at a y) or "vertical" (at an x).
    orientation: OrientationEntry,
    position: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum OrientationEntry {
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize)]
struct ChannelEntry {
    name: String,
    /// Archive path of its gray pixels.
    image: String,
}

fn one() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize)]
struct LayerEntry {
    name: String,
    #[serde(default)]
    kind: KindEntry,
    /// Archive path of the pixels; none for a layer without pixels (0 × 0).
    image: Option<String>,
    width: u32,
    height: u32,
    /// Top-left corner in the canvas (may be outside it).
    x: i64,
    y: i64,
    visible: bool,
    /// 0 to 1.
    opacity: f32,
    /// Fill opacity, 0 to 1.
    #[serde(default = "one")]
    fill: f32,
    locked: bool,
    blend: Blend,
    /// Number of groups it is in (its group is the first layer above it
    /// that is less deep).
    #[serde(default)]
    depth: u32,
    /// Clipped to the layer below.
    #[serde(default)]
    clipped: bool,
    /// Layers with the same link move together.
    #[serde(default)]
    link: Option<u64>,
    #[serde(default)]
    mask: Option<MaskEntry>,
    stroke: StrokeEntry,
    shadow: ShadowEntry,
    #[serde(default)]
    inner_shadow: Option<ShadowEntry>,
    #[serde(default)]
    outer_glow: Option<GlowEntry>,
    #[serde(default)]
    inner_glow: Option<GlowEntry>,
    #[serde(default)]
    color_overlay: Option<OverlayEntry>,
    #[serde(default)]
    gradient_overlay: Option<GradientFxEntry>,
    #[serde(default)]
    bevel: Option<BevelEntry>,
    /// Editable text the pixels are the rendering of.
    text: Option<TextEntry>,
    /// Shape the pixels are the rendering of.
    #[serde(default)]
    shape: Option<vectors::ShapeEntry>,
    /// A group that is an artboard (its corner: the layer's position).
    #[serde(default)]
    artboard: Option<ArtboardEntry>,
    /// Smart object the pixels are the rendering of.
    #[serde(default)]
    smart: Option<smart::SmartEntry>,
}

#[derive(Serialize, Deserialize)]
struct ArtboardEntry {
    width: u32,
    height: u32,
    background: ArtboardBackgroundEntry,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum ArtboardBackgroundEntry {
    White,
    Black,
    Transparent,
    Other { color: [u8; 3] },
}

#[derive(Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum KindEntry {
    #[default]
    Pixels,
    Group {
        expanded: bool,
    },
    Adjustment {
        adjustment: AdjustmentEntry,
    },
    Fill {
        fill: FillEntry,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum Blend {
    PassThrough,
    Normal,
    Dissolve,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

/// Both ways between the modes and their names in the file.
const BLENDS: [(BlendMode, Blend); 28] = [
    (BlendMode::PassThrough, Blend::PassThrough),
    (BlendMode::Normal, Blend::Normal),
    (BlendMode::Dissolve, Blend::Dissolve),
    (BlendMode::Darken, Blend::Darken),
    (BlendMode::Multiply, Blend::Multiply),
    (BlendMode::ColorBurn, Blend::ColorBurn),
    (BlendMode::LinearBurn, Blend::LinearBurn),
    (BlendMode::DarkerColor, Blend::DarkerColor),
    (BlendMode::Lighten, Blend::Lighten),
    (BlendMode::Screen, Blend::Screen),
    (BlendMode::ColorDodge, Blend::ColorDodge),
    (BlendMode::LinearDodge, Blend::LinearDodge),
    (BlendMode::LighterColor, Blend::LighterColor),
    (BlendMode::Overlay, Blend::Overlay),
    (BlendMode::SoftLight, Blend::SoftLight),
    (BlendMode::HardLight, Blend::HardLight),
    (BlendMode::VividLight, Blend::VividLight),
    (BlendMode::LinearLight, Blend::LinearLight),
    (BlendMode::PinLight, Blend::PinLight),
    (BlendMode::HardMix, Blend::HardMix),
    (BlendMode::Difference, Blend::Difference),
    (BlendMode::Exclusion, Blend::Exclusion),
    (BlendMode::Subtract, Blend::Subtract),
    (BlendMode::Divide, Blend::Divide),
    (BlendMode::Hue, Blend::Hue),
    (BlendMode::Saturation, Blend::Saturation),
    (BlendMode::Color, Blend::Color),
    (BlendMode::Luminosity, Blend::Luminosity),
];

fn blend_entry(mode: BlendMode) -> Blend {
    BLENDS
        .iter()
        .find(|(m, _)| *m == mode)
        .map_or(Blend::Normal, |b| b.1)
}

fn blend_mode(entry: Blend) -> BlendMode {
    BLENDS
        .iter()
        .find(|(_, b)| *b as u8 == entry as u8)
        .map_or(BlendMode::Normal, |b| b.0)
}

fn normal() -> Blend {
    Blend::Normal
}

/// Colors are non-premultiplied sRGB RGBA, sizes in pixels.
#[derive(Serialize, Deserialize)]
struct StrokeEntry {
    enabled: bool,
    color: [u8; 4],
    width: f32,
    #[serde(default = "one")]
    opacity: f32,
    #[serde(default = "normal")]
    blend: Blend,
}

/// Drop and inner shadow. (Version 1's drop shadow blended normally.)
#[derive(Serialize, Deserialize)]
struct ShadowEntry {
    enabled: bool,
    color: [u8; 4],
    opacity: f32,
    dx: f32,
    dy: f32,
    blur: f32,
    #[serde(default = "normal")]
    blend: Blend,
}

#[derive(Serialize, Deserialize)]
struct GlowEntry {
    enabled: bool,
    color: [u8; 4],
    opacity: f32,
    size: f32,
    spread: f32,
    blend: Blend,
}

#[derive(Serialize, Deserialize)]
struct OverlayEntry {
    enabled: bool,
    color: [u8; 4],
    opacity: f32,
    blend: Blend,
}

#[derive(Serialize, Deserialize)]
struct GradientFxEntry {
    enabled: bool,
    fill: GradientFillEntry,
    opacity: f32,
    blend: Blend,
}

#[derive(Serialize, Deserialize)]
struct BevelEntry {
    enabled: bool,
    depth: f32,
    size: f32,
    soften: f32,
    angle: f32,
    altitude: f32,
    up: bool,
    highlight: [u8; 4],
    highlight_opacity: f32,
    highlight_blend: Blend,
    shadow: [u8; 4],
    shadow_opacity: f32,
    shadow_blend: Blend,
}

#[derive(Serialize, Deserialize)]
struct MaskEntry {
    /// Archive path of the gray pixels; none for an empty mask.
    image: Option<String>,
    width: u32,
    height: u32,
    /// Top-left corner in the canvas.
    x: i64,
    y: i64,
    /// Value outside the image: 0 hides, 255 shows.
    default: u8,
    enabled: bool,
    /// Moves with the layer.
    linked: bool,
    density: f32,
    feather: f32,
}

/// A gradient: its stops (version 1 and the first files of version 2
/// had only `start` and `end`, still written for them).
#[derive(Serialize, Deserialize)]
struct GradientEntry {
    start: [u8; 4],
    end: [u8; 4],
    #[serde(default)]
    colors: Vec<ColorStopEntry>,
    #[serde(default)]
    opacities: Vec<OpacityStopEntry>,
}

/// Location and midpoint 0..1; layers' gradients have fixed colors.
#[derive(Serialize, Deserialize)]
struct ColorStopEntry {
    location: f32,
    midpoint: f32,
    color: [u8; 3],
}

#[derive(Serialize, Deserialize)]
struct OpacityStopEntry {
    location: f32,
    midpoint: f32,
    opacity: f32,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum StyleEntry {
    Linear,
    Radial,
    Angle,
    Reflected,
    Diamond,
}

#[derive(Serialize, Deserialize)]
struct GradientFillEntry {
    gradient: GradientEntry,
    style: StyleEntry,
    angle: f32,
    scale: f32,
    reverse: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum FillEntry {
    Solid { color: [u8; 3] },
    Gradient(GradientFillEntry),
}

#[derive(Serialize, Deserialize)]
struct LevelsEntry {
    in_black: u8,
    in_white: u8,
    gamma: f32,
    out_black: u8,
    out_white: u8,
}

/// Settings of an adjustment layer; curves as their points.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum AdjustmentEntry {
    BrightnessContrast {
        brightness: f32,
        contrast: f32,
    },
    Levels {
        rgb: LevelsEntry,
        channels: [LevelsEntry; 3],
    },
    Curves {
        rgb: Vec<[f32; 2]>,
        channels: [Vec<[f32; 2]>; 3],
    },
    Vibrance {
        vibrance: f32,
        saturation: f32,
    },
    HueSaturation {
        hue: f32,
        saturation: f32,
        lightness: f32,
        colorize: bool,
    },
    ColorBalance {
        shadows: [f32; 3],
        midtones: [f32; 3],
        highlights: [f32; 3],
        preserve_luminosity: bool,
    },
    BlackWhite {
        weights: [f32; 6],
        tint: bool,
        hue: f32,
        saturation: f32,
    },
    PhotoFilter {
        color: [u8; 3],
        density: f32,
        preserve_luminosity: bool,
    },
    Invert,
    Posterize {
        levels: u8,
    },
    Threshold {
        level: u8,
    },
    GradientMap {
        gradient: GradientEntry,
        reverse: bool,
    },
}

/// A text layer: its characters by runs of one style, its paragraphs, its
/// box and transformation. The first version 2 files had a single style
/// (`family` … `align`), still written for them (the first character's and
/// paragraph's).
#[derive(Serialize, Deserialize)]
struct TextEntry {
    text: String,
    /// Empty: the default sans-serif font.
    family: String,
    size: f32,
    color: [u8; 4],
    bold: bool,
    italic: bool,
    align: Align,
    /// Lengths in UTF-8 bytes of `text`.
    #[serde(default)]
    runs: Vec<RunEntry>,
    #[serde(default)]
    paragraphs: Vec<ParagraphEntry>,
    /// Paragraph text: width and height of its box.
    #[serde(default)]
    frame: Option<[f32; 2]>,
    #[serde(default)]
    antialias: AntiAliasEntry,
    /// (x, y) → (a·x + b·y, c·x + d·y).
    #[serde(default = "identity")]
    transform: [f32; 4],
    /// Origin of the text in the layer's pixels.
    #[serde(default)]
    anchor: Option<[f32; 2]>,
}

fn identity() -> [f32; 4] {
    text::IDENTITY
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct RunEntry {
    len: usize,
    family: String,
    weight: u16,
    italic: bool,
    stretch: u16,
    size: f32,
    /// `None`: auto.
    leading: Option<f32>,
    tracking: f32,
    kerning: KerningEntry,
    horizontal_scale: f32,
    vertical_scale: f32,
    baseline_shift: f32,
    color: [u8; 4],
    faux_bold: bool,
    faux_italic: bool,
    all_caps: bool,
    small_caps: bool,
    underline: bool,
    strikethrough: bool,
}

impl Default for RunEntry {
    fn default() -> Self {
        Self::of(0, &CharStyle::default())
    }
}

impl RunEntry {
    fn of(len: usize, s: &CharStyle) -> Self {
        Self {
            len,
            family: s.family.clone(),
            weight: s.weight,
            italic: s.italic,
            stretch: s.stretch,
            size: s.size,
            leading: s.leading,
            tracking: s.tracking,
            kerning: match s.kerning {
                Kerning::Metrics => KerningEntry::Metrics,
                Kerning::Zero => KerningEntry::Zero,
            },
            horizontal_scale: s.horizontal_scale,
            vertical_scale: s.vertical_scale,
            baseline_shift: s.baseline_shift,
            color: s.color,
            faux_bold: s.faux_bold,
            faux_italic: s.faux_italic,
            all_caps: s.all_caps,
            small_caps: s.small_caps,
            underline: s.underline,
            strikethrough: s.strikethrough,
        }
    }

    fn style(self) -> CharStyle {
        CharStyle {
            family: self.family,
            weight: self.weight,
            italic: self.italic,
            stretch: self.stretch,
            size: self.size,
            leading: self.leading,
            tracking: self.tracking,
            kerning: match self.kerning {
                KerningEntry::Metrics => Kerning::Metrics,
                KerningEntry::Zero => Kerning::Zero,
            },
            horizontal_scale: self.horizontal_scale,
            vertical_scale: self.vertical_scale,
            baseline_shift: self.baseline_shift,
            color: self.color,
            faux_bold: self.faux_bold,
            faux_italic: self.faux_italic,
            all_caps: self.all_caps,
            small_caps: self.small_caps,
            underline: self.underline,
            strikethrough: self.strikethrough,
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum KerningEntry {
    #[default]
    Metrics,
    Zero,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum AntiAliasEntry {
    None,
    #[default]
    Sharp,
    Smooth,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct ParagraphEntry {
    align: ParagraphAlign,
    indent_left: f32,
    indent_right: f32,
    indent_first: f32,
    space_before: f32,
    space_after: f32,
}

impl Default for ParagraphEntry {
    fn default() -> Self {
        Self::of(&ParagraphStyle::default())
    }
}

impl ParagraphEntry {
    fn of(p: &ParagraphStyle) -> Self {
        Self {
            align: match p.align {
                TextAlign::Left => ParagraphAlign::Left,
                TextAlign::Center => ParagraphAlign::Center,
                TextAlign::Right => ParagraphAlign::Right,
                TextAlign::Justify => ParagraphAlign::Justify,
            },
            indent_left: p.indent_left,
            indent_right: p.indent_right,
            indent_first: p.indent_first,
            space_before: p.space_before,
            space_after: p.space_after,
        }
    }

    fn style(&self) -> ParagraphStyle {
        ParagraphStyle {
            align: match self.align {
                ParagraphAlign::Left => TextAlign::Left,
                ParagraphAlign::Center => TextAlign::Center,
                ParagraphAlign::Right => TextAlign::Right,
                ParagraphAlign::Justify => TextAlign::Justify,
            },
            indent_left: self.indent_left,
            indent_right: self.indent_right,
            indent_first: self.indent_first,
            space_before: self.space_before,
            space_after: self.space_after,
        }
    }
}

/// The single alignment of the first version 2 files.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Align {
    Left,
    Center,
    Right,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum ParagraphAlign {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

fn text_entry(t: &TextData) -> TextEntry {
    let first = t.style_at(0);
    TextEntry {
        text: t.text.clone(),
        family: first.family.clone(),
        size: first.size,
        color: first.color,
        bold: first.weight >= 600,
        italic: first.italic,
        align: match t.paragraphs[0].align {
            TextAlign::Left | TextAlign::Justify => Align::Left,
            TextAlign::Center => Align::Center,
            TextAlign::Right => Align::Right,
        },
        runs: t
            .runs
            .iter()
            .map(|r| RunEntry::of(r.len, &r.style))
            .collect(),
        paragraphs: t.paragraphs.iter().map(ParagraphEntry::of).collect(),
        frame: t.frame.map(|(w, h)| [w, h]),
        antialias: match t.antialias {
            AntiAlias::None => AntiAliasEntry::None,
            AntiAlias::Sharp => AntiAliasEntry::Sharp,
            AntiAlias::Smooth => AntiAliasEntry::Smooth,
        },
        transform: t.transform,
        anchor: Some([t.anchor.0, t.anchor.1]),
    }
}

/// The text of a layer whose pixels are `image`.
fn text_data(t: TextEntry, image: &RgbaImage) -> TextData {
    let mut data = if t.runs.is_empty() {
        // A single style.
        let style = CharStyle {
            family: t.family,
            size: finite_or(t.size, CharStyle::default().size).max(1.0),
            color: t.color,
            weight: if t.bold { 700 } else { 400 },
            italic: t.italic,
            ..Default::default()
        };
        let mut data = TextData::plain(&t.text, style);
        let align = match t.align {
            Align::Left => TextAlign::Left,
            Align::Center => TextAlign::Center,
            Align::Right => TextAlign::Right,
        };
        for p in &mut data.paragraphs {
            p.align = align;
        }
        data
    } else {
        TextData {
            text: t.text,
            runs: t
                .runs
                .into_iter()
                .map(|r| text::Run {
                    len: r.len,
                    style: r.style(),
                })
                .collect(),
            paragraphs: t.paragraphs.iter().map(ParagraphEntry::style).collect(),
            ..Default::default()
        }
    };
    data.frame = t.frame.map(|[w, h]| (w, h));
    data.antialias = match t.antialias {
        AntiAliasEntry::None => AntiAlias::None,
        AntiAliasEntry::Sharp => AntiAlias::Sharp,
        AntiAliasEntry::Smooth => AntiAlias::Smooth,
    };
    data.transform = t.transform;
    data.anchor = match t.anchor {
        Some([x, y]) => (x, y),
        // Where the first renderer put the first line (left, center or
        // right of the box of the lines, a margin of 30 % of the size around
        // it), about on its baseline.
        None => {
            let style = data.style_at(0);
            let pad = (style.size * 0.3).ceil();
            let w = image.width() as f32;
            let x = match data.paragraphs[0].align {
                TextAlign::Center => w / 2.0,
                TextAlign::Right => w - pad,
                TextAlign::Left | TextAlign::Justify => pad,
            };
            (x, pad + style.size * 0.95)
        }
    };
    data.repair();
    data
}

/// Writes `doc` as an Atelier document, to be kept in `folder` (linked
/// files are also recorded relative to it).
pub(super) fn write(
    doc: &Document,
    out: impl Write + Seek,
    folder: Option<&Path>,
) -> Result<(), FileError> {
    let layers = doc.render_layers();
    // The layers' pixels then their masks (as RGBA or gray PNGs), encoded on
    // the other cores while this one flattens.
    enum Pixels<'a> {
        Color(&'a RgbaImage),
        Gray(&'a GrayImage),
    }
    let images: Vec<Pixels> = layers
        .iter()
        .map(|l| Pixels::Color(l.image()))
        .chain(
            layers
                .iter()
                .filter_map(|l| l.mask.as_ref())
                .map(|m| Pixels::Gray(m.image())),
        )
        .chain(doc.channels.iter().map(|c| Pixels::Gray(&c.mask)))
        .collect();
    let (pngs, thumbnail) = par_map(
        &images,
        |image| match image {
            Pixels::Color(i) => (i.width() > 0 && i.height() > 0).then(|| encode_png(i)),
            Pixels::Gray(i) => (i.width() > 0 && i.height() > 0).then(|| encode_gray(i)),
        },
        || encode_png(&thumbnail(&doc.flatten())),
    );
    let mut pngs = pngs.into_iter();
    let mut entries = Vec::with_capacity(layers.len());
    let mut files = Vec::new();
    let store = |files: &mut Vec<(String, Vec<u8>)>,
                 name: String,
                 png: Option<Result<Vec<u8>, FileError>>| {
        png.map(|png| {
            png.map(|data| {
                files.push((name.clone(), data));
                name
            })
        })
        .transpose()
    };
    for (i, layer) in layers.iter().enumerate() {
        let image = store(&mut files, format!("layers/{i}.png"), pngs.next().flatten())?;
        entries.push(layer_entry(layer, image));
    }
    for (i, layer) in layers.iter().enumerate() {
        if layer.mask.is_some() {
            let image = store(&mut files, format!("masks/{i}.png"), pngs.next().flatten())?;
            if let Some(m) = &mut entries[i].mask {
                m.image = image;
            }
        }
    }
    let mut channels = Vec::with_capacity(doc.channels.len());
    for (i, c) in doc.channels.iter().enumerate() {
        let name = format!("channels/{i}.png");
        if let Some(image) = store(&mut files, name, pngs.next().flatten())? {
            channels.push(ChannelEntry {
                name: c.name.clone(),
                image,
            });
        }
    }
    smart::write_all(&layers, &mut entries, &mut files, folder)?;
    let manifest = Manifest {
        format: FORMAT.into(),
        version: VERSION,
        width: doc.width(),
        height: doc.height(),
        active: doc.active,
        layers: entries,
        guides: doc
            .guides
            .iter()
            .map(|g| GuideEntry {
                orientation: match g.orientation {
                    Orientation::Horizontal => OrientationEntry::Horizontal,
                    Orientation::Vertical => OrientationEntry::Vertical,
                },
                position: g.position,
            })
            .collect(),
        channels,
        paths: doc.paths.iter().map(vectors::saved_path_entry).collect(),
        work_path: doc.work_path.as_ref().map(vectors::path_entry),
        proof: (super::ProofSetup {
            colors: false,
            gamut: false,
            ..doc.proof.clone()
        } != super::ProofSetup::default())
        .then_some(ProofEntry {
            intent: doc.proof.intent,
            black_point_compensation: doc.proof.bpc,
            paper_color: doc.proof.paper,
            black_ink: doc.proof.ink,
        }),
        exif: doc.file_metadata.exif.is_some().then(|| EXIF.into()),
        photoshop_resources: (!doc.file_metadata.psd_resources.is_empty())
            .then(|| PSD_RESOURCES.into()),
    };

    let mut zip = ZipWriter::new(out);
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).map_err(zip_error)?;
    zip.write_all(MIMETYPE.as_bytes())?;
    let deflated = stored.compression_method(CompressionMethod::Deflated);
    zip.start_file(MANIFEST, deflated).map_err(zip_error)?;
    serde_json::to_writer_pretty(&mut zip, &manifest).map_err(std::io::Error::from)?;
    files.push((THUMBNAIL.into(), thumbnail?));
    if let Some(profile) = &doc.icc_profile {
        files.push((PROFILE.into(), profile.to_vec()));
    }
    if let Some(proof) = &doc.proof.custom {
        files.push((PROOF_PROFILE.into(), proof.icc().to_vec()));
    }
    if let Some(exif) = &doc.file_metadata.exif {
        files.push((EXIF.into(), exif.to_vec()));
    }
    if !doc.file_metadata.psd_resources.is_empty() {
        let data = resources_bytes(&doc.file_metadata.psd_resources);
        files.push((PSD_RESOURCES.into(), data));
    }
    for (name, data) in files {
        let large = data.len() as u64 >= u32::MAX as u64;
        zip.start_file(name, stored.large_file(large))
            .map_err(zip_error)?;
        zip.write_all(&data)?;
    }
    zip.finish().map_err(zip_error)?;
    Ok(())
}

fn gradient_entry(g: &Gradient) -> GradientEntry {
    let end = |t: f32| {
        let [r, gr, b, a] = g.sample(t, [0; 3], [255; 3], true);
        [r, gr, b, a * 255.0].map(|v| v.round().clamp(0.0, 255.0) as u8)
    };
    GradientEntry {
        start: end(0.0),
        end: end(1.0),
        colors: g
            .colors
            .iter()
            .map(|s| ColorStopEntry {
                location: s.location,
                midpoint: s.midpoint,
                color: s.color.resolve([0; 3], [255; 3]),
            })
            .collect(),
        opacities: g
            .opacities
            .iter()
            .map(|s| OpacityStopEntry {
                location: s.location,
                midpoint: s.midpoint,
                opacity: s.opacity,
            })
            .collect(),
    }
}

fn gradient_of(g: &GradientEntry) -> Gradient {
    let unit = |v: f32, default: f32| finite_or(v, default).clamp(0.0, 1.0);
    let mut colors: Vec<ColorStop> = g
        .colors
        .iter()
        .map(|s| ColorStop {
            location: unit(s.location, 0.0),
            midpoint: unit(s.midpoint, 0.5),
            color: StopColor::Rgb(s.color),
        })
        .collect();
    let mut opacities: Vec<OpacityStop> = g
        .opacities
        .iter()
        .map(|s| OpacityStop {
            location: unit(s.location, 0.0),
            midpoint: unit(s.midpoint, 0.5),
            opacity: unit(s.opacity, 1.0),
        })
        .collect();
    // From its two colors (older files).
    if colors.is_empty() {
        colors = [(0.0, g.start), (1.0, g.end)]
            .map(|(location, [r, gr, b, _])| ColorStop {
                location,
                midpoint: 0.5,
                color: StopColor::Rgb([r, gr, b]),
            })
            .to_vec();
    }
    if opacities.is_empty() {
        opacities = [(0.0, g.start[3]), (1.0, g.end[3])]
            .map(|(location, a)| OpacityStop {
                location,
                midpoint: 0.5,
                opacity: a as f32 / 255.0,
            })
            .to_vec();
    }
    Gradient { colors, opacities }
}

const STYLES: [(Shape, StyleEntry); 5] = [
    (Shape::Linear, StyleEntry::Linear),
    (Shape::Radial, StyleEntry::Radial),
    (Shape::Angle, StyleEntry::Angle),
    (Shape::Reflected, StyleEntry::Reflected),
    (Shape::Diamond, StyleEntry::Diamond),
];

fn gradient_fill_entry(f: &GradientFill) -> GradientFillEntry {
    GradientFillEntry {
        gradient: gradient_entry(&f.gradient),
        style: STYLES
            .iter()
            .find(|s| s.0 == f.style)
            .map_or(StyleEntry::Linear, |s| s.1),
        angle: f.angle,
        scale: f.scale,
        reverse: f.reverse,
    }
}

fn gradient_fill_of(f: &GradientFillEntry) -> GradientFill {
    GradientFill {
        gradient: gradient_of(&f.gradient),
        style: STYLES
            .iter()
            .find(|s| s.1 as u8 == f.style as u8)
            .map_or(Shape::Linear, |s| s.0),
        angle: finite_or(f.angle, 90.0),
        scale: finite_or(f.scale, 1.0).clamp(0.1, 10.0),
        reverse: f.reverse,
    }
}

fn levels_entry(c: &LevelsChannel) -> LevelsEntry {
    LevelsEntry {
        in_black: c.in_black,
        in_white: c.in_white,
        gamma: c.gamma,
        out_black: c.out_black,
        out_white: c.out_white,
    }
}

fn levels_of(e: &LevelsEntry) -> LevelsChannel {
    LevelsChannel {
        in_black: e.in_black,
        in_white: e.in_white,
        gamma: finite_or(e.gamma, 1.0)
            .clamp(crate::pixel_ops::MIN_GAMMA, crate::pixel_ops::MAX_GAMMA),
        out_black: e.out_black,
        out_white: e.out_white,
    }
}

fn adjustment_entry(a: &Adjustment) -> AdjustmentEntry {
    match a {
        Adjustment::BrightnessContrast(bc) => AdjustmentEntry::BrightnessContrast {
            brightness: bc.brightness,
            contrast: bc.contrast,
        },
        Adjustment::Levels(lv) => AdjustmentEntry::Levels {
            rgb: levels_entry(&lv.rgb),
            channels: lv.channels.each_ref().map(levels_entry),
        },
        Adjustment::Curves(cv) => AdjustmentEntry::Curves {
            rgb: cv.rgb.points().to_vec(),
            channels: cv.channels.each_ref().map(|c| c.points().to_vec()),
        },
        Adjustment::Vibrance(v) => AdjustmentEntry::Vibrance {
            vibrance: v.vibrance,
            saturation: v.saturation,
        },
        Adjustment::HueSaturation(hs) => AdjustmentEntry::HueSaturation {
            hue: hs.hue,
            saturation: hs.saturation,
            lightness: hs.lightness,
            colorize: hs.colorize,
        },
        Adjustment::ColorBalance(cb) => AdjustmentEntry::ColorBalance {
            shadows: cb.shadows,
            midtones: cb.midtones,
            highlights: cb.highlights,
            preserve_luminosity: cb.preserve_luminosity,
        },
        Adjustment::BlackWhite(bw) => AdjustmentEntry::BlackWhite {
            weights: bw.weights,
            tint: bw.tint,
            hue: bw.hue,
            saturation: bw.saturation,
        },
        Adjustment::PhotoFilter(f) => AdjustmentEntry::PhotoFilter {
            color: f.color,
            density: f.density,
            preserve_luminosity: f.preserve_luminosity,
        },
        Adjustment::Invert => AdjustmentEntry::Invert,
        Adjustment::Posterize(p) => AdjustmentEntry::Posterize { levels: p.levels },
        Adjustment::Threshold(t) => AdjustmentEntry::Threshold { level: t.level },
        Adjustment::GradientMap(g) => AdjustmentEntry::GradientMap {
            gradient: gradient_entry(&g.gradient),
            reverse: g.reverse,
        },
    }
}

fn adjustment_of(e: &AdjustmentEntry) -> Result<Adjustment, FileError> {
    let f = |v: f32, lo: f32, hi: f32| finite_or(v, 0.0).clamp(lo, hi);
    let curve = |points: &[[f32; 2]]| Curve::from_points(points).ok_or(FileError::Damaged);
    Ok(match e {
        AdjustmentEntry::BrightnessContrast {
            brightness,
            contrast,
        } => Adjustment::BrightnessContrast(BrightnessContrast {
            brightness: f(*brightness, -1.5, 1.5),
            contrast: f(*contrast, -0.5, 1.0),
        }),
        AdjustmentEntry::Levels { rgb, channels } => Adjustment::Levels(Levels {
            rgb: levels_of(rgb),
            channels: channels.each_ref().map(levels_of),
        }),
        AdjustmentEntry::Curves { rgb, channels } => Adjustment::Curves(Curves {
            rgb: curve(rgb)?,
            channels: [
                curve(&channels[0])?,
                curve(&channels[1])?,
                curve(&channels[2])?,
            ],
        }),
        AdjustmentEntry::Vibrance {
            vibrance,
            saturation,
        } => Adjustment::Vibrance(Vibrance {
            vibrance: f(*vibrance, -1.0, 1.0),
            saturation: f(*saturation, -1.0, 1.0),
        }),
        AdjustmentEntry::HueSaturation {
            hue,
            saturation,
            lightness,
            colorize,
        } => Adjustment::HueSaturation(HueSaturation {
            hue: f(*hue, -180.0, 360.0),
            saturation: f(*saturation, -1.0, 1.0),
            lightness: f(*lightness, -1.0, 1.0),
            colorize: *colorize,
        }),
        AdjustmentEntry::ColorBalance {
            shadows,
            midtones,
            highlights,
            preserve_luminosity,
        } => Adjustment::ColorBalance(ColorBalance {
            shadows: shadows.map(|v| f(v, -1.0, 1.0)),
            midtones: midtones.map(|v| f(v, -1.0, 1.0)),
            highlights: highlights.map(|v| f(v, -1.0, 1.0)),
            preserve_luminosity: *preserve_luminosity,
        }),
        AdjustmentEntry::BlackWhite {
            weights,
            tint,
            hue,
            saturation,
        } => Adjustment::BlackWhite(BlackWhite {
            weights: weights.map(|v| f(v, -200.0, 300.0)),
            tint: *tint,
            hue: f(*hue, 0.0, 360.0),
            saturation: f(*saturation, 0.0, 1.0),
        }),
        AdjustmentEntry::PhotoFilter {
            color,
            density,
            preserve_luminosity,
        } => Adjustment::PhotoFilter(PhotoFilter {
            color: *color,
            density: f(*density, 0.0, 1.0),
            preserve_luminosity: *preserve_luminosity,
        }),
        AdjustmentEntry::Invert => Adjustment::Invert,
        AdjustmentEntry::Posterize { levels } => Adjustment::Posterize(Posterize {
            levels: (*levels).max(2),
        }),
        AdjustmentEntry::Threshold { level } => Adjustment::Threshold(Threshold {
            level: (*level).max(1),
        }),
        AdjustmentEntry::GradientMap { gradient, reverse } => {
            Adjustment::GradientMap(GradientMap {
                gradient: gradient_of(gradient),
                reverse: *reverse,
            })
        }
    })
}

/// An adjustment's settings as JSON, in the form adjustment layers are
/// saved in (the Actions panel records them so).
pub fn adjustment_to_json(a: &Adjustment) -> serde_json::Value {
    serde_json::to_value(adjustment_entry(a)).unwrap_or_default()
}

/// [`adjustment_to_json`]'s reverse; `None` if it isn't one.
pub fn adjustment_from_json(value: &serde_json::Value) -> Option<Adjustment> {
    adjustment_of(&AdjustmentEntry::deserialize(value).ok()?).ok()
}

fn shadow_entry(s: &ShadowFx) -> ShadowEntry {
    ShadowEntry {
        enabled: s.enabled,
        color: s.color,
        opacity: s.opacity,
        dx: s.dx,
        dy: s.dy,
        blur: s.blur,
        blend: blend_entry(s.blend),
    }
}

fn glow_entry(g: &GlowFx) -> GlowEntry {
    GlowEntry {
        enabled: g.enabled,
        color: g.color,
        opacity: g.opacity,
        size: g.size,
        spread: g.spread,
        blend: blend_entry(g.blend),
    }
}

fn layer_entry(layer: &super::Layer, image: Option<String>) -> LayerEntry {
    let fx = &layer.effects;
    let kind = match &layer.kind {
        LayerKind::Pixels => KindEntry::Pixels,
        LayerKind::Group { expanded } => KindEntry::Group {
            expanded: *expanded,
        },
        LayerKind::Adjustment(a) => KindEntry::Adjustment {
            adjustment: adjustment_entry(a),
        },
        LayerKind::Fill(Fill::Solid(color)) => KindEntry::Fill {
            fill: FillEntry::Solid { color: *color },
        },
        LayerKind::Fill(Fill::Gradient(g)) => KindEntry::Fill {
            fill: FillEntry::Gradient(gradient_fill_entry(g)),
        },
    };
    let mask = layer
        .mask
        .as_ref()
        .map(|m| smart::mask_entry(m, layer.offset));
    let b = &fx.bevel;
    LayerEntry {
        name: layer.name.clone(),
        kind,
        image,
        width: layer.image().width(),
        height: layer.image().height(),
        x: layer.offset.0,
        y: layer.offset.1,
        visible: layer.visible,
        opacity: layer.opacity,
        fill: layer.fill,
        locked: layer.locked,
        blend: blend_entry(layer.blend),
        depth: layer.depth,
        clipped: layer.clipped,
        link: layer.link,
        mask,
        stroke: StrokeEntry {
            enabled: fx.stroke.enabled,
            color: fx.stroke.color,
            width: fx.stroke.width,
            opacity: fx.stroke.opacity,
            blend: blend_entry(fx.stroke.blend),
        },
        shadow: shadow_entry(&fx.shadow),
        inner_shadow: Some(shadow_entry(&fx.inner_shadow)),
        outer_glow: Some(glow_entry(&fx.outer_glow)),
        inner_glow: Some(glow_entry(&fx.inner_glow)),
        color_overlay: Some(OverlayEntry {
            enabled: fx.color_overlay.enabled,
            color: fx.color_overlay.color,
            opacity: fx.color_overlay.opacity,
            blend: blend_entry(fx.color_overlay.blend),
        }),
        gradient_overlay: Some(GradientFxEntry {
            enabled: fx.gradient_overlay.enabled,
            fill: gradient_fill_entry(&fx.gradient_overlay.fill),
            opacity: fx.gradient_overlay.opacity,
            blend: blend_entry(fx.gradient_overlay.blend),
        }),
        bevel: Some(BevelEntry {
            enabled: b.enabled,
            depth: b.depth,
            size: b.size,
            soften: b.soften,
            angle: b.angle,
            altitude: b.altitude,
            up: b.up,
            highlight: b.highlight,
            highlight_opacity: b.highlight_opacity,
            highlight_blend: blend_entry(b.highlight_blend),
            shadow: b.shadow,
            shadow_opacity: b.shadow_opacity,
            shadow_blend: blend_entry(b.shadow_blend),
        }),
        text: layer.text.as_ref().map(text_entry),
        shape: layer.shape.as_deref().map(vectors::shape_entry),
        artboard: layer.artboard.map(|a| ArtboardEntry {
            width: a.size.0,
            height: a.size.1,
            background: match a.background {
                ArtboardBackground::White => ArtboardBackgroundEntry::White,
                ArtboardBackground::Black => ArtboardBackgroundEntry::Black,
                ArtboardBackground::Transparent => ArtboardBackgroundEntry::Transparent,
                ArtboardBackground::Custom(color) => ArtboardBackgroundEntry::Other { color },
            },
        }),
        // Filled in by `smart::write_all`.
        smart: None,
    }
}

/// Reads an Atelier document.
pub(super) fn read(path: &Path) -> Result<Document, FileError> {
    read_archive(&mut open(path)?, Some(path))
}

/// An Atelier document held in memory (a smart object's contents).
fn read_bytes(data: &[u8]) -> Result<Document, FileError> {
    let mut zip = ZipArchive::new(Cursor::new(data)).map_err(|_| FileError::Damaged)?;
    read_archive(&mut zip, None)
}

/// [`read`] from an open archive, the document's file at `path`.
fn read_archive(
    zip: &mut ZipArchive<impl Read + Seek>,
    path: Option<&Path>,
) -> Result<Document, FileError> {
    let manifest = read_manifest(zip)?;
    check_size(manifest.width, manifest.height)?;
    if manifest.layers.is_empty() {
        return Err(FileError::Damaged);
    }
    // The pixels of each layer, then of each mask.
    let mut blobs = Vec::with_capacity(manifest.layers.len());
    let sizes = manifest
        .layers
        .iter()
        .map(|e| (&e.image, e.width, e.height, false))
        .chain(
            manifest
                .layers
                .iter()
                .filter_map(|e| e.mask.as_ref())
                .map(|m| (&m.image, m.width, m.height, true)),
        );
    for (image, w, h, gray) in sizes {
        if w > 4 * MAX_DIMENSION || h > 4 * MAX_DIMENSION {
            return Err(FileError::Damaged);
        }
        let blob = match image {
            Some(name) => Some(read_pixels(zip, name, (w, h), if gray { 1 } else { 4 })?),
            None if w == 0 || h == 0 => None,
            None => return Err(FileError::Damaged),
        };
        blobs.push((blob, w, h, gray));
    }
    enum Decoded {
        Color(RgbaImage),
        Gray(GrayImage),
    }
    let (images, ()) = par_map(
        &blobs,
        |(blob, w, h, gray)| match (blob, gray) {
            (Some(png), false) => decode_png(png, (*w, *h)).map(Decoded::Color),
            (Some(png), true) => decode_gray(png, (*w, *h)).map(Decoded::Gray),
            (None, false) => Ok(Decoded::Color(RgbaImage::new(*w, *h))),
            (None, true) => Ok(Decoded::Gray(GrayImage::new(*w, *h))),
        },
        || (),
    );
    let icc_profile = match zip.index_for_name(PROFILE) {
        Some(_) => Some(Arc::new(read_entry(zip, PROFILE, MAX_SMALL_ENTRY)?)),
        None => None,
    };
    let proof_profile = match zip.index_for_name(PROOF_PROFILE) {
        Some(_) => Some(Arc::new(read_entry(zip, PROOF_PROFILE, MAX_SMALL_ENTRY)?)),
        None => None,
    };
    let mut file_metadata = super::FileMetadata::default();
    if let Some(name) = &manifest.exif {
        file_metadata.exif = Some(Arc::new(read_entry(zip, name, MAX_SMALL_ENTRY)?));
    }
    if let Some(name) = &manifest.photoshop_resources {
        let data = read_entry(zip, name, MAX_SMALL_ENTRY)?;
        file_metadata.psd_resources = Arc::new(resources_of(&data).ok_or(FileError::Damaged)?);
    }

    let mut doc = Document::new(RgbaImage::new(0, 0), path.map(Path::to_path_buf));
    let folder = path.and_then(Path::parent);
    let mut contents = std::collections::HashMap::new();
    doc.layers.clear();
    doc.canvas = (manifest.width, manifest.height);
    let mut images = images.into_iter();
    let mut pixels = Vec::with_capacity(manifest.layers.len());
    for _ in &manifest.layers {
        match images.next() {
            Some(Ok(Decoded::Color(image))) => pixels.push(image),
            Some(Err(e)) => return Err(e),
            _ => return Err(FileError::Damaged),
        }
    }
    // Offsets far beyond any canvas are damage (and would overflow).
    let far = |x: i64, y: i64| {
        let limit = 4 * MAX_DIMENSION as u64;
        x.unsigned_abs() > limit || y.unsigned_abs() > limit
    };
    for (entry, image) in manifest.layers.into_iter().zip(pixels) {
        if far(entry.x, entry.y) || entry.mask.as_ref().is_some_and(|m| far(m.x, m.y)) {
            return Err(FileError::Damaged);
        }
        let fx = effects(&entry);
        let mut layer = doc.make_layer(entry.name, image, (entry.x, entry.y));
        layer.kind = match entry.kind {
            KindEntry::Pixels => LayerKind::Pixels,
            KindEntry::Group { expanded } => LayerKind::Group { expanded },
            KindEntry::Adjustment { adjustment } => {
                LayerKind::Adjustment(adjustment_of(&adjustment)?)
            }
            KindEntry::Fill {
                fill: FillEntry::Solid { color },
            } => LayerKind::Fill(Fill::Solid(color)),
            KindEntry::Fill {
                fill: FillEntry::Gradient(g),
            } => LayerKind::Fill(Fill::Gradient(gradient_fill_of(&g))),
        };
        layer.visible = entry.visible;
        layer.opacity = finite_or(entry.opacity, 1.0).clamp(0.0, 1.0);
        layer.fill = finite_or(entry.fill, 1.0).clamp(0.0, 1.0);
        layer.locked = entry.locked;
        layer.blend = blend_mode(entry.blend);
        if layer.blend == BlendMode::PassThrough && !layer.is_group() {
            layer.blend = BlendMode::Normal;
        }
        layer.depth = entry.depth;
        layer.clipped = entry.clipped;
        layer.link = entry.link;
        if let Some(m) = entry.mask {
            let Some(Ok(Decoded::Gray(image))) = images.next() else {
                return Err(FileError::Damaged);
            };
            layer.mask = Some(smart::mask_of(&m, image, layer.offset));
        }
        layer.effects = fx;
        layer.text = entry.text.map(|t| text_data(t, layer.image()));
        layer.shape = entry.shape.map(|s| Box::new(vectors::shape_of(&s)));
        if let Some(e) = entry.smart.filter(|_| layer.has_pixels()) {
            let s = smart::smart_of(e, zip, &mut contents, layer.offset, folder)?;
            layer.smart = Some(Box::new(s));
        }
        layer.artboard = entry
            .artboard
            .filter(|a| layer.is_group() && a.width > 0 && a.height > 0)
            .map(|a| Artboard {
                size: (a.width.min(MAX_DIMENSION), a.height.min(MAX_DIMENSION)),
                background: match a.background {
                    ArtboardBackgroundEntry::White => ArtboardBackground::White,
                    ArtboardBackgroundEntry::Black => ArtboardBackground::Black,
                    ArtboardBackgroundEntry::Transparent => ArtboardBackground::Transparent,
                    ArtboardBackgroundEntry::Other { color } => ArtboardBackground::Custom(color),
                },
            });
        doc.layers.push(layer);
    }
    doc.fix_depths();
    doc.guides = manifest
        .guides
        .iter()
        .filter(|g| g.position.is_finite())
        .map(|g| {
            let orientation = match g.orientation {
                OrientationEntry::Horizontal => Orientation::Horizontal,
                OrientationEntry::Vertical => Orientation::Vertical,
            };
            Guide::new(orientation, g.position)
        })
        .collect();
    let canvas = (manifest.width, manifest.height);
    for c in &manifest.channels {
        let png = read_pixels(zip, &c.image, canvas, 1)?;
        doc.channels.push(Channel {
            name: c.name.clone(),
            mask: Arc::new(decode_gray(&png, canvas)?),
        });
    }
    doc.paths = manifest.paths.iter().map(vectors::saved_path_of).collect();
    doc.work_path = manifest.work_path.as_ref().map(vectors::path_of);
    doc.active = manifest.active.min(doc.layers.len() - 1);
    doc.icc_profile = icc_profile;
    doc.file_metadata = file_metadata;
    if let Some(p) = &manifest.proof {
        doc.proof = super::ProofSetup {
            custom: proof_profile.and_then(crate::cms::Profile::from_icc),
            intent: p.intent,
            bpc: p.black_point_compensation,
            paper: p.paper_color,
            ink: p.black_ink,
            colors: false,
            gamut: false,
        };
    }
    doc.mark_saved();
    Ok(doc)
}

fn effects(e: &LayerEntry) -> Effects {
    let d = Effects::default();
    let shadow = |s: &ShadowEntry, d: ShadowFx| ShadowFx {
        enabled: s.enabled,
        color: s.color,
        opacity: finite_or(s.opacity, d.opacity).clamp(0.0, 1.0),
        dx: finite_or(s.dx, d.dx).clamp(-30000.0, 30000.0),
        dy: finite_or(s.dy, d.dy).clamp(-30000.0, 30000.0),
        blur: finite_or(s.blur, d.blur).clamp(0.0, 250.0),
        blend: blend_mode(s.blend),
    };
    let glow = |g: &Option<GlowEntry>, d: GlowFx| match g {
        Some(g) => GlowFx {
            enabled: g.enabled,
            color: g.color,
            opacity: finite_or(g.opacity, d.opacity).clamp(0.0, 1.0),
            size: finite_or(g.size, d.size).clamp(0.0, 250.0),
            spread: finite_or(g.spread, d.spread).clamp(0.0, 1.0),
            blend: blend_mode(g.blend),
        },
        None => d,
    };
    Effects {
        stroke: StrokeFx {
            enabled: e.stroke.enabled,
            color: e.stroke.color,
            width: finite_or(e.stroke.width, d.stroke.width).clamp(0.0, 250.0),
            opacity: finite_or(e.stroke.opacity, 1.0).clamp(0.0, 1.0),
            blend: blend_mode(e.stroke.blend),
        },
        shadow: shadow(&e.shadow, d.shadow),
        inner_shadow: e
            .inner_shadow
            .as_ref()
            .map_or(d.inner_shadow, |s| shadow(s, d.inner_shadow)),
        outer_glow: glow(&e.outer_glow, d.outer_glow),
        inner_glow: glow(&e.inner_glow, d.inner_glow),
        color_overlay: e
            .color_overlay
            .as_ref()
            .map_or(d.color_overlay, |o| OverlayFx {
                enabled: o.enabled,
                color: o.color,
                opacity: finite_or(o.opacity, 1.0).clamp(0.0, 1.0),
                blend: blend_mode(o.blend),
            }),
        gradient_overlay: e
            .gradient_overlay
            .as_ref()
            .map_or(d.gradient_overlay, |g| GradientFx {
                enabled: g.enabled,
                fill: gradient_fill_of(&g.fill),
                opacity: finite_or(g.opacity, 1.0).clamp(0.0, 1.0),
                blend: blend_mode(g.blend),
            }),
        bevel: e.bevel.as_ref().map_or(d.bevel, |b| {
            let db = d.bevel;
            BevelFx {
                enabled: b.enabled,
                depth: finite_or(b.depth, db.depth).clamp(0.0, 10.0),
                size: finite_or(b.size, db.size).clamp(0.0, 250.0),
                soften: finite_or(b.soften, db.soften).clamp(0.0, 16.0),
                angle: finite_or(b.angle, db.angle),
                altitude: finite_or(b.altitude, db.altitude).clamp(0.0, 90.0),
                up: b.up,
                highlight: b.highlight,
                highlight_opacity: finite_or(b.highlight_opacity, 0.75).clamp(0.0, 1.0),
                highlight_blend: blend_mode(b.highlight_blend),
                shadow: b.shadow,
                shadow_opacity: finite_or(b.shadow_opacity, 0.75).clamp(0.0, 1.0),
                shadow_blend: blend_mode(b.shadow_blend),
            }
        }),
    }
}

fn finite_or(v: f32, default: f32) -> f32 {
    if v.is_finite() { v } else { default }
}

/// Canvas size of an Atelier document, from its manifest.
pub fn canvas_size(path: &Path) -> Option<(u32, u32)> {
    #[derive(Deserialize)]
    struct Size {
        width: u32,
        height: u32,
    }
    let mut zip = open(path).ok()?;
    let data = read_entry(&mut zip, MANIFEST, MAX_SMALL_ENTRY).ok()?;
    let size: Size = serde_json::from_slice(&data).ok()?;
    Some((size.width, size.height))
}

/// The preview stored in an Atelier document.
pub fn thumbnail_of(path: &Path) -> Option<RgbaImage> {
    let mut zip = open(path).ok()?;
    let png = read_entry(&mut zip, THUMBNAIL, MAX_SMALL_ENTRY).ok()?;
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png).ok()?;
    Some(image.into_rgba8())
}

fn open(path: &Path) -> Result<ZipArchive<BufReader<File>>, FileError> {
    ZipArchive::new(BufReader::new(File::open(path)?)).map_err(|e| match e {
        ZipError::Io(e) if e.kind() != std::io::ErrorKind::UnexpectedEof => FileError::Io(e),
        _ => FileError::NotNative,
    })
}

/// The manifest, after checking the file is an Atelier document of a
/// version this Atelier reads.
fn read_manifest(zip: &mut ZipArchive<impl Read + Seek>) -> Result<Manifest, FileError> {
    let data = read_entry(zip, MANIFEST, MAX_SMALL_ENTRY).map_err(|e| match e {
        FileError::Io(e) => FileError::Io(e),
        _ => FileError::NotNative,
    })?;
    let header: Header = serde_json::from_slice(&data).map_err(|_| FileError::NotNative)?;
    if header.format != FORMAT {
        return Err(FileError::NotNative);
    }
    if header.version > VERSION {
        return Err(FileError::NewerVersion(header.version));
    }
    serde_json::from_slice(&data).map_err(|_| FileError::Damaged)
}

/// Contents of an archive entry, at most `limit` bytes.
fn read_entry(
    zip: &mut ZipArchive<impl Read + Seek>,
    name: &str,
    limit: u64,
) -> Result<Vec<u8>, FileError> {
    let end = zip.central_directory_start();
    let entry = zip.by_name(name).map_err(zip_error)?;
    if entry.size() > limit || entry.compressed_size() > end {
        return Err(FileError::Damaged);
    }
    // The declared size only bounds the reservation: the real growth is
    // bounded by `take`.
    let mut data = Vec::with_capacity(entry.size().min(MAX_SMALL_ENTRY) as usize);
    entry.take(limit + 1).read_to_end(&mut data)?;
    if data.len() as u64 > limit {
        return Err(FileError::Damaged);
    }
    Ok(data)
}

/// Deflate shrinks data at most about 1032 times.
const DEFLATE_RATIO: u64 = 1100;

/// The PNG of a layer, mask or channel of `size` pixels of `bpp` bytes.
/// Refused when it is larger than any layer, or when its bytes in the
/// archive are too few to hold that many pixels (a few kilobytes would
/// otherwise make the decoder reserve gigabytes).
pub(super) fn read_pixels(
    zip: &mut ZipArchive<impl Read + Seek>,
    name: &str,
    size: (u32, u32),
    bpp: u64,
) -> Result<Vec<u8>, FileError> {
    let (w, h) = (u64::from(size.0), u64::from(size.1));
    if w * h > u64::from(MAX_DIMENSION).pow(2) {
        return Err(FileError::Damaged);
    }
    let raw = h * (1 + w * bpp);
    let stored = zip.by_name(name).map_err(zip_error)?.compressed_size();
    if stored.saturating_mul(DEFLATE_RATIO) + (1 << 16) < raw {
        return Err(FileError::Damaged);
    }
    // A PNG is never much larger than its raw pixels.
    read_entry(zip, name, 2 * w * h * bpp + (1 << 20))
}

/// Photoshop image resources: each its id (u16), name (a length byte,
/// then it) and data (a u32 length, then it), big-endian.
fn resources_bytes(resources: &[super::PsdResource]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in resources {
        let name = &r.name[..r.name.len().min(255)];
        out.extend(r.id.to_be_bytes());
        out.push(name.len() as u8);
        out.extend(name);
        out.extend((r.data.len() as u32).to_be_bytes());
        out.extend(&r.data);
    }
    out
}

/// What [`resources_bytes`] wrote; `None` if damaged.
fn resources_of(mut data: &[u8]) -> Option<Vec<super::PsdResource>> {
    let mut take = |n: usize| {
        let (head, rest) = data.split_at_checked(n)?;
        data = rest;
        Some(head)
    };
    let mut out = Vec::new();
    while let Some(id) = take(2) {
        let id = u16::from_be_bytes([id[0], id[1]]);
        let n = take(1)?[0] as usize;
        let name = take(n)?.to_vec();
        let len = u32::from_be_bytes(take(4)?.try_into().ok()?) as usize;
        let data = take(len)?.to_vec();
        out.push(super::PsdResource { id, name, data });
    }
    Some(out)
}

fn zip_error(e: ZipError) -> FileError {
    match e {
        ZipError::Io(e) => FileError::Io(e),
        _ => FileError::Damaged,
    }
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, FileError> {
    let mut png = Vec::new();
    let (w, h) = image.dimensions();
    PngEncoder::new(&mut png).write_image(image.as_raw(), w, h, ExtendedColorType::Rgba8)?;
    Ok(png)
}

fn encode_gray(image: &GrayImage) -> Result<Vec<u8>, FileError> {
    let mut png = Vec::new();
    let (w, h) = image.dimensions();
    PngEncoder::new(&mut png).write_image(image.as_raw(), w, h, ExtendedColorType::L8)?;
    Ok(png)
}

fn decode_gray(png: &[u8], size: (u32, u32)) -> Result<GrayImage, FileError> {
    let decoder = PngDecoder::new(Cursor::new(png)).map_err(|_| FileError::Damaged)?;
    if decoder.dimensions() != size {
        return Err(FileError::Damaged);
    }
    let image = DynamicImage::from_decoder(decoder).map_err(|_| FileError::Damaged)?;
    Ok(image.into_luma8())
}

fn decode_png(png: &[u8], size: (u32, u32)) -> Result<RgbaImage, FileError> {
    let decoder = PngDecoder::new(Cursor::new(png)).map_err(|_| FileError::Damaged)?;
    if decoder.dimensions() != size {
        return Err(FileError::Damaged);
    }
    let image = DynamicImage::from_decoder(decoder).map_err(|_| FileError::Damaged)?;
    Ok(image.into_rgba8())
}

/// The flattened image reduced to fit [`THUMBNAIL_SIZE`].
fn thumbnail(flat: &RgbaImage) -> RgbaImage {
    let (w, h) = flat.dimensions();
    let scale = (THUMBNAIL_SIZE as f32 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    image::imageops::thumbnail(flat, tw, th)
}

/// `f` on each item, spread over the processor's cores, while `meanwhile`
/// runs on this thread.
pub(super) fn par_map<T: Sync, R: Send, M>(
    items: &[T],
    f: impl Fn(&T) -> R + Sync,
    meanwhile: impl FnOnce() -> M,
) -> (Vec<R>, M) {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = items.len().div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let workers: Vec<_> = items
            .chunks(chunk)
            .map(|items| s.spawn(move || items.iter().map(f).collect::<Vec<R>>()))
            .collect();
        let m = meanwhile();
        let out = workers
            .into_iter()
            .flat_map(|w| w.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
            .collect();
        (out, m)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::ExportFormat;
    use image::Rgba;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-native-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("doc.atelier")
    }

    /// Three layers with every setting away from its default.
    fn sample() -> Document {
        let bg = RgbaImage::from_fn(40, 30, |x, y| Rgba([x as u8 * 6, y as u8 * 8, 77, 255]));
        let mut doc = Document::new(bg, None);
        doc.icc_profile = Some(Arc::new(vec![1, 2, 3, 4]));
        let mut half = RgbaImage::from_pixel(12, 9, Rgba([200, 10, 30, 128]));
        half.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        doc.add_layer("Half".into(), half, (-5, 25));
        {
            let layer = doc.active_layer_mut();
            layer.opacity = 0.4;
            layer.blend = BlendMode::Multiply;
            layer.visible = false;
            layer.locked = true;
            layer.effects.stroke.enabled = true;
            layer.effects.stroke.color = [1, 2, 3, 4];
            layer.effects.stroke.width = 7.5;
            layer.effects.shadow.enabled = true;
            layer.effects.shadow.dx = -3.0;
            layer.effects.shadow.blur = 11.0;
        }
        // Rich text: two styles, two paragraphs, a box, a transformation.
        let style = CharStyle {
            family: "Inter".into(),
            weight: 700,
            italic: true,
            stretch: 3,
            size: 31.0,
            leading: Some(40.0),
            tracking: 50.0,
            kerning: Kerning::Zero,
            horizontal_scale: 1.5,
            vertical_scale: 0.75,
            baseline_shift: -2.0,
            color: [9, 8, 7, 255],
            faux_bold: true,
            faux_italic: true,
            all_caps: true,
            small_caps: true,
            underline: true,
            strikethrough: true,
        };
        let mut text = TextData::plain("Bonjour\nété", style);
        text.restyle(3..5, |s| *s = CharStyle::default());
        text.paragraphs[1] = ParagraphStyle {
            align: TextAlign::Justify,
            indent_left: 1.0,
            indent_right: 2.0,
            indent_first: -3.0,
            space_before: 4.0,
            space_after: 5.0,
        };
        text.frame = Some((120.0, 80.0));
        text.antialias = AntiAlias::None;
        text.transform = [0.0, -1.0, 1.0, 0.5];
        doc.add_text_layer(
            text,
            text::Rendered {
                image: RgbaImage::from_pixel(20, 10, Rgba([9, 8, 7, 255])),
                anchor: (2.25, 3.5),
            },
            (5.25, 7.5),
        );
        // A clipped layer with a mask (unlinked, soft), styles and a link.
        doc.add_layer(
            "Clipped".into(),
            RgbaImage::from_pixel(8, 8, Rgba([5, 6, 7, 255])),
            (2, 2),
        );
        doc.toggle_clipping();
        let mask = image::GrayImage::from_fn(6, 5, |x, y| image::Luma([(x * 40 + y) as u8]));
        {
            let layer = doc.active_layer_mut();
            let mut m = crate::document::Mask::new(mask, (1, 3), 255, layer.offset);
            m.set_linked(false, layer.offset);
            (m.density, m.feather, m.enabled) = (0.7, 2.5, false);
            layer.mask = Some(m);
            layer.fill = 0.3;
            layer.link = Some(77);
            layer.blend = BlendMode::LinearDodge;
            let fx = &mut layer.effects;
            fx.inner_shadow.enabled = true;
            fx.outer_glow.spread = 0.4;
            fx.inner_glow.blend = BlendMode::Overlay;
            fx.color_overlay.color = [1, 2, 3, 200];
            fx.gradient_overlay.fill.style = Shape::Diamond;
            fx.gradient_overlay.fill.reverse = true;
            fx.bevel.enabled = true;
            fx.bevel.up = false;
            fx.bevel.altitude = 42.0;
            fx.stroke.blend = BlendMode::Hue;
        }
        // Adjustment and fill layers, in a closed group.
        let mut curves = Curves::default();
        curves.rgb.insert(64.0, 30.0);
        doc.add_adjustment_layer(Adjustment::Curves(curves), None);
        doc.add_fill_layer(
            Fill::Gradient(GradientFill {
                style: Shape::Radial,
                angle: 33.0,
                ..Default::default()
            }),
            None,
        );
        let n = doc.layers.len();
        doc.group_layers(&[n - 2, n - 1]);
        let group = doc.active_layer_mut();
        group.kind = LayerKind::Group { expanded: false };
        group.opacity = 0.8;
        doc.guides = vec![Guide::new(Orientation::Vertical, 12.5)];
        doc.channels = vec![Channel {
            name: "Alpha 1".into(),
            mask: Arc::new(image::GrayImage::from_fn(40, 30, |x, _| {
                image::Luma([x as u8 * 6])
            })),
        }];
        doc.active = 1;
        doc
    }

    #[test]
    fn round_trip_keeps_everything() {
        let path = temp("round-trip");
        let mut doc = sample();
        doc.modified = true;
        doc.save(&path).unwrap();
        assert!(!doc.modified);
        assert_eq!(doc.path.as_deref(), Some(path.as_path()));

        let back = Document::open(&path).unwrap().doc;
        assert!(!back.modified);
        assert_eq!((back.width(), back.height()), (40, 30));
        assert_eq!(back.active, 1);
        assert_eq!(
            back.icc_profile.as_deref().map(Vec::as_slice),
            Some(&[1, 2, 3, 4][..])
        );
        assert_eq!(back.layers.len(), 7);
        for (a, b) in doc.layers.iter().zip(&back.layers) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
            assert_eq!(a.image(), b.image());
            assert_eq!(a.offset, b.offset);
            assert_eq!(a.visible, b.visible);
            assert_eq!(a.opacity, b.opacity);
            assert_eq!(a.fill, b.fill);
            assert_eq!(a.locked, b.locked);
            assert_eq!(a.blend, b.blend);
            assert_eq!((a.depth, a.clipped, a.link), (b.depth, b.clipped, b.link));
            assert_eq!(a.effects, b.effects);
            assert_eq!(a.text, b.text);
            assert_eq!(a.mask.is_some(), b.mask.is_some());
            if let (Some(m), Some(n)) = (&a.mask, &b.mask) {
                assert_eq!(m.image(), n.image());
                assert_eq!(m.origin(a.offset), n.origin(b.offset));
                assert_eq!(
                    (m.default, m.enabled, m.linked(), m.density, m.feather),
                    (n.default, n.enabled, n.linked(), n.density, n.feather)
                );
            }
        }
        assert_eq!(back.flatten(), doc.flatten());
        assert_eq!(back.guides.len(), 1);
        assert_eq!(back.guides[0].position, 12.5);
        assert_eq!(back.channels[0].name, "Alpha 1");
        assert_eq!(back.channels[0].mask, doc.channels[0].mask);
        assert_eq!(canvas_size(&path), Some((40, 30)));
        assert_eq!(thumbnail_of(&path).map(|t| t.dimensions()), Some((40, 30)));
    }

    /// A version 1 file (before groups, masks…) opens, with the look it
    /// had: its drop shadow blended normally.
    #[test]
    fn version_1_documents_still_open() {
        let path = temp("version-1");
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([200, 100, 0, 255])), None);
        doc.add_layer(
            "A".into(),
            RgbaImage::from_pixel(2, 2, Rgba([0, 0, 255, 255])),
            (3, 3),
        );
        doc.active_layer_mut().effects.shadow.enabled = true;
        doc.save(&path).unwrap();
        // The manifest as version 1 wrote it.
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let manifest = read_entry(&mut zip, MANIFEST, MAX_SMALL_ENTRY).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        json["version"] = 1.into();
        for layer in json["layers"].as_array_mut().unwrap() {
            let layer = layer.as_object_mut().unwrap();
            for new in [
                "kind",
                "fill",
                "depth",
                "clipped",
                "link",
                "mask",
                "inner_shadow",
                "outer_glow",
                "inner_glow",
                "color_overlay",
                "gradient_overlay",
                "bevel",
            ] {
                layer.remove(new);
            }
            layer["stroke"]
                .as_object_mut()
                .unwrap()
                .retain(|k, _| k != "opacity" && k != "blend");
            layer["shadow"].as_object_mut().unwrap().remove("blend");
        }
        let old = path.with_file_name("old.atelier");
        let mut out = ZipWriter::new(File::create(&old).unwrap());
        out.start_file(MANIFEST, SimpleFileOptions::default())
            .unwrap();
        out.write_all(json.to_string().as_bytes()).unwrap();
        for i in 0..2 {
            let name = format!("layers/{i}.png");
            let png = read_entry(&mut zip, &name, 1 << 30).unwrap();
            out.start_file(name, SimpleFileOptions::default()).unwrap();
            out.write_all(&png).unwrap();
        }
        out.finish().unwrap();
        let back = Document::open(&old).unwrap().doc;
        let layer = &back.layers[1];
        assert_eq!(
            (layer.kind.clone(), layer.fill, layer.depth),
            (LayerKind::Pixels, 1.0, 0)
        );
        assert!(layer.mask.is_none() && !layer.clipped);
        assert_eq!(layer.effects.shadow.blend, BlendMode::Normal);
        assert_eq!(layer.effects.stroke.opacity, 1.0);
        assert!(!layer.effects.bevel.enabled);
    }

    /// A text of the first version 2 files, with a single style, opens with
    /// it; the newer readers' fields are written for the older ones.
    #[test]
    fn single_style_texts_still_open() {
        let mut text = TextData::plain(
            "Hi\nyou",
            CharStyle {
                family: "Inter".into(),
                size: 20.0,
                weight: 700,
                color: [1, 2, 3, 255],
                ..Default::default()
            },
        );
        text.paragraphs[0].align = TextAlign::Center;
        let mut json = serde_json::to_value(text_entry(&text)).unwrap();
        assert_eq!(json["bold"], true);
        assert_eq!(json["align"], "center");
        let entry = json.as_object_mut().unwrap();
        for new in [
            "runs",
            "paragraphs",
            "frame",
            "antialias",
            "transform",
            "anchor",
        ] {
            entry.remove(new);
        }
        let old: TextEntry = serde_json::from_value(json).unwrap();
        let back = text_data(old, &RgbaImage::new(100, 40));
        assert_eq!(back.text, "Hi\nyou");
        assert_eq!(back.runs.len(), 1);
        let style = &back.runs[0].style;
        assert_eq!(
            (style.family.as_str(), style.size, style.weight),
            ("Inter", 20.0, 700)
        );
        assert_eq!(style.color, [1, 2, 3, 255]);
        assert!(back.paragraphs.iter().all(|p| p.align == TextAlign::Center));
        // Its first line where the first renderer put it: centered.
        assert_eq!(back.anchor.0, 50.0);
        assert_eq!(back.transform, text::IDENTITY);
        assert_eq!(back.frame, None);
    }

    #[test]
    fn newer_versions_and_other_files_are_refused() {
        let path = temp("versions");
        sample().save(&path).unwrap();
        // The same document, claiming a future version.
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let manifest = read_entry(&mut zip, MANIFEST, MAX_SMALL_ENTRY).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        json["version"] = (VERSION + 1).into();
        let future = path.with_file_name("future.atelier");
        let mut out = ZipWriter::new(File::create(&future).unwrap());
        out.start_file(MANIFEST, SimpleFileOptions::default())
            .unwrap();
        out.write_all(json.to_string().as_bytes()).unwrap();
        out.finish().unwrap();
        assert!(matches!(
            Document::open(&future).map(|_| ()),
            Err(FileError::NewerVersion(v)) if v == VERSION + 1
        ));
        // A PNG named .atelier.
        let png = path.with_file_name("image.atelier");
        RgbaImage::new(2, 2)
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();
        assert!(matches!(
            Document::open(&png).map(|_| ()),
            Err(FileError::NotNative)
        ));
        // Cut short.
        let data = std::fs::read(&path).unwrap();
        let cut = path.with_file_name("cut.atelier");
        std::fs::write(&cut, &data[..data.len() / 2]).unwrap();
        assert!(Document::open(&cut).is_err());
    }

    /// `path` again as `name`, its manifest changed by `edit`.
    fn with_manifest(
        path: &Path,
        name: &str,
        edit: impl Fn(&mut serde_json::Value),
    ) -> std::path::PathBuf {
        let mut zip = ZipArchive::new(File::open(path).unwrap()).unwrap();
        let manifest = read_entry(&mut zip, MANIFEST, MAX_SMALL_ENTRY).unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        edit(&mut json);
        let changed = path.with_file_name(name);
        let mut out = ZipWriter::new(File::create(&changed).unwrap());
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).unwrap();
            let entry_name = entry.name().to_string();
            out.start_file(&entry_name, SimpleFileOptions::default())
                .unwrap();
            if entry_name == MANIFEST {
                out.write_all(json.to_string().as_bytes()).unwrap();
            } else {
                std::io::copy(&mut entry, &mut out).unwrap();
            }
        }
        out.finish().unwrap();
        changed
    }

    #[test]
    fn extreme_offsets_and_style_sizes_are_bounded() {
        let path = temp("extremes");
        sample().save(&path).unwrap();
        let far = with_manifest(&path, "far.atelier", |j| {
            j["layers"][1]["x"] = i64::MIN.into()
        });
        assert!(matches!(
            Document::open(&far).map(|_| ()),
            Err(FileError::Damaged)
        ));
        let huge = with_manifest(&path, "huge.atelier", |j| {
            let shadow = &mut j["layers"][1]["shadow"];
            shadow["enabled"] = true.into();
            shadow["blur"] = 4_294_967_295u64.into();
            shadow["dx"] = 1e12.into();
        });
        let doc = Document::open(&huge).unwrap().doc;
        let shadow = &doc.layers[1].effects.shadow;
        assert_eq!((shadow.blur, shadow.dx), (250.0, 30_000.0));
        doc.flatten();
    }

    #[test]
    fn saving_over_a_document_replaces_it_whole() {
        let path = temp("overwrite");
        let mut doc = sample();
        doc.save(&path).unwrap();
        doc.delete_active();
        doc.save(&path).unwrap();
        assert_eq!(
            Document::open(&path).unwrap().doc.layers.len(),
            doc.layers.len()
        );
        let dir = path.parent().unwrap();
        assert_eq!(
            std::fs::read_dir(dir).unwrap().count(),
            1,
            "no temporary file left"
        );
    }

    #[test]
    fn export_leaves_the_document_to_save() {
        let path = temp("export");
        let png = path.with_file_name("flat.png");
        let mut doc = sample();
        doc.modified = true;
        doc.export(&png, ExportFormat::Png, 90).unwrap();
        assert!(doc.modified);
        assert_eq!(doc.path, None);
        assert_eq!(image::open(&png).unwrap().to_rgba8(), doc.flatten());
        // A document placed shows flattened.
        doc.save(&path).unwrap();
        let mut other = Document::new(RgbaImage::new(40, 30), None);
        other.place_file(&path, false).unwrap();
        assert_eq!(other.active_layer().image(), &doc.flatten());
    }

    #[test]
    fn artboards_round_trip() {
        use crate::document::{Artboard, ArtboardBackground, PixelRect};
        let path = temp("artboards");
        let mut doc = Document::with_artboard(60, 40, ArtboardBackground::Custom([1, 2, 3]));
        let rect = PixelRect {
            x: 80,
            y: 10,
            w: 30,
            h: 20,
        };
        doc.add_artboard(
            "new".into(),
            "B".into(),
            rect,
            ArtboardBackground::Transparent,
        );
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert_eq!((back.width(), back.height()), (110, 40));
        let boards: Vec<(Option<Artboard>, (i64, i64), u32)> = back
            .layers
            .iter()
            .map(|l| (l.artboard, l.offset, l.depth))
            .collect();
        let expected: Vec<_> = doc
            .layers
            .iter()
            .map(|l| (l.artboard, l.offset, l.depth))
            .collect();
        assert_eq!(boards, expected);
        assert_eq!(back.artboards().len(), 2);
        assert_eq!(back.flatten(), doc.flatten());
    }

    /// `path` rewritten with its manifest changed by `edit` (entries
    /// stored, as Atelier writes them), then the sizes the archive
    /// declares for `entry` set to `declared`.
    fn crafted(
        path: &Path,
        edit: impl FnOnce(&mut serde_json::Value),
        entry: &str,
        declared: Option<u32>,
    ) -> std::path::PathBuf {
        let mut zip = ZipArchive::new(File::open(path).unwrap()).unwrap();
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let mut edit = Some(edit);
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().to_string();
            let mut data = Vec::new();
            e.read_to_end(&mut data).unwrap();
            if name == MANIFEST {
                let mut json: serde_json::Value = serde_json::from_slice(&data).unwrap();
                (edit.take().unwrap())(&mut json);
                data = json.to_string().into_bytes();
            }
            out.start_file(name, stored).unwrap();
            out.write_all(&data).unwrap();
        }
        let mut bytes = out.finish().unwrap().into_inner();
        if let Some(size) = declared {
            let u16_at = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]) as usize;
            // Central directory: size at 24, name length at 28, name at 46;
            // local headers: 22, 26, 30.
            for (magic, size_at, len_at, name_at) in
                [(b"PK\x01\x02", 24, 28, 46), (b"PK\x03\x04", 22, 26, 30)]
            {
                let mut at = 0;
                while let Some(i) = bytes[at..].windows(4).position(|w| w == magic) {
                    let h = at + i;
                    let n = u16_at(&bytes, h + len_at);
                    if bytes.get(h + name_at..h + name_at + n) == Some(entry.as_bytes()) {
                        bytes[h + size_at..h + size_at + 4].copy_from_slice(&size.to_le_bytes());
                    }
                    at = h + 4;
                }
            }
        }
        let crafted = path.with_file_name("crafted.atelier");
        std::fs::write(&crafted, bytes).unwrap();
        crafted
    }

    /// A few kilobytes never make the reader reserve gigabytes: neither
    /// the sizes the archive declares nor a layer's declared dimensions
    /// are trusted beyond what its bytes can hold.
    #[test]
    fn declared_sizes_never_reserve_memory() {
        let path = temp("declared-sizes");
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([9, 9, 9, 255])), None);
        doc.add_layer(
            "A".into(),
            RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255])),
            (0, 0),
        );
        doc.save(&path).unwrap();
        let png = "layers/1.png";
        let side = |v: u32| {
            move |json: &mut serde_json::Value| {
                json["layers"][1]["width"] = v.into();
                json["layers"][1]["height"] = v.into();
            }
        };
        let cases = [
            // 4 GB declared for the layer's few bytes.
            (
                crafted(&path, side(30_000), png, Some(0xFFFF_FFF0)),
                "declared size",
            ),
            // 30000² pixels in a few bytes.
            (crafted(&path, side(30_000), png, None), "dimensions"),
            // Larger than any document.
            (
                crafted(&path, side(100_000), png, Some(0xFFFF_FFF0)),
                "area",
            ),
        ];
        for (file, what) in cases {
            let started = std::time::Instant::now();
            let opened = Document::open(&file);
            assert!(matches!(opened, Err(FileError::Damaged)), "{what}");
            assert!(started.elapsed().as_secs_f32() < 1.0, "{what}");
        }
        // Untouched, it opens.
        let same = crafted(&path, |_| {}, png, None);
        assert_eq!(Document::open(&same).unwrap().doc.flatten(), doc.flatten());
    }

    /// The pixels a PNG entry may declare: what deflate can make of its
    /// bytes, and no more than a document holds.
    #[test]
    fn pixel_entries_hold_what_their_bytes_can() {
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        out.start_file("small", stored).unwrap();
        out.write_all(&[0; 1000]).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(out.finish().unwrap().into_inner())).unwrap();
        assert!(read_pixels(&mut zip, "small", (100, 100), 4).is_ok());
        let refused = |zip: &mut ZipArchive<_>, size| {
            matches!(read_pixels(zip, "small", size, 4), Err(FileError::Damaged))
        };
        assert!(refused(&mut zip, (2000, 2000)));
        assert!(refused(&mut zip, (MAX_DIMENSION + 1, MAX_DIMENSION)));
    }

    /// What a document's first file held beside its layers (EXIF, a
    /// Photoshop document's resolution and metadata) stays with it in an
    /// Atelier document.
    #[test]
    fn file_metadata_round_trip() {
        let path = temp("file-metadata");
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, Rgba([9, 9, 9, 255])), None);
        doc.file_metadata.exif = Some(Arc::new(b"II*\0exif".to_vec()));
        doc.file_metadata.psd_resources = Arc::new(vec![
            crate::document::PsdResource {
                id: 1005,
                name: Vec::new(),
                data: vec![1, 44, 0, 0, 0, 1, 0, 2],
            },
            crate::document::PsdResource {
                id: 1060,
                name: b"x".to_vec(),
                data: b"<x:xmpmeta/>".to_vec(),
            },
        ]);
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert_eq!(back.file_metadata, doc.file_metadata);
        // None: nothing written.
        let plain = temp("file-metadata-none");
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, Rgba([9, 9, 9, 255])), None);
        doc.save(&plain).unwrap();
        let mut zip = ZipArchive::new(File::open(&plain).unwrap()).unwrap();
        assert!(zip.by_name(EXIF).is_err() && zip.by_name(PSD_RESOURCES).is_err());
    }
}
