//! Crop tool: its options (ratio presets, overlay, shield, Delete Cropped
//! Pixels), the turned box (a drag outside it, Straighten) and how it is
//! applied. The box stays in `canvas_rect`, turned by [`CropTool::angle`]
//! around its center: applying turns the image the other way, and keeps
//! what the box covered.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::canvas::{draw_frame, rotate_around};
use super::*;
use crate::actions::Recorded;
use crate::document::{CropOrder, WholeChange, WholeDone};
use crate::i18n::t;
use crate::text::TextData;

/// The presets of the options bar's first list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CropPreset {
    /// W and H are a ratio, or free when empty.
    Ratio,
    /// W × H pixels at a resolution: the result is resampled to that size.
    Size,
    Original,
    Square,
    R4x5,
    R5x7,
    R2x3,
    R16x9,
}

impl CropPreset {
    pub(super) const ALL: [Self; 8] = [
        Self::Ratio,
        Self::Size,
        Self::Original,
        Self::Square,
        Self::R4x5,
        Self::R5x7,
        Self::R2x3,
        Self::R16x9,
    ];

    pub(super) fn label(self) -> String {
        match self {
            Self::Ratio => t!("crop8-preset-ratio"),
            Self::Size => t!("crop8-preset-size"),
            Self::Original => t!("crop8-preset-original"),
            Self::Square => format!("1:1 ({})", t!("crop8-preset-square")),
            Self::R4x5 => "4:5 (8:10)".to_owned(),
            Self::R5x7 => "5:7".to_owned(),
            Self::R2x3 => "2:3 (4:6)".to_owned(),
            Self::R16x9 => "16:9".to_owned(),
        }
    }

    fn values(self, canvas: (u32, u32)) -> Option<(f32, f32)> {
        match self {
            Self::Ratio | Self::Size => None,
            Self::Original => Some((canvas.0 as f32, canvas.1 as f32)),
            Self::Square => Some((1.0, 1.0)),
            Self::R4x5 => Some((4.0, 5.0)),
            Self::R5x7 => Some((5.0, 7.0)),
            Self::R2x3 => Some((2.0, 3.0)),
            Self::R16x9 => Some((16.0, 9.0)),
        }
    }
}

/// The lines drawn over the box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CropOverlay {
    Thirds,
    Grid,
    /// Lines at 45° from each corner.
    Diagonal,
    /// A diagonal, and the perpendiculars to it from the other corners.
    Triangle,
    /// Thirds at the golden ratio (0.382, 0.618).
    GoldenRatio,
    Never,
}

impl CropOverlay {
    pub(super) const ALL: [Self; 6] = [
        Self::Thirds,
        Self::Grid,
        Self::Diagonal,
        Self::Triangle,
        Self::GoldenRatio,
        Self::Never,
    ];

    pub(super) fn label(self) -> String {
        match self {
            Self::Thirds => t!("crop8-overlay-thirds"),
            Self::Grid => t!("crop8-overlay-grid"),
            Self::Diagonal => t!("crop11-overlay-diagonal"),
            Self::Triangle => t!("crop11-overlay-triangle"),
            Self::GoldenRatio => t!("crop11-overlay-golden-ratio"),
            Self::Never => t!("crop8-overlay-none"),
        }
    }

    /// Its lines in the box, from corner (0, 0) to (1, 1), for a box of
    /// width / height `aspect`.
    pub(super) fn lines(self, aspect: f32) -> Vec<[(f32, f32); 2]> {
        let parts = |fractions: &[f32]| {
            (fractions.iter())
                .flat_map(|&f| [[(f, 0.0), (f, 1.0)], [(0.0, f), (1.0, f)]])
                .collect()
        };
        match self {
            Self::Thirds => parts(&[1.0 / 3.0, 2.0 / 3.0]),
            Self::Grid => parts(&(1..8).map(|i| i as f32 / 8.0).collect::<Vec<_>>()),
            Self::GoldenRatio => {
                let phi = 1.0 - 1.0 / 1.618_034;
                parts(&[phi, 1.0 - phi])
            }
            Self::Diagonal => {
                // As far as 45° goes: the shorter side, in each direction.
                let (u, v) = match aspect >= 1.0 {
                    true => (1.0 / aspect, 1.0),
                    false => (1.0, aspect),
                };
                vec![
                    [(0.0, 0.0), (u, v)],
                    [(1.0, 0.0), (1.0 - u, v)],
                    [(0.0, 1.0), (u, 1.0 - v)],
                    [(1.0, 1.0), (1.0 - u, 1.0 - v)],
                ]
            }
            Self::Triangle => {
                // The feet of the perpendiculars, along the diagonal.
                let (w2, h2) = (aspect * aspect, 1.0);
                let (a, b) = (w2 / (w2 + h2), h2 / (w2 + h2));
                vec![[(0.0, 0.0), (1.0, 1.0)], [(1.0, 0.0), (a, a)], [(0.0, 1.0), (b, b)]]
            }
            Self::Never => Vec::new(),
        }
    }
}

