//! Smart objects in Atelier documents: their contents as an Atelier
//! document of their own inside the archive (`smart/<id>.atelier`, once
//! for the instances sharing them), their link, transform, smart filters
//! and filter mask (`smart/masks/<n>.png`). Their layer's pixels stay the
//! rendering, so that they show as saved.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use super::*;
use crate::document::{Contents, Link, SmartFilter, SmartFilterKind, SmartObject};
use crate::liquify::Mesh;
use crate::xform::{self, Homography, Map, Warp};

#[derive(Serialize, Deserialize)]
pub(super) struct SmartEntry {
    /// Archive path of the contents.
    contents: String,
    /// Shared by the instances of the same contents.
    id: u64,
    name: String,
    #[serde(default)]
    link: Option<LinkEntry>,
    /// Contents → layer (its coordinates).
    transform: MapEntry,
    #[serde(default)]
    interpolation: InterpolationEntry,
    #[serde(default)]
    filters: Vec<FilterEntry>,
    #[serde(default = "yes")]
    filters_enabled: bool,
    #[serde(default)]
    filter_mask: Option<MaskEntry>,
}

fn yes() -> bool {
    true
}

/// Place Linked: the file, also relative to the document's folder (found
/// there when the whole folder moved).
#[derive(Serialize, Deserialize)]
struct LinkEntry {
    path: PathBuf,
    #[serde(default)]
    relative: Option<PathBuf>,
    /// Modification time of the file when last read: seconds since 1970,
    /// nanoseconds.
    #[serde(default)]
    modified: Option<(u64, u32)>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum MapEntry {
    Projective {
        matrix: [f64; 9],
    },
    /// Bezier patch over the contents' box, by rows from the top.
    Warp {
        points: [[[f64; 2]; 4]; 4],
    },
}

#[derive(Serialize, Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum InterpolationEntry {
    Nearest,
    Bilinear,
    #[default]
    Bicubic,
    Lanczos,
}

#[derive(Serialize, Deserialize)]
struct FilterEntry {
    /// A filter and its settings, as the Actions panel records them…
    #[serde(default)]
    filter: Option<crate::filters::Filter>,
    /// …or Liquify: the archive path of its mesh (`smart/meshes/<n>.msh`).
    #[serde(default)]
    liquify: Option<String>,
    /// …or the Camera Raw Filter: its settings.
    #[serde(default)]
    camera_raw: Option<crate::camera_raw::Settings>,
    visible: bool,
    blend: Blend,
    opacity: f32,
}

/// The manifest's entry of a mask (its image's archive path left to set).
pub(super) fn mask_entry(m: &Mask, layer_offset: (i64, i64)) -> MaskEntry {
    let (x, y) = m.origin(layer_offset);
    MaskEntry {
        image: None,
        width: m.image().width(),
        height: m.image().height(),
        x,
        y,
        default: m.default,
        enabled: m.enabled,
        linked: m.linked(),
        density: m.density,
        feather: m.feather,
    }
}

/// The mask of entry `m` with its pixels `image`, for a layer at
/// `layer_offset`.
pub(super) fn mask_of(m: &MaskEntry, image: GrayImage, layer_offset: (i64, i64)) -> Mask {
    let mut mask = Mask::new(image, (m.x, m.y), m.default, layer_offset);
    mask.enabled = m.enabled;
    mask.set_linked(m.linked, layer_offset);
    mask.set_origin((m.x, m.y), layer_offset);
    mask.density = finite_or(m.density, 1.0).clamp(0.0, 1.0);
    mask.feather = finite_or(m.feather, 0.0).clamp(0.0, 1000.0);
    mask
}

