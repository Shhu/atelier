//! Canvas: tool input, drawing of the image and frames, texture uploads.

use super::keys;
use super::*;
use crate::i18n::t;

impl AtelierApp {
    /// Preview of the open dialog (transparent color, image adjustment) on
    /// the active layer (inside the selection), without touching the
    /// document.
    pub(super) fn sync_layer_preview(&mut self) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let preview = match (self.transparency, &self.adjust) {
            (Some(t), _) if t.preview => LayerPreview::Transparency(t),
            (None, Some(dialog)) if dialog.preview => {
                LayerPreview::Adjust(dialog.adjustment.clone())
            }
            // Preview unchecked: the layer shows its real pixels.
            (Some(_), _) | (None, Some(_)) => return self.end_layer_preview(),
            (None, None) => return,
        };
        if let Some(p) = &self.preview_shown
            && (&p.preview, p.layer, p.rev) == (&preview, layer.id, layer.rev())
        {
            return;
        }
        // Like the result: only inside the selection.
        let Some(image) =
            doc.processed_active(self.selection.as_ref(), |image| preview.apply(image))
        else {
            return self.end_layer_preview();
        };
        self.show_preview(preview, image);
    }

    /// Shows `image` (the result of a dialog's processing, see
    /// [`Document::processed_active`]) instead of what the active layer's
    /// target holds: its pixels, or its mask.
    pub(super) fn show_preview(&mut self, preview: LayerPreview, image: image::RgbaImage) {
        let Some(doc) = &self.doc else { return };
        let Some(target) = doc.target_layer() else {
            return;
        };
        if image.dimensions() != target.image().dimensions() {
            return;
        }
        let shown = ShownPreview::new(preview, doc.active_layer(), image);
        self.preview_shown = Some(if doc.mask_targeted() {
            shown.on_mask(target.offset)
        } else {
            shown
        });
    }

    /// End of preview: the layer shows its real pixels again.
    pub(super) fn end_layer_preview(&mut self) {
        self.preview_shown = None;
    }

    pub(super) fn canvas(&mut self, ui: &mut egui::Ui) {
        let whole = ui.available_rect_before_wrap();
        let (strip, area) = whole.split_top_bottom_at_y(whole.top() + TAB_STRIP_HEIGHT);
        self.document_tabs(ui, strip);
        paint_backdrop(&ui.painter_at(area), area);
        let Some(full) = self.full_rect() else {
            self.rulers.forget_area();
            self.welcome(ui, area);
            return;
        };
        let area = self.rulers.layout(area);
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);

        if let Some(max_zoom) = self.fit_pending
            && area.width() > 0.0
        {
            // An artboard (double-click on its thumbnail), or the canvas.
            let target = self.artboards.fit_to.take().map_or(full, to_rect);
            let fit = ((area.width() - 40.0) / target.width())
                .min((area.height() - 40.0) / target.height());
            let ppp = ui.ctx().pixels_per_point();
            self.zoom = (fit * ppp).clamp(MIN_ZOOM, max_zoom);
            self.pan = (full.center() - target.center()) * self.zoom / ppp;
            self.fit_pending = None;
        }

        let ctx = ui.ctx().clone();
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        // The floating zoom bar covers the canvas: no tool under it.
        let (bar, ..) = zoom_bar_geometry(ui, area);
        let hovered = response.hovered() && !pointer.is_some_and(|p| bar.contains(p));

        // Mouse wheel, like Photoshop: scrolling (Shift: horizontal), zoom with
        // Alt; Ctrl+wheel or pinch zoom too. Centered on the cursor.
        if hovered && let Some(pos) = pointer {
            let (scroll, pinch, alt) =
                ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers.alt));
            let wheel_zoom = if alt { scroll.y + scroll.x } else { 0.0 };
            let factor = pinch * (wheel_zoom * 0.0015).exp();
            if factor != 1.0 {
                self.set_zoom_around(self.zoom * factor, pos, area.center());
            }
            if !alt {
                self.pan += scroll;
            }
        }

        // Panning the view: middle click, or Space (or Hand tool) + left click.
        let space = (ctx.input(|i| i.key_down(Key::Space))
            && !ctx.egui_wants_keyboard_input()
            && !self.typing_on_canvas())
            || self.tool == Tool::Hand;
        let panning = response.dragged_by(PointerButton::Middle)
            || (space && response.dragged_by(PointerButton::Primary));
        if panning {
            self.pan += response.drag_delta();
        }

        // On the grid of physical pixels: at 100 %, one image pixel is
        // exactly one pixel of the screen, as in Photoshop.
        let ppp = ctx.pixels_per_point();
        let size = full.size() * self.zoom / ppp;
        let corner = area.center() + self.pan - size / 2.0;
        let corner = (corner.to_vec2() * ppp).round().to_pos2() / ppp;
        let canvas_screen = Rect::from_min_size(corner, size);
        let view = View {
            origin: canvas_screen.min,
            zoom: self.zoom / ppp,
        };
        self.last_view = Some(view);

        self.hover_pixel = pointer.filter(|_| hovered).map(|p| {
            let ip = view.to_image(p);
            (ip.x.floor() as i64, ip.y.floor() as i64)
        });

        let (pressed, down, released, shift, alt) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.modifiers.shift,
                i.modifiers.alt,
            )
        });
        self.guides = Snap::default();
        self.snap_off = ctx.input(|i| i.modifiers.command);
        let mut start = pressed && hovered && !space && !panning;

        // "Transparent color" dialog open: the click picks the color.
        if start
            && let (Some(t), Some(p), Some(doc)) = (&mut self.transparency, pointer, &self.doc)
            && t.picking
        {
            let ip = view.to_image(p);
            if let Some([r, g, b, _]) = doc
                .active_layer()
                .pixel_at(ip.x.floor() as i64, ip.y.floor() as i64)
            {
                t.color = Color32::from_rgb(r, g, b);
            }
            start = false;
        }

        // Alt + click (brush, bucket, eyedropper): pick a color, like Photoshop.
        let picks = self.tool == Tool::Eyedropper
            || (alt && matches!(self.tool, Tool::Brush | Tool::Bucket));
        if start
            && picks
            && let Some(p) = pointer
        {
            let ip = view.to_image(p);
            let to_background = self.tool == Tool::Eyedropper && alt;
            self.pick_color(ip, to_background);
            start = false;
        }

        // Alt + click (Clone Stamp, Healing Brush): the source point.
        if start
            && alt
            && self.tool.clones()
            && let Some(p) = pointer
        {
            self.set_clone_source(view.to_image(p));
            start = false;
        }

        let guide_cursor = self.guides_input(ui, view, pointer, hovered, &mut start);

        // Color Range picks colors on the image; Select and Mask and
        // Transform Selection take the pointer from the tools.
        if start && self.select_window_click(view, pointer) {
            start = false;
        }
        let transforming = self.sel.transform.is_some().then_some(start);
        if transforming.is_some() {
            start = false;
        }

        // Pointer used by tools: over the canvas, or button held (dragging off
        // the canvas), up to the release of that drag.
        let drag_end = released && response.drag_stopped_by(PointerButton::Primary);
        let tool_pointer = pointer.filter(|_| hovered || down || drag_end);
        let mut cursor = None;
        match self.tool {
            Tool::Brush | Tool::Eraser if self.sel.quick_mask.is_some() => {
                self.quick_mask_input(view, tool_pointer, start, down);
            }
            Tool::Brush
            | Tool::Eraser
            | Tool::Dodge
            | Tool::Burn
            | Tool::Sponge
            | Tool::Blur
            | Tool::Sharpen
            | Tool::Smudge => {
                if alt && self.tool == Tool::Brush && self.stroke.is_none() {
                    // Temporary eyedropper.
                    cursor = Some(CursorIcon::Crosshair);
                }
                self.brush_input(view, pointer, start, down, released, shift);
            }
            Tool::CloneStamp | Tool::HealingBrush | Tool::SpotHealing => {
                if alt && self.tool.clones() && self.stroke.is_none() {
                    cursor = Some(CursorIcon::Crosshair);
                }
                self.brush_input(view, pointer, start, down, released, shift);
            }
            Tool::Patch => {
                let double = hovered
                    && ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
                cursor = Some(self.patch_input(view, tool_pointer, start, down, double));
            }
            Tool::RedEye => {
                cursor = Some(CursorIcon::Crosshair);
                if start && let Some(p) = pointer {
                    self.red_eye_click(view.to_image(p));
                }
            }
            Tool::Gradient => {
                cursor = Some(CursorIcon::Crosshair);
                self.gradient_input(view, pointer, start, down, released, shift);
            }
            Tool::Eyedropper => cursor = Some(CursorIcon::Crosshair),
            Tool::Artboard => {
                cursor = Some(self.artboard_input(view, tool_pointer, start, down, shift, alt));
            }
            // The hand goes through view panning (like Space).
            Tool::Hand => {}
            Tool::Zoom => {
                cursor = Some(if alt {
                    CursorIcon::ZoomOut
                } else {
                    CursorIcon::ZoomIn
                });
                if start && let Some(p) = pointer {
                    self.zoom_step(!alt, p, area.center());
                }
            }
            Tool::Select | Tool::Ellipse => {
                cursor = Some(self.select_input(view, tool_pointer, start, down, shift, alt));
            }
            Tool::FreeLasso | Tool::PolyLasso => {
                let double = hovered
                    && ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
                cursor = Some(self.free_lasso_input(view, tool_pointer, start, down, double));
            }
            Tool::MagicWand => {
                cursor = Some(CursorIcon::Crosshair);
                self.wand_input(view, pointer, start);
            }
            Tool::ObjectSelect => {
                cursor = Some(self.object_select_input(view, tool_pointer, hovered, start, down));
            }
            Tool::Lasso => {
                let double = hovered
                    && ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
                cursor = Some(self.lasso_input(view, tool_pointer, start, double));
            }
            Tool::Bucket => {
                cursor = Some(CursorIcon::Crosshair);
                if start && let Some(p) = pointer {
                    let ip = view.to_image(p);
                    self.bucket_fill(ip);
                }
            }
            Tool::Text => {
                cursor = Some(self.type_input(view, tool_pointer, start, down));
            }
            Tool::Pen(_) | Tool::PathSelect | Tool::DirectSelect | Tool::Shape(_) => {
                cursor = Some(self.vector_input(view, tool_pointer, start, down));
            }
            Tool::Canvas | Tool::Transform => {
                // The Move tool drags an artboard by its name.
                let label = (self.tool == Tool::Transform)
                    .then(|| self.artboard_label_input(view, tool_pointer, start, down))
                    .flatten();
                cursor = Some(
                    label
                        .or_else(|| {
                            self.handles_input(view, full, tool_pointer, start, down, shift)
                        })
                        .unwrap_or(CursorIcon::Default),
                );
                if self.tool == Tool::Transform {
                    self.commit_artboard_move(drag_end);
                }
            }
        }
        if let Some(start) = transforming {
            let double =
                hovered && ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
            cursor = Some(self.selection_transform_input(view, tool_pointer, start, down, double));
        }

        if guide_cursor.is_some() {
            cursor = guide_cursor;
        }

        // Right click: selects the element under the mouse and opens the menu
        // (not during a transform or crop: that would apply it).
        if hovered
            && self.layer_drag.is_none()
            && self.drag.is_none()
            && self.transform_rect.is_none()
            && self.canvas_rect.is_none()
            && ctx.input(|i| i.pointer.secondary_pressed())
            && let Some(p) = pointer
        {
            let ip = view.to_image(p);
            let picked = self
                .doc
                .as_ref()
                .and_then(|d| d.pick_layer(ip.x.floor() as i64, ip.y.floor() as i64));
            if let Some(i) = picked {
                self.select_layer(i);
            }
        }
        let mut menu_action = None;
        response.context_menu(|ui| {
            if self.transform_context_menu(ui, &mut menu_action) {
                return;
            }
            let name = self
                .doc
                .as_ref()
                .map(|d| d.active_layer().name.clone())
                .unwrap_or_default();
            ui.weak(name);
            ui.separator();
            if self.selection.as_ref().is_some_and(|s| !s.inverted)
                && ui
                    .button(format!(
                        "{} {}",
                        icon::CROP,
                        t!("canvas-menu-crop-to-selection")
                    ))
                    .clicked()
            {
                menu_action = Some(Action::CropToSelection);
            }
            for (label, act) in [
                (
                    format!("✂ {}", t!("canvas-menu-crop-to-element")),
                    Action::CropToElement,
                ),
                (
                    format!("⛶ {}", t!("canvas-menu-crop-to-content")),
                    Action::CropToContent,
                ),
            ] {
                if ui.button(label).clicked() {
                    menu_action = Some(act);
                }
            }
            ui.separator();
            for (label, act) in [
                (
                    format!("⌖ {}", t!("canvas-menu-center")),
                    Action::CenterElement,
                ),
                (
                    format!("⬆ {}", t!("canvas-menu-bring-to-front")),
                    Action::BringToFront,
                ),
                (
                    format!("⬇ {}", t!("canvas-menu-send-to-back")),
                    Action::SendToBack,
                ),
                (
                    format!("🗐 {}", t!("canvas-menu-duplicate")),
                    Action::Duplicate,
                ),
                (
                    format!("🗑 {}", t!("canvas-menu-delete-layer")),
                    Action::Delete,
                ),
                (
                    if self.active_locked() {
                        format!("🔓 {}", t!("canvas-menu-unlock"))
                    } else {
                        format!("🔒 {}", t!("canvas-menu-lock"))
                    },
                    Action::ToggleLock,
                ),
            ] {
                if ui.button(label).clicked() {
                    menu_action = Some(act);
                }
            }
            if ui
                .button(format!(
                    "{} {}",
                    icon::DROP_HALF,
                    t!("canvas-menu-make-transparent")
                ))
                .clicked()
            {
                menu_action = Some(Action::MakeTransparent);
            }
            if ui
                .button(format!(
                    "{} {}",
                    icon::USER_FOCUS,
                    t!("canvas-menu-remove-background")
                ))
                .clicked()
            {
                menu_action = Some(Action::RemoveBackground);
            }
            if ui
                .button(format!(
                    "{} {}",
                    icon::SELECTION_FOREGROUND,
                    t!("canvas-menu-select-subject")
                ))
                .clicked()
            {
                menu_action = Some(Action::SelectSubject);
            }
            ui.menu_button(
                format!(
                    "{} {}",
                    icon::SLIDERS_HORIZONTAL,
                    t!("canvas-menu-adjustments")
                ),
                |ui| {
                    for a in &ADJUSTMENTS {
                        if ui.button(format!("{} {}", a.icon, a.label())).clicked() {
                            menu_action = Some(Action::Adjust(a.kind));
                        }
                    }
                },
            );
        });
        if let Some(act) = menu_action {
            self.run(act);
        }

        self.sync_layer_preview();
        self.draw_canvas(&ctx, &painter, area, view, full, canvas_screen);
        if self.split_open
            && let Some(boxes) = self.split_boxes()
        {
            draw_split_preview(&painter, view, boxes);
        }
        self.draw_select_overlay(&painter, view);
        if self.selection_outline_shown() {
            self.draw_selection(&painter, view);
        }
        self.draw_selection_transform(&painter, view);
        self.draw_object_preview(&painter, view);
        self.draw_marquee_preview(&painter, view);
        self.draw_lasso(&painter, view);
        self.draw_gradient_line(&painter, view);
        self.draw_artboard_overlay(&painter, view);
        self.draw_vector_overlay(&painter, view, pointer.filter(|_| hovered));
        self.draw_retouch_overlay(&painter, view, pointer.filter(|_| hovered));
        self.draw_guides(&painter, view, full);
        let guide = UiStroke::new(1.0, ui_kit::color::GUIDE);
        if let Some(x) = self.guides.guide_x {
            painter.vline(view.to_screen(pos2(x, 0.0)).x, area.y_range(), guide);
        }
        if let Some(y) = self.guides.guide_y {
            painter.hline(area.x_range(), view.to_screen(pos2(0.0, y)).y, guide);
        }
        self.draw_type_overlay(&painter, view);

        self.zoom_bar(ui, area);
        self.draw_rulers(ui.painter(), view);

        // Cursor.
        if hovered && let Some(pos) = pointer {
            if space || panning {
                ctx.set_cursor_icon(if panning {
                    CursorIcon::Grabbing
                } else {
                    CursorIcon::Grab
                });
            } else if let Some(icon) = cursor {
                ctx.set_cursor_icon(icon);
            } else {
                ctx.set_cursor_icon(if self.draw_brush_cursor(&painter, pos, view) {
                    CursorIcon::None
                } else {
                    CursorIcon::Crosshair
                });
            }
        }

        if self.stroke.is_some()
            || self.drag.is_some()
            || self.layer_drag.is_some()
            || self.rotate_drag.is_some()
            || self.artboards.dragging()
        {
            ctx.request_repaint();
        }
    }

    pub(super) fn draw_canvas(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        area: Rect,
        view: View,
        full: Rect,
        canvas_screen: Rect,
    ) {
        let full_uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));

        // Element being transformed: its source region and its planned place.
        let transform = if self.tool == Tool::Transform && !self.active_locked() {
            self.transform_source()
                .map(|src| (src, self.transform_rect.unwrap_or(src)))
        } else {
            None
        };
        let angle = self.transform_angle;

        // Frame of the final image as it will be after applying.
        let frame = match (self.tool, transform) {
            (Tool::Canvas, _) => self.canvas_rect.unwrap_or(full),
            (Tool::Transform, Some((_, t))) if self.fit_canvas => {
                self.transform_reach().unwrap_or_else(|| {
                    let (w, h) = rotated_size(t.width(), t.height(), angle);
                    Rect::from_center_size(t.center(), vec2(w, h))
                })
            }
            _ => full,
        };
        // With artboards, the view is a pasteboard: what lies around the
        // boards shows too (except while a crop is prepared).
        let boards = if self.tool == Tool::Canvas {
            Vec::new()
        } else {
            self.board_backdrops()
        };
        let frame = if boards.is_empty() {
            frame
        } else {
            Rect::from_min_max(view.to_image(area.min), view.to_image(area.max))
        };
        let frame_screen = view.rect_to_screen(frame);
        let inside = painter.with_clip_rect(frame_screen.intersect(area));
        // Drop shadow and faint outline of the document (of each artboard),
        // and the checkerboard where it is transparent.
        let sheets = if boards.is_empty() {
            vec![(frame, true)]
        } else {
            boards
        };
        for (sheet, transparent) in sheets {
            let sheet = view.rect_to_screen(sheet);
            painter.add(
                egui::epaint::Shadow {
                    offset: [0, 18],
                    blur: 50,
                    spread: 0,
                    color: Color32::from_black_alpha(140),
                }
                .as_shape(sheet, 0.0),
            );
            painter.rect_stroke(
                sheet,
                0.0,
                UiStroke::new(1.0, Color32::from_white_alpha(15)),
                StrokeKind::Outside,
            );
            if transparent {
                draw_checker(painter, sheet);
            }
        }

        let Some(doc) = &self.doc else { return };
        let active = doc.active;
        // The transformed element is drawn separately, at its planned place.
        let previewing = transform.is_some()
            && (self.transform_rect.is_some() || self.drag.is_some() || self.rotate_drag.is_some());
        // In its place in the stack, as the transform will put it.
        let visible = Rect::from_min_max(view.to_image(area.min), view.to_image(area.max))
            .intersect(frame)
            .expand(2.0);
        let previewed = transform.filter(|_| previewing).map(|(src, _)| src);
        let placed = self.transform_preview(ctx, previewed, visible);
        self.draw_document(ctx, &inside, view, to_pixel_rect(frame), placed);

        // What overflows the frame is hidden, except for the element being
        // manipulated: it is shown semi-transparent so it isn't lost.
        let dragged = self.layer_drag.as_ref().map(|d| d.layers[0].0);
        let grabbed = dragged.or_else(|| {
            let lifted = !self.doc.as_ref().is_some_and(Document::transforms_mask);
            (transform.is_some() && lifted && (self.drag.is_some() || self.rotate_drag.is_some()))
                .then_some(active)
        });
        let Some(i) = grabbed else {
            return self.draw_frames(painter, view, transform, frame, full, canvas_screen);
        };
        // Out of its artboard, or of the frame.
        let hole = self.ghost_hole(i, view).unwrap_or(frame_screen);
        let Some((gpu, rect)) = self.lifted(ctx, i) else {
            return self.draw_frames(painter, view, transform, frame, full, canvas_screen);
        };
        // Moved, scaled and turned with the element being transformed.
        let (place, pivot, layer_angle) = match transform {
            Some((src, t)) if previewing => {
                let scale = t.size() / src.size();
                let map = |p: Pos2| t.min + (p - src.min) * scale;
                let place = Rect::from_min_max(map(rect.min), map(rect.max));
                (place, view.to_screen(t.center()), angle)
            }
            _ => (rect, view.to_screen(rect.center()), 0.0),
        };
        let dest = view.rect_to_screen(place);
        let ghost = Color32::from_white_alpha((255.0 * GHOST_ALPHA) as u8);
        for clip in outside(area, hole) {
            if clip.is_positive() {
                let clipped = painter.with_clip_rect(clip);
                gpu.paint(&clipped, dest, full_uv, ghost, layer_angle, pivot);
            }
        }
        self.draw_frames(painter, view, transform, frame, full, canvas_screen);
    }

    /// Frame of the crop or of the transform being prepared.
    fn draw_frames(
        &self,
        painter: &Painter,
        view: View,
        transform: Option<(Rect, Rect)>,
        frame: Rect,
        full: Rect,
        canvas_screen: Rect,
    ) {
        let frame_screen = view.rect_to_screen(frame);
        let angle = self.transform_angle;
        match (self.tool, transform) {
            (Tool::Canvas, _) => {
                if frame != full {
                    // Current frame, faint.
                    let faint = UiStroke::new(1.0, ui_kit::color::LINE_HOVER);
                    painter.rect_stroke(canvas_screen, 0.0, faint, StrokeKind::Outside);
                }
                let dims = (frame.width() as u32, frame.height() as u32);
                draw_frame(painter, frame_screen, 0.0, false, dims);
            }
            (Tool::Transform, Some((_, t))) => {
                if !self.draw_transform_frame(painter, view) {
                    // As applied: the frame's edges rounded to pixels.
                    let shown = round_rect(t);
                    let dims = (shown.width() as u32, shown.height() as u32);
                    let turns = self.transform_turns();
                    draw_frame(painter, view.rect_to_screen(t), angle, turns, dims);
                }
                self.draw_reference_point(painter, view);
            }
            _ => {}
        }
    }

    /// Handles the handles of the canvas and move/resize tools.
    /// Returns the cursor to show.
    pub(super) fn handles_input(
        &mut self,
        mut view: View,
        full: Rect,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
    ) -> Option<CursorIcon> {
        let is_canvas = self.tool == Tool::Canvas;
        if self.layer_drag.is_some() {
            return self.drag_layer_input(view, pointer, start, down, false);
        }
        // Distorted frames, warps, modifier drags, modes, reference point.
        if !is_canvas && let Some(cursor) = self.transform_input(view, pointer, start, down, shift)
        {
            return Some(cursor);
        }
        let alt = self.ctx.input(|i| i.modifiers.alt);
        let mut rect = if is_canvas {
            self.canvas_rect.unwrap_or(full)
        } else {
            self.transform_rect
                .or(self.transform_source())
                .unwrap_or(full)
        };
        // Handles are hit-tested in the element's unrotated frame.
        let angle = if is_canvas { 0.0 } else { self.transform_angle };
        let rect_screen = view.rect_to_screen(rect);
        let local = |p: Pos2| rotate_around(p, rect_screen.center(), -angle);
        let locked = !is_canvas && self.active_locked();
        let mut hover = pointer
            .filter(|_| !locked)
            .and_then(|p| hit_test(rect_screen, local(p)));
        let on_rotate_handle = !is_canvas
            && !locked
            && self.drag.is_none()
            && pointer.is_some_and(|p| {
                local(p).distance(rotate_handle_pos(rect_screen)) <= handles::GRAB_RADIUS
            });
        if on_rotate_handle {
            hover = None;
        }
        // The canvas doesn't move: only its edges and corners can be
        // dragged. Dragging elsewhere moves the element under the mouse.
        if is_canvas {
            if hover == Some(Handle::Move) {
                hover = None;
            }
            if hover.is_none() && self.drag.is_none() {
                return self.drag_layer_input(view, pointer, start, down, false);
            }
        }

        // Move tool: a click on another element selects its layer, except
        // inside the frame of a pending transform (its pixels aren't there yet).
        let pending = self.transform_rect.is_some() || self.transform_angle != 0.0;
        let mut picked = None;
        if !is_canvas
            && !on_rotate_handle
            && hover.is_none_or(|h| h == Handle::Move && !pending)
            && self.drag.is_none()
            && self.rotate_drag.is_none()
            && let Some(p) = pointer
        {
            let ip = view.to_image(p);
            let doc = self.doc.as_ref();
            picked = doc
                .and_then(|d| d.pick_layer(ip.x.floor() as i64, ip.y.floor() as i64))
                .filter(|&i| doc.is_some_and(|d| d.active != i));
            if picked.is_some() {
                hover = Some(Handle::Move);
            }
        }

        if start && let Some(p) = pointer {
            if on_rotate_handle {
                self.rotate_drag = Some((pointer_angle(p, rect_screen.center()), angle));
            } else if let Some(doc) = &self.doc {
                let mut angle = angle;
                let target = picked.unwrap_or(doc.active);
                let before = doc.layers[target].offset;
                if let Some(i) = picked {
                    self.select_layer(i);
                    angle = self.transform_angle;
                    if let Some(src) = self.transform_source() {
                        rect = src;
                    }
                }
                // Several layers selected: a move drags them all.
                let group =
                    !is_canvas && hover == Some(Handle::Move) && self.selected_indices().len() > 1;
                if group {
                    self.commit_pending();
                }
                // Applying a transform with "Fit canvas" moves the canvas
                // origin (the content stays put on screen): the view follows.
                if let Some(doc) = &self.doc {
                    let after = doc.layers[target].offset;
                    let shift = vec2((before.0 - after.0) as f32, (before.1 - after.1) as f32);
                    view.origin += shift * view.zoom;
                }
                if group {
                    self.start_layer_drag(target, view.to_image(p), true);
                    return Some(CursorIcon::Grabbing);
                }
                let ip = rotate_around(view.to_image(p), rect.center(), -angle);
                self.drag = hover.map(|h| RectDrag::new(h, rect, ip));
                if !is_canvas && hover == Some(Handle::Move) {
                    self.snap_targets = self
                        .doc
                        .as_ref()
                        .map(|d| snap_targets(d, &[d.active], &self.rulers.settings));
                }
            }
        }

        if !down {
            self.drag = None;
            self.rotate_drag = None;
            self.snap_targets = None;
        } else if let (Some((pointer_start, angle_start)), Some(p)) = (self.rotate_drag, pointer) {
            let mut a = angle_start + pointer_angle(p, rect_screen.center()) - pointer_start;
            if shift {
                a = (a / ROTATE_SNAP).round() * ROTATE_SNAP;
            }
            let a = normalize_angle(a);
            // Turned, the element must still fit in a layer.
            if fits_max_dimension(rect.size(), a) {
                self.transform_angle = a;
                self.transform_rect = Some(rect);
            }
        } else if let (Some(drag), Some(p)) = (self.drag, pointer) {
            // Pointer brought back into the starting element's unrotated frame.
            let mut ip = rotate_around(view.to_image(p), drag.start_center(), -angle);
            // Move: proportions kept by default, Shift inverts. Canvas: free, Shift constrains.
            let keep_ratio = if is_canvas {
                shift
            } else {
                self.keep_ratio != shift
            };
            // Shift during a move: horizontally or vertically (snapping too).
            let mut axes = Vec2::splat(1.0);
            if drag.handle == Handle::Move && shift {
                axes = drag.free_axes(ip);
                ip = drag.constrain_axis(ip);
            }
            let local_rect = if alt {
                drag.update_from_center(ip, keep_ratio)
            } else {
                drag.update(ip, keep_ratio)
            };
            // The center moves in the rotated frame: bring it back to the screen so
            // the opposite edge or corner stays fixed.
            let center = rotate_around(local_rect.center(), drag.start_center(), angle);
            rect = Rect::from_center_size(center, local_rect.size());
            if drag.handle == Handle::Move {
                // Snap the bounding box (the element may be rotated).
                let (w, h) = rotated_size(rect.width(), rect.height(), angle);
                let bbox = Rect::from_center_size(rect.center(), vec2(w, h));
                rect = rect.translate(self.snap_offset(bbox, axes));
            }
            // The canvas, or the element, must fit in a layer.
            if is_canvas {
                rect = round_rect(rect);
                if rect.width() >= 1.0
                    && rect.height() >= 1.0
                    && fits_max_dimension(rect.size(), 0.0)
                {
                    self.canvas_rect = Some(rect);
                }
            } else if fits_max_dimension(rect.size(), angle) {
                self.transform_rect = Some(rect);
            }
        }

        if self.rotate_drag.is_some() {
            return Some(CursorIcon::Grabbing);
        }
        if on_rotate_handle {
            return Some(CursorIcon::Grab);
        }
        self.drag
            .map(|d| d.handle)
            .or(hover)
            .map(|h| h.cursor(angle))
    }

    /// Offset that aligns `bbox` with the nearest target (and remembers
    /// the guides to draw), on the free `axes` (1: free, 0: locked by
    /// Shift). Zero if Ctrl is held or nothing is close enough.
    pub(super) fn snap_offset(&mut self, bbox: Rect, axes: Vec2) -> Vec2 {
        let Some(targets) = self.snap_targets.as_ref().filter(|_| !self.snap_off) else {
            return Vec2::ZERO;
        };
        let mut guides = snap(bbox, targets, SNAP_DISTANCE / self.screen_zoom());
        if axes.x == 0.0 {
            guides.guide_x = None;
        }
        if axes.y == 0.0 {
            guides.guide_y = None;
        }
        guides.offset = guides.offset * axes;
        self.guides = guides;
        guides.offset
    }

    /// Drag an element to move it as is (without resampling).
    /// Returns the cursor to show.
    pub(super) fn drag_layer_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        text_only: bool,
    ) -> Option<CursorIcon> {
        if self.layer_drag.is_some() {
            return Some(self.move_layers(view, pointer, down));
        }
        let ip = view.to_image(pointer?);
        let (px, py) = (ip.x.floor() as i64, ip.y.floor() as i64);
        let target = if text_only {
            // Topmost text layer whose box contains the point.
            let doc = self.doc.as_ref()?;
            (0..doc.layers.len()).rev().find(|&i| {
                let l = &doc.layers[i];
                l.visible && !l.locked && l.text.is_some() && to_rect(l.rect()).contains(ip)
            })?
        } else {
            // Visible pixel of an element, or else the active element's
            // box (to grab a text between its letters).
            let in_active =
                !self.active_locked() && self.transform_source().is_some_and(|r| r.contains(ip));
            let doc = self.doc.as_ref()?;
            doc.pick_layer(px, py).or(in_active.then_some(doc.active))?
        };
        if start {
            self.start_layer_drag(target, ip, !text_only);
        }
        Some(CursorIcon::Move)
    }

    /// Starts moving layer `target` from the point `ip`; with `group`, the
    /// other selected layers follow if it is one of them.
    fn start_layer_drag(&mut self, target: usize, ip: Pos2, group: bool) {
        let Some(doc) = &mut self.doc else { return };
        doc.active = target;
        let mut moved = vec![target];
        if group {
            let others = self.selected_indices().into_iter().filter(|&i| i != target);
            let doc = self.doc.as_ref().expect("checked above");
            moved.extend(others.filter(|&i| !doc.layers[i].locked));
        }
        let Some(doc) = &self.doc else { return };
        // A group moves with its layers, a layer with those linked to it
        // (the grabbed one stays first).
        let mut all: Vec<usize> = moved.iter().flat_map(|&i| doc.moving_with(i)).collect();
        all.retain(|&i| i != target && !doc.layers[i].locked);
        all.sort_unstable();
        all.dedup();
        let moved: Vec<usize> = [target].into_iter().chain(all).collect();
        let bounds = |i: usize| {
            let l = &doc.layers[i];
            to_rect(l.content_bounds().unwrap_or(l.rect()))
        };
        let start_bounds = moved.iter().map(|&i| bounds(i)).reduce(Rect::union);
        self.layer_drag = Some(LayerDrag {
            layers: moved.iter().map(|&i| (i, doc.layers[i].offset)).collect(),
            start: ip,
            start_bounds: start_bounds.unwrap_or(Rect::NOTHING),
            moved: false,
        });
        self.snap_targets = Some(snap_targets(doc, &moved, &self.rulers.settings));
    }

    /// Layers being dragged: they follow the pointer (Shift: horizontally
    /// or vertically), until the button is released, wherever that is.
    fn move_layers(&mut self, view: View, pointer: Option<Pos2>, down: bool) -> CursorIcon {
        let Some(mut drag) = self.layer_drag.take() else {
            return CursorIcon::Default;
        };
        if !down {
            self.snap_targets = None;
            if drag.moved
                && let Some(layer) = self.doc.as_ref().map(|d| &d.layers[drag.layers[0].0])
            {
                let (x, y) = layer.offset;
                self.status = t!(
                    "canvas-status-layer-moved",
                    name = layer.name.as_str(),
                    x = x,
                    y = y
                );
            }
            if drag.moved {
                let ids: Option<Vec<u64>> = self
                    .doc
                    .as_ref()
                    .map(|d| drag.layers.iter().map(|&(i, _)| d.layers[i].id).collect());
                self.artboards_after_move(&ids.unwrap_or_default());
            }
            return CursorIcon::Grabbing;
        }
        if let Some(p) = pointer {
            let mut d = view.to_image(p) - drag.start;
            let axes = if self.ctx.input(|i| i.modifiers.shift) {
                handles::free_axes(d)
            } else {
                Vec2::splat(1.0)
            };
            d = d * axes;
            // Snapping computed before borrowing the document.
            if drag.moved {
                d += self.snap_offset(drag.start_bounds.translate(d), axes);
            }
            let (dx, dy) = ((d.x + 0.5).floor() as i64, (d.y + 0.5).floor() as i64);
            if let Some(doc) = &mut self.doc {
                if (dx, dy) != (0, 0) && !drag.moved {
                    doc.checkpoint(t!("xchg-hist-move"));
                    drag.moved = true;
                }
                if drag.moved {
                    for &(i, (ox, oy)) in &drag.layers {
                        doc.layers[i].offset = (ox + dx, oy + dy);
                    }
                }
            }
        }
        self.layer_drag = Some(drag);
        CursorIcon::Grabbing
    }
}