/// Crop tool's state, besides the box itself.
pub(super) struct CropTool {
    pub(super) preset: CropPreset,
    /// The W and H fields: a ratio, or pixels with [`CropPreset::Size`];
    /// 0 is empty.
    pub(super) w: f32,
    pub(super) h: f32,
    /// Pixels per inch given to the result by [`CropPreset::Size`]; 0: as
    /// it is.
    pub(super) resolution: f32,
    pub(super) overlay: CropOverlay,
    /// How dark what the box leaves out is (0..=1).
    pub(super) shield: f32,
    pub(super) delete_cropped: bool,
    /// The box's turn (degrees, clockwise), in -180..=180.
    pub(super) angle: f32,
    /// Straighten: the next drag draws the line to level.
    pub(super) straighten: bool,
    /// That line, in canvas coordinates.
    line: Option<(Pos2, Pos2)>,
    /// The box as it was when its turn started: it never grows back past
    /// this, and shrinks from it to stay on the image.
    turned_from: Option<Rect>,
    /// The box that turn made: moved or resized by hand since, it starts
    /// again from there.
    turned_to: Option<Rect>,
    /// The change of the whole document being computed aside.
    pub(super) whole: Option<WholeRun>,
    /// The window's close waits for a change computed aside.
    close_waits: bool,
    /// Input given while a change is computed aside.
    held: HeldInput,
}

/// Input given while a change is computed aside, held back rather than the
/// window waiting for it (see `AtelierApp::hold_input`), and given back
/// once it lands, frame by frame, as if given then.
#[derive(Default)]
struct HeldInput {
    frames: std::collections::VecDeque<(Vec<egui::Event>, Vec<egui::DroppedFileHandle>)>,
    /// A held press, its release not given yet: all of the pointer waits
    /// with it.
    down: bool,
    /// A press on the job's indicator: its release is its own too.
    on_indicator: bool,
    /// The modifiers as last told, given again after what was held.
    modifiers: Option<Modifiers>,
}

/// Something to do once a change lands.
type Then = Box<dyn FnOnce(&mut AtelierApp)>;

/// A change of the whole document being computed aside: not modal, input
/// waits for it (see `wait_whole_for_input`).
pub(super) struct WholeRun {
    rx: std::sync::mpsc::Receiver<Option<WholeDone>>,
    cancel: Arc<AtomicBool>,
    doc: u64,
    step: u64,
    started: f64,
    landing: Landing,
    /// What was asked for meanwhile, once it lands.
    then: Vec<Then>,
    /// Where its window shows: a press there isn't input to wait for.
    shown: Option<Rect>,
    /// Its window was laid out once: its first frame only measures it, out
    /// of sight and elsewhere.
    laid_out: bool,
}

impl WholeRun {
    /// `then` once it lands.
    pub(super) fn then_do(&mut self, then: impl FnOnce(&mut AtelierApp) + 'static) {
        self.then.push(Box::new(then));
    }

    /// Its indicator on the next frame.
    pub(super) fn show_at_once(&mut self) {
        self.started = f64::NEG_INFINITY;
    }

    #[cfg(test)]
    pub(super) fn shown_rect(&self) -> Option<Rect> {
        self.shown
    }

    #[cfg(test)]
    pub(super) fn showing(&self) -> bool {
        self.shown.is_some()
    }
}

/// What a change of the whole document does once computed.
struct Landing {
    name: String,
    /// Its name in Diagnostics, in English (see `diagnostics::static_operation`).
    job: &'static str,
    texts: Vec<(u64, TextData, (i64, i64))>,
    recorded: Option<Recorded>,
    kind: LandingKind,
}

enum LandingKind {
    Rotate(f32),
    Flip,
    Crop {
        /// The view keeps showing what the box showed.
        shift: Vec2,
        /// The box, back if the crop is cancelled.
        back: Option<(Rect, f32)>,
    },
}

impl Default for CropTool {
    fn default() -> Self {
        Self {
            preset: CropPreset::Ratio,
            w: 0.0,
            h: 0.0,
            resolution: 0.0,
            overlay: CropOverlay::Thirds,
            shield: 0.75,
            delete_cropped: false,
            angle: 0.0,
            straighten: false,
            line: None,
            turned_from: None,
            turned_to: None,
            whole: None,
            close_waits: false,
            held: HeldInput::default(),
        }
    }
}

impl CropTool {
    /// Width / height the box must keep, if any.
    pub(super) fn ratio(&self) -> Option<f32> {
        (self.w > 0.0 && self.h > 0.0).then(|| self.w / self.h)
    }

    /// Forgets what belongs to the box being prepared.
    pub(super) fn reset_box(&mut self) {
        self.angle = 0.0;
        self.straighten = false;
        self.line = None;
        self.turned_from = None;
        self.turned_to = None;
    }
}

