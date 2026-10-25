//! Edit in Quick Mask Mode (Q), like Photoshop: the selection becomes a
//! mask, shown in red over what isn't selected. Every tool and operation
//! then edits it instead of the layer (black masks, white selects, grays
//! in between: see `document/quick_mask.rs`). Q again turns it back into
//! the selection. Entering and leaving it, and each change of the mask,
//! are history steps. The Move tool and Edit › Transform move and
//! transform the mask as an image (with a selection, the part of it
//! selected), through the transforms' pipeline.

use image::GrayImage;

use super::keys;
use super::select_menu::{OverlayMask, OverlaySource, OverlayStyle};
use super::*;
use crate::document::TransformTarget;
use crate::i18n::t;
use crate::mask_ops::par_rows;
use crate::xform::{self, Grid, Homography, Source, Transform};

/// A drag of the quick mask with the Move tool.
struct QuickMove {
    /// Pointer at the press (canvas).
    from: Pos2,
    /// What moves: what the selection selects of the mask; all of it
    /// without one.
    selection: Option<Selection>,
    /// Alt, with a selection: a copy of it moves.
    copy: bool,
    by: (i64, i64),
}

/// A copy of part of the quick mask being moved by Alt+arrows: the mask
/// before it, the selection then, how far it went, and what the mask and
/// the selection were after the last press (the series goes on as long as
/// they stay).
struct QuickCopy {
    base: Arc<GrayImage>,
    selection: Selection,
    by: (i64, i64),
    mask_key: u64,
    selection_key: u64,
}

/// The quick mask reduced to a level of the view, for the previews: what
/// moves (times the selection), and with a selection what stays and the
/// selection's coverage.
struct QuickParts {
    key: u64,
    part: Arc<GrayImage>,
    rest: Option<(Arc<GrayImage>, Arc<GrayImage>)>,
}

/// The quick mask as previewed during a move or a transform, at a level
/// of the view: its image and the canvas pixels each of its pixels covers,
/// from `origin`.
struct QuickPreview {
    key: u64,
    image: Arc<GrayImage>,
    origin: (i64, i64),
    px: u32,
}

/// Moves and transforms of the quick mask.
#[derive(Default)]
pub(super) struct QuickEdit {
    drag: Option<QuickMove>,
    /// The selection whose part of the mask Free Transform takes, its
    /// outline hidden meanwhile.
    pub(super) selection: Option<Selection>,
    parts: Option<QuickParts>,
    preview: Option<QuickPreview>,
    copy: Option<QuickCopy>,
    /// Box of what a transform takes, by a fingerprint of the mask and the
    /// selection.
    bounds: std::cell::Cell<Option<(u64, Option<PixelRect>)>>,
}

impl QuickEdit {
    /// Gives up the preview (and the drag, when `drag`).
    pub(super) fn clear(&mut self, drag: bool) {
        self.preview = None;
        self.parts = None;
        if drag {
            self.drag = None;
        }
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    #[cfg(test)]
    pub(super) fn previewed(&self) -> bool {
        self.preview.is_some()
    }
}

/// What identifies a selection's content (cache keys).
fn selection_key(s: Option<&Selection>) -> u64 {
    hash_of(s.map(|s| {
        let r = s.rect;
        let mask = s.mask.as_ref().map(|m| Arc::as_ptr(m) as usize);
        (r.x, r.y, r.w, r.h, s.inverted, mask)
    }))
}

/// "Quick Mask moved by (+12, −3)": the move in all, signed.
fn moved_status((dx, dy): (i64, i64)) -> String {
    let sign = |v: i64| match v {
        1.. => format!("+{v}"),
        _ => signed(v as f32),
    };
    t!("qm6-status-moved", x = sign(dx), y = sign(dy))
}

/// The move by `(dx, dy)` of what lies in `canvas`.
fn shift(canvas: PixelRect, (dx, dy): (i64, i64)) -> Transform {
    let b = [0.0, 0.0, canvas.w as f64, canvas.h as f64];
    Transform::projective(
        b,
        Homography::affine([1.0, 0.0, dx as f64, 0.0, 1.0, dy as f64]),
    )
}

impl AtelierApp {
    pub(super) fn toggle_quick_mask(&mut self) {
        self.finish_stroke();
        self.settle_selection();
        let Some(doc) = &mut self.doc else { return };
        let name = t!("hist3-quick-mask");
        if doc.quick_mask().is_some() {
            doc.leave_quick_mask(name);
            self.status = t!("sel-status-quick-mask-off");
        } else {
            doc.enter_quick_mask(name);
            self.status = t!("sel-status-quick-mask-on", key = keys::key(Key::Q));
        }
        self.prop_session = None;
        self.xform.quick.copy = None;
        self.pull_selection();
    }

