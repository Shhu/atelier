//! Filter › Liquify: a mesh of offsets pushed around by brushes (Forward
//! Warp, Reconstruct, Smooth, Twirl, Pucker, Bloat, Push Left), a freeze
//! mask protecting parts of it, and the image resampled through the mesh
//! (premultiplied, multithreaded).
//!
//! Like Photoshop, the distortion is a mesh rather than an offset per
//! pixel: a node every few pixels (every pixel for small images, every 4
//! for 6000 × 4000), the offsets in between interpolated bilinearly. A dab
//! then touches `spacing²` times fewer values, the mesh of a large image
//! takes a few megabytes instead of hundreds, and a saved mesh stays small;
//! the brushes being much wider than a node, nothing visible is lost.
//!
//! Positions are in "pixel space": pixel (x, y) is the point (x, y), node
//! (i, j) the point (i·spacing, j·spacing). The result at `p` is the source
//! at `p + offset(p)`: brushes compose a new map with the current one, so
//! that strokes add up without resampling the image again.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

use image::RgbaImage;

use crate::filters::par_rows;
use crate::retouch::Progress;

/// Most nodes of a mesh, and along a side: its spacing doubles until it
/// has fewer (the freeze mask is shown as a texture of a texel per node).
const MAX_NODES: usize = 1 << 21;
const MAX_SIDE_NODES: usize = 4096;
/// Nodes along each side of the tiles saved for undo.
const TILE: usize = 64;
/// Steps of a stroke, in brush diameters: small enough for Forward Warp
/// to keep the pixels under the brush's center.
const STEP: f32 = 0.125;
/// Speeds of the tools that work over time (per second, at full pressure
/// and rate, at the brush's center).
const RECONSTRUCT_SPEED: f32 = 4.0;
const SMOOTH_SPEED: f32 = 6.0;
/// Radians per second.
const TWIRL_SPEED: f32 = 3.0;
/// Logarithm of the scale per second.
const PUCKER_SPEED: f32 = 1.0;
/// Header of a saved mesh.
const MAGIC: &[u8; 8] = b"ATLQMESH";
const VERSION: u32 = 1;

/// Nodes needed along `len` pixels.
fn nodes(len: u32, spacing: u32) -> usize {
    (len.max(1) - 1).div_ceil(spacing) as usize + 1
}

/// The distortion of an image: an offset per node.
#[derive(Clone, PartialEq)]
pub struct Mesh {
    width: u32,
    height: u32,
    spacing: u32,
    cols: usize,
    rows: usize,
    /// Row by row: where the result at the node takes its pixel, relative
    /// to the node.
    offsets: Vec<[f32; 2]>,
}

impl std::fmt::Debug for Mesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let moved = self.offsets.iter().filter(|o| **o != [0.0; 2]).count();
        write!(
            f,
            "Mesh({}×{}, every {} px, {moved} nodes moved)",
            self.width, self.height, self.spacing
        )
    }
}

impl Mesh {
    /// No distortion, for an image of `width` × `height`.
    pub fn new(width: u32, height: u32) -> Self {
        let mut spacing = 1;
        while nodes(width, spacing) * nodes(height, spacing) > MAX_NODES
            || nodes(width.max(height), spacing) > MAX_SIDE_NODES
        {
            spacing *= 2;
        }
        Self::with_spacing(width, height, spacing)
    }

