//! Geometry and resampling of Edit › Transform: projective maps (scale,
//! rotate, skew, distort, perspective), the warp's Bezier grid, and images
//! resampled through them (premultiplied, multithreaded).

use image::imageops::FilterType;
use image::{GrayImage, RgbaImage};

use crate::compose::levels::{reduce_gray, reduce_rgba};
use crate::mask_ops::par_rows;

/// A point (canvas coordinates).
pub type Pt = [f64; 2];

/// A box: left, top, right, bottom.
pub type Bounds = [f64; 4];

/// Below this `w`, a point of a perspective is at (or beyond) its horizon.
const HORIZON: f64 = 1e-9;

/// Projective map of the plane: (x, y) → ((a·x + b·y + c) / w,
/// (d·x + e·y + f) / w), w = g·x + h·y + i. The affine maps (scale, rotate,
/// skew, flip) are those with g = h = 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    pub const IDENTITY: Self = Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);

    pub fn affine([a, b, c, d, e, f]: [f64; 6]) -> Self {
        Self([a, b, c, d, e, f, 0.0, 0.0, 1.0])
    }

    /// Turned by `degrees` (clockwise on screen) around `center`.
    pub fn rotate([x, y]: Pt, degrees: f64) -> Self {
        let (s, c) = degrees.to_radians().sin_cos();
        Self::affine([c, -s, x - c * x + s * y, s, c, y - s * x - c * y])
    }

    /// Scaled by `sx`, `sy` from `center` (−1: mirrored).
    pub fn scale([x, y]: Pt, sx: f64, sy: f64) -> Self {
        Self::affine([sx, 0.0, x - sx * x, 0.0, sy, y - sy * y])
    }

    /// The map taking the corners of the unit square (0, 0), (1, 0), (1, 1),
    /// (0, 1) to `q` (Heckbert's construction). `None` if `q` is flat.
    pub fn square_to_quad(q: [Pt; 4]) -> Option<Self> {
        let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = q;
        let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
        let (g, h) = if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            (0.0, 0.0)
        } else {
            let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
            let det = dx1 * dy2 - dx2 * dy1;
            if det.abs() < 1e-12 {
                return None;
            }
            ((sx * dy2 - dx2 * sy) / det, (dx1 * sy - sx * dy1) / det)
        };
        let m = Self([
            x1 - x0 + g * x1,
            x3 - x0 + h * x3,
            x0,
            y1 - y0 + g * y1,
            y3 - y0 + h * y3,
            y0,
            g,
            h,
            1.0,
        ]);
        (m.det().abs() > 1e-12).then_some(m)
    }

    /// The map taking box `r` to `q` (corners top-left, top-right,
    /// bottom-right, bottom-left).
    pub fn rect_to_quad(r: Bounds, q: [Pt; 4]) -> Option<Self> {
        let [x0, y0, x1, y1] = r;
        let (w, h) = (x1 - x0, y1 - y0);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let to_unit = Self::affine([1.0 / w, 0.0, -x0 / w, 0.0, 1.0 / h, -y0 / h]);
        Some(to_unit.then(&Self::square_to_quad(q)?))
    }

    /// `self`, then `next`.
    pub fn then(&self, next: &Self) -> Self {
        let (a, b) = (next.0, self.0);
        Self(std::array::from_fn(|k| {
            let (r, c) = (k / 3, k % 3);
            (0..3).map(|j| a[r * 3 + j] * b[j * 3 + c]).sum()
        }))
    }

    fn det(&self) -> f64 {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    }

    pub fn inverse(&self) -> Option<Self> {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        let det = self.det();
        if det.abs() < 1e-15 {
            return None;
        }
        let m = [
            e * i - f * h,
            c * h - b * i,
            b * f - c * e,
            f * g - d * i,
            a * i - c * g,
            c * d - a * f,
            d * h - e * g,
            b * g - a * h,
            a * e - b * d,
        ];
        Some(Self(m.map(|v| v / det)))
    }

    /// Where `p` goes; `None` beyond the horizon of a perspective.
    pub fn apply(&self, [x, y]: Pt) -> Option<Pt> {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        let w = g * x + h * y + i;
        (w > HORIZON).then(|| [(a * x + b * y + c) / w, (d * x + e * y + f) / w])
    }

    pub fn is_affine(&self) -> bool {
        let [.., g, h, i] = self.0;
        g.abs() <= 1e-12 * i.abs() && h.abs() <= 1e-12 * i.abs()
    }

    /// Where the corners of `r` go (top-left, top-right, bottom-right,
    /// bottom-left).
    pub fn corners(&self, r: Bounds) -> Option<[Pt; 4]> {
        let [x0, y0, x1, y1] = r;
        Some([
            self.apply([x0, y0])?,
            self.apply([x1, y0])?,
            self.apply([x1, y1])?,
            self.apply([x0, y1])?,
        ])
    }

    /// An affine map that is a scale and a rotation (no skew, no mirror),
    /// as the box `r` becomes: the box before rotation and the angle
    /// (degrees, clockwise) around its center.
    pub fn as_turned_box(&self, r: Bounds) -> Option<(Bounds, f64)> {
        if !self.is_affine() {
            return None;
        }
        let [a, b, _, d, e, ..] = self.0.map(|v| v / self.0[8]);
        let (sx, sy) = (a.hypot(d), b.hypot(e));
        let det = a * e - b * d;
        if det <= 0.0 || (a * b + d * e).abs() > 1e-9 * sx * sy {
            return None;
        }
        let [x0, y0, x1, y1] = r;
        let [cx, cy] = self.apply([(x0 + x1) / 2.0, (y0 + y1) / 2.0])?;
        let (w, h) = ((x1 - x0) * sx / 2.0, (y1 - y0) * sy / 2.0);
        Some(([cx - w, cy - h, cx + w, cy + h], d.atan2(a).to_degrees()))
    }
}