    pub(super) fn in_quick_mask(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.quick_mask().is_some())
    }

    /// The mask in red over the canvas, in Quick Mask Mode; a dialog's
    /// preview (a filter, an adjustment), a move or a transform being
    /// prepared show there.
    pub(super) fn quick_mask_overlay(&self) -> Option<OverlaySource> {
        let doc = self.doc.as_ref()?;
        let mask = doc.quick_mask()?;
        let moving = self.xform.quick.drag.is_some() || self.transform_rect.is_some();
        if let Some(p) = self.xform.quick.preview.as_ref().filter(|_| moving) {
            return Some(OverlaySource {
                mask: OverlayMask::Gray(Arc::clone(&p.image)),
                scale: 1.0 / p.px as f32,
                origin: pos2(p.origin.0 as f32, p.origin.1 as f32),
                style: OverlayStyle::Tint([255, 0, 0], 0.5),
                key: hash_of((doc.uid(), "moved", p.key)),
            });
        }
        let previewed = self.preview_shown.as_ref().and_then(|p| match &p.mask {
            Some((gray, _)) if p.quick_mask && gray.dimensions() == mask.dimensions() => {
                Some((gray, p.fingerprint))
            }
            _ => None,
        });
        let (mask, key) = match previewed {
            Some((gray, version)) => (gray, hash_of((doc.uid(), "preview", version))),
            None => (mask, hash_of((doc.uid(), doc.quick_mask_key()))),
        };
        Some(OverlaySource {
            mask: OverlayMask::Gray(Arc::clone(mask)),
            scale: 1.0,
            origin: Pos2::ZERO,
            style: OverlayStyle::Tint([255, 0, 0], 0.5),
            key,
        })
    }