    pub fn with_spacing(width: u32, height: u32, spacing: u32) -> Self {
        let spacing = spacing.max(1);
        let (cols, rows) = (nodes(width, spacing), nodes(height, spacing));
        Self {
            width: width.max(1),
            height: height.max(1),
            spacing,
            cols,
            rows,
            offsets: vec![[0.0; 2]; cols * rows],
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn spacing(&self) -> u32 {
        self.spacing
    }

    /// Nodes per row and per column.
    pub fn nodes(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn is_identity(&self) -> bool {
        self.offsets.iter().all(|o| *o == [0.0; 2])
    }

    fn node(&self, i: usize, j: usize) -> [f32; 2] {
        self.offsets[j * self.cols + i]
    }

    /// Position of node (i, j).
    fn node_pos(&self, i: usize, j: usize) -> [f32; 2] {
        let s = self.spacing as f32;
        [i as f32 * s, j as f32 * s]
    }

    /// The offset at point (x, y), interpolated between the nodes around
    /// (beyond the mesh, the nearest edge's).
    pub fn offset_at(&self, x: f32, y: f32) -> [f32; 2] {
        let s = self.spacing as f32;
        let u = (x / s).clamp(0.0, (self.cols - 1) as f32);
        let v = (y / s).clamp(0.0, (self.rows - 1) as f32);
        let (i, j) = (u as usize, v as usize);
        let (i1, j1) = ((i + 1).min(self.cols - 1), (j + 1).min(self.rows - 1));
        let (fx, fy) = (u - i as f32, v - j as f32);
        let [a, b, c, d] = [
            self.node(i, j),
            self.node(i1, j),
            self.node(i, j1),
            self.node(i1, j1),
        ];
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }

    /// Where the source's point `p` shows in the result (to draw the mesh
    /// distorted): the offset map inverted by fixed-point iteration.
    pub fn displaced(&self, p: [f32; 2]) -> [f32; 2] {
        let mut at = p;
        for _ in 0..3 {
            let o = self.offset_at(at[0], at[1]);
            at = [p[0] - o[0], p[1] - o[1]];
        }
        at
    }

    /// The same distortion stretched over an image of another size (a mesh
    /// saved for another image, a Smart Object resized).
    pub fn fitted(&self, width: u32, height: u32) -> Mesh {
        if (width, height) == self.size() {
            return self.clone();
        }
        let mut out = Mesh::new(width, height);
        let kx = self.width as f32 / out.width as f32;
        let ky = self.height as f32 / out.height as f32;
        for j in 0..out.rows {
            for i in 0..out.cols {
                let [x, y] = out.node_pos(i, j);
                let o = self.offset_at((x + 0.5) * kx - 0.5, (y + 0.5) * ky - 0.5);
                out.offsets[j * out.cols + i] = [o[0] / kx, o[1] / ky];
            }
        }
        out
    }

    /// The same distortion in another frame of `width` × `height` whose
    /// corner is at `corner` in this one (a layer enlarged to the canvas):
    /// none outside this frame.
    pub fn reframed(&self, corner: (i64, i64), width: u32, height: u32) -> Mesh {
        let mut out = Mesh::new(width, height);
        let (w, h) = (self.width as f32 - 1.0, self.height as f32 - 1.0);
        for j in 0..out.rows {
            for i in 0..out.cols {
                let [x, y] = out.node_pos(i, j);
                let (x, y) = (x + corner.0 as f32, y + corner.1 as f32);
                if (0.0..=w).contains(&x) && (0.0..=h).contains(&y) {
                    out.offsets[j * out.cols + i] = self.offset_at(x, y);
                }
            }
        }
        out
    }

    /// The mesh as a file: a header, then the offsets compressed.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for v in [VERSION, self.width, self.height, self.spacing] {
            out.extend(v.to_le_bytes());
        }
        let raw: Vec<u8> = self
            .offsets
            .iter()
            .flat_map(|o| o.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let mut z = flate2::write::DeflateEncoder::new(out, flate2::Compression::fast());
        z.write_all(&raw).expect("in memory");
        z.finish().expect("in memory")
    }

    /// A mesh from [`Self::to_bytes`]; `None` if it is not one.
    pub fn from_bytes(bytes: &[u8]) -> Option<Mesh> {
        let header = bytes.get(..MAGIC.len() + 16)?;
        if &header[..MAGIC.len()] != MAGIC {
            return None;
        }
        let field = |k: usize| {
            let at = MAGIC.len() + 4 * k;
            u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes"))
        };
        let (version, width, height, spacing) = (field(0), field(1), field(2), field(3));
        let limit = crate::document::MAX_DIMENSION;
        if version != VERSION
            || !(1..=limit).contains(&width)
            || !(1..=limit).contains(&height)
            || !(1..=limit).contains(&spacing)
        {
            return None;
        }
        let mut mesh = Mesh::with_spacing(width, height, spacing);
        if mesh.offsets.len() > 4 * MAX_NODES {
            return None;
        }
        let mut raw = Vec::with_capacity(mesh.offsets.len() * 8);
        flate2::read::DeflateDecoder::new(&bytes[header.len()..])
            .take(mesh.offsets.len() as u64 * 8 + 1)
            .read_to_end(&mut raw)
            .ok()?;
        if raw.len() != mesh.offsets.len() * 8 {
            return None;
        }
        for (o, chunk) in mesh.offsets.iter_mut().zip(raw.chunks_exact(8)) {
            let x = f32::from_le_bytes(chunk[..4].try_into().expect("4 bytes"));
            let y = f32::from_le_bytes(chunk[4..].try_into().expect("4 bytes"));
            if !x.is_finite() || !y.is_finite() {
                return None;
            }
            *o = [x, y];
        }
        Some(mesh)
    }
}

/// What a dab does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Pushes the pixels along the stroke.
    Warp,
    /// Brings the distortion back toward the original.
    Reconstruct,
    /// Evens out the distortion.
    Smooth,
    /// Rotates the pixels under the brush: clockwise (`true`) or not.
    Twirl(bool),
    /// Draws the pixels toward the brush's center.
    Pucker,
    /// Pushes them away from it.
    Bloat,
    /// Moves the pixels perpendicular to the stroke: left of its direction
    /// (`true`), or right.
    Push(bool),
    Freeze,
    Thaw,
}

impl Effect {
    /// Works as the brush moves (else also over time, while held).
    fn follows_motion(self) -> bool {
        matches!(self, Effect::Warp | Effect::Push(_))
    }
}

/// Brush Tool Options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    /// Diameter, in pixels.
    pub size: f32,
    /// How far from the center the effect stays strong (0..1).
    pub density: f32,
    /// 0..1.
    pub pressure: f32,
    /// Speed of the tools that work while held still (0..1).
    pub rate: f32,
    /// The image's edges stay in place, nothing transparent comes in.
    pub pin_edges: bool,
}

impl Default for Brush {
    /// Photoshop's.
    fn default() -> Self {
        Self {
            size: 100.0,
            density: 0.5,
            pressure: 1.0,
            rate: 0.8,
            pin_edges: false,
        }
    }
}

impl Brush {
    /// Strength at `t` (distance from the center over the radius): full in
    /// a core that grows with the density, fading smoothly to the edge.
    fn falloff(&self, t: f32) -> f32 {
        if t >= 1.0 {
            return 0.0;
        }
        let core = 0.8 * self.density.clamp(0.0, 1.0);
        let x = ((t - core) / (1.0 - core)).clamp(0.0, 1.0);
        1.0 - x * x * (3.0 - 2.0 * x)
    }

    fn radius(&self) -> f32 {
        (self.size / 2.0).max(0.5)
    }

    /// Multiplier of the speeds over time: some effect even at rate 0.
    fn rate_factor(&self) -> f32 {
        0.1 + 0.9 * self.rate.clamp(0.0, 1.0)
    }
}

/// Nodes x0..x1 × y0..y1 (exclusive).
pub type NodeRect = [usize; 4];

fn union(a: Option<NodeRect>, b: Option<NodeRect>) -> Option<NodeRect> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            a[0].min(b[0]),
            a[1].min(b[1]),
            a[2].max(b[2]),
            a[3].max(b[3]),
        ]),
        (a, b) => a.or(b),
    }
}

/// Tiles of a mesh and its freeze mask, as they were (undo, redo).
pub struct Snapshot {
    tiles: Vec<Tile>,
}