/// Whether `q` (corners in order) is a convex quadrilateral with some area:
/// the shapes a perspective can give a box.
pub fn convex(q: &[Pt; 4]) -> bool {
    let cross = |k: usize| {
        let [a, b, c] = [q[k], q[(k + 1) % 4], q[(k + 2) % 4]];
        (b[0] - a[0]) * (c[1] - b[1]) - (b[1] - a[1]) * (c[0] - b[0])
    };
    let turns = [0, 1, 2, 3].map(cross);
    let area = turns.iter().sum::<f64>().abs();
    area > 1e-6 && (turns.iter().all(|&t| t > 0.0) || turns.iter().all(|&t| t < 0.0))
}

// ------------------------------------------------------------------ warp

/// Cubic Bernstein polynomials at `t`.
fn bernstein(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
}

/// Warp: a bicubic Bezier patch over the source box, 4 × 4 control points
/// (canvas) by rows from the top: the 4 corners, the 8 handles beside
/// them and 4 inner points — the 3 × 3 grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp(pub [[Pt; 4]; 4]);

impl Warp {
    /// The patch showing the unit square as `h` places it: its control
    /// points at the thirds (exact for an affine `h`).
    pub fn from_map(h: &Homography) -> Option<Self> {
        let mut points = [[[0.0; 2]; 4]; 4];
        for (j, row) in points.iter_mut().enumerate() {
            for (i, p) in row.iter_mut().enumerate() {
                *p = h.apply([i as f64 / 3.0, j as f64 / 3.0])?;
            }
        }
        Some(Self(points))
    }

    /// The point at (u, v) of the source box (0..1).
    pub fn eval(&self, u: f64, v: f64) -> Pt {
        let (bu, bv) = (bernstein(u), bernstein(v));
        let mut out = [0.0; 2];
        for (j, row) in self.0.iter().enumerate() {
            for (i, p) in row.iter().enumerate() {
                let w = bu[i] * bv[j];
                out[0] += w * p[0];
                out[1] += w * p[1];
            }
        }
        out
    }

    /// Moves the patch so that its point at (u, v) moves by `d`: each
    /// control point by its share (the smallest change that does it).
    pub fn drag(&mut self, (u, v): (f64, f64), d: Pt) {
        let (bu, bv) = (bernstein(u), bernstein(v));
        let norm: f64 = (0..16).map(|k| (bu[k % 4] * bv[k / 4]).powi(2)).sum();
        for (j, row) in self.0.iter_mut().enumerate() {
            for (i, p) in row.iter_mut().enumerate() {
                let w = bu[i] * bv[j] / norm;
                p[0] += w * d[0];
                p[1] += w * d[1];
            }
        }
    }

    /// Corners: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self) -> [Pt; 4] {
        let p = &self.0;
        [p[0][0], p[0][3], p[3][3], p[3][0]]
    }

    pub fn mapped(&self, f: impl Fn(Pt) -> Pt) -> Self {
        Self(self.0.map(|row| row.map(&f)))
    }

    /// The (u, v) whose point is nearest to `p`, within the patch.
    pub fn locate(&self, p: Pt) -> (f64, f64) {
        let dist = |(u, v): (f64, f64)| {
            let q = self.eval(u, v);
            (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)
        };
        const N: usize = 32;
        let mut best = (0.5, 0.5);
        for k in 0..(N + 1) * (N + 1) {
            let c = (
                (k % (N + 1)) as f64 / N as f64,
                (k / (N + 1)) as f64 / N as f64,
            );
            if dist(c) < dist(best) {
                best = c;
            }
        }
        // Refined by halving steps around it.
        let mut step = 1.0 / N as f64;
        for _ in 0..20 {
            step /= 2.0;
            for (du, dv) in [(step, 0.0), (-step, 0.0), (0.0, step), (0.0, -step)] {
                let c = ((best.0 + du).clamp(0.0, 1.0), (best.1 + dv).clamp(0.0, 1.0));
                if dist(c) < dist(best) {
                    best = c;
                }
            }
        }
        best
    }
}

/// The warp styles of the options bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarpStyle {
    /// The grid as dragged.
    Custom,
    Arc,
    Arch,
    Bulge,
    Flag,
    Wave,
}

impl WarpStyle {
    pub const ALL: [WarpStyle; 6] = [
        WarpStyle::Custom,
        WarpStyle::Arc,
        WarpStyle::Arch,
        WarpStyle::Bulge,
        WarpStyle::Flag,
        WarpStyle::Wave,
    ];

    /// The style's patch in the unit square, for a box `aspect` times as
    /// high as wide, bent by `bend` (−1..1); `vertical` turns it a quarter.
    pub fn grid(self, bend: f64, vertical: bool, aspect: f64) -> Warp {
        // Vertical: the horizontal style of the box turned a quarter turn.
        let a = if vertical { 1.0 / aspect } else { aspect };
        let mut g = self.horizontal(bend, a);
        // In the unit square.
        g = g.mapped(|[x, y]| [x, y / a]);
        if vertical {
            let t = g.0;
            g = Warp(std::array::from_fn(|j| {
                std::array::from_fn(|i| {
                    let [x, y] = t[i][j];
                    [y, x]
                })
            }));
        }
        g
    }

    /// The style in the box (0, 0)–(1, `a`).
    fn horizontal(self, bend: f64, a: f64) -> Warp {
        let mut g = Warp(std::array::from_fn(|j| {
            std::array::from_fn(|i| [i as f64 / 3.0, j as f64 / 3.0 * a])
        }));
        // Height of the bend at its peak: half the box at 100%.
        let c = bend * a * 2.0 / 3.0;
        match self {
            WarpStyle::Custom => {}
            WarpStyle::Arch => {
                for row in &mut g.0 {
                    row[1][1] -= c;
                    row[2][1] -= c;
                }
            }
            WarpStyle::Bulge => {
                for (j, row) in g.0.iter_mut().enumerate() {
                    let k = c * (1.0 - 2.0 * j as f64 / 3.0);
                    row[1][1] -= k;
                    row[2][1] -= k;
                }
            }
            WarpStyle::Flag | WarpStyle::Wave => {
                let k = c * 0.8;
                for row in &mut g.0 {
                    row[1][1] -= k;
                    row[2][1] += k;
                    if self == WarpStyle::Wave {
                        // The crests lean: a travelling wave.
                        row[1][0] += k * 0.5;
                        row[2][0] += k * 0.5;
                    }
                }
            }
            WarpStyle::Arc => g = arc(bend, a),
        }
        g
    }
}

