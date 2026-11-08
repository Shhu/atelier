//! Photoshop documents (.psd, and the large .psb), after Adobe's
//! "Photoshop File Formats Specification".
//!
//! Reading takes 8-bit RGB as it is; 16 and 32 bits, grayscale, CMYK, Lab,
//! duotone, indexed and bitmap images are converted to 8-bit RGB (CMYK
//! and grayscale through their profile, into sRGB). Layers
//! come with their names, positions, visibility, opacity, fill opacity,
//! blend modes, groups, clipping, locks, masks (vector masks rasterized),
//! the adjustment and fill layers and the layer styles Atelier has; type
//! layers and smart objects come as their rendered pixels. What Atelier
//! can't hold is left out or simplified, and listed in notes for the user.
//!
//! Writing (8-bit RGB, or CMYK converted through a profile; RLE
//! compressed; a PSB for a .psb file) keeps the layers with all of the
//! above, the saved selections as alpha channels, the guides, the color
//! profile, the opened file's resources that describe the document
//! (resolution, XMP, IPTC, EXIF, print settings) as they were, and the
//! flattened image for the applications that only read that.

use std::path::Path;

use super::{BlendMode, Document, FileError};

mod adjust;
mod artboard;
mod bytes;
mod descriptor;
mod effects;
mod pattern;
mod read;
mod vector;
mod write;

pub use read::{canvas_size, thumbnail_of};

/// Whether `path` names a Photoshop document (by its extension).
pub fn is_psd(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
}

/// Whether `path` names a Large Document Format file (.psb): written in
/// version 2, which holds larger layers and files.
pub fn is_psb(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("psb"))
}

/// A Photoshop document read, and what had to be simplified to open it
/// (translated, for the status bar).
pub struct Imported {
    pub doc: Document,
    pub notes: Vec<String>,
}

/// Reads a Photoshop document, within `memory`.
pub fn read(
    path: &Path,
    memory: crate::document::file_io::MemoryUse,
) -> Result<Imported, FileError> {
    let data = std::fs::read(path)?;
    let mut budget = memory.budget();
    budget.take_bytes(data.len() as u64)?;
    let mut imported = read::document_within(&data, budget)?;
    imported.doc.path = Some(path.to_path_buf());
    Ok(imported)
}

/// Writes `doc` as a Photoshop document (a PSB if `big`); returns what it
/// could not keep (translated notes).
pub fn write(
    doc: &Document,
    out: &mut impl std::io::Write,
    big: bool,
) -> Result<Vec<String>, FileError> {
    let (data, notes) = write::document(doc, None, big)?;
    out.write_all(&data)?;
    Ok(notes)
}

/// What saving `doc` as a Photoshop document (a PSB if `big`) would lose,
/// as [`write`] says it (translated notes).
pub fn losses(doc: &Document, big: bool) -> Vec<String> {
    write::losses(doc, big)
}

/// [`write`] in CMYK: every layer and the merged image converted by
/// `cmyk` (to its CMYK profile, which the file embeds).
pub fn write_cmyk(
    doc: &Document,
    out: &mut impl std::io::Write,
    cmyk: &crate::cms::Conversion,
    big: bool,
) -> Result<Vec<String>, FileError> {
    let (data, notes) = write::document(doc, Some(cmyk), big)?;
    out.write_all(&data)?;
    Ok(notes)
}

/// Blend modes and their keys in the file.
const BLENDS: [(BlendMode, &[u8; 4]); 28] = [
    (BlendMode::PassThrough, b"pass"),
    (BlendMode::Normal, b"norm"),
    (BlendMode::Dissolve, b"diss"),
    (BlendMode::Darken, b"dark"),
    (BlendMode::Multiply, b"mul "),
    (BlendMode::ColorBurn, b"idiv"),
    (BlendMode::LinearBurn, b"lbrn"),
    (BlendMode::DarkerColor, b"dkCl"),
    (BlendMode::Lighten, b"lite"),
    (BlendMode::Screen, b"scrn"),
    (BlendMode::ColorDodge, b"div "),
    (BlendMode::LinearDodge, b"lddg"),
    (BlendMode::LighterColor, b"lgCl"),
    (BlendMode::Overlay, b"over"),
    (BlendMode::SoftLight, b"sLit"),
    (BlendMode::HardLight, b"hLit"),
    (BlendMode::VividLight, b"vLit"),
    (BlendMode::LinearLight, b"lLit"),
    (BlendMode::PinLight, b"pLit"),
    (BlendMode::HardMix, b"hMix"),
    (BlendMode::Difference, b"diff"),
    (BlendMode::Exclusion, b"smud"),
    (BlendMode::Subtract, b"fsub"),
    (BlendMode::Divide, b"fdiv"),
    (BlendMode::Hue, b"hue "),
    (BlendMode::Saturation, b"sat "),
    (BlendMode::Color, b"colr"),
    (BlendMode::Luminosity, b"lum "),
];

/// The same modes in descriptors (layer styles).
const DESCRIPTOR_BLENDS: [(BlendMode, &[u8]); 28] = [
    (BlendMode::PassThrough, b"passThrough"),
    (BlendMode::Normal, b"Nrml"),
    (BlendMode::Dissolve, b"Dslv"),
    (BlendMode::Darken, b"Drkn"),
    (BlendMode::Multiply, b"Mltp"),
    (BlendMode::ColorBurn, b"CBrn"),
    (BlendMode::LinearBurn, b"linearBurn"),
    (BlendMode::DarkerColor, b"darkerColor"),
    (BlendMode::Lighten, b"Lghn"),
    (BlendMode::Screen, b"Scrn"),
    (BlendMode::ColorDodge, b"CDdg"),
    (BlendMode::LinearDodge, b"linearDodge"),
    (BlendMode::LighterColor, b"lighterColor"),
    (BlendMode::Overlay, b"Ovrl"),
    (BlendMode::SoftLight, b"SftL"),
    (BlendMode::HardLight, b"HrdL"),
    (BlendMode::VividLight, b"vividLight"),
    (BlendMode::LinearLight, b"linearLight"),
    (BlendMode::PinLight, b"pinLight"),
    (BlendMode::HardMix, b"hardMix"),
    (BlendMode::Difference, b"Dfrn"),
    (BlendMode::Exclusion, b"Xclu"),
    (BlendMode::Subtract, b"blendSubtraction"),
    (BlendMode::Divide, b"blendDivide"),
    (BlendMode::Hue, b"H   "),
    (BlendMode::Saturation, b"Strt"),
    (BlendMode::Color, b"Clr "),
    (BlendMode::Luminosity, b"Lmns"),
];

fn blend_key(mode: BlendMode) -> &'static [u8; 4] {
    BLENDS.iter().find(|b| b.0 == mode).map_or(b"norm", |b| b.1)
}

fn blend_of(key: &[u8]) -> BlendMode {
    BLENDS
        .iter()
        .find(|b| b.1 == key)
        .map_or(BlendMode::Normal, |b| b.0)
}

fn descriptor_blend_key(mode: BlendMode) -> &'static [u8] {
    DESCRIPTOR_BLENDS
        .iter()
        .find(|b| b.0 == mode)
        .map_or(b"Nrml", |b| b.1)
}

