//! Shape layers: a path filled and stroked (color or gradient; stroke
//! width, alignment, caps, joins, dashes), and the live shapes the shape
//! tools draw (rectangle with its corner radii, ellipse, triangle,
//! polygon or star, line with arrowheads, custom shapes), editable
//! afterwards.

use image::RgbaImage;

use super::geom::{Anchor, Bounds, Path, Pt, SubPath, pt};
use super::raster::{self, Cap, Join, StrokeStyle};
use crate::compose::gradient::{GradientFill, Ramp};
use crate::document::PixelRect;
use crate::mask_ops::{Affine, par_rows};

/// Handle length of a quarter circle, per unit of radius.
const KAPPA: f32 = 0.552_284_8;

/// What fills a shape or its stroke.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    None,
    Color([u8; 3]),
    Gradient(GradientFill),
}

impl Paint {
    pub fn is_none(&self) -> bool {
        matches!(self, Paint::None)
    }

    /// Ready to draw.
    fn source(&self) -> Source {
        match self {
            Paint::None => Source::Color([0.0; 4]),
            Paint::Color(c) => {
                let [r, g, b] = c.map(|v| v as f32 / 255.0);
                Source::Color([r, g, b, 1.0])
            }
            Paint::Gradient(g) => Source::Ramp(Ramp::new(g)),
        }
    }
}

/// A paint ready to draw.
enum Source {
    Color([f32; 4]),
    Ramp(Ramp),
}

impl Source {
    /// Color (non-premultiplied, 0..1) at (x, y), a gradient spanning
    /// `bounds` ([x, y, w, h]).
    fn at(&self, x: f32, y: f32, bounds: [f32; 4]) -> [f32; 4] {
        match self {
            Source::Color(c) => *c,
            Source::Ramp(r) => r.at(x, y, bounds),
        }
    }
}

/// Where the stroke lies on the outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Inside,
    Center,
    Outside,
}

/// The stroke of a shape.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeStroke {
    pub paint: Paint,
    pub width: f32,
    pub align: Align,
    pub cap: Cap,
    pub join: Join,
    /// Dash and gap, in stroke widths; `None`: solid.
    pub dash: Option<[f32; 2]>,
}

impl Default for ShapeStroke {
    /// Photoshop's: none drawn, 1 px inside, butt caps, miter joins.
    fn default() -> Self {
        Self {
            paint: Paint::None,
            width: 1.0,
            align: Align::Inside,
            cap: Cap::Butt,
            join: Join::Miter,
            dash: None,
        }
    }
}

impl ShapeStroke {
    pub fn drawn(&self) -> bool {
        !self.paint.is_none() && self.width > 0.0
    }

    /// The stroke as drawn on the outline: twice as wide when only one side
    /// of it shows.
    fn style(&self) -> StrokeStyle {
        let k = if self.align == Align::Center {
            1.0
        } else {
            2.0
        };
        StrokeStyle {
            width: self.width * k,
            cap: self.cap,
            join: self.join,
            dash: self.dash.map(|d| d.map(|v| v * self.width)),
        }
    }
}

/// The tool a live shape comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    Ellipse,
    Triangle,
    Polygon,
    Line,
    Custom(Custom),
}

/// The built-in custom shapes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Custom {
    #[default]
    Arrow,
    Heart,
    Star,
    Check,
    Bubble,
    Bolt,
}

impl Custom {
    pub const ALL: [Custom; 6] = [
        Custom::Arrow,
        Custom::Heart,
        Custom::Star,
        Custom::Check,
        Custom::Bubble,
        Custom::Bolt,
    ];