/// Arc: the box bent into a ring sector of `bend` × 180°, its middle line
/// keeping its length; upward for a positive bend.
fn arc(bend: f64, a: f64) -> Warp {
    let theta = bend.abs() * std::f64::consts::PI;
    if theta < 1e-6 {
        return WarpStyle::Custom.horizontal(0.0, a);
    }
    let r = 1.0 / theta;
    let center = [0.5, a / 2.0 + r];
    let k = 4.0 / 3.0 * (theta / 4.0).tan();
    let at = |phi: f64, radius: f64| -> (Pt, Pt) {
        let (s, c) = phi.sin_cos();
        ([center[0] + radius * s, center[1] - radius * c], [c, s])
    };
    let mut g = Warp([[[0.0; 2]; 4]; 4]);
    for (j, row) in g.0.iter_mut().enumerate() {
        let radius = r + (0.5 - j as f64 / 3.0) * a;
        let (p0, t0) = at(-theta / 2.0, radius);
        let (p3, t3) = at(theta / 2.0, radius);
        let l = k * radius;
        *row = [
            p0,
            [p0[0] + l * t0[0], p0[1] + l * t0[1]],
            [p3[0] - l * t3[0], p3[1] - l * t3[1]],
            p3,
        ];
    }
    if bend < 0.0 {
        // Downward: the upward one mirrored, its rows reversed.
        let up = g.0;
        g = Warp(std::array::from_fn(|j| up[3 - j].map(|[x, y]| [x, a - y])));
    }
    g
}

// ------------------------------------------------------------- transform

/// Where a transform puts the points of its source box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Map {
    /// Canvas → canvas.
    Projective(Homography),
    /// (u, v) of the source box (0..1) → canvas.
    Warp(Warp),
}

/// A transform: the source box (canvas) and where it goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub src: Bounds,
    pub map: Map,
}

impl Transform {
    pub fn projective(src: Bounds, h: Homography) -> Self {
        Self {
            src,
            map: Map::Projective(h),
        }
    }

    /// Where `p` (canvas) goes.
    pub fn forward(&self, p: Pt) -> Option<Pt> {
        match &self.map {
            Map::Projective(h) => h.apply(p),
            Map::Warp(w) => {
                let (u, v) = self.uv(p);
                Some(w.eval(u, v))
            }
        }
    }

    fn uv(&self, [x, y]: Pt) -> (f64, f64) {
        let [x0, y0, x1, y1] = self.src;
        ((x - x0) / (x1 - x0), (y - y0) / (y1 - y0))
    }

    /// Leaves everything in place.
    pub fn is_identity(&self) -> bool {
        let [x0, y0, x1, y1] = self.src;
        let same = |p: Pt| {
            self.forward(p)
                .is_some_and(|q| (q[0] - p[0]).abs() < 1e-6 && (q[1] - p[1]).abs() < 1e-6)
        };
        // A warp: at the thirds too (its control points).
        (0..16).all(|k| {
            let (i, j) = ((k % 4) as f64 / 3.0, (k / 4) as f64 / 3.0);
            same([x0 + (x1 - x0) * i, y0 + (y1 - y0) * j])
        })
    }

    /// Changes whenever the transform does (cache keys).
    pub fn fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.src.map(f64::to_bits).hash(&mut h);
        match &self.map {
            Map::Projective(m) => m.0.map(f64::to_bits).hash(&mut h),
            Map::Warp(w) => {
                for p in w.0.iter().flatten() {
                    p.map(f64::to_bits).hash(&mut h);
                }
            }
        }
        h.finish()
    }

    /// Box (canvas) of what the box `area` becomes; `None` if part of it
    /// goes beyond the horizon.
    pub fn bounds(&self, area: Bounds) -> Option<Bounds> {
        let points: Vec<Pt> = match &self.map {
            Map::Projective(h) => h.corners(area)?.to_vec(),
            Map::Warp(_) => {
                const N: usize = 32;
                (0..(N + 1) * (N + 1))
                    .filter_map(|k| {
                        let (i, j) = ((k % (N + 1)) as f64, (k / (N + 1)) as f64);
                        let x = area[0] + (area[2] - area[0]) * i / N as f64;
                        let y = area[1] + (area[3] - area[1]) * j / N as f64;
                        self.forward([x, y])
                    })
                    .collect()
            }
        };
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for [x, y] in points {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        b.iter().all(|v| v.is_finite()).then_some(b)
    }
}

/// The part of box `a` inside box `b` (empty: right of or below its left
/// top).
pub fn clip(a: Bounds, b: Bounds) -> Bounds {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}

// ------------------------------------------------------------ resampling

/// Resampling filters (the Resample choice).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos,
}

impl From<FilterType> for Filter {
    fn from(f: FilterType) -> Self {
        match f {
            FilterType::Nearest => Filter::Nearest,
            FilterType::Triangle => Filter::Bilinear,
            FilterType::CatmullRom | FilterType::Gaussian => Filter::Bicubic,
            FilterType::Lanczos3 => Filter::Lanczos,
        }
    }
}

impl Filter {
    /// Half-width of the kernel, in pixels.
    fn support(self) -> f64 {
        match self {
            Filter::Nearest | Filter::Bilinear => 1.0,
            Filter::Bicubic => 2.0,
            Filter::Lanczos => 3.0,
        }
    }