/// How a crop is applied: the box in the canvas, turned by `angle`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct CropPlan {
    pub(super) rect: PixelRect,
    pub(super) angle: f32,
    pub(super) delete: bool,
    pub(super) size: Option<(u32, u32)>,
    pub(super) resolution: f32,
}

impl CropPlan {
    /// Image › Crop and its kin: the rectangle, nothing else.
    pub(super) fn plain(rect: PixelRect) -> Self {
        Self {
            rect,
            angle: 0.0,
            delete: false,
            size: None,
            resolution: 0.0,
        }
    }
}

/// The largest box of width/height `ratio` in `area`, around its center.
pub(super) fn fit_ratio(area: Rect, ratio: f32) -> Rect {
    let (mut w, mut h) = (area.width(), area.width() / ratio);
    if h > area.height() {
        (w, h) = (area.height() * ratio, area.height());
    }
    round_rect(Rect::from_center_size(
        area.center(),
        vec2(w.max(1.0), h.max(1.0)),
    ))
}

/// The smallest box of width/height `ratio` holding `area`, around its
/// center (it may reach past the canvas, as a box drawn by hand can).
pub(super) fn around_ratio(area: Rect, ratio: f32) -> Rect {
    let (mut w, mut h) = (area.width(), area.width() / ratio);
    if h < area.height() {
        (w, h) = (area.height() * ratio, area.height());
    }
    // Rounded outward: nothing of `area` is left out.
    let r = Rect::from_center_size(area.center(), vec2(w.max(1.0), h.max(1.0)));
    let (w, h) = (r.width().ceil(), r.height().ceil());
    Rect::from_min_size(
        pos2((r.center().x - w / 2.0).floor(), (r.center().y - h / 2.0).floor()),
        vec2(w, h),
    )
}

/// `from` shrunk around its center, its shape kept, so that turned by
/// `degrees` it stays within `image`. A box reaching past the image (the
/// canvas being enlarged) stays as it is.
pub(super) fn fit_turned(from: Rect, degrees: f32, image: Rect) -> Rect {
    if !image.contains_rect(from) {
        return from;
    }
    let (hw, hh) = (from.width() / 2.0, from.height() / 2.0);
    let (s, c) = degrees.to_radians().sin_cos();
    let (s, c) = (s.abs(), c.abs());
    let (reach_x, reach_y) = (hw * c + hh * s, hw * s + hh * c);
    let center = from.center();
    let room_x = (center.x - image.min.x).min(image.max.x - center.x);
    let room_y = (center.y - image.min.y).min(image.max.y - center.y);
    let scale = (room_x / reach_x.max(1e-3))
        .min(room_y / reach_y.max(1e-3))
        .min(1.0);
    if scale >= 1.0 {
        return from;
    }
    // Rounded inward, a pixel to spare: its corners stay on the image.
    let (w, h) = (from.width() * scale - 1.0, from.height() * scale - 1.0);
    let (w, h) = (w.floor().max(1.0), h.floor().max(1.0));
    Rect::from_min_size(
        pos2((center.x - w / 2.0).round(), (center.y - h / 2.0).round()),
        vec2(w, h),
    )
}

/// The turn that levels a line from `from` to `to`: in -45..=45, to its
/// nearest axis.
pub(super) fn straighten_angle(from: Pos2, to: Pos2) -> f32 {
    let d = to - from;
    let a = d.y.atan2(d.x).to_degrees();
    (a + 45.0).rem_euclid(90.0) - 45.0
}

/// The corners of `r` turned by `degrees` around its center, clockwise from
/// the top left.
pub(super) fn turned_corners(r: Rect, degrees: f32) -> [Pos2; 4] {
    [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()]
        .map(|p| rotate_around(p, r.center(), degrees))
}

/// What is left of a convex polygon on the positive side of the line from
/// `a` to `b` (`keep` > 0), or on its negative side.
fn clipped_by_line(polygon: &[Pos2], a: Pos2, b: Pos2, keep: f32) -> Vec<Pos2> {
    let side = |p: Pos2| keep * (b - a).dot((p - a).rot90());
    let mut out = Vec::with_capacity(polygon.len() + 1);
    for (i, &p) in polygon.iter().enumerate() {
        let q = polygon[(i + 1) % polygon.len()];
        let (sp, sq) = (side(p), side(q));
        if sp >= 0.0 {
            out.push(p);
        }
        if (sp > 0.0 && sq < 0.0) || (sp < 0.0 && sq > 0.0) {
            out.push(p.lerp(q, sp / (sp - sq)));
        }
    }
    out
}

