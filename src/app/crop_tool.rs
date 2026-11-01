//! Crop tool: its options (ratio presets, overlay, shield, Delete Cropped
//! Pixels), the turned box (a drag outside it, Straighten) and how it is
//! applied. The box stays in `canvas_rect`, turned by [`CropTool::angle`]
//! around its center: applying turns the image the other way, and keeps
//! what the box covered.

use super::canvas::{draw_frame, rotate_around};
use super::*;
use crate::i18n::t;
use crate::mask_ops::Affine;

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
    Never,
}

impl CropOverlay {
    pub(super) const ALL: [Self; 3] = [Self::Thirds, Self::Grid, Self::Never];

    pub(super) fn label(self) -> String {
        match self {
            Self::Thirds => t!("crop8-overlay-thirds"),
            Self::Grid => t!("crop8-overlay-grid"),
            Self::Never => t!("crop8-overlay-none"),
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
        let Some(r) = self.canvas_rect else {
            return full;
        };
        let (w, h) = rotated_size(r.width(), r.height(), self.crop.angle);
        let reach = Rect::from_center_size(r.center(), vec2(w, h));
        let reach = Rect::from_min_max(
            pos2(reach.min.x.floor(), reach.min.y.floor()),
            pos2(reach.max.x.ceil(), reach.max.y.ceil()),
        );
        full.union(reach)
    }

    /// Whether the box differs from the plain canvas.
    pub(super) fn crop_box_drawn(&self, full: Rect) -> bool {
        self.crop.angle != 0.0 || self.canvas_rect.is_some_and(|r| r != full)
    }

    /// The shield, the overlay, the frame and the line being drawn.
    pub(super) fn draw_crop(&self, painter: &Painter, view: View, frame: Rect, full: Rect) {
        let r = self.canvas_rect.unwrap_or(full);
        let angle = self.crop.angle;
        let on_screen = |corners: [Pos2; 4]| corners.map(|p| view.to_screen(p));
        let hole = on_screen(turned_corners(r, angle));
        if self.crop_box_drawn(full) && self.crop.shield > 0.0 {
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
        if self.crop_box_drawn(full) || working {
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
    fn draw_crop_overlay(&self, painter: &Painter, [nw, ne, se, sw]: [Pos2; 4]) {
        let parts = match self.crop.overlay {
            CropOverlay::Thirds => 3,
            CropOverlay::Grid => 8,
            CropOverlay::Never => return,
        };
        let stroke = UiStroke::new(1.0, Color32::from_white_alpha(110));
        for i in 1..parts {
            let f = i as f32 / parts as f32;
            painter.line_segment([nw.lerp(ne, f), sw.lerp(se, f)], stroke);
            painter.line_segment([nw.lerp(sw, f), ne.lerp(se, f)], stroke);
        }
    }

    /// The box as a drag on it starts from: without one yet, what the ratio
    /// makes of the canvas.
    pub(super) fn crop_start_rect(&self, rect: Rect) -> Rect {
        match (self.canvas_rect, self.crop.ratio()) {
            (None, Some(ratio)) => fit_ratio(rect, ratio),
            _ => rect,
        }
    }

    /// Starts turning the box, the pointer leaving it.
    pub(super) fn crop_turn_starts(&mut self, rect: Rect) {
        self.crop.turned_from = Some(self.crop_start_rect(rect));
    }

    /// The box turned to `degrees`: it stays on the image, and the turned
    /// canvas must fit in a layer.
    pub(super) fn crop_turn_to(&mut self, degrees: f32, full: Rect) {
        let from = self.crop.turned_from.or(self.canvas_rect).unwrap_or(full);
        if !fits_max_dimension(full.size(), degrees) {
            return;
        }
        self.crop.angle = degrees;
        self.canvas_rect = Some(fit_turned(from, degrees, full));
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
                self.crop.turned_from = Some(self.crop_start_rect(self.canvas_rect.unwrap_or(full)));
                self.crop_turn_to(straighten_angle(from, to), full);
                self.crop.turned_from = None;
            }
            self.crop.straighten = false;
        }
        CursorIcon::Crosshair
    }

    /// A ratio or preset chosen: the box takes that shape, as large as the
    /// image allows.
    pub(super) fn crop_reshape(&mut self, full: Rect) {
        let Some(ratio) = self.crop.ratio() else {
            return;
        };
        let angle = self.crop.angle;
        self.canvas_rect = Some(fit_turned(fit_ratio(full, ratio), angle, full));
    }

    pub(super) fn crop_pick_preset(&mut self, preset: CropPreset, full: Rect) {
        self.crop.preset = preset;
        let canvas = (full.width() as u32, full.height() as u32);
        if let Some((w, h)) = preset.values(canvas) {
            (self.crop.w, self.crop.h) = (w, h);
        } else if preset == CropPreset::Ratio {
            (self.crop.w, self.crop.h) = (0.0, 0.0);
        } else if self.crop.ratio().is_none() {
            // W × H × Resolution starts from the box's own size.
            let r = self.canvas_rect.unwrap_or(full);
            (self.crop.w, self.crop.h) = (r.width(), r.height());
        } else {
            (self.crop.w, self.crop.h) = (self.crop.w.round(), self.crop.h.round());
        }
        self.crop_reshape(full);
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
        std::mem::swap(&mut self.crop.w, &mut self.crop.h);
        if self.crop.ratio().is_some() {
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
        self.record_crop(plan);
        self.canvas_rect = None;
        self.apply_crop(plan);
    }

    /// A crop as one step: turned first if it must be, cut, then resampled.
    pub(super) fn apply_crop(&mut self, plan: CropPlan) {
        let mut r = plan.rect;
        if r.w == 0 || r.h == 0 {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let mark = doc.history_mark();
        let old = (doc.width(), doc.height());
        let old_center = pos2(old.0 as f32, old.1 as f32) / 2.0;
        let box_center = to_rect(r).center();
        if plan.angle != 0.0 {
            let degrees = -plan.angle;
            let texts = self.texts_before();
            let Some(doc) = &mut self.doc else { return };
            if doc.rotate_canvas(degrees) {
                let map = type_tool::canvas_rotation(old, (doc.width(), doc.height()), degrees);
                r = turned_box(r, box_center, map);
                self.texts_after(texts, map);
            }
        }
        let Some(doc) = &mut self.doc else { return };
        doc.crop_area(r, plan.delete);
        let (mut w, mut h) = (r.w, r.h);
        if let Some((tw, th)) = plan.size {
            let filter = self.resize_filter.filter();
            let doc = self.doc.as_mut().expect("checked");
            if doc.resize_image(tw, th, filter) {
                self.fit_pending = Some(1.0);
                self.rerender_texts();
            }
            (w, h) = (tw, th);
        }
        let Some(doc) = &mut self.doc else { return };
        if plan.resolution > 0.0 {
            let density = f64::from(plan.resolution);
            let meta = doc.file_metadata.image.get_or_insert_default();
            std::sync::Arc::make_mut(meta).density = Some((density, density));
        }
        if doc.steps_since(mark) > 0 {
            doc.join_since(mark);
            doc.rename_last_step(t!("menu-image-crop"));
        }
        self.status = t!("status-canvas-resized", width = w, height = h);
        // The selection's coordinates no longer make sense.
        self.forget_selection();
        self.after_geometry_change(box_center - old_center);
    }
}

/// `r`, centered on `center` before the canvas turned by `map`, in the
/// turned canvas: same size, around the new place of its center.
fn turned_box(r: PixelRect, center: Pos2, Affine(m): Affine) -> PixelRect {
    let x = m[0] * center.x + m[1] * center.y + m[2];
    let y = m[3] * center.x + m[4] * center.y + m[5];
    PixelRect {
        x: (x - r.w as f32 / 2.0).round() as i64,
        y: (y - r.h as f32 / 2.0).round() as i64,
        ..r
    }
}