    fn weight(self, t: f64) -> f64 {
        let t = t.abs();
        match self {
            Filter::Nearest | Filter::Bilinear => (1.0 - t).max(0.0),
            // Catmull-Rom.
            Filter::Bicubic if t < 1.0 => (1.5 * t - 2.5) * t * t + 1.0,
            Filter::Bicubic if t < 2.0 => ((-0.5 * t + 2.5) * t - 4.0) * t + 2.0,
            Filter::Bicubic => 0.0,
            Filter::Lanczos if t < 1e-8 => 1.0,
            Filter::Lanczos if t < 3.0 => {
                let x = std::f64::consts::PI * t;
                3.0 * x.sin() * (x / 3.0).sin() / (x * x)
            }
            Filter::Lanczos => 0.0,
        }
    }
}

/// A grid of pixels `px` canvas pixels wide: pixel (i, j) covers canvas
/// [(x + i)·px, (x + i + 1)·px) × [(y + j)·px, (y + j + 1)·px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
    pub px: f64,
}

impl Grid {
    /// The pixels `px` wide that cover the canvas box `b`.
    pub fn covering(b: Bounds, px: f64) -> Grid {
        let (x0, y0) = ((b[0] / px).floor() as i64, (b[1] / px).floor() as i64);
        let (x1, y1) = ((b[2] / px).ceil() as i64, (b[3] / px).ceil() as i64);
        Grid {
            x: x0,
            y: y0,
            w: (x1 - x0).max(0) as u32,
            h: (y1 - y0).max(0) as u32,
            px,
        }
    }
}

/// An image on the canvas: the top-left corner of its first pixel and the
/// canvas size of its pixels.
pub struct Source<'a, I> {
    pub image: &'a I,
    pub origin: Pt,
    pub px: f64,
}

impl<I> Clone for Source<'_, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for Source<'_, I> {}

/// `src` as `t` transforms it, over the pixels of `out`; transparent
/// where nothing comes.
pub fn transform_rgba(
    src: Source<RgbaImage>,
    t: &Transform,
    out: Grid,
    filter: Filter,
) -> RgbaImage {
    let data = render::<4, RgbaImage>(src, t, out, filter, [0; 4]);
    RgbaImage::from_raw(out.w, out.h, data).expect("size")
}

/// [`transform_rgba`] for a mask: `outside` where nothing comes.
pub fn transform_gray(
    src: Source<GrayImage>,
    t: &Transform,
    out: Grid,
    filter: Filter,
    outside: u8,
) -> GrayImage {
    let data = render::<1, GrayImage>(src, t, out, filter, [outside]);
    GrayImage::from_raw(out.w, out.h, data).expect("size")
}

/// The images resampled: RGBA (premultiplied while filtering) or gray.
trait Pixels: Sync + Sized {
    fn raw(&self) -> &[u8];
    fn size(&self) -> (u32, u32);
    /// Halved (box filter).
    fn half(&self) -> Self;
}

impl Pixels for RgbaImage {
    fn raw(&self) -> &[u8] {
        self.as_raw()
    }

    fn size(&self) -> (u32, u32) {
        self.dimensions()
    }

    fn half(&self) -> Self {
        reduce_rgba(self, 2)
    }
}

impl Pixels for GrayImage {
    fn raw(&self) -> &[u8] {
        self.as_raw()
    }

    fn size(&self) -> (u32, u32) {
        self.dimensions()
    }

    fn half(&self) -> Self {
        reduce_gray(self, 2)
    }
}

/// Most source pixels a kernel spreads over per destination pixel along
/// an axis, beyond the reduced levels (strong anisotropy).
const MAX_SPREAD: f64 = 4.0;

/// How many times the source must be reduced for a destination pixel
/// covering `kx` × `ky` of its pixels: enough for the narrower side, and
/// for the wider one to stay within [`MAX_SPREAD`].
fn reduction(kx: f64, ky: f64) -> f64 {
    kx.min(ky).max(kx.max(ky) / MAX_SPREAD).max(1.0)
}

/// The source, and copies of it halved again and again for the parts the
/// transform shrinks.
struct Sampler<'a, const C: usize, I: Clone> {
    levels: Vec<std::borrow::Cow<'a, I>>,
    filter: Filter,
    outside: [f32; C],
}

impl<const C: usize, I: Pixels + Clone> Sampler<'_, C, I> {
    /// Premultiplied value of pixel (x, y) of level `l`.
    fn texel(&self, l: usize, x: i64, y: i64) -> [f32; C] {
        let image = &self.levels[l];
        let (w, h) = image.size();
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return self.outside;
        }
        let p = &image.raw()[(y as usize * w as usize + x as usize) * C..][..C];
        if C == 4 {
            let a = p[3] as f32 / 255.0;
            std::array::from_fn(|c| if c == 3 { p[3] as f32 } else { p[c] as f32 * a })
        } else {
            std::array::from_fn(|c| p[c] as f32)
        }
    }

    /// The source at (x, y) (its pixels, centers at i + 0.5), for a
    /// destination pixel covering `kx` × `ky` of them.
    fn sample(&self, x: f64, y: f64, kx: f64, ky: f64) -> [f32; C] {
        if self.filter == Filter::Nearest {
            return self.texel(0, x.floor() as i64, y.floor() as i64);
        }
        let l = match self.levels.len() {
            1 => 0,
            n => (reduction(kx, ky).log2().floor() as usize).min(n - 1),
        };
        let s = (1u64 << l) as f64;
        let (x, y) = (x / s - 0.5, y / s - 0.5);
        let (kx, ky) = (
            (kx / s).clamp(1.0, MAX_SPREAD),
            (ky / s).clamp(1.0, MAX_SPREAD),
        );
        if self.filter == Filter::Bilinear && kx == 1.0 && ky == 1.0 {
            let (x0, y0) = (x.floor(), y.floor());
            let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
            let (x0, y0) = (x0 as i64, y0 as i64);
            let [a, b, c, d] = [
                self.texel(l, x0, y0),
                self.texel(l, x0 + 1, y0),
                self.texel(l, x0, y0 + 1),
                self.texel(l, x0 + 1, y0 + 1),
            ];
            return std::array::from_fn(|ch| {
                let top = a[ch] + (b[ch] - a[ch]) * fx;
                let bottom = c[ch] + (d[ch] - c[ch]) * fx;
                top + (bottom - top) * fy
            });
        }
        let r = self.filter.support();
        let taps = |c: f64, k: f64, out: &mut [f32; 32]| -> (i64, usize, f32) {
            let first = (c - r * k).ceil() as i64;
            let last = (c + r * k).floor() as i64;
            let n = ((last - first + 1).max(0) as usize).min(out.len());
            let mut sum = 0.0;
            for (j, w) in out.iter_mut().take(n).enumerate() {
                *w = self.filter.weight((first + j as i64) as f64 - c) as f32 / k as f32;
                sum += *w;
            }
            (first, n, sum)
        };
        let (mut wx, mut wy) = ([0f32; 32], [0f32; 32]);
        let (x0, nx, sx) = taps(x, kx, &mut wx);
        let (y0, ny, sy) = taps(y, ky, &mut wy);
        let norm = sx * sy;
        if norm.abs() < 1e-6 {
            return self.outside;
        }
        let mut acc = [0f32; C];
        for (j, &w) in wy.iter().take(ny).enumerate() {
            let mut row = [0f32; C];
            for (i, &v) in wx.iter().take(nx).enumerate() {
                let p = self.texel(l, x0 + i as i64, y0 + j as i64);
                for c in 0..C {
                    row[c] += v * p[c];
                }
            }
            for c in 0..C {
                acc[c] += w * row[c];
            }
        }
        acc.map(|v| v / norm)
    }
}