/// The smart objects of `layers`: their entries in `entries`, their
/// contents and filter masks added to `files`.
pub(super) fn write_all(
    layers: &[super::super::Layer],
    entries: &mut [LayerEntry],
    files: &mut Vec<(String, Vec<u8>)>,
    folder: Option<&Path>,
) -> Result<(), FileError> {
    // (id in the document, its contents, id in the file).
    let mut written: Vec<(u64, &Contents, u64)> = Vec::new();
    for (i, layer) in layers.iter().enumerate() {
        let Some(s) = layer.smart.as_deref() else {
            continue;
        };
        let known = written
            .iter()
            .find(|(id, c, _)| *id == s.contents.id && c.same_as(&s.contents));
        let id = match known {
            Some(&(_, _, id)) => id,
            None => {
                // Smart objects that differ under one id (documents of
                // older versions): each kept, under an id of its own.
                let id = if written.iter().any(|(id, ..)| *id == s.contents.id) {
                    super::super::next_rev()
                } else {
                    s.contents.id
                };
                let mut data = Cursor::new(Vec::new());
                write(&Document::from_contents(&s.contents), &mut data, folder)?;
                files.push((format!("smart/{id}.atelier"), data.into_inner()));
                written.push((s.contents.id, &s.contents, id));
                id
            }
        };
        let contents = format!("smart/{id}.atelier");
        let filter_mask = match &s.filter_mask {
            Some(m) => {
                let mut entry = mask_entry(m, layer.offset);
                let image = m.image();
                if image.width() > 0 && image.height() > 0 {
                    let name = format!("smart/masks/{i}.png");
                    files.push((name.clone(), encode_gray(image)?));
                    entry.image = Some(name);
                }
                Some(entry)
            }
            None => None,
        };
        entries[i].smart = Some(SmartEntry {
            contents,
            id,
            name: s.contents.name.clone(),
            link: s.link.as_ref().map(|l| LinkEntry {
                path: l.path.clone(),
                relative: folder.and_then(|f| relative(&l.path, f)),
                modified: l
                    .modified
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| (d.as_secs(), d.subsec_nanos())),
            }),
            transform: match s.map {
                Map::Projective(h) => MapEntry::Projective { matrix: h.0 },
                Map::Warp(w) => MapEntry::Warp { points: w.0 },
            },
            interpolation: match s.interpolation {
                xform::Filter::Nearest => InterpolationEntry::Nearest,
                xform::Filter::Bilinear => InterpolationEntry::Bilinear,
                xform::Filter::Bicubic => InterpolationEntry::Bicubic,
                xform::Filter::Lanczos => InterpolationEntry::Lanczos,
            },
            filters: s
                .filters
                .iter()
                .enumerate()
                .map(|(k, f)| filter_entry(f, &format!("smart/meshes/{i}-{k}.msh"), files))
                .collect(),
            filters_enabled: s.filters_enabled,
            filter_mask,
        });
    }
    Ok(())
}

/// `path` relative to `folder` ("../assets/logo.png" for a file beside
/// it), when both are absolute on the same drive or share.
fn relative(path: &Path, folder: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if !path.is_absolute() || !folder.is_absolute() {
        return None;
    }
    let (mut p, mut f) = (path.components().peekable(), folder.components().peekable());
    while let (Some(a), Some(b)) = (p.peek(), f.peek()) {
        if a != b {
            break;
        }
        p.next();
        f.next();
    }
    // Not even the same root (a Windows drive or share).
    if matches!(p.peek(), Some(Component::Prefix(_) | Component::RootDir)) {
        return None;
    }
    let mut out: PathBuf = f.map(|_| Component::ParentDir).collect();
    out.extend(p);
    Some(out)
}

/// A path on another machine: UNC (`\\host\share`, also `//host/share`
/// on Windows), as written on any system.
pub(in crate::document) fn remote(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let local = ["\\\\?\\", "\\\\.\\"];
    if text.starts_with("\\\\") && !local.iter().any(|l| text.starts_with(l)) {
        return true;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(p)) = path.components().next() {
            return matches!(
                p.kind(),
                Prefix::UNC(..) | Prefix::VerbatimUNC(..) | Prefix::DeviceNS(..)
            );
        }
    }
    false
}

/// The entry of `f`; a Liquify mesh goes to `files` as `mesh`.
fn filter_entry(f: &SmartFilter, mesh: &str, files: &mut Vec<(String, Vec<u8>)>) -> FilterEntry {
    let (filter, liquify, camera_raw) = match &f.filter {
        SmartFilterKind::Filter(filter) => (Some(*filter), None, None),
        SmartFilterKind::Liquify(m) => {
            files.push((mesh.to_string(), m.to_bytes()));
            (None, Some(mesh.to_string()), None)
        }
        SmartFilterKind::CameraRaw(s) => (None, None, Some((**s).clone())),
    };
    FilterEntry {
        filter,
        liquify,
        camera_raw,
        visible: f.visible,
        blend: blend_entry(f.blend),
        opacity: f.opacity,
    }
}

