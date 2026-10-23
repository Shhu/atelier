//! Paths, like Photoshop's: anchor points with two direction handles,
//! joined by cubic Bézier segments into open or closed components, each
//! combined with those below it by a path operation. Flattening, exact
//! bounds, hit testing and editing.

use std::ops::{Add, Mul, Neg, Sub};

use crate::mask_ops::Affine;

/// A point or a vector, in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

pub const fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

impl Pt {
    pub fn len(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn dist(self, o: Pt) -> f32 {
        (self - o).len()
    }

    pub fn dot(self, o: Pt) -> f32 {
        self.x * o.x + self.y * o.y
    }

    pub fn lerp(self, o: Pt, t: f32) -> Pt {
        self + (o - self) * t
    }

    pub fn map(self, a: &Affine) -> Pt {
        let (x, y) = a.apply(self.x, self.y);
        pt(x, y)
    }
}

impl Add for Pt {
    type Output = Pt;
    fn add(self, o: Pt) -> Pt {
        pt(self.x + o.x, self.y + o.y)
    }
}

impl Sub for Pt {
    type Output = Pt;
    fn sub(self, o: Pt) -> Pt {
        pt(self.x - o.x, self.y - o.y)
    }
}

impl Mul<f32> for Pt {
    type Output = Pt;
    fn mul(self, k: f32) -> Pt {
        pt(self.x * k, self.y * k)
    }
}

impl Neg for Pt {
    type Output = Pt;
    fn neg(self) -> Pt {
        pt(-self.x, -self.y)
    }
}

/// Axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Pt,
    pub max: Pt,
}

impl Bounds {
    pub fn of(a: Pt, b: Pt) -> Bounds {
        Bounds {
            min: pt(a.x.min(b.x), a.y.min(b.y)),
            max: pt(a.x.max(b.x), a.y.max(b.y)),
        }
    }

    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Pt {
        self.min.lerp(self.max, 0.5)
    }

    pub fn add(self, p: Pt) -> Bounds {
        Bounds {
            min: pt(self.min.x.min(p.x), self.min.y.min(p.y)),
            max: pt(self.max.x.max(p.x), self.max.y.max(p.y)),
        }
    }

    pub fn union(self, o: Bounds) -> Bounds {
        self.add(o.min).add(o.max)
    }

    pub fn expand(self, by: f32) -> Bounds {
        Bounds {
            min: self.min - pt(by, by),
            max: self.max + pt(by, by),
        }
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn intersects(&self, o: &Bounds) -> bool {
        self.min.x <= o.max.x
            && o.min.x <= self.max.x
            && self.min.y <= o.max.y
            && o.min.y <= self.max.y
    }
}

/// An anchor point and its direction handles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub p: Pt,
    /// Handle of the segment coming in (from the previous anchor); at `p`
    /// when retracted.
    pub before: Pt,
    /// Handle of the segment going out (to the next anchor).
    pub after: Pt,
    /// Smooth point: dragging one handle turns the other one with it.
    pub smooth: bool,
}

impl Anchor {
    /// Corner point, its handles retracted.
    pub fn corner(p: Pt) -> Anchor {
        Anchor {
            p,
            before: p,
            after: p,
            smooth: false,
        }
    }

    /// Smooth point whose outgoing handle is at `after`, the other opposite.
    pub fn smooth(p: Pt, after: Pt) -> Anchor {
        Anchor {
            p,
            before: p * 2.0 - after,
            after,
            smooth: true,
        }
    }

    fn map(&mut self, f: &impl Fn(Pt) -> Pt) {
        (self.p, self.before, self.after) = (f(self.p), f(self.before), f(self.after));
    }
}

/// How a component combines with the components below it (Photoshop's
/// path operations).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PathOp {
    #[default]
    Combine,
    Subtract,
    Intersect,
    Exclude,
}

impl PathOp {
    pub const ALL: [PathOp; 4] = [
        PathOp::Combine,
        PathOp::Subtract,
        PathOp::Intersect,
        PathOp::Exclude,
    ];

    /// Whether a point inside the components below (`a`) and inside this
    /// one (`b`) is inside the result.
    #[cfg(test)]
    fn holds(self, a: bool, b: bool) -> bool {
        match self {
            PathOp::Combine => a || b,
            PathOp::Subtract => a && !b,
            PathOp::Intersect => a && b,
            PathOp::Exclude => a != b,
        }
    }
}

/// A component: anchors joined in order, back to the first when closed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubPath {
    pub anchors: Vec<Anchor>,
    pub closed: bool,
    pub op: PathOp,
}

