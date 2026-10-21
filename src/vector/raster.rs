//! Paths turned into coverages (0..255 per pixel): exact-area scanline
//! rasterization by `zeno` (already built for the text: no new
//! dependency), each component on its own, combined by the path
//! operations; strokes with their caps, joins and dashes. By bands of
//! rows on all the cores.

use image::GrayImage;
use zeno::{Command, Mask, Stroke, Style, Vector};

use super::geom::{Bounds, Path, PathOp, Pt, SubPath, pt};
use crate::document::{PixelRect, intersect};
use crate::mask_ops::par_rows;

/// End of an open stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Cap {
    #[default]
    Butt,
    Round,
    Square,
}

/// Corner of a stroke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Join {
    #[default]
    Miter,
    Round,
    Bevel,
}

/// How a stroke is drawn along the outline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeStyle {
    pub width: f32,
    pub cap: Cap,
    pub join: Join,
    /// Dash and gap lengths, in pixels; `None`: a solid line.
    pub dash: Option<[f32; 2]>,
}

/// Miter limit of the strokes, Photoshop's.
const MITER_LIMIT: f32 = 4.0;

/// Flattening tolerance of the fills (px): the area stays exact to a
/// fraction of a pixel along the outline (`zeno`'s own is coarser).
const FILL_TOLERANCE: f32 = 0.02;

fn v(p: Pt) -> Vector {
    Vector::new(p.x, p.y)
}

/// The commands of a component's stroke.
fn commands(s: &SubPath) -> Vec<Command> {
    let Some(first) = s.anchors.first() else {
        return Vec::new();
    };
    let mut out = vec![Command::MoveTo(v(first.p))];
    for [p0, c1, c2, p3] in s.segments() {
        out.push(if c1 == p0 && c2 == p3 {
            Command::LineTo(v(p3))
        } else {
            Command::CurveTo(v(c1), v(c2), v(p3))
        });
    }
    if s.closed {
        out.push(Command::Close);
    }
    out
}

/// The commands of a component's fill: its outline flattened, closed.
fn fill_commands(s: &SubPath) -> Vec<Command> {
    let points = s.flatten(FILL_TOLERANCE);
    let mut out: Vec<Command> = points.iter().map(|&p| Command::LineTo(v(p))).collect();
    if let Some(first) = out.first_mut() {
        *first = Command::MoveTo(v(points[0]));
        out.push(Command::Close);
    }
    out
}

fn zeno_stroke<'a>(style: &StrokeStyle, dashes: &'a [f32]) -> Stroke<'a> {
    let mut s = Stroke::new(style.width.max(0.0));
    s.cap(match style.cap {
        Cap::Butt => zeno::Cap::Butt,
        Cap::Round => zeno::Cap::Round,
        Cap::Square => zeno::Cap::Square,
    })
    .join(match style.join {
        Join::Miter => zeno::Join::Miter,
        Join::Round => zeno::Join::Round,
        Join::Bevel => zeno::Join::Bevel,
    })
    .miter_limit(MITER_LIMIT)
    .dash(dashes, 0.0);
    s
}

/// Pixels a box touches.
pub fn pixel_box(b: Bounds) -> PixelRect {
    let (x0, y0) = (b.min.x.floor() as i64, b.min.y.floor() as i64);
    let (x1, y1) = (b.max.x.ceil() as i64, b.max.y.ceil() as i64);
    PixelRect {
        x: x0,
        y: y0,
        w: (x1 - x0).max(0) as u32,
        h: (y1 - y0).max(0) as u32,
    }
}

/// Coverage of `cmds` (in `style`) over `area`.
fn render(cmds: &[Command], style: Style, area: PixelRect) -> Vec<u8> {
    let mut buf = vec![0u8; area.w as usize * area.h as usize];
    Mask::new(cmds)
        .style(style)
        .size(area.w, area.h)
        .offset(Vector::new(-(area.x as f32), -(area.y as f32)))
        .render_into(&mut buf, None);
    buf
}

/// A component ready to rasterize: commands, style, pixels it can touch,
/// and how it combines with those below.
struct Part {
    cmds: Vec<Command>,
    reach: PixelRect,
    op: PathOp,
}