/// `frame` without the convex quad `hole` (corners in order), filled: each
/// side of the hole takes what lies beyond it of what the previous ones left,
/// so the pieces neither overlap nor enter the hole, at any turn, even when
/// the hole reaches past the frame.
pub(super) fn shield_mesh(frame: Rect, hole: [Pos2; 4], color: Color32) -> egui::epaint::Mesh {
    let mut mesh = egui::epaint::Mesh::default();
    let mut left = vec![
        frame.left_top(),
        frame.right_top(),
        frame.right_bottom(),
        frame.left_bottom(),
    ];
    // Which side of its sides the hole lies on.
    let (a, b, c) = (hole[0], hole[1], hole[2]);
    let inner = (b - a).dot((c - a).rot90()).signum();
    for i in 0..4 {
        let (a, b) = (hole[i], hole[(i + 1) % 4]);
        let piece = clipped_by_line(&left, a, b, -inner);
        let base = mesh.vertices.len() as u32;
        for &p in &piece {
            mesh.colored_vertex(p, color);
        }
        for k in 2..piece.len() as u32 {
            mesh.add_triangle(base, base + k - 1, base + k);
        }
        left = clipped_by_line(&left, a, b, inner);
    }
    mesh
}

impl AtelierApp {
    /// What the view shows with the Crop tool: the canvas and the box.
    pub(super) fn crop_view_frame(&self, full: Rect) -> Rect {
        let Some((r, angle)) = self.crop_box_shown() else {
            return full;
        };
        let (w, h) = rotated_size(r.width(), r.height(), angle);
        let reach = Rect::from_center_size(r.center(), vec2(w, h));
        let reach = Rect::from_min_max(
            pos2(reach.min.x.floor(), reach.min.y.floor()),
            pos2(reach.max.x.ceil(), reach.max.y.ceil()),
        );
        full.union(reach)
    }

    /// The box drawn, or the one being applied aside (shown until it lands).
    pub(super) fn crop_box_shown(&self) -> Option<(Rect, f32)> {
        let applied = self.crop.whole.as_ref().and_then(|run| match run.landing.kind {
            LandingKind::Crop { back, .. } => back,
            _ => None,
        });
        self.canvas_rect.map(|r| (r, self.crop.angle)).or(applied)
    }

    /// Whether the box differs from the plain canvas.
    pub(super) fn crop_box_drawn(&self, full: Rect) -> bool {
        self.crop.angle != 0.0 || self.canvas_rect.is_some_and(|r| r != full)
    }

    /// The shield, the overlay, the frame and the line being drawn.
    pub(super) fn draw_crop(&self, painter: &Painter, view: View, frame: Rect, full: Rect) {
        let (r, angle) = self.crop_box_shown().unwrap_or((full, self.crop.angle));
        let drawn = angle != 0.0 || r != full;
        let on_screen = |corners: [Pos2; 4]| corners.map(|p| view.to_screen(p));
        let hole = on_screen(turned_corners(r, angle));
        if drawn && self.crop.shield > 0.0 {
            let dark = Color32::from_black_alpha((self.crop.shield * 255.0).round() as u8);
            let frame_screen = view.rect_to_screen(frame);
            painter.add(egui::Shape::mesh(shield_mesh(frame_screen, hole, dark)));
        }
        if frame != full {
            // The current canvas, faint.
            let faint = UiStroke::new(1.0, ui_kit::color::LINE_HOVER);
            let canvas = on_screen(turned_corners(full, 0.0)).to_vec();
            painter.add(egui::Shape::closed_line(canvas, faint));
        }
        let working = self.drag.is_some() || self.rotate_drag.is_some() || self.crop_draw.is_some();
        if drawn || working {
            self.draw_crop_overlay(painter, hole);
        }
        let dims = (r.width() as u32, r.height() as u32);
        draw_frame(painter, view.rect_to_screen(r), angle, false, dims);
        if let Some((from, to)) = self.crop.line {
            let (from, to) = (view.to_screen(from), view.to_screen(to));
            painter.line_segment([from, to], UiStroke::new(3.0, Color32::from_black_alpha(160)));
            painter.line_segment([from, to], UiStroke::new(1.5, ui_kit::color::ACCENT));
        }
    }

    /// Rule of Thirds or Grid, in the box (`corners` on screen).
    fn draw_crop_overlay(&self, painter: &Painter, [nw, ne, _, sw]: [Pos2; 4]) {
        let stroke = UiStroke::new(1.0, Color32::from_white_alpha(110));
        let aspect = nw.distance(ne) / nw.distance(sw).max(1e-3);
        let at = |(u, v): (f32, f32)| nw + (ne - nw) * u + (sw - nw) * v;
        for [a, b] in self.crop.overlay.lines(aspect) {
            painter.line_segment([at(a), at(b)], stroke);
        }
    }

    /// Width / height the box must keep, if any: Original Ratio is the
    /// current document's.
    pub(super) fn crop_ratio(&self) -> Option<f32> {
        match (self.crop.preset, &self.doc) {
            (CropPreset::Original, Some(d)) => Some(d.width() as f32 / d.height().max(1) as f32),
            _ => self.crop.ratio(),
        }
    }

    /// Original Ratio's fields: the current document's size.
    pub(super) fn crop_sync_original(&mut self) {
        if let (CropPreset::Original, Some(d)) = (self.crop.preset, &self.doc) {
            (self.crop.w, self.crop.h) = (d.width() as f32, d.height() as f32);
        }
    }