    /// The shape in a unit box (0..1).
    fn path(self) -> SubPath {
        let corner = |x: f32, y: f32| Anchor::corner(pt(x, y));
        // A point and its two handles.
        let curve = |[x, y, bx, by, ax, ay]: [f32; 6]| Anchor {
            p: pt(x, y),
            before: pt(bx, by),
            after: pt(ax, ay),
            smooth: false,
        };
        let anchors = match self {
            Custom::Arrow => vec![
                corner(0.0, 0.35),
                corner(0.55, 0.35),
                corner(0.55, 0.05),
                corner(1.0, 0.5),
                corner(0.55, 0.95),
                corner(0.55, 0.65),
                corner(0.0, 0.65),
            ],
            Custom::Heart => [
                [0.5, 0.25, 0.38, -0.02, 0.62, -0.02],
                [1.0, 0.32, 1.0, 0.02, 1.0, 0.62],
                [0.5, 1.0, 0.6, 0.8, 0.4, 0.8],
                [0.0, 0.32, 0.0, 0.62, 0.0, 0.02],
            ]
            .map(curve)
            .to_vec(),
            Custom::Star => star_points(pt(0.5, 0.5), 0.5, 5, 0.45, true)
                .into_iter()
                .map(Anchor::corner)
                .collect(),
            Custom::Check => vec![
                corner(0.0, 0.55),
                corner(0.15, 0.4),
                corner(0.38, 0.62),
                corner(0.85, 0.1),
                corner(1.0, 0.25),
                corner(0.38, 0.9),
            ],
            // A rounded box and its tail.
            Custom::Bubble => {
                let k = 0.15 * (1.0 - KAPPA);
                let mut a = [
                    [0.15, 0.0, k, 0.0, 0.15, 0.0],
                    [0.85, 0.0, 0.85, 0.0, 1.0 - k, 0.0],
                    [1.0, 0.15, 1.0, k, 1.0, 0.15],
                    [1.0, 0.6, 1.0, 0.6, 1.0, 0.75 - k],
                    [0.85, 0.75, 1.0 - k, 0.75, 0.85, 0.75],
                ]
                .map(curve)
                .to_vec();
                a.extend([corner(0.45, 0.75), corner(0.2, 1.0), corner(0.28, 0.75)]);
                a.extend(
                    [
                        [0.15, 0.75, 0.15, 0.75, k, 0.75],
                        [0.0, 0.6, 0.0, 0.75 - k, 0.0, 0.6],
                        [0.0, 0.15, 0.0, 0.15, 0.0, k],
                    ]
                    .map(curve),
                );
                a
            }
            Custom::Bolt => vec![
                corner(0.6, 0.0),
                corner(0.15, 0.55),
                corner(0.45, 0.55),
                corner(0.35, 1.0),
                corner(0.85, 0.4),
                corner(0.55, 0.4),
                corner(0.7, 0.0),
            ],
        };
        SubPath::new(anchors, true)
    }
}

/// Settings of a live shape, kept with it: the Properties panel edits them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Live {
    pub kind: ShapeKind,
    /// The box it was drawn in (layer coordinates); for a line, from its
    /// start (`min`) to its end (`max`), in any direction.
    pub rect: Bounds,
    /// Corner radii: top left, top right, bottom right, bottom left (a
    /// triangle and a polygon use the first).
    pub radii: [f32; 4],
    /// Polygon: number of sides (points of a star).
    pub sides: u32,
    /// Polygon: inner radius over outer one, 1 for no star.
    pub star: f32,
    /// Line: its weight; arrowheads at its start and end.
    pub weight: f32,
    pub arrows: [bool; 2],
}

impl Live {
    pub fn new(kind: ShapeKind, rect: Bounds) -> Live {
        Live {
            kind,
            rect,
            radii: [0.0; 4],
            sides: 5,
            star: 1.0,
            weight: 1.0,
            arrows: [false; 2],
        }
    }

    /// Its outline.
    pub fn path(&self) -> Path {
        let r = self.rect;
        let b = Bounds::of(r.min, r.max);
        let sub = match self.kind {
            ShapeKind::Rectangle => rounded_rect(b, self.radii),
            ShapeKind::Ellipse => ellipse(b),
            ShapeKind::Triangle => rounded_polygon(
                &[
                    pt(b.center().x, b.min.y),
                    pt(b.max.x, b.max.y),
                    pt(b.min.x, b.max.y),
                ],
                self.radii[0],
            ),
            ShapeKind::Polygon => {
                let radius = 0.5;
                let unit = star_points(pt(0.5, 0.5), radius, self.sides, self.star, false);
                let points: Vec<Pt> = unit
                    .into_iter()
                    .map(|p| pt(b.min.x + p.x * b.width(), b.min.y + p.y * b.height()))
                    .collect();
                rounded_polygon(&points, self.radii[0])
            }
            ShapeKind::Line => line(r.min, r.max, self.weight, self.arrows),
            ShapeKind::Custom(c) => {
                let mut s = c.path();
                s.map(&|p| pt(b.min.x + p.x * b.width(), b.min.y + p.y * b.height()));
                s
            }
        };
        Path {
            subpaths: vec![sub],
        }
    }