/// Rasterizes the parts over `window` (`style` for all), by bands.
fn rasterize(parts: &[Part], style: Style, window: PixelRect) -> GrayImage {
    let mut out = GrayImage::new(window.w, window.h);
    let w = window.w as usize;
    if w == 0 || window.h == 0 {
        return out;
    }
    par_rows(out.as_mut(), w, |y0, band| {
        let rect = PixelRect {
            y: window.y + y0 as i64,
            h: (band.len() / w) as u32,
            ..window
        };
        for (i, part) in parts.iter().enumerate() {
            // The bottom component is the base, whatever its operation.
            let op = if i == 0 { PathOp::Combine } else { part.op };
            let Some(area) = intersect(part.reach, rect) else {
                if op == PathOp::Intersect {
                    band.fill(0);
                }
                continue;
            };
            let cov = render(&part.cmds, style, area);
            let (ox, oy) = ((area.x - rect.x) as usize, (area.y - rect.y) as usize);
            for (y, line) in band.chunks_exact_mut(w).enumerate() {
                let inside = y >= oy && y < oy + area.h as usize;
                for (x, a) in line.iter_mut().enumerate() {
                    let b = if inside && x >= ox && x < ox + area.w as usize {
                        cov[(y - oy) * area.w as usize + x - ox]
                    } else if op == PathOp::Intersect {
                        0
                    } else {
                        continue;
                    };
                    *a = mix(op, *a, b);
                }
            }
        }
    });
    out
}

/// Coverage `a` (below) combined with `b` (a component) by `op`.
fn mix(op: PathOp, a: u8, b: u8) -> u8 {
    let (a, b) = (a as u32, b as u32);
    let ab = (a * b + 127) / 255;
    (match op {
        PathOp::Combine => a + b - ab,
        PathOp::Subtract => a - (a * b + 127) / 255,
        PathOp::Intersect => ab,
        PathOp::Exclude => a + b - 2 * ab,
    }) as u8
}

/// Coverage of `path` filled (each component nonzero, implicitly closed;
/// combined by their operations), over `window`.
pub fn fill_coverage(path: &Path, window: PixelRect) -> GrayImage {
    let parts: Vec<Part> = path
        .subpaths
        .iter()
        .filter(|s| s.anchors.len() >= 2)
        .filter_map(|s| {
            Some(Part {
                cmds: fill_commands(s),
                reach: pixel_box(s.bounds()?),
                op: s.op,
            })
        })
        .collect();
    rasterize(&parts, Style::Fill(zeno::Fill::NonZero), window)
}

/// Coverage of the stroke of every component of `path`, over `window`.
pub fn stroke_coverage(path: &Path, style: &StrokeStyle, window: PixelRect) -> GrayImage {
    let dashes: Vec<f32> = style
        .dash
        .filter(|[d, g]| d + g > 0.01)
        .map_or(Vec::new(), |[d, g]| vec![d, g]);
    let stroke = zeno_stroke(style, &dashes);
    let parts: Vec<Part> = path
        .subpaths
        .iter()
        .filter(|s| s.anchors.len() >= 2 && style.width > 0.0)
        .map(|s| {
            let cmds = commands(s);
            let b = zeno::bounds(&cmds, stroke, None);
            Part {
                reach: pixel_box(bounds_of(b).expand(1.0)),
                cmds,
                op: PathOp::Combine,
            }
        })
        .collect();
    rasterize(&parts, Style::Stroke(stroke), window)
}

/// Box of the stroke of `path` (exact: caps, joins and miters included).
pub fn stroke_bounds(path: &Path, style: &StrokeStyle) -> Option<Bounds> {
    let stroke = zeno_stroke(style, &[]);
    path.subpaths
        .iter()
        .filter(|s| s.anchors.len() >= 2)
        .map(|s| bounds_of(zeno::bounds(&commands(s), stroke, None)))
        .reduce(Bounds::union)
}