/// A cubic segment: start, its handle, the end's handle, end.
pub type Cubic = [Pt; 4];

impl SubPath {
    pub fn new(anchors: Vec<Anchor>, closed: bool) -> SubPath {
        SubPath {
            anchors,
            closed,
            op: PathOp::Combine,
        }
    }

    /// Closed polygon through `points`.
    pub fn polygon(points: impl IntoIterator<Item = Pt>) -> SubPath {
        SubPath::new(points.into_iter().map(Anchor::corner).collect(), true)
    }

    pub fn segment_count(&self) -> usize {
        match self.anchors.len() {
            0 | 1 => 0,
            n if self.closed => n,
            n => n - 1,
        }
    }

    /// Segment `i`, from anchor `i` to the next one.
    pub fn segment(&self, i: usize) -> Cubic {
        let a = &self.anchors[i];
        let b = &self.anchors[(i + 1) % self.anchors.len()];
        [a.p, a.after, b.before, b.p]
    }

    pub fn segments(&self) -> impl Iterator<Item = Cubic> + '_ {
        (0..self.segment_count()).map(|i| self.segment(i))
    }

    /// The same outline with each segment cut in `n` (at equal steps of
    /// its parameter): mapped through a curved map, its points then follow
    /// the curve closely.
    pub fn subdivided(&self, n: usize) -> SubPath {
        let count = self.segment_count();
        if n < 2 || count == 0 {
            return self.clone();
        }
        let mut anchors = self.anchors.clone();
        let mut out: Vec<Anchor> = Vec::with_capacity(self.anchors.len() * n);
        for i in 0..count {
            let mut rest = self.segment(i);
            for k in 0..n - 1 {
                let (left, right) = split_cubic(rest, 1.0 / (n - k) as f32);
                // The segment's start gets its new outgoing handle.
                if k == 0 {
                    anchors[i].after = left[1];
                    out.push(anchors[i]);
                } else {
                    out.last_mut().expect("pushed").after = left[1];
                }
                out.push(Anchor {
                    p: left[3],
                    before: left[2],
                    after: right[1],
                    smooth: true,
                });
                rest = right;
            }
            out.last_mut().expect("pushed").after = rest[1];
            let next = (i + 1) % anchors.len();
            anchors[next].before = rest[2];
            if next == 0 {
                // Closed: the first anchor gets its new incoming handle.
                out[0].before = rest[2];
            }
        }
        if !self.closed {
            out.push(anchors[count]);
        }
        SubPath {
            anchors: out,
            closed: self.closed,
            op: self.op,
        }
    }

    /// Exact box of the outline (handles excluded).
    pub fn bounds(&self) -> Option<Bounds> {
        let first = self.anchors.first()?.p;
        let mut b = Bounds::of(first, first);
        for s in self.segments() {
            b = b.union(cubic_bounds(&s));
        }
        Some(b)
    }

    /// Points along the outline, at most `tolerance` px from the curves;
    /// the closing point is not repeated.
    pub fn flatten(&self, tolerance: f32) -> Vec<Pt> {
        let Some(first) = self.anchors.first() else {
            return Vec::new();
        };
        let mut out = vec![first.p];
        for s in self.segments() {
            flatten_cubic(&s, tolerance, &mut out);
        }
        if self.closed && out.len() > 1 {
            out.pop();
        }
        out
    }

    /// Nonzero winding of the (implicitly closed) outline around `p`.
    fn winds_around(&self, p: Pt) -> bool {
        let poly = self.flatten(0.1);
        let mut winding = 0;
        for (i, a) in poly.iter().enumerate() {
            let b = poly[(i + 1) % poly.len()];
            if (a.y <= p.y) != (b.y <= p.y) {
                let x = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
                if x > p.x {
                    winding += if b.y > a.y { 1 } else { -1 };
                }
            }
        }
        winding != 0
    }

    pub fn map(&mut self, f: &impl Fn(Pt) -> Pt) {
        self.anchors.iter_mut().for_each(|a| a.map(f));
    }
}

/// Where a point lies on a path: component, segment, parameter, distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentHit {
    pub sub: usize,
    pub seg: usize,
    pub t: f32,
    pub dist: f32,
}

/// A path: components from the bottom up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub subpaths: Vec<SubPath>,
}

impl Path {
    pub fn is_empty(&self) -> bool {
        self.subpaths.iter().all(|s| s.anchors.is_empty())
    }