    /// Moved and scaled by `a` if it keeps the axes (else `None`: the
    /// shape becomes a plain path).
    fn mapped(&self, a: &Affine) -> Option<Live> {
        let [sx, kx, _, ky, sy, _] = a.0;
        if kx.abs() > 1e-6 || ky.abs() > 1e-6 {
            return None;
        }
        // A flipped triangle or polygon would no longer be one.
        let flips = sx < 0.0 || sy < 0.0;
        if flips && matches!(self.kind, ShapeKind::Triangle | ShapeKind::Polygon) {
            return None;
        }
        let (p, q) = (self.rect.min.map(a), self.rect.max.map(a));
        let rect = if self.kind == ShapeKind::Line {
            Bounds { min: p, max: q }
        } else {
            Bounds::of(p, q)
        };
        let k = (sx * sy).abs().sqrt();
        Some(Live {
            rect,
            weight: self.weight * k,
            ..*self
        })
    }
}

/// A shape layer's content (in the layer's coordinates).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub path: Path,
    pub fill: Paint,
    pub stroke: ShapeStroke,
    /// The live shape the path comes from, while it is one.
    pub live: Option<Live>,
}

impl Shape {
    /// A live shape, its path made from it.
    pub fn live(live: Live, fill: Paint, stroke: ShapeStroke) -> Shape {
        Shape {
            path: live.path(),
            fill,
            stroke,
            live: Some(live),
        }
    }

    /// The path again from the live shape's settings.
    pub fn rebuild(&mut self) {
        if let Some(l) = &self.live {
            self.path = l.path();
        }
    }

    pub fn translate(&mut self, d: Pt) {
        self.path.translate(d);
        if let Some(l) = &mut self.live {
            l.rect = Bounds {
                min: l.rect.min + d,
                max: l.rect.max + d,
            };
        }
    }

    /// Through an affine map: a live shape stays one if the map keeps the
    /// axes; the stroke scales with it.
    pub fn transform(&mut self, a: &Affine) {
        self.path.transform(a);
        self.live = self.live.and_then(|l| l.mapped(a));
        let [sx, kx, _, ky, sy, _] = a.0;
        self.stroke.width *= (sx * sy - kx * ky).abs().sqrt();
    }

    /// Through a map that isn't affine (perspective): its points are
    /// mapped, it is no longer a live shape. `None` if `f` fails anywhere.
    pub fn map_points(&self, f: impl Fn(Pt) -> Option<Pt>) -> Option<Shape> {
        let mut path = self.path.clone();
        for s in &mut path.subpaths {
            for a in &mut s.anchors {
                (a.p, a.before, a.after) = (f(a.p)?, f(a.before)?, f(a.after)?);
            }
        }
        Some(Shape {
            path,
            live: None,
            ..self.clone()
        })
    }

    /// Box of what it shows: the fill, and the stroke on its side.
    pub fn extent(&self) -> Option<Bounds> {
        let fill = self.path.bounds()?;
        if !self.stroke.drawn() || self.stroke.align == Align::Inside {
            return Some(fill.expand(1.0));
        }
        let stroke = raster::stroke_bounds(&self.path, &self.stroke.style());
        Some(stroke.map_or(fill, |s| fill.union(s)).expand(1.0))
    }

