//! Edit › Transform, like Photoshop: Free Transform's modes (Scale, Rotate,
//! Skew, Distort, Perspective, Warp) and modifier drags (Ctrl: distort,
//! Ctrl+Shift: skew, Ctrl+Alt+Shift: perspective, Alt: around the reference
//! point), its context menu, Transform Again, the quarter turns and flips;
//! of the selected layers (a group: with its layers), of a mask, or of the
//! selected pixels, lifted from their layer.
//!
//! The turned rectangle of `transform_rect` and `transform_angle` stays the
//! frame of the plain scales and turns (texts stay editable); a distorted
//! frame or a warp grid takes over from it.

use std::cell::Cell;
use std::sync::{Arc, mpsc};

use image::GrayImage;

use super::canvas::{rotate_around, rotate_handle_pos};
use super::*;
use crate::document::{TransformTarget, Transformed, transformed_selection};
use crate::xform::{self, Bounds, Grid, Homography, Map, Pt, Source, Transform, Warp, WarpStyle};

/// Transform Again: Shift+Ctrl+T, like Photoshop.
pub(super) const SHORTCUT_AGAIN: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::T);

/// Edit › Transform commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    /// Free Transform (Ctrl+T).
    Free,
    /// The last transform again, on what is selected now.
    Again,
    /// Free Transform in one of its modes.
    Mode(Mode),
    Rotate180,
    RotateCw,
    RotateCcw,
    FlipHorizontal,
    FlipVertical,
}

impl Command {
    /// The quarter turns and flips, in the menu's order.
    const TURNS: [Command; 5] = [
        Command::Rotate180,
        Command::RotateCw,
        Command::RotateCcw,
        Command::FlipHorizontal,
        Command::FlipVertical,
    ];

    /// Name in the menus and in the history.
    pub(super) fn label(self) -> String {
        match self {
            Command::Free => t!("menu-edit-free-transform"),
            Command::Again => t!("xform-again"),
            Command::Mode(m) => m.label(),
            Command::Rotate180 => t!("xform-rotate-180"),
            Command::RotateCw => t!("optbar-rotate-cw"),
            Command::RotateCcw => t!("optbar-rotate-ccw"),
            Command::FlipHorizontal => t!("xform-flip-horizontal"),
            Command::FlipVertical => t!("xform-flip-vertical"),
        }
    }

    /// The map of a quarter turn or flip around `pivot`.
    fn map(self, pivot: Pt) -> Option<Homography> {
        Some(match self {
            Command::Rotate180 => Homography::rotate(pivot, 180.0),
            Command::RotateCw => Homography::rotate(pivot, 90.0),
            Command::RotateCcw => Homography::rotate(pivot, -90.0),
            Command::FlipHorizontal => Homography::scale(pivot, -1.0, 1.0),
            Command::FlipVertical => Homography::scale(pivot, 1.0, -1.0),
            _ => return None,
        })
    }
}

/// What a plain drag of Free Transform's frame does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Mode {
    #[default]
    Free,
    Scale,
    Rotate,
    Skew,
    Distort,
    Perspective,
    Warp,
}

impl Mode {
    /// The modes of the Transform menu.
    const MENU: [Mode; 6] = [
        Mode::Scale,
        Mode::Rotate,
        Mode::Skew,
        Mode::Distort,
        Mode::Perspective,
        Mode::Warp,
    ];

    fn label(self) -> String {
        match self {
            Mode::Free => t!("menu-edit-free-transform"),
            Mode::Scale => t!("xform-scale"),
            Mode::Rotate => t!("xform-rotate"),
            Mode::Skew => t!("xform-skew"),
            Mode::Distort => t!("xform-distort"),
            Mode::Perspective => t!("xform-perspective"),
            Mode::Warp => t!("xform-warp"),
        }
    }

    /// What dragging a handle does in this mode.
    fn edit(self) -> Option<Edit> {
        match self {
            Mode::Skew => Some(Edit::Skew),
            Mode::Distort => Some(Edit::Distort),
            Mode::Perspective => Some(Edit::Perspective),
            _ => None,
        }
    }
}

/// What a drag of a corner or an edge does to the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edit {
    /// Scales it (from the opposite side, Alt: from the reference point).
    Scale,
    /// The handle follows the pointer (Alt: the opposite one mirrors it).
    Distort,
    /// The handle slides along its edge.
    Skew,
    /// A corner slides along an edge, the other end of it the other way.
    Perspective,
}

/// What is grabbed.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Move,
    /// Corner k of the frame (top-left first, clockwise).
    Corner(usize),
    /// Edge k: from corner k to corner k + 1 (top, right, bottom, left).
    Edge(usize),
    Rotate,
    Reference,
    /// A control point of the warp grid (row, column).
    WarpPoint(usize, usize),
    /// The warp's surface, at (u, v).
    WarpSurface(f64, f64),
}

#[derive(Clone, Copy)]
struct Drag {
    grab: Grab,
    edit: Edit,
    /// Pointer at the start (canvas).
    start: Pt,
    /// The frame, the warp grid and the turned rectangle at the start.
    quad: [Pt; 4],
    warp: Option<Warp>,
    rect: Rect,
    angle: f32,
    /// The reference point (canvas) at the start.
    pivot: Pt,
}

/// Pixels lifted from the selection to be transformed.
struct Lift {
    /// The floating layer.
    id: u64,
    selection: Selection,
    /// History position right after the lift.
    step: u64,
}

/// Above this many pixels, a transform is computed on another thread.
const ASYNC_PIXELS: u64 = 4 << 20;

/// What applying a transform takes (see `finish_transform`).
struct Commit {
    target: TransformTarget,
    /// The layers transformed (a group's listed), those of them whose
    /// pixels are resampled, and the texts that follow the map as texts.
    layers: Vec<usize>,
    pixels: Vec<usize>,
    texts: Vec<usize>,
    text_map: Option<crate::mask_ops::Affine>,
    t: Transform,
    /// History name, the active layer's name, and whether texts become
    /// pixels.
    name: String,
    layer: String,
    rasterized: bool,
    lift: Option<Lift>,
    fit: bool,
    /// The canvas before.
    canvas: PixelRect,
}

/// A transform computed on another thread.
struct Applying {
    rx: mpsc::Receiver<Vec<Transformed>>,
    /// The document it is for.
    doc: u64,
    started: f64,
    commit: Commit,
}

/// The last transform applied, for Transform Again.
// Only one, kept between transforms.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy)]
enum Last {
    /// Canvas → canvas.
    Map(Homography),
    /// A warp of the source box `Bounds`, to fit to the next one.
    Warp(Bounds, Warp),
}

/// A mask (its fingerprint) reduced to a level of the view, for the
/// previews; the canvas position of its image.
struct ReducedMask {
    key: u64,
    level: u32,
    image: Arc<GrayImage>,
    origin: (i64, i64),
}

/// A mask transformed alone, as previewed (see `compose::PlacedMask`).
pub(super) struct MaskPreview {
    layer: u64,
    key: u64,
    image: Arc<GrayImage>,
    at: (i64, i64),
    rect: PixelRect,
}

impl MaskPreview {
    pub(super) fn placed(&self) -> crate::compose::PlacedMask<'_> {
        crate::compose::PlacedMask {
            layer: self.layer,
            version: self.key,
            image: &self.image,
            at: self.at,
            rect: self.rect,
        }
    }
}

/// Free Transform beyond the turned rectangle, and Edit › Transform's
/// memory.
pub(super) struct Transforming {
    mode: Mode,
    /// Distorted frame: where the corners of the source box go (canvas,
    /// top-left first, clockwise).
    quad: Option<[Pt; 4]>,
    /// Warp grid (canvas), the frame it bends (unit square → canvas) and
    /// the distorted frame before it.
    warp: Option<(Warp, Homography, Option<[Pt; 4]>)>,
    style: WarpStyle,
    /// Bend of the style, −100..100 %, and its orientation.
    bend: f32,
    vertical: bool,
    /// Reference point in the source box (0..1); `None`: its center.
    reference: Option<Pt>,
    drag: Option<Drag>,
    lift: Option<Lift>,
    /// History name of the commit (a command applied at once).
    step: Option<String>,
    last: Option<Last>,
    /// Box of the active mask's pixels, for a mask fingerprint.
    mask_bounds: Cell<Option<(u64, Option<PixelRect>)>>,
    pub(super) mask_preview: Option<MaskPreview>,
    reduced_mask: Option<ReducedMask>,
    /// The other layers transformed with the active one, on their own (see
    /// `render::Lifted`), by id.
    lifted: Vec<(u64, render::Lifted)>,
    applying: Option<Applying>,
}