struct Tile {
    index: usize,
    offsets: Vec<[f32; 2]>,
    freeze: Vec<f32>,
}

/// Liquify being edited: the mesh and the freeze mask.
#[derive(Clone)]
pub struct Liquify {
    pub mesh: Mesh,
    /// Per node, 0 (thawed) to 1 (frozen): frozen nodes don't move.
    freeze: Vec<f32>,
    /// The stroke being painted: the tiles it changed, as they were.
    stroke: Option<(Vec<bool>, Vec<usize>)>,
    backup: Option<(Vec<[f32; 2]>, Vec<f32>)>,
}

impl Liquify {
    pub fn new(mesh: Mesh) -> Self {
        let freeze = vec![0.0; mesh.offsets.len()];
        Self {
            mesh,
            freeze,
            stroke: None,
            backup: None,
        }
    }

    /// Freeze value of each node, row by row.
    pub fn freeze(&self) -> &[f32] {
        &self.freeze
    }

    /// The freeze mask set from `coverage` (0..1) at each node's position.
    pub fn freeze_from(&mut self, coverage: impl Fn(f32, f32) -> f32) {
        let cols = self.mesh.cols;
        for (n, f) in self.freeze.iter_mut().enumerate() {
            let [x, y] = self.mesh.node_pos(n % cols, n / cols);
            *f = coverage(x, y).clamp(0.0, 1.0);
        }
    }

    /// Mask Options: None (`false`) or Mask All.
    pub fn mask_all(&mut self, frozen: bool) {
        self.freeze.fill(if frozen { 1.0 } else { 0.0 });
    }

    pub fn invert_mask(&mut self) {
        self.freeze.iter_mut().for_each(|f| *f = 1.0 - *f);
    }

    /// Restore All: no distortion at all, frozen areas included.
    pub fn restore_all(&mut self) {
        self.mesh.offsets.fill([0.0; 2]);
    }

    /// Reconstruct…: `base`'s distortion kept at `amount` (0..1) outside
    /// the frozen areas.
    pub fn revert(&mut self, base: &Mesh, amount: f32) {
        for ((o, b), f) in self
            .mesh
            .offsets
            .iter_mut()
            .zip(&base.offsets)
            .zip(&self.freeze)
        {
            let k = f + (1.0 - f) * amount;
            *o = [b[0] * k, b[1] * k];
        }
    }

    // ------------------------------------------------------------ undo

    fn tiles(&self) -> (usize, usize) {
        (self.mesh.cols.div_ceil(TILE), self.mesh.rows.div_ceil(TILE))
    }

    fn tile_nodes(&self, index: usize) -> NodeRect {
        let (tx, ty) = (index % self.tiles().0, index / self.tiles().0);
        [
            tx * TILE,
            ty * TILE,
            ((tx + 1) * TILE).min(self.mesh.cols),
            ((ty + 1) * TILE).min(self.mesh.rows),
        ]
    }

    fn save_tile(&self, index: usize) -> Tile {
        self.tile_of(index, &self.mesh.offsets, &self.freeze)
    }

    /// Tile `index` of `offsets` and `freeze` (this mesh's or a copy).
    fn tile_of(&self, index: usize, offsets: &[[f32; 2]], freeze: &[f32]) -> Tile {
        let [x0, y0, x1, y1] = self.tile_nodes(index);
        let mut tile = Tile {
            index,
            offsets: Vec::with_capacity((x1 - x0) * (y1 - y0)),
            freeze: Vec::with_capacity((x1 - x0) * (y1 - y0)),
        };
        for j in y0..y1 {
            let row = j * self.mesh.cols;
            tile.offsets.extend_from_slice(&offsets[row + x0..row + x1]);
            tile.freeze.extend_from_slice(&freeze[row + x0..row + x1]);
        }
        tile
    }

    fn load_tile(&mut self, tile: &Tile) {
        let [x0, y0, x1, y1] = self.tile_nodes(tile.index);
        let w = x1 - x0;
        for j in y0..y1 {
            let row = j * self.mesh.cols;
            let k = (j - y0) * w;
            self.mesh.offsets[row + x0..row + x1].copy_from_slice(&tile.offsets[k..k + w]);
            self.freeze[row + x0..row + x1].copy_from_slice(&tile.freeze[k..k + w]);
        }
    }

    /// Everything as it is now.
    pub fn snapshot(&self) -> Snapshot {
        let (tx, ty) = self.tiles();
        Snapshot {
            tiles: (0..tx * ty).map(|i| self.save_tile(i)).collect(),
        }
    }

    /// Puts back what `snapshot` saved; returns what it replaced (redo).
    pub fn restore(&mut self, snapshot: Snapshot) -> Snapshot {
        let current = snapshot
            .tiles
            .iter()
            .map(|t| self.save_tile(t.index))
            .collect();
        for tile in &snapshot.tiles {
            self.load_tile(tile);
        }
        Snapshot { tiles: current }
    }

    /// The nodes `snapshot` holds.
    pub fn extent(&self, snapshot: &Snapshot) -> Option<NodeRect> {
        snapshot
            .tiles
            .iter()
            .map(|t| Some(self.tile_nodes(t.index)))
            .fold(None, union)
    }

    /// A stroke starts: what it changes is saved as it goes.
    pub fn begin_stroke(&mut self) {
        let (tx, ty) = self.tiles();
        self.stroke = Some((vec![false; tx * ty], Vec::new()));
        self.backup = Some((self.mesh.offsets.clone(), self.freeze.clone()));
    }

    /// The stroke is over: what it changed, as it was (`None`: nothing).
    pub fn end_stroke(&mut self) -> Option<Snapshot> {
        let (_, touched) = self.stroke.take()?;
        let (offsets, freeze) = self.backup.take()?;
        let tiles: Vec<Tile> = touched
            .into_iter()
            .map(|i| self.tile_of(i, &offsets, &freeze))
            .collect();
        (!tiles.is_empty()).then_some(Snapshot { tiles })
    }

