//! Make Work Path: the outline of a selection as a path. Its 50 % contours
//! (marching squares between pixel centers), simplified within a
//! tolerance, smooth where they turn gently; holes subtracted, islands in
//! holes added back.

use std::collections::HashMap;

use image::GrayImage;

use super::geom::{Anchor, Path, PathOp, Pt, SubPath, pt};

/// Turns sharper than this (radians) stay corners.
const CORNER: f32 = 1.0;

/// The path around the pixels of `mask` (placed at `origin`) covered at
/// least half; `tolerance` (px): how far it may stray from the contours.
pub fn trace(mask: &GrayImage, origin: (i64, i64), tolerance: f32) -> Path {
    let loops = contours(mask);
    let mut parts: Vec<(usize, SubPath)> = Vec::new();
    for points in &loops {
        let simple = simplify(points, tolerance.max(0.1));
        if simple.len() < 3 {
            continue;
        }
        // Nesting: how many other contours it is in.
        let depth = loops
            .iter()
            .filter(|o| !std::ptr::eq(*o, points) && inside(o, points[0]))
            .count();
        let o = pt(origin.0 as f32, origin.1 as f32);
        let mut sub = smooth(&simple);
        sub.map(&|p| p + o);
        sub.op = if depth % 2 == 0 {
            PathOp::Combine
        } else {
            PathOp::Subtract
        };
        parts.push((depth, sub));
    }
    parts.sort_by_key(|(d, _)| *d);
    Path {
        subpaths: parts.into_iter().map(|(_, s)| s).collect(),
    }
}

/// Closed 50 % contours of the coverage (pixel centers at x + 0.5), outside
/// counting as empty.
fn contours(mask: &GrayImage) -> Vec<Vec<Pt>> {
    let (w, h) = (mask.width() as i64, mask.height() as i64);
    let v = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            mask.get_pixel(x as u32, y as u32).0[0] as f32
        }
    };
    const ISO: f32 = 127.5;
    // Where the contour crosses the edge between two samples.
    let cross = |a: (i64, i64), b: (i64, i64)| -> Pt {
        let (va, vb) = (v(a.0, a.1), v(b.0, b.1));
        let t = ((ISO - va) / (vb - va)).clamp(0.0, 1.0);
        let (ax, ay) = (a.0 as f32 + 0.5, a.1 as f32 + 0.5);
        pt(ax + (b.0 - a.0) as f32 * t, ay + (b.1 - a.1) as f32 * t)
    };
    // Edges of a cell are named by their sample pair; a segment goes from
    // one edge to another, the inside on its right.
    type Edge = ((i64, i64), (i64, i64));
    let mut next: HashMap<Edge, Edge> = HashMap::new();
    let mut order = Vec::new();
    for y in -1..h {
        for x in -1..w {
            let (tl, tr, br, bl) = ((x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1));
            let on = |p: (i64, i64)| v(p.0, p.1) > ISO;
            let code =
                (on(tl) as u8) << 3 | (on(tr) as u8) << 2 | (on(br) as u8) << 1 | on(bl) as u8;
            let (top, right, bottom, left) = ((tl, tr), (tr, br), (bl, br), (tl, bl));
            let center_on =
                (v(tl.0, tl.1) + v(tr.0, tr.1) + v(br.0, br.1) + v(bl.0, bl.1)) / 4.0 > ISO;
            let segments: &[(Edge, Edge)] = match code {
                0 | 15 => &[],
                0b1000 => &[(left, top)],
                0b0100 => &[(top, right)],
                0b0010 => &[(right, bottom)],
                0b0001 => &[(bottom, left)],
                0b1100 => &[(left, right)],
                0b0110 => &[(top, bottom)],
                0b0011 => &[(right, left)],
                0b1001 => &[(bottom, top)],
                0b0111 => &[(top, left)],
                0b1011 => &[(right, top)],
                0b1101 => &[(bottom, right)],
                0b1110 => &[(left, bottom)],
                // Saddles: the middle decides whether the corners join.
                0b1010 if center_on => &[(left, bottom), (right, top)],
                0b1010 => &[(left, top), (right, bottom)],
                0b0101 if center_on => &[(top, left), (bottom, right)],
                _ => &[(top, right), (bottom, left)],
            };
            for &(a, b) in segments {
                next.insert(a, b);
                order.push(a);
            }
        }
    }
    let mut loops = Vec::new();
    for start in order {
        let mut points = Vec::new();
        let mut at = start;
        while let Some(to) = next.remove(&at) {
            points.push(cross(at.0, at.1));
            at = to;
        }
        if points.len() >= 3 {
            loops.push(points);
        }
    }
    loops
}