/// Writes a premultiplied (RGBA) or gray value.
fn store<const C: usize>(px: &mut [u8], v: [f32; C]) {
    if C == 4 {
        // Unpremultiplied by the filtered alpha, even beyond 255 (a ringing
        // filter's overshoot): the colors keep their ratios.
        let a = v[3];
        if a < 0.5 {
            px.fill(0);
            return;
        }
        for c in 0..3 {
            px[c] = (v[c].clamp(0.0, a) * 255.0 / a).round() as u8;
        }
        px[3] = a.min(255.0).round() as u8;
    } else {
        px[0] = v[0].round().clamp(0.0, 255.0) as u8;
    }
}

/// A triangle of the warp's mesh, in destination pixels: its corners, and
/// the affine map back to source pixels.
struct Triangle {
    v: [[f64; 2]; 3],
    /// Source x and y = k[0] + k[1]·x + k[2]·y (destination pixels).
    sx: [f64; 3],
    sy: [f64; 3],
    /// How many source pixels a destination pixel covers.
    spread: (f64, f64),
}

impl Triangle {
    fn new(v: [[f64; 2]; 3], s: [[f64; 2]; 3]) -> Option<Self> {
        let [a, b, c] = v;
        let det = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        if det.abs() < 1e-9 {
            return None;
        }
        let solve = |k: usize| {
            let (fa, fb, fc) = (s[0][k], s[1][k], s[2][k]);
            let dx = ((fb - fa) * (c[1] - a[1]) - (fc - fa) * (b[1] - a[1])) / det;
            let dy = ((fc - fa) * (b[0] - a[0]) - (fb - fa) * (c[0] - a[0])) / det;
            [fa - dx * a[0] - dy * a[1], dx, dy]
        };
        let (sx, sy) = (solve(0), solve(1));
        let spread = (sx[1].hypot(sx[2]), sy[1].hypot(sy[2]));
        Some(Self { v, sx, sy, spread })
    }

    fn y_range(&self) -> (f64, f64) {
        let ys = self.v.map(|p| p[1]);
        (ys[0].min(ys[1]).min(ys[2]), ys[0].max(ys[1]).max(ys[2]))
    }

    /// Calls `f(x, y)` for the pixels of rows `rows` whose center is inside.
    fn cover(&self, rows: (i64, i64), width: i64, mut f: impl FnMut(i64, i64)) {
        let [a, b, c] = self.v;
        let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        let edge = |p: [f64; 2], q: [f64; 2], x: f64, y: f64| {
            ((q[0] - p[0]) * (y - p[1]) - (q[1] - p[1]) * (x - p[0])) * area.signum()
        };
        let eps = -1e-7 * area.abs();
        let (ymin, ymax) = self.y_range();
        let xs = self.v.map(|p| p[0]);
        let (xmin, xmax) = (xs[0].min(xs[1]).min(xs[2]), xs[0].max(xs[1]).max(xs[2]));
        let y0 = ((ymin - 0.5).ceil() as i64).max(rows.0);
        let y1 = ((ymax - 0.5).floor() as i64).min(rows.1 - 1);
        let x0 = ((xmin - 0.5).ceil() as i64).max(0);
        let x1 = ((xmax - 0.5).floor() as i64).min(width - 1);
        for y in y0..=y1 {
            let cy = y as f64 + 0.5;
            for x in x0..=x1 {
                let cx = x as f64 + 0.5;
                if edge(a, b, cx, cy) >= eps
                    && edge(b, c, cx, cy) >= eps
                    && edge(c, a, cx, cy) >= eps
                {
                    f(x, y);
                }
            }
        }
    }
}