/// The 4 strips of `area` around `hole`.
pub(super) fn outside(area: Rect, hole: Rect) -> [Rect; 4] {
    let h = hole.intersect(area);
    [
        Rect::from_min_max(area.min, pos2(area.max.x, h.min.y)),
        Rect::from_min_max(pos2(area.min.x, h.max.y), area.max),
        Rect::from_min_max(pos2(area.min.x, h.min.y), pos2(h.min.x, h.max.y)),
        Rect::from_min_max(pos2(h.max.x, h.min.y), pos2(area.max.x, h.max.y)),
    ]
}

/// Numbered colored frames around the found elements.
pub(super) fn draw_split_preview(painter: &Painter, view: View, boxes: &[Rect]) {
    let mut order: Vec<&Rect> = boxes.iter().collect();
    order.sort_by(|a, b| {
        (a.min.y, a.min.x)
            .partial_cmp(&(b.min.y, b.min.x))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (n, r) in order.into_iter().enumerate() {
        let color: Color32 =
            egui::ecolor::Hsva::new((n as f32 * 0.618).fract(), 0.85, 1.0, 1.0).into();
        let sr = view.rect_to_screen(*r).expand(2.0);
        painter.rect_stroke(sr, 2.0, UiStroke::new(2.0, color), StrokeKind::Outside);
        let label = format!("{}", n + 1);
        let font = egui::FontId::proportional(12.0);
        let galley = painter.layout_no_wrap(label, font, Color32::BLACK);
        let tag = Rect::from_min_size(
            sr.left_top() - vec2(0.0, galley.size().y + 2.0),
            galley.size() + vec2(6.0, 2.0),
        );
        painter.rect_filled(tag, 2.0, color);
        painter.galley(tag.min + vec2(3.0, 1.0), galley, Color32::BLACK);
    }
}

/// Rotates `p` by `degrees` (clockwise on screen) around `center`.
pub(super) fn rotate_around(p: Pos2, center: Pos2, degrees: f32) -> Pos2 {
    if degrees == 0.0 {
        return p;
    }
    center + egui::emath::Rot2::from_angle(degrees.to_radians()) * (p - center)
}

/// Angle (degrees) of the pointer seen from `center`.
pub(super) fn pointer_angle(p: Pos2, center: Pos2) -> f32 {
    let d = p - center;
    d.y.atan2(d.x).to_degrees()
}

/// Position of the rotation handle, in the unrotated frame (screen).
pub(super) fn rotate_handle_pos(r: Rect) -> Pos2 {
    pos2(r.center().x, r.top() - ROTATE_HANDLE_DISTANCE)
}

/// Accent frame with its 8 handles, rotated by `angle` around its center,
/// optionally the round rotation handle on its stem, and the "W × H" pill
/// under it.
pub(super) fn draw_frame(
    painter: &Painter,
    r: Rect,
    angle: f32,
    rotate_handle: bool,
    (w, h): (u32, u32),
) {
    let accent = ui_kit::color::ACCENT;
    let stroke = UiStroke::new(1.5, accent);
    let turn = |p: Pos2| rotate_around(p, r.center(), angle);
    let corners: Vec<Pos2> = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ]
    .into_iter()
    .map(turn)
    .collect();
    let bottom = corners.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    painter.add(egui::Shape::closed_line(corners, stroke));

    let knob = turn(rotate_handle_pos(r));
    if rotate_handle {
        let top = turn(pos2(r.center().x, r.top() - 4.5));
        painter.line_segment([top, knob], stroke);
    }
    for (_, p) in handle_positions(r) {
        let h = Rect::from_center_size(turn(p), Vec2::splat(9.0));
        painter.rect(h, 2.0, Color32::WHITE, stroke, StrokeKind::Inside);
    }
    if rotate_handle {
        painter.circle(knob, 5.5, Color32::WHITE, stroke);
    }

    ui_kit::pill(
        painter,
        pos2(r.center().x, bottom + 12.0),
        egui::Align2::CENTER_TOP,
        &format!("{w} × {h}"),
    );
}

/// Height of the document tab strip above the canvas.
const TAB_STRIP_HEIGHT: f32 = 34.0;
/// Longest tab name, and how short names get when tabs don't fit.
const TAB_NAME_MAX: f32 = 180.0;
const TAB_NAME_MIN: f32 = 48.0;
/// Room for the "+" button after the tabs.
const TAB_PLUS_WIDTH: f32 = 40.0;

impl AtelierApp {
    /// Tab strip: one tab per open document (name, zoom, unsaved dot /
    /// close; middle click closes too) and a "+" button that opens an image
    /// in a new tab.
    fn document_tabs(&mut self, ui: &mut egui::Ui, strip: Rect) {
        use ui_kit::color;
        let painter = ui.painter_at(strip);
        painter.rect_filled(strip, 0.0, color::BG1);
        let font = egui::FontId::proportional(ui_kit::TEXT_SIZE);
        let name_width = |name: &str| {
            painter
                .layout_no_wrap(name.to_owned(), font.clone(), color::TEXT)
                .size()
                .x
        };
        // Each tab: name, zoom, close button.
        let tabs: Vec<(String, f32, String, f32)> = (0..self.tab_count())
            .map_while(|i| self.tab(i))
            .map(|(doc, zoom)| {
                let name = doc.display_name();
                let zoom = zoom_label(zoom);
                let zoom_w = painter
                    .layout_no_wrap(zoom.clone(), ui_kit::mono(12.0), color::LABEL)
                    .size()
                    .x;
                let natural = name_width(&name).min(TAB_NAME_MAX);
                (name, natural, zoom, 14.0 + 8.0 + zoom_w + 8.0 + 20.0 + 8.0)
            })
            .collect();
        // Too many to fit: the names get shorter (all the same maximum),
        // then the strip scrolls, keeping the active tab in view; "+" stays
        // at the right end.
        let room = strip.width() - TAB_PLUS_WIDTH;
        let total = |cap: f32| -> f32 { tabs.iter().map(|t| t.1.min(cap) + t.3).sum() };
        let mut cap = TAB_NAME_MAX;
        if total(cap) > room {
            let (mut lo, mut hi) = (TAB_NAME_MIN, TAB_NAME_MAX);
            for _ in 0..16 {
                let mid = (lo + hi) / 2.0;
                if total(mid) > room {
                    hi = mid
                } else {
                    lo = mid
                }
            }
            cap = lo;
        }
        let widths: Vec<f32> = tabs.iter().map(|t| t.1.min(cap) + t.3).collect();
        let overflow = (total(cap) - room).max(0.0);
        let scroll_id = ui.id().with("doc-tabs-scroll");
        let mut scroll: f32 = ui.data(|d| d.get_temp(scroll_id)).unwrap_or_default();
        if ui.rect_contains_pointer(strip) {
            let delta = ui.input(|i| i.smooth_scroll_delta);
            scroll -= delta.x + delta.y;
        }
        // A tab that becomes active scrolls into view.
        let last_active = ui.data(|d| d.get_temp::<usize>(scroll_id.with("active")));
        if last_active != Some(self.active_tab)
            && let Some(w) = widths.get(self.active_tab)
        {
            let left: f32 = widths[..self.active_tab].iter().sum();
            scroll = scroll.clamp(left + w - room, left);
        }
        let scroll = scroll.clamp(0.0, overflow);
        ui.data_mut(|d| {
            d.insert_temp(scroll_id, scroll);
            d.insert_temp(scroll_id.with("active"), self.active_tab);
        });

        let clip = Rect::from_min_max(strip.min, pos2(strip.left() + room, strip.bottom()));
        let painter = ui.painter_at(clip);
        let mut x = strip.left() - scroll;
        let (mut switch, mut close) = (None, None);
        for (i, ((name, _, zoom, _), width)) in tabs.into_iter().zip(widths).enumerate() {
            let tab = Rect::from_min_size(pos2(x, strip.top()), vec2(width, strip.height()));
            x = tab.right();
            if !tab.intersects(clip) {
                continue;
            }
            let Some((doc, _)) = self.tab(i) else { break };
            let active = i == self.active_tab;
            let name =
                painter.layout_no_wrap(shorten(&painter, &name, cap), font.clone(), color::TEXT);
            let name_w = name.size().x;
            let zoom = painter.layout_no_wrap(zoom, ui_kit::mono(12.0), Color32::PLACEHOLDER);
            let response = ui.interact(
                tab.intersect(clip),
                ui.id().with(("doc-tab", i)),
                Sense::click(),
            );
            let fill = if active {
                color::BG0
            } else if response.hovered() {
                color::BG2
            } else {
                color::BG1
            };
            painter.rect_filled(tab, 0.0, fill);
            if active {
                let top = Rect::from_min_size(tab.min, vec2(tab.width(), 2.0));
                painter.rect_filled(top, 0.0, color::ACCENT);
            }
            painter.vline(
                tab.right() - 0.5,
                tab.y_range(),
                UiStroke::new(1.0, color::LINE),
            );
            let text = if active { color::TEXT } else { color::MUTED };
            let mid = tab.center().y;
            let mut at = tab.left() + 14.0;
            painter.galley(pos2(at, mid - name.size().y / 2.0), name, text);
            at += name_w + 8.0;
            painter.galley(pos2(at, mid - zoom.size().y / 2.0), zoom, color::LABEL);
            if response.clicked() {
                switch = Some(i);
            }
            if response.middle_clicked() {
                close = Some(i);
            }
            if let Some(path) = &doc.path {
                response.on_hover_text(path.display().to_string());
            }

            // Close button; while unsaved, a dot until hovered.
            let button = Rect::from_center_size(pos2(tab.right() - 18.0, mid), Vec2::splat(20.0));
            if !clip.contains_rect(button) {
                continue;
            }
            let response = ui.interact(button, ui.id().with(("doc-tab-close", i)), Sense::click());
            if response.hovered() {
                painter.rect_filled(button, 5.0, color::BG3);
            }
            if doc.modified && !response.hovered() {
                painter.circle_filled(button.center(), 4.0, color::MUTED);
            } else {
                let fg = if response.hovered() {
                    color::TEXT
                } else {
                    color::LABEL
                };
                painter.text(
                    button.center(),
                    egui::Align2::CENTER_CENTER,
                    icon::X,
                    egui::FontId::proportional(12.0),
                    fg,
                );
            }
            let key = keys::shortcut(ui.ctx(), &SHORTCUT_CLOSE);
            let response = ui_kit::tip_with_key(response, &t!("doc-tab-close"), &key);
            if response.clicked() {
                close = Some(i);
            }
        }
        ui.painter_at(strip).hline(
            strip.x_range(),
            strip.bottom() - 0.5,
            UiStroke::new(1.0, color::LINE),
        );

        let plus_x = x.min(clip.right()) + 6.0;
        let plus = Rect::from_min_size(pos2(plus_x, strip.top() + 3.0), Vec2::splat(28.0));
        let response = ui.put(plus, |ui: &mut egui::Ui| {
            ui_kit::icon_button(ui, icon::PLUS, "", true)
        });
        let key = keys::shortcut(ui.ctx(), &SHORTCUT_OPEN);
        if ui_kit::tip_with_key(response, &t!("menu-file-open"), &key).clicked() {
            self.run(Action::Open);
        }
        if let Some(i) = close.or(switch) {
            self.switch_tab(i);
        }
        if close.is_some() {
            self.run(Action::CloseDocument);
        }
    }

    /// Floating zoom bar at the bottom center: −, zoom, +, Fit, 1:1.
    fn zoom_bar(&mut self, ui: &mut egui::Ui, area: Rect) {
        use ui_kit::color;
        let (bar, actual_w) = zoom_bar_geometry(ui, area);
        let painter = ui.painter_at(area);
        painter.add(
            egui::epaint::Shadow {
                offset: [0, 10],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(115),
            }
            .as_shape(bar, 10.0),
        );
        painter.rect(
            bar,
            10.0,
            color::BG2.gamma_multiply(0.92),
            UiStroke::new(1.0, color::LINE),
            StrokeKind::Inside,
        );

        let builder = egui::UiBuilder::new()
            .max_rect(bar.shrink(4.0))
            .layout(egui::Layout::left_to_right(egui::Align::Center));
        let mut zoom_to = None;
        ui.scope_builder(builder, |ui| {
            ui.spacing_mut().item_spacing.x = ZOOM_BAR_GAP;
            let ctx = ui.ctx().clone();
            let key = |s: &KeyboardShortcut| keys::shortcut(&ctx, s);
            let out = ui_kit::icon_button(ui, icon::MINUS, "", true);
            if ui_kit::tip_with_key(out, &t!("menu-view-zoom-out"), &key(&SHORTCUT_ZOOM_OUT))
                .clicked()
            {
                zoom_to = Some(Action::ZoomOut);
            }
            let (label, _) = ui.allocate_exact_size(vec2(56.0, 28.0), Sense::hover());
            ui.painter().text(
                label.center(),
                egui::Align2::CENTER_CENTER,
                zoom_label(self.zoom),
                ui_kit::mono(12.0),
                color::TEXT,
            );
            let zoom_in = ui_kit::icon_button(ui, icon::PLUS, "", true);
            if ui_kit::tip_with_key(zoom_in, &t!("menu-view-zoom-in"), &key(&SHORTCUT_ZOOM_IN))
                .clicked()
            {
                zoom_to = Some(Action::ZoomIn);
            }
            let (sep, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
            ui.painter().rect_filled(
                Rect::from_center_size(sep.center(), vec2(1.0, 20.0)),
                0.0,
                color::LINE,
            );
            let fit = ui_kit::icon_button(ui, icon::CORNERS_OUT, "", true);
            if ui_kit::tip_with_key(fit, &t!("menu-view-fit-on-screen"), &key(&SHORTCUT_FIT))
                .clicked()
            {
                zoom_to = Some(Action::Fit);
            }
            let actual = plain_button(ui, "1:1", actual_w);
            if ui_kit::tip_with_key(actual, &t!("zoombar-actual-pixels"), &key(&SHORTCUT_ACTUAL))
                .clicked()
            {
                zoom_to = Some(Action::ActualSize);
            }
        });
        if let Some(action) = zoom_to {
            self.run(action);
        }
    }

    /// Closes the document (after "Save changes?" if needed, see `run`).
    pub(super) fn close_document(&mut self) {
        self.reset_pending();
        self.leave_select_modes();
        self.forget_document_jobs();
        self.doc = None;
        self.stroke = None;
        self.selection = None;
        self.last_selection = None;
        self.marquee = None;
        self.marquee_preview = None;
        self.select_move = None;
        self.lasso = None;
        self.layer_set.clear();
        self.transparency = None;
        self.split_open = false;
        self.hover_pixel = None;
        self.prop_session = None;
        self.edges = Default::default();
        self.canvas_view = None;
        self.lifted = None;
        self.thumbs.clear();
        self.fit_pending = Some(1.0);
        self.activate_neighbor_tab();
    }
}

const ZOOM_BAR_GAP: f32 = 2.0;

/// Zoom bar: its rectangle (bottom center of `area`) and the width of its
/// "1:1" button.
fn zoom_bar_geometry(ui: &egui::Ui, area: Rect) -> (Rect, f32) {
    let font = ui_kit::medium(ui_kit::TEXT_SIZE);
    let actual_w = ui
        .painter()
        .layout_no_wrap("1:1".to_owned(), font, ui_kit::color::TEXT)
        .size()
        .x
        + 20.0;
    let width = 4.0 + 28.0 + 56.0 + 28.0 + 9.0 + 28.0 + actual_w + 5.0 * ZOOM_BAR_GAP + 4.0;
    let bar = Rect::from_center_size(
        pos2(area.center().x, area.bottom() - 16.0 - 18.0),
        vec2(width, 36.0),
    );
    (bar, actual_w)
}

/// Frameless text button of the zoom bar.
fn plain_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    use ui_kit::color;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    let fg = if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, color::BG3);
        color::TEXT
    } else {
        color::MUTED
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        ui_kit::medium(ui_kit::TEXT_SIZE),
        fg,
    );
    response
}