    /// W × H × Resolution asks for more than the box as it is.
    fn crop_resamples(&self, r: Rect) -> bool {
        if self.crop.preset != CropPreset::Size || self.crop.ratio().is_none() {
            return false;
        }
        let density = self.doc.as_ref().and_then(|d| d.file_metadata.image.as_ref()?.density);
        let ppi = f64::from(self.crop.resolution);
        (self.crop.w.round(), self.crop.h.round()) != (r.width(), r.height())
            || (ppi > 0.0 && density != Some((ppi, ppi)))
    }

    /// A crop box drawn or asking for a change, not applied yet: what Enter,
    /// Apply and "Crop the image?" all go by.
    pub(super) fn crop_box_pending(&self) -> bool {
        self.tool == Tool::Canvas
            && self.doc.is_some()
            && self.canvas_rect.is_some_and(|r| {
                Some(r) != self.full_rect() || self.crop.angle != 0.0 || self.crop_resamples(r)
            })
    }

    /// The box as a drag on it starts from: without one yet, what the ratio
    /// makes of the canvas.
    pub(super) fn crop_start_rect(&self, rect: Rect) -> Rect {
        match (self.canvas_rect, self.crop_ratio()) {
            (None, Some(ratio)) => fit_ratio(rect, ratio),
            _ => rect,
        }
    }

    /// Starts turning the box, the pointer leaving it.
    pub(super) fn crop_turn_starts(&mut self, rect: Rect) {
        // The box as the last turn left it: that turn goes on from where it
        // started, and the box can grow back.
        if self.crop.turned_from.is_none() || self.crop.turned_to != self.canvas_rect {
            self.crop.turned_from = Some(self.crop_start_rect(rect));
        }
    }

    /// The box turned to `degrees`: it stays on the image, and the turned
    /// canvas must fit in a layer.
    pub(super) fn crop_turn_to(&mut self, degrees: f32, full: Rect) {
        let from = self.crop.turned_from.or(self.canvas_rect).unwrap_or(full);
        if !fits_max_dimension(full.size(), degrees) {
            self.status_cannot(t!("crop9-status-too-large"));
            return;
        }
        self.crop.angle = degrees;
        self.canvas_rect = Some(fit_turned(from, degrees, full));
        self.crop.turned_to = self.canvas_rect;
    }

    /// Straighten: a line is drawn, then the box levels with it.
    pub(super) fn crop_straighten_input(
        &mut self,
        view: View,
        full: Rect,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) -> CursorIcon {
        if start && let Some(p) = pointer {
            let at = view.to_image(p);
            self.crop.line = Some((at, at));
        }
        if let (Some((from, _)), Some(p), true) = (self.crop.line, pointer, down) {
            self.crop.line = Some((from, view.to_image(p)));
        }
        if !down && let Some((from, to)) = self.crop.line.take() {
            if (to - from).length() * view.zoom >= canvas::CROP_SLIP {
                self.crop_turn_starts(self.canvas_rect.unwrap_or(full));
                self.crop_turn_to(straighten_angle(from, to), full);
            }
            self.crop.straighten = false;
        }
        CursorIcon::Crosshair
    }

    /// A ratio or preset chosen: the box takes that shape, as large as the
    /// image allows.
    pub(super) fn crop_reshape(&mut self, full: Rect) {
        let Some(ratio) = self.crop_ratio() else {
            return;
        };
        let angle = self.crop.angle;
        self.canvas_rect = Some(fit_turned(fit_ratio(full, ratio), angle, full));
    }

    pub(super) fn crop_pick_preset(&mut self, preset: CropPreset, full: Rect) {
        let was = self.crop.preset;
        if preset == CropPreset::Size {
            // The box stays and its size is what comes out, until W or H are
            // typed; a ratio makes its box first. Never its values as pixels.
            if was != CropPreset::Size {
                let r = self.crop_start_rect(self.canvas_rect.unwrap_or(full));
                self.canvas_rect = Some(r);
                (self.crop.w, self.crop.h) = (r.width(), r.height());
            }
            self.crop.preset = preset;
            return;
        }
        self.crop.preset = preset;
        let canvas = (full.width() as u32, full.height() as u32);
        match preset.values(canvas) {
            Some((w, h)) => (self.crop.w, self.crop.h) = (w, h),
            None => (self.crop.w, self.crop.h) = (0.0, 0.0),
        }
        self.crop_reshape(full);
    }

    /// Fit to Visible Pixels: the box around them; a ratio still holds.
    pub(super) fn crop_fit_visible(&mut self) {
        let Some(b) = self.doc.as_ref().and_then(Document::content_bounds) else {
            return;
        };
        let bounds = to_rect(b);
        self.canvas_rect = Some(match self.crop_ratio() {
            Some(ratio) => around_ratio(bounds, ratio),
            None => bounds,
        });
        self.crop.angle = 0.0;
        self.crop.turned_from = None;
        self.crop.turned_to = None;
    }