    pub fn bounds(&self) -> Option<Bounds> {
        self.subpaths
            .iter()
            .filter_map(SubPath::bounds)
            .reduce(Bounds::union)
    }

    pub fn map(&mut self, f: impl Fn(Pt) -> Pt) {
        self.subpaths.iter_mut().for_each(|s| s.map(&f));
    }

    pub fn transform(&mut self, a: &Affine) {
        self.map(|p| p.map(a));
    }

    pub fn translate(&mut self, d: Pt) {
        self.map(|p| p + d);
    }

    pub fn translated(&self, d: Pt) -> Path {
        let mut p = self.clone();
        p.translate(d);
        p
    }

    /// Whether `p` is inside the filled path (nonzero per component, the
    /// components combined by their operations).
    #[cfg(test)]
    pub fn contains(&self, p: Pt) -> bool {
        let mut inside = false;
        for (i, s) in self.subpaths.iter().enumerate() {
            if s.anchors.len() < 2 {
                continue;
            }
            let here = s.bounds().is_some_and(|b| b.contains(p)) && s.winds_around(p);
            inside = if i == 0 {
                here
            } else {
                s.op.holds(inside, here)
            };
        }
        inside
    }

    /// The nearest point of the outline to `p`, if within `max` px.
    pub fn nearest(&self, p: Pt, max: f32) -> Option<SegmentHit> {
        let mut best: Option<SegmentHit> = None;
        for (si, s) in self.subpaths.iter().enumerate() {
            for (k, seg) in s.segments().enumerate() {
                if !cubic_bounds(&seg).expand(max).contains(p) {
                    continue;
                }
                let (t, dist) = nearest_on_cubic(&seg, p);
                if dist <= max && best.is_none_or(|b| dist < b.dist) {
                    best = Some(SegmentHit {
                        sub: si,
                        seg: k,
                        t,
                        dist,
                    });
                }
            }
        }
        best
    }

    /// The anchor within `max` px of `p` (the topmost component first).
    pub fn anchor_at(&self, p: Pt, max: f32) -> Option<(usize, usize)> {
        let mut best = None;
        let mut best_d = max;
        for (si, s) in self.subpaths.iter().enumerate().rev() {
            for (ai, a) in s.anchors.iter().enumerate() {
                let d = a.p.dist(p);
                if d <= best_d {
                    (best, best_d) = (Some((si, ai)), d);
                }
            }
        }
        best
    }

    /// The component at `p`: on its outline (within `max` px) or inside it.
    pub fn component_at(&self, p: Pt, max: f32) -> Option<usize> {
        if let Some(h) = self.nearest(p, max) {
            return Some(h.sub);
        }
        (0..self.subpaths.len()).rev().find(|&i| {
            let s = &self.subpaths[i];
            s.closed && s.bounds().is_some_and(|b| b.contains(p)) && s.winds_around(p)
        })
    }

    /// Adds an anchor at `t` on segment `seg` of component `sub`, keeping
    /// the curve's shape (de Casteljau). Returns its index.
    pub fn insert_anchor(&mut self, sub: usize, seg: usize, t: f32) -> usize {
        let s = &mut self.subpaths[sub];
        let [p0, c1, c2, p3] = s.segment(seg);
        let next = (seg + 1) % s.anchors.len();
        let straight = c1 == p0 && c2 == p3;
        let anchor = if straight {
            Anchor::corner(p0.lerp(p3, t))
        } else {
            let (p01, p12, p23) = (p0.lerp(c1, t), c1.lerp(c2, t), c2.lerp(p3, t));
            let (p012, p123) = (p01.lerp(p12, t), p12.lerp(p23, t));
            s.anchors[seg].after = p01;
            s.anchors[next].before = p23;
            Anchor {
                p: p012.lerp(p123, t),
                before: p012,
                after: p123,
                smooth: true,
            }
        };
        s.anchors.insert(seg + 1, anchor);
        seg + 1
    }

    /// Removes an anchor (its neighbors are joined); an emptied component
    /// goes too.
    pub fn remove_anchor(&mut self, sub: usize, i: usize) {
        let s = &mut self.subpaths[sub];
        s.anchors.remove(i);
        if s.anchors.len() < 2 {
            s.closed = false;
        }
        if s.anchors.is_empty() {
            self.subpaths.remove(sub);
        }
    }
}

/// Point at `t` of a cubic.
pub fn cubic_at([p0, c1, c2, p3]: &Cubic, t: f32) -> Pt {
    let u = 1.0 - t;
    p0.mul(u * u * u) + c1.mul(3.0 * u * u * t) + c2.mul(3.0 * u * t * t) + p3.mul(t * t * t)
}