/// The warp's patch cut into triangles (destination pixels of `out`,
/// source pixels of `src`), overflowing the source box a little so that
/// its edges are smoothed like the rest.
fn mesh(w: &Warp, t: &Transform, origin: Pt, px: f64, out: Grid) -> Vec<Triangle> {
    let to_out = |[x, y]: Pt| [x / out.px - out.x as f64, y / out.px - out.y as f64];
    let corners = w.corners().map(to_out);
    let extent = corners
        .iter()
        .flat_map(|a| {
            corners
                .iter()
                .map(move |b| (a[0] - b[0]).hypot(a[1] - b[1]))
        })
        .fold(1.0, f64::max);
    let n = ((extent / 6.0).ceil() as usize).clamp(8, 256);
    let margin = (2.0 / extent).min(0.25);
    let [x0, y0, x1, y1] = t.src;
    let at = |k: usize| -margin + (1.0 + 2.0 * margin) * k as f64 / n as f64;
    let vertices: Vec<([f64; 2], [f64; 2])> = (0..=n)
        .flat_map(|j| (0..=n).map(move |i| (at(i), at(j))))
        .map(|(u, v)| {
            let src = [
                (x0 + (x1 - x0) * u - origin[0]) / px,
                (y0 + (y1 - y0) * v - origin[1]) / px,
            ];
            (to_out(w.eval(u, v)), src)
        })
        .collect();
    let mut out = Vec::with_capacity(2 * n * n);
    for j in 0..n {
        for i in 0..n {
            let k = j * (n + 1) + i;
            let [a, b, c, d] = [k, k + 1, k + n + 2, k + n + 1].map(|k| vertices[k]);
            for tri in [[a, b, c], [a, c, d]] {
                out.extend(Triangle::new(tri.map(|t| t.0), tri.map(|t| t.1)));
            }
        }
    }
    out
}