    /// W or H typed: a named preset no longer applies.
    pub(super) fn crop_values_edited(&mut self, full: Rect) {
        if self.crop.preset != CropPreset::Size {
            self.crop.preset = CropPreset::Ratio;
        }
        self.crop_reshape(full);
    }

    /// The swap button: W and H trade places, and the box its sides.
    pub(super) fn crop_swap(&mut self, full: Rect) {
        self.crop_sync_original();
        std::mem::swap(&mut self.crop.w, &mut self.crop.h);
        if self.crop.ratio().is_some() {
            if self.crop.preset == CropPreset::Original {
                self.crop.preset = CropPreset::Ratio;
            }
            return self.crop_values_edited(full);
        }
        let r = self.canvas_rect.unwrap_or(full);
        let turned = Rect::from_center_size(r.center(), vec2(r.height(), r.width()));
        // Too tall for the image: its largest box of the swapped shape.
        let fits = if full.contains_rect(turned) {
            turned
        } else {
            fit_ratio(full, r.height() / r.width())
        };
        self.canvas_rect = Some(fit_turned(fits, self.crop.angle, full));
    }

    /// The Clear button: free again, the box as it is.
    pub(super) fn crop_clear(&mut self) {
        self.crop.preset = CropPreset::Ratio;
        (self.crop.w, self.crop.h, self.crop.resolution) = (0.0, 0.0, 0.0);
    }

    /// The box as the options make it apply.
    pub(super) fn crop_plan(&self) -> Option<CropPlan> {
        let rect = to_pixel_rect(round_rect(self.canvas_rect?));
        let size = (self.crop.preset == CropPreset::Size && self.crop.ratio().is_some())
            .then(|| (self.crop.w.round() as u32, self.crop.h.round() as u32));
        Some(CropPlan {
            rect,
            angle: self.crop.angle,
            delete: self.crop.delete_cropped,
            size,
            resolution: if size.is_some() {
                self.crop.resolution
            } else {
                0.0
            },
        })
    }

    /// Applies the Crop tool's box, as a recorded step.
    pub(super) fn apply_crop_box(&mut self) {
        let Some(plan) = self.crop_plan() else { return };
        let recorded = self.crop_step(plan);
        let back = self.canvas_rect.take().map(|r| (r, self.crop.angle));
        self.start_crop(plan, recorded, back);
    }

    /// A crop as one step: turned first if it must be, cut, then resampled.
    pub(super) fn apply_crop(&mut self, plan: CropPlan) {
        self.start_crop(plan, None, None);
    }