fn bounds_of(b: zeno::Bounds) -> Bounds {
    Bounds {
        min: pt(b.min.x, b.min.y),
        max: pt(b.max.x, b.max.y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::geom::Anchor;

    fn window(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    fn square(x0: f32, y0: f32, x1: f32, y1: f32) -> SubPath {
        SubPath::polygon([pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1)])
    }

    fn total(m: &GrayImage) -> f64 {
        m.as_raw().iter().map(|&v| v as f64 / 255.0).sum()
    }

    #[test]
    fn coverage_is_the_exact_area() {
        // A square on pixel edges: whole pixels in, none out.
        let path = Path {
            subpaths: vec![square(2.0, 2.0, 6.0, 5.0)],
        };
        let m = fill_coverage(&path, window(0, 0, 8, 8));
        assert_eq!(m.get_pixel(2, 2).0[0], 255);
        assert_eq!(m.get_pixel(5, 4).0[0], 255);
        assert_eq!(m.get_pixel(6, 4).0[0], 0);
        assert_eq!(m.get_pixel(1, 2).0[0], 0);
        assert!((total(&m) - 12.0).abs() < 0.01);
        // Half a pixel off: half covered edges, the same area.
        let path = Path {
            subpaths: vec![square(2.5, 2.0, 6.5, 5.0)],
        };
        let m = fill_coverage(&path, window(0, 0, 8, 8));
        assert!((m.get_pixel(2, 3).0[0] as i32 - 128).abs() <= 1);
        assert!((m.get_pixel(6, 3).0[0] as i32 - 128).abs() <= 1);
        assert!((total(&m) - 12.0).abs() < 0.05);
        // A window elsewhere sees the same pixels at its own coordinates.
        let moved = fill_coverage(&path, window(2, 3, 5, 2));
        assert_eq!(moved.get_pixel(0, 0), m.get_pixel(2, 3));
        assert_eq!(moved.get_pixel(4, 1), m.get_pixel(6, 4));
        // A disc of radius 20: π r² within a fraction of a pixel.
        let k = 20.0 * 0.552_284_8;
        let c = pt(30.0, 30.0);
        let anchors = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]
            .map(|(x, y)| {
                let p = c + pt(x, y) * 20.0;
                Anchor::smooth(p, p + pt(-y, x) * k)
            })
            .to_vec();
        let disc = Path {
            subpaths: vec![SubPath::new(anchors, true)],
        };
        let m = fill_coverage(&disc, window(0, 0, 60, 60));
        let area = std::f64::consts::PI * 400.0;
        assert!((total(&m) - area).abs() < 2.0, "{} vs {area}", total(&m));
    }

    #[test]
    fn path_operations_combine_components() {
        let base = square(0.0, 0.0, 10.0, 10.0);
        let with = |op| {
            let mut front = square(5.0, 0.0, 15.0, 10.0);
            front.op = op;
            let path = Path {
                subpaths: vec![base.clone(), front],
            };
            total(&fill_coverage(&path, window(0, 0, 20, 10)))
        };
        assert!((with(PathOp::Combine) - 150.0).abs() < 0.1);
        assert!((with(PathOp::Subtract) - 50.0).abs() < 0.1);
        assert!((with(PathOp::Intersect) - 50.0).abs() < 0.1);
        assert!((with(PathOp::Exclude) - 100.0).abs() < 0.1);
    }

    #[test]
    fn strokes_follow_width_caps_and_dashes() {
        let line = Path {
            subpaths: vec![SubPath::new(
                vec![
                    Anchor::corner(pt(10.0, 10.0)),
                    Anchor::corner(pt(50.0, 10.0)),
                ],
                false,
            )],
        };
        let style = StrokeStyle {
            width: 4.0,
            cap: Cap::Butt,
            join: Join::Miter,
            dash: None,
        };
        let area = |s: &StrokeStyle| total(&stroke_coverage(&line, s, window(0, 0, 64, 20)));
        assert!((area(&style) - 160.0).abs() < 0.5);
        // Square caps add half the width at each end.
        let square_caps = StrokeStyle {
            cap: Cap::Square,
            ..style
        };
        assert!((area(&square_caps) - 176.0).abs() < 0.5);
        // Dashes of 4, gaps of 4: half of it.
        let dashed = StrokeStyle {
            dash: Some([4.0, 4.0]),
            ..style
        };
        assert!((area(&dashed) - 80.0).abs() < 2.0);
        let b = stroke_bounds(&line, &square_caps).unwrap();
        assert!((b.min.x - 8.0).abs() < 0.01 && (b.max.y - 12.0).abs() < 0.01);
    }
}