    /// The Quick Mask button under the colors, like Photoshop's: on while
    /// in Quick Mask Mode.
    pub(super) fn quick_mask_button(&mut self, ui: &mut egui::Ui) {
        let on = self.doc.as_ref().is_some_and(|d| d.quick_mask().is_some());
        let (rect, response) = ui.allocate_exact_size(vec2(28.0, 22.0), Sense::click());
        let enabled = self.doc.is_some();
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let fill = match (on, hovered) {
            (true, _) => ui_kit::color::ACCENT_SOFT,
            (false, true) => ui_kit::color::BUTTON_HOVER,
            (false, false) => Color32::TRANSPARENT,
        };
        painter.rect_filled(rect, 5.0, fill);
        // A frame with a circle in it: filled when the outside is masked.
        let ink = match (on, enabled) {
            (true, _) => ui_kit::color::ACCENT_TEXT,
            (false, true) if hovered => ui_kit::color::TEXT,
            (false, true) => ui_kit::color::MUTED,
            (false, false) => ui_kit::color::DISABLED,
        };
        let frame = Rect::from_center_size(rect.center(), vec2(16.0, 12.0));
        painter.rect_stroke(frame, 2.0, UiStroke::new(1.0, ink), StrokeKind::Inside);
        if on {
            painter.circle_filled(frame.center(), 3.5, ink);
        } else {
            painter.circle_stroke(frame.center(), 3.5, UiStroke::new(1.0, ink));
        }
        let tip = t!("sel-menu-quick-mask");
        // Painted: a named toggle for screen readers.
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, tip.as_str())
        });
        if enabled && ui_kit::tip_with_key(response, &tip, &keys::key(Key::Q)).clicked() {
            self.run(Action::Select(super::select_menu::Command::QuickMask));
        }
    }

    /// A press of the Type tool, or of a shape tool or the Pen making shapes, in
    /// Quick Mask Mode: refused with a status, as they would make layers
    /// (Ctrl with them still moves the mask).
    pub(super) fn quick_mask_refuses_tool(&mut self) -> bool {
        use super::vector_tools::{PenTool, ToolMode};
        if !self.in_quick_mask() || self.ctx.input(|i| i.modifiers.command) {
            return false;
        }
        let refused = match self.tool {
            Tool::Text => true,
            Tool::Shape(_) => self.vector.shape_mode == ToolMode::Shape,
            Tool::Pen(PenTool::Pen | PenTool::Freeform) => self.vector.pen_mode == ToolMode::Shape,
            _ => false,
        };
        if refused {
            self.status = t!("sel6-status-quick-mask-no-layers", key = keys::key(Key::Q));
        }
        refused
    }

    /// The selection whose part of the quick mask a move or a transform
    /// takes: the drag's, Free Transform's, or the current one.
    pub(super) fn quick_mask_selection(&self) -> Option<&Selection> {
        match &self.xform.quick.drag {
            Some(m) => m.selection.as_ref(),
            None => self
                .xform
                .quick
                .selection
                .as_ref()
                .or(self.selection.as_ref()),
        }
    }

    /// What `quick_mask_source` gives for the mask and the selection as
    /// they are, known otherwise.
    fn seed_quick_mask_bounds(&self, b: Option<PixelRect>) {
        let Some(doc) = self.doc.as_ref().filter(|d| d.quick_mask().is_some()) else {
            return;
        };
        let selection = self.quick_mask_selection();
        let key = hash_of((doc.uid(), doc.quick_mask_key(), selection_key(selection)));
        self.xform.quick.bounds.set(Some((key, b)));
    }

    /// Box of what a transform of the quick mask takes (see
    /// `Document::quick_mask_bounds`), kept as long as the mask and the
    /// selection stay.
    pub(super) fn quick_mask_source(&self) -> Option<Option<Rect>> {
        let doc = self.doc.as_ref()?;
        doc.quick_mask()?;
        let selection = self.quick_mask_selection();
        let key = hash_of((doc.uid(), doc.quick_mask_key(), selection_key(selection)));
        let b = match self.xform.quick.bounds.get().filter(|(k, _)| *k == key) {
            Some((_, b)) => b,
            None => {
                let b = doc.quick_mask_bounds(selection);
                self.xform.quick.bounds.set(Some((key, b)));
                b
            }
        };
        Some(b.map(to_rect))
    }

    /// The Move tool in Quick Mask Mode, unless Free Transform is being
    /// prepared or a handle of Show Transform Controls is under the
    /// pointer: a drag moves the mask. The cursor, `None` to leave it to
    /// the handles.
    pub(super) fn quick_mask_tool_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) -> Option<CursorIcon> {
        if !self.in_quick_mask() {
            return None;
        }
        if self.xform.quick.drag.is_none() {
            let pending = self.transform_rect.is_some()
                || self.transform_angle != 0.0
                || self.free_transform
                || self.drag.is_some()
                || self.rotate_drag.is_some();
            if pending {
                return None;
            }
            if self.move_controls()
                && let (Some(src), Some(p)) = (self.transform_source(), pointer)
            {
                let r = view.rect_to_screen(src);
                let handle = handles::hit_test(r, p).is_some_and(|h| h != Handle::Move);
                let turn = p.distance(super::canvas::rotate_handle_pos(r)) <= handles::GRAB_RADIUS;
                if handle || turn {
                    return None;
                }
            }
        }
        Some(self.quick_mask_move_input(view, pointer, start, down))
    }

    /// A drag of the quick mask (Shift: horizontally or vertically): what
    /// the selection selects of it, with the outline (Alt: a copy of it),
    /// or all of it. Shown as it goes, applied on release as a step.
    pub(super) fn quick_mask_move_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) -> CursorIcon {
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        if self.xform.quick.drag.is_none() {
            let Some(p) = pointer.filter(|_| start) else {
                return CursorIcon::Move;
            };
            // Nothing selected in the mask (or in its selected part):
            // nothing moves, no step.
            if self.quick_mask_empty() {
                return CursorIcon::Move;
            }
            self.xform.quick.drag = Some(QuickMove {
                from: view.to_image(p),
                selection: self.selection.clone(),
                copy: alt && self.selection.is_some(),
                by: (0, 0),
            });
        }
        if let (Some(p), Some(m)) = (pointer, &mut self.xform.quick.drag) {
            let mut d = view.to_image(p) - m.from;
            if shift {
                d = handles::constrain_axis(d);
            }
            m.by = (d.x.round() as i64, d.y.round() as i64);
            if let Some(s) = &m.selection {
                self.selection = Some(s.clone().translate(m.by.0, m.by.1));
            }
        }
        if down {
            return CursorIcon::Grabbing;
        }
        if let Some(m) = self.xform.quick.drag.take() {
            self.finish_quick_move(m);
        }
        self.xform.quick.clear(false);
        CursorIcon::Move
    }

    /// The drag `m` applied: one step.
    fn finish_quick_move(&mut self, m: QuickMove) {
        let Some(doc) = &mut self.doc else { return };
        let canvas = doc.full_rect();
        if m.by == (0, 0) || doc.quick_mask().is_none() {
            self.selection = m.selection;
            return;
        }
        let t = shift(canvas, m.by);
        let filter = image::imageops::FilterType::Nearest;
        let done = doc
            .quick_mask_job(&t, m.selection.as_ref(), m.copy, filter)
            .run();
        // The step keeps the selection as it was before.
        doc.sync_selection(m.selection.clone());
        doc.apply_transformed(TransformTarget::QuickMask, &t, done, t!("xchg-hist-move"));
        let (dx, dy) = m.by;
        self.selection = m.selection.and_then(|s| s.translate(dx, dy).within(canvas));
        self.sync_selection();
        self.status = moved_status((dx, dy));
    }

    /// Whether what a move of the quick mask takes is empty (said in the
    /// status): nothing selected in the mask, or in its selected part.
    fn quick_mask_empty(&mut self) -> bool {
        if self.quick_mask_source().is_some_and(|b| b.is_some()) {
            return false;
        }
        self.status = t!("xform-status-nothing", name = t!("qm6-quick-mask-name"));
        true
    }

    /// Arrow keys in Quick Mask Mode: the mask moves (with a selection,
    /// what it selects of it, with the outline). A series of presses is a
    /// step, as for a layer; with a selection, each press, as for pixels.
    /// Alt with the Move tool and a selection, as its drag: a copy moves,
    /// the presses of the series one step.
    pub(super) fn nudge_quick_mask(&mut self, d: (i64, i64)) {
        // A selection still being computed lands first: it is what moves.
        self.wait_select_job();
        let Some(id) = self.doc.as_ref().map(|doc| doc.active_layer().id) else {
            return;
        };
        let alt = self.tool == Tool::Transform && self.ctx.input(|i| i.modifiers.alt);
        if alt && self.selection.is_some() && self.nudge_quick_mask_copy(id, d) {
            return;
        }
        if self.quick_mask_empty() {
            return;
        }
        let selection = self.selection.clone();
        let key = selection.is_none().then_some((id, "quick mask"));
        // The status gives the series' total, as its step does.
        let series = key.is_some()
            && self.doc.as_ref().is_some_and(|doc| {
                self.prop_session
                    .is_some_and(|(k, step)| Some(k) == key && step == doc.history_step())
            });
        let total_id = egui::Id::new("quick-mask-nudged");
        let before = self
            .ctx
            .data(|data| data.get_temp::<(i64, i64)>(total_id))
            .filter(|_| series)
            .unwrap_or_default();
        let total = (before.0 + d.0, before.1 + d.1);
        self.ctx.data_mut(|data| data.insert_temp(total_id, total));
        let bounds = self.xform.quick.bounds.get().and_then(|(_, b)| b);
        self.record_change(t!("xchg-hist-move"), key);
        let Some(doc) = &mut self.doc else { return };
        doc.move_quick_mask(d, selection.as_ref());
        let canvas = doc.full_rect();
        // With a hard edge (or none), what the next press takes is what
        // this one moved: its box, without scanning the mask again.
        let hard = selection.as_ref().is_none_or(|s| s.mask.is_none());
        if let Some(s) = selection {
            self.selection = s.translate(d.0, d.1).within(canvas);
            self.sync_selection();
        }
        if let Some(b) = bounds.filter(|_| hard) {
            self.seed_quick_mask_bounds(intersect(b.translate(d.0, d.1), canvas));
        }
        self.status = moved_status(total);
    }

    /// Align in Quick Mask Mode: what a move of the mask takes (with a
    /// selection, what it selects of it, with the outline) goes to the
    /// canvas' edge or center, one step. Distribute has a single element:
    /// nothing to do.
    pub(super) fn align_quick_mask(&mut self, op: AlignOp) {
        self.wait_select_job();
        if self.quick_mask_empty() {
            return;
        }
        let (Some(Some(b)), Some(doc)) = (self.quick_mask_source(), self.doc.as_ref()) else {
            return;
        };
        let (b, r) = (to_pixel_rect(b), doc.full_rect());
        let center = |a: i64, len: u32| a + len as i64 / 2;
        let d = match op {
            AlignOp::Left => (r.x - b.x, 0),
            AlignOp::CenterH => (center(r.x, r.w) - center(b.x, b.w), 0),
            AlignOp::Right => (r.right() - b.right(), 0),
            AlignOp::Top => (0, r.y - b.y),
            AlignOp::CenterV => (0, center(r.y, r.h) - center(b.y, b.h)),
            AlignOp::Bottom => (0, r.bottom() - b.bottom()),
            AlignOp::DistributeH | AlignOp::DistributeV => (0, 0),
        };
        if d == (0, 0) {
            return;
        }
        let selection = self.selection.clone();
        self.record_change(t!("xchg-hist-align"), None);
        let Some(doc) = &mut self.doc else { return };
        doc.move_quick_mask(d, selection.as_ref());
        if let Some(s) = selection {
            self.selection = s.translate(d.0, d.1).within(r);
            self.sync_selection();
        }
    }

    /// Alt+arrow in Quick Mask Mode with a selection: a copy of what it
    /// selects of the mask moves by `d`; the following presses of the
    /// series move that copy further, within the same step. `false` when
    /// there is nothing to copy.
    fn nudge_quick_mask_copy(&mut self, id: u64, d: (i64, i64)) -> bool {
        let key = (id, "quick mask copy");
        let Some(doc) = &self.doc else { return false };
        let step = doc.history_step();
        let mask_key = doc.quick_mask_key();
        let series = self.prop_session == Some((key, step));
        let selection = self.selection.clone();
        let copy = self.xform.quick.copy.take().filter(|c| {
            series
                && self.alt_nudge_copied
                && c.mask_key == mask_key
                && selection_key(selection.as_ref()) == c.selection_key
        });
        let copy = match copy {
            Some(c) => c,
            None => {
                if self.quick_mask_empty() {
                    return true;
                }
                let Some(base) = self.doc.as_ref().and_then(|doc| doc.quick_mask().cloned()) else {
                    return false;
                };
                let Some(selection) = selection else {
                    return false;
                };
                self.alt_nudge_copied = true;
                self.record_change(t!("xchg-hist-move"), Some(key));
                QuickCopy {
                    base,
                    selection,
                    by: (0, 0),
                    mask_key: 0,
                    selection_key: 0,
                }
            }
        };
        let by = (copy.by.0 + d.0, copy.by.1 + d.1);
        let Some(doc) = &mut self.doc else {
            return true;
        };
        doc.mark_modified();
        doc.copy_quick_mask(&copy.base, by, Some(&copy.selection));
        let canvas = doc.full_rect();
        self.selection = copy.selection.clone().translate(by.0, by.1).within(canvas);
        self.sync_selection();
        let Some(doc) = &self.doc else { return true };
        self.xform.quick.copy = Some(QuickCopy {
            by,
            mask_key: doc.quick_mask_key(),
            selection_key: selection_key(self.selection.as_ref()),
            ..copy
        });
        self.status = moved_status(by);
        true
    }

    /// The quick mask as the drag or the transform being prepared will
    /// make it, where `visible`, for `quick_mask_overlay`; `frame`: the
    /// source box of Free Transform, while previewing.
    pub(super) fn preview_quick_mask(&mut self, frame: Option<Rect>, visible: Rect) {
        let canvas = self.doc.as_ref().map(Document::full_rect);
        let by = self.xform.quick.drag.as_ref().map(|m| m.by);
        let t = match by {
            Some(by) => canvas.map(|c| shift(c, by)),
            None => frame.and_then(|src| self.pending_transform(src, false)),
        };
        let copy = self.xform.quick.drag.as_ref().is_some_and(|m| m.copy);
        if t.is_none_or(|t| self.quick_preview(&t, by, copy, visible).is_none()) {
            self.xform.quick.preview = None;
        }
    }

    /// `by`: `t` is that move (a drag), shown in whole pixels of the
    /// preview's level, without resampling.
    fn quick_preview(
        &mut self,
        t: &Transform,
        by: Option<(i64, i64)>,
        copy: bool,
        visible: Rect,
    ) -> Option<()> {
        let doc = self.doc.as_ref()?;
        let mask = doc.quick_mask()?;
        let canvas = doc.full_rect();
        let level = render::render_level(self.zoom);
        let b = 1u32 << level;
        let selection = self.quick_mask_selection();
        let parts_key = hash_of((
            doc.uid(),
            doc.quick_mask_key(),
            selection_key(selection),
            copy,
            level,
        ));
        let to_bounds = |r: Rect| [r.min.x, r.min.y, r.max.x, r.max.y].map(f64::from);
        let v = xform::clip(to_bounds(visible), to_bounds(to_rect(canvas)));
        let key = hash_of((parts_key, t.fingerprint(), v.map(f64::to_bits)));
        if self
            .xform
            .quick
            .preview
            .as_ref()
            .is_some_and(|p| p.key == key)
        {
            return Some(());
        }
        if self
            .xform
            .quick
            .parts
            .as_ref()
            .is_none_or(|p| p.key != parts_key)
        {
            let parts = quick_parts(mask, selection, copy, canvas, b, parts_key);
            self.xform.quick.parts = Some(parts);
        }
        let parts = self.xform.quick.parts.as_ref()?;
        let grid = Grid::covering(v, f64::from(b));
        if grid.w == 0 || grid.h == 0 {
            return None;
        }
        let bi = i64::from(b);
        if let Some((dx, dy)) = by {
            let level = |d: i64| (d as f64 / b as f64).round() as i64;
            let out = shifted_parts(parts, (level(dx), level(dy)), &grid);
            self.xform.quick.preview = Some(QuickPreview {
                key,
                image: Arc::new(out),
                origin: (grid.x * bi, grid.y * bi),
                px: b,
            });
            return Some(());
        }
        let src = |image| Source {
            image,
            origin: [0.0, 0.0],
            px: f64::from(b),
        };
        let filter = xform::Filter::Bilinear;
        let mut out = xform::transform_gray(src(&*parts.part), t, grid, filter, 0);
        if let Some((rest, cov)) = &parts.rest {
            // Over what stays, as far as it covers it.
            let moved = xform::transform_gray(src(&**cov), t, grid, filter, 0);
            let (gw, rw) = (grid.w as usize, rest.width() as i64);
            let (raw, c) = (rest.as_raw(), moved.as_raw());
            let rh = rest.height() as i64;
            par_rows(out.as_mut(), gw, |first, band| {
                for (j, line) in band.chunks_exact_mut(gw).enumerate() {
                    let y = (grid.y + (first + j) as i64).clamp(0, rh - 1);
                    for (i, v) in line.iter_mut().enumerate() {
                        let x = (grid.x + i as i64).clamp(0, rw - 1);
                        let under = raw[(y * rw + x) as usize] as u32;
                        let s = c[(first + j) * gw + i] as u32;
                        *v = (*v as u32 + (under * (255 - s) + 127) / 255).min(255) as u8;
                    }
                }
            });
        }
        self.xform.quick.preview = Some(QuickPreview {
            key,
            image: Arc::new(out),
            origin: (grid.x * bi, grid.y * bi),
            px: b,
        });
        Some(())
    }
}