    /// Its pixels over `window` (in its coordinates): the fill, the stroke
    /// over it, anti-aliased.
    pub fn render(&self, window: PixelRect) -> RgbaImage {
        let mut out = RgbaImage::new(window.w, window.h);
        let Some(b) = self.path.bounds() else {
            return out;
        };
        let stroke = self.stroke.drawn();
        let need_fill = !self.fill.is_none() || (stroke && self.stroke.align != Align::Center);
        let fill = need_fill.then(|| raster::fill_coverage(&self.path, window));
        let lines =
            stroke.then(|| raster::stroke_coverage(&self.path, &self.stroke.style(), window));
        let bounds = [b.min.x, b.min.y, b.width(), b.height()];
        let (fill_paint, stroke_paint) = (self.fill.source(), self.stroke.paint.source());
        let w = window.w as usize;
        par_rows(out.as_mut(), w * 4, |y0, band| {
            for (i, px) in band.chunks_exact_mut(4).enumerate() {
                let k = y0 * w + i;
                let (x, y) = ((k % w) as i64 + window.x, (k / w) as i64 + window.y);
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let f = fill.as_ref().map_or(0.0, |m| m.as_raw()[k] as f32 / 255.0);
                let mut s = lines.as_ref().map_or(0.0, |m| m.as_raw()[k] as f32 / 255.0);
                s *= match self.stroke.align {
                    Align::Center => 1.0,
                    Align::Inside => f,
                    Align::Outside => 1.0 - f,
                };
                let mut c = [0.0f32; 4];
                if f > 0.0 && !self.fill.is_none() {
                    c = fill_paint.at(cx, cy, bounds);
                    c[3] *= f;
                }
                if s > 0.0 {
                    let mut t = stroke_paint.at(cx, cy, bounds);
                    t[3] *= s;
                    let a = t[3] + c[3] * (1.0 - t[3]);
                    if a > 0.0 {
                        for i in 0..3 {
                            c[i] = (t[i] * t[3] + c[i] * c[3] * (1.0 - t[3])) / a;
                        }
                    }
                    c[3] = a;
                }
                if c[3] > 0.0 {
                    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    px.copy_from_slice(&[q(c[0]), q(c[1]), q(c[2]), q(c[3])]);
                }
            }
        });
        out
    }
}

/// A rectangle, its corners rounded by `radii` (top left first, clockwise).
pub fn rounded_rect(b: Bounds, radii: [f32; 4]) -> SubPath {
    let (w, h) = (b.width(), b.height());
    let max = w.min(h) / 2.0;
    let [tl, tr, br, bl] = radii.map(|r| r.clamp(0.0, max));
    let corners = [
        (pt(b.min.x, b.min.y), tl, pt(0.0, 1.0), pt(1.0, 0.0)),
        (pt(b.max.x, b.min.y), tr, pt(-1.0, 0.0), pt(0.0, 1.0)),
        (pt(b.max.x, b.max.y), br, pt(0.0, -1.0), pt(-1.0, 0.0)),
        (pt(b.min.x, b.max.y), bl, pt(1.0, 0.0), pt(0.0, -1.0)),
    ];
    let mut anchors = Vec::new();
    for (c, r, into, out) in corners {
        if r <= 0.0 {
            anchors.push(Anchor::corner(c));
            continue;
        }
        // The arc from the edge coming in to the edge going out.
        let (a, z) = (c + into * r, c + out * r);
        anchors.push(Anchor {
            after: a - into * (r * KAPPA),
            ..Anchor::corner(a)
        });
        anchors.push(Anchor {
            before: z - out * (r * KAPPA),
            ..Anchor::corner(z)
        });
    }
    SubPath::new(anchors, true)
}

/// The ellipse inscribed in `b`, from its top, clockwise.
pub fn ellipse(b: Bounds) -> SubPath {
    let (c, rx, ry) = (b.center(), b.width() / 2.0, b.height() / 2.0);
    let anchors = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]
        .map(|(x, y)| {
            let p = c + pt(x * rx, y * ry);
            Anchor::smooth(p, p + pt(-y * rx, x * ry) * KAPPA)
        })
        .to_vec();
    SubPath::new(anchors, true)
}

