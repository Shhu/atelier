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

/// What saving `doc` as a Photoshop document would lose, as [`write`]
/// says it (translated notes).
pub fn losses(doc: &Document) -> Vec<String> {
    write::losses(doc)
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
        assert!(start.elapsed().as_secs() < 2);
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
        let dir = std::env::temp_dir().join("atelier-psd-thumbnails");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.psd");
        // Too large (and zero wide) headers: no preview, no allocation.
        for (w, h) in [(1_000_000u32, 1_000_000u32), (0, 2)] {
            let mut data = flat_file(3, 8, &[&[1, 2], &[3, 4], &[5, 6]]);
            data[14..18].copy_from_slice(&h.to_be_bytes());
            data[18..22].copy_from_slice(&w.to_be_bytes());
            std::fs::write(&path, &data).unwrap();
            assert!(thumbnail_of(&path).is_none());
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
        let dir = std::env::temp_dir().join("atelier-psd-shapes");
        std::fs::create_dir_all(&dir).unwrap();
        let notes = doc.save(&dir.join("shape.psd")).unwrap();
        assert!(!notes.is_empty());
        assert!(doc.modified);
    }

    #[test]
    fn an_opened_psd_remembers_what_it_lost() {
        let dir = std::env::temp_dir().join("atelier-psd-file-notes");
        std::fs::create_dir_all(&dir).unwrap();
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
        for side in [30_000, 30_001, 120_000] {
            let started = std::time::Instant::now();
            let read = read::document(&rle_counts_only(side));
            assert!(matches!(read, Err(FileError::Damaged)), "{side}");
            assert!(started.elapsed().as_secs_f32() < 1.0, "{side}");
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
        let cases: [(Vec<crate::document::PsdResource>, Vec<String>); 6] = [
            (
                vec![resource(4000, b"maniIRFR\0\0\0\0")],
                vec![t!("files5-psd-note-animation")],
            ),
            (
                vec![resource(1077, &display(2))],
                vec![t!("files5-psd-note-spot")],
            ),
            (vec![resource(1077, &display(0))], vec![]),
            (
                vec![resource(7000, b"vars")],
                vec![t!("files5-psd-note-variables")],
            ),
            (
                vec![resource(9999, b"?")],
                vec![t!("files5-psd-note-other")],
            ),
            (vec![resource(1080, b"count")], vec![]),
        ];
        for (resources, notes) in cases {
            let mut doc = sample();
            doc.file_metadata.psd_resources = Arc::new(resources.clone());
            let (back, _) = write_read(&doc);
            assert_eq!(back.notes, notes, "{:?}", resources[0].id);
        }
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
        let (back, _) = write_read(&doc);
        assert_eq!(back.doc.layers[1].image().dimensions(), (37_500, 2));
        let dir = std::env::temp_dir().join("atelier-psd-wide");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.atelier");
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert_eq!(back.layers[1].image().dimensions(), (37_500, 2));
    }

    /// The layers of a file decode at once: refused, not aborted, when
    /// they need more memory than the computer has.
    #[test]
    fn the_layers_must_fit_in_memory() {
        let (data, _) = write::document(&sample(), None).unwrap();
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
        assert!(losses(&opaque).is_empty());
        assert!(write::document(&opaque, None).unwrap().1.is_empty());
        channels(&mut opaque, 54);
        assert!(!losses(&opaque).is_empty());
        let mut clear = Document::new(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 0])), None);
        channels(&mut clear, 52);
        assert!(losses(&clear).is_empty());
        channels(&mut clear, 53);
        assert_eq!(losses(&clear), write::document(&clear, None).unwrap().1);
    }
}