fn descriptor_blend_of(key: &[u8]) -> BlendMode {
    DESCRIPTOR_BLENDS
        .iter()
        .find(|b| b.1 == key)
        .map_or(BlendMode::Normal, |b| b.0)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use image::{GrayImage, Luma, Rgba, RgbaImage};

    use super::*;
    use crate::compose::gradient::GradientFill;
    use crate::document::{Channel, Fill, LayerKind, Mask, NewMask};
    use crate::guides::{Guide, Orientation};
    use crate::paint::gradient::{Preset, Shape};
    use crate::pixel_ops::{AdjustKind, Adjustment};

    fn write_read(doc: &Document) -> (Imported, Vec<String>) {
        let (data, notes) = write::document(doc, None, false).unwrap();
        (read::document(&data).unwrap(), notes)
    }

    /// Every supported feature, away from its default.
    fn sample() -> Document {
        let bg = RgbaImage::from_fn(40, 30, |x, y| Rgba([x as u8 * 6, y as u8 * 8, 77, 255]));
        let mut doc = Document::new(bg, None);
        let square = RgbaImage::from_fn(10, 8, |x, _| Rgba([200, 30, x as u8 * 20, 180]));
        doc.add_layer("Ünïcode ✓".into(), square, (-3, 5));
        let l = doc.active_layer_mut();
        (l.opacity, l.fill, l.blend, l.locked) = (0.5, 0.75, BlendMode::Multiply, true);
        l.effects.shadow.enabled = true;
        l.effects.stroke.enabled = true;
        l.effects.stroke.width = 2.0;
        doc.add_layer(
            "clipped".into(),
            RgbaImage::from_pixel(6, 6, Rgba([0, 255, 0, 255])),
            (2, 2),
        );
        doc.active_layer_mut().clipped = true;
        doc.active_layer_mut().visible = false;
        doc.add_layer(
            "masked".into(),
            RgbaImage::from_pixel(20, 20, Rgba([9, 9, 250, 255])),
            (10, 5),
        );
        let mut mask = GrayImage::from_fn(12, 10, |x, _| Luma([x as u8 * 20]));
        mask.put_pixel(0, 0, Luma([7]));
        let mut m = Mask::new(mask, (12, 6), 255, (10, 5));
        (m.density, m.feather, m.enabled) = (0.6, 3.0, false);
        doc.active_layer_mut().mask = Some(m);
        doc.active_layer_mut().clipped = false;
        doc.group_layers(&[3]);
        let g = doc.active;
        if let LayerKind::Group { expanded } = &mut doc.layers[g].kind {
            *expanded = false;
        }
        doc.layers[g].blend = BlendMode::Screen;
        doc.layers[g].opacity = 0.8;
        for kind in [
            AdjustKind::Levels,
            AdjustKind::Invert,
            AdjustKind::Posterize,
        ] {
            doc.add_adjustment_layer(kind.initial(), None);
        }
        doc.add_fill_layer(Fill::Solid([1, 2, 3]), None);
        let gradient = GradientFill {
            gradient: Preset::Spectrum.gradient(),
            style: Shape::Radial,
            ..Default::default()
        };
        doc.add_fill_layer(Fill::Gradient(gradient), None);
        doc.delete_mask();
        doc.add_mask(NewMask::HideAll, None);
        doc.guides = vec![
            Guide::new(Orientation::Horizontal, 12.5),
            Guide::new(Orientation::Vertical, -4.0),
        ];
        doc.channels.push(Channel {
            name: "Sky".into(),
            mask: Arc::new(GrayImage::from_fn(40, 30, |x, _| Luma([x as u8 * 6]))),
        });
        doc.icc_profile = Some(Arc::new(vec![1, 2, 3, 4, 5]));
        doc.active = 2;
        doc
    }

    /// The Color Sampler points; a later version's data or a damaged block
    /// costs the points, not the file.
    #[test]
    fn color_samplers_round_trip() {
        let mut doc = sample();
        doc.samplers = vec![
            crate::document::ColorSampler { x: 3.5, y: 7.5 },
            crate::document::ColorSampler { x: 39.5, y: 0.5 },
        ];
        let (back, notes) = write_read(&doc);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back.doc.samplers, doc.samplers);
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        // Saved again: only one block of them.
        let (again, _) = write_read(&back.doc);
        assert_eq!(again.doc.samplers, doc.samplers);
    }

    /// A 400 × 300 document holding a resource block, read back.
    fn with_block(id: u16, data: Vec<u8>) -> Document {
        let mut doc = Document::new(RgbaImage::new(400, 300), None);
        doc.file_metadata.psd_resources = Arc::new(vec![crate::document::PsdResource {
            id,
            name: Vec::new(),
            data,
        }]);
        write_read(&doc).0.doc
    }

    /// Bytes of a samplers block: header, then `record` for each point.
    fn samplers_block(
        version: u32,
        points: &[(f32, f32)],
        record: &dyn Fn(f32, f32, &mut Vec<u8>),
    ) -> Vec<u8> {
        let mut data = version.to_be_bytes().to_vec();
        data.extend((points.len() as u32).to_be_bytes());
        for &(x, y) in points {
            record(x, y, &mut data);
        }
        data
    }

    /// Each layout of the block, byte by byte: vertical first (Photoshop's
    /// files according to ag-psd), points off the canvas left out.
    #[test]
    fn color_sampler_blocks_read_in_every_version() {
        use crate::document::ColorSampler;
        let points = [(100.5, 200.5), (300.5, 50.5)];
        let want = [
            ColorSampler { x: 100.5, y: 200.5 },
            ColorSampler { x: 300.5, y: 50.5 },
        ];
        let fixed = |v: f32, by: f32| ((v * by) as i32).to_be_bytes();
        let float = |v: f32| v.to_bits().to_be_bytes();
        let cases: Vec<(&str, u16, Vec<u8>)> = vec![
            (
                "1038 v1 in 1/16",
                1038,
                samplers_block(1, &points, &|x, y, d| {
                    d.extend(fixed(y, 16.0));
                    d.extend(fixed(x, 16.0));
                    d.extend([0xff, 0xff]);
                }),
            ),
            (
                "1038 v1 in 16.16",
                1038,
                samplers_block(1, &points, &|x, y, d| {
                    d.extend(fixed(y, 65536.0));
                    d.extend(fixed(x, 65536.0));
                    d.extend([0xff, 0xff]);
                }),
            ),
            (
                "1073 v2",
                1073,
                samplers_block(2, &points, &|x, y, d| {
                    d.extend(float(y));
                    d.extend(float(x));
                    d.extend([0xff, 0xff, 0, 1]);
                }),
            ),
            (
                "1073 v3, fixed, depth (ag-psd)",
                1073,
                samplers_block(3, &points, &|x, y, d| {
                    d.extend(1u32.to_be_bytes());
                    d.extend(fixed(y, 16.0));
                    d.extend(fixed(x, 16.0));
                    d.extend([0xff, 0xff, 0, 1]);
                }),
            ),
            (
                "1073 v3, fixed, no depth (specification)",
                1073,
                samplers_block(3, &points, &|x, y, d| {
                    d.extend(1u32.to_be_bytes());
                    d.extend(fixed(y, 16.0));
                    d.extend(fixed(x, 16.0));
                    d.extend([0xff, 0xff]);
                }),
            ),
            (
                "1073 v3, floats, depth",
                1073,
                samplers_block(3, &points, &|x, y, d| {
                    d.extend(2u32.to_be_bytes());
                    d.extend(float(y));
                    d.extend(float(x));
                    d.extend([0xff, 0xff, 0, 1]);
                }),
            ),
        ];
        for (name, id, data) in cases {
            assert_eq!(with_block(id, data).samplers, want, "{name}");
        }
        // Off the canvas, not finite, a later version: not points.
        let off = samplers_block(
            2,
            &[(500.5, 10.5), (f32::NAN, 1.0), (10.5, 20.5)],
            &|x, y, d| {
                d.extend(float(y));
                d.extend(float(x));
                d.extend([0xff, 0xff, 0, 1]);
            },
        );
        assert_eq!(
            with_block(1073, off).samplers,
            [ColorSampler { x: 10.5, y: 20.5 }]
        );
        assert!(
            with_block(1073, vec![0, 0, 0, 9, 0, 0, 0, 1])
                .samplers
                .is_empty()
        );
    }

    /// 16.16 points near the top left of a large canvas, which would fall
    /// inside it in 1/16 too, are not taken for far away ones.
    #[test]
    fn small_16_16_sampler_positions_are_not_read_as_sixteenths() {
        use crate::document::ColorSampler;
        let block = |by: f32, points: &[(f32, f32)]| {
            samplers_block(1, points, &|x, y, d| {
                d.extend(((y * by) as i32).to_be_bytes());
                d.extend(((x * by) as i32).to_be_bytes());
                d.extend([0xff, 0xff]);
            })
        };
        let read = |data: Vec<u8>| {
            let mut doc = Document::new(RgbaImage::new(2100, 2100), None);
            doc.file_metadata.psd_resources = Arc::new(vec![crate::document::PsdResource {
                id: 1038,
                name: Vec::new(),
                data,
            }]);
            write_read(&doc).0.doc.samplers
        };
        let small = [(0.5, 0.5), (1.5, 0.5)];
        assert_eq!(
            read(block(65536.0, &small)),
            [ColorSampler { x: 0.5, y: 0.5 }, ColorSampler { x: 1.5, y: 0.5 }]
        );
        // Points of Photoshop's 1/16 stay where they are.
        let centers = [(0.5, 0.5), (2048.5, 1024.5)];
        assert_eq!(
            read(block(16.0, &centers)),
            [
                ColorSampler { x: 0.5, y: 0.5 },
                ColorSampler { x: 2048.5, y: 1024.5 }
            ]
        );
    }

    /// What is written: only the current block, version 2, its bytes known.
    #[test]
    fn color_samplers_are_written_in_the_current_block() {
        let mut doc = Document::new(RgbaImage::new(20_000, 8), None);
        doc.samplers = vec![crate::document::ColorSampler {
            x: 19_000.5,
            y: 7.5,
        }];
        let (back, _) = write_read(&doc);
        assert_eq!(back.doc.samplers, doc.samplers);
        let bytes = write::document(&doc, None, false).unwrap().0;
        let mut want = b"8BIM\x04\x31\0\0".to_vec();
        want.extend(20u32.to_be_bytes());
        want.extend([0, 0, 0, 2, 0, 0, 0, 1]);
        want.extend(7.5f32.to_bits().to_be_bytes());
        want.extend(19_000.5f32.to_bits().to_be_bytes());
        want.extend([0xff, 0xff, 0, 1]);
        assert!(bytes.windows(want.len()).any(|w| w == want), "the block");
        assert!(
            !bytes.windows(6).any(|w| w == b"8BIM\x04\x0e"),
            "no old block"
        );
    }

    /// Each lock comes back by itself; Lock All stays apart from them.
    #[test]
    fn partial_locks_round_trip() {
        let mut doc = Document::new(RgbaImage::new(8, 8), None);
        for (t, p, q, all) in [
            (true, false, false, false),
            (false, true, false, false),
            (false, false, true, false),
            (false, true, true, false),
            (false, false, false, true),
        ] {
            doc.add_layer("l".into(), RgbaImage::new(4, 4), (0, 0));
            let l = doc.active_layer_mut();
            l.locks = crate::document::Locks {
                transparent: t,
                pixels: p,
                position: q,
            };
            l.locked = all;
        }
        let (back, _) = write_read(&doc);
        for (x, y) in back.doc.layers.iter().zip(&doc.layers) {
            assert_eq!((x.locks, x.locked), (y.locks, y.locked));
        }
    }

    /// Styles on a group, a fill and an adjustment layer come back, without
    /// a note; a styled group keeps its fill opacity.
    #[test]
    fn styles_on_groups_fills_and_adjustments_round_trip() {
        let mut doc = Document::new(RgbaImage::from_pixel(16, 16, Rgba([9, 9, 9, 255])), None);
        doc.add_fill_layer(crate::document::Fill::Solid([200, 10, 10]), None);
        doc.active_layer_mut().effects.color_overlay.enabled = true;
        doc.add_adjustment_layer(crate::pixel_ops::Adjustment::Invert, None);
        doc.active_layer_mut().effects.inner_glow.enabled = true;
        doc.active = 0;
        doc.add_group();
        doc.active_layer_mut().effects.stroke.enabled = true;
        doc.active_layer_mut().fill = 0.5;
        let (back, notes) = write_read(&doc);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        for (x, y) in back.doc.layers.iter().zip(&doc.layers) {
            assert_eq!(x.effects.any(), y.effects.any(), "{}", x.name);
        }
        let group = back.doc.layers.iter().find(|l| l.is_group()).unwrap();
        assert!(group.effects.stroke.enabled);
        assert!((group.fill - 0.5).abs() < 0.01);
    }

    #[test]
    fn documents_round_trip() {
        let doc = sample();
        let (back, notes) = write_read(&doc);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        let b = &back.doc;
        assert_eq!((b.width(), b.height()), (40, 30));
        assert_eq!(b.layers.len(), doc.layers.len());
        for (x, y) in b.layers.iter().zip(&doc.layers) {
            assert_eq!(x.name, y.name);
            assert_eq!(
                (x.depth, x.visible, x.clipped, x.locked),
                (y.depth, y.visible, y.clipped, y.locked),
                "{}",
                x.name
            );
            assert_eq!(x.blend, y.blend, "{}", x.name);
            assert!((x.opacity - y.opacity).abs() < 0.003 && (x.fill - y.fill).abs() < 0.003);
            match (&x.kind, &y.kind) {
                (LayerKind::Fill(Fill::Gradient(p)), LayerKind::Fill(Fill::Gradient(q))) => {
                    assert_eq!(
                        (p.style, p.angle, p.scale, p.reverse),
                        (q.style, q.angle, q.scale, q.reverse)
                    );
                }
                (p, q) => assert_eq!(p, q, "{}", x.name),
            }
            let (p, q) = (&x.effects.shadow, &y.effects.shadow);
            assert!((p.dx - q.dx).abs() < 1e-3 && (p.dy - q.dy).abs() < 1e-3);
            assert_eq!(
                (p.enabled, p.blur, p.color, p.blend),
                (q.enabled, q.blur, q.color, q.blend)
            );
            assert_eq!(x.effects.stroke, y.effects.stroke);
            if y.has_pixels() {
                // The pixels, cropped to what they show.
                let r = y.content_bounds().unwrap();
                assert_eq!(x.rect(), r, "{}", x.name);
                assert_eq!(x.pixel_at(r.x, r.y), y.pixel_at(r.x, r.y));
            }
            assert_eq!(x.mask.is_some(), y.mask.is_some());
            if let (Some(p), Some(q)) = (&x.mask, &y.mask) {
                assert_eq!(p.image(), q.image(), "{}", x.name);
                assert_eq!(p.origin(x.offset), q.origin(y.offset));
                assert_eq!(
                    (p.default, p.enabled, p.feather),
                    (q.default, q.enabled, q.feather)
                );
                assert!((p.density - q.density).abs() < 0.003);
            }
        }
        assert_eq!(b.active, 2);
        assert_eq!(b.guides, doc.guides);
        assert_eq!(b.channels.len(), 1);
        assert_eq!(b.channels[0].name, "Sky");
        assert_eq!(b.channels[0].mask, doc.channels[0].mask);
        assert_eq!(b.icc_profile, doc.icc_profile);
        // The same image (gradient stops rounded to 1/4096).
        let diff = b
            .flatten()
            .as_raw()
            .iter()
            .zip(doc.flatten().as_raw())
            .map(|(p, q)| p.abs_diff(*q))
            .max();
        assert!(diff.is_some_and(|d| d <= 2), "{diff:?}");
    }

    #[test]
    fn every_adjustment_layer_round_trips() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, Rgba([90, 140, 200, 255])), None);
        let kinds = [
            AdjustKind::BrightnessContrast,
            AdjustKind::Levels,
            AdjustKind::Curves,
            AdjustKind::Vibrance,
            AdjustKind::HueSaturation,
            AdjustKind::ColorBalance,
            AdjustKind::BlackWhite,
            AdjustKind::PhotoFilter,
            AdjustKind::Invert,
            AdjustKind::Posterize,
            AdjustKind::Threshold,
            AdjustKind::GradientMap,
            AdjustKind::Exposure,
            AdjustKind::ChannelMixer,
            AdjustKind::SelectiveColor,
        ];
        for kind in kinds {
            doc.add_adjustment_layer(kind.initial(), None);
            doc.active_layer_mut().visible = false;
        }
        let (back, _) = write_read(&doc);
        let got: Vec<AdjustKind> = back
            .doc
            .layers
            .iter()
            .filter_map(|l| match &l.kind {
                LayerKind::Adjustment(a) => Some(a.kind()),
                _ => None,
            })
            .collect();
        assert_eq!(got, kinds);
        assert!(matches!(
            &back.doc.layers[9].kind,
            LayerKind::Adjustment(Adjustment::Invert)
        ));
    }

    #[test]
    fn transparent_documents_keep_their_merged_transparency() {
        let mut doc = Document::new(RgbaImage::new(3, 2), None);
        doc.layers[0].replace_pixels(RgbaImage::from_fn(3, 2, |x, _| {
            Rgba([255, 0, 0, x as u8 * 100])
        }));
        let (data, _) = write::document(&doc, None, false).unwrap();
        // Negative layer count: the first alpha channel is the merged
        // transparency; colors matted against white.
        let flat = read::merged_image(&data).unwrap();
        assert_eq!(flat.get_pixel(2, 0).0, [255, 0, 0, 200]);
        assert_eq!(flat.get_pixel(0, 1).0[3], 0);
    }

    /// A PSD with no layers and its merged image raw: 2 × 1, `mode`,
    /// `depth`, `planes` of samples.
    fn flat_file(mode: u16, depth: u16, planes: &[&[u8]]) -> Vec<u8> {
        let mut w = bytes::Writer::default();
        w.bytes(b"8BPS");
        w.u16(1);
        w.zeros(6);
        w.u16(planes.len() as u16);
        w.u32(1);
        w.u32(2);
        w.u16(depth);
        w.u16(mode);
        w.u32(0);
        w.u32(0);
        w.u32(0);
        w.u16(0);
        for p in planes {
            w.bytes(p);
        }
        w.data
    }

    #[test]
    fn reads_hand_built_files() {
        // RGB, 8 bits.
        let doc = read::document(&flat_file(3, 8, &[&[255, 0], &[0, 128], &[0, 255]]))
            .unwrap()
            .doc;
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].pixel_at(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(doc.layers[0].pixel_at(1, 0), Some([0, 128, 255, 255]));
        // Grayscale, 16 bits: converted, with notes.
        let imported = read::document(&flat_file(1, 16, &[&[0xff, 0xff, 0x80, 0x00]])).unwrap();
        assert_eq!(
            imported.doc.layers[0].pixel_at(1, 0),
            Some([128, 128, 128, 255])
        );
        assert_eq!(imported.notes.len(), 2);
        // CMYK (inverted samples: 255 is no ink): cyan, then black, through
        // the default CMYK profile (the file has none), into sRGB.
        let cmyk = flat_file(4, 8, &[&[0, 255], &[255, 255], &[255, 255], &[255, 0]]);
        let doc = read::document(&cmyk).unwrap().doc;
        let [r, g, b, a] = doc.layers[0].pixel_at(0, 0).unwrap();
        assert!(r < 10 && (150..170).contains(&g) && (215..235).contains(&b) && a == 255);
        let [r, g, b, _] = doc.layers[0].pixel_at(1, 0).unwrap();
        assert!(
            r < 60 && r.abs_diff(g) < 8 && g.abs_diff(b) < 8,
            "{:?}",
            [r, g, b]
        );
        let srgb = crate::cms::srgb().icc();
        assert_eq!(doc.icc_profile.as_deref(), Some(srgb.as_ref()));
        // Not a PSD; cut short.
        assert!(matches!(
            read::document(b"GIF89a....."),
            Err(FileError::NotPsd)
        ));
        let rgb = flat_file(3, 8, &[&[1, 2], &[3, 4], &[5, 6]]);
        assert!(read::document(&rgb[..rgb.len() - 1]).is_err());
    }

    #[test]
    fn sixteen_bit_layers_are_read_from_their_block() {
        // A layer in an "Lr16" global block, its channels raw.
        let mut info = bytes::Writer::default();
        info.i16(1);
        for v in [0i32, 0, 1, 1] {
            info.i32(v);
        }
        info.u16(4);
        for id in [-1i16, 0, 1, 2] {
            info.i16(id);
            info.u32(4);
        }
        info.bytes(b"8BIMnorm");
        info.bytes(&[255, 0, 8, 0]);
        info.block(1, |x| {
            x.u32(0);
            x.u32(0);
            x.pascal(b"deep", 4);
        });
        for v in [0xffffu16, 0x8000, 0x0000, 0xffff] {
            info.u16(0);
            info.u16(v);
        }
        let mut w = bytes::Writer::default();
        w.bytes(b"8BPS");
        w.u16(1);
        w.zeros(6);
        w.u16(3);
        w.u32(1);
        w.u32(1);
        w.u16(16);
        w.u16(3);
        w.u32(0);
        w.u32(0);
        w.block(1, |s| {
            s.u32(0);
            s.u32(0);
            s.tagged(b"Lr16", |x| x.bytes(&info.data));
        });
        w.u16(0);
        w.zeros(6);
        let doc = read::document(&w.data).unwrap().doc;
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].name, "deep");
        assert_eq!(doc.layers[0].pixel_at(0, 0), Some([128, 0, 255, 255]));
    }

    #[test]
    fn artboards_round_trip() {
        use crate::document::{ArtboardBackground, PixelRect};
        let mut doc = Document::with_artboard(50, 40, ArtboardBackground::Black);
        let rect = PixelRect {
            x: 70,
            y: 5,
            w: 20,
            h: 30,
        };
        doc.add_artboard(
            "new".into(),
            "B".into(),
            rect,
            ArtboardBackground::Custom([9, 80, 200]),
        );
        let (back, _) = write_read(&doc);
        let boards = |d: &Document| -> Vec<_> {
            d.layers
                .iter()
                .map(|l| (l.artboard, l.artboard_rect()))
                .collect()
        };
        let (doc_boards, back_boards) = (boards(&doc), boards(&back.doc));
        assert_eq!(back_boards, doc_boards);
        assert_eq!(back.doc.flatten(), doc.flatten());
    }

    /// A file with one layer of box `rect` (top, left, bottom, right) and
    /// no channel data, on a 4 × 4 canvas.
    fn one_layer(rect: [i32; 4]) -> Vec<u8> {
        let mut rec = bytes::Writer::default();
        for v in rect {
            rec.i32(v);
        }
        rec.u16(0);
        rec.bytes(b"8BIMnorm");
        rec.bytes(&[255, 0, 0, 0]);
        rec.u32(12);
        rec.zeros(8);
        rec.bytes(&[0, 0, 0, 0]);
        let mut info = bytes::Writer::default();
        info.u16(1);
        info.bytes(&rec.data);
        let mut w = bytes::Writer::default();
        w.bytes(b"8BPS");
        w.u16(1);
        w.zeros(6);
        w.u16(3);
        w.u32(4);
        w.u32(4);
        w.u16(8);
        w.u16(3);
        w.zeros(8);
        w.u32(info.data.len() as u32 + 4);
        w.u32(info.data.len() as u32);
        w.bytes(&info.data);
        w.u16(0);
        w.zeros(48);
        w.data
    }

    #[test]
    fn damaged_layer_boxes_never_take_the_memory() {
        // Whatever the machine has free: the budget is not what is tested.
        crate::document::set_test_free_memory(Some(64 << 30));
        // Far beyond any canvas, or overflowing: refused.
        for rect in [
            [0, 0, 1_000_000_000, 1_000_000_000],
            [i32::MIN, i32::MIN, i32::MAX, i32::MAX],
        ] {
            assert!(matches!(
                read::document(&one_layer(rect)),
                Err(FileError::Damaged)
            ));
        }
        // Large, but without any data: an empty layer, at once.
        let start = std::time::Instant::now();
        let doc = read::document(&one_layer([0, 0, 30_000, 30_000]))
            .unwrap()
            .doc;
        // Generous: a decode of 3.6 GB would take far longer, even unloaded.
        assert!(start.elapsed().as_secs() < 10);
        assert_eq!(doc.layers[0].image().dimensions(), (0, 0));
    }

    #[test]
    fn row_counts_must_fit_the_data() {
        // 30000 × 30000, RLE rows of 2 bytes: they can't fill 30000 bytes.
        let mut w = bytes::Writer::default();
        w.bytes(b"8BPS");
        w.u16(1);
        w.zeros(6);
        w.u16(3);
        w.u32(30_000);
        w.u32(30_000);
        w.u16(8);
        w.u16(3);
        w.zeros(12);
        w.u16(1);
        for _ in 0..3 * 30_000 {
            w.u16(2);
        }
        w.zeros(3 * 30_000 * 2);
        assert!(read::document(&w.data).is_err());
        // More channels than Photoshop allows.
        let mut many = flat_file(3, 8, &[&[0, 0], &[0, 0], &[0, 0]]);
        many[12..14].copy_from_slice(&65_535u16.to_be_bytes());
        assert!(matches!(read::document(&many), Err(FileError::Damaged)));
    }

    #[test]
    fn thumbnails_of_damaged_files_are_refused() {
        let dir = crate::testing::test_dir("psd-thumbnails");
        let path = dir.join("big.psd");
        // Too large (and zero wide) headers: no preview, no allocation.
        for (w, h) in [(1_000_000u32, 1_000_000u32), (0, 2)] {
            let mut data = flat_file(3, 8, &[&[1, 2], &[3, 4], &[5, 6]]);
            data[14..18].copy_from_slice(&h.to_be_bytes());
            data[18..22].copy_from_slice(&w.to_be_bytes());
            std::fs::write(&path, &data).unwrap();
            assert!(thumbnail_of(&path, &mut crate::document::Budget::new()).is_none());
        }
    }

    #[test]
    fn paths_round_trip_and_shapes_are_noted() {
        use crate::vector::{
            Align, Bounds, Cap, Join, Live, Paint, Path, Shape, ShapeKind, ShapeStroke, SubPath, pt,
        };
        let mut doc = Document::new(RgbaImage::from_pixel(60, 40, Rgba([255; 4])), None);
        let mut path = Path::default();
        path.subpaths
            .push(SubPath::polygon([pt(3.0, 3.0), pt(9.0, 3.0), pt(6.0, 8.0)]));
        let mut open = SubPath::polygon([pt(10.0, 20.0), pt(30.0, 25.0)]);
        open.closed = false;
        open.op = crate::vector::PathOp::Subtract;
        path.subpaths.push(open);
        doc.work_path = Some(path.clone());
        doc.paths.push(crate::document::SavedPath {
            name: "Outline".into(),
            path: path.clone(),
        });
        let (back, notes) = write_read(&doc);
        assert!(notes.is_empty(), "{notes:?}");
        let near = |a: &Path, b: &Path| {
            a.subpaths.len() == b.subpaths.len()
                && a.subpaths.iter().zip(&b.subpaths).all(|(s, t)| {
                    s.closed == t.closed
                        && s.op == t.op
                        && s.anchors
                            .iter()
                            .zip(&t.anchors)
                            .all(|(x, y)| x.p.dist(y.p) < 0.01)
                })
        };
        assert_eq!(back.doc.paths.len(), 1);
        assert_eq!(back.doc.paths[0].name, "Outline");
        assert!(near(&back.doc.paths[0].path, &path));
        assert!(near(back.doc.work_path.as_ref().unwrap(), &path));

        // A shape layer is saved as pixels, and says so: the document is
        // still to be saved.
        let live = Live::new(ShapeKind::Polygon, Bounds::of(pt(5.0, 5.0), pt(35.0, 30.0)));
        let stroke = ShapeStroke {
            paint: Paint::Color([0, 0, 0]),
            width: 3.0,
            align: Align::Outside,
            cap: Cap::Round,
            join: Join::Bevel,
            dash: None,
        };
        let shape = Shape::live(live, Paint::Color([10, 20, 30]), stroke);
        doc.add_shape_layer("Polygon".into(), "Polygon", shape);
        assert!(!doc.psd_losses().is_empty());
        let dir = crate::testing::test_dir("psd-shapes");
        let notes = doc.save(&dir.join("shape.psd")).unwrap();
        assert!(!notes.is_empty());
        assert!(doc.modified);
    }

    #[test]
    fn an_opened_psd_remembers_what_it_lost() {
        let dir = crate::testing::test_dir("psd-file-notes");
        let path = dir.join("gray16.psd");
        std::fs::write(&path, flat_file(1, 16, &[&[0xff, 0xff, 0x80, 0x00]])).unwrap();
        let opened = Document::open(&path).unwrap();
        assert!(!opened.notes.is_empty());
        let mut doc = opened.doc;
        assert_eq!(doc.file_notes, opened.notes);
        assert_eq!(doc.psd_losses(), opened.notes);
        // Saved as an Atelier document: nothing is lost any more.
        doc.save(&dir.join("gray16.atelier")).unwrap();
        assert!(doc.psd_losses().is_empty() && !doc.modified);
    }

    /// A PSD of a 4 × 4 canvas with one `side`² layer whose one channel
    /// holds only its PackBits row counts (no rows).
    fn rle_counts_only(side: u32) -> Vec<u8> {
        use bytes::Writer;
        let count = (side as usize).div_ceil(64) as u16;
        let mut chan = Writer::default();
        chan.u16(1);
        for _ in 0..side {
            chan.u16(count);
        }
        let mut rec = Writer::default();
        for v in [0, 0, side as i32, side as i32] {
            rec.i32(v);
        }
        rec.u16(1);
        rec.i16(0);
        rec.u32(chan.data.len() as u32);
        rec.bytes(b"8BIMnorm");
        rec.bytes(&[255, 0, 0, 0]);
        rec.u32(12);
        rec.zeros(8);
        rec.bytes(&[0, 0, 0, 0]);
        let mut info = Writer::default();
        info.u16(1);
        info.bytes(&rec.data);
        info.bytes(&chan.data);
        let mut w = Writer::default();
        w.bytes(b"8BPS");
        w.u16(1);
        w.zeros(6);
        w.u16(3);
        w.u32(4);
        w.u32(4);
        w.u16(8);
        w.u16(3);
        w.zeros(8);
        w.u32(info.data.len() as u32 + 4);
        w.u32(info.data.len() as u32);
        w.bytes(&info.data);
        w.u16(0);
        w.zeros(48);
        w.data
    }

    /// Row counts without their rows are damage, found before the
    /// channel is allocated; a PSD's layer is at most 30000 a side.
    #[test]
    fn layers_hold_what_their_data_can() {
        crate::document::set_test_free_memory(Some(64 << 30));
        for side in [30_000, 30_001, 120_000] {
            let started = std::time::Instant::now();
            let read = read::document(&rle_counts_only(side));
            assert!(matches!(read, Err(FileError::Damaged)), "{side}");
            assert!(started.elapsed().as_secs_f32() < 10.0, "{side}");
        }
    }

    /// The image resources that describe the document (resolution, XMP,
    /// IPTC…) are written back as they were; layer comps, which name
    /// layers by identifiers, are said to be left out.
    #[test]
    fn document_resources_survive_a_save() {
        use crate::i18n::t;
        let resource = |id: u16, data: &[u8]| crate::document::PsdResource {
            id,
            name: Vec::new(),
            data: data.to_vec(),
        };
        let mut ppi300 = Vec::new();
        for _ in 0..2 {
            ppi300.extend((300u32 << 16).to_be_bytes());
            ppi300.extend([0, 1, 0, 2]);
        }
        let kept = vec![
            resource(1005, &ppi300),
            resource(
                1060,
                b"<x:xmpmeta><dc:rights>QA Copyright</dc:rights></x:xmpmeta>",
            ),
            resource(1028, b"\x1c\x02\x74\x00\x02(c)"),
        ];
        let mut doc = sample();
        doc.file_metadata.psd_resources = Arc::new(kept.clone());
        let (back, notes) = write_read(&doc);
        assert!(
            notes.is_empty() && back.notes.is_empty(),
            "{:?}",
            back.notes
        );
        assert_eq!(*back.doc.file_metadata.psd_resources, kept);
        // Without any: 72 pixels per inch, as before.
        let (back, _) = write_read(&sample());
        assert_eq!(
            back.doc.file_metadata.psd_resources[0].data[..4],
            (72u32 << 16).to_be_bytes()
        );
        // Layer comps.
        let mut doc = sample();
        doc.file_metadata.psd_resources = Arc::new(vec![resource(1065, b"comps")]);
        let (back, _) = write_read(&doc);
        assert_eq!(back.notes, [t!("files4-psd-note-comps")]);
        assert!(
            back.doc
                .file_metadata
                .psd_resources
                .iter()
                .all(|r| r.id != 1065)
        );
        // What isn't kept is said, never dropped silently: a frame
        // animation, spot colors, variables, unknown data. Count data and
        // a one-frame animation go along or are left out without loss.
        let display = |kind: u8| {
            let mut d = 1u32.to_be_bytes().to_vec();
            d.extend([0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 100, kind]);
            d
        };
        let cases: [(Vec<crate::document::PsdResource>, Vec<String>); 9] = [
            (
                vec![resource(4000, b"maniIRFR\0\0\0\0")],
                vec![t!("files5-psd-note-animation")],
            ),
            (vec![resource(1077, &display(0))], vec![]),
            (
                vec![resource(7000, b"vars")],
                vec![t!("files5-psd-note-variables")],
            ),
            (
                vec![resource(9999, b"?")],
                vec![t!("files6-psd-note-other", ids = "9999")],
            ),
            (vec![resource(1080, b"count")], vec![]),
            // Alternate spot colors without any (Photoshop writes it so).
            (vec![resource(1067, &[0, 1, 0, 0])], vec![]),
            (
                vec![resource(
                    1067,
                    &[0, 1, 0, 1, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                )],
                vec![t!("files5-psd-note-spot")],
            ),
            // Unknown ones are named; harmless known ones go along.
            (
                vec![
                    resource(1068, b"?"),
                    resource(1079, b"?"),
                    resource(1068, b"?"),
                ],
                vec![t!("files6-psd-note-other", ids = "1068, 1079")],
            ),
            (
                vec![resource(1030, b"quality"), resource(1040, b"mark")],
                vec![],
            ),
        ];
        for (resources, notes) in cases {
            let mut doc = sample();
            doc.file_metadata.psd_resources = Arc::new(resources.clone());
            let (back, _) = write_read(&doc);
            assert_eq!(back.notes, notes, "{:?}", resources[0].id);
        }
        // The XMP describes the image saved: upright, its size.
        let mut doc = sample();
        let xmp = b"<x:xmpmeta tiff:Orientation=\"6\" exif:PixelXDimension=\"9\"/>";
        doc.file_metadata.psd_resources = Arc::new(vec![resource(1060, xmp)]);
        let (back, _) = write_read(&doc);
        let resources = &back.doc.file_metadata.psd_resources;
        let xmp = &resources.iter().find(|r| r.id == 1060).unwrap().data;
        let xmp = String::from_utf8_lossy(xmp);
        assert!(xmp.contains("tiff:Orientation=\"1\""), "{xmp}");
        assert!(
            xmp.contains(&format!("exif:PixelXDimension=\"{}\"", doc.width())),
            "{xmp}"
        );
        let mut doc = sample();
        doc.file_metadata.psd_resources = Arc::new(vec![resource(1080, b"count")]);
        let (back, _) = write_read(&doc);
        assert!(
            back.doc
                .file_metadata
                .psd_resources
                .iter()
                .any(|r| r.id == 1080)
        );
    }

    /// The alpha channels' display settings go back to the channels of
    /// their name; what is placed on the canvas goes back only to a canvas
    /// of the same size, said otherwise (but the whole image's own slice,
    /// which Photoshop makes again).
    #[test]
    fn display_settings_and_placed_resources_follow_the_document() {
        use crate::i18n::t;
        let resource = |id: u16, data: &[u8]| crate::document::PsdResource {
            id,
            name: Vec::new(),
            data: data.to_vec(),
        };
        let entry = |opacity: u8, kind: u8| [0, 0, 0, 0xFF, 0, 0, 0, 0, 0, 0, 0, opacity, kind];
        let display = [&1u32.to_be_bytes()[..], &entry(30, 1), &entry(70, 2)].concat();
        let mut doc = sample();
        doc.channels.clear();
        let mask = Arc::new(image::GrayImage::new(40, 30));
        for name in ["A", "B"] {
            doc.channels.push(crate::document::Channel {
                name: name.into(),
                mask: mask.clone(),
            });
        }
        let mut slices = 6u32.to_be_bytes().to_vec();
        slices.extend([0; 16]);
        slices.extend(0u32.to_be_bytes());
        slices.extend(1u32.to_be_bytes());
        doc.file_metadata.psd_resources =
            Arc::new(vec![resource(1077, &display), resource(1050, &slices)]);
        doc.file_metadata.psd_layout = Some(crate::document::PsdLayout {
            canvas: (40, 30),
            channels: vec!["A".into(), "B".into()],
        });
        let (back, notes) = write_read(&doc);
        assert!(notes.is_empty() && back.notes.is_empty(), "{notes:?}");
        let find = |d: &Document, id: u16| {
            (d.file_metadata.psd_resources.iter())
                .find(|r| r.id == id)
                .map(|r| r.data.clone())
        };
        // The spot color became an alpha channel, said so on opening.
        let mut alpha = display.clone();
        *alpha.last_mut().unwrap() = 0;
        assert_eq!(find(&back.doc, 1077).unwrap(), alpha);
        assert!(find(&back.doc, 1050).is_some());
        // B removed, C added: B's settings go, C gets the default.
        doc.channels.remove(1);
        doc.channels.push(crate::document::Channel {
            name: "C".into(),
            mask: mask.clone(),
        });
        let (back, _) = write_read(&doc);
        let data = find(&back.doc, 1077).unwrap();
        assert_eq!(&data[4..17], &entry(30, 1));
        assert_eq!(data[4 + 13 + 11], 50);
        // Another canvas: the whole image's slice left out without loss,
        // color samplers said.
        doc.crop(crate::document::PixelRect {
            x: 0,
            y: 0,
            w: 20,
            h: 10,
        });
        let (back, notes) = write_read(&doc);
        assert!(notes.is_empty(), "{notes:?}");
        assert!(find(&back.doc, 1050).is_none());
        doc.file_metadata.psd_resources = Arc::new(vec![resource(1073, b"samplers")]);
        assert_eq!(losses(&doc, false), [t!("files6-psd-note-placed")]);
        let (back, _) = write_read(&doc);
        assert!(find(&back.doc, 1073).is_none());
    }

    /// A layer wider than the canvas's limit (Image Size on a layer that
    /// reaches beyond the canvas) is written and read back, in both formats.
    #[test]
    fn layers_wider_than_a_canvas_round_trip() {
        let mut doc = Document::new(RgbaImage::from_pixel(20_000, 2, Rgba([9, 9, 9, 255])), None);
        doc.add_layer(
            "Panorama".into(),
            RgbaImage::from_pixel(37_500, 2, Rgba([200, 10, 10, 255])),
            (-3000, 0),
        );
        // A PSD's sides stop at 30,000 px (said), a PSB's don't.
        let (back, notes) = write_read(&doc);
        assert_eq!(back.doc.layers[1].image().dimensions(), (30_000, 2));
        assert_eq!(notes.len(), 1);
        let (data, notes) = write::document(&doc, None, true).unwrap();
        assert!(notes.is_empty());
        let back = read::document(&data).unwrap();
        assert_eq!(back.doc.layers[1].image().dimensions(), (37_500, 2));
        let dir = crate::testing::test_dir("psd-wide");
        let path = dir.join("wide.atelier");
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert_eq!(back.layers[1].image().dimensions(), (37_500, 2));
    }

    /// The layers of a file decode at once: refused, not aborted, when
    /// they need more memory than the computer has.
    #[test]
    fn the_layers_must_fit_in_memory() {
        let (data, _) = write::document(&sample(), None, false).unwrap();
        assert!(read::document(&data).is_ok());
        assert!(matches!(
            read::document_within(&data, crate::document::file_io::Budget::of(1000)),
            Err(FileError::OutOfMemory(1000))
        ));
    }

    /// Saving over a PSD keeps as many alpha channels as the file writes:
    /// an opaque document has room for one more.
    #[test]
    fn alpha_channels_lost_as_the_file_writes_them() {
        let channels = |doc: &mut Document, n: usize| {
            doc.channels = (0..n)
                .map(|i| Channel {
                    name: format!("A{i}"),
                    mask: Arc::new(GrayImage::new(4, 4)),
                })
                .collect();
        };
        let mut opaque = Document::new(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255])), None);
        channels(&mut opaque, 53);
        assert!(losses(&opaque, false).is_empty());
        assert!(write::document(&opaque, None, false).unwrap().1.is_empty());
        channels(&mut opaque, 54);
        assert!(!losses(&opaque, false).is_empty());
        let mut clear = Document::new(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 0])), None);
        channels(&mut clear, 52);
        assert!(losses(&clear, false).is_empty());
        channels(&mut clear, 53);
        assert_eq!(losses(&clear, false), write::document(&clear, None, false).unwrap().1);
    }
}