    fn start_crop(&mut self, plan: CropPlan, recorded: Option<Recorded>, back: Option<(Rect, f32)>) {
        let r = plan.rect;
        if r.w == 0 || r.h == 0 {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let center = pos2(doc.width() as f32, doc.height() as f32) / 2.0;
        let order = CropOrder {
            rect: r,
            angle: plan.angle,
            delete: plan.delete,
            size: plan.size,
            resolution: plan.resolution,
            filter: self.resize_filter.filter(),
        };
        self.start_whole(
            WholeChange::Crop(order),
            Landing {
                name: t!("menu-image-crop"),
                job: "Crop",
                texts: self.texts_before(),
                recorded,
                kind: LandingKind::Crop {
                    shift: to_rect(r).center() - center,
                    back,
                },
            },
        );
    }

    /// Image Rotation, on [`Self::start_whole`]'s way.
    pub(super) fn start_rotation(&mut self, degrees: f32) {
        let Some(full) = self.full_rect() else { return };
        if !degrees.is_finite() || !fits_max_dimension(full.size(), degrees) {
            self.status_cannot(t!("crop9-status-too-large"));
            return;
        }
        self.start_whole(
            WholeChange::Rotate(degrees),
            Landing {
                name: t!("xchg-hist-rotate-canvas"),
                job: "Image Rotation",
                texts: self.texts_before(),
                recorded: Some(Recorded::RotateCanvas { angle: degrees }),
                kind: LandingKind::Rotate(degrees),
            },
        );
    }

    /// Flip Canvas, on [`Self::start_whole`]'s way (big tiles take their
    /// time to mirror).
    pub(super) fn start_flip(&mut self, horizontal: bool) {
        if self.doc.is_none() {
            return;
        }
        let name = match horizontal {
            true => t!("menu-image-flip-horizontal"),
            false => t!("menu-image-flip-vertical"),
        };
        self.start_whole(
            WholeChange::Flip(horizontal),
            Landing {
                name,
                job: match horizontal {
                    true => "Flip Canvas Horizontal",
                    false => "Flip Canvas Vertical",
                },
                texts: self.texts_before(),
                recorded: None,
                kind: LandingKind::Flip,
            },
        );
    }

    /// Runs a change of the whole document: at once when small, otherwise
    /// aside, with its indicator and Cancel.
    fn start_whole(&mut self, change: WholeChange, landing: Landing) {
        self.wait_whole();
        let Some(doc) = &self.doc else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        let job = doc.whole_job(change, cancel.clone());
        let (uid, step) = (doc.uid(), doc.history_step());
        let (tx, rx) = std::sync::mpsc::channel();
        let run = WholeRun {
            rx,
            cancel,
            doc: uid,
            step,
            started: self.ctx.input(|i| i.time),
            landing,
            then: Vec::new(),
            shown: None,
            laid_out: false,
        };
        if job.size() < transform::ASYNC_PIXELS && !job.has_filters() {
            let done = job.run();
            return self.land_whole(run, done);
        }
        let ctx = self.ctx.clone();
        let label = run.landing.job;
        std::thread::spawn(move || {
            let _job = crate::diag::job(label);
            let _ = tx.send(job.run());
            ctx.request_repaint();
        });
        self.crop.whole = Some(run);
    }

    /// Whether the indicator of a change computed aside is at `p` (screen).
    pub(super) fn busy_indicator_at(&self, p: Pos2) -> bool {
        self.crop.whole.as_ref().and_then(|run| run.shown).is_some_and(|r| r.contains(p))
    }

    /// The window's close asked while a change is computed aside (the
    /// document isn't modified yet): once it lands, the close goes on as
    /// Quit does, asking about unsaved changes. Cancelled, the window stays.
    pub(super) fn close_after_job(&mut self) {
        if std::mem::replace(&mut self.crop.close_waits, true) {
            return;
        }
        self.after_transform(|app| {
            app.crop.close_waits = false;
            app.run(Action::Quit);
        });
    }

    /// Before a frame: while a change is computed aside, what would act on
    /// the document (presses, keys, text, drops) is held back, its
    /// indicator showing at once, and given back once it lands, in order:
    /// nothing is lost and the window never waits. Esc and the indicator's
    /// own presses go through (they cancel it), as do the pointer's moves
    /// and the wheel when no held press is down.
    pub(super) fn hold_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let busy = self.transform_busy();
        let held = &mut self.crop.held;
        if !busy && held.frames.is_empty() {
            return;
        }
        let (cancellable, shown) = match &self.crop.whole {
            Some(run) => (true, run.shown),
            None => (false, None),
        };
        let mut events = Vec::new();
        raw.events.retain(|e| {
            use egui::Event as E;
            let through = match e {
                E::ModifiersChanged(m) => {
                    held.modifiers = Some(*m);
                    true
                }
                E::Key {
                    key: Key::Escape, ..
                } if busy && cancellable && !held.down => true,
                E::PointerButton { pressed: true, pos, .. } => {
                    let own = busy && !held.down && shown.is_some_and(|r| r.contains(*pos));
                    if own {
                        held.on_indicator = true;
                    } else {
                        // Where it was pressed, before the press.
                        events.push(E::PointerMoved(*pos));
                        held.down = true;
                    }
                    own
                }
                E::PointerButton { pressed: false, .. } => {
                    if std::mem::take(&mut held.on_indicator) {
                        true
                    } else {
                        held.down = false;
                        false
                    }
                }
                E::PointerMoved(_) | E::MouseMoved(_) | E::PointerGone | E::MouseWheel { .. }
                | E::Zoom(_) | E::Rotate(_) => !held.down,
                E::Key { .. }
                | E::Text(_)
                | E::Paste(_)
                | E::Copy
                | E::Cut
                | E::Ime(_)
                | E::Touch { .. } => false,
                _ => true,
            };
            if !through {
                events.push(e.clone());
            }
            through
        });
        let dropped = std::mem::take(&mut raw.dropped_files);
        if !events.is_empty() || !dropped.is_empty() {
            held.frames.push_back((events, dropped));
        }
        if busy {
            if !held.frames.is_empty() {
                // Held: the indicator says why at once.
                self.show_busy_at_once();
            }
            return;
        }
        // Landed: one frame of what was held at a time, before this one's.
        let Some((mut events, dropped)) = held.frames.pop_front() else {
            return;
        };
        if held.frames.is_empty()
            && let Some(m) = held.modifiers
        {
            events.push(egui::Event::ModifiersChanged(m));
        }
        events.append(&mut raw.events);
        raw.events = events;
        raw.dropped_files.splice(0..0, dropped);
        ctx.request_repaint();
    }

    /// Waits for the change being computed and puts it in place.
    pub(super) fn wait_whole(&mut self) {
        if let Some(run) = self.crop.whole.take() {
            let done = run.rx.recv().ok().flatten();
            self.land_whole(run, done);
        }
    }

    /// A press, a key, a paste or a drop during a change computed aside: it
    /// lands first, then the input applies. Its window keeps its own.
    pub(super) fn wait_whole_for_input(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.crop.whole else { return };
        let shown = run.shown;
        if ctx.input(|i| super::retouch_tools::input_waits(i, shown, true)) {
            self.wait_whole();
        }
    }

