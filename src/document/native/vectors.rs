//! Paths and shape layers in the manifest: anchors with their handles,
//! components and their operations, fill and stroke, live shape settings.

use serde::{Deserialize, Serialize};

use super::{GradientFillEntry, finite_or, gradient_fill_entry, gradient_fill_of};
use crate::document::SavedPath;
use crate::vector::{
    Align, Anchor, Bounds, Cap, Custom, Join, Live, Paint, Path, PathOp, Shape, ShapeKind,
    ShapeStroke, SubPath, pt,
};

/// A path: its components from the bottom up.
#[derive(Serialize, Deserialize)]
pub(super) struct PathEntry {
    subpaths: Vec<SubPathEntry>,
}

#[derive(Serialize, Deserialize)]
struct SubPathEntry {
    closed: bool,
    #[serde(default)]
    op: OpEntry,
    anchors: Vec<AnchorEntry>,
}

/// An anchor point and its handles (in, out), canvas or layer coordinates.
#[derive(Serialize, Deserialize)]
struct AnchorEntry {
    point: [f32; 2],
    before: [f32; 2],
    after: [f32; 2],
    #[serde(default)]
    smooth: bool,
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum OpEntry {
    #[default]
    Combine,
    Subtract,
    Intersect,
    Exclude,
}

/// A path of the Paths panel.
#[derive(Serialize, Deserialize)]
pub(super) struct SavedPathEntry {
    name: String,
    path: PathEntry,
}

/// A shape layer's content, in the layer's coordinates (its pixels are
/// the rendering of it).
#[derive(Serialize, Deserialize)]
pub(super) struct ShapeEntry {
    path: PathEntry,
    fill: PaintEntry,
    stroke: StrokeEntry,
    #[serde(default)]
    live: Option<LiveEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum PaintEntry {
    None,
    Color { color: [u8; 3] },
    Gradient(GradientFillEntry),
}

#[derive(Serialize, Deserialize)]
struct StrokeEntry {
    paint: PaintEntry,
    width: f32,
    align: AlignEntry,
    cap: CapEntry,
    join: JoinEntry,
    /// Dash and gap, in stroke widths.
    #[serde(default)]
    dash: Option<[f32; 2]>,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum AlignEntry {
    Inside,
    Center,
    Outside,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum CapEntry {
    Butt,
    Round,
    Square,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum JoinEntry {
    Miter,
    Round,
    Bevel,
}

/// The settings of a live shape; `rect`: x0, y0, x1, y1 (a line: from,
/// to).
#[derive(Serialize, Deserialize)]
struct LiveEntry {
    kind: LiveKindEntry,
    rect: [f32; 4],
    radii: [f32; 4],
    sides: u32,
    star: f32,
    weight: f32,
    arrows: [bool; 2],
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum LiveKindEntry {
    Rectangle,
    Ellipse,
    Triangle,
    Polygon,
    Line,
    Arrow,
    Heart,
    Star,
    Check,
    Bubble,
    Bolt,
}

const OPS: [(PathOp, OpEntry); 4] = [
    (PathOp::Combine, OpEntry::Combine),
    (PathOp::Subtract, OpEntry::Subtract),
    (PathOp::Intersect, OpEntry::Intersect),
    (PathOp::Exclude, OpEntry::Exclude),
];

const KINDS: [(ShapeKind, LiveKindEntry); 11] = [
    (ShapeKind::Rectangle, LiveKindEntry::Rectangle),
    (ShapeKind::Ellipse, LiveKindEntry::Ellipse),
    (ShapeKind::Triangle, LiveKindEntry::Triangle),
    (ShapeKind::Polygon, LiveKindEntry::Polygon),
    (ShapeKind::Line, LiveKindEntry::Line),
    (ShapeKind::Custom(Custom::Arrow), LiveKindEntry::Arrow),
    (ShapeKind::Custom(Custom::Heart), LiveKindEntry::Heart),
    (ShapeKind::Custom(Custom::Star), LiveKindEntry::Star),
    (ShapeKind::Custom(Custom::Check), LiveKindEntry::Check),
    (ShapeKind::Custom(Custom::Bubble), LiveKindEntry::Bubble),
    (ShapeKind::Custom(Custom::Bolt), LiveKindEntry::Bolt),
];

pub(super) fn path_entry(p: &Path) -> PathEntry {
    let xy = |p: crate::vector::Pt| [p.x, p.y];
    PathEntry {
        subpaths: p
            .subpaths
            .iter()
            .map(|s| SubPathEntry {
                closed: s.closed,
                op: OPS
                    .iter()
                    .find(|(o, _)| *o == s.op)
                    .map_or(OpEntry::Combine, |(_, e)| *e),
                anchors: s
                    .anchors
                    .iter()
                    .map(|a| AnchorEntry {
                        point: xy(a.p),
                        before: xy(a.before),
                        after: xy(a.after),
                        smooth: a.smooth,
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub(super) fn path_of(e: &PathEntry) -> Path {
    let p = |[x, y]: [f32; 2]| pt(finite_or(x, 0.0), finite_or(y, 0.0));
    Path {
        subpaths: e
            .subpaths
            .iter()
            .filter(|s| !s.anchors.is_empty())
            .map(|s| SubPath {
                anchors: s
                    .anchors
                    .iter()
                    .map(|a| Anchor {
                        p: p(a.point),
                        before: p(a.before),
                        after: p(a.after),
                        smooth: a.smooth,
                    })
                    .collect(),
                closed: s.closed && s.anchors.len() > 1,
                op: OPS
                    .iter()
                    .find(|(_, o)| *o == s.op)
                    .map_or(PathOp::Combine, |(o, _)| *o),
            })
            .collect(),
    }
}

pub(super) fn saved_path_entry(p: &SavedPath) -> SavedPathEntry {
    SavedPathEntry {
        name: p.name.clone(),
        path: path_entry(&p.path),
    }
}

pub(super) fn saved_path_of(e: &SavedPathEntry) -> SavedPath {
    SavedPath {
        name: e.name.clone(),
        path: path_of(&e.path),
    }
}

fn paint_entry(p: &Paint) -> PaintEntry {
    match p {
        Paint::None => PaintEntry::None,
        Paint::Color(color) => PaintEntry::Color { color: *color },
        Paint::Gradient(g) => PaintEntry::Gradient(gradient_fill_entry(g)),
    }
}

fn paint_of(e: &PaintEntry) -> Paint {
    match e {
        PaintEntry::None => Paint::None,
        PaintEntry::Color { color } => Paint::Color(*color),
        PaintEntry::Gradient(g) => Paint::Gradient(gradient_fill_of(g)),
    }
}

pub(super) fn shape_entry(s: &Shape) -> ShapeEntry {
    let k = &s.stroke;
    ShapeEntry {
        path: path_entry(&s.path),
        fill: paint_entry(&s.fill),
        stroke: StrokeEntry {
            paint: paint_entry(&k.paint),
            width: k.width,
            align: match k.align {
                Align::Inside => AlignEntry::Inside,
                Align::Center => AlignEntry::Center,
                Align::Outside => AlignEntry::Outside,
            },
            cap: match k.cap {
                Cap::Butt => CapEntry::Butt,
                Cap::Round => CapEntry::Round,
                Cap::Square => CapEntry::Square,
            },
            join: match k.join {
                Join::Miter => JoinEntry::Miter,
                Join::Round => JoinEntry::Round,
                Join::Bevel => JoinEntry::Bevel,
            },
            dash: k.dash,
        },
        live: s.live.map(|l| LiveEntry {
            kind: KINDS
                .iter()
                .find(|(k, _)| *k == l.kind)
                .map_or(LiveKindEntry::Rectangle, |(_, e)| *e),
            rect: [l.rect.min.x, l.rect.min.y, l.rect.max.x, l.rect.max.y],
            radii: l.radii,
            sides: l.sides,
            star: l.star,
            weight: l.weight,
            arrows: l.arrows,
        }),
    }
}

pub(super) fn shape_of(e: &ShapeEntry) -> Shape {
    let k = &e.stroke;
    let live = e.live.as_ref().map(|l| {
        let [x0, y0, x1, y1] = l.rect.map(|v| finite_or(v, 0.0));
        let kind = KINDS
            .iter()
            .find(|(_, k)| *k == l.kind)
            .map_or(ShapeKind::Rectangle, |(k, _)| *k);
        Live {
            radii: l.radii.map(|r| finite_or(r, 0.0).max(0.0)),
            sides: l.sides.clamp(3, 100),
            star: finite_or(l.star, 1.0).clamp(0.01, 1.0),
            weight: finite_or(l.weight, 1.0).max(0.0),
            arrows: l.arrows,
            ..Live::new(
                kind,
                Bounds {
                    min: pt(x0, y0),
                    max: pt(x1, y1),
                },
            )
        }
    });
    Shape {
        path: path_of(&e.path),
        fill: paint_of(&e.fill),
        stroke: ShapeStroke {
            paint: paint_of(&k.paint),
            width: finite_or(k.width, 1.0).clamp(0.0, 1000.0),
            align: match k.align {
                AlignEntry::Inside => Align::Inside,
                AlignEntry::Center => Align::Center,
                AlignEntry::Outside => Align::Outside,
            },
            cap: match k.cap {
                CapEntry::Butt => Cap::Butt,
                CapEntry::Round => Cap::Round,
                CapEntry::Square => Cap::Square,
            },
            join: match k.join {
                JoinEntry::Miter => Join::Miter,
                JoinEntry::Round => Join::Round,
                JoinEntry::Bevel => Join::Bevel,
            },
            dash: k
                .dash
                .filter(|d| d.iter().all(|v| v.is_finite() && *v >= 0.0)),
        },
        live,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::gradient::GradientFill;
    use crate::document::{Document, PathRef};
    use image::{Rgba, RgbaImage};

    #[test]
    fn paths_and_shapes_survive_a_round_trip() {
        let dir = std::env::temp_dir().join("atelier-native-vectors");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("doc.atelier");

        let mut doc = Document::new(RgbaImage::from_pixel(60, 40, Rgba([255; 4])), None);
        let mut live = Live::new(ShapeKind::Polygon, Bounds::of(pt(5.0, 5.0), pt(35.0, 30.0)));
        (live.sides, live.star, live.radii) = (6, 0.5, [2.0; 4]);
        let stroke = ShapeStroke {
            paint: Paint::Gradient(GradientFill::default()),
            width: 3.0,
            align: Align::Outside,
            cap: Cap::Round,
            join: Join::Bevel,
            dash: Some([2.0, 1.0]),
        };
        doc.add_shape_layer(
            "Polygon".into(),
            "Polygon",
            Shape::live(live, Paint::Color([10, 20, 30]), stroke),
        );
        // A pen path with a curve and a subtracted component, kept as is.
        let mut path = Path::default();
        path.subpaths.push(SubPath::new(
            vec![
                Anchor::smooth(pt(1.0, 2.0), pt(5.0, 2.5)),
                Anchor::corner(pt(20.0, 20.0)),
            ],
            false,
        ));
        let mut hole = SubPath::polygon([pt(3.0, 3.0), pt(9.0, 3.0), pt(6.0, 8.0)]);
        hole.op = PathOp::Subtract;
        path.subpaths.push(hole);
        doc.work_path = Some(path.clone());
        doc.paths.push(SavedPath {
            name: "Outline".into(),
            path: path.clone(),
        });
        doc.save(&file).unwrap();

        let back = Document::open(&file).unwrap().doc;
        assert_eq!(back.work_path, Some(path.clone()));
        assert_eq!(back.paths, doc.paths);
        let (a, b) = (doc.active_layer(), back.active_layer());
        assert_eq!(b.shape, a.shape);
        assert_eq!(b.offset, a.offset);
        assert!(b.image() == a.image());
        let id = b.id;
        assert_eq!(
            back.path(PathRef::Layer(id)),
            doc.path(PathRef::Layer(a.id))
        );
    }
}