/// The quick mask's `parts` (at a level of the view) over `grid` (pixels
/// of that level), what moves shifted by `(sx, sy)` of them: whole pixels,
/// as a Move drag shows it, what it uncovers unselected.
fn shifted_parts(parts: &QuickParts, (sx, sy): (i64, i64), grid: &Grid) -> GrayImage {
    let part = &*parts.part;
    let (pw, ph) = (i64::from(part.width()), i64::from(part.height()));
    let gw = grid.w as usize;
    let mut out = GrayImage::new(grid.w, grid.h);
    if gw == 0 {
        return out;
    }
    // The columns of the grid what moves covers, and where they come from.
    let lo = (sx - grid.x).clamp(0, gw as i64) as usize;
    let hi = (sx - grid.x + pw).clamp(0, gw as i64) as usize;
    let from = |i: usize| (grid.x + i as i64 - sx) as usize;
    let row = |y: i64| (0..ph).contains(&y).then(|| (y * pw) as usize);
    par_rows(out.as_mut(), gw, |first, band| {
        for (j, line) in band.chunks_exact_mut(gw).enumerate() {
            let y = grid.y + (first + j) as i64;
            let moved = row(y - sy).filter(|_| lo < hi);
            if let Some(start) = moved {
                let src = &part.as_raw()[start + from(lo)..start + from(lo) + (hi - lo)];
                line[lo..hi].copy_from_slice(src);
            }
            let Some((rest, cov)) = &parts.rest else {
                continue;
            };
            // Over what stays, as far as it covers it.
            let (rw, rh) = (i64::from(rest.width()), i64::from(rest.height()));
            if rw == 0 || rh == 0 {
                continue;
            }
            let under = &rest.as_raw()[(y.clamp(0, rh - 1) * rw) as usize..][..rw as usize];
            let c = moved.map(|start| &cov.as_raw()[start..start + pw as usize]);
            for (i, v) in line.iter_mut().enumerate() {
                let u = u32::from(under[(grid.x + i as i64).clamp(0, rw - 1) as usize]);
                let s = match c {
                    Some(c) if (lo..hi).contains(&i) => u32::from(c[from(i)]),
                    _ => 0,
                };
                *v = (u32::from(*v) + (u * (255 - s) + 127) / 255).min(255) as u8;
            }
        }
    });
    out
}