/// Canvas area background: darkest color with a dot grid fixed to the screen.
fn paint_backdrop(painter: &Painter, area: Rect) {
    painter.rect_filled(area, 0.0, ui_kit::color::BG0);
    const SPACING: f32 = 24.0;
    let mut dots = egui::epaint::Mesh::default();
    let mut y = area.top() + SPACING / 2.0;
    while y < area.bottom() {
        let mut x = area.left() + SPACING / 2.0;
        while x < area.right() {
            let dot = Rect::from_center_size(pos2(x.round(), y.round()), Vec2::splat(2.0));
            dots.add_colored_rect(dot, ui_kit::color::DOT);
            x += SPACING;
        }
        y += SPACING;
    }
    painter.add(dots);
}

/// Zoom as a percentage ("100%", "33%"; one decimal below 10%), in the
/// language's format.
pub(super) fn zoom_label(zoom: f32) -> String {
    let percent = f64::from(zoom) * 100.0;
    i18n::percent(i18n::decimal(percent, if percent < 10.0 { 1 } else { 0 }))
}

/// `name` shortened with "…" to fit in `max` points (tab names).
fn shorten(painter: &egui::Painter, name: &str, max: f32) -> String {
    let fits = |text: &str| {
        let font = egui::FontId::proportional(ui_kit::TEXT_SIZE);
        painter
            .layout_no_wrap(text.to_owned(), font, ui_kit::color::TEXT)
            .size()
            .x
            <= max
    };
    if fits(name) {
        return name.to_owned();
    }
    let mut chars: Vec<char> = name.chars().collect();
    while chars.len() > 1 && !fits(&format!("{}…", String::from_iter(&chars))) {
        chars.pop();
    }
    format!("{}…", String::from_iter(&chars))
}