impl Transforming {
    /// The image of layer `id` on its own, if kept (see `render::Lifted`).
    pub(super) fn take_lifted(&mut self, id: u64) -> Option<render::Lifted> {
        let k = self.lifted.iter().position(|(i, _)| *i == id)?;
        Some(self.lifted.swap_remove(k).1)
    }

    pub(super) fn put_lifted(&mut self, id: u64, lifted: Option<render::Lifted>) {
        self.lifted.extend(lifted.map(|l| (id, l)));
    }
}

impl Default for Transforming {
    fn default() -> Self {
        Self {
            mode: Mode::Free,
            quad: None,
            warp: None,
            style: WarpStyle::Custom,
            // Photoshop's default bend.
            bend: 50.0,
            vertical: false,
            reference: None,
            drag: None,
            lift: None,
            step: None,
            last: None,
            mask_bounds: Cell::new(None),
            mask_preview: None,
            reduced_mask: None,
            lifted: Vec::new(),
            applying: None,
        }
    }
}

fn pt(p: Pos2) -> Pt {
    [p.x as f64, p.y as f64]
}

fn pos(p: Pt) -> Pos2 {
    pos2(p[0] as f32, p[1] as f32)
}

fn bounds(r: Rect) -> Bounds {
    [r.min.x, r.min.y, r.max.x, r.max.y].map(f64::from)
}

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Pt, b: Pt) -> Pt {
    [a[0] + b[0], a[1] + b[1]]
}

fn scaled(a: Pt, k: f64) -> Pt {
    [a[0] * k, a[1] * k]
}

fn dot(a: Pt, b: Pt) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn unit(a: Pt) -> Pt {
    let l = dot(a, a).sqrt();
    if l < 1e-12 {
        [0.0, 0.0]
    } else {
        scaled(a, 1.0 / l)
    }
}