/// The parts of the quick mask `mask` for a preview, reduced `b` times:
/// what `selection` selects of it, what stays (all of it for a `copy`) and
/// the selection's coverage.
fn quick_parts(
    mask: &Arc<GrayImage>,
    selection: Option<&Selection>,
    copy: bool,
    canvas: PixelRect,
    b: u32,
    key: u64,
) -> QuickParts {
    let reduce = |m: &GrayImage| crate::compose::levels::reduce_gray(m, b);
    let reduced = if b == 1 {
        Arc::clone(mask)
    } else {
        Arc::new(reduce(mask))
    };
    let Some(sel) = selection else {
        return QuickParts {
            key,
            part: reduced,
            rest: None,
        };
    };
    let cov = sel.to_mask(canvas);
    let cov = if b == 1 { cov } else { reduce(&cov) };
    let mut part = (*reduced).clone();
    let c = cov.as_raw();
    let w = part.width() as usize;
    par_rows(part.as_mut(), w.max(1), |first, band| {
        for (v, &s) in band.iter_mut().zip(&c[first * w..]) {
            *v = ((*v as u32 * s as u32 + 127) / 255) as u8;
        }
    });
    let rest = if copy {
        reduced
    } else {
        let mut rest = (*reduced).clone();
        for (v, &p) in rest.iter_mut().zip(part.as_raw()) {
            *v -= p;
        }
        Arc::new(rest)
    };
    QuickParts {
        key,
        part: Arc::new(part),
        rest: Some((rest, Arc::new(cov))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A drag's preview in whole pixels is what the release makes, over
    /// the grid shown.
    #[test]
    fn a_shifted_preview_matches_the_move() {
        let mask = Arc::new(GrayImage::from_fn(60, 40, |x, y| {
            image::Luma([((x * 7 + y * 13) % 256) as u8])
        }));
        let canvas = PixelRect {
            x: 0,
            y: 0,
            w: 60,
            h: 40,
        };
        let sel: Selection = PixelRect {
            x: 10,
            y: 5,
            w: 20,
            h: 20,
        }
        .into();
        for (selection, copy) in [(None, false), (Some(&sel), false), (Some(&sel), true)] {
            let parts = quick_parts(&mask, selection, copy, canvas, 1, 0);
            let grid = Grid::covering([5.0, 3.0, 50.0, 38.0], 1.0);
            let by = (7, -4);
            let out = shifted_parts(&parts, by, &grid);
            for gy in 0..grid.h {
                for gx in 0..grid.w {
                    let (x, y) = (grid.x + gx as i64, grid.y + gy as i64);
                    let (sx, sy) = (x - by.0, y - by.1);
                    let inside = |x: i64, y: i64| (0..60).contains(&x) && (0..40).contains(&y);
                    let m = |x: i64, y: i64| {
                        if inside(x, y) {
                            u32::from(mask.get_pixel(x as u32, y as u32).0[0])
                        } else {
                            0
                        }
                    };
                    let c = |x: i64, y: i64| {
                        u32::from(selection.is_some_and(|_| inside(x, y) && sel.coverage(x, y) > 0))
                            * 255
                    };
                    let want = match selection {
                        None => m(sx, sy),
                        Some(_) => {
                            let part = m(sx, sy) * c(sx, sy) / 255;
                            let rest = if copy {
                                m(x, y)
                            } else {
                                m(x, y) - m(x, y) * c(x, y) / 255
                            };
                            (part + rest * (255 - c(sx, sy)) / 255).min(255)
                        }
                    };
                    let got = u32::from(out.get_pixel(gx, gy).0[0]);
                    assert_eq!(
                        got,
                        want,
                        "at ({x}, {y}), selection {}, copy {copy}",
                        selection.is_some()
                    );
                }
            }
        }
    }
}