/// Even-odd point-in-polygon test.
fn inside(poly: &[Pt], p: Pt) -> bool {
    let mut odd = false;
    for (i, a) in poly.iter().enumerate() {
        let b = poly[(i + 1) % poly.len()];
        if (a.y <= p.y) != (b.y <= p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

/// Distance from `p` to the segment `a`–`b`.
fn to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let d = b - a;
    let l = d.dot(d);
    let t = if l > 0.0 {
        ((p - a).dot(d) / l).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.dist(a + d * t)
}

/// A closed polygon with as few points as keep within `tolerance` of it
/// (Ramer–Douglas–Peucker, from its first point and the farthest one).
fn simplify(points: &[Pt], tolerance: f32) -> Vec<Pt> {
    let n = points.len();
    let far = (0..n)
        .max_by(|&i, &j| {
            points[i]
                .dist(points[0])
                .total_cmp(&points[j].dist(points[0]))
        })
        .unwrap_or(0);
    let ring: Vec<Pt> = points.iter().chain(points).copied().collect();
    let mut kept = vec![false; n + 1];
    // Spans still to examine (indices in `ring`, ends kept).
    let mut spans = vec![(0, far), (far, n)];
    kept[0] = true;
    kept[far] = true;
    while let Some((a, b)) = spans.pop() {
        let far = (a + 1..b)
            .map(|i| (i, to_segment(ring[i], ring[a], ring[b])))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((i, d)) = far
            && d > tolerance
        {
            kept[i] = true;
            spans.extend([(a, i), (i, b)]);
        }
    }
    (0..n).filter(|&i| kept[i]).map(|i| ring[i]).collect()
}

/// Anchors at the points: smooth (tangent to their neighbors) where the
/// outline turns gently, corners elsewhere.
fn smooth(points: &[Pt]) -> SubPath {
    let n = points.len();
    let anchors = (0..n)
        .map(|i| {
            let (prev, p, next) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
            let (u, v) = (p - prev, next - p);
            let turn = (u.x * v.y - u.y * v.x).atan2(u.dot(v)).abs();
            if turn > CORNER {
                return Anchor::corner(p);
            }
            let tangent = next - prev;
            let len = tangent.len().max(1e-6);
            let dir = tangent * (1.0 / len);
            Anchor {
                p,
                before: p - dir * (u.len() / 3.0),
                after: p + dir * (v.len() / 3.0),
                smooth: true,
            }
        })
        .collect();
    SubPath::new(anchors, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::PixelRect;
    use crate::vector::raster::fill_coverage;

    #[test]
    fn a_traced_selection_fills_the_same_pixels() {
        // A square with a hole, and a disc.
        let mut mask = GrayImage::new(60, 40);
        for (x, y, p) in mask.enumerate_pixels_mut() {
            let square = (5..25).contains(&x) && (5..25).contains(&y);
            let hole = (12..18).contains(&x) && (12..18).contains(&y);
            let disc = (x as f32 - 44.5).hypot(y as f32 - 20.5) < 10.0;
            if (square && !hole) || disc {
                p.0[0] = 255;
            }
        }
        let path = trace(&mask, (100, 50), 0.5);
        assert_eq!(path.subpaths.len(), 3);
        assert_eq!(path.subpaths[2].op, PathOp::Subtract);
        let window = PixelRect {
            x: 100,
            y: 50,
            w: 60,
            h: 40,
        };
        let filled = fill_coverage(&path, window);
        let wrong = mask
            .as_raw()
            .iter()
            .zip(filled.as_raw())
            .filter(|(a, b)| (**a >= 128) != (**b >= 128))
            .count();
        assert!(wrong < 30, "{wrong} pixels differ");
        // The square's sides stay straight: few anchors.
        let square = path
            .subpaths
            .iter()
            .find(|s| s.bounds().unwrap().min.x < 110.0)
            .unwrap();
        assert!(square.anchors.len() <= 8, "{}", square.anchors.len());
        // A looser tolerance, fewer anchors.
        let loose = trace(&mask, (0, 0), 3.0);
        let count = |p: &Path| p.subpaths.iter().map(|s| s.anchors.len()).sum::<usize>();
        assert!(count(&loose) < count(&path));
    }
}