/// The source box `src` brought to `rect`, turned by `angle` around its
/// center: the map of the turned rectangle.
fn turned_map(src: Rect, rect: Rect, angle: f32) -> Homography {
    let b = bounds(src);
    let [x0, y0, x1, y1] = bounds(rect);
    let fit = Homography::rect_to_quad(b, [[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
        .unwrap_or(Homography::IDENTITY);
    fit.then(&Homography::rotate(pt(rect.center()), f64::from(angle)))
}

/// Which sides of the box a handle moves: −1 the left (top), 1 the right
/// (bottom), 0 neither.
fn sides(grab: Grab) -> (i8, i8) {
    match grab {
        Grab::Corner(0) => (-1, -1),
        Grab::Corner(1) => (1, -1),
        Grab::Corner(2) => (1, 1),
        Grab::Corner(_) => (-1, 1),
        Grab::Edge(0) => (0, -1),
        Grab::Edge(1) => (1, 0),
        Grab::Edge(2) => (0, 1),
        Grab::Edge(_) => (-1, 0),
        _ => (0, 0),
    }
}

/// Box `b` scaled by a handle moving its `sides` by `d`: from the opposite
/// side (the middle of the other axis for an edge), or from `pivot`; `keep`
/// keeps its proportions.
fn scale_box(b: Bounds, (sx, sy): (i8, i8), d: Pt, keep: bool, pivot: Option<Pt>) -> Bounds {
    let [x0, y0, x1, y1] = b;
    let fixed = |s: i8, lo: f64, hi: f64| match s {
        -1 => hi,
        1 => lo,
        _ => (lo + hi) / 2.0,
    };
    let p = pivot.unwrap_or([fixed(sx, x0, x1), fixed(sy, y0, y1)]);
    let factor = |s: i8, lo: f64, hi: f64, d: f64, p: f64| {
        let edge = if s < 0 { lo } else { hi };
        if s == 0 || (edge - p).abs() < 1e-9 {
            return 1.0;
        }
        // At least a pixel.
        ((edge + d - p) / (edge - p)).max(1.0 / (hi - lo))
    };
    let (mut fx, mut fy) = (
        factor(sx, x0, x1, d[0], p[0]),
        factor(sy, y0, y1, d[1], p[1]),
    );
    if keep {
        let f = match (sx, sy) {
            (0, _) => fy,
            (_, 0) => fx,
            _ if (fx - 1.0).abs() > (fy - 1.0).abs() => fx,
            _ => fy,
        };
        (fx, fy) = (f, f);
    }
    [
        p[0] + (x0 - p[0]) * fx,
        p[1] + (y0 - p[1]) * fy,
        p[0] + (x1 - p[0]) * fx,
        p[1] + (y1 - p[1]) * fy,
    ]
}

/// The frame `q` with its handle `grab` moved by `d` the `edit` way; `alt`:
/// the opposite handle the other way.
fn edit_quad(q: [Pt; 4], grab: Grab, edit: Edit, d: Pt, alt: bool) -> [Pt; 4] {
    let mut out = q;
    let mut shift = |k: usize, v: Pt| out[k % 4] = add(out[k % 4], v);
    match (grab, edit) {
        (Grab::Corner(k), Edit::Distort) => {
            shift(k, d);
            if alt {
                shift(k + 2, scaled(d, -1.0));
            }
        }
        (Grab::Corner(k), Edit::Skew | Edit::Perspective) => {
            // Along the adjacent edge closest to the drag.
            let along = |other: usize| unit(sub(q[k], q[other % 4]));
            let other = [k + 1, k + 3]
                .into_iter()
                .max_by(|&a, &b| dot(d, along(a)).abs().total_cmp(&dot(d, along(b)).abs()))
                .expect("two edges");
            let v = scaled(along(other), dot(d, along(other)));
            shift(k, v);
            if edit == Edit::Perspective {
                shift(other, scaled(v, -1.0));
            } else if alt {
                shift(k + 2, scaled(v, -1.0));
            }
        }
        (Grab::Edge(k), edit) => {
            let v = match edit {
                Edit::Distort => d,
                _ => {
                    let e = unit(sub(q[(k + 1) % 4], q[k]));
                    scaled(e, dot(d, e))
                }
            };
            shift(k, v);
            shift(k + 1, v);
            if alt {
                shift(k + 2, scaled(v, -1.0));
                shift(k + 3, scaled(v, -1.0));
            }
        }
        _ => {}
    }
    out
}

/// Exact frame of a quarter turn or flip of the box `r` (whole pixels)
/// around its center: the corners stay on whole pixels.
fn turned_box(r: PixelRect, cmd: Command) -> Option<[Pt; 4]> {
    let (x, y, w, h) = (r.x, r.y, r.w as i64, r.h as i64);
    let corners = |x: i64, y: i64, w: i64, h: i64| {
        [(x, y), (x + w, y), (x + w, y + h), (x, y + h)].map(|(a, b)| [a as f64, b as f64])
    };
    let [tl, tr, br, bl] = corners(x, y, w, h);
    let turned = || {
        let (nx, ny) = ((2 * x + w - h).div_euclid(2), (2 * y + h - w).div_euclid(2));
        corners(nx, ny, h, w)
    };
    Some(match cmd {
        Command::Rotate180 => [br, bl, tl, tr],
        Command::RotateCw => {
            let [a, b, c, d] = turned();
            [b, c, d, a]
        }
        Command::RotateCcw => {
            let [a, b, c, d] = turned();
            [d, a, b, c]
        }
        Command::FlipHorizontal => [tr, tl, bl, br],
        Command::FlipVertical => [bl, br, tr, tl],
        _ => return None,
    })
}

/// Position of the rotation handle of the frame `q` (screen): beyond the
/// middle of its top edge.
fn knob(q: &[Pos2; 4]) -> Pos2 {
    let mid = q[0].lerp(q[1], 0.5);
    let center = q.iter().fold(Vec2::ZERO, |s, p| s + p.to_vec2()) / 4.0;
    let edge = q[1] - q[0];
    let mut normal = vec2(edge.y, -edge.x).normalized();
    if normal.dot(mid.to_vec2() - center) < 0.0 {
        normal = -normal;
    }
    if !normal.is_finite() {
        normal = vec2(0.0, -1.0);
    }
    mid + normal * ROTATE_HANDLE_DISTANCE
}

/// Whether `p` is inside the polygon `q`.
fn inside(q: &[Pos2], p: Pos2) -> bool {
    let mut odd = false;
    for k in 0..q.len() {
        let (a, b) = (q[k], q[(k + 1) % q.len()]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            odd = !odd;
        }
    }
    odd
}

/// Distance from `p` to the segment `a`–`b`.
fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

impl AtelierApp {
    /// Runs an Edit › Transform command.
    pub(super) fn transform_command(&mut self, cmd: Command) {
        match cmd {
            Command::Free => {
                self.enter_transform();
            }
            Command::Again => self.transform_again(),
            Command::Mode(mode) => self.set_transform_mode(mode),
            _ if self.transform_pending() => self.turn_pending(cmd),
            _ => {
                if !self.enter_transform() {
                    return;
                }
                let Some(src) = self.transform_source() else {
                    return;
                };
                self.xform.quad = turned_box(to_pixel_rect(src), cmd);
                self.xform.step = Some(cmd.label());
                self.apply_transform();
            }
        }
    }

    /// A Free Transform is being prepared.
    pub(super) fn transform_pending(&self) -> bool {
        self.tool == Tool::Transform && self.transform_rect.is_some()
    }

    /// The frame is no longer a turned rectangle: distorted or warped.
    pub(super) fn transform_distorted(&self) -> bool {
        self.xform.quad.is_some() || self.xform.warp.is_some()
    }

    /// Free Transform starts (Ctrl+T): with a selection, its pixels are
    /// lifted from the layer first. Returns `false` if there is nothing to
    /// transform.
    fn enter_transform(&mut self) -> bool {
        if self.doc.is_none() || self.refuse_if_locked() {
            return false;
        }
        self.set_tool(Tool::Transform);
        if self.transform_rect.is_some() {
            return true;
        }
        if !self.lift_selection() {
            return false;
        }
        let Some(src) = self.transform_source() else {
            let name = self.doc.as_ref().map(|d| d.active_layer().name.clone());
            self.status = t!("xform-status-nothing", name = name.unwrap_or_default());
            return false;
        };
        self.transform_rect = Some(src);
        true
    }

    /// The selected pixels of the active layer go into a floating layer
    /// above it, the hole left transparent. Returns `false` if the
    /// selection has nothing to lift.
    fn lift_selection(&mut self) -> bool {
        let Some(doc) = &mut self.doc else {
            return false;
        };
        let Some(selection) = self
            .selection
            .clone()
            .filter(|_| doc.active_layer().paintable() && !doc.mask_targeted())
        else {
            return true;
        };
        let Some(id) = doc.lift_region(&selection, false) else {
            self.status = t!("xform-status-empty");
            return false;
        };
        let step = doc.history_step();
        self.xform.lift = Some(Lift {
            id,
            selection,
            step,
        });
        // Its outline comes back where the pixels go.
        self.selection = None;
        true
    }

    /// Gives up what Free Transform prepared beyond the turned rectangle;
    /// lifted pixels go back as they were.
    pub(super) fn cancel_transform_extras(&mut self) {
        self.cancel_placing();
        if let Some(lift) = self.xform.lift.take() {
            if let Some(doc) = &mut self.doc {
                if doc.history_step() == lift.step {
                    doc.discard_step();
                } else {
                    doc.drop_floating(lift.id);
                }
            }
            self.selection = Some(lift.selection);
        }
        let x = &mut self.xform;
        x.mode = Mode::Free;
        (x.quad, x.warp, x.reference, x.drag, x.step) = (None, None, None, None, None);
        x.mask_preview = None;
    }

    /// The layers a transform changes: the selected ones (a group with its
    /// layers), unlocked; usually the active one alone.
    pub(super) fn transform_targets(&self) -> Vec<usize> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let selected = self.selected_indices();
        if selected.len() < 2 || self.xform.lift.is_some() {
            return vec![doc.active];
        }
        let inside = |i: usize| {
            selected
                .iter()
                .any(|&g| g != i && doc.block(g).contains(&i))
        };
        selected
            .iter()
            .copied()
            .filter(|&i| !doc.layers[i].locked && !inside(i))
            .collect()
    }

    /// Box of what the transform takes when it isn't the active element's:
    /// several layers' (`None` without any), or the mask's pixels when it
    /// is transformed alone (see `Document::transforms_mask`).
    pub(super) fn transform_source_override(&self) -> Option<Option<Rect>> {
        let doc = self.doc.as_ref()?;
        let targets = self.transform_targets();
        if targets.len() > 1 {
            let b = targets
                .iter()
                .filter_map(|&i| doc.element_bounds(i))
                .reduce(PixelRect::union);
            return Some(b.map(to_rect));
        }
        if !doc.transforms_mask() {
            return None;
        }
        let layer = doc.active_layer();
        let key = layer.mask.as_ref().map(|m| m.key(layer.offset))?;
        let cached = self.xform.mask_bounds.get().filter(|(k, _)| *k == key);
        let b = match cached {
            Some((_, b)) => b,
            None => {
                let b = doc.mask_bounds(doc.active);
                self.xform.mask_bounds.set(Some((key, b)));
                b
            }
        };
        Some(b.map(to_rect))
    }

    /// The frame being prepared (canvas corners, top-left first, clockwise).
    fn transform_frame(&self, src: Rect) -> [Pt; 4] {
        if let Some((w, ..)) = &self.xform.warp {
            return w.corners();
        }
        if let Some(q) = self.xform.quad {
            return q;
        }
        let r = self.transform_rect.unwrap_or(src);
        turned_map(src, r, self.transform_angle)
            .corners(bounds(src))
            .unwrap_or_default()
    }

    /// The transform being prepared, from the source box `src`. `applied`:
    /// a turned rectangle as applied, its edges on whole pixels.
    pub(super) fn pending_transform(&self, src: Rect, applied: bool) -> Option<Transform> {
        let b = bounds(src);
        if let Some((w, ..)) = self.xform.warp {
            return Some(Transform {
                src: b,
                map: Map::Warp(w),
            });
        }
        let h = match self.xform.quad {
            Some(q) => Homography::rect_to_quad(b, q)?,
            None => {
                let r = self.transform_rect.unwrap_or(src);
                let r = if applied { round_rect(r) } else { r };
                turned_map(src, r, self.transform_angle)
            }
        };
        Some(Transform::projective(b, h))
    }

    /// The reference point (canvas).
    fn reference_point(&self, src: Rect) -> Pt {
        let [u, v] = self.xform.reference.unwrap_or([0.5, 0.5]);
        if let Some((w, ..)) = &self.xform.warp {
            return w.eval(u, v);
        }
        let b = bounds(src);
        let p = [b[0] + (b[2] - b[0]) * u, b[1] + (b[3] - b[1]) * v];
        self.pending_transform(src, false)
            .and_then(|t| t.forward(p))
            .unwrap_or(p)
    }

    /// Sets the reference point at `p` (canvas).
    fn set_reference(&mut self, src: Rect, p: Pt) {
        let uv = if let Some((w, ..)) = &self.xform.warp {
            let (u, v) = w.locate(p);
            [u, v]
        } else {
            let b = bounds(src);
            let Some(back) = self
                .pending_transform(src, false)
                .and_then(|t| match t.map {
                    Map::Projective(h) => h.inverse(),
                    Map::Warp(_) => None,
                })
                .and_then(|h| h.apply(p))
            else {
                return;
            };
            [
                (back[0] - b[0]) / (b[2] - b[0]),
                (back[1] - b[1]) / (b[3] - b[1]),
            ]
        };
        self.xform.reference = Some(uv);
    }

    /// The reference point at one of the 9 places of the options bar's
    /// locator (column, row: 0..2).
    pub(super) fn reference_cell(&self) -> Option<(u8, u8)> {
        let [u, v] = self.xform.reference.unwrap_or([0.5, 0.5]);
        let cell = |t: f64| (0..3u8).find(|&k| (t - k as f64 / 2.0).abs() < 1e-9);
        Some((cell(u)?, cell(v)?))
    }

    pub(super) fn set_reference_cell(&mut self, (col, row): (u8, u8)) {
        let r = [col as f64 / 2.0, row as f64 / 2.0];
        self.xform.reference = (r != [0.5, 0.5]).then_some(r);
    }

    /// Edit › Transform › a mode: Free Transform starts in it, or switches
    /// to it.
    fn set_transform_mode(&mut self, mode: Mode) {
        if !self.enter_transform() {
            return;
        }
        if mode == Mode::Warp {
            if self.xform.warp.is_none() {
                self.start_warp();
            }
        } else if let Some((w, frame, quad)) = self.xform.warp {
            // A warp left unchanged gives back its frame; otherwise it is
            // applied, and Free Transform goes on from it.
            if Warp::from_map(&frame) == Some(w) {
                self.xform.warp = None;
                self.xform.quad = quad;
            } else {
                self.apply_transform();
                if !self.enter_transform() {
                    return;
                }
            }
        }
        self.xform.mode = mode;
    }

    /// Warp mode: the grid over the frame, bent by the chosen style.
    fn start_warp(&mut self) {
        let Some(src) = self.transform_source() else {
            return;
        };
        let Some(frame) = Homography::square_to_quad(self.transform_frame(src)) else {
            return;
        };
        let quad = self.xform.quad;
        self.xform.warp = Warp::from_map(&frame).map(|w| (w, frame, quad));
        if self.xform.style != WarpStyle::Custom {
            self.restyle_warp();
        }
    }

    /// The warp grid after a change of style, bend or orientation.
    pub(super) fn restyle_warp(&mut self) {
        let Some((_, frame, quad)) = self.xform.warp else {
            return;
        };
        let Some(src) = self.transform_source() else {
            return;
        };
        let x = &self.xform;
        let aspect = f64::from(src.height() / src.width().max(1.0));
        let grid = x.style.grid(f64::from(x.bend) / 100.0, x.vertical, aspect);
        let mut points = [[[0.0; 2]; 4]; 4];
        for (row, out) in grid.0.iter().zip(&mut points) {
            for (p, o) in row.iter().zip(out.iter_mut()) {
                let Some(q) = frame.apply(*p) else { return };
                *o = q;
            }
        }
        self.xform.warp = Some((Warp(points), frame, quad));
    }

    pub(super) fn transform_warping(&self) -> bool {
        self.xform.warp.is_some()
    }

    /// The warp's style, bend (%) and orientation (vertical).
    pub(super) fn warp_style(&self) -> (WarpStyle, f32, bool) {
        (self.xform.style, self.xform.bend, self.xform.vertical)
    }

    pub(super) fn set_warp_style(&mut self, (style, bend, vertical): (WarpStyle, f32, bool)) {
        (self.xform.style, self.xform.bend, self.xform.vertical) = (style, bend, vertical);
        // Custom: the grid as it is, to drag.
        if style != WarpStyle::Custom {
            self.restyle_warp();
        }
    }

    /// Warp ⇄ Free Transform (the options bar's switch).
    pub(super) fn toggle_warp(&mut self) {
        let mode = if self.xform.warp.is_some() {
            Mode::Free
        } else {
            Mode::Warp
        };
        self.set_transform_mode(mode);
    }

    /// A quarter turn or flip of the frame being prepared, around the
    /// reference point.
    fn turn_pending(&mut self, cmd: Command) {
        let Some(src) = self.transform_source() else {
            return;
        };
        let pivot = self.reference_point(src);
        let Some(h) = cmd.map(pivot) else { return };
        let map = |p: Pt| h.apply(p).unwrap_or(p);
        if let Some((w, frame, quad)) = self.xform.warp {
            self.xform.warp = Some((w.mapped(map), frame.then(&h), quad.map(|q| q.map(map))));
        } else if let Some(q) = self.xform.quad {
            self.xform.quad = Some(q.map(map));
        } else if let Some((b, angle)) = turned_map(
            src,
            self.transform_rect.unwrap_or(src),
            self.transform_angle,
        )
        .then(&h)
        .as_turned_box(bounds(src))
        {
            self.transform_rect = Some(Rect::from_min_max(pos([b[0], b[1]]), pos([b[2], b[3]])));
            self.transform_angle = normalize_angle(angle as f32);
        } else {
            self.xform.quad = Some(self.transform_frame(src).map(map));
        }
    }

    /// Transform Again (Shift+Ctrl+T): the last transform, on what is
    /// selected now.
    fn transform_again(&mut self) {
        let Some(last) = self.xform.last else {
            self.status = t!("xform-status-no-again");
            return;
        };
        self.commit_pending();
        if !self.enter_transform() {
            return;
        }
        let Some(src) = self.transform_source() else {
            return;
        };
        let b = bounds(src);
        match last {
            Last::Map(h) => match h.as_turned_box(b) {
                Some((r, angle)) => {
                    self.transform_rect =
                        Some(Rect::from_min_max(pos([r[0], r[1]]), pos([r[2], r[3]])));
                    self.transform_angle = normalize_angle(angle as f32);
                }
                None => self.xform.quad = h.corners(b),
            },
            Last::Warp(old, w) => {
                let fit = Homography::rect_to_quad(
                    old,
                    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
                );
                let unit = Homography::rect_to_quad(
                    [0.0, 0.0, 1.0, 1.0],
                    [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
                );
                if let (Some(fit), Some(unit)) = (fit, unit) {
                    let grid = w.mapped(|p| fit.apply(p).unwrap_or(p));
                    self.xform.warp = Some((grid, unit, None));
                }
            }
        }
        self.xform.step = Some(t!("xform-history-again"));
        self.apply_transform();
    }

    /// Remembers a plain scale or turn applied (from the box `src` to the
    /// rectangle `frame` turned by `angle`), for Transform Again.
    pub(super) fn remember_transform(&mut self, src: Rect, frame: Rect, angle: f32) {
        self.xform.last = Some(Last::Map(turned_map(src, frame, angle)));
        // Named after the command or the mode, rather than as a plain
        // Free Transform.
        let mode = self.xform.mode;
        let name = self
            .xform
            .step
            .take()
            .or_else(|| (mode != Mode::Free).then(|| mode.label()));
        if let (Some(name), Some(doc)) = (name, &mut self.doc) {
            doc.rename_last_step(name);
        }
    }

    /// The frame about to be applied, in units of the source box (see
    /// `Recorded::Transform`); `None` for a warp, or if nothing moves.
    pub(super) fn recorded_frame(&self) -> Option<[Pt; 4]> {
        if self.xform.warp.is_some() {
            return None;
        }
        let src = self.transform_source()?;
        let b = bounds(src);
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let frame = match self.xform.quad {
            Some(q) => q,
            None => {
                let r = round_rect(self.transform_rect.unwrap_or(src));
                turned_map(src, r, self.transform_angle).corners(b)?
            }
        };
        let unit = frame.map(|[x, y]| [(x - b[0]) / w, (y - b[1]) / h]);
        let identity = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let moved = unit
            .iter()
            .zip(identity)
            .any(|(p, q)| (p[0] - q[0]).abs() > 1e-9 || (p[1] - q[1]).abs() > 1e-9);
        moved.then_some(unit)
    }

    /// Plays a recorded Free Transform (corners in units of the source box)
    /// on what is selected now: applied at once, or with `dialog` left to
    /// be adjusted (Enter applies it). Returns `false` if there is nothing
    /// to transform.
    pub(super) fn play_transform(&mut self, unit: [Pt; 4], dialog: bool) -> bool {
        self.commit_pending();
        if !self.enter_transform() {
            return false;
        }
        let Some(src) = self.transform_source() else {
            return false;
        };
        let b = bounds(src);
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let quad = unit.map(|[u, v]| [b[0] + u * w, b[1] + v * h]);
        match Homography::rect_to_quad(b, quad).and_then(|m| m.as_turned_box(b)) {
            Some((r, angle)) => {
                let (min, max) = (pos([r[0], r[1]]), pos([r[2], r[3]]));
                self.transform_rect = Some(Rect::from_min_max(min, max));
                self.transform_angle = normalize_angle(angle as f32);
            }
            None => self.xform.quad = Some(quad),
        }
        if !dialog {
            self.apply_transform();
        }
        true
    }

    /// Applies a Free Transform that the turned rectangle doesn't describe
    /// (distorted, warped), or of a mask, of lifted pixels or of several
    /// layers; one undo step. Returns `false` to leave it to the turned
    /// rectangle's way.
    pub(super) fn apply_transform_extras(&mut self) -> bool {
        let targets = self.transform_targets();
        let Some(doc) = &self.doc else { return false };
        let mask = targets.len() == 1 && doc.transforms_mask();
        let plain = !self.transform_distorted() && self.xform.lift.is_none();
        // A smart object is rendered again from its contents, on another
        // thread when large.
        let smart = !mask
            && targets
                .iter()
                .flat_map(|&i| doc.block(i))
                .any(|i| doc.layers[i].is_smart());
        if plain && !mask && !smart && targets.len() == 1 {
            return false;
        }
        let Some(t) = self
            .transform_source()
            .and_then(|src| self.pending_transform(src, true))
        else {
            self.reset_transform();
            return true;
        };
        let Some(doc) = &self.doc else { return true };
        let target = if mask {
            TransformTarget::Mask
        } else {
            TransformTarget::Layer
        };
        let mut layers: Vec<usize> = match target {
            TransformTarget::Mask => Vec::new(),
            TransformTarget::Layer => targets.iter().flat_map(|&i| doc.block(i)).collect(),
        };
        layers.sort_unstable();
        layers.dedup();
        let result = doc.transformed_bounds(&layers, &t);
        if result.is_some_and(|r| r.w > MAX_DIMENSION || r.h > MAX_DIMENSION) {
            self.status = t!("select-element-too-large", max = MAX_DIMENSION);
            self.reset_transform();
            return true;
        }
        let lift = self.xform.lift.take();
        // An affine map (skew, flip…) keeps a text editable.
        let text_map = match t.map {
            Map::Projective(h) if h.is_affine() && lift.is_none() => {
                let [a, b, c, d, e, f, .., i] = h.0;
                Some(crate::mask_ops::Affine(
                    [a, b, c, d, e, f].map(|v| (v / i) as f32),
                ))
            }
            _ => None,
        };
        let (texts, pixels): (Vec<usize>, Vec<usize>) = layers
            .iter()
            .partition(|&&i| text_map.is_some() && doc.layers[i].text.is_some());
        let rasterized = pixels.iter().any(|&i| doc.layers[i].text.is_some());
        let name = self
            .xform
            .step
            .take()
            .unwrap_or_else(|| self.xform.mode.label());
        let layer = doc.active_layer().name.clone();
        let canvas = doc.full_rect();
        let fit = self.fit_canvas && result.is_some();
        let commit = Commit {
            target,
            layers,
            pixels,
            texts,
            text_map,
            t,
            name,
            layer,
            rasterized,
            lift,
            fit,
            canvas,
        };
        if t.is_identity() {
            self.finish_transform(commit, None);
            return true;
        }
        let filter = self.resize_filter.filter();
        let Some(doc) = &self.doc else { return true };
        let job = doc.transform_job(target, &commit.pixels, &t, filter);
        if job.size() < ASYNC_PIXELS {
            self.finish_transform(commit, Some(job.run()));
            return true;
        }
        // Large: computed on another thread, the frame shown meanwhile.
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job.run());
            ctx.request_repaint();
        });
        self.xform.applying = Some(Applying {
            rx,
            doc: doc.uid(),
            started: self.ctx.input(|i| i.time),
            commit,
        });
        true
    }

    /// Whether a transform is being computed (it keeps the keyboard and
    /// the image).
    pub(super) fn transform_busy(&self) -> bool {
        self.xform.applying.is_some()
    }

    /// The transform being computed, once done, is applied; meanwhile, its
    /// busy indicator.
    pub(super) fn transform_window(&mut self, ctx: &egui::Context) {
        let Some(a) = &self.xform.applying else {
            return;
        };
        let done = match a.rx.try_recv() {
            Ok(done) => Some(done),
            Err(mpsc::TryRecvError::Empty) => {
                let name = a.commit.name.clone();
                return filters_ui::busy_indicator(ctx, &name, a.started);
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let Some(a) = self.xform.applying.take() else {
            return;
        };
        if self.doc.as_ref().is_some_and(|d| d.uid() == a.doc) {
            self.finish_transform(a.commit, done);
        }
    }

    /// Puts a transform in place with the pixels computed for it (`None`:
    /// it changes nothing), with its texts, lifted pixels, canvas and status.
    fn finish_transform(&mut self, c: Commit, done: Option<Vec<Transformed>>) {
        let Some(doc) = &mut self.doc else { return };
        let changed = done.is_some();
        if let Some(done) = done {
            doc.apply_transformed(c.target, &c.pixels, &c.t, done, c.name.clone());
        }
        if let Some(map) = c.text_map.filter(|_| changed) {
            for &i in &c.texts {
                self.transform_text(i, map);
            }
        }
        if changed {
            // A file being placed: placed and transformed in one step.
            self.join_placing();
        }
        let Some(doc) = &mut self.doc else { return };
        if let Some(lift) = c.lift {
            doc.drop_floating(lift.id);
            if changed {
                doc.join_last_step();
                self.selection = transformed_selection(&lift.selection, &c.t, c.canvas);
            } else {
                if doc.history_step() == lift.step {
                    doc.discard_step();
                }
                self.selection = Some(lift.selection);
            }
        }
        if !changed {
            return self.reset_transform();
        }
        let fitted = c
            .fit
            .then(|| {
                c.layers
                    .iter()
                    .filter(|&&i| doc.layers.get(i).is_some_and(Layer::has_pixels))
                    .map(|&i| doc.layers[i].rect())
                    .reduce(PixelRect::union)
            })
            .flatten();
        if let Some(r) = fitted {
            doc.fit_canvas_to(r);
        }
        self.xform.last = Some(match c.t.map {
            Map::Projective(h) => Last::Map(h),
            Map::Warp(w) => Last::Warp(c.t.src, w),
        });
        let (name, layer) = (c.name.as_str(), c.layer.as_str());
        self.status = t!("xform-status-done", what = name, name = layer);
        if c.rasterized {
            let note = t!("canvas-status-text-rasterized", name = layer);
            self.status = format!("{} — {note}", self.status);
        }
        if let Some(r) = fitted {
            let old_center = pos2(c.canvas.w as f32, c.canvas.h as f32) / 2.0;
            self.forget_selection();
            self.after_geometry_change(to_rect(r).center() - old_center);
        }
        self.reset_transform();
    }

    /// Moves the distorted frame or the warp grid (arrow keys).
    pub(super) fn nudge_transform(&mut self, d: Vec2) {
        let d = [f64::from(d.x), f64::from(d.y)];
        if let Some(q) = &mut self.xform.quad {
            *q = q.map(|p| add(p, d));
        }
        if let Some((w, ..)) = &mut self.xform.warp {
            *w = w.mapped(|p| add(p, d));
        }
    }

    // ----------------------------------------------------------- pointer

    /// What is under the pointer `p` (screen) in the frame.
    fn grab_at(&self, view: View, src: Rect, p: Pos2) -> Option<Grab> {
        let near = |a: Pos2| a.distance(p) <= handles::GRAB_RADIUS;
        if self.xform.reference.is_some() && near(view.to_screen(pos(self.reference_point(src)))) {
            return Some(Grab::Reference);
        }
        if let Some((w, ..)) = &self.xform.warp {
            let handle = (0..16)
                .map(|k| (k / 4, k % 4))
                .filter(|&(j, i)| !(1..=2).contains(&j) || !(1..=2).contains(&i))
                .map(|(j, i)| (j, i, view.to_screen(pos(w.0[j][i])).distance(p)))
                .filter(|&(.., d)| d <= handles::GRAB_RADIUS)
                .min_by(|a, b| a.2.total_cmp(&b.2));
            if let Some((j, i, _)) = handle {
                return Some(Grab::WarpPoint(j, i));
            }
            let (u, v) = w.locate(pt(view.to_image(p)));
            let on = view.to_screen(pos(w.eval(u, v))).distance(p) <= handles::GRAB_RADIUS;
            return on.then_some(Grab::WarpSurface(u, v));
        }
        let q = self.transform_frame(src).map(|c| view.to_screen(pos(c)));
        if self.xform.mode != Mode::Scale && near(knob(&q)) {
            return Some(Grab::Rotate);
        }
        let nearest = |points: [Pos2; 4]| {
            (0..4)
                .map(|k| (k, points[k].distance(p)))
                .filter(|&(_, d)| d <= handles::GRAB_RADIUS)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(k, _)| k)
        };
        if let Some(k) = nearest(q) {
            return Some(Grab::Corner(k));
        }
        let mids = [0, 1, 2, 3].map(|k| q[k].lerp(q[(k + 1) % 4], 0.5));
        if let Some(k) = nearest(mids) {
            return Some(Grab::Edge(k));
        }
        if let Some(k) = (0..4)
            .find(|&k| segment_distance(p, q[k], q[(k + 1) % 4]) <= handles::GRAB_RADIUS * 0.6)
        {
            return Some(Grab::Edge(k));
        }
        inside(&q, p).then_some(Grab::Move)
    }

    /// Free Transform's pointer input beyond the plain scales and turns of
    /// the turned rectangle (see `handles_input`): distorted frames, warps,
    /// modifier drags, the modes, the reference point. `None` leaves the
    /// pointer to the turned rectangle.
    pub(super) fn transform_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
    ) -> Option<CursorIcon> {
        if self.active_locked() {
            return None;
        }
        let src = self.transform_source()?;
        let (ctrl, alt, double) = self.ctx.input(|i| {
            let double = i.pointer.button_double_clicked(PointerButton::Primary);
            (i.modifiers.command, i.modifiers.alt, double)
        });
        // A double click inside applies it, like Enter.
        if double
            && self.transform_pending()
            && pointer.is_some_and(|p| {
                let grab = self.grab_at(view, src, p);
                matches!(grab, Some(Grab::Move | Grab::WarpSurface(..)))
            })
        {
            self.apply_transform();
            return Some(CursorIcon::Default);
        }
        if let Some(drag) = self.xform.drag {
            if !down {
                self.xform.drag = None;
            } else if let Some(p) = pointer {
                self.drag_frame(drag, src, pt(view.to_image(p)), shift, alt);
            }
            return Some(grab_cursor(drag.grab, true));
        }
        // A drag of the turned rectangle goes on.
        if self.drag.is_some() || self.rotate_drag.is_some() {
            return None;
        }
        let distorted = self.transform_distorted();
        let Some(p) = pointer else {
            return distorted.then_some(CursorIcon::Default);
        };
        let grab = self.grab_at(view, src, p);
        let edit = match (ctrl, shift, alt) {
            (true, true, true) => Some(Edit::Perspective),
            (true, true, false) => Some(Edit::Skew),
            (true, false, _) => Some(Edit::Distort),
            _ => self.xform.mode.edit(),
        };
        let pivot_moved = self.xform.reference.is_some();
        let warping = self.xform.warp.is_some();
        let grab = match grab {
            Some(Grab::Reference) => Some(Grab::Reference),
            Some(g @ (Grab::WarpPoint(..) | Grab::WarpSurface(..))) => Some(g),
            _ if warping => None,
            _ if self.xform.mode == Mode::Rotate => Some(Grab::Rotate),
            Some(g @ (Grab::Corner(_) | Grab::Edge(_)))
                if edit.is_some() || distorted || (alt && pivot_moved) =>
            {
                Some(g)
            }
            Some(Grab::Rotate) if distorted || pivot_moved => Some(Grab::Rotate),
            // Alt+click: the reference point there.
            Some(Grab::Move) | None if alt && !ctrl && self.transform_rect.is_some() => {
                Some(Grab::Reference)
            }
            Some(Grab::Move) if distorted => Some(Grab::Move),
            // Outside the frame: turns it.
            None if distorted => Some(Grab::Rotate),
            _ => None,
        };
        // Scale mode: the frame doesn't turn.
        if grab.is_none() && self.xform.mode == Mode::Scale && !distorted {
            let r = view.rect_to_screen(self.transform_rect.unwrap_or(src));
            let local = rotate_around(p, r.center(), -self.transform_angle);
            if local.distance(rotate_handle_pos(r)) <= handles::GRAB_RADIUS {
                return Some(CursorIcon::NotAllowed);
            }
        }
        let Some(grab) = grab else {
            return (distorted || warping).then_some(CursorIcon::Default);
        };
        if start {
            let ip = pt(view.to_image(p));
            let edit = edit.unwrap_or(Edit::Scale);
            if matches!(grab, Grab::Corner(_) | Grab::Edge(_)) && edit != Edit::Scale && !distorted
            {
                self.xform.quad = Some(self.transform_frame(src));
            }
            if grab == Grab::Reference && alt {
                self.set_reference(src, ip);
            }
            self.transform_rect.get_or_insert(src);
            self.xform.drag = Some(Drag {
                grab,
                edit,
                start: ip,
                quad: self.transform_frame(src),
                warp: self.xform.warp.map(|w| w.0),
                rect: self.transform_rect.unwrap_or(src),
                angle: self.transform_angle,
                pivot: self.reference_point(src),
            });
        }
        let cursor = match (grab, edit) {
            (Grab::Corner(_) | Grab::Edge(_), Some(_)) => CursorIcon::Crosshair,
            _ => grab_cursor(grab, false),
        };
        Some(cursor)
    }

    /// The frame as `drag` makes it with the pointer at `p` (canvas).
    fn drag_frame(&mut self, drag: Drag, src: Rect, p: Pt, shift: bool, alt: bool) {
        let mut d = sub(p, drag.start);
        let quad = match drag.grab {
            Grab::Move => {
                if shift {
                    let c = handles::constrain_axis(vec2(d[0] as f32, d[1] as f32));
                    d = [f64::from(c.x), f64::from(c.y)];
                }
                if let Some(w) = drag.warp {
                    return self.set_warp(w.mapped(|q| add(q, d)));
                }
                drag.quad.map(|q| add(q, d))
            }
            Grab::Reference => return self.set_reference(src, p),
            Grab::Rotate => {
                let angle = |q: Pt| {
                    let v = sub(q, drag.pivot);
                    v[1].atan2(v[0]).to_degrees()
                };
                let mut delta = angle(p) - angle(drag.start);
                if shift {
                    let snap = f64::from(ROTATE_SNAP);
                    let base = if self.transform_distorted() {
                        0.0
                    } else {
                        f64::from(drag.angle)
                    };
                    delta = ((base + delta) / snap).round() * snap - base;
                }
                let h = Homography::rotate(drag.pivot, delta);
                let map = |q: Pt| h.apply(q).unwrap_or(q);
                if let Some(w) = drag.warp {
                    return self.set_warp(w.mapped(map));
                }
                if !self.transform_distorted() {
                    let a = normalize_angle(drag.angle + delta as f32);
                    let center = pos(map(pt(drag.rect.center())));
                    if fits_max_dimension(drag.rect.size(), a) {
                        self.transform_rect =
                            Some(Rect::from_center_size(center, drag.rect.size()));
                        self.transform_angle = a;
                    }
                    return;
                }
                drag.quad.map(map)
            }
            Grab::WarpPoint(j, i) => {
                let Some(mut w) = drag.warp else { return };
                w.0[j][i] = add(w.0[j][i], d);
                // A corner takes its handles along.
                if (j == 0 || j == 3) && (i == 0 || i == 3) {
                    let (dj, di) = (if j == 0 { 1 } else { 2 }, if i == 0 { 1 } else { 2 });
                    for (a, b) in [(j, di), (dj, i), (dj, di)] {
                        w.0[a][b] = add(w.0[a][b], d);
                    }
                }
                return self.set_warp(w);
            }
            Grab::WarpSurface(u, v) => {
                let Some(mut w) = drag.warp else { return };
                w.drag((u, v), d);
                return self.set_warp(w);
            }
            Grab::Corner(_) | Grab::Edge(_) if drag.edit == Edit::Scale => {
                let Some(h0) = Homography::rect_to_quad(bounds(src), drag.quad) else {
                    return;
                };
                let Some(back) = h0.inverse() else { return };
                let (Some(s0), Some(s1)) = (back.apply(drag.start), back.apply(p)) else {
                    return;
                };
                let pivot = alt.then(|| back.apply(drag.pivot)).flatten();
                let keep = self.keep_ratio != shift;
                let b = scale_box(bounds(src), sides(drag.grab), sub(s1, s0), keep, pivot);
                let Some(q) = h0.corners(b) else { return };
                if !self.transform_distorted() {
                    let fit = Homography::rect_to_quad(bounds(src), q)
                        .and_then(|h| h.as_turned_box(bounds(src)));
                    if let Some((r, _)) = fit {
                        let r = Rect::from_min_max(pos([r[0], r[1]]), pos([r[2], r[3]]));
                        if fits_max_dimension(r.size(), self.transform_angle) {
                            self.transform_rect = Some(r);
                        }
                    }
                    return;
                }
                q
            }
            Grab::Corner(_) | Grab::Edge(_) => edit_quad(drag.quad, drag.grab, drag.edit, d, alt),
        };
        // Only shapes a perspective can give, that fit in a layer.
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for [x, y] in quad {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        let max = f64::from(MAX_DIMENSION);
        if xform::convex(&quad) && b[2] - b[0] <= max && b[3] - b[1] <= max {
            self.xform.quad = Some(quad);
        }
    }

    fn set_warp(&mut self, w: Warp) {
        if let Some((grid, ..)) = &mut self.xform.warp {
            *grid = w;
            self.xform.style = WarpStyle::Custom;
        }
    }

    // ------------------------------------------------------------- menus

    /// The Edit › Transform submenu.
    pub(super) fn transform_menu(&self, has_doc: bool) -> Vec<MenuEntry> {
        let again = has_doc && self.xform.last.is_some();
        let mut entries = vec![
            MenuEntry::Item(
                Command::Again.label(),
                again,
                Some(&SHORTCUT_AGAIN),
                Action::Transform(Command::Again),
            ),
            MenuEntry::Separator,
        ];
        for mode in Mode::MENU {
            let action = Action::Transform(Command::Mode(mode));
            entries.push(MenuEntry::Item(mode.label(), has_doc, None, action));
        }
        for (k, cmd) in Command::TURNS.into_iter().enumerate() {
            if k == 0 || k == 3 {
                entries.push(MenuEntry::Separator);
            }
            entries.push(MenuEntry::Item(
                cmd.label(),
                has_doc,
                None,
                Action::Transform(cmd),
            ));
        }
        entries
    }

    /// The canvas's context menu during Free Transform: its modes, the
    /// quarter turns and flips. Returns `false` when no transform is
    /// pending (the layer menu shows).
    pub(super) fn transform_context_menu(
        &self,
        ui: &mut egui::Ui,
        action: &mut Option<Action>,
    ) -> bool {
        if !self.transform_pending() {
            return false;
        }
        let current = self.xform.mode;
        for mode in [Mode::Free].into_iter().chain(Mode::MENU) {
            if ui.selectable_label(current == mode, mode.label()).clicked() {
                *action = Some(Action::Transform(Command::Mode(mode)));
                ui.close();
            }
        }
        for (k, cmd) in Command::TURNS.into_iter().enumerate() {
            if k == 0 || k == 3 {
                ui.separator();
            }
            if ui.button(cmd.label()).clicked() {
                *action = Some(Action::Transform(cmd));
                ui.close();
            }
        }
        true
    }

    // ----------------------------------------------------------- drawing

    /// Whether the frame can be turned (not in Scale mode).
    pub(super) fn transform_turns(&self) -> bool {
        self.xform.mode != Mode::Scale
    }

    /// Box (canvas) of a distorted frame or warp grid.
    pub(super) fn transform_reach(&self) -> Option<Rect> {
        let points: Vec<Pos2> = match (&self.xform.warp, self.xform.quad) {
            (Some((w, ..)), _) => w.0.iter().flatten().map(|&p| pos(p)).collect(),
            (None, Some(q)) => q.map(pos).to_vec(),
            (None, None) => return None,
        };
        Some(Rect::from_points(&points))
    }

    /// Frame of a distorted Free Transform, or its warp grid. `false`: the
    /// turned rectangle's frame is to be drawn.
    pub(super) fn draw_transform_frame(&self, painter: &Painter, view: View) -> bool {
        let accent = ui_kit::color::ACCENT;
        let stroke = UiStroke::new(1.5, accent);
        let thin = UiStroke::new(1.0, accent);
        let square = |p: Pos2| {
            let h = Rect::from_center_size(p, Vec2::splat(9.0));
            painter.rect(h, 2.0, Color32::WHITE, stroke, StrokeKind::Inside);
        };
        let screen = |p: Pt| view.to_screen(pos(p));
        if let Some((w, ..)) = &self.xform.warp {
            const STEPS: usize = 32;
            for k in 0..4 {
                let t = k as f64 / 3.0;
                let line = |f: &dyn Fn(f64) -> Pt| -> Vec<Pos2> {
                    (0..=STEPS)
                        .map(|s| screen(f(s as f64 / STEPS as f64)))
                        .collect()
                };
                let s = if k == 0 || k == 3 { stroke } else { thin };
                painter.add(egui::Shape::line(line(&|v| w.eval(t, v)), s));
                painter.add(egui::Shape::line(line(&|u| w.eval(u, t)), s));
            }
            let g = &w.0;
            for (j, i) in [(0, 0), (0, 3), (3, 3), (3, 0)] {
                let (dj, di) = (if j == 0 { 1 } else { 2 }, if i == 0 { 1 } else { 2 });
                for (a, b) in [(j, di), (dj, i)] {
                    painter.line_segment([screen(g[j][i]), screen(g[a][b])], thin);
                    painter.circle(screen(g[a][b]), 4.0, Color32::WHITE, stroke);
                }
                square(screen(g[j][i]));
            }
            return true;
        }
        let Some(q) = self.xform.quad else {
            return false;
        };
        let q = q.map(screen);
        painter.add(egui::Shape::closed_line(q.to_vec(), stroke));
        if self.transform_turns() {
            let k = knob(&q);
            painter.line_segment([q[0].lerp(q[1], 0.5), k], stroke);
            painter.circle(k, 5.5, Color32::WHITE, stroke);
        }
        for k in 0..4 {
            square(q[k]);
            square(q[k].lerp(q[(k + 1) % 4], 0.5));
        }
        true
    }

    /// The reference point, once moved from the center.
    pub(super) fn draw_reference_point(&self, painter: &Painter, view: View) {
        let Some(src) = self
            .transform_source()
            .filter(|_| self.xform.reference.is_some())
        else {
            return;
        };
        let accent = ui_kit::color::ACCENT;
        let c = view.to_screen(pos(self.reference_point(src)));
        painter.circle_stroke(c, 5.0, UiStroke::new(3.0, Color32::WHITE));
        painter.circle_stroke(c, 5.0, UiStroke::new(1.5, accent));
        for d in [vec2(9.0, 0.0), vec2(0.0, 9.0)] {
            painter.line_segment([c - d, c + d], UiStroke::new(1.5, accent));
        }
    }

    // ----------------------------------------------------------- preview

    /// The layers transformed as the transform being prepared will place
    /// them (`frame`: the source box, while previewing), where `visible`; a
    /// mask transformed alone is previewed through `self.xform`.
    pub(super) fn transform_preview(
        &mut self,
        ctx: &egui::Context,
        frame: Option<Rect>,
        visible: Rect,
    ) -> Vec<(usize, render::PlacedImage)> {
        let t = frame.and_then(|src| self.pending_transform(src, false));
        let targets = self.transform_targets();
        let Some((t, doc)) = t.zip(self.doc.as_ref()) else {
            self.xform.mask_preview = None;
            self.xform.lifted.clear();
            return Vec::new();
        };
        let ids: Vec<u64> = targets.iter().map(|&i| doc.layers[i].id).collect();
        self.xform.lifted.retain(|(id, _)| ids.contains(id));
        if targets.len() > 1 || !doc.transforms_mask() {
            self.xform.mask_preview = None;
            return targets
                .into_iter()
                .filter_map(|i| Some((i, self.placed(ctx, i, &t, visible)?)))
                .collect();
        }
        self.preview_mask(&t, visible);
        Vec::new()
    }

    /// The active layer's mask as the transform `t` will make it, where
    /// `visible` (see `compose::PlacedMask`).
    fn preview_mask(&mut self, t: &Transform, visible: Rect) -> Option<()> {
        let doc = self.doc.as_ref()?;
        let level = render::render_level(self.zoom);
        let layer = doc.active_layer();
        let mask = layer.mask.as_ref()?;
        let mask_key = mask.key(layer.offset);
        let key = hash_of((
            mask_key,
            level,
            t.fingerprint(),
            bounds(visible).map(f64::to_bits),
        ));
        if self
            .xform
            .mask_preview
            .as_ref()
            .is_some_and(|p| p.key == key)
        {
            return Some(());
        }
        let b = 1u32 << level;
        if self
            .xform
            .reduced_mask
            .as_ref()
            .is_none_or(|r| (r.key, r.level) != (mask_key, level))
        {
            let (image, origin) = mask.shaped(layer.offset);
            let reduced = if b == 1 {
                image
            } else {
                Arc::new(crate::compose::levels::reduce_gray(&image, b))
            };
            self.xform.reduced_mask = Some(ReducedMask {
                key: mask_key,
                level,
                image: reduced,
                origin,
            });
        }
        let (default, id) = (mask.default, layer.id);
        let ReducedMask { image, origin, .. } = self.xform.reduced_mask.as_ref()?;
        let area = [
            origin.0 as f64,
            origin.1 as f64,
            (origin.0 + (image.width() * b) as i64) as f64,
            (origin.1 + (image.height() * b) as i64) as f64,
        ];
        let v = bounds(visible);
        let reach = t.bounds(area).unwrap_or(v);
        let grid = Grid::covering(xform::clip(reach, v), f64::from(b));
        let src = Source {
            image: &**image,
            origin: [origin.0 as f64, origin.1 as f64],
            px: f64::from(b),
        };
        let moved = xform::transform_gray(src, t, grid, xform::Filter::Bilinear, default);
        let bi = i64::from(b);
        self.xform.mask_preview = Some(MaskPreview {
            layer: id,
            key,
            image: Arc::new(moved),
            at: (grid.x, grid.y),
            rect: PixelRect {
                x: grid.x * bi,
                y: grid.y * bi,
                w: grid.w * b,
                h: grid.h * b,
            },
        });
        Some(())
    }
}

fn grab_cursor(grab: Grab, dragging: bool) -> CursorIcon {
    match grab {
        Grab::Move | Grab::WarpPoint(..) => CursorIcon::Move,
        Grab::Rotate | Grab::WarpSurface(..) if dragging => CursorIcon::Grabbing,
        Grab::Rotate | Grab::WarpSurface(..) => CursorIcon::Grab,
        Grab::Reference => CursorIcon::Crosshair,
        Grab::Corner(k) => [
            CursorIcon::ResizeNwSe,
            CursorIcon::ResizeNeSw,
            CursorIcon::ResizeNwSe,
            CursorIcon::ResizeNeSw,
        ][k % 4],
        Grab::Edge(k) if k % 2 == 0 => CursorIcon::ResizeVertical,
        Grab::Edge(_) => CursorIcon::ResizeHorizontal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_scale_from_the_opposite_side_or_the_pivot() {
        let b = [0.0, 0.0, 100.0, 50.0];
        let scale_box = |b, sides, d, keep, pivot| {
            scale_box(b, sides, d, keep, pivot).map(|v: f64| (v * 1e6).round() / 1e6)
        };
        // Bottom-right corner, free: the top-left stays.
        assert_eq!(
            scale_box(b, (1, 1), [10.0, 5.0], false, None),
            [0.0, 0.0, 110.0, 55.0]
        );
        // Proportions kept: the larger change wins.
        assert_eq!(
            scale_box(b, (1, 1), [100.0, 5.0], true, None),
            [0.0, 0.0, 200.0, 100.0]
        );
        // An edge keeps the middle of the other axis.
        assert_eq!(
            scale_box(b, (0, 1), [0.0, 50.0], true, None),
            [-50.0, 0.0, 150.0, 100.0]
        );
        // Around a pivot (Alt + the reference point).
        assert_eq!(
            scale_box(b, (1, 0), [100.0, 0.0], false, Some([0.0, 25.0])),
            [0.0, 0.0, 200.0, 50.0]
        );
        assert_eq!(
            scale_box(b, (1, 0), [50.0, 0.0], false, Some([50.0, 25.0])),
            [-50.0, 0.0, 150.0, 50.0]
        );
    }

    #[test]
    fn modifier_edits_of_the_frame() {
        let q = [[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]];
        // Distort: the corner follows; Alt: the opposite one mirrors it.
        let d = edit_quad(q, Grab::Corner(2), Edit::Distort, [10.0, 20.0], false);
        assert_eq!(d, [[0.0, 0.0], [100.0, 0.0], [110.0, 70.0], [0.0, 50.0]]);
        let d = edit_quad(q, Grab::Corner(2), Edit::Distort, [10.0, 20.0], true);
        assert_eq!(d[0], [-10.0, -20.0]);
        // Skew: the top edge slides along itself.
        let s = edit_quad(q, Grab::Edge(0), Edit::Skew, [30.0, 12.0], false);
        assert_eq!(s, [[30.0, 0.0], [130.0, 0.0], [100.0, 50.0], [0.0, 50.0]]);
        // Perspective: the top-right corner out, the top-left in.
        let p = edit_quad(q, Grab::Corner(1), Edit::Perspective, [20.0, 3.0], false);
        assert_eq!(p, [[-20.0, 0.0], [120.0, 0.0], [100.0, 50.0], [0.0, 50.0]]);
        assert!(xform::convex(&p));
    }

    #[test]
    fn quarter_turns_and_flips_land_on_whole_pixels() {
        let r = PixelRect {
            x: 10,
            y: 20,
            w: 7,
            h: 4,
        };
        let [a, b, c, d] = turned_box(r, Command::RotateCw).unwrap();
        // 4 × 7, its center as near as whole pixels allow.
        assert_eq!(
            [a, b, c, d],
            [[15.0, 18.0], [15.0, 25.0], [11.0, 25.0], [11.0, 18.0]]
        );
        let h = Homography::rect_to_quad([10.0, 20.0, 17.0, 24.0], [a, b, c, d]).unwrap();
        assert!(h.as_turned_box([10.0, 20.0, 17.0, 24.0]).is_some());
        let flip = turned_box(r, Command::FlipHorizontal).unwrap();
        assert_eq!(flip[0], [17.0, 20.0]);
        // Two quarter turns: a half turn.
        let half = turned_box(r, Command::Rotate180).unwrap();
        assert_eq!(half[0], [17.0, 24.0]);
    }
}