/// A cubic cut at `t` (de Casteljau): the two parts.
pub fn split_cubic([p0, c1, c2, p3]: Cubic, t: f32) -> (Cubic, Cubic) {
    let (a, b, c) = (p0.lerp(c1, t), c1.lerp(c2, t), c2.lerp(p3, t));
    let (d, e) = (a.lerp(b, t), b.lerp(c, t));
    let m = d.lerp(e, t);
    ([p0, a, d, m], [m, e, c, p3])
}

/// Exact box of a cubic: its ends and the extrema of each coordinate.
pub fn cubic_bounds(c: &Cubic) -> Bounds {
    let mut b = Bounds::of(c[0], c[3]);
    let coord = |p: Pt, axis: usize| if axis == 0 { p.x } else { p.y };
    for axis in 0..2 {
        let [p0, p1, p2, p3] = c.map(|p| coord(p, axis));
        // B'(t) / 3 = a t² + b t + c.
        let a = p3 - 3.0 * p2 + 3.0 * p1 - p0;
        let bb = 2.0 * (p2 - 2.0 * p1 + p0);
        let cc = p1 - p0;
        let mut roots = [None, None];
        if a.abs() < 1e-6 {
            if bb.abs() > 1e-9 {
                roots[0] = Some(-cc / bb);
            }
        } else {
            let disc = bb * bb - 4.0 * a * cc;
            if disc >= 0.0 {
                let s = disc.sqrt();
                roots = [Some((-bb + s) / (2.0 * a)), Some((-bb - s) / (2.0 * a))];
            }
        }
        for t in roots.into_iter().flatten().filter(|t| *t > 0.0 && *t < 1.0) {
            b = b.add(cubic_at(c, t));
        }
    }
    b
}

/// Appends points along `c` (its start excluded), at most `tolerance` px
/// from it: uniform steps, as many as its curvature needs.
pub fn flatten_cubic(c: &Cubic, tolerance: f32, out: &mut Vec<Pt>) {
    let [p0, c1, c2, p3] = *c;
    // Handles on the chord: a straight line.
    let chord = p3 - p0;
    let off = |q: Pt| {
        let l = chord.len();
        if l < 1e-6 {
            q.dist(p0)
        } else {
            ((q - p0).x * chord.y - (q - p0).y * chord.x).abs() / l
        }
    };
    if off(c1).max(off(c2)) <= tolerance / 4.0 {
        out.push(p3);
        return;
    }
    let dd = (p0 - c1 * 2.0 + c2).len().max((c1 - c2 * 2.0 + p3).len());
    let n = ((0.75 * dd / tolerance.max(1e-3)).sqrt().ceil() as usize).clamp(1, 1000);
    for i in 1..=n {
        out.push(cubic_at(c, i as f32 / n as f32));
    }
}

/// Parameter and distance of the point of `c` nearest to `p`.
pub fn nearest_on_cubic(c: &Cubic, p: Pt) -> (f32, f32) {
    const STEPS: usize = 32;
    let mut best = (0.0, f32::MAX);
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let d = cubic_at(c, t).dist(p);
        if d < best.1 {
            best = (t, d);
        }
    }
    // Refined by halving the step around the best sample.
    let mut step = 1.0 / STEPS as f32;
    while step > 1e-4 {
        step /= 2.0;
        for t in [best.0 - step, best.0 + step] {
            let t = t.clamp(0.0, 1.0);
            let d = cubic_at(c, t).dist(p);
            if d < best.1 {
                best = (t, d);
            }
        }
    }
    best
}