    fn touch(&mut self, [x0, y0, x1, y1]: NodeRect) {
        let tx = self.tiles().0;
        let Some((saved, touched)) = &mut self.stroke else {
            return;
        };
        for ty in y0 / TILE..y1.div_ceil(TILE) {
            for t in x0 / TILE..x1.div_ceil(TILE) {
                let i = ty * tx + t;
                if !saved[i] {
                    saved[i] = true;
                    touched.push(i);
                }
            }
        }
    }

    // ---------------------------------------------------------- brushes

    /// The brush moved from `from` to `to` in `dt` seconds (both the same
    /// at the press): dabs along the way. `strength` (0..1) multiplies the
    /// pressure (a pen's). Returns the nodes changed.
    pub fn stroke(
        &mut self,
        effect: Effect,
        brush: &Brush,
        from: [f32; 2],
        to: [f32; 2],
        dt: f32,
        strength: f32,
    ) -> Option<NodeRect> {
        let v = [to[0] - from[0], to[1] - from[1]];
        let len = v[0].hypot(v[1]);
        if effect.follows_motion() && len == 0.0 {
            return None;
        }
        let n = ((len / (brush.size * STEP).max(0.5)).ceil() as usize).max(1);
        let step = [v[0] / n as f32, v[1] / n as f32];
        let mut dirty = None;
        for k in 0..n {
            let at = |k: usize| [from[0] + step[0] * k as f32, from[1] + step[1] * k as f32];
            // The pixels under the brush where the step starts move with it.
            let center = if effect.follows_motion() {
                at(k)
            } else {
                at(k + 1)
            };
            let changed = self.dab(effect, brush, center, step, dt / n as f32, strength);
            dirty = union(dirty, changed);
        }
        dirty
    }

    /// Nodes within `radius` of `c`.
    fn nodes_around(&self, c: [f32; 2], radius: f32) -> Option<NodeRect> {
        let s = self.mesh.spacing as f32;
        let lo = |v: f32| ((v - radius) / s).ceil().max(0.0) as usize;
        let hi =
            |v: f32, n: usize| (((v + radius) / s).floor() + 1.0).clamp(0.0, n as f32) as usize;
        let r = [
            lo(c[0]),
            lo(c[1]),
            hi(c[0], self.mesh.cols),
            hi(c[1], self.mesh.rows),
        ];
        (r[0] < r[2] && r[1] < r[3]).then_some(r)
    }

    /// One dab at `c`: `motion` for the effects that follow it, `dt` for
    /// the others.
    fn dab(
        &mut self,
        effect: Effect,
        brush: &Brush,
        c: [f32; 2],
        motion: [f32; 2],
        dt: f32,
        strength: f32,
    ) -> Option<NodeRect> {
        let radius = brush.radius();
        let rect = self.nodes_around(c, radius)?;
        self.touch(rect);
        let strength = (brush.pressure * strength).clamp(0.0, 1.0);
        let [x0, y0, x1, y1] = rect;
        let cols = self.mesh.cols;
        let (last_col, last_row) = (cols - 1, self.mesh.rows - 1);
        // Strength at each node, the freeze mask and pinned edges counted.
        let weight = |this: &Self, i: usize, j: usize| {
            let [x, y] = this.mesh.node_pos(i, j);
            let t = (x - c[0]).hypot(y - c[1]) / radius;
            let w = strength * brush.falloff(t);
            if matches!(effect, Effect::Freeze | Effect::Thaw) {
                return w;
            }
            let pinned = brush.pin_edges && (i == 0 || j == 0 || i == last_col || j == last_row);
            if pinned {
                0.0
            } else {
                w * (1.0 - this.freeze[j * cols + i])
            }
        };
        let k = brush.rate_factor() * dt;
        match effect {
            Effect::Freeze | Effect::Thaw => {
                for j in y0..y1 {
                    for i in x0..x1 {
                        let w = weight(self, i, j);
                        let f = &mut self.freeze[j * cols + i];
                        *f = if effect == Effect::Freeze {
                            f.max(w)
                        } else {
                            f.min(1.0 - w)
                        };
                    }
                }
            }
            Effect::Reconstruct => {
                for j in y0..y1 {
                    for i in x0..x1 {
                        let keep = (-RECONSTRUCT_SPEED * k * weight(self, i, j)).exp();
                        let o = &mut self.mesh.offsets[j * cols + i];
                        *o = [o[0] * keep, o[1] * keep];
                    }
                }
            }
            Effect::Smooth => {
                let r = ((radius / self.mesh.spacing as f32 / 6.0).round() as usize).max(1);
                let average = self.box_average(rect, r);
                let w = x1 - x0;
                for j in y0..y1 {
                    for i in x0..x1 {
                        let t = 1.0 - (-SMOOTH_SPEED * k * weight(self, i, j)).exp();
                        let a = average[(j - y0) * w + i - x0];
                        let o = &mut self.mesh.offsets[j * cols + i];
                        *o = [o[0] + (a[0] - o[0]) * t, o[1] + (a[1] - o[1]) * t];
                    }
                }
            }
            _ => {
                // Where the result at each node now takes its pixels from,
                // in the current result: composed with the current map.
                let mut new = Vec::with_capacity((x1 - x0) * (y1 - y0));
                let (max_x, max_y) = (self.mesh.width as f32 - 1.0, self.mesh.height as f32 - 1.0);
                for j in y0..y1 {
                    for i in x0..x1 {
                        let p = self.mesh.node_pos(i, j);
                        let w = weight(self, i, j);
                        let old = self.mesh.node(i, j);
                        if w <= 0.0 {
                            new.push(old);
                            continue;
                        }
                        let q = source_point(effect, p, c, motion, w * k, w);
                        let o = self.mesh.offset_at(q[0], q[1]);
                        let mut to = [q[0] + o[0], q[1] + o[1]];
                        if brush.pin_edges {
                            to = [to[0].clamp(0.0, max_x), to[1].clamp(0.0, max_y)];
                        }
                        new.push([to[0] - p[0], to[1] - p[1]]);
                    }
                }
                let w = x1 - x0;
                for (row, j) in new.chunks_exact(w).zip(y0..y1) {
                    self.mesh.offsets[j * cols + x0..j * cols + x1].copy_from_slice(row);
                }
            }
        }
        Some(rect)
    }

