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
    /// History position right after the lift, and the mark before it.
    step: u64,
    mark: crate::document::HistoryMark,
}

/// The pending transform as it was before a drag: Ctrl+Z inside Free
/// Transform goes back to it.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Snapshot {
    rect: Option<Rect>,
    angle: f32,
    quad: Option<[Pt; 4]>,
    warp: Option<(Warp, Homography, Option<[Pt; 4]>)>,
    reference: Option<Pt>,
    mode: Mode,
    style: (WarpStyle, f32, bool),
    /// The state before a warp was applied by a change of mode: going
    /// back to it gives the applied warp up.
    switch: bool,
}

/// A warp applied when switching to another mode, kept pending with the
/// transform that goes on: where the history was before it, and the
/// selection then.
struct Applied {
    mark: crate::document::HistoryMark,
    selection: Option<Selection>,
    /// The pixels were lifted then (the selection's): lifted again when
    /// Ctrl+Z comes back to before it.
    lifted: bool,
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
    /// The quick mask: the selection whose part of it is transformed.
    selection: Option<Selection>,
    fit: bool,
    /// The canvas before.
    canvas: PixelRect,
    /// A plain scale or turn of one large layer (computed aside): its frame's
    /// size and angle, said as the plain path says it.
    placed: Option<(u32, u32, f32)>,
}

/// A transform computed on another thread.
struct Applying {
    rx: mpsc::Receiver<Vec<Transformed>>,
    /// The document it is for.
    doc: u64,
    started: f64,
    commit: Commit,
    applied: Option<Applied>,
    /// What was asked for after it (a command, a tab, a layer clicked),
    /// done once it lands (see `commit_pending_aside`).
    then: Vec<super::Then>,
}