    /// The change being computed, once done, is put in place; meanwhile,
    /// after a moment, its indicator and Cancel (Esc too).
    pub(super) fn whole_window(&mut self, ctx: &egui::Context) {
        let Some(run) = &mut self.crop.whole else {
            return;
        };
        let done = match run.rx.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Esc cancels at once, the indicator shown or not yet.
                let mut cancel = ctx.input(|i| i.key_pressed(Key::Escape));
                let elapsed = ctx.input(|i| i.time) - run.started;
                let delay = filters_ui::BUSY_DELAY;
                if elapsed < delay && !cancel {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay - elapsed));
                    return;
                }
                let name = run.landing.name.clone();
                let area = egui::Area::new(egui::Id::new("whole_busy"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        use ui_kit::color;
                        egui::Frame::new()
                            .fill(color::BG2)
                            .stroke(UiStroke::new(1.0, color::WINDOW_LINE))
                            .corner_radius(12)
                            .inner_margin(egui::Margin::symmetric(18, 14))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    ui.add(egui::Spinner::new().size(16.0).color(color::MUTED));
                                    ui.label(
                                        egui::RichText::new(format!("{name}…")).color(color::TEXT),
                                    );
                                    cancel |= ui_kit::button(
                                        ui,
                                        &t!("dialog-cancel"),
                                        ui_kit::Kind::Normal,
                                        None,
                                        true,
                                    )
                                    .clicked();
                                });
                            });
                    });
                run.shown = run.laid_out.then_some(area.response.rect);
                run.laid_out = true;
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                if !cancel {
                    return;
                }
                // Dropped at once: the work stops at its next layer.
                run.cancel.store(true, Ordering::Relaxed);
                None
            }
        };
        if let Some(run) = self.crop.whole.take() {
            self.land_whole(run, done);
        }
    }

    /// A change of the whole document done (`None`: cancelled): one step,
    /// if it changed anything, with all that goes with it.
    fn land_whole(&mut self, run: WholeRun, done: Option<WholeDone>) {
        let WholeRun {
            doc: uid,
            step,
            landing,
            then,
            ..
        } = run;
        let same = self
            .doc
            .as_ref()
            .is_some_and(|d| (d.uid(), d.history_step()) == (uid, step));
        // Not applied: the box drawn comes back, on its document.
        let give_back = |app: &mut Self, kind: &LandingKind| {
            if let LandingKind::Crop {
                back: Some((r, angle)),
                ..
            } = *kind
                && app.tool == Tool::Canvas
                && app.doc.as_ref().is_some_and(|d| d.uid() == uid)
            {
                app.canvas_rect = Some(r);
                app.crop.angle = angle;
            }
        };
        match done {
            Some(done) if same => self.put_whole(done, landing),
            Some(_) => {
                self.status = t!("crop9-status-dropped", name = landing.name.as_str());
                give_back(self, &landing.kind);
            }
            None => {
                self.crop.close_waits = false;
                // What was given meanwhile was for its result: it goes too.
                let held = &mut self.crop.held;
                (held.frames, held.down) = (Default::default(), false);
                self.status = t!("crop9-status-cancelled", name = landing.name.as_str());
                self.actions.step_stopped(actions_panel::Stopped::Cancelled);
                give_back(self, &landing.kind);
                // What was to follow the crop waited for it: it stops too.
                return;
            }
        }
        for then in then {
            then(self);
        }
    }

    fn put_whole(&mut self, done: WholeDone, landing: Landing) {
        let (map, (w, h), resampled) = (done.map, done.size, done.resampled);
        let Some(doc) = &mut self.doc else { return };
        // A step that changes nothing says nothing: an action goes on.
        if !doc.put_whole(done, landing.name) {
            return;
        }
        if let Some(recorded) = landing.recorded {
            self.record(recorded);
        }
        self.texts_after(landing.texts, map);
        if resampled {
            self.fit_pending = Some(1.0);
        }
        // The selection's coordinates no longer make sense.
        self.forget_selection();
        match landing.kind {
            LandingKind::Rotate(degrees) => {
                let angle = i18n::short_decimal(f64::from(degrees.rem_euclid(360.0)));
                self.status = t!("status-image-rotated", angle = angle);
                self.reset_pending();
            }
            LandingKind::Flip => self.reset_pending(),
            LandingKind::Crop { shift, .. } => {
                self.status = t!("status-canvas-resized", width = w, height = h);
                self.after_geometry_change(shift);
            }
        }
    }

    /// The Crop tool's keys: O goes through the overlays, X swaps W and H.
    /// Shift + O has nothing to change here, but stays in the tool. Returns
    /// whether it took `key`.
    pub(super) fn crop_key(&mut self, key: Key, shift: bool) -> bool {
        if self.tool != Tool::Canvas {
            return false;
        }
        match key {
            Key::O => {
                if !shift {
                    let all = CropOverlay::ALL;
                    let i = all.iter().position(|o| *o == self.crop.overlay).unwrap_or(0);
                    self.crop.overlay = all[(i + 1) % all.len()];
                }
                true
            }
            Key::X => {
                if let Some(full) = self.full_rect() {
                    self.crop_swap(full);
                }
                true
            }
            _ => false,
        }
    }
}