/// Vertices of a regular polygon (`star` < 1: a star, every other vertex at
/// that fraction of the radius), the first at the top.
fn star_points(c: Pt, radius: f32, sides: u32, star: f32, always_star: bool) -> Vec<Pt> {
    let sides = sides.clamp(3, 100);
    let starred = always_star || star < 0.999;
    let n = if starred { sides * 2 } else { sides };
    (0..n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / n as f32;
            let r = if starred && i % 2 == 1 {
                radius * star.clamp(0.01, 1.0)
            } else {
                radius
            };
            c + pt(a.cos(), a.sin()) * r
        })
        .collect()
}

/// A closed polygon, its corners rounded by `radius`.
pub fn rounded_polygon(points: &[Pt], radius: f32) -> SubPath {
    if radius <= 0.0 {
        return SubPath::polygon(points.iter().copied());
    }
    let n = points.len();
    let mut anchors = Vec::with_capacity(n * 2);
    for i in 0..n {
        let (prev, c, next) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        let (u, v) = (prev - c, next - c);
        let (lu, lv) = (u.len(), v.len());
        if lu < 1e-3 || lv < 1e-3 {
            anchors.push(Anchor::corner(c));
            continue;
        }
        let (u, v) = (u * (1.0 / lu), v * (1.0 / lv));
        // Distance from the corner to where the arc meets the edges.
        let half = (u.dot(v).clamp(-1.0, 1.0).acos() / 2.0).max(1e-3);
        let d = (radius / half.tan()).min(lu / 2.0).min(lv / 2.0);
        let r = d * half.tan();
        // Handles of a circular arc of that angle.
        let sweep = std::f32::consts::PI - 2.0 * half;
        let h = 4.0 / 3.0 * (sweep / 4.0).tan() * r;
        let (a, z) = (c + u * d, c + v * d);
        anchors.push(Anchor {
            after: a - u * h,
            ..Anchor::corner(a)
        });
        anchors.push(Anchor {
            before: z - v * h,
            ..Anchor::corner(z)
        });
    }
    SubPath::new(anchors, true)
}