/// The smart object of entry `e`, for a layer at `layer_offset`; the
/// contents read once per id (`read`), `folder` the document's.
pub(super) fn smart_of(
    e: SmartEntry,
    zip: &mut ZipArchive<impl Read + Seek>,
    read: &mut HashMap<u64, Contents>,
    layer_offset: (i64, i64),
    folder: Option<&Path>,
    claims: &mut Claims,
) -> Result<SmartObject, FileError> {
    let contents = match read.get(&e.id) {
        Some(c) => c.clone(),
        None => {
            let c = (claims.document(zip, &e.contents, MAX_CONTENTS)?)
                .to_contents(e.id, e.name.clone());
            read.insert(e.id, c.clone());
            c
        }
    };
    let map = match e.transform {
        MapEntry::Projective { matrix } if matrix.iter().all(|v| v.is_finite()) => {
            Map::Projective(Homography(matrix))
        }
        MapEntry::Warp { points } if points.iter().flatten().flatten().all(|v| v.is_finite()) => {
            Map::Warp(Warp(points))
        }
        _ => return Err(FileError::Damaged),
    };
    let mut s = SmartObject::new(contents, map);
    s.interpolation = match e.interpolation {
        InterpolationEntry::Nearest => xform::Filter::Nearest,
        InterpolationEntry::Bilinear => xform::Filter::Bilinear,
        InterpolationEntry::Bicubic => xform::Filter::Bicubic,
        InterpolationEntry::Lanczos => xform::Filter::Lanczos,
    };
    s.link = e.link.map(|l| {
        // A network path a received file names is not visited on opening
        // (Windows would connect to that host): missing until updated.
        let exists = |p: &Path| !remote(p) && p.exists();
        let path = match (&l.relative, folder) {
            (Some(r), Some(f)) if !exists(&l.path) && exists(&f.join(r)) => f.join(r),
            _ => l.path,
        };
        Link {
            missing: !exists(&path),
            modified: l
                .modified
                .and_then(|(s, n)| UNIX_EPOCH.checked_add(Duration::new(s, n.min(999_999_999)))),
            path,
        }
    });
    for f in e.filters {
        let filter = match (f.filter, &f.liquify, f.camera_raw) {
            (Some(filter), ..) => SmartFilterKind::Filter(filter),
            (None, Some(name), _) => {
                let data = read_entry(zip, name, MAX_CONTENTS.min(claims.budget.left()))?;
                claims.budget.take_bytes(data.len() as u64)?;
                let mesh = Mesh::from_bytes(&data);
                SmartFilterKind::Liquify(Arc::new(mesh.ok_or(FileError::Damaged)?))
            }
            (None, None, Some(settings)) => SmartFilterKind::CameraRaw(Arc::new(settings)),
            (None, None, None) => return Err(FileError::Damaged),
        };
        s.filters.push(SmartFilter {
            filter,
            visible: f.visible,
            blend: blend_mode(f.blend),
            opacity: finite_or(f.opacity, 1.0).clamp(0.0, 1.0),
        });
    }
    s.filters_enabled = e.filters_enabled;
    if let Some(m) = e.filter_mask {
        let image = match &m.image {
            Some(name) => {
                let size = (m.width, m.height);
                decode_gray(&claims.pixels(zip, name, size, 1)?, size)?
            }
            None => GrayImage::new(0, 0),
        };
        s.filter_mask = Some(mask_of(&m, image, layer_offset));
    }
    Ok(s)
}