fn render<const C: usize, I: Pixels + Clone>(
    src: Source<I>,
    t: &Transform,
    out: Grid,
    filter: Filter,
    outside: [u8; C],
) -> Vec<u8> {
    let (w, h) = (out.w as usize, out.h as usize);
    let mut data: Vec<u8> = outside.iter().copied().cycle().take(w * h * C).collect();
    if w == 0 || h == 0 {
        return data;
    }
    let scale = out.px / src.px;
    let to_src = |[x, y]: Pt| [(x - src.origin[0]) / src.px, (y - src.origin[1]) / src.px];
    // Destination pixel (x, y) → source pixel coordinates and spread.
    let inverse = match &t.map {
        Map::Projective(h) => Some(h.inverse()),
        Map::Warp(_) => None,
    };
    let at = |x: f64, y: f64| -> Option<(Pt, (f64, f64))> {
        let hi = inverse.flatten()?;
        let [a, b, c, d, e, f, g, hh, i] = hi.0;
        let (cx, cy) = (
            (out.x as f64 + x + 0.5) * out.px,
            (out.y as f64 + y + 0.5) * out.px,
        );
        let wv = g * cx + hh * cy + i;
        if wv <= HORIZON {
            return None;
        }
        let (sx, sy) = ((a * cx + b * cy + c) / wv, (d * cx + e * cy + f) / wv);
        let k = scale / wv;
        let gx = k * ((a - g * sx).powi(2) + (b - hh * sx).powi(2)).sqrt();
        let gy = k * ((d - g * sy).powi(2) + (e - hh * sy).powi(2)).sqrt();
        Some((to_src([sx, sy]), (gx, gy)))
    };
    let triangles = match &t.map {
        Map::Warp(wp) => mesh(wp, t, src.origin, src.px, out),
        Map::Projective(_) => Vec::new(),
    };
    // How much the transform shrinks at most: the reduced levels needed.
    let shrink = match &t.map {
        Map::Projective(_) => (0..=16 * 17)
            .filter_map(|k| {
                at(
                    (k % 17) as f64 * w as f64 / 16.0,
                    (k / 17) as f64 * h as f64 / 16.0,
                )
            })
            .map(|(_, (a, b))| reduction(a, b))
            .fold(1.0, f64::max),
        Map::Warp(_) => triangles
            .iter()
            .map(|t| reduction(t.spread.0, t.spread.1))
            .fold(1.0, f64::max),
    };
    let mut levels = vec![std::borrow::Cow::Borrowed(src.image)];
    if filter != Filter::Nearest {
        while (1u64 << levels.len()) as f64 <= shrink {
            let last = levels.last().expect("level 0");
            if last.size().0 < 2 && last.size().1 < 2 {
                break;
            }
            let half = last.half();
            levels.push(std::borrow::Cow::Owned(half));
        }
    }
    let sampler = Sampler {
        levels,
        filter,
        outside: if C == 4 {
            [0.0; C]
        } else {
            outside.map(f32::from)
        },
    };
    let row = w * C;
    par_rows(&mut data, row, |first, band| {
        let rows = band.len() / row;
        if triangles.is_empty() {
            for (k, line) in band.chunks_exact_mut(row).enumerate() {
                let y = (first + k) as f64;
                for (x, px) in line.chunks_exact_mut(C).enumerate() {
                    if let Some(([sx, sy], (kx, ky))) = at(x as f64, y) {
                        store(px, sampler.sample(sx, sy, kx, ky));
                    }
                }
            }
            return;
        }
        let span = (first as i64, (first + rows) as i64);
        for tri in &triangles {
            let (ymin, ymax) = tri.y_range();
            if ymax < span.0 as f64 || ymin > span.1 as f64 {
                continue;
            }
            tri.cover(span, w as i64, |x, y| {
                let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
                let sx = tri.sx[0] + tri.sx[1] * cx + tri.sx[2] * cy;
                let sy = tri.sy[0] + tri.sy[1] * cx + tri.sy[2] * cy;
                let i = ((y - span.0) as usize * w + x as usize) * C;
                store(
                    &mut band[i..i + C],
                    sampler.sample(sx, sy, tri.spread.0, tri.spread.1),
                );
            });
        }
    });
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba};

    fn close(a: Pt, b: Pt) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    const QUAD: [Pt; 4] = [[10.0, 20.0], [110.0, 5.0], [130.0, 90.0], [-5.0, 70.0]];

    #[test]
    fn projective_map_puts_the_corners_exactly() {
        let src = [0.0, 0.0, 40.0, 30.0];
        let h = Homography::rect_to_quad(src, QUAD).unwrap();
        assert!(!h.is_affine());
        for (got, want) in h.corners(src).unwrap().into_iter().zip(QUAD) {
            assert!(close(got, want), "{got:?} {want:?}");
        }
        // Straight lines stay straight: the middle of an edge is on it.
        let [m] = [h.apply([20.0, 0.0]).unwrap()];
        let [a, b] = [QUAD[0], QUAD[1]];
        let cross = (b[0] - a[0]) * (m[1] - a[1]) - (b[1] - a[1]) * (m[0] - a[0]);
        assert!(cross.abs() < 1e-9);
    }

    #[test]
    fn inverse_mapping_goes_back() {
        let h = Homography::rect_to_quad([0.0, 0.0, 40.0, 30.0], QUAD).unwrap();
        let back = h.inverse().unwrap();
        for p in [[3.0, 4.0], [39.0, 1.0], [20.0, 15.0], [0.5, 29.5]] {
            let q = back.apply(h.apply(p).unwrap()).unwrap();
            assert!(close(p, q), "{p:?} {q:?}");
        }
        assert!(close(h.then(&back).apply([7.0, 8.0]).unwrap(), [7.0, 8.0]));
        // Beyond the horizon: no point.
        let far = Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, -0.01, 0.0, 1.0]);
        assert!(far.apply([200.0, 0.0]).is_none());
    }

    #[test]
    fn affine_maps_are_recognized() {
        let src = [0.0, 0.0, 10.0, 20.0];
        let turn = Homography::rotate([5.0, 10.0], 30.0);
        let quad = turn.corners(src).unwrap();
        let h = Homography::rect_to_quad(src, quad).unwrap();
        assert!(h.is_affine());
        let (b, angle) = h.as_turned_box(src).unwrap();
        assert!((angle - 30.0).abs() < 1e-9);
        assert!(close([b[0], b[1]], [0.0, 0.0]) && close([b[2], b[3]], [10.0, 20.0]));
        // Mirrored or skewed: not a turned box.
        let flip = Homography::scale([5.0, 0.0], -1.0, 1.0);
        assert!(flip.as_turned_box(src).is_none());
        let skew = Homography::affine([1.0, 0.5, 0.0, 0.0, 1.0, 0.0]);
        assert!(skew.as_turned_box(src).is_none());
    }

    #[test]
    fn convex_quads_only() {
        assert!(convex(&QUAD));
        let mut bow = QUAD;
        bow.swap(1, 2);
        assert!(!convex(&bow));
        let mut dent = QUAD;
        dent[2] = [30.0, 30.0];
        assert!(!convex(&dent));
    }

    #[test]
    fn warp_grid_identity_drag_and_styles() {
        let h = Homography::rect_to_quad(
            [0.0, 0.0, 1.0, 1.0],
            [[0.0, 0.0], [90.0, 0.0], [90.0, 60.0], [0.0, 60.0]],
        )
        .unwrap();
        let mut w = Warp::from_map(&h).unwrap();
        for (u, v) in [(0.0, 0.0), (0.3, 0.7), (1.0, 0.5), (0.25, 1.0)] {
            assert!(close(w.eval(u, v), [90.0 * u, 60.0 * v]));
        }
        assert_eq!(
            w.corners(),
            [[0.0, 0.0], [90.0, 0.0], [90.0, 60.0], [0.0, 60.0]]
        );
        // A dragged point follows the pointer exactly.
        let before = w.eval(0.4, 0.6);
        w.drag((0.4, 0.6), [5.0, -3.0]);
        assert!(close(w.eval(0.4, 0.6), [before[0] + 5.0, before[1] - 3.0]));
        let (u, v) = w.locate(w.eval(0.2, 0.9));
        assert!((u - 0.2).abs() < 1e-4 && (v - 0.9).abs() < 1e-4);
        // Without bend, every style is the box itself.
        let flat = WarpStyle::Custom.grid(0.0, false, 0.5);
        for style in WarpStyle::ALL {
            for vertical in [false, true] {
                let g = style.grid(0.0, vertical, 0.5);
                for (a, b) in g.0.iter().flatten().zip(flat.0.iter().flatten()) {
                    assert!(close(*a, *b), "{style:?}");
                }
            }
        }
        // Arch: the middle goes up, the corners stay.
        let arch = WarpStyle::Arch.grid(0.5, false, 1.0);
        assert!(arch.eval(0.5, 0.0)[1] < -0.2);
        assert!(close(arch.eval(0.0, 0.0), [0.0, 0.0]));
        // Arc: the middle line keeps its length, the corners spread.
        let arc = WarpStyle::Arc.grid(0.5, false, 0.5);
        let length: f64 = (0..100)
            .map(|k| {
                let (a, b) = (
                    arc.eval(k as f64 / 100.0, 0.5),
                    arc.eval((k + 1) as f64 / 100.0, 0.5),
                );
                (a[0] - b[0]).hypot((a[1] - b[1]) * 0.5)
            })
            .sum();
        assert!((length - 1.0).abs() < 0.01, "{length}");
        assert!(arc.eval(0.5, 0.0)[1].abs() < 1e-9);
        assert!(arc.eval(0.0, 0.0)[1] > 0.2, "the ends go down");
        let down = WarpStyle::Arc.grid(-0.5, false, 0.5);
        assert!(down.eval(0.0, 1.0)[1] < 0.8, "the ends go up");
        // Vertical: the same, turned.
        let side = WarpStyle::Arch.grid(0.5, true, 1.0);
        assert!(side.eval(0.0, 0.5)[0] < -0.2);
    }

    fn checker(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            Rgba([
                (x * 37 % 256) as u8,
                (y * 53 % 256) as u8,
                ((x + y) * 11 % 256) as u8,
                255,
            ])
        })
    }

    #[test]
    fn exact_turns_and_flips_are_lossless() {
        let image = checker(7, 4);
        let src = [0.0, 0.0, 7.0, 4.0];
        let source = Source {
            image: &image,
            origin: [0.0, 0.0],
            px: 1.0,
        };
        for filter in [
            Filter::Nearest,
            Filter::Bilinear,
            Filter::Bicubic,
            Filter::Lanczos,
        ] {
            // A quarter turn clockwise, into (10, 20)–(14, 27).
            let quad = [[14.0, 20.0], [14.0, 27.0], [10.0, 27.0], [10.0, 20.0]];
            let h = Homography::rect_to_quad(src, quad).unwrap();
            let t = Transform::projective(src, h);
            let out = Grid {
                x: 10,
                y: 20,
                w: 4,
                h: 7,
                px: 1.0,
            };
            let turned = transform_rgba(source, &t, out, filter);
            assert_eq!(turned, image::imageops::rotate90(&image), "{filter:?}");
            let flip = Homography::scale([3.5, 0.0], -1.0, 1.0);
            let t = Transform::projective(src, flip);
            let out = Grid {
                x: 0,
                y: 0,
                w: 7,
                h: 4,
                px: 1.0,
            };
            let flipped = transform_rgba(source, &t, out, filter);
            assert_eq!(
                flipped,
                image::imageops::flip_horizontal(&image),
                "{filter:?}"
            );
        }
    }

    #[test]
    fn perspective_fills_its_quad_and_edges_stay_clean() {
        let image = RgbaImage::from_pixel(40, 30, Rgba([200, 30, 30, 255]));
        let src = [0.0, 0.0, 40.0, 30.0];
        let h = Homography::rect_to_quad(src, QUAD).unwrap();
        let t = Transform::projective(src, h);
        let b = t.bounds(src).unwrap();
        let out = Grid::covering(b, 1.0);
        let source = Source {
            image: &image,
            origin: [0.0, 0.0],
            px: 1.0,
        };
        let result = transform_rgba(source, &t, out, Filter::Bicubic);
        let at = |x: f64, y: f64| {
            *result.get_pixel((x - out.x as f64) as u32, (y - out.y as f64) as u32)
        };
        assert_eq!(at(60.0, 45.0).0, [200, 30, 30, 255]);
        assert_eq!(at(0.0, 5.0).0[3], 0, "outside the quad");
        // Premultiplied: the half-covered edge pixels keep the color.
        for p in result.pixels().filter(|p| p.0[3] > 20) {
            assert!(p.0[0] >= 195 && p.0[1] <= 35, "{p:?}");
        }
    }

    #[test]
    fn shrinking_averages_instead_of_aliasing() {
        // Stripes one pixel wide, made 8 times narrower: gray.
        let image = RgbaImage::from_fn(64, 8, |x, _| {
            let v = if x % 2 == 0 { 255 } else { 0 };
            Rgba([v, v, v, 255])
        });
        let src = [0.0, 0.0, 64.0, 8.0];
        let h = Homography::scale([0.0, 0.0], 0.125, 1.0);
        let t = Transform::projective(src, h);
        let source = Source {
            image: &image,
            origin: [0.0, 0.0],
            px: 1.0,
        };
        let out = Grid {
            x: 0,
            y: 0,
            w: 8,
            h: 8,
            px: 1.0,
        };
        let small = transform_rgba(source, &t, out, Filter::Lanczos);
        for p in small.pixels() {
            assert!(p.0[0].abs_diff(128) < 20, "{p:?}");
        }
    }

    #[test]
    fn masks_get_their_default_outside() {
        let mask = GrayImage::from_pixel(10, 10, Luma([0]));
        let src = [0.0, 0.0, 10.0, 10.0];
        let t = Transform::projective(src, Homography::affine([1.0, 0.0, 20.0, 0.0, 1.0, 0.0]));
        let source = Source {
            image: &mask,
            origin: [0.0, 0.0],
            px: 1.0,
        };
        let out = Grid {
            x: 0,
            y: 0,
            w: 40,
            h: 10,
            px: 1.0,
        };
        let moved = transform_gray(source, &t, out, Filter::Bilinear, 255);
        assert_eq!(moved.get_pixel(5, 5).0, [255]);
        assert_eq!(moved.get_pixel(25, 5).0, [0]);
    }

    #[test]
    fn warp_renders_its_grid() {
        let image = checker(30, 20);
        let src = [0.0, 0.0, 30.0, 20.0];
        let unit = Homography::rect_to_quad(
            [0.0, 0.0, 1.0, 1.0],
            [[0.0, 0.0], [30.0, 0.0], [30.0, 20.0], [0.0, 20.0]],
        )
        .unwrap();
        let identity = Warp::from_map(&unit).unwrap();
        let t = Transform {
            src,
            map: Map::Warp(identity),
        };
        let source = Source {
            image: &image,
            origin: [0.0, 0.0],
            px: 1.0,
        };
        let out = Grid {
            x: 0,
            y: 0,
            w: 30,
            h: 20,
            px: 1.0,
        };
        let same = transform_rgba(source, &t, out, Filter::Bilinear);
        let diff = same
            .pixels()
            .zip(image.pixels())
            .map(|(a, b)| (0..4).map(|c| a.0[c].abs_diff(b.0[c])).max().unwrap())
            .max()
            .unwrap();
        assert!(diff <= 1, "{diff}");
        // Arched: the top middle goes up, the bottom corners stay.
        let arch = WarpStyle::Arch
            .grid(0.5, false, 20.0 / 30.0)
            .mapped(|p| unit.apply(p).unwrap());
        let t = Transform {
            src,
            map: Map::Warp(arch),
        };
        let b = t.bounds(src).unwrap();
        assert!(b[1] < -4.5, "{b:?}");
        let out = Grid::covering(b, 1.0);
        let bent = transform_rgba(source, &t, out, Filter::Bilinear);
        let at = |x: i64, y: i64| bent.get_pixel((x - out.x) as u32, (y - out.y) as u32).0[3];
        assert_eq!(at(15, b[1] as i64 + 2), 255);
        assert_eq!(at(1, 18), 255);
        assert_eq!(at(15, 18), 0, "the middle of the bottom went up");
    }
}