/// A line from `a` to `b`, `weight` thick, with arrowheads (start, end)
/// five times as wide and as long as it is thick (at least 10 px).
pub fn line(a: Pt, b: Pt, weight: f32, arrows: [bool; 2]) -> SubPath {
    let d = b - a;
    let len = d.len().max(1e-3);
    let dir = d * (1.0 / len);
    let side = pt(-dir.y, dir.x);
    let half = weight.max(0.5) / 2.0;
    let head = (weight * 5.0).max(10.0);
    let head_len = head.min(len / if arrows[0] && arrows[1] { 2.0 } else { 1.0 });
    let (start, end) = (
        if arrows[0] { a + dir * head_len } else { a },
        if arrows[1] { b - dir * head_len } else { b },
    );
    let mut points = vec![start + side * half];
    if arrows[1] {
        points.extend([end + side * half, end + side * (head / 2.0), b]);
        points.extend([end - side * (head / 2.0), end - side * half]);
    } else {
        points.extend([end + side * half, end - side * half]);
    }
    points.push(start - side * half);
    if arrows[0] {
        points.extend([start - side * (head / 2.0), a, start + side * (head / 2.0)]);
    }
    SubPath::polygon(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(shape: &Shape, w: u32, h: u32) -> f64 {
        let image = shape.render(PixelRect { x: 0, y: 0, w, h });
        image.pixels().map(|p| p.0[3] as f64 / 255.0).sum()
    }

    fn filled(live: Live) -> Shape {
        Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default())
    }

    #[test]
    fn live_shapes_have_their_area() {
        let r = Bounds::of(pt(10.0, 10.0), pt(50.0, 40.0));
        let rect = filled(Live::new(ShapeKind::Rectangle, r));
        assert!((area(&rect, 64, 64) - 1200.0).abs() < 0.5);
        // Rounded corners take (4 − π) r² off.
        let mut round = Live::new(ShapeKind::Rectangle, r);
        round.radii = [10.0; 4];
        let lost = (4.0 - std::f64::consts::PI) * 100.0;
        assert!((area(&filled(round), 64, 64) - (1200.0 - lost)).abs() < 2.0);
        let e = filled(Live::new(ShapeKind::Ellipse, r));
        let want = std::f64::consts::PI * 20.0 * 15.0;
        assert!((area(&e, 64, 64) - want).abs() < 2.0);
        let t = filled(Live::new(ShapeKind::Triangle, r));
        assert!((area(&t, 64, 64) - 600.0).abs() < 1.0);
        // A square polygon of 4 sides inscribed in the box: half of it.
        let mut p = Live::new(ShapeKind::Polygon, Bounds::of(pt(0.0, 0.0), pt(40.0, 40.0)));
        p.sides = 4;
        assert!((area(&filled(p), 64, 64) - 800.0).abs() < 1.0);
        p.star = 0.5;
        assert!(area(&filled(p), 64, 64) < 800.0);
        let mut l = Live::new(
            ShapeKind::Line,
            Bounds {
                min: pt(5.0, 20.0),
                max: pt(55.0, 20.0),
            },
        );
        l.weight = 4.0;
        assert!((area(&filled(l), 64, 64) - 200.0).abs() < 1.0);
        l.arrows = [false, true];
        assert!(area(&filled(l), 64, 64) > 200.0);
        for c in Custom::ALL {
            let shape = filled(Live::new(ShapeKind::Custom(c), r));
            let a = area(&shape, 64, 64);
            assert!(a > 100.0 && a < 1200.0, "{c:?}: {a}");
        }
    }

    #[test]
    fn strokes_sit_inside_on_or_outside_the_outline() {
        let r = Bounds::of(pt(10.0, 10.0), pt(30.0, 30.0));
        let mut shape = Shape::live(
            Live::new(ShapeKind::Rectangle, r),
            Paint::None,
            ShapeStroke {
                paint: Paint::Color([0, 0, 255]),
                width: 2.0,
                ..Default::default()
            },
        );
        let px = |s: &Shape, x, y| {
            s.render(PixelRect {
                x: 0,
                y: 0,
                w: 40,
                h: 40,
            })
            .get_pixel(x, y)
            .0
        };
        assert_eq!(px(&shape, 10, 20), [0, 0, 255, 255]);
        assert_eq!(px(&shape, 11, 20), [0, 0, 255, 255]);
        assert_eq!(px(&shape, 9, 20)[3], 0);
        assert_eq!(px(&shape, 12, 20)[3], 0);
        shape.stroke.align = Align::Outside;
        assert_eq!(px(&shape, 8, 20), [0, 0, 255, 255]);
        assert_eq!(px(&shape, 10, 20)[3], 0);
        let e = shape.extent().unwrap();
        assert!(e.min.x <= 8.0 && e.max.x >= 32.0);
        shape.stroke.align = Align::Center;
        assert_eq!(px(&shape, 9, 20), [0, 0, 255, 255]);
        assert_eq!(px(&shape, 10, 20), [0, 0, 255, 255]);
        // Over a fill.
        shape.fill = Paint::Color([255, 0, 0]);
        assert_eq!(px(&shape, 20, 20), [255, 0, 0, 255]);
        assert_eq!(px(&shape, 10, 20), [0, 0, 255, 255]);
    }

    #[test]
    fn transforms_keep_live_shapes_while_they_can() {
        let r = Bounds::of(pt(0.0, 0.0), pt(10.0, 20.0));
        let mut s = filled(Live::new(ShapeKind::Rectangle, r));
        s.stroke.width = 2.0;
        s.transform(&Affine([2.0, 0.0, 5.0, 0.0, 2.0, 0.0]));
        let live = s.live.unwrap();
        assert_eq!(
            (live.rect.min, live.rect.max),
            (pt(5.0, 0.0), pt(25.0, 40.0))
        );
        assert_eq!(s.stroke.width, 4.0);
        let b = s.path.bounds().unwrap();
        assert_eq!((b.min, b.max), (pt(5.0, 0.0), pt(25.0, 40.0)));
        // Turned: a plain path.
        let (sin, cos) = 0.5f32.sin_cos();
        s.transform(&Affine([cos, -sin, 0.0, sin, cos, 0.0]));
        assert!(s.live.is_none());
        assert_eq!(s.path.subpaths[0].anchors.len(), 4);
    }
}