/// Read limit of an embedded document.
const MAX_CONTENTS: u64 = 16 << 30;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{PixelRect, Selection};
    use crate::filters::Filter;

    #[cfg(unix)]
    #[test]
    fn linked_paths_relative_to_the_folder() {
        let r = |p: &str, f: &str| relative(Path::new(p), Path::new(f));
        let some = |p: &str| Some(PathBuf::from(p));
        assert_eq!(r("/w/project/logo.png", "/w/project"), some("logo.png"));
        assert_eq!(r("/w/project/a/logo.png", "/w/project"), some("a/logo.png"));
        let sibling = r("/w/assets/logo.png", "/w/project/doc");
        assert_eq!(sibling, some("../../assets/logo.png"));
        assert_eq!(r("logo.png", "/w/project"), None);
    }
    use image::{Rgba, RgbaImage};

    #[test]
    fn smart_objects_round_trip() {
        let dir = std::env::temp_dir().join("atelier-native-smart");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(120, 90, Rgba([255; 4])), None);
        let pattern =
            RgbaImage::from_fn(40, 30, |x, y| Rgba([(x * 6) as u8, (y * 8) as u8, 90, 255]));
        doc.add_layer("pattern".into(), pattern, (10, 10));
        assert!(doc.convert_to_smart(&[1]));
        let blur = SmartFilter::new(Filter::GaussianBlur { radius: 2.0 });
        let edges = SmartFilter {
            opacity: 0.5,
            blend: crate::document::BlendMode::Multiply,
            visible: false,
            ..SmartFilter::new(Filter::FindEdges)
        };
        let mut editor = crate::liquify::Liquify::new(Mesh::new(40, 30));
        let brush = crate::liquify::Brush {
            size: 20.0,
            density: 0.5,
            pressure: 1.0,
            rate: 0.5,
            pin_edges: false,
        };
        let warp = crate::liquify::Effect::Warp;
        editor.stroke(warp, &brush, [10.0, 15.0], [25.0, 15.0], 0.1, 1.0);
        let liquify = SmartFilter::new(SmartFilterKind::Liquify(Arc::new(editor.mesh)));
        doc.edit_smart(1, "Filters".into(), |s| {
            s.filters = vec![blur, edges, liquify];
            s.map = Map::Warp(
                Warp::from_map(&Homography::affine([40.0, 0.0, 0.0, 3.0, 30.0, 0.0])).unwrap(),
            );
        });
        let sel: Selection = PixelRect {
            x: 10,
            y: 10,
            w: 20,
            h: 30,
        }
        .into();
        assert!(doc.add_filter_mask(1, Some(&sel)));
        doc.duplicate_active();
        let linked = dir.join("linked.png");
        RgbaImage::from_pixel(8, 8, Rgba([0, 255, 0, 255]))
            .save(&linked)
            .unwrap();
        doc.place_file(&linked, true).unwrap();

        let path = dir.join("smart.atelier");
        doc.save(&path).unwrap();
        let zip = ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let nested = zip.file_names().filter(|n| n.ends_with(".atelier")).count();
        assert_eq!(nested, 2, "the instances share their contents");
        let back = Document::open(&path).unwrap().doc;
        assert_eq!(back.flatten(), doc.flatten());
        for i in 1..4 {
            let (a, b) = (
                doc.layers[i].smart.as_deref().unwrap(),
                back.layers[i].smart.as_deref().unwrap(),
            );
            assert_eq!(a.map, b.map);
            assert_eq!(a.filters, b.filters);
            assert_eq!(a.filters_enabled, b.filters_enabled);
            assert_eq!(a.contents.id, b.contents.id);
            assert_eq!(a.contents.name, b.contents.name);
            let flat = |c: &Contents| Document::from_contents(c).flatten();
            assert_eq!(flat(&a.contents), flat(&b.contents));
            assert_eq!(a.link, b.link);
            let mask = |s: &SmartObject| {
                s.filter_mask
                    .as_ref()
                    .map(|m| (m.image().clone(), m.origin((0, 0)), m.default))
            };
            assert_eq!(mask(a), mask(b));
        }
        // Rendered again, it looks the same.
        let mut again = back;
        let canvas = again.full_rect();
        again.layers[1].render_smart(canvas);
        assert_eq!(again.layers[1].image(), doc.layers[1].image());
        let inner = again.smart_contents(1).unwrap();
        assert_eq!(inner.layers.len(), 1);
        assert_eq!(inner.layers[0].name, "pattern");
    }

    #[test]
    fn the_first_save_records_links_relative_to_its_folder() {
        let base = std::env::temp_dir().join("atelier-link-first-save");
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let linked = project.join("photo.png");
        RgbaImage::from_pixel(8, 8, Rgba([0, 255, 0, 255]))
            .save(&linked)
            .unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255; 4])), None);
        doc.place_file(&linked, true).unwrap();
        doc.save(&project.join("doc.atelier")).unwrap();
        // The whole folder moves: the link is found there.
        let moved = base.join("moved");
        std::fs::rename(&project, &moved).unwrap();
        let back = Document::open(&moved.join("doc.atelier")).unwrap().doc;
        let link = back
            .layers
            .iter()
            .find_map(|l| l.smart.as_ref().and_then(|s| s.link.as_ref()))
            .unwrap();
        assert!(!link.missing);
        assert_eq!(link.path, moved.join("photo.png"));
    }

    #[test]
    fn network_paths_are_not_visited_on_opening() {
        assert!(remote(Path::new(r"\\host\share\x.png")));
        assert!(!remote(Path::new(r"\\?\C:\img\x.png")));
        assert!(!remote(Path::new("/home/me/x.png")));
    }
}