/// The last transform applied, for Transform Again.
// Only one, kept between transforms.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy)]
enum Last {
    /// Canvas → canvas, from the source box (first) to the box of what it
    /// made (second).
    Map(Homography, Bounds, Bounds),
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
    /// Box of the selected pixels a lift would take, for the Move tool's
    /// frame (by a fingerprint of the layer and the selection).
    selected_bounds: Cell<Option<(u64, Option<PixelRect>)>>,
    pub(super) mask_preview: Option<MaskPreview>,
    reduced_mask: Option<ReducedMask>,
    /// The other layers transformed with the active one, on their own (see
    /// `render::Lifted`), by id.
    lifted: Vec<(u64, render::Lifted)>,
    applying: Option<Applying>,
    /// Ctrl+Z inside Free Transform: the states before each edit.
    undo: Vec<Snapshot>,
    /// Ctrl+Shift+Z inside it: the states undone, with the state each
    /// undo left (an edit since then makes them stale).
    redo: Vec<(Snapshot, Snapshot)>,
    /// The series of edits the last state recorded starts (arrow keys,
    /// a number field dragged): its kind and when it was last edited.
    series: Option<(&'static str, f64)>,
    applied: Option<Applied>,
    /// The later warps applied by a change of mode, the last one last:
    /// Ctrl+Z comes back to before each.
    switches: Vec<Applied>,
    /// The mode to go on in once the warp being applied is done.
    reenter: Option<Mode>,
    /// Moves and transforms of the quick mask.
    pub(super) quick: super::quick_mask::QuickEdit,
    /// The Ctrl+Z points from before that warp, and the state then.
    reenter_undo: Option<(Vec<Snapshot>, Snapshot)>,
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
            selected_bounds: Cell::new(None),
            mask_preview: None,
            reduced_mask: None,
            lifted: Vec::new(),
            applying: None,
            undo: Vec::new(),
            redo: Vec::new(),
            series: None,
            applied: None,
            switches: Vec::new(),
            reenter: None,
            reenter_undo: None,
            quick: Default::default(),
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
/// Transform Again's memory of `h` applied to the box `src`, which made
/// the element box `made` (what Transform Again then finds selected: of an
/// ellipse turned, smaller than the turned box's).
fn last_map(h: Homography, src: Bounds, made: Option<Rect>) -> Last {
    if let Some(made) = made {
        return Last::Map(h, src, bounds(made));
    }
    let made = h.corners(src).map_or(src, |q| {
        let (xs, ys) = (q.map(|p| p[0]), q.map(|p| p[1]));
        let min = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
        let max = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
        [min(xs), min(ys), max(xs), max(ys)]
    });
    Last::Map(h, src, made)
}

/// The map Transform Again applies to the box `b`: the last one `h` (of
/// the box `from`, which became `made`) as it is for what it made (or a
/// copy of it in place: step and repeat), otherwise the same scale and
/// turn around the center of `b`.
fn again_map(h: Homography, from: Bounds, made: Bounds, b: Bounds) -> Homography {
    if (0..4).all(|k| (made[k] - b[k]).abs() <= 2.0) {
        return h;
    }
    let center = |r: Bounds| [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
    let (c0, c1) = (center(from), center(b));
    let (dx, dy) = (c1[0] - c0[0], c1[1] - c0[1]);
    Homography::affine([1.0, 0.0, -dx, 0.0, 1.0, -dy])
        .then(&h)
        .then(&Homography::affine([1.0, 0.0, dx, 0.0, 1.0, dy]))
}

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

/// While a transform is computed: after a moment, says so. Not modal:
/// input meanwhile waits for it (see `wait_transform_for_input`).
fn busy_note(ctx: &egui::Context, name: &str, started: f64) {
    let elapsed = ctx.input(|i| i.time) - started;
    let delay = filters_ui::BUSY_DELAY;
    if elapsed < delay {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
        return;
    }
    use ui_kit::color;
    egui::Area::new(egui::Id::new("transform_busy"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(color::BG2)
                .stroke(UiStroke::new(1.0, color::WINDOW_LINE))
                .corner_radius(12)
                .inner_margin(egui::Margin::symmetric(18, 14))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.add(egui::Spinner::new().size(16.0).color(color::MUTED));
                        ui.label(egui::RichText::new(format!("{name}…")).color(color::TEXT));
                    });
                });
        });
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
            // Every command but Again (which repeats a layer's transform).
            _ if !matches!(cmd, Command::Again) && self.path_transform_wanted() => {
                self.status = t!("tools5-status-path-transform");
            }
            Command::Free => {
                self.enter_transform();
            }
            Command::Again => self.transform_again(),
            Command::Mode(mode) => {
                let before = self.snapshot();
                self.set_transform_mode(mode);
                self.transform_edited(before, None);
            }
            _ if self.transform_pending() => {
                let before = self.snapshot();
                self.turn_pending(cmd);
                self.transform_edited(before, None);
            }
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

    /// Ctrl+T with a path tool and a path (or some of a shape's
    /// components) selected: the path is meant, not the layer's pixels.
    fn path_transform_wanted(&self) -> bool {
        use crate::document::PathRef;
        if !matches!(
            self.tool,
            Tool::Pen(_) | Tool::PathSelect | Tool::DirectSelect
        ) {
            return false;
        }
        let v = &self.vector;
        if matches!(v.selected, Some(PathRef::Work | PathRef::Saved(_))) {
            return true;
        }
        let parts = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().shape.as_ref())
            .map_or(0, |s| s.path.subpaths.len());
        !v.components.is_empty() && v.components.len() < parts
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
        // The quick mask: the layer's lock doesn't protect it.
        let quick = self.in_quick_mask();
        if self.doc.is_none() || (!quick && self.refuse_if_locked()) {
            return false;
        }
        self.set_tool(Tool::Transform);
        // A crop box drawn: "Crop the image?" first.
        if self.tool != Tool::Transform {
            return false;
        }
        // The Move tool's transform applied aside: framed once in place.
        self.wait_transform();
        if self.transform_rect.is_some() {
            self.free_transform = true;
            return true;
        }
        if quick {
            // What the selection selects of the mask: its outline comes
            // back where that goes.
            self.xform.quick.selection = self.selection.take();
        } else if !self.lift_selection() {
            return false;
        }
        let Some(src) = self.transform_source() else {
            if quick {
                self.selection = self.xform.quick.selection.take();
                self.status = t!("xform-status-nothing", name = t!("qm6-quick-mask-name"));
                return false;
            }
            let name = self.doc.as_ref().map(|d| d.active_layer().name.clone());
            self.status = t!("xform-status-nothing", name = name.unwrap_or_default());
            return false;
        };
        self.transform_rect = Some(src);
        self.free_transform = true;
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
        let mark = doc.history_mark();
        let Some(id) = doc.lift_region(&selection, false) else {
            self.status = t!("xform-status-empty");
            return false;
        };
        let step = doc.history_step();
        self.xform.lift = Some(Lift {
            id,
            selection,
            step,
            mark,
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
        if let Some(selection) = self.xform.quick.selection.take() {
            self.selection = Some(selection);
        }
        // A warp applied on the way (another mode chosen after it) goes
        // too, with the selection from before it.
        self.xform.switches.clear();
        if let Some(applied) = self.xform.applied.take() {
            if let Some(doc) = &mut self.doc {
                doc.rollback_to(applied.mark);
            }
            self.selection = applied.selection;
        }
        self.xform.quick.clear(false);
        let x = &mut self.xform;
        x.mode = Mode::Free;
        (x.quad, x.warp, x.reference, x.drag, x.step) = (None, None, None, None, None);
        x.mask_preview = None;
        x.undo.clear();
        x.redo.clear();
        x.series = None;
    }

    /// Move tool with Show Transform Controls and a selection on a layer
    /// whose pixels it can lift: the box of what it selects there.
    pub(super) fn selection_frame(&self) -> Option<Rect> {
        if self.tool != Tool::Transform
            || !self.show_transform_controls
            || self.transform_rect.is_some()
            || self.select_move.is_some()
        {
            return None;
        }
        let selection = self.selection.as_ref()?;
        let doc = self.doc.as_ref()?;
        let layer = doc.active_layer();
        if !layer.paintable() || doc.mask_targeted() {
            return None;
        }
        // The tight box of what a lift takes (selected and not
        // transparent), so that a handle grabbed stays under the pointer.
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (doc.uid(), layer.id, layer.rev(), layer.offset).hash(&mut h);
            let r = selection.rect;
            (r.x, r.y, r.w, r.h, selection.inverted).hash(&mut h);
            selection.mask.as_ref().map(Arc::as_ptr).hash(&mut h);
            h.finish()
        };
        let bounds = match self.xform.selected_bounds.get() {
            Some((k, b)) if k == key => b,
            _ => {
                let b = selected_pixels_bounds(doc, selection);
                self.xform.selected_bounds.set(Some((key, b)));
                b
            }
        };
        bounds.map(to_rect).filter(|r| r.is_positive())
    }

    /// Move tool with a selection, like Photoshop: a drag moves the
    /// selected pixels (Alt: a copy of them) rather than the whole layer.
    /// The cursor while it does.
    pub(super) fn move_tool_selection(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
    ) -> Option<CursorIcon> {
        if self.select_move.is_some() {
            return Some(self.move_selection(view, pointer, down, shift));
        }
        if self.transform_rect.is_some() || self.transform_angle != 0.0 {
            return None;
        }
        // Show Transform Controls: the frame of the selected pixels, whose
        // handles lift them into Free Transform.
        let handle = self.selection_frame().zip(pointer).and_then(|(frame, p)| {
            handles::hit_test(view.rect_to_screen(frame), p).filter(|&h| h != Handle::Move)
        });
        if !start {
            return handle.map(|h| h.cursor(0.0));
        }
        let selection = self.selection.clone()?;
        let doc = self.doc.as_ref()?;
        // Groups, smart objects… move whole; a targeted mask too.
        if !doc.active_layer().paintable() || doc.mask_targeted() {
            return None;
        }
        let p = pointer?;
        if !self.can_lift() {
            return Some(CursorIcon::NotAllowed);
        }
        if let Some(h) = handle {
            if self.enter_transform()
                && let Some(src) = self.transform_source()
            {
                // From the lifted pixels' box, the handle following the
                // pointer.
                self.drag = Some(handles::RectDrag::new(h, src, view.to_image(p)));
                self.free_transform = true;
            }
            return Some(h.cursor(0.0));
        }
        let alt = self.ctx.input(|i| i.modifiers.alt);
        self.select_move = Some(SelectMove {
            from: view.to_image(p),
            selection,
            pixels: Some(alt),
            floating: None,
            moved: false,
        });
        Some(self.move_selection(view, pointer, down, shift))
    }

    /// The pending transform's state, for Ctrl+Z inside it, with the
    /// history step it is on.
    pub(super) fn snapshot(&self) -> (Snapshot, Option<u64>) {
        let x = &self.xform;
        let s = Snapshot {
            rect: self.transform_rect,
            angle: self.transform_angle,
            quad: x.quad,
            warp: x.warp,
            reference: x.reference,
            mode: x.mode,
            style: (x.style, x.bend, x.vertical),
            switch: false,
        };
        (s, self.doc.as_ref().map(Document::history_step))
    }

    /// After an edit of the pending transform: the state `before` it
    /// becomes a Ctrl+Z point. Edits of one `series` (arrow keys, a field
    /// dragged) less than a second apart share one.
    pub(super) fn transform_edited(
        &mut self,
        before: (Snapshot, Option<u64>),
        series: Option<&'static str>,
    ) {
        let (now, step) = self.snapshot();
        let (before, before_step) = before;
        // Applied (a warp on a change of mode) or left: nothing to go
        // back to inside it.
        if before.rect.is_none() || now.rect.is_none() || step != before_step || now == before {
            return;
        }
        let time = self.ctx.input(|i| i.time);
        let x = &mut self.xform;
        let same = series.is_some()
            && x.series
                .is_some_and(|(k, at)| Some(k) == series && time - at < 1.0);
        x.series = series.map(|k| (k, time));
        if !same && x.undo.last() != Some(&before) {
            x.undo.push(before);
        }
    }

    /// Ctrl+Z during Free Transform: back to before the last drag, still
    /// transforming. `false` when there is none (the transform is then
    /// cancelled).
    pub(super) fn undo_transform_edit(&mut self) -> bool {
        let now = self.snapshot().0;
        while let Some(s) = self.xform.undo.pop() {
            if s.switch {
                // Before the warp applied by a change of mode: it is given
                // up, and its grid comes back (the pixels lifted again).
                let Some(applied) = self.xform.switches.pop().or_else(|| self.xform.applied.take())
                else {
                    continue;
                };
                if let Some(doc) = &mut self.doc {
                    doc.rollback_to(applied.mark);
                }
                self.selection = applied.selection;
                if applied.lifted {
                    // Its floating layer went with the rollback.
                    self.xform.lift = None;
                    self.lift_selection();
                }
                self.restore_snapshot(s);
                self.xform.redo.clear();
                return true;
            }
            if s != now && s.rect.is_some() {
                self.restore_snapshot(s);
                self.xform.redo.push((now, s));
                return true;
            }
        }
        false
    }

    /// Ctrl+Shift+Z during Free Transform: the edit undone last comes
    /// back. `false` when there is none (or it was edited since).
    pub(super) fn redo_transform_edit(&mut self) -> bool {
        let now = self.snapshot().0;
        match self.xform.redo.pop() {
            Some((s, left)) if left == now => {
                self.restore_snapshot(s);
                self.xform.undo.push(now);
                true
            }
            _ => {
                self.xform.redo.clear();
                false
            }
        }
    }

    fn restore_snapshot(&mut self, s: Snapshot) {
        self.transform_rect = s.rect;
        self.transform_angle = s.angle;
        let x = &mut self.xform;
        (x.quad, x.warp) = (s.quad, s.warp);
        x.reference = s.reference;
        x.mode = s.mode;
        (x.style, x.bend, x.vertical) = s.style;
        x.series = None;
        (self.xform.drag, self.drag, self.rotate_drag) = (None, None, None);
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
            .filter(|&i| !doc.is_locked(i) && !inside(i))
            .collect()
    }

    /// Box of what the transform takes when it isn't the active element's:
    /// several layers' (`None` without any), or the mask's pixels when it
    /// is transformed alone (see `Document::transforms_mask`).
    pub(super) fn transform_source_override(&self) -> Option<Option<Rect>> {
        if let Some(source) = self.quick_mask_source() {
            return Some(source);
        }
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
            // applied, and Free Transform goes on from it: the two stay
            // pending together (one step on Enter, nothing left on Esc).
            if Warp::from_map(&frame) == Some(w) {
                self.xform.warp = None;
                self.xform.quad = quad;
            } else {
                // Each warp applied: Ctrl+Z can come back to it, with the
                // points before it.
                let before = Snapshot {
                    switch: true,
                    ..self.snapshot().0
                };
                self.xform.reenter_undo = Some((std::mem::take(&mut self.xform.undo), before));
                let Some(doc) = &self.doc else { return };
                // From before the pixels were lifted (with the selection
                // then); in Quick Mask Mode, the selection was set aside.
                let lift = self.xform.lift.as_ref();
                let selection = lift.map(|l| l.selection.clone());
                let this = Applied {
                    mark: lift.map_or_else(|| doc.history_mark(), |l| l.mark),
                    selection: selection
                        .or_else(|| self.xform.quick.selection.clone())
                        .or_else(|| self.selection.clone()),
                    lifted: lift.is_some(),
                };
                let first = self.xform.applied.take();
                self.xform.reenter = Some(mode);
                self.apply_transform();
                self.xform.applied = Some(match first {
                    Some(first) => {
                        self.xform.switches.push(this);
                        first
                    }
                    None => this,
                });
                // Large: once computed (see `transform_window`).
                if !self.transform_busy() {
                    self.reenter_transform();
                }
                return;
            }
        }
        self.xform.mode = mode;
    }

    /// After a warp applied on a change of mode: Free Transform again, in
    /// the mode chosen.
    fn reenter_transform(&mut self) {
        let Some(mode) = self.xform.reenter.take() else {
            return;
        };
        let applied = self.xform.applied.take();
        let undo = self.xform.reenter_undo.take();
        if !self.enter_transform() {
            // Nothing left to transform: the warp stays applied.
            return;
        }
        self.xform.applied = applied;
        self.xform.mode = mode;
        if let Some((mut points, before)) = undo.filter(|_| self.xform.applied.is_some()) {
            points.push(before);
            self.xform.undo = points;
        }
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
            Last::Map(h, from, made) => match again_map(h, from, made, b).as_turned_box(b) {
                Some((r, angle)) => {
                    self.transform_rect =
                        Some(Rect::from_min_max(pos([r[0], r[1]]), pos([r[2], r[3]])));
                    self.transform_angle = normalize_angle(angle as f32);
                }
                None => self.xform.quad = again_map(h, from, made, b).corners(b),
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
        let made = self.transform_source();
        self.xform.last = Some(last_map(turned_map(src, frame, angle), bounds(src), made));
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
        if !self.enter_transform() || !self.set_transform_frame(unit) {
            return false;
        }
        if !dialog {
            self.apply_transform();
        }
        true
    }

    /// The Free Transform in progress takes the corners of its box to
    /// `unit` (as recorded: see [`Self::recorded_frame`]).
    pub(super) fn set_transform_frame(&mut self, unit: [Pt; 4]) -> bool {
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
        true
    }

    /// Applies a Free Transform that the turned rectangle doesn't describe
    /// (distorted, warped), or of a mask, of lifted pixels or of several
    /// layers; one undo step. Returns `false` to leave it to the turned
    /// rectangle's way.
    pub(super) fn apply_transform_extras(&mut self) -> bool {
        let targets = self.transform_targets();
        let Some(doc) = &self.doc else { return false };
        // The quick mask: the gray image alone, whatever the layers.
        let quick = doc.quick_mask().is_some();
        let mask = !quick && targets.len() == 1 && doc.transforms_mask();
        let plain = !self.transform_distorted() && self.xform.lift.is_none();
        // A smart object is rendered again from its contents, on another
        // thread when large.
        let smart = !mask
            && targets
                .iter()
                .flat_map(|&i| doc.block(i))
                .any(|i| doc.layers[i].is_smart());
        // The texts of a group scaled or turned stay texts (the plain path
        // makes them pixels).
        let moved_only = self.transform_angle == 0.0
            && self
                .transform_rect
                .zip(self.transform_source())
                .is_none_or(|(r, s)| (r.size() - s.size()).length() < 0.5);
        let grouped_text = !mask
            && !moved_only
            && targets.iter().any(|&i| {
                doc.layers[i].is_group() && doc.block(i).any(|j| doc.layers[j].text.is_some())
            });
        // After a warp applied on the way, the commit joins it (see
        // `finish_transform`).
        let after_warp = self.xform.applied.is_some();
        // A large layer scaled or turned: resampled on another thread too
        // (seconds at 24 megapixels), a text kept as such the plain way.
        let large = !moved_only && targets.len() == 1 && {
            let l = &doc.layers[targets[0]];
            let (w, h) = l.image().dimensions();
            l.text.is_none() && !l.is_group() && u64::from(w) * u64::from(h) >= ASYNC_PIXELS
        };
        let alone = targets.len() == 1 && !quick;
        let simple = plain && !mask && !smart && !grouped_text && !after_warp && alone;
        if simple && !large {
            return false;
        }
        let placed = simple
            .then(|| self.transform_rect.map(round_rect))
            .flatten()
            .map(|r| (r.width().max(1.0) as u32, r.height().max(1.0) as u32, self.transform_angle));
        let Some(t) = self
            .transform_source()
            .and_then(|src| self.pending_transform(src, true))
        else {
            self.reset_transform();
            return true;
        };
        let Some(doc) = &self.doc else { return true };
        let target = if quick {
            TransformTarget::QuickMask
        } else if mask {
            TransformTarget::Mask
        } else {
            TransformTarget::Layer
        };
        let mut layers: Vec<usize> = match target {
            TransformTarget::Mask | TransformTarget::QuickMask => Vec::new(),
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
        let layer = if quick {
            t!("qm6-quick-mask-name")
        } else {
            doc.active_layer().name.clone()
        };
        let selection = match quick {
            true => self
                .xform
                .quick
                .selection
                .take()
                .or_else(|| self.selection.clone()),
            false => None,
        };
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
            selection,
            fit,
            canvas,
            placed,
        };
        if t.is_identity() {
            self.finish_transform(commit, None);
            return true;
        }
        let filter = self.resize_filter.filter();
        let Some(doc) = &self.doc else { return true };
        let job = match target {
            TransformTarget::QuickMask => {
                doc.quick_mask_job(&t, commit.selection.as_ref(), false, filter)
            }
            _ => doc.transform_job(target, &commit.pixels, &t, filter),
        };
        // Small and quick: at once (smart filters never are, said).
        if job.size() < ASYNC_PIXELS && !job.has_filters() {
            self.finish_transform(commit, Some(job.run()));
            return true;
        }
        // Large: computed on another thread, the frame shown meanwhile.
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Transform");
            let _ = tx.send(job.run());
            ctx.request_repaint();
        });
        // A warp applied on the way joins it once done (Enter clears the
        // pending state meanwhile).
        let applied = self.xform.applied.take();
        self.xform.applying = Some(Applying {
            rx,
            doc: doc.uid(),
            started: self.ctx.input(|i| i.time),
            commit,
            applied,
            then: Vec::new(),
        });
        true
    }

    /// Whether a transform is being computed (it keeps the keyboard and
    /// the image).
    pub(super) fn transform_busy(&self) -> bool {
        self.xform.applying.is_some()
    }

    /// Waits for the transform being computed and puts it in place: what
    /// follows (a command, a save, another tab) works with its result.
    pub(super) fn wait_transform(&mut self) {
        let Some(a) = self.xform.applying.take() else {
            return;
        };
        let done = a.rx.recv().ok();
        self.xform.reenter = None;
        self.land_transform(a, done);
    }

    /// A press, a key, a paste or a drop while a transform is computed on
    /// another thread: it is waited for and put in place first, as if it
    /// had landed by itself, then the input applies: nothing is lost.
    pub(super) fn wait_transform_for_input(&mut self, ctx: &egui::Context) {
        if self.xform.applying.is_none()
            || !ctx.input(|i| super::retouch_tools::input_waits(i, None))
        {
            return;
        }
        let Some(a) = self.xform.applying.take() else {
            return;
        };
        let done = a.rx.recv().ok();
        self.land_transform(a, done);
    }

    /// A transform computed aside, once done: in place if its document is
    /// still the one shown; then what was asked for meanwhile.
    fn land_transform(&mut self, a: Applying, done: Option<Vec<Transformed>>) {
        if a.applied.is_some() {
            self.xform.applied = a.applied;
        }
        if self.doc.as_ref().is_some_and(|d| d.uid() == a.doc) {
            self.finish_transform(a.commit, done);
            self.reenter_transform();
        } else {
            (self.xform.reenter, self.xform.applied) = (None, None);
            self.xform.switches.clear();
        }
        for then in a.then {
            then(self);
        }
    }

    /// Applies what is being prepared when it is a transform, without
    /// waiting for one computed on another thread (seconds at 24
    /// megapixels): whether one is. What comes next then goes through
    /// [`Self::after_transform`], the frames showing its indicator
    /// meanwhile, rather than the window freezing.
    pub(super) fn commit_pending_aside(&mut self) -> bool {
        if self.transform_rect.is_none() || self.crop_box_pending() {
            return false;
        }
        self.apply_pending(false);
        // Something else asked for: the transform isn't entered again.
        self.xform.reenter = None;
        self.xform.applying.is_some()
    }

    /// `then` once the transform being computed lands (at once without
    /// one).
    pub(super) fn after_transform(&mut self, then: impl FnOnce(&mut Self) + 'static) {
        match &mut self.xform.applying {
            Some(a) => a.then.push(Box::new(then)),
            None => then(self),
        }
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
                return busy_note(ctx, &a.commit.name, a.started);
            }
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let Some(a) = self.xform.applying.take() else {
            return;
        };
        self.land_transform(a, done);
    }

    /// Puts a transform in place with the pixels computed for it (`None`:
    /// it changes nothing), with its texts, lifted pixels, canvas and status.
    fn finish_transform(&mut self, c: Commit, done: Option<Vec<Transformed>>) {
        // A warp applied by a change of mode: the warps applied before it
        // stay pending, the transform goes on (see `reenter_transform`).
        if self.xform.reenter.is_some() {
            let kept = self.xform.applied.take();
            let switches = std::mem::take(&mut self.xform.switches);
            self.put_transform(c, done);
            (self.xform.applied, self.xform.switches) = (kept, switches);
            return;
        }
        self.put_transform(c, done);
    }

    /// [`Self::finish_transform`], everything pending with it applied.
    fn put_transform(&mut self, c: Commit, done: Option<Vec<Transformed>>) {
        // A warp applied on the way is kept, with this in the same step.
        let applied = self.xform.applied.take();
        self.xform.switches.clear();
        let Some(doc) = &mut self.doc else { return };
        let changed = done.is_some();
        let quick = c.target == TransformTarget::QuickMask;
        if quick {
            // The step keeps the selection as it was before.
            doc.sync_selection(c.selection.clone());
            self.selection = match changed {
                true => c
                    .selection
                    .as_ref()
                    .and_then(|s| transformed_selection(s, &c.t, c.canvas)),
                false => c.selection.clone(),
            };
        }
        if let Some(done) = done {
            doc.apply_transformed(c.target, &c.t, done, c.name.clone());
        }
        if quick {
            self.sync_selection();
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
        // What it made, for Transform Again (before the canvas fits it).
        let made = (changed && c.lift.is_none() && c.target == TransformTarget::Layer)
            .then(|| self.transform_source())
            .flatten();
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
        if let Some(applied) = applied {
            doc.join_since(applied.mark);
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
                    // What shows, not the transparent margins of the box.
                    .map(|&i| {
                        let l = &doc.layers[i];
                        l.content_bounds().unwrap_or(l.rect())
                    })
                    .reduce(PixelRect::union)
            })
            .flatten();
        if let Some(r) = fitted {
            doc.fit_canvas_to(r);
        }
        self.xform.last = Some(match c.t.map {
            Map::Projective(h) => last_map(h, c.t.src, made),
            Map::Warp(w) => Last::Warp(c.t.src, w),
        });
        let (name, layer) = (c.name.as_str(), c.layer.as_str());
        self.status = t!("xform-status-done", what = name, name = layer);
        if let Some((w, h, angle)) = c.placed {
            self.placed_status(c.layers[0], (w, h, angle), fitted.is_some());
        }
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

    /// A plain scale or turn computed aside: said as the plain path says
    /// it (see `apply_transform`), the layer going into the artboard it
    /// now is over.
    fn placed_status(&mut self, index: usize, (w, h, angle): (u32, u32, f32), fitted: bool) {
        let turned = match angle {
            0.0 => String::new(),
            a => t!("status-rotated", angle = i18n::short_decimal(f64::from(a))),
        };
        if fitted {
            self.status = t!("status-canvas-fit-element", width = w, height = h, rotated = turned);
            return;
        }
        let Some(layer) = self.doc.as_ref().and_then(|d| d.layers.get(index)) else {
            return;
        };
        let (id, r) = (layer.id, layer.content_bounds().unwrap_or(layer.rect()));
        self.status = t!(
            "status-element-placed",
            width = w,
            height = h,
            x = r.x,
            y = r.y,
            rotated = turned
        );
        self.artboards_after_move(&[id]);
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
        if self.transform_locked() {
            return None;
        }
        // Every press may start an edit of the pending transform.
        if start && self.transform_rect.is_some() {
            let now = self.snapshot().0;
            self.xform.series = None;
            if self.xform.undo.last() != Some(&now) {
                self.xform.undo.push(now);
            }
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
            // Rotate mode: inside, the frame still moves.
            Some(Grab::Move) if self.xform.mode == Mode::Rotate && !distorted => None,
            Some(Grab::Move) if self.xform.mode == Mode::Rotate => Some(Grab::Move),
            _ if self.xform.mode == Mode::Rotate => Some(Grab::Rotate),
            Some(g @ (Grab::Corner(_) | Grab::Edge(_)))
                if edit.is_some() || distorted || (alt && pivot_moved) =>
            {
                Some(g)
            }
            Some(Grab::Rotate) if distorted || pivot_moved => Some(Grab::Rotate),
            // Alt+click: the reference point there (in Free Transform; the
            // Move tool's Alt+drag moves a copy).
            Some(Grab::Move) | None
                if alt && !ctrl && self.transform_rect.is_some() && self.free_transform =>
            {
                Some(Grab::Reference)
            }
            Some(Grab::Move) if distorted => Some(Grab::Move),
            // Outside the frame: turns it.
            None if distorted && self.xform.mode != Mode::Scale => Some(Grab::Rotate),
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

    /// A transform was applied: Edit › Transform › Again can repeat it.
    pub(super) fn has_last_transform(&self) -> bool {
        self.xform.last.is_some()
    }

    /// The Edit › Transform submenu.
    pub(super) fn transform_menu(&self, has_doc: bool) -> Vec<MenuEntry> {
        let again = has_doc && self.has_last_transform();
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
        if self.in_quick_mask() {
            self.xform.mask_preview = None;
            self.xform.lifted.clear();
            self.preview_quick_mask(frame, visible);
            return Vec::new();
        }
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

/// Canvas box of the pixels of the active layer `selection` lifts (the
/// floating layer's content).
fn selected_pixels_bounds(doc: &Document, selection: &Selection) -> Option<PixelRect> {
    let region = selection.bounds(doc.full_rect())?;
    let pixels = doc.active_pixels(Some(selection))?;
    // Each row's first and last pixel shown, rows on all the cores.
    use rayon::prelude::*;
    let w = pixels.width() as usize;
    let (y0, y1, x0, x1) = pixels
        .par_chunks_exact(w.max(1) * 4)
        .enumerate()
        .filter_map(|(y, row)| {
            let shown = |p: &[u8]| p[3] > 0;
            let first = row.chunks_exact(4).position(shown)?;
            let last = row.chunks_exact(4).rposition(shown)?;
            Some((y as u32, y as u32, first as u32, last as u32))
        })
        .reduce_with(|a, b| (a.0.min(b.0), a.1.max(b.1), a.2.min(b.2), a.3.max(b.3)))?;
    Some(PixelRect {
        x: region.x + i64::from(x0),
        y: region.y + i64::from(y0),
        w: x1 - x0 + 1,
        h: y1 - y0 + 1,
    })
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