#[cfg(test)]
mod pattern_files {
    use super::*;
    use crate::compose::pattern::{Pattern, PatternFill};
    use crate::compose::styles::Effects;
    use crate::document::{Document, Fill, LayerKind};
    use image::{Rgba, RgbaImage};

    fn doc_with(f: PatternFill) -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255; 4])), None);
        doc.add_fill_layer(Fill::Pattern(f), None);
        doc
    }

    fn tile() -> std::sync::Arc<Pattern> {
        Pattern::new(
            "t",
            RgbaImage::from_fn(3, 1, |x, _| Rgba([x as u8 * 100, 0, 0, 255])),
        )
    }

    fn find(data: &[u8], key: &[u8]) -> usize {
        data.windows(key.len()).position(|w| w == key).expect("key")
    }

    #[test]
    fn the_three_names_of_the_tiles_block_are_read() {
        let (mut data, _) = write::document(&doc_with(PatternFill::of(tile())), None, false).unwrap();
        let at = find(&data, b"8BIMPatt");
        for key in [b"Pat2", b"Pat3"] {
            data[at + 4..at + 8].copy_from_slice(key);
            let back = read::document(&data).unwrap();
            assert!(matches!(
                back.doc.layers[1].kind,
                LayerKind::Fill(Fill::Pattern(_))
            ));
            assert!(back.notes.is_empty(), "{:?}", back.notes);
        }
    }

    #[test]
    fn a_fill_without_its_tile_stays_a_layer_and_says_so() {
        let (mut data, _) = write::document(&doc_with(PatternFill::of(tile())), None, false).unwrap();
        let at = find(&data, b"8BIMPatt");
        data[at + 4..at + 8].copy_from_slice(b"Xxxx");
        let back = read::document(&data).unwrap();
        assert_eq!(back.doc.layers.len(), 2, "kept");
        assert!(back.doc.layers[1].has_pixels());
        assert_eq!(back.notes.len(), 1, "{:?}", back.notes);
    }

    /// The list's rectangle patched: a few bytes for a big tile.
    #[test]
    fn a_tile_bigger_than_its_data_is_refused_within_the_budget() {
        let (mut data, _) = write::document(&doc_with(PatternFill::of(tile())), None, false).unwrap();
        let at = find(&data, b"8BIMPatt");
        let id = data[at + 8 + 4 + 4 + 4 + 4 + 4 + 4 + 2] as usize;
        let rect = at + 8 + 4 + 4 + 4 + 4 + 4 + 4 + 2 + 1 + id + 4 + 4;
        assert_eq!(&data[rect + 8..rect + 16], &[0, 0, 0, 1, 0, 0, 0, 3]);
        let side = 12000i32;
        data[rect + 8..rect + 12].copy_from_slice(&side.to_be_bytes());
        data[rect + 12..rect + 16].copy_from_slice(&side.to_be_bytes());
        let budget = crate::document::file_io::Budget::of(10_000_000);
        // Refused for not holding its data: the layer stays, as pixels.
        let back = read::document_within(&data, budget).expect("read");
        assert!(!matches!(
            back.doc.layers[1].kind,
            LayerKind::Fill(Fill::Pattern(_))
        ));
    }

    #[test]
    fn a_linked_tile_stays_in_place_through_a_file() {
        let t = Pattern::new(
            "asym",
            RgbaImage::from_fn(5, 3, |x, y| Rgba([(x * 50) as u8, (y * 80) as u8, 7, 255])),
        );
        // A moved fill, and a pixel layer with margins: the file trims them.
        let mut doc = doc_with(PatternFill::of(t.clone()));
        doc.active_layer_mut().translate(2, 1);
        let mut image = RgbaImage::new(16, 16);
        for y in 4..12 {
            for x in 3..11 {
                image.put_pixel(x, y, Rgba([1, 2, 3, 255]));
            }
        }
        doc.add_layer("x".into(), image, (2, 2));
        let fx = &mut doc.active_layer_mut().effects;
        fx.pattern_overlay.enabled = true;
        fx.pattern_overlay.fill = PatternFill::of(t);
        let (data, _) = write::document(&doc, None, false).unwrap();
        let back = read::document(&data).unwrap().doc;
        assert_eq!(doc.flatten(), back.flatten());
    }

    #[test]
    fn styles_turned_off_keep_their_settings_through_a_file() {
        let mut doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255; 4])), None);
        doc.add_layer("x".into(), RgbaImage::from_pixel(8, 8, Rgba([9, 8, 7, 255])), (4, 4));
        let fx = &mut doc.active_layer_mut().effects;
        fx.pattern_overlay.fill = PatternFill::of(tile());
        fx.pattern_overlay.fill.scale = 2.0;
        fx.pattern_overlay.opacity = 0.3;
        fx.shadow.blur = 11.0;
        fx.shadow.opacity = 0.25;
        fx.bevel.depth = 3.0;
        fx.stroke.enabled = true;
        fx.stroke.width = 2.0;
        let before = fx.clone();
        assert!(doc.psd_losses().is_empty());
        let (data, notes) = write::document(&doc, None, false).unwrap();
        assert!(notes.is_empty());
        let opened = read::document(&data).unwrap();
        assert!(opened.notes.is_empty(), "{:?}", opened.notes);
        let back = &opened.doc.active_layer().effects;
        assert!(!back.pattern_overlay.enabled && !back.shadow.enabled && !back.bevel.enabled);
        assert!(back.stroke.enabled);
        let p = &back.pattern_overlay;
        assert_eq!(
            (&*p.fill.pattern.image, p.fill.scale, p.opacity),
            (&*before.pattern_overlay.fill.pattern.image, 2.0, 0.3)
        );
        assert_eq!((back.shadow.blur, back.shadow.opacity), (11.0, 0.25));
        assert_eq!(back.bevel, before.bevel);
        assert_eq!(back.stroke, before.stroke);
        // Untouched styles that are off stay out of the file.
        assert_eq!(back.inner_glow, Effects::default().inner_glow);
        assert_eq!(doc.flatten(), opened.doc.flatten());
    }
}

