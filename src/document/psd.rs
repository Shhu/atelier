//! Photoshop documents (.psd, and .psb for reading), after Adobe's
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
//! compressed) keeps the layers with all of the
//! above, the saved selections as alpha channels, the guides, the color
//! profile, and the flattened image for the applications that only read
//! that.

use std::path::Path;

use super::{BlendMode, Document, FileError};

mod adjust;
mod artboard;
mod bytes;
mod descriptor;
mod effects;
mod read;
mod vector;
mod write;

pub use read::{canvas_size, thumbnail_of};

/// Whether `path` names a Photoshop document (by its extension).
pub fn is_psd(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("psd") || e.eq_ignore_ascii_case("psb"))
}

/// A Photoshop document read, and what had to be simplified to open it
/// (translated, for the status bar).
pub struct Imported {
    pub doc: Document,
    pub notes: Vec<String>,
}

/// Reads a Photoshop document.
pub fn read(path: &Path) -> Result<Imported, FileError> {
    let data = std::fs::read(path)?;
    let mut imported = read::document(&data)?;
    imported.doc.path = Some(path.to_path_buf());
    Ok(imported)
}

/// Writes `doc` as a Photoshop document; returns what it could not keep
/// (translated notes).
pub fn write(doc: &Document, out: &mut impl std::io::Write) -> Result<Vec<String>, FileError> {
    let (data, notes) = write::document(doc, None)?;
    out.write_all(&data)?;
    Ok(notes)
}

/// [`write`] in CMYK: every layer and the merged image converted by
/// `cmyk` (to its CMYK profile, which the file embeds).
pub fn write_cmyk(
    doc: &Document,
    out: &mut impl std::io::Write,
    cmyk: &crate::cms::Conversion,
) -> Result<Vec<String>, FileError> {
    let (data, notes) = write::document(doc, Some(cmyk))?;
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
        let (data, notes) = write::document(doc, None).unwrap();
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
        let (data, _) = write::document(&doc, None).unwrap();
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
}