    /// Average offset over (2r + 1)² nodes around each node of `rect`
    /// (separable box, the edge nodes repeated).
    fn box_average(&self, [x0, y0, x1, y1]: NodeRect, r: usize) -> Vec<[f32; 2]> {
        let (cols, rows) = (self.mesh.cols as isize, self.mesh.rows as isize);
        let r = r as isize;
        let (ya, yb) = (y0 as isize - r, y1 as isize + r);
        let w = x1 - x0;
        let n = (2 * r + 1) as f32;
        // Horizontal pass over the rows needed by the vertical one.
        let mut horizontal = Vec::with_capacity(w * (yb - ya) as usize);
        for j in ya..yb {
            let j = j.clamp(0, rows - 1) as usize;
            for i in x0..x1 {
                let mut sum = [0.0; 2];
                for di in -r..=r {
                    let o = self
                        .mesh
                        .node((i as isize + di).clamp(0, cols - 1) as usize, j);
                    sum = [sum[0] + o[0], sum[1] + o[1]];
                }
                horizontal.push([sum[0] / n, sum[1] / n]);
            }
        }
        let mut out = Vec::with_capacity(w * (y1 - y0));
        for j in 0..y1 - y0 {
            for i in 0..w {
                let mut sum = [0.0; 2];
                for dj in 0..(2 * r + 1) as usize {
                    let o = horizontal[(j + dj) * w + i];
                    sum = [sum[0] + o[0], sum[1] + o[1]];
                }
                out.push([sum[0] / n, sum[1] / n]);
            }
        }
        out
    }
}

/// Where the pixel shown at `p` comes from after a dab of `effect` at `c`
/// (in the result before it): `w` the dab's strength there, `amount` that
/// times the time and rate (the effects over time).
fn source_point(
    effect: Effect,
    p: [f32; 2],
    c: [f32; 2],
    motion: [f32; 2],
    amount: f32,
    w: f32,
) -> [f32; 2] {
    let d = [p[0] - c[0], p[1] - c[1]];
    match effect {
        Effect::Warp => [p[0] - w * motion[0], p[1] - w * motion[1]],
        Effect::Push(left) => {
            // Left of the direction of travel, on a screen (y down).
            let side = if left { 1.0 } else { -1.0 };
            let m = [motion[1] * side, -motion[0] * side];
            [p[0] - w * m[0], p[1] - w * m[1]]
        }
        Effect::Twirl(clockwise) => {
            // Clockwise on a screen: the pixels come from counterclockwise.
            let a = TWIRL_SPEED * amount * if clockwise { -1.0 } else { 1.0 };
            let (sin, cos) = a.sin_cos();
            [
                c[0] + d[0] * cos - d[1] * sin,
                c[1] + d[0] * sin + d[1] * cos,
            ]
        }
        Effect::Pucker | Effect::Bloat => {
            let sign = if effect == Effect::Pucker { 1.0 } else { -1.0 };
            let m = (sign * PUCKER_SPEED * amount).exp();
            [c[0] + d[0] * m, c[1] + d[1] * m]
        }
        Effect::Reconstruct | Effect::Smooth | Effect::Freeze | Effect::Thaw => p,
    }
}

// ------------------------------------------------------------ rendering

/// Premultiplied value of pixel (x, y); transparent outside the image.
#[inline]
fn texel(image: &RgbaImage, x: i64, y: i64) -> [f32; 4] {
    let (w, h) = image.dimensions();
    if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
        return [0.0; 4];
    }
    let p = &image.as_raw()[(y as usize * w as usize + x as usize) * 4..][..4];
    let a = p[3] as f32 / 255.0;
    [
        p[0] as f32 * a,
        p[1] as f32 * a,
        p[2] as f32 * a,
        p[3] as f32,
    ]
}