/// `to`, moved so that the line from `from` makes a multiple of 45°.
pub fn constrain_45(from: Pt, to: Pt) -> Pt {
    let d = to - from;
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (d.y.atan2(d.x) / step).round() * step;
    let dir = pt(angle.cos(), angle.sin());
    from + dir * d.dot(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// A circle of radius 10 around (10, 10), four smooth anchors.
    fn circle() -> Path {
        let k = 10.0 * 0.552_284_8;
        let c = pt(10.0, 10.0);
        let anchors = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]
            .map(|(x, y)| {
                let p = c + pt(x, y) * 10.0;
                // Clockwise tangent.
                Anchor::smooth(p, p + pt(-y, x) * k)
            })
            .to_vec();
        Path {
            subpaths: vec![SubPath::new(anchors, true)],
        }
    }

    #[test]
    fn subdividing_keeps_the_outline() {
        let path = circle();
        let sub = path.subpaths[0].subdivided(3);
        assert_eq!(sub.anchors.len(), 12);
        assert!(sub.closed);
        for (i, s) in path.subpaths[0].segments().enumerate() {
            for k in 0..3 {
                let part = sub.segment(i * 3 + k);
                for j in 0..=4 {
                    let t = j as f32 / 4.0;
                    let (a, b) = (cubic_at(&part, t), cubic_at(&s, (k as f32 + t) / 3.0));
                    assert!(close(a.x, b.x) && close(a.y, b.y), "{a:?} {b:?}");
                }
            }
        }
        let open = SubPath::new(path.subpaths[0].anchors.clone(), false).subdivided(2);
        assert_eq!(open.anchors.len(), 7);
    }

    #[test]
    fn flattening_stays_within_tolerance() {
        let path = circle();
        for tolerance in [1.0, 0.25, 0.05] {
            let points = path.subpaths[0].flatten(tolerance);
            // Every point on the curve, and every chord's middle close to it.
            for (i, a) in points.iter().enumerate() {
                assert!((a.dist(pt(10.0, 10.0)) - 10.0).abs() < 0.03);
                let b = points[(i + 1) % points.len()];
                let mid = a.lerp(b, 0.5);
                assert!(10.0 - mid.dist(pt(10.0, 10.0)) <= tolerance + 0.03);
            }
            assert!(points.len() >= 8);
        }
        // Finer: more points; a straight segment: its end only.
        assert!(path.subpaths[0].flatten(0.05).len() > path.subpaths[0].flatten(1.0).len());
        let mut out = Vec::new();
        flatten_cubic(
            &[pt(0.0, 0.0), pt(0.0, 0.0), pt(5.0, 5.0), pt(5.0, 5.0)],
            0.1,
            &mut out,
        );
        assert_eq!(out, [pt(5.0, 5.0)]);
    }

    #[test]
    fn bounds_are_exact() {
        let b = circle().bounds().unwrap();
        for (v, want) in [
            (b.min.x, 0.0),
            (b.min.y, 0.0),
            (b.max.x, 20.0),
            (b.max.y, 20.0),
        ] {
            assert!(close(v, want), "{b:?}");
        }
        // A bulging curve: beyond its ends, inside its handles.
        let arch = [pt(0.0, 0.0), pt(0.0, -40.0), pt(30.0, -40.0), pt(30.0, 0.0)];
        let b = cubic_bounds(&arch);
        assert!(close(b.min.y, -30.0) && close(b.max.x, 30.0), "{b:?}");
    }

    #[test]
    fn hit_testing() {
        let path = circle();
        let hit = path.nearest(pt(19.0, 10.0), 2.0).unwrap();
        assert!(
            close(hit.dist, 1.0) && hit.seg == 0 && hit.t > 0.99,
            "{hit:?}"
        );
        assert!(path.nearest(pt(10.0, 10.0), 2.0).is_none());
        assert!(path.contains(pt(10.0, 10.0)) && !path.contains(pt(1.0, 1.0)));
        assert_eq!(path.anchor_at(pt(20.5, 10.0), 2.0), Some((0, 1)));
        assert_eq!(path.component_at(pt(12.0, 12.0), 1.0), Some(0));
        // A hole subtracted from a square.
        let square =
            |x0: f32, x1: f32| SubPath::polygon([pt(x0, x0), pt(x1, x0), pt(x1, x1), pt(x0, x1)]);
        let mut hole = square(4.0, 6.0);
        hole.op = PathOp::Subtract;
        let path = Path {
            subpaths: vec![square(0.0, 10.0), hole],
        };
        assert!(path.contains(pt(2.0, 2.0)) && !path.contains(pt(5.0, 5.0)));
        assert!(!path.contains(pt(11.0, 5.0)));
    }

    #[test]
    fn inserting_an_anchor_keeps_the_curve() {
        let mut path = circle();
        let before: Vec<Pt> = (0..=10)
            .map(|i| cubic_at(&path.subpaths[0].segment(1), i as f32 / 10.0))
            .collect();
        let i = path.insert_anchor(0, 1, 0.3);
        assert_eq!((i, path.subpaths[0].anchors.len()), (2, 5));
        // The two halves cover the same curve.
        for p in before {
            assert!(path.nearest(p, 0.01).is_some());
        }
        path.remove_anchor(0, 2);
        assert_eq!(path.subpaths[0].anchors.len(), 4);
        assert!(constrain_45(pt(0.0, 0.0), pt(10.0, 1.0)).y.abs() < 1e-5);
    }
}