/// Interoperability with the other readers (ag-psd, psd-tools): sizes,
/// PSB, clipping, links, path names, Hue/Saturation ranges.
#[cfg(test)]
mod interop_tests {
    use image::{GrayImage, Luma, Rgba, RgbaImage};

    use super::*;
    use crate::document::{LayerKind, Mask};
    use crate::i18n::t;
    use crate::pixel_ops::{Adjustment, HueSaturation};

    /// Pixels that don't pack (a PackBits row then outgrows its width).
    fn noise(w: u32, h: u32) -> RgbaImage {
        let mut seed = 12345u32;
        RgbaImage::from_fn(w, h, |_, _| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let [a, b, c, _] = seed.to_be_bytes();
            Rgba([a, b, c, 255])
        })
    }

    fn wide(w: u32) -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 4, Rgba([9, 9, 9, 255])), None);
        doc.add_layer("Wide".into(), noise(w, 4), (-1000, 0));
        doc
    }

    /// A layer over 30,000 px a side is cropped in a PSD (which other
    /// readers then take, and which reopens), and said first; a PSB keeps
    /// it whole.
    #[test]
    fn layers_past_a_psd_side_are_cropped_and_a_psb_keeps_them() {
        let doc = wide(70_000);
        let cropped = t!("psd14-psd-note-cropped", names = "Wide", max = 30_000);
        assert_eq!(losses(&doc, false), std::slice::from_ref(&cropped));
        let (data, notes) = write::document(&doc, None, false).unwrap();
        assert_eq!(notes, [cropped]);
        let back = read::document(&data).unwrap().doc;
        let layer = &back.layers[1];
        assert_eq!((layer.image().dimensions(), layer.offset), ((30_000, 4), (-1000, 0)));
        assert_eq!(back.flatten(), doc.flatten());
        let src = doc.layers[1].image();
        assert_eq!(layer.image().get_pixel(29_999, 3), src.get_pixel(29_999, 3));

        assert!(losses(&doc, true).is_empty());
        let (data, notes) = write::document(&doc, None, true).unwrap();
        assert!(notes.is_empty());
        assert_eq!(&data[4..6], &[0, 2]);
        let back = read::document(&data).unwrap();
        assert!(back.notes.is_empty(), "{:?}", back.notes);
        assert_eq!(back.doc.layers[1].image(), doc.layers[1].image());
    }

    /// The same for a mask, whose rows would overflow a PSD's row lengths.
    #[test]
    fn masks_past_a_psd_side_are_cropped_and_a_psb_keeps_them() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 4, Rgba([9, 9, 9, 255])), None);
        doc.add_layer("Masked".into(), RgbaImage::from_pixel(10, 4, Rgba([1, 2, 3, 255])), (0, 0));
        let mask = GrayImage::from_fn(70_000, 4, |x, _| Luma([(x * 7919 % 251) as u8]));
        doc.active_layer_mut().mask = Some(Mask::new(mask.clone(), (-5, 0), 0, (0, 0)));
        let (data, notes) = write::document(&doc, None, false).unwrap();
        assert_eq!(notes.len(), 1);
        let back = read::document(&data).unwrap().doc;
        let m = back.layers[1].mask.as_ref().unwrap();
        assert_eq!(m.image().dimensions(), (30_000, 4));
        assert_eq!(back.flatten(), doc.flatten());
        let (data, _) = write::document(&doc, None, true).unwrap();
        let back = read::document(&data).unwrap().doc;
        assert_eq!(back.layers[1].mask.as_ref().unwrap().image(), &mask);
    }

    /// Saving to a .psb writes a PSB, which Save keeps.
    #[test]
    fn a_psb_file_is_saved_as_a_psb() {
        let dir = crate::testing::test_dir("psd-psb-save");
        let path = dir.join("big.psb");
        let mut doc = wide(40_000);
        assert!(doc.save(&path).unwrap().is_empty());
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[..6], b"8BPS\0\x02");
        let opened = Document::open(&path).unwrap();
        assert!(opened.doc.psd_losses().is_empty());
        assert_eq!(opened.doc.layers[1].image().width(), 40_000);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A clipped layer at the bottom of its group is written unclipped, as
    /// Atelier draws it (other readers would clip it to the group's end).
    #[test]
    fn clipping_at_the_bottom_of_a_group_is_not_written() {
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([255; 4])), None);
        doc.add_layer("base".into(), RgbaImage::from_pixel(4, 4, Rgba([0, 0, 255, 255])), (0, 0));
        doc.add_layer("clipped".into(), RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255])), (0, 0));
        doc.active_layer_mut().clipped = true;
        doc.add_layer("alone".into(), RgbaImage::from_pixel(2, 2, Rgba([0, 255, 0, 255])), (0, 0));
        doc.active_layer_mut().clipped = true;
        let top = doc.layers.len() - 1;
        assert!(doc.group_layers(&[top]));
        let alone = doc.layers.iter().position(|l| l.name == "alone").unwrap();
        doc.layers[alone].clipped = true;
        let (data, _) = write::document(&doc, None, false).unwrap();
        let flags = read::records_of(&data);
        let flag = |name: &str| flags.iter().find(|(n, _)| n == name).unwrap().1;
        assert!(flag("clipped"));
        assert!(!flag("alone"));
    }

    /// Linked layers stay linked through a PSD (resources 1026 and 1072).
    #[test]
    fn linked_layers_keep_their_link() {
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([255; 4])), None);
        for name in ["a", "b", "c", "d"] {
            doc.add_layer(name.into(), RgbaImage::from_pixel(2, 2, Rgba([1, 2, 3, 255])), (0, 0));
        }
        assert!(doc.toggle_link(&[1, 3]));
        let (data, _) = write::document(&doc, None, false).unwrap();
        let back = read::document(&data).unwrap();
        assert!(back.notes.is_empty());
        let links: Vec<Option<u64>> = back.doc.layers.iter().map(|l| l.link).collect();
        assert!(links[1].is_some() && links[1] == links[3]);
        assert_eq!((links[0], links[2], links[4]), (None, None, None));
        // Moving one moves the other.
        assert_eq!(back.doc.moving_with(1), [1, 3]);
    }

    /// Path names outside Mac OS Roman come back whole ("pths").
    #[test]
    fn saved_path_names_keep_every_character() {
        let mut doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255; 4])), None);
        let mut path = crate::vector::Path::default();
        let corners = [(2.0, 2.0), (9.0, 2.0), (6.0, 8.0)].map(|(x, y)| crate::vector::pt(x, y));
        path.subpaths.push(crate::vector::SubPath::polygon(corners));
        for name in ["路径 1", "Plain", "Émoji 😀"] {
            doc.paths.push(crate::document::SavedPath {
                name: name.into(),
                path: path.clone(),
            });
        }
        for big in [false, true] {
            let (data, _) = write::document(&doc, None, big).unwrap();
            let back = read::document(&data).unwrap().doc;
            let names: Vec<&str> = back.paths.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, ["路径 1", "Plain", "Émoji 😀"]);
        }
    }

    /// A clipping path naming a path no longer there is left out.
    #[test]
    fn a_clipping_path_goes_with_its_path() {
        let mut doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255; 4])), None);
        let data = [&[4u8][..], b"Gone", &[0, 1, 0, 0, 0, 1]].concat();
        doc.file_metadata.psd_resources = std::sync::Arc::new(vec![crate::document::PsdResource {
            id: 2999,
            name: Vec::new(),
            data,
        }]);
        let (file, _) = write::document(&doc, None, false).unwrap();
        let has = |file: &[u8]| file.windows(6).any(|w| w == [b'8', b'B', b'I', b'M', 0x0B, 0xB7]);
        assert!(!has(&file));
        let mut path = crate::vector::Path::default();
        let corners = [(2.0, 2.0), (9.0, 2.0), (6.0, 8.0)].map(|(x, y)| crate::vector::pt(x, y));
        path.subpaths.push(crate::vector::SubPath::polygon(corners));
        doc.paths.push(crate::document::SavedPath {
            name: "Gone".into(),
            path,
        });
        let (file, _) = write::document(&doc, None, false).unwrap();
        assert!(has(&file));
    }

    /// The default Pattern Overlay, unused, stays out of the file of a layer
    /// whose pixels have margins (text, shapes): it reopens without loss.
    #[test]
    fn an_unused_pattern_overlay_is_not_written() {
        let mut doc = Document::new(RgbaImage::from_pixel(40, 40, Rgba([255; 4])), None);
        let mut image = RgbaImage::new(20, 20);
        for y in 5..15 {
            for x in 4..12 {
                image.put_pixel(x, y, Rgba([1, 2, 3, 255]));
            }
        }
        doc.add_layer("Hello".into(), image, (3, 3));
        let (data, _) = write::document(&doc, None, false).unwrap();
        assert!(!data.windows(11).any(|w| w == b"patternFill"));
        let mut opened = read::document(&data).unwrap();
        assert!(opened.notes.is_empty(), "{:?}", opened.notes);
        opened.doc.file_notes = opened.notes;
        assert!(opened.doc.psd_losses().is_empty());
    }

    /// Hue/Saturation's ranges: drawn, and kept by both formats.
    #[test]
    fn hue_saturation_ranges_render_and_round_trip() {
        let mut hs = HueSaturation::default();
        hs.ranges.0[0].saturation = -1.0;
        hs.ranges.0[4].hue = 30.0;
        let mut image = RgbaImage::from_vec(3, 1, vec![220, 30, 30, 255, 30, 200, 40, 255, 128, 128, 128, 255]).unwrap();
        crate::pixel_ops::hue_saturation(&mut image, &hs);
        let red = image.get_pixel(0, 0).0;
        assert!(red[0] == red[1] && red[1] == red[2], "{red:?}");
        assert_eq!(image.get_pixel(1, 0).0, [30, 200, 40, 255]);
        assert_eq!(image.get_pixel(2, 0).0, [128, 128, 128, 255]);
        assert!(!hs.is_neutral());

        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([220, 30, 30, 255])), None);
        doc.add_adjustment_layer(Adjustment::HueSaturation(hs), None);
        let (data, notes) = write::document(&doc, None, false).unwrap();
        assert!(notes.is_empty());
        let back = read::document(&data).unwrap().doc;
        let Some(LayerKind::Adjustment(Adjustment::HueSaturation(read))) =
            back.layers.last().map(|l| &l.kind)
        else {
            panic!("no adjustment");
        };
        assert_eq!(read.ranges, hs.ranges);
        assert_eq!(back.flatten(), doc.flatten());
        let dir = crate::testing::test_dir("psd-hue-ranges");
        let path = dir.join("hue.atelier");
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert!(matches!(
            &back.layers.last().unwrap().kind,
            LayerKind::Adjustment(Adjustment::HueSaturation(h)) if h.ranges == hs.ranges
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A Brightness/Contrast in the legacy algorithm is said on opening.
    #[test]
    fn legacy_brightness_contrast_is_said() {
        let brit: &[u8] = &[0, 50, 0, 0, 0, 0, 0, 0];
        assert!(adjust::legacy_brightness(&[(*b"brit", brit)]));
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([9; 4])), None);
        doc.add_adjustment_layer(Adjustment::BrightnessContrast(Default::default()), None);
        let (data, _) = write::document(&doc, None, false).unwrap();
        assert!(read::document(&data).unwrap().notes.is_empty());
    }
}