/// The image at point (x, y), between its 4 nearest pixels.
fn bilinear(image: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let [a, b, c, d] = [
        texel(image, x0, y0),
        texel(image, x0 + 1, y0),
        texel(image, x0, y0 + 1),
        texel(image, x0 + 1, y0 + 1),
    ];
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// Catmull-Rom weights of the 4 pixels around a point `t` past the second.
fn cubic_weights(t: f32) -> [f32; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [
        (-t3 + 2.0 * t2 - t) / 2.0,
        (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0,
        (-3.0 * t3 + 4.0 * t2 + t) / 2.0,
        (t3 - t2) / 2.0,
    ]
}

/// The image at point (x, y), from its 16 nearest pixels (sharper). The
/// edge pixels repeat for the taps beyond them; past the last pixel, the
/// image fades out over a pixel, as between two pixels.
fn bicubic(image: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
    let (w, h) = image.dimensions();
    let inside = |v: f32, n: u32| (v + 1.0).min(n as f32 - v).clamp(0.0, 1.0);
    let coverage = inside(x, w) * inside(y, h);
    if coverage == 0.0 {
        return [0.0; 4];
    }
    let (x0, y0) = (x.floor(), y.floor());
    let (wx, wy) = (cubic_weights(x - x0), cubic_weights(y - y0));
    let (x0, y0) = (x0 as i64 - 1, y0 as i64 - 1);
    let clamp = |v: i64, n: u32| v.clamp(0, n as i64 - 1);
    let mut acc = [0.0; 4];
    for (j, wy) in wy.iter().enumerate() {
        let mut row = [0.0; 4];
        for (i, wx) in wx.iter().enumerate() {
            let p = texel(image, clamp(x0 + i as i64, w), clamp(y0 + j as i64, h));
            for c in 0..4 {
                row[c] += wx * p[c];
            }
        }
        for c in 0..4 {
            acc[c] += wy * row[c];
        }
    }
    acc.map(|v| v * coverage)
}

/// Writes a premultiplied value as straight colors (a ringing filter's
/// overshoot clamped: no color brighter than its alpha allows).
fn store(px: &mut [u8], v: [f32; 4]) {
    let a = v[3].clamp(0.0, 255.0);
    if a < 0.5 {
        px.fill(0);
        return;
    }
    for c in 0..3 {
        px[c] = (v[c].clamp(0.0, a) * 255.0 / a).round() as u8;
    }
    px[3] = a.round() as u8;
}

/// `source` distorted by `mesh` (stretched to it if made for another
/// size), bicubic, on all cores; `None` if cancelled.
pub fn render(source: &RgbaImage, mesh: &Mesh, progress: &Progress) -> Option<RgbaImage> {
    let (w, h) = source.dimensions();
    let fitted;
    let mesh = if mesh.size() == (w, h) {
        mesh
    } else {
        fitted = mesh.fitted(w, h);
        &fitted
    };
    let mut out = source.clone();
    if mesh.is_identity() || w == 0 || h == 0 {
        return Some(out);
    }
    let done = AtomicUsize::new(0);
    let row = w as usize * 4;
    par_rows(out.as_mut(), row, |first, band| {
        for (k, line) in band.chunks_exact_mut(row).enumerate() {
            if progress.cancelled() {
                return;
            }
            let y = (first + k) as f32;
            for x in 0..w as usize {
                let o = mesh.offset_at(x as f32, y);
                // Not moved: already the source's pixel.
                if o != [0.0; 2] {
                    store(
                        &mut line[x * 4..][..4],
                        bicubic(source, x as f32 + o[0], y + o[1]),
                    );
                }
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            progress.set(n as f32 / h as f32);
        }
    });
    (!progress.cancelled()).then_some(out)
}

/// A view of the result: texel (i, j) shows point `origin + (i, j)·step`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin: [f32; 2],
    pub step: f32,
}

/// Texels `rect` (x0, y0, x1, y1) of `frame`, as premultiplied RGBA rows,
/// bilinear from `source` whose pixels are `factor` pixels wide (a reduced
/// copy of the image); without `mesh`, the original.
pub fn render_preview(
    source: &RgbaImage,
    factor: u32,
    mesh: Option<&Mesh>,
    frame: Frame,
    [x0, y0, x1, y1]: [u32; 4],
) -> Vec<u8> {
    let w = x1.saturating_sub(x0) as usize;
    let h = y1.saturating_sub(y0) as usize;
    let mut out = vec![0u8; w * h * 4];
    if w == 0 {
        return out;
    }
    let f = factor.max(1) as f32;
    par_rows(&mut out, w * 4, |first, band| {
        for (k, line) in band.chunks_exact_mut(w * 4).enumerate() {
            let y = frame.origin[1] + (y0 as usize + first + k) as f32 * frame.step;
            for (i, px) in line.chunks_exact_mut(4).enumerate() {
                let x = frame.origin[0] + (x0 as usize + i) as f32 * frame.step;
                let o = mesh.map_or([0.0; 2], |m| m.offset_at(x, y));
                // The reduced source's pixel n is centered on (n + 0.5)·f − 0.5.
                let v = bilinear(
                    source,
                    (x + o[0] + 0.5) / f - 0.5,
                    (y + o[1] + 0.5) / f - 0.5,
                );
                for c in 0..4 {
                    px[c] = v[c].round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Black, with a white 5 × 5 square centered on (`x`, `y`).
    fn dot(w: u32, h: u32, x: u32, y: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |px, py| {
            if px.abs_diff(x) <= 2 && py.abs_diff(y) <= 2 {
                image::Rgba([255, 255, 255, 255])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        })
    }

    /// Center of the bright pixels.
    fn centroid(image: &RgbaImage) -> (f32, f32) {
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for (x, y, p) in image.enumerate_pixels() {
            let v = p.0[0] as f32;
            sx += x as f32 * v;
            sy += y as f32 * v;
            n += v;
        }
        (sx / n, sy / n)
    }

    fn rendered(image: &RgbaImage, mesh: &Mesh) -> RgbaImage {
        render(image, mesh, &Progress::default()).expect("not cancelled")
    }

    fn gradient(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([(x * 3) as u8, (y * 5) as u8, ((x + y) * 2) as u8, 200])
        })
    }

    #[test]
    fn the_mesh_spacing_keeps_large_images_light() {
        assert_eq!(Mesh::new(1920, 1080).spacing(), 1);
        let big = Mesh::new(6000, 4000);
        assert_eq!(big.spacing(), 4);
        let (cols, rows) = big.nodes();
        assert_eq!((cols, rows), (1501, 1001));
        assert!(cols * rows <= MAX_NODES);
    }

    #[test]
    fn an_identity_mesh_gives_the_same_image() {
        let image = gradient(40, 30);
        let mesh = Mesh::new(40, 30);
        assert_eq!(rendered(&image, &mesh), image);
        let preview = render_preview(
            &image,
            1,
            Some(&mesh),
            Frame {
                origin: [0.0; 2],
                step: 1.0,
            },
            [0, 0, 40, 30],
        );
        let premultiplied: Vec<u8> = image
            .pixels()
            .flat_map(|p| {
                let a = p.0[3] as f32 / 255.0;
                [0, 1, 2]
                    .map(|c| (p.0[c] as f32 * a).round() as u8)
                    .into_iter()
                    .chain([p.0[3]])
            })
            .collect();
        assert_eq!(preview, premultiplied);
    }

    #[test]
    fn forward_warp_moves_what_is_under_the_brush_with_it() {
        let image = dot(200, 200, 80, 100);
        let mut liquify = Liquify::new(Mesh::new(200, 200));
        let brush = Brush {
            size: 60.0,
            ..Brush::default()
        };
        // Dragged 25 px right, in a few moves as the pointer reports them.
        let mut at = [80.0, 100.0];
        for _ in 0..5 {
            let to = [at[0] + 5.0, at[1]];
            liquify.stroke(Effect::Warp, &brush, at, to, 0.016, 1.0);
            at = to;
        }
        let (x, y) = centroid(&rendered(&image, &liquify.mesh));
        assert!((x - 105.0).abs() < 1.0, "{x}");
        assert!((y - 100.0).abs() < 0.5, "{y}");
        // Half the pressure: about half as far.
        let mut liquify = Liquify::new(Mesh::new(200, 200));
        let soft = Brush {
            pressure: 0.5,
            ..brush
        };
        liquify.stroke(Effect::Warp, &soft, [80.0, 100.0], [90.0, 100.0], 0.0, 1.0);
        let (x, _) = centroid(&rendered(&image, &liquify.mesh));
        assert!(x > 83.0 && x < 87.0, "{x}");
    }

    #[test]
    fn a_coarse_mesh_warps_the_same() {
        let image = dot(200, 200, 80, 100);
        let mut liquify = Liquify::new(Mesh::with_spacing(200, 200, 4));
        let brush = Brush {
            size: 60.0,
            ..Brush::default()
        };
        liquify.stroke(
            Effect::Warp,
            &brush,
            [80.0, 100.0],
            [100.0, 100.0],
            0.0,
            1.0,
        );
        let (x, _) = centroid(&rendered(&image, &liquify.mesh));
        assert!((x - 100.0).abs() < 1.0, "{x}");
    }

    #[test]
    fn the_freeze_mask_keeps_pixels_in_place() {
        let image = dot(120, 120, 60, 60);
        let brush = Brush {
            size: 80.0,
            ..Brush::default()
        };
        let mut liquify = Liquify::new(Mesh::new(120, 120));
        liquify.mask_all(true);
        liquify.stroke(Effect::Warp, &brush, [60.0, 60.0], [80.0, 60.0], 0.0, 1.0);
        liquify.stroke(Effect::Bloat, &brush, [60.0, 60.0], [60.0, 60.0], 1.0, 1.0);
        assert!(liquify.mesh.is_identity());
        assert_eq!(rendered(&image, &liquify.mesh), image);
        // Frozen on the left half only: the left stays, the right moves.
        liquify.mask_all(false);
        liquify.freeze_from(|x, _| if x < 60.0 { 1.0 } else { 0.0 });
        let image = gradient(120, 120);
        liquify.stroke(Effect::Warp, &brush, [60.0, 60.0], [60.0, 80.0], 0.0, 1.0);
        let out = rendered(&image, &liquify.mesh);
        for y in 0..120 {
            for x in 0..59 {
                assert_eq!(out.get_pixel(x, y), image.get_pixel(x, y), "({x}, {y})");
            }
        }
        assert_ne!(out.get_pixel(70, 60), image.get_pixel(70, 60));
        // Painted with the Freeze Mask tool, then thawed.
        let mut liquify = Liquify::new(Mesh::new(120, 120));
        liquify.stroke(Effect::Freeze, &brush, [60.0, 60.0], [60.0, 60.0], 0.0, 1.0);
        assert_eq!(liquify.freeze()[60 * 120 + 60], 1.0);
        liquify.stroke(Effect::Thaw, &brush, [60.0, 60.0], [60.0, 60.0], 0.0, 1.0);
        assert_eq!(liquify.freeze()[60 * 120 + 60], 0.0);
    }

    #[test]
    fn reconstruct_and_restore_all_go_back_to_the_original() {
        let image = dot(160, 160, 80, 80);
        let brush = Brush {
            size: 80.0,
            ..Brush::default()
        };
        let mut liquify = Liquify::new(Mesh::new(160, 160));
        liquify.stroke(Effect::Warp, &brush, [80.0, 80.0], [95.0, 80.0], 0.0, 1.0);
        let warped = liquify.mesh.clone();
        assert!(centroid(&rendered(&image, &warped)).0 > 90.0);
        // Held still with Reconstruct over the warp.
        for _ in 0..60 {
            liquify.stroke(
                Effect::Reconstruct,
                &brush,
                [90.0, 80.0],
                [90.0, 80.0],
                0.1,
                1.0,
            );
        }
        let (x, _) = centroid(&rendered(&image, &liquify.mesh));
        assert!((x - 80.0).abs() < 0.5, "{x}");
        // Reconstruct…: the distortion scaled; Restore All: none.
        liquify.revert(&warped, 0.5);
        let (x, _) = centroid(&rendered(&image, &liquify.mesh));
        assert!(x > 84.0 && x < 91.0, "{x}");
        liquify.restore_all();
        assert!(liquify.mesh.is_identity());
    }

    #[test]
    fn twirl_pucker_bloat_and_push_move_the_right_way() {
        let image = dot(200, 200, 120, 100);
        let brush = Brush {
            size: 120.0,
            density: 1.0,
            ..Brush::default()
        };
        let run = |effect: Effect, from: [f32; 2], to: [f32; 2], dt: f32| {
            let mut liquify = Liquify::new(Mesh::new(200, 200));
            liquify.stroke(effect, &brush, from, to, dt, 1.0);
            centroid(&rendered(&image, &liquify.mesh))
        };
        let c = [100.0, 100.0];
        // The dot is right of the center: clockwise on screen takes it down.
        assert!(run(Effect::Twirl(true), c, c, 0.2).1 > 102.0);
        assert!(run(Effect::Twirl(false), c, c, 0.2).1 < 98.0);
        assert!(run(Effect::Pucker, c, c, 0.5).0 < 118.0);
        assert!(run(Effect::Bloat, c, c, 0.5).0 > 122.0);
        // Dragging down pushes to the right of the screen... with Push Left
        // the pixels go left of the direction of travel: dragging up, left.
        let (x, _) = run(Effect::Push(true), [120.0, 110.0], [120.0, 90.0], 0.0);
        assert!(x < 110.0, "{x}");
        let (x, _) = run(Effect::Push(false), [120.0, 110.0], [120.0, 90.0], 0.0);
        assert!(x > 130.0, "{x}");
    }

    #[test]
    fn smooth_evens_out_a_distortion() {
        let mut liquify = Liquify::new(Mesh::new(100, 100));
        let small = Brush {
            size: 16.0,
            ..Brush::default()
        };
        liquify.stroke(Effect::Warp, &small, [50.0, 50.0], [56.0, 50.0], 0.0, 1.0);
        let peak = |l: &Liquify| l.mesh.offset_at(50.0, 50.0)[0].abs();
        let before = peak(&liquify);
        let wide = Brush {
            size: 60.0,
            ..Brush::default()
        };
        liquify.stroke(Effect::Smooth, &wide, [50.0, 50.0], [50.0, 50.0], 0.5, 1.0);
        assert!(
            peak(&liquify) < before * 0.8,
            "{} → {}",
            before,
            peak(&liquify)
        );
    }

    #[test]
    fn pin_edges_lets_nothing_transparent_in() {
        let image = gradient(100, 100);
        let brush = Brush {
            size: 80.0,
            pin_edges: true,
            ..Brush::default()
        };
        let mut liquify = Liquify::new(Mesh::new(100, 100));
        liquify.stroke(Effect::Warp, &brush, [5.0, 50.0], [40.0, 50.0], 0.0, 1.0);
        let out = rendered(&image, &liquify.mesh);
        assert!(out.pixels().all(|p| p.0[3] == 200));
        // Without: the warp pulls in what is beyond the edge.
        let free = Brush {
            pin_edges: false,
            ..brush
        };
        let mut liquify = Liquify::new(Mesh::new(100, 100));
        liquify.stroke(Effect::Warp, &free, [5.0, 50.0], [40.0, 50.0], 0.0, 1.0);
        let out = rendered(&image, &liquify.mesh);
        assert!(out.get_pixel(10, 50).0[3] < 200);
    }

    #[test]
    fn a_stroke_undoes_and_redoes() {
        let brush = Brush::default();
        let mut liquify = Liquify::new(Mesh::new(300, 300));
        liquify.begin_stroke();
        liquify.stroke(
            Effect::Warp,
            &brush,
            [150.0, 150.0],
            [170.0, 150.0],
            0.0,
            1.0,
        );
        liquify.stroke(Effect::Freeze, &brush, [40.0, 40.0], [40.0, 40.0], 0.0, 1.0);
        let undo = liquify.end_stroke().expect("changed");
        let after = (liquify.mesh.clone(), liquify.freeze().to_vec());
        let redo = liquify.restore(undo);
        assert!(liquify.mesh.is_identity());
        assert!(liquify.freeze().iter().all(|&f| f == 0.0));
        liquify.restore(redo);
        assert_eq!((liquify.mesh.clone(), liquify.freeze().to_vec()), after);
        // A whole snapshot (Restore All, Load Mesh…).
        let all = liquify.snapshot();
        liquify.restore_all();
        liquify.restore(all);
        assert_eq!(liquify.mesh, after.0);
    }

    #[test]
    fn a_mesh_saves_and_loads() {
        let mut liquify = Liquify::new(Mesh::new(300, 200));
        liquify.stroke(
            Effect::Twirl(true),
            &Brush::default(),
            [150.0, 100.0],
            [150.0, 100.0],
            0.5,
            1.0,
        );
        let bytes = liquify.mesh.to_bytes();
        assert!(
            bytes.len() < 300 * 200 * 8 / 4,
            "compressed: {}",
            bytes.len()
        );
        assert_eq!(Mesh::from_bytes(&bytes), Some(liquify.mesh.clone()));
        assert_eq!(Mesh::from_bytes(b"ATLQMESH"), None);
        assert_eq!(Mesh::from_bytes(&bytes[..bytes.len() - 10]), None);
        assert_eq!(Mesh::from_bytes(b"not a mesh at all, really not"), None);
        // On an image twice as large: the same distortion, twice as large.
        let big = liquify.mesh.fitted(600, 400);
        let a = liquify.mesh.offset_at(170.0, 100.0);
        let b = big.offset_at(340.5, 200.5);
        assert!((b[0] - 2.0 * a[0]).abs() < 0.5 && (b[1] - 2.0 * a[1]).abs() < 0.5);
        // Moved into a larger frame.
        let framed = liquify.mesh.reframed((-10, -20), 400, 300);
        assert_eq!(
            framed.offset_at(180.0, 120.0),
            liquify.mesh.offset_at(170.0, 100.0)
        );
        assert_eq!(framed.offset_at(5.0, 5.0), [0.0; 2]);
    }

    #[test]
    fn the_mesh_is_drawn_where_the_pixels_went() {
        let mut liquify = Liquify::new(Mesh::new(200, 200));
        let brush = Brush {
            size: 100.0,
            ..Brush::default()
        };
        liquify.stroke(
            Effect::Warp,
            &brush,
            [100.0, 100.0],
            [110.0, 100.0],
            0.0,
            1.0,
        );
        let p = liquify.mesh.displaced([100.0, 100.0]);
        assert!(
            (p[0] - 110.0).abs() < 1.0 && (p[1] - 100.0).abs() < 0.5,
            "{p:?}"
        );
    }
}
