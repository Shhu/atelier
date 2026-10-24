//! Floating windows: split into elements, color to transparency, image
//! adjustments, fill, image size, confirmations.

use std::ops::RangeInclusive;

use super::*;
use crate::i18n::t;
use ui_kit::color;

impl AtelierApp {
    /// Elements of the active layer for the chosen gap (recomputed if needed).
    pub(super) fn split_boxes(&mut self) -> Option<&[Rect]> {
        let layer = self.doc.as_ref()?.active_layer();
        let key = hash_of((layer.id, layer.rev(), layer.offset, self.split_gap));
        if self.split_preview.as_ref().is_none_or(|(k, _)| *k != key) {
            let (ox, oy) = (layer.offset.0 as f32, layer.offset.1 as f32);
            let boxes = group_pixels(layer.image(), self.split_gap)
                .boxes
                .into_iter()
                .map(|[x, y, w, h]| {
                    Rect::from_min_size(
                        pos2(ox + x as f32, oy + y as f32),
                        vec2(w as f32, h as f32),
                    )
                })
                .collect();
            self.split_preview = Some((key, boxes));
        }
        self.split_preview.as_ref().map(|(_, b)| b.as_slice())
    }

    pub(super) fn apply_split(&mut self) {
        let gap = self.split_gap;
        let Some(doc) = &mut self.doc else { return };
        let name = doc.active_layer().name.clone();
        let n = doc.split_active(gap);
        self.status = match n {
            0 => t!("dialog-split-empty", name = name),
            1 => t!("dialog-split-single", name = name),
            n => t!("dialog-split-done", name = name, count = n),
        };
        if n >= 2 {
            self.split_open = false;
            self.tool = Tool::Transform;
            self.reset_pending();
        }
    }

    pub(super) fn split_window(&mut self, ctx: &egui::Context) {
        if !self.split_open {
            return;
        }
        let Some(doc) = &self.doc else {
            self.split_open = false;
            return;
        };
        let layers: Vec<String> = doc.layers.iter().map(|l| l.name.clone()).collect();
        let active = doc.active;
        let count = self.split_boxes().map_or(0, <[Rect]>::len);

        let mut pick = None;
        let mut gap = self.split_gap as f32;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                ok: t!("dialog-split-apply"),
                ok_enabled: count >= 2,
                ..DialogSpec::new("split", t!("dialog-split-title"), tool_dialog_pos(ctx))
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-split-layer"));
                    let width = ui.available_width();
                    ui_kit::combo(ui, "split_layer", None, &layers[active], width, |ui| {
                        for (i, name) in layers.iter().enumerate().rev() {
                            if ui.selectable_label(i == active, name).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                });
                gap_between_blocks(ui);
                let label = t!("dialog-split-gap");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut gap,
                    1.0..=300.0,
                    " px",
                    Track::Accent,
                    8.0,
                );
                gap_between_blocks(ui);
                help(ui, &t!("dialog-split-gap-hint"));
                gap_between_blocks(ui);
                result_banner(
                    ui,
                    &match count {
                        0 => t!("dialog-split-no-pixels"),
                        n => t!("dialog-split-found", count = n),
                    },
                );
            },
        );
        self.split_gap = gap.round() as u32;
        if let Some(i) = pick {
            self.select_layer(i);
        }
        if out.ok {
            self.apply_split();
        }
        if out.cancel {
            self.split_open = false;
        }
    }

    pub(super) fn transparency_window(&mut self, ctx: &egui::Context) {
        let Some(mut tr) = self.transparency else {
            return;
        };
        let reset = Transparency {
            picking: tr.picking,
            preview: tr.preview,
            ..TRANSPARENCY_DEFAULT
        };
        let mut preview = tr.preview;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                reset: Some(tr != reset),
                preview: Some(&mut preview),
                ok: t!("dialog-apply"),
                ..DialogSpec::new(
                    "transparency",
                    t!("dialog-transparency-title"),
                    tool_dialog_pos(ctx),
                )
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-transparency-color"));
                    color_to_remove(ui, &mut tr.color, &mut tr.picking);
                    help(ui, &t!("dialog-transparency-hint"));
                });
                gap_between_blocks(ui);
                let mut tolerance = tr.tolerance * 100.0;
                let label = t!("dialog-transparency-tolerance");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut tolerance,
                    0.0..=100.0,
                    &i18n::percent(""),
                    Track::Accent,
                    8.0,
                );
                tr.tolerance = tolerance / 100.0;
                gap_between_blocks(ui);
                let mut softness = tr.softness * 100.0;
                let label = t!("dialog-transparency-softness");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut softness,
                    0.0..=100.0,
                    &i18n::percent(""),
                    Track::Accent,
                    15.0,
                );
                tr.softness = softness / 100.0;
            },
        );
        tr.preview = preview;
        if out.reset {
            tr = reset;
        }
        self.transparency = Some(tr);
        if out.ok || out.cancel {
            self.close_transparency(out.ok);
        }
    }

    /// Closes "Make a color transparent" (OK with `apply`, else Cancel).
    pub(super) fn close_transparency(&mut self, apply: bool) {
        let Some(tr) = self.transparency.take() else {
            return;
        };
        if apply {
            let [r, g, b, _] = tr.color.to_srgba_unmultiplied();
            if let Some(doc) = &mut self.doc
                && doc.color_to_alpha_active(
                    [r, g, b],
                    tr.tolerance,
                    tr.softness,
                    self.selection.as_ref(),
                )
            {
                self.status = t!("dialog-transparency-done", color = hex_color([r, g, b]));
            }
        }
        self.end_layer_preview();
        self.reset_pending();
    }

    /// Histograms of what an adjustment changes: the active layer's pixels
    /// (its mask's when targeted), inside the selection (computed once per
    /// pixel revision and selection).
    fn active_histogram(&mut self) -> Option<Histograms> {
        let doc = self.doc.as_ref()?;
        let layer = doc.active_layer();
        let selection = self.selection.as_ref().map(|s| {
            let r = s.rect;
            let mask = s.mask.as_ref().map_or(0, |m| Arc::as_ptr(m) as usize);
            (r.x, r.y, r.w, r.h, mask, s.inverted)
        });
        let key = (
            layer.id,
            hash_of((layer.rev(), doc.mask_targeted(), selection)),
        );
        if self.histogram.as_ref().is_none_or(|(k, _)| *k != key) {
            let target = super::filters_ui::active_target(doc, self.selection.as_ref())?;
            let h = crate::pixel_ops::histogram_within(&target.image, target.clip.as_ref());
            self.histogram = Some((key, h));
        }
        self.histogram.as_ref().map(|(_, h)| *h)
    }

    /// Image adjustment (see [`AdjustKind`]): blocks the rest of the window
    /// but without a backdrop, to see the corrected image live.
    pub(super) fn adjust_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.adjust.take() else {
            return;
        };
        let histogram = matches!(
            dialog.adjustment,
            Adjustment::Levels(_) | Adjustment::Curves(_)
        )
        .then(|| self.active_histogram())
        .flatten();
        let primary = self.primary.to_srgba_unmultiplied();
        // Alt+2..5 (like Photoshop): RGB, red, green, blue channel.
        if matches!(
            dialog.adjustment,
            Adjustment::Levels(_) | Adjustment::Curves(_)
        ) && let Some(digit @ 2..=5) = ctx.input(|i| digit_press(i, Modifiers::ALT))
            && digit as usize - 2 != dialog.range
        {
            dialog.range = digit as usize - 2;
            dialog.point = None;
        }
        let reset = dialog.adjustment.reset();
        let mut preview = dialog.preview;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                reset: Some(dialog.adjustment != reset),
                preview: Some(&mut preview),
                // Their keys move the points and stops.
                first_field: !matches!(
                    dialog.adjustment,
                    Adjustment::Curves(_) | Adjustment::GradientMap(_)
                ),
                ..DialogSpec::new(
                    "adjustment",
                    dialog.adjustment.title(),
                    tool_dialog_pos(ctx),
                )
            },
            |ui| {
                let (range, point) = (&mut dialog.range, &mut dialog.point);
                adjustment_editor(
                    ui,
                    &mut dialog.adjustment,
                    range,
                    point,
                    histogram.as_ref(),
                    primary,
                );
            },
        );
        dialog.preview = preview;
        if out.reset {
            dialog.adjustment = reset;
            dialog.point = None;
        }
        if out.ok {
            let adjustment = dialog.adjustment.clone();
            self.record(crate::actions::Recorded::Adjust { adjustment });
        }
        if out.ok
            && let Some(doc) = &mut self.doc
            && doc.adjust_active(&dialog.adjustment, self.selection.as_ref())
        {
            self.status = t!("dialog-adjust-applied", name = dialog.adjustment.title());
        }
        self.adjust = Some(dialog);
        if out.ok || out.cancel {
            self.adjust = None;
            self.end_layer_preview();
            self.reset_pending();
        }
    }

    /// Fill… (Shift+F5): contents, opacity, preserve transparency.
    pub(super) fn fill_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.fill_dialog else {
            return;
        };
        let center = ctx.content_rect().center();
        // Content-Aware fills a selection.
        let content_aware = d.content == FillContent::ContentAware;
        let unselected = content_aware && self.selection.is_none();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok_enabled: !unselected,
                note: unselected.then(|| t!("retouch-needs-selection")),
                ..DialogSpec::new("fill", t!("dialog-fill-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-fill-contents"));
                    let width = ui.available_width();
                    let shown = d.content.label();
                    ui_kit::combo(ui, "fill_content", None, &shown, width, |ui| {
                        for c in FillContent::ALL {
                            ui.selectable_value(&mut d.content, c, c.label());
                        }
                    });
                    if d.content == FillContent::Color {
                        ui.horizontal(|ui| {
                            ui_kit::swatch(ui, &mut d.color, 28.0, &t!("dialog-fill-color-tip"));
                            let [r, g, b, _] = d.color.to_srgba_unmultiplied();
                            ui.label(
                                egui::RichText::new(hex_color([r, g, b])).font(ui_kit::mono(12.0)),
                            );
                        });
                    }
                    if d.content == FillContent::ContentAware {
                        ui_kit::checkbox(ui, &mut d.adapt, &t!("retouch-color-adaptation"))
                            .on_hover_text(t!("retouch-color-adaptation-tip"));
                    }
                });
                gap_between_blocks(ui);
                let mut opacity = d.opacity * 100.0;
                let label = t!("dialog-fill-opacity");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut opacity,
                    0.0..=100.0,
                    &i18n::percent(""),
                    Track::Accent,
                    100.0,
                );
                d.opacity = opacity / 100.0;
                gap_between_blocks(ui);
                ui_kit::checkbox(
                    ui,
                    &mut d.preserve_transparency,
                    &t!("dialog-fill-preserve-transparency"),
                )
                .on_hover_text(t!("dialog-fill-preserve-transparency-tip"));
            },
        );
        self.fill_dialog = Some(d);
        if out.ok {
            self.apply_fill(d);
        }
        if out.ok || out.cancel {
            self.fill_dialog = None;
        }
    }

    /// Fill… confirmed (or played by an action) with the settings `d`.
    pub(super) fn apply_fill(&mut self, d: FillDialog) {
        if self.refuse_hidden_layer() {
            return;
        }
        self.record(crate::actions::Recorded::Fill {
            contents: actions_panel::contents(d.content),
            color: actions_panel::rgb(d.color),
            opacity: d.opacity,
            preserve_transparency: d.preserve_transparency,
        });
        if d.content == FillContent::ContentAware {
            // The AI model only when it is there: no question here.
            let settings = retouch_tools::FillSettings {
                adapt: d.adapt,
                output: retouch_tools::Output::CurrentLayer,
                ai: self.retouch.settings.ai && crate::ai::models::Model::Inpaint.ready(),
                ..self.retouch.settings
            };
            self.start_content_aware(retouch_tools::FillRequest {
                settings,
                opacity: d.opacity,
                keep_alpha: d.preserve_transparency,
                from_fill: true,
            });
        } else if !self.refuse_if_target_locked() {
            let (color, which) = match d.content {
                FillContent::Foreground => (self.primary, t!("dialog-fill-with-foreground")),
                FillContent::Background => (self.secondary, t!("dialog-fill-with-background")),
                FillContent::Color => {
                    let [r, g, b, _] = d.color.to_srgba_unmultiplied();
                    let color = hex_color([r, g, b]);
                    (d.color, t!("dialog-fill-with-color", color = color))
                }
                FillContent::Black => (Color32::BLACK, t!("dialog-fill-with-black")),
                FillContent::Gray => (Color32::from_gray(128), t!("dialog-fill-with-gray")),
                FillContent::White => (Color32::WHITE, t!("dialog-fill-with-white")),
                FillContent::ContentAware => unreachable!("filled above"),
            };
            self.fill(color, &which, d.opacity, d.preserve_transparency);
        }
    }

    /// Image Size (Ctrl+Alt+I): new dimensions, proportions,
    /// resampling.
    pub(super) fn image_size_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.image_size else {
            return;
        };
        let Some((cw, ch)) = self.doc.as_ref().map(|doc| (doc.width(), doc.height())) else {
            self.image_size = None;
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("image_size", t!("dialog-image-size-title"), center)
            },
            |ui| {
                ui_kit::key_value(
                    ui,
                    &t!("dialog-image-size-current"),
                    &format!("{cw} × {ch} px"),
                );
                gap_between_blocks(ui);
                let ratio = cw as f64 / ch as f64;
                let fit = |v: f64| (v.round() as u32).clamp(1, MAX_DIMENSION);
                let half = (ui.available_width() - 8.0) / 2.0;
                let (mut w_changed, mut h_changed) = (false, false);
                ui.horizontal(|ui| {
                    let size = 1..=MAX_DIMENSION;
                    let w = t!("optbar-width-short");
                    let h = t!("optbar-height-short");
                    w_changed =
                        ui_kit::number_field(ui, &w, &mut d.width, size.clone(), " px", half)
                            .changed();
                    h_changed =
                        ui_kit::number_field(ui, &h, &mut d.height, size, " px", half).changed();
                });
                if d.keep_ratio && w_changed {
                    d.height = fit(d.width as f64 / ratio);
                } else if d.keep_ratio && h_changed {
                    d.width = fit(d.height as f64 * ratio);
                }
                let constrain = t!("dialog-image-size-constrain");
                if ui_kit::checkbox(ui, &mut d.keep_ratio, &constrain).changed() && d.keep_ratio {
                    d.height = fit(d.width as f64 / ratio);
                }
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-image-size-resample"));
                    let width = ui.available_width();
                    let shown = d.filter.label();
                    ui_kit::combo(ui, "image_size_filter", None, &shown, width, |ui| {
                        for f in ResizeFilter::ALL {
                            ui.selectable_value(&mut d.filter, f, f.label());
                        }
                    });
                });
                gap_between_blocks(ui);
                help(ui, &t!("dialog-image-size-hint"));
            },
        );
        self.image_size = Some(d);
        if out.ok {
            self.apply_image_size(d);
        }
        if out.ok || out.cancel {
            self.image_size = None;
        }
    }

    /// Image Rotation: the whole image turns (clockwise), the canvas
    /// follows; the selection is dropped.
    pub(super) fn rotate_canvas(&mut self, degrees: f32) {
        self.record(crate::actions::Recorded::RotateCanvas { angle: degrees });
        let texts = self.texts_before();
        let Some(doc) = &mut self.doc else { return };
        let old = (doc.width(), doc.height());
        if doc.rotate_canvas(degrees) {
            let map = type_tool::canvas_rotation(old, (doc.width(), doc.height()), degrees);
            self.texts_after(texts, map);
            let angle = i18n::short_decimal(f64::from(degrees.rem_euclid(360.0)));
            self.status = t!("status-image-rotated", angle = angle);
            self.forget_selection();
            self.reset_pending();
        }
    }

    /// Canvas Size (Ctrl+Alt+C), like Photoshop: new size (or what to add
    /// with Relative), anchor, color of the added area.
    pub(super) fn canvas_size_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.canvas_size else {
            return;
        };
        let Some((cw, ch)) = self
            .doc
            .as_ref()
            .map(|doc| (doc.width() as i64, doc.height() as i64))
        else {
            self.canvas_size = None;
            return;
        };
        let center = ctx.content_rect().center();
        let max = MAX_DIMENSION as i64;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("canvas_size", t!("dialog-canvas-size-title"), center)
            },
            |ui| {
                ui_kit::key_value(
                    ui,
                    &t!("dialog-canvas-size-current"),
                    &format!("{cw} × {ch} px"),
                );
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-canvas-size-new"));
                    let half = (ui.available_width() - 8.0) / 2.0;
                    let (w_range, h_range) = if d.relative {
                        (1 - cw..=max - cw, 1 - ch..=max - ch)
                    } else {
                        (1..=max, 1..=max)
                    };
                    ui.horizontal(|ui| {
                        let w = t!("optbar-width-short");
                        let h = t!("optbar-height-short");
                        ui_kit::number_field(ui, &w, &mut d.width, w_range, " px", half);
                        ui_kit::number_field(ui, &h, &mut d.height, h_range, " px", half);
                    });
                    let was = d.relative;
                    ui_kit::checkbox(ui, &mut d.relative, &t!("dialog-canvas-size-relative"));
                    // Same size, other way of writing it.
                    if d.relative != was {
                        let sign = if d.relative { -1 } else { 1 };
                        d.width += sign * cw;
                        d.height += sign * ch;
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-canvas-size-anchor"));
                    anchor_grid(ui, &mut d.anchor);
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("dialog-canvas-size-extension"));
                    let width = ui.available_width();
                    let label = |c: Option<FillContent>| {
                        c.map_or_else(|| t!("dialog-canvas-size-transparent"), FillContent::label)
                    };
                    let options = [
                        None,
                        Some(FillContent::Foreground),
                        Some(FillContent::Background),
                        Some(FillContent::White),
                        Some(FillContent::Black),
                        Some(FillContent::Gray),
                    ];
                    let shown = label(d.extension);
                    ui_kit::combo(ui, "canvas_extension", None, &shown, width, |ui| {
                        for c in options {
                            ui.selectable_value(&mut d.extension, c, label(c));
                        }
                    });
                });
            },
        );
        self.canvas_size = Some(d);
        if out.ok {
            self.apply_canvas_size(d);
        }
        if out.ok || out.cancel {
            self.canvas_size = None;
        }
    }

    /// Canvas Size confirmed (or played by an action) with the settings `d`.
    pub(super) fn apply_canvas_size(&mut self, d: CanvasSizeDialog) {
        let Some((cw, ch)) = self
            .doc
            .as_ref()
            .map(|doc| (doc.width() as i64, doc.height() as i64))
        else {
            return;
        };
        let max = MAX_DIMENSION as i64;
        self.record(crate::actions::Recorded::CanvasSize {
            width: d.width,
            height: d.height,
            relative: d.relative,
            anchor: [d.anchor.0, d.anchor.1],
            extension: d.extension.map(actions_panel::contents),
        });
        let (w, h) = if d.relative {
            (cw + d.width, ch + d.height)
        } else {
            (d.width, d.height)
        };
        let (w, h) = (w.clamp(1, max), h.clamp(1, max));
        // Origin of the new canvas in the current one.
        let x = (cw - w) * d.anchor.0 as i64 / 2;
        let y = (ch - h) * d.anchor.1 as i64 / 2;
        let color = d.extension.map(|c| match c {
            FillContent::Foreground => self.primary,
            FillContent::Background => self.secondary,
            FillContent::Black => Color32::BLACK,
            FillContent::Gray => Color32::from_gray(128),
            _ => Color32::WHITE,
        });
        if let Some(doc) = &mut self.doc {
            doc.resize_canvas(x, y, w as u32, h as u32);
            let old = crate::document::PixelRect {
                x: -x,
                y: -y,
                w: cw as u32,
                h: ch as u32,
            };
            if let Some(color) = color
                && (w > cw || h > ch)
            {
                doc.fill_canvas_extension(old, color.to_srgba_unmultiplied());
            }
            self.status = t!("status-canvas-resized", width = w, height = h);
            self.forget_selection();
            self.reset_pending();
        }
    }

    /// Image Rotation > Arbitrary: angle and direction.
    pub(super) fn rotate_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.rotate_dialog else {
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("rotate_canvas", t!("dialog-rotate-title"), center)
            },
            |ui| {
                ui.horizontal(|ui| {
                    let label = t!("dialog-rotate-angle");
                    ui_kit::number_field(ui, &label, &mut d.angle, 0.0..=359.9, "°", 140.0);
                    let (cw, ccw) = (t!("dialog-rotate-cw"), t!("dialog-rotate-ccw"));
                    ui_kit::segmented(ui, &mut d.clockwise, &[(true, &cw, ""), (false, &ccw, "")]);
                });
            },
        );
        self.rotate_dialog = Some(d);
        if out.ok {
            self.rotate_canvas(if d.clockwise { d.angle } else { -d.angle });
        }
        if out.ok || out.cancel {
            self.rotate_dialog = None;
        }
    }

    /// Flatten Image with hidden layers: "Discard hidden layers?", like
    /// Photoshop.
    pub(super) fn flatten_confirm_window(&mut self, ctx: &egui::Context) {
        if !self.flatten_confirm {
            return;
        }
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("flatten_confirm", t!("menu-layer-flatten"), center)
            },
            |ui| {
                ui.label(t!("doc-discard-hidden"));
            },
        );
        if out.ok || out.cancel {
            self.flatten_confirm = false;
        }
        if out.ok {
            // `run` only opened this question: the recording is made here.
            let before = self.doc.as_ref().map(|d| (d.uid(), d.history_step()));
            self.merge_layers(Action::FlattenImage);
            if let Some(command) = self.recorded_action(Action::FlattenImage) {
                self.record_done(command, before);
            }
        }
    }

    pub(super) fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.confirm else { return };
        let name = self
            .doc
            .as_ref()
            .map(Document::display_name)
            .unwrap_or_default();
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 400.0,
                ok: t!("dialog-confirm-save"),
                other: Some(t!("dialog-confirm-dont-save")),
                ..DialogSpec::new("confirm", t!("dialog-confirm-title"), center)
            },
            |ui| {
                ui.label(t!("dialog-confirm-text", name = name.as_str()));
                gap_between_blocks(ui);
                help(ui, &t!("dialog-confirm-hint"));
            },
        );
        if out.cancel {
            self.confirm = None;
        }
        if !out.ok && !out.other {
            return;
        }
        self.confirm = None;
        if out.ok {
            self.save();
            // A linked file asking first: the close waits for its answer.
            if let Some(ask) = &mut self.smart.link_save {
                ask.then = Some(pending);
                return;
            }
            if self.doc.as_ref().is_some_and(|d| d.modified) {
                return; // Save cancelled or failed: stay.
            }
        }
        match pending {
            // That document is dealt with: close it and try again (the next
            // unsaved one, if any, asks in turn).
            Confirm::Close => {
                self.close_document();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Confirm::CloseDocument => self.close_document(),
        }
    }
}

/// Settings "Make a Color Transparent" opens with (and resets to).
pub(super) const TRANSPARENCY_DEFAULT: Transparency = Transparency {
    color: Color32::WHITE,
    tolerance: 0.08,
    softness: 0.15,
    picking: true,
    preview: true,
};

/// Color of the RGB, red, green, blue channels (histograms, gradients, curves).
const CHANNEL_TINTS: [Color32; 4] = [
    Color32::WHITE,
    Color32::from_rgb(235, 70, 70),
    Color32::from_rgb(80, 210, 90),
    Color32::from_rgb(80, 140, 255),
];

// Colored slider tracks.
const CYAN: Color32 = Color32::from_rgb(0x1f, 0xb5, 0xc9);
const RED: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
const MAGENTA: Color32 = Color32::from_rgb(0xd1, 0x4b, 0xc4);
const GREEN: Color32 = Color32::from_rgb(0x3d, 0xb8, 0x6b);
const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc6, 0x3a);
const BLUE: Color32 = Color32::from_rgb(0x3b, 0x7b, 0xf6);
/// Middle of the color balance tracks.
const NEUTRAL: Color32 = color::LINE_HOVER;
const SATURATION: &[Color32] = &[
    Color32::from_rgb(0x6e, 0x70, 0x77),
    Color32::from_rgb(0xa8, 0x8f, 0x7e),
    Color32::from_rgb(0xf2, 0x60, 0x3c),
];
const LIGHTNESS: &[Color32] = &[
    Color32::BLACK,
    Color32::from_rgb(0x80, 0x80, 0x80),
    Color32::WHITE,
];
/// Hue wheel from 0° to 360° (colorize).
const HUES: [Color32; 7] = [
    Color32::from_rgb(255, 0, 0),
    Color32::from_rgb(255, 255, 0),
    Color32::from_rgb(0, 255, 0),
    Color32::from_rgb(0, 255, 255),
    Color32::from_rgb(0, 0, 255),
    Color32::from_rgb(255, 0, 255),
    Color32::from_rgb(255, 0, 0),
];
/// Hue shift from −180° to +180°: the colors red turns into.
const HUES_CENTERED: [Color32; 7] = [
    Color32::from_rgb(0, 255, 255),
    Color32::from_rgb(0, 0, 255),
    Color32::from_rgb(255, 0, 255),
    Color32::from_rgb(255, 0, 0),
    Color32::from_rgb(255, 255, 0),
    Color32::from_rgb(0, 255, 0),
    Color32::from_rgb(0, 255, 255),
];

/// Smallest size of a resized panel: its width, its body's height.
const MIN_PANEL_WIDTH: f32 = 220.0;
const MIN_PANEL_BODY: f32 = 80.0;

/// Where tool dialogs (adjustments, split, transparency) first appear: top
/// right of the canvas, next to the side panel.
pub(super) fn tool_dialog_pos(ctx: &egui::Context) -> Pos2 {
    let r = canvas_area(ctx);
    pos2(r.right() - 16.0, r.top() + 46.0)
}

fn work_area_id() -> egui::Id {
    egui::Id::new("work-area")
}

/// Records the canvas area (between the bars and panels) of this frame.
pub(super) fn set_work_area(ctx: &egui::Context, area: Rect) {
    ctx.data_mut(|d| d.insert_temp(work_area_id(), area));
}

/// The canvas area, or the whole window before it is known.
fn canvas_area(ctx: &egui::Context) -> Rect {
    ctx.data(|d| d.get_temp::<Rect>(work_area_id()))
        .unwrap_or_else(|| ctx.content_rect())
}

/// Where dialogs stay: the window's width, between the options bar and the
/// status bar, so that they never cover the menus.
/// A window too low for that gives them its whole height.
fn dialog_bounds(ctx: &egui::Context) -> Rect {
    let window = ctx.content_rect();
    let area = canvas_area(ctx);
    let bounds = if area.height() >= 240.0 {
        Rect::from_x_y_ranges(window.x_range(), area.y_range())
    } else {
        window
    };
    bounds.shrink(DIALOG_MARGIN)
}

/// Floating panels (History, Channels, Actions, Paths, Swatches, Keyboard
/// Shortcuts) whose place was chosen: the place, the frame it was chosen,
/// the last frame the panel showed.
#[derive(Clone, Default)]
struct PanelPlaces(Vec<(&'static str, Pos2, u64, u64)>);

fn panel_places_id() -> egui::Id {
    egui::Id::new("panel-places")
}

/// Where a floating panel first appears (its top right corner, with the
/// `RIGHT_TOP` pivot): next to the side panel, under the panels already
/// open, then in a new column on their left when the column is full.
pub(super) fn panel_pos(ctx: &egui::Context, id: &'static str) -> Pos2 {
    let pass = ctx.cumulative_pass_nr();
    let mut places = ctx.data(|d| {
        d.get_temp::<PanelPlaces>(panel_places_id())
            .unwrap_or_default()
    });
    // Closed panels give their place back.
    places.0.retain(|&(_, _, _, seen)| seen + 2 >= pass);
    let own = ctx.memory(|m| m.area_rect(egui::Id::new(id)));
    let others: Vec<Rect> = places
        .0
        .iter()
        .filter(|p| p.0 != id)
        .filter_map(|p| ctx.memory(|m| m.area_rect(egui::Id::new(p.0))))
        .collect();
    let pos = match places.0.iter_mut().find(|p| p.0 == id) {
        // Placed again in its first frames, once its height is known.
        Some(place) if pass <= place.2 + 2 => {
            let height = own.map_or(160.0, |r| r.height());
            place.1 = free_place(ctx, &others, height);
            place.3 = pass;
            place.1
        }
        Some(place) => {
            place.3 = pass;
            place.1
        }
        None => {
            // The size it had last time, if it was open before.
            let height = own.map_or(160.0, |r| r.height());
            let pos = free_place(ctx, &others, height);
            places.0.push((id, pos, pass, pass));
            // Opened from a menu: in front, even over a panel moved there.
            ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(id)));
            pos
        }
    };
    ctx.data_mut(|d| d.insert_temp(panel_places_id(), places));
    pos
}

/// The first place, from the top right of the canvas, down then leftwards,
/// where a panel `height` high covers none of `others`.
fn free_place(ctx: &egui::Context, others: &[Rect], height: f32) -> Pos2 {
    const WIDTH: f32 = 300.0;
    let start = tool_dialog_pos(ctx);
    let bottom = dialog_bounds(ctx).bottom();
    // New columns stay over the canvas, not over the tool bar.
    let left = canvas_area(ctx).left() + DIALOG_MARGIN;
    let mut pos = start;
    for _ in 0..16 {
        let probe = Rect::from_min_max(pos2(pos.x - WIDTH, pos.y), pos2(pos.x, pos.y + height));
        match others.iter().find(|r| r.intersects(probe)) {
            None => return pos,
            Some(r) if r.bottom() + 8.0 + height <= bottom => pos.y = r.bottom() + 8.0,
            Some(_) if pos.x - 2.0 * WIDTH - 8.0 >= left => {
                pos = pos2(pos.x - WIDTH - 8.0, start.y);
            }
            Some(_) => break,
        }
    }
    // No free place: cascaded from the last panel placed, its title bar
    // left in sight (never exactly over another one).
    let Some(last) = others.last() else {
        return start;
    };
    let mut pos = pos2(last.right() - CASCADE, last.top() + CASCADE);
    if pos.y + height > bottom || pos.x - WIDTH < left {
        pos = pos2(start.x - CASCADE, start.y);
    }
    for _ in 0..others.len() {
        if !others.iter().any(|r| (r.right_top() - pos).length() < 4.0) {
            break;
        }
        pos += vec2(-CASCADE, CASCADE);
    }
    pos
}

/// Offset of a floating panel cascaded over another (px).
const CASCADE: f32 = 28.0;

/// Modal dialogs that don't give the keyboard to their first field: those
/// with none (confirmations, alerts, progress), and those whose keys go to
/// something else or that are about choices rather than values. The others
/// open with it focused, so that "5 Enter" needs no mouse.
const NO_FIRST_FIELD: [&str; 23] = [
    "confirm",
    "flatten_confirm",
    "rasterize_confirm",
    "alert",
    "about",
    "actions_message",
    "batch",
    "batch_alert",
    "batch_progress",
    "image_processor",
    "artboards_replace",
    "model_prompt",
    "models_delete",
    "ai_models",
    "liquify_revert",
    // Letters choose their tools.
    "liquify",
    "camera_raw",
    // Mostly a matter of clicks; its hex field is a click away.
    "color_picker",
    "color_settings",
    "assign_profile",
    "convert_profile",
    "proof_setup",
    "grayscale",
];

/// The floating panels that Tab hides with the bars and the side panel.
const HIDDEN_WITH_PANELS: [&str; 5] = ["history", "channels", "actions", "paths", "swatches"];

fn panels_hidden_id() -> egui::Id {
    egui::Id::new("panels-hidden")
}

/// Tab (hide the panels) is on for this frame: the floating panels stay
/// open but aren't shown, and come back with the others.
pub(super) fn set_panels_hidden(ctx: &egui::Context, hidden: bool) {
    ctx.data_mut(|d| d.insert_temp(panels_hidden_id(), hidden));
}

/// Window › Reset Workspace: the floating panels go back to their first
/// place and size.
pub(super) fn forget_panel_places(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove::<PanelPlaces>(panel_places_id());
        for id in [
            "history",
            "channels",
            "actions",
            "paths",
            "swatches",
            "shortcuts",
            "diagnostics",
        ] {
            let id = egui::Id::new(id);
            d.remove::<Vec2>(id.with("offset"));
            d.remove::<Vec2>(id.with("size"));
        }
    });
}

/// What a floating dialog looks like and offers.
pub(super) struct DialogSpec<'a> {
    pub id: &'static str,
    pub title: String,
    /// Default spot (`pivot` of the window at `pos`); dragging the header
    /// moves it, for the session.
    pub pivot: egui::Align2,
    pub pos: Pos2,
    pub width: f32,
    /// Blocks the rest of the window while open (no backdrop: the image
    /// stays visible). Enter = OK, Esc = Cancel (for the other windows,
    /// `handle_shortcuts` does it).
    pub modal: bool,
    /// Reset button in the header (enabled or not); Alt also turns Cancel
    /// into Reset, like Photoshop.
    pub reset: Option<bool>,
    /// "Preview" checkbox in the footer.
    pub preview: Option<&'a mut bool>,
    pub ok: String,
    pub ok_enabled: bool,
    /// Second choice, between Cancel and OK (e.g. "Don't Save").
    pub other: Option<String>,
    /// Cancel button in the footer (windows with nothing to cancel go
    /// without).
    pub cancel: bool,
    /// Text on the left of the footer (instead of the preview checkbox).
    pub note: Option<String>,
    /// Footer shown (panels go without: their header closes them).
    pub footer: bool,
    /// A panel resized by its bottom right corner (for the session): the
    /// default height of its body, which then lays out in that height (its
    /// list scrolls itself).
    pub resize: Option<f32>,
    /// A modal: its first text field takes the keyboard when it opens.
    pub first_field: bool,
}

impl DialogSpec<'_> {
    pub(super) fn new(id: &'static str, title: String, pos: Pos2) -> Self {
        Self {
            id,
            title,
            pivot: egui::Align2::RIGHT_TOP,
            pos,
            width: 320.0,
            modal: true,
            reset: None,
            preview: None,
            ok: t!("dialog-ok"),
            ok_enabled: true,
            other: None,
            cancel: true,
            note: None,
            footer: true,
            resize: None,
            first_field: !NO_FIRST_FIELD.contains(&id),
        }
    }
}

/// What the dialog's buttons asked for this frame.
#[derive(Default)]
pub(super) struct DialogOutcome {
    pub ok: bool,
    pub cancel: bool,
    pub reset: bool,
    pub other: bool,
}

/// Floating dialog: header (title, reset, close; drag it to move the
/// window), body, footer (preview, Cancel, OK).
pub(super) fn floating_dialog(
    ctx: &egui::Context,
    spec: DialogSpec<'_>,
    body: impl FnOnce(&mut egui::Ui),
) -> DialogOutcome {
    let DialogSpec {
        id,
        title,
        pivot,
        pos,
        width,
        modal,
        reset,
        preview,
        ok,
        ok_enabled,
        other,
        cancel,
        note,
        footer,
        resize,
        first_field,
    } = spec;
    if !modal
        && HIDDEN_WITH_PANELS.contains(&id)
        && ctx.data(|d| d.get_temp::<bool>(panels_hidden_id()).unwrap_or(false))
    {
        return DialogOutcome::default();
    }
    let id = egui::Id::new(id);
    let offset_id = id.with("offset");
    let offset = ctx.data(|d| d.get_temp::<Vec2>(offset_id).unwrap_or_default());
    let size_id = id.with("size");
    let resized_to = resize.and_then(|_| ctx.data(|d| d.get_temp::<Vec2>(size_id)));
    let width = resized_to.map_or(width, |s| s.x);
    // Always within the window, below the menus and options bar, header
    // and footer included: a dialog taller than that scrolls its body.
    let screen = dialog_bounds(ctx);
    // Between header, body and footer: two gaps of 8 px; the border: 2 px.
    // Without a footer, its room goes to the body.
    let footer_room = if footer { DIALOG_FOOTER + 8.0 } else { 0.0 };
    let body_max = (screen.height() - DIALOG_HEADER - footer_room - 10.0).max(0.0);
    // Its margins aside.
    let body_height = resize.map(|h| {
        resized_to
            .map_or(h, |s| s.y)
            .clamp(MIN_PANEL_BODY, (body_max - 34.0).max(MIN_PANEL_BODY))
    });
    let mut grip_drag = None;
    let area = egui::Area::new(id)
        .kind(if modal {
            egui::UiKind::Modal
        } else {
            egui::UiKind::Window
        })
        .order(egui::Order::Foreground)
        .interactable(true)
        .pivot(pivot)
        .current_pos(pos + offset)
        .constrain_to(screen)
        // Sized at once, rather than growing frame after frame, however tall.
        .default_size(screen.size());
    let frame = egui::Frame::new()
        .fill(color::BG2)
        .stroke(UiStroke::new(1.0, color::WINDOW_LINE))
        .corner_radius(12)
        .shadow(egui::Shadow {
            offset: [0, 24],
            blur: 60,
            spread: 0,
            color: Color32::from_black_alpha(140),
        });

    let mut out = DialogOutcome::default();
    let alt = ctx.input(|i| i.modifiers.alt);
    let mut header = Rect::NOTHING;
    let mut header_drag = false;
    let mut background = Vec::new();
    let content = |ui: &mut egui::Ui| {
        background = ui.stack().iter().map(|s| s.id).collect();
        ui.set_width(width);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

        // Header.
        let (rect, response) = ui.allocate_exact_size(vec2(width, DIALOG_HEADER), Sense::drag());
        header = rect;
        header_drag = response.dragged();
        let painter = ui.painter();
        painter.text(
            pos2(rect.left() + 16.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &title,
            ui_kit::bold(ui_kit::TEXT_SIZE),
            color::TEXT,
        );
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            UiStroke::new(1.0, color::LINE),
        );
        let mut buttons = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(rect.min, rect.max - vec2(8.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        buttons.spacing_mut().item_spacing.x = 2.0;
        if ui_kit::icon_button(&mut buttons, icon::X, &t!("dialog-close-tip"), true).clicked() {
            out.cancel = true;
        }
        if let Some(enabled) = reset {
            let tip = t!("dialog-adjust-reset");
            let reset_icon = icon::ARROW_COUNTER_CLOCKWISE;
            if ui_kit::icon_button(&mut buttons, reset_icon, &tip, enabled).clicked() {
                out.reset = true;
            }
        }

        // Body.
        if let Some(height) = body_height {
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 16,
                    bottom: 18,
                })
                .show(ui, |ui| {
                    ui.set_width(width - 32.0);
                    ui.set_height(height);
                    body(ui);
                });
            // The resize grip, in the bottom right corner.
            let corner = ui.min_rect().right_bottom();
            let grip = Rect::from_min_max(corner - vec2(16.0, 16.0), corner);
            let response = ui.interact(grip, id.with("grip"), Sense::drag());
            let stroke = UiStroke::new(
                1.0,
                if response.hovered() || response.dragged() {
                    color::TEXT
                } else {
                    color::MUTED
                },
            );
            for k in [4.0, 8.0] {
                let end = corner - vec2(5.0, 5.0);
                ui.painter()
                    .line_segment([end - vec2(k, 0.0), end - vec2(0.0, k)], stroke);
            }
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
            }
            // From the size and the pointer at the press: past the minimum
            // size, the corner comes back with the pointer.
            let start_id = id.with("grip-start");
            if response.drag_started()
                && let Some(press) = ui.input(|i| i.pointer.press_origin())
            {
                ui.data_mut(|d| d.insert_temp(start_id, (vec2(width, height), press)));
            }
            if response.dragged()
                && let Some((size, press)) = ui.data(|d| d.get_temp::<(Vec2, Pos2)>(start_id))
                && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
            {
                grip_drag = Some((size, pointer - press));
            }
        } else {
            // A body that scrolls shows it (its last lines aren't just cut).
            ui.spacing_mut().scroll.dormant_handle_opacity = 0.6;
            egui::ScrollArea::vertical()
                .max_height(body_max)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin {
                            left: 16,
                            right: 16,
                            top: 16,
                            bottom: 18,
                        })
                        .show(ui, |ui| {
                            ui.set_width(width - 32.0);
                            body(ui);
                        });
                });
        }

        // Footer.
        if !footer {
            return;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(width, DIALOG_FOOTER), Sense::hover());
        let corners = egui::CornerRadius {
            sw: 12,
            se: 12,
            ..Default::default()
        };
        ui.painter().rect_filled(rect, corners, color::LAYERS);
        ui.painter().hline(
            rect.x_range(),
            rect.top() + 0.5,
            UiStroke::new(1.0, color::LINE),
        );
        let inner = Rect::from_min_max(rect.min + vec2(16.0, 0.0), rect.max - vec2(12.0, 0.0));
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if let Some(preview) = preview {
            ui_kit::checkbox(&mut row, preview, &t!("dialog-preview"));
        }
        if let Some(note) = &note {
            ui_kit::hint(&mut row, note);
        }
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if footer_button(ui, &ok, true, ok_enabled).clicked() {
                out.ok = true;
            }
            if let Some(other) = &other
                && footer_button(ui, other, false, true).clicked()
            {
                out.other = true;
            }
            if !cancel {
                return;
            }
            let reset_instead = alt && reset.is_some();
            let label = if reset_instead {
                t!("dialog-adjust-reset")
            } else {
                t!("dialog-cancel")
            };
            if footer_button(ui, &label, false, true).clicked() {
                if reset_instead {
                    out.reset = true;
                } else {
                    out.cancel = true;
                }
            }
        });
    };
    let opening = !ctx.memory(|m| {
        m.areas()
            .visible_last_frame(&egui::LayerId::new(egui::Order::Foreground, id))
    });
    let rect = if modal {
        let shown = egui::Modal::new(id)
            .area(area)
            .backdrop_color(Color32::TRANSPARENT)
            .frame(frame.inner_margin(0))
            .show(ctx, content);
        // Not while a drop-down is open (Esc only closes it), nor under
        // another modal (a Color Picker opened from this one).
        // Its first field takes the keyboard, so that "1920 Tab 1080
        // Enter" needs no mouse.
        if opening && first_field {
            super::focus_first_field(ctx);
        }
        if !shown.any_popup_open && !popup_open_at_frame_start(ctx) && shown.is_top_modal {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                out.cancel = true;
            }
            if ok_enabled && ctx.input(|i| i.key_pressed(Key::Enter)) {
                out.ok = true;
            }
        }
        shown.response.rect
    } else {
        area.show(ctx, |ui| frame.show(ui, content)).response.rect
    };
    // The offset keeps only what was shown: a dialog pushed against an
    // edge comes back as soon as it is dragged back.
    let mut offset = offset + (rect.min - pivot.anchor_size(pos + offset, rect.size()).min);

    // Resizing: the corner follows the pointer, whatever the pivot.
    if let Some((start, delta)) = grip_drag {
        let size = vec2(
            (start.x + delta.x).max(MIN_PANEL_WIDTH),
            (start.y + delta.y).clamp(MIN_PANEL_BODY, body_max.max(MIN_PANEL_BODY)),
        );
        let height = body_height.unwrap_or(start.y);
        let grown = size - vec2(width, height);
        offset += vec2(
            grown.x * pivot.x().to_factor(),
            grown.y * pivot.y().to_factor(),
        );
        ctx.data_mut(|d| {
            d.insert_temp(size_id, size);
            d.insert_temp(offset_id, offset);
        });
    }

    // Moving: the header is the handle (in a modal, its container takes the
    // drag, so the press position decides).
    let from_header = ctx
        .input(|i| i.pointer.press_origin())
        .is_some_and(|p| header.contains(p));
    let dragged =
        header_drag || (from_header && ctx.dragged_id().is_some_and(|d| background.contains(&d)));
    if dragged {
        let delta = ctx.input(|i| i.pointer.delta());
        ctx.data_mut(|d| d.insert_temp(offset_id, offset + delta));
        ctx.set_cursor_icon(CursorIcon::Grabbing);
    } else if ctx.pointer_hover_pos().is_some_and(|p| header.contains(p))
        && ctx.dragged_id().is_none()
    {
        ctx.set_cursor_icon(CursorIcon::Grab);
    }
    out
}

/// Height of a dialog's body when it takes all the window's (a workspace),
/// its margins aside.
pub(super) fn dialog_body_height(ctx: &egui::Context) -> f32 {
    let screen = ctx.content_rect().shrink(DIALOG_MARGIN);
    (screen.height() - DIALOG_HEADER - DIALOG_FOOTER - 18.0 - 34.0).max(0.0)
}

/// Room kept around dialogs, within the window.
const DIALOG_MARGIN: f32 = 8.0;
const DIALOG_HEADER: f32 = 44.0;
const DIALOG_FOOTER: f32 = 56.0;

/// Footer button: 30 px high, at least 76 px wide.
fn footer_button(ui: &mut egui::Ui, label: &str, primary: bool, enabled: bool) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        ui_kit::medium(ui_kit::TEXT_SIZE),
        Color32::PLACEHOLDER,
    );
    let w = (galley.size().x + 28.0).max(76.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(w, 30.0), sense);
    let kind = if primary { Kind::Primary } else { Kind::Normal };
    let (fill, border, fg) = ui_kit::button_colors(kind, enabled && response.hovered(), enabled);
    ui.painter().rect(
        rect,
        7.0,
        fill,
        UiStroke::new(1.0, border),
        StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    response
}

/// A group of rows kept close together (8 px), e.g. a label and its control.
pub(super) fn block(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        add_contents(ui);
    });
}

/// 18 px between blocks (with the 8 px item spacing).
pub(super) fn gap_between_blocks(ui: &mut egui::Ui) {
    ui.add_space(10.0);
}

/// Help text under the controls, with an info icon.
pub(super) fn help(ui: &mut egui::Ui, text: &str) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(
            egui::RichText::new(icon::INFO)
                .size(14.0)
                .color(color::LABEL),
        );
        ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(color::LABEL)).wrap());
    });
}

/// Result line on an accent background (e.g. "5 elements found.").
fn result_banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(color::ACCENT_SOFT)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.label(
                    egui::RichText::new(icon::SQUARES_FOUR)
                        .size(16.0)
                        .color(color::ACCENT_TEXT),
                );
                ui.label(egui::RichText::new(text).color(color::TEXT));
            });
        });
}

/// Color to make transparent: swatch, hex code, eyedropper toggle (a click
/// on the image picks the color while it is on).
fn color_to_remove(ui: &mut egui::Ui, color: &mut Color32, picking: &mut bool) {
    egui::Frame::new()
        .fill(color::BG0)
        .stroke(UiStroke::new(1.0, color::LINE))
        .corner_radius(8)
        .inner_margin(6)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui_kit::swatch(ui, color, 30.0, &t!("dialog-transparency-color"));
                let [r, g, b, _] = color.to_srgba_unmultiplied();
                ui.label(egui::RichText::new(hex_color([r, g, b])).font(ui_kit::mono(12.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let tip = t!("dialog-transparency-color-tip");
                    ui_kit::icon_toggle(ui, icon::EYEDROPPER, &tip, picking);
                });
            });
        });
}

/// Segments sharing the full width (tone balance, channels). Each gets its
/// label's width plus an equal share of the rest; labels that still don't
/// fit are cut with "…" and shown whole in a tooltip.
pub(super) fn full_segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, &str)],
) {
    let width = ui.available_width();
    let (rect, group) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    ui.painter().rect(
        rect,
        8.0,
        color::BG0,
        UiStroke::new(1.0, color::LINE),
        StrokeKind::Inside,
    );
    let font = ui_kit::medium(ui_kit::TEXT_SIZE);
    let labels: Vec<f32> = options
        .iter()
        .map(|(_, label)| {
            let galley = ui
                .painter()
                .layout_no_wrap(label.to_string(), font.clone(), color::TEXT);
            galley.size().x + 16.0
        })
        .collect();
    let cells = segment_widths(&labels, rect.width() - 4.0, 2.0);
    let mut left = rect.left() + 2.0;
    for (i, (option, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(left, rect.top() + 2.0), vec2(cells[i], 26.0));
        left += cells[i] + 2.0;
        let mut response = ui.interact(r, group.id.with(i), Sense::click());
        let selected = *value == *option;
        if selected {
            ui.painter().rect(
                r,
                6.0,
                color::BG3,
                UiStroke::new(1.0, color::LINE_HOVER),
                StrokeKind::Inside,
            );
        }
        let fg = if selected || response.hovered() {
            color::TEXT
        } else {
            color::MUTED
        };
        let mut job = egui::text::LayoutJob::single_section(
            label.to_string(),
            egui::TextFormat::simple(font.clone(), fg),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((r.width() - 12.0).max(0.0));
        let galley = ui.painter().layout_job(job);
        let elided = galley.elided;
        ui.painter()
            .galley(r.center() - galley.size() / 2.0, galley, fg);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, *label)
        });
        if elided {
            response = response.on_hover_text(*label);
        }
        if response.clicked() {
            *value = *option;
        }
    }
}

/// Widths of segments sharing `total` (with `gap` between them): each gets
/// its wanted width plus an equal share of what is left, or, when they
/// don't all fit, a share of the total in proportion to what it wants.
pub(super) fn segment_widths(wanted: &[f32], total: f32, gap: f32) -> Vec<f32> {
    let n = wanted.len().max(1) as f32;
    let room = (total - gap * (n - 1.0)).max(0.0);
    let sum: f32 = wanted.iter().sum();
    if sum <= room {
        let extra = (room - sum) / n;
        wanted.iter().map(|w| w + extra).collect()
    } else {
        // Not every label fits: shares in proportion, but never below an
        // equal share's half, so a short label stays clickable.
        let floor = room / n / 2.0;
        let above: f32 = wanted.iter().map(|w| (w - floor).max(0.0)).sum();
        let left = room - floor * n;
        wanted
            .iter()
            .map(|w| floor + (w - floor).max(0.0) / above.max(1e-3) * left)
            .collect()
    }
}

/// Anchor of Canvas Size: 3 × 3 cells, the chosen one filled; the others
/// point away from it, like Photoshop's.
fn anchor_grid(ui: &mut egui::Ui, anchor: &mut (u8, u8)) {
    const CELL: f32 = 26.0;
    let (rect, group) = ui.allocate_exact_size(Vec2::splat(CELL * 3.0 + 4.0), Sense::hover());
    for row in 0..3u8 {
        for col in 0..3u8 {
            let r = Rect::from_min_size(
                rect.min + vec2(col as f32 * (CELL + 2.0), row as f32 * (CELL + 2.0)),
                Vec2::splat(CELL),
            );
            let response = ui.interact(r, group.id.with((col, row)), Sense::click());
            let chosen = (col, row) == *anchor;
            let fill = if chosen {
                color::ACCENT
            } else if response.hovered() {
                color::BG3
            } else {
                color::BG0
            };
            ui.painter().rect(
                r,
                4.0,
                fill,
                UiStroke::new(1.0, color::LINE),
                StrokeKind::Inside,
            );
            let (dx, dy) = (col as i32 - anchor.0 as i32, row as i32 - anchor.1 as i32);
            if !chosen && dx.abs() <= 1 && dy.abs() <= 1 {
                // Arrow pointing away from the anchor.
                let dir = vec2(dx as f32, dy as f32).normalized();
                let tip = r.center() + dir * 5.0;
                let back = r.center() - dir * 4.0;
                let side = vec2(-dir.y, dir.x) * 3.5;
                let stroke = UiStroke::new(1.3, color::MUTED);
                ui.painter().line_segment([back, tip], stroke);
                ui.painter()
                    .line_segment([tip, tip - dir * 3.5 + side], stroke);
                ui.painter()
                    .line_segment([tip, tip - dir * 3.5 - side], stroke);
            }
            if response.clicked() {
                *anchor = (col, row);
            }
        }
    }
}

/// Channel selector of Levels and Curves: RGB, red, green, blue.
pub(super) fn channel_selector(ui: &mut egui::Ui, channel: &mut usize) {
    let (rgb, red, green, blue) = (
        t!("dialog-channel-rgb"),
        t!("dialog-color-red"),
        t!("dialog-color-green"),
        t!("dialog-color-blue"),
    );
    full_segmented(
        ui,
        channel,
        &[(0, &rgb), (1, &red), (2, &green), (3, &blue)],
    );
}

/// Look of a slider's track.
#[derive(Clone, Copy)]
pub(super) enum Track<'a> {
    /// Grey, filled with the accent from the left (or from the middle when
    /// the range goes both sides of zero).
    Accent,
    /// Evenly spaced colors, without fill.
    Gradient(&'a [Color32]),
}

/// Slider from −100 to +100 for `value` from −1 to 1.
fn percent_row(ui: &mut egui::Ui, left: &str, right: Option<&str>, value: &mut f32, track: Track) {
    let mut shown = *value * 100.0;
    slider_row(ui, left, right, &mut shown, -100.0..=100.0, "", track, 0.0);
    *value = shown / 100.0;
}

/// Slider showing `value × scale`, from `range`; reset to 0.
fn scaled_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    scale: f32,
    range: RangeInclusive<f32>,
    unit: &str,
    track: Track,
) {
    let mut shown = *value * scale;
    let default = 0.0f32.clamp(*range.start(), *range.end());
    slider_row(ui, label, None, &mut shown, range, unit, track, default);
    *value = shown / scale;
}

/// Slider row: label (and the opposite label at the right end, for color
/// balance), compact value field, then the track. Whole values; a
/// double-click on the label or the track puts back `default`, like
/// Photoshop.
#[allow(clippy::too_many_arguments)]
pub(super) fn slider_row(
    ui: &mut egui::Ui,
    label: &str,
    right: Option<&str>,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    track: Track,
    default: f32,
) -> bool {
    let steps = Steps::WHOLE;
    slider_row_steps(ui, label, right, value, range, unit, track, default, steps)
}

/// Precision and response of a slider.
#[derive(Clone, Copy)]
pub(super) struct Steps {
    /// Decimals shown and kept.
    pub decimals: usize,
    /// Above 1, the track gives more room to the small values: its
    /// position is the fraction of the range to the power 1 / `exponent`
    /// (radii from 0.1 to 1000 px).
    pub exponent: f32,
}

impl Steps {
    pub const WHOLE: Steps = Steps {
        decimals: 0,
        exponent: 1.0,
    };
}

/// [`slider_row`] with decimals, or a track favoring the small values.
#[allow(clippy::too_many_arguments)]
pub(super) fn slider_row_steps(
    ui: &mut egui::Ui,
    label: &str,
    right: Option<&str>,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    track: Track,
    default: f32,
    steps: Steps,
) -> bool {
    let (min, max) = (*range.start(), *range.end());
    let bipolar = min < 0.0;
    let before = *value;
    let unit_w = ui
        .painter()
        .layout_no_wrap(unit.to_owned(), ui_kit::mono(12.0), color::LABEL)
        .size()
        .x;
    // Wider for a long unit ("levels").
    let field_w = (unit_w + 44.0).max(64.0);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let width = ui.available_width();
        let (head, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
        let font = egui::FontId::proportional(ui_kit::TEXT_SIZE);
        let label_rect = ui.painter().text(
            head.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            font.clone(),
            color::MUTED,
        );
        let label_hit = ui.interact(label_rect, ui.id().with(("label", label)), Sense::click());
        if label_hit.double_clicked() {
            *value = default;
        }
        let field = if let Some(right) = right {
            ui.painter().text(
                head.right_center(),
                egui::Align2::RIGHT_CENTER,
                right,
                font,
                color::MUTED,
            );
            Rect::from_center_size(head.center(), vec2(field_w, 26.0))
        } else {
            Rect::from_min_size(head.right_top() - vec2(field_w, 0.0), vec2(field_w, 26.0))
        };
        value_field(
            ui,
            field,
            value,
            range.clone(),
            unit,
            (bipolar, right.is_some()),
            steps.decimals,
        );

        // Track and handle.
        let (rect, response) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
        let line = Rect::from_min_max(
            pos2(rect.left(), rect.center().y - 2.5),
            pos2(rect.right(), rect.center().y + 2.5),
        );
        let inner = rect.shrink2(vec2(7.0, 0.0));
        let to_x = |v: f32| {
            let t = ((v - min) / (max - min)).clamp(0.0, 1.0);
            inner.left() + t.powf(1.0 / steps.exponent) * inner.width()
        };
        if let Some(p) = response.interact_pointer_pos() {
            let t = ((p.x - inner.left()) / inner.width()).clamp(0.0, 1.0);
            *value = min + t.powf(steps.exponent) * (max - min);
        }
        // A double-click on the track resets too, like Camera Raw.
        if response.double_clicked() {
            *value = default;
        }
        let painter = ui.painter();
        match track {
            Track::Accent => {
                painter.rect_filled(line, 3.0, color::KBD_LINE);
                let from = if bipolar { to_x(0.0) } else { line.left() };
                let x = to_x(*value);
                painter.rect_filled(
                    Rect::from_min_max(
                        pos2(from.min(x), line.top()),
                        pos2(from.max(x), line.bottom()),
                    ),
                    0.0,
                    color::ACCENT,
                );
            }
            Track::Gradient(colors) => gradient(painter, line, colors),
        }
        if bipolar {
            let x = to_x(0.0);
            painter.vline(
                x,
                rect.center().y - 7.0..=rect.center().y + 7.0,
                UiStroke::new(1.0, color::SWATCH_LINE),
            );
        }
        let center = pos2(to_x(*value), rect.center().y);
        let radius = if response.hovered() || response.dragged() {
            8.0
        } else {
            7.0
        };
        painter.circle_filled(
            center + vec2(0.0, 1.0),
            radius + 1.0,
            Color32::from_black_alpha(110),
        );
        painter.circle_filled(center, radius, Color32::WHITE);
    });
    let scale = 10f32.powi(steps.decimals as i32);
    *value = ((*value * scale).round() / scale).clamp(min, max);
    *value != before
}

/// Compact value field of a slider (64 × 26): Geist Mono, unit in grey
/// right after the value (`unit` carries its own space: " px", "°"), "+"
/// before positive values of a two-sided range.
fn value_field(
    ui: &mut egui::Ui,
    rect: Rect,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    (signed, centered): (bool, bool),
    decimals: usize,
) {
    let painter = ui.painter().clone();
    let frame = painter.add(egui::Shape::Noop);
    let unit_w = if unit.is_empty() {
        0.0
    } else {
        painter
            .text(
                pos2(rect.right() - 8.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                unit,
                ui_kit::mono(12.0),
                color::LABEL,
            )
            .width()
    };
    let value_rect = Rect::from_min_max(
        rect.min + vec2(6.0, 1.0),
        rect.max - vec2(8.0 + unit_w, 1.0),
    );
    let layout = if centered {
        egui::Layout::centered_and_justified(egui::Direction::LeftToRight)
    } else {
        egui::Layout::right_to_left(egui::Align::Center)
    };
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(value_rect).layout(layout));
    // As wide as its text: the value ends against the unit.
    child.spacing_mut().interact_size.x = 0.0;
    child.spacing_mut().button_padding.x = 0.0;
    let w = &mut child.visuals_mut().widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.bg_fill = Color32::TRANSPARENT;
        state.weak_bg_fill = Color32::TRANSPARENT;
        state.bg_stroke = UiStroke::NONE;
    }
    let response = child.add(
        egui::DragValue::new(value)
            .range(range)
            .speed(if decimals > 0 { 0.1 } else { 0.5 })
            .custom_formatter(move |v, _| {
                let text = i18n::decimal(v, decimals);
                if signed && v > 0.0 {
                    format!("+{text}")
                } else {
                    text
                }
            })
            .custom_parser(|s| {
                s.trim()
                    .trim_start_matches('+')
                    .replace(',', ".")
                    .parse()
                    .ok()
            }),
    );
    let border = if response.has_focus() {
        color::ACCENT
    } else if response.hovered() {
        color::LINE_HOVER
    } else {
        color::LINE
    };
    painter.set(
        frame,
        egui::epaint::RectShape::new(
            rect,
            6.0,
            color::BG0,
            UiStroke::new(1.0, border),
            StrokeKind::Inside,
        ),
    );
}

/// Horizontal gradient through evenly spaced `colors`, rounded ends.
fn gradient(painter: &egui::Painter, rect: Rect, colors: &[Color32]) {
    let mut mesh = egui::epaint::Mesh::default();
    let n = colors.len().max(2) - 1;
    for (i, c) in colors.iter().enumerate() {
        let x = rect.left() + i as f32 / n as f32 * rect.width();
        mesh.colored_vertex(pos2(x, rect.top()), *c);
        mesh.colored_vertex(pos2(x, rect.bottom()), *c);
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(mesh);
}

/// Levels editor for one channel: histogram, input sliders (black,
/// midtones, white) and output sliders (black, white), editable values.
fn levels_editor(ui: &mut egui::Ui, ch: &mut LevelsChannel, hist: &[u32; 256], tint: Color32) {
    use ui_kit::color;
    let width = ui.available_width();
    ui_kit::field_label(ui, &t!("dialog-levels-input"));

    // Histogram (square root: peaks don't flatten the rest).
    let (rect, _) = ui.allocate_exact_size(vec2(width, 90.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 1.0, color::BG0);
    let max = hist.iter().copied().max().unwrap_or(0).max(1) as f32;
    let bar = rect.width() / 256.0;
    let fill = tint.gamma_multiply(0.55);
    for (i, &n) in hist.iter().enumerate() {
        let h = (n as f32 / max).sqrt() * (rect.height() - 4.0);
        if h > 0.0 {
            let x = rect.left() + i as f32 * bar;
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(x, rect.bottom() - h),
                    pos2(x + bar.max(1.0), rect.bottom()),
                ),
                0.0,
                fill,
            );
        }
    }

    // Input levels.
    let (lo, hi) = (ch.in_black as f32, ch.in_white as f32);
    let mid = lo + (hi - lo) * ch.midpoint();
    let mut handles = [lo, mid, hi];
    let moved = level_track(ui, "input", tint, &mut handles);
    if let Some(i) = moved {
        let v = handles[i];
        match i {
            0 => ch.in_black = (v.round() as u8).min(ch.in_white.saturating_sub(1)),
            2 => ch.in_white = (v.round() as u8).max(ch.in_black.saturating_add(1)),
            _ => ch.set_midpoint((v - lo) / (hi - lo).max(1.0)),
        }
    }
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 2.0 * ui.spacing().item_spacing.x) / 3.0;
        let mut black = ch.in_black;
        ui.add_sized([w, 26.0], egui::DragValue::new(&mut black).range(0..=254));
        ui.add_sized(
            [w, 26.0],
            egui::DragValue::new(&mut ch.gamma)
                .range(crate::pixel_ops::MIN_GAMMA..=crate::pixel_ops::MAX_GAMMA)
                .speed(0.01)
                .custom_formatter(|v, _| i18n::decimal(v, 2))
                .custom_parser(i18n::parse_decimal),
        );
        let mut white = ch.in_white;
        ui.add_sized([w, 26.0], egui::DragValue::new(&mut white).range(1..=255));
        ch.in_black = black.min(white.saturating_sub(1));
        ch.in_white = white.max(ch.in_black.saturating_add(1));
    });

    // Output levels.
    ui.add_space(6.0);
    ui_kit::field_label(ui, &t!("dialog-levels-output"));
    let mut handles = [ch.out_black as f32, ch.out_white as f32];
    if let Some(i) = level_track(ui, "output", tint, &mut handles) {
        let v = handles[i].round() as u8;
        if i == 0 {
            ch.out_black = v;
        } else {
            ch.out_white = v;
        }
    }
    ui.horizontal(|ui| {
        let w = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        ui.add_sized(
            [w, 26.0],
            egui::DragValue::new(&mut ch.out_black).range(0..=255),
        );
        ui.add_sized(
            [w, 26.0],
            egui::DragValue::new(&mut ch.out_white).range(0..=255),
        );
    });
}

/// Two hue spectra, as at the bottom of Photoshop's window: before
/// (top) and after the adjustment (bottom).
fn hue_bars(ui: &mut egui::Ui, hs: &HueSaturation) {
    const STEPS: usize = 36;
    let before: Vec<Color32> = (0..=STEPS)
        .map(|i| egui::ecolor::Hsva::new(i as f32 / STEPS as f32, 1.0, 1.0, 1.0).into())
        .collect();
    let mut after = image::RgbaImage::from_fn(STEPS as u32 + 1, 1, |x, _| {
        image::Rgba(before[x as usize].to_array())
    });
    crate::pixel_ops::hue_saturation(&mut after, hs);
    let after: Vec<Color32> = after
        .pixels()
        .map(|p| Color32::from_rgb(p.0[0], p.0[1], p.0[2]))
        .collect();
    for colors in [before, after] {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
        let mut mesh = egui::epaint::Mesh::default();
        for (i, c) in colors.iter().enumerate() {
            let x = rect.left() + i as f32 / STEPS as f32 * rect.width();
            mesh.colored_vertex(pos2(x, rect.top()), *c);
            mesh.colored_vertex(pos2(x, rect.bottom()), *c);
            if i > 0 {
                let k = 2 * i as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k + 1, k);
            }
        }
        ui.painter().add(mesh);
        ui.add_space(2.0);
    }
}

/// Gradient bar (black → `tint`) with triangular handles to drag;
/// `values` (0..255) are updated. Returns the moved handle.
pub(super) fn level_track(
    ui: &mut egui::Ui,
    id: &str,
    tint: Color32,
    values: &mut [f32],
) -> Option<usize> {
    use ui_kit::color;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let bar = Rect::from_min_size(rect.min + vec2(7.0, 0.0), vec2(width - 14.0, 10.0));
    let mut mesh = egui::epaint::Mesh::default();
    for (pos, c) in [
        (bar.left_top(), Color32::BLACK),
        (bar.right_top(), tint),
        (bar.right_bottom(), tint),
        (bar.left_bottom(), Color32::BLACK),
    ] {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);
    ui.painter().rect_stroke(
        bar,
        0.0,
        UiStroke::new(1.0, color::LINE),
        StrokeKind::Outside,
    );

    let to_x = |v: f32| bar.left() + v / 255.0 * bar.width();
    let n = values.len();
    let mut moved = None;
    for (i, value) in values.iter_mut().enumerate() {
        let x = to_x(*value);
        let hit = Rect::from_center_size(pos2(x, bar.bottom() + 8.0), vec2(14.0, 16.0));
        let response = ui.interact(hit, ui.id().with((id, i)), Sense::drag());
        if response.dragged()
            && let Some(p) = response.interact_pointer_pos()
        {
            *value = ((p.x - bar.left()) / bar.width() * 255.0).clamp(0.0, 255.0);
            moved = Some(i);
        }
        // Black, gray (midtones if there are three), white.
        let fill = match (i, n) {
            (0, _) => Color32::BLACK,
            (1, 3) => Color32::from_gray(128),
            _ => Color32::WHITE,
        };
        let x = to_x(*value);
        let top = bar.bottom() + 1.0;
        let border = if response.hovered() || response.dragged() {
            color::ACCENT
        } else {
            color::LINE_HOVER
        };
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                pos2(x, top),
                pos2(x + 6.0, top + 11.0),
                pos2(x - 6.0, top + 11.0),
            ],
            fill,
            UiStroke::new(1.0, border),
        ));
    }
    moved
}

/// Curve editor: histogram in the background, grid, draggable points.
/// A click adds a point, a right click or dragging off the graph removes
/// it; `selected` is the current point, editable from the keyboard below.
pub(super) fn curves_editor(
    ui: &mut egui::Ui,
    curve: &mut Curve,
    hist: &[u32; 256],
    tint: Color32,
    selected: &mut Option<usize>,
) {
    use ui_kit::color;
    const GRAB: f32 = 8.0;
    let size = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let to_screen = |[x, y]: [f32; 2]| {
        pos2(
            rect.left() + x / 255.0 * rect.width(),
            rect.bottom() - y / 255.0 * rect.height(),
        )
    };
    let to_curve = |p: Pos2| {
        [
            (p.x - rect.left()) / rect.width() * 255.0,
            (rect.bottom() - p.y) / rect.height() * 255.0,
        ]
    };
    *selected = selected.filter(|&i| i < curve.points().len());
    let point_at = |curve: &Curve, p: Pos2| {
        curve
            .points()
            .iter()
            .position(|q| to_screen(*q).distance(p) <= GRAB)
    };

    // Press: grab a point, or create one (on the curve if clicking very
    // close to it, otherwise where clicked).
    let origin = ui.input(|i| i.pointer.press_origin());
    let command = ui.input(|i| i.modifiers.command);
    if response.clicked()
        && command
        && let Some(p) = origin
        && let Some(i) = point_at(curve, p)
    {
        // Ctrl+click on a point: remove it (like Photoshop).
        if curve.remove(i) {
            *selected = None;
        }
    } else if (response.drag_started() || response.clicked())
        && let Some(p) = origin
    {
        *selected = point_at(curve, p).or_else(|| {
            let [x, y] = to_curve(p);
            let on_curve = curve.eval(x);
            let near = (to_screen([x, on_curve]).y - p.y).abs() <= GRAB * 1.5;
            curve.insert(x, if near { on_curve } else { y })
        });
    }
    if response.secondary_clicked()
        && let Some(p) = response.interact_pointer_pos()
        && let Some(i) = point_at(curve, p)
        && curve.remove(i)
    {
        *selected = None;
    }
    if response.dragged()
        && let (Some(i), Some(p)) = (*selected, response.interact_pointer_pos())
    {
        let last = curve.points().len() - 1;
        if i != 0 && i != last && !rect.expand(24.0).contains(p) && curve.remove(i) {
            *selected = None;
        } else {
            let [x, y] = to_curve(p);
            curve.set(i, x, y);
        }
    }
    // Keyboard (outside value editing): arrows to move the point
    // (10 with Shift), Delete to remove it, Ctrl+Tab / Ctrl+Shift+Tab to
    // go to the next / previous point.
    if !ui.ctx().egui_wants_keyboard_input() {
        let (step, next, nudge, delete) = ui.input(|i| {
            let step = if i.modifiers.shift { 10.0 } else { 1.0 };
            let next = (i.modifiers.command && i.key_pressed(Key::Tab))
                .then_some(if i.modifiers.shift { -1 } else { 1 });
            let mut d = [0.0f32; 2];
            if !i.modifiers.command && !i.modifiers.alt {
                for (key, dx, dy) in [
                    (Key::ArrowLeft, -1.0, 0.0),
                    (Key::ArrowRight, 1.0, 0.0),
                    (Key::ArrowUp, 0.0, 1.0),
                    (Key::ArrowDown, 0.0, -1.0),
                ] {
                    if i.key_pressed(key) {
                        d[0] += dx;
                        d[1] += dy;
                    }
                }
            }
            let delete = i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace);
            (step, next, d, delete)
        });
        let n = curve.points().len();
        if let Some(dir) = next {
            *selected = Some(match *selected {
                Some(i) => (i as i32 + dir).rem_euclid(n as i32) as usize,
                None if dir > 0 => 0,
                None => n - 1,
            });
        }
        if let Some(i) = *selected {
            if delete {
                if curve.remove(i) {
                    *selected = None;
                }
            } else if nudge != [0.0; 2] {
                let [x, y] = curve.points()[i];
                curve.set(i, x + nudge[0] * step, y + nudge[1] * step);
            }
        }
    }
    if response.hovered()
        && let Some(p) = response.hover_pos()
    {
        ui.ctx().set_cursor_icon(if point_at(curve, p).is_some() {
            egui::CursorIcon::Grab
        } else {
            egui::CursorIcon::Crosshair
        });
    }

    // Background, histogram (square root, like levels), grid.
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 1.0, color::BG0);
    let max = hist.iter().copied().max().unwrap_or(0).max(1) as f32;
    let bar = rect.width() / 256.0;
    let fill = tint.gamma_multiply(0.18);
    for (i, &n) in hist.iter().enumerate() {
        let h = (n as f32 / max).sqrt() * rect.height();
        if h > 0.0 {
            let x = rect.left() + i as f32 * bar;
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(x, rect.bottom() - h),
                    pos2(x + bar.max(1.0), rect.bottom()),
                ),
                0.0,
                fill,
            );
        }
    }
    let grid = UiStroke::new(1.0, color::LINE);
    for k in 1..4 {
        let t = k as f32 / 4.0;
        let x = rect.left() + t * rect.width();
        let y = rect.top() + t * rect.height();
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], grid);
        painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], grid);
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        UiStroke::new(1.0, color::LINE_HOVER),
    );

    // Curve and points.
    let line: Vec<Pos2> = (curve.samples().into_iter().enumerate())
        .map(|(x, y)| to_screen([x as f32, y.clamp(0.0, 255.0)]))
        .collect();
    painter.add(egui::Shape::line(line, UiStroke::new(1.5, tint)));
    for (i, p) in curve.points().iter().enumerate() {
        let r = Rect::from_center_size(to_screen(*p), vec2(7.0, 7.0));
        if Some(i) == *selected {
            painter.rect_filled(r, 0.0, color::ACCENT);
        } else {
            painter.rect_filled(r, 0.0, color::BG0);
            painter.rect_stroke(r, 0.0, UiStroke::new(1.0, tint), StrokeKind::Inside);
        }
    }
    ui.painter().rect_stroke(
        rect,
        1.0,
        UiStroke::new(1.0, color::LINE),
        StrokeKind::Outside,
    );

    // Gradient of the input tones below the graph.
    let (strip, _) = ui.allocate_exact_size(vec2(size, 8.0), Sense::hover());
    let mut mesh = egui::epaint::Mesh::default();
    for (pos, c) in [
        (strip.left_top(), Color32::BLACK),
        (strip.right_top(), tint),
        (strip.right_bottom(), tint),
        (strip.left_bottom(), Color32::BLACK),
    ] {
        mesh.colored_vertex(pos, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);

    // Values of the selected point.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let Some(i) = *selected else {
            ui_kit::field_label(ui, &t!("dialog-curves-no-point"));
            return;
        };
        let [mut x, mut y] = curve.points()[i];
        ui_kit::field_label(ui, &t!("dialog-curves-input"));
        ui.add(
            egui::DragValue::new(&mut x)
                .range(0.0..=255.0)
                .fixed_decimals(0),
        );
        ui.add_space(8.0);
        ui_kit::field_label(ui, &t!("dialog-curves-output"));
        ui.add(
            egui::DragValue::new(&mut y)
                .range(0.0..=255.0)
                .fixed_decimals(0),
        );
        curve.set(i, x, y);
    });
}

/// Settings of an adjustment, in its window or for an adjustment layer in
/// the Properties panel. `range`: the tonal range or channel shown, `point`:
/// the selected curve point; `histogram`: of what it adjusts (Levels,
/// Curves); `primary`: the foreground color (Colorize starts from it).
pub(super) fn adjustment_editor(
    ui: &mut egui::Ui,
    adjustment: &mut Adjustment,
    range: &mut usize,
    point: &mut Option<usize>,
    histogram: Option<&Histograms>,
    primary: [u8; 4],
) {
    match adjustment {
        Adjustment::ColorBalance(cb) => {
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("dialog-adjust-tone-balance"));
                let (shadows, midtones, highlights) = (
                    t!("dialog-adjust-shadows"),
                    t!("dialog-adjust-midtones"),
                    t!("dialog-adjust-highlights"),
                );
                full_segmented(
                    ui,
                    range,
                    &[(0, &shadows), (1, &midtones), (2, &highlights)],
                );
            });
            let values = match *range {
                0 => &mut cb.shadows,
                1 => &mut cb.midtones,
                _ => &mut cb.highlights,
            };
            for (value, (left, right, track)) in values.iter_mut().zip([
                (
                    t!("dialog-color-cyan"),
                    t!("dialog-color-red"),
                    &[CYAN, NEUTRAL, RED][..],
                ),
                (
                    t!("dialog-color-magenta"),
                    t!("dialog-color-green"),
                    &[MAGENTA, NEUTRAL, GREEN][..],
                ),
                (
                    t!("dialog-color-yellow"),
                    t!("dialog-color-blue"),
                    &[YELLOW, NEUTRAL, BLUE][..],
                ),
            ]) {
                gap_between_blocks(ui);
                percent_row(ui, &left, Some(&right), value, Track::Gradient(track));
            }
            gap_between_blocks(ui);
            ui_kit::checkbox(
                ui,
                &mut cb.preserve_luminosity,
                &t!("dialog-adjust-preserve-luminosity"),
            );
        }
        Adjustment::BrightnessContrast(bc) => {
            let range = |r: RangeInclusive<f32>| *r.start() * 100.0..=*r.end() * 100.0;
            let brightness = t!("dialog-adjust-brightness");
            let contrast = t!("dialog-adjust-contrast");
            scaled_row(
                ui,
                &brightness,
                &mut bc.brightness,
                100.0,
                range(BrightnessContrast::BRIGHTNESS),
                "",
                Track::Accent,
            );
            gap_between_blocks(ui);
            scaled_row(
                ui,
                &contrast,
                &mut bc.contrast,
                100.0,
                range(BrightnessContrast::CONTRAST),
                "",
                Track::Accent,
            );
        }
        Adjustment::Levels(lv) => {
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("dialog-adjust-channel"));
                channel_selector(ui, range);
            });
            gap_between_blocks(ui);
            let c = (*range).min(3);
            let tint = CHANNEL_TINTS[c];
            let channel = if c == 0 {
                &mut lv.rgb
            } else {
                &mut lv.channels[c - 1]
            };
            let empty = [0u32; 256];
            let hist = histogram.map_or(&empty, |h| &h[c]);
            levels_editor(ui, channel, hist, tint);
            gap_between_blocks(ui);
            let auto = t!("dialog-adjust-auto");
            if ui_kit::button(ui, &auto, Kind::Normal, None, histogram.is_some())
                .on_hover_text(t!("dialog-adjust-auto-tip"))
                .clicked()
                && let Some(h) = histogram
            {
                *lv = Levels::auto(h);
            }
        }
        Adjustment::Curves(cv) => {
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("dialog-adjust-channel"));
                let before = *range;
                channel_selector(ui, range);
                if *range != before {
                    *point = None;
                }
            });
            gap_between_blocks(ui);
            let c = (*range).min(3);
            let curve = if c == 0 {
                &mut cv.rgb
            } else {
                &mut cv.channels[c - 1]
            };
            let empty = [0u32; 256];
            let hist = histogram.map_or(&empty, |h| &h[c]);
            curves_editor(ui, curve, hist, CHANNEL_TINTS[c], point);
            gap_between_blocks(ui);
            let ctx = ui.ctx().clone();
            let next = KeyboardShortcut::new(Modifiers::COMMAND, Key::Tab);
            let hint = t!(
                "input3-curves-hint",
                ctrl_click = keys::click(&ctx, Modifiers::COMMAND),
                delete = keys::key(Key::Delete),
                next = keys::shortcut(&ctx, &next)
            );
            help(ui, &hint);
        }
        Adjustment::Vibrance(v) => {
            let vibrance = t!("dialog-adjust-vibrance");
            percent_row(ui, &vibrance, None, &mut v.vibrance, Track::Accent);
            gap_between_blocks(ui);
            let saturation = t!("dialog-adjust-saturation");
            let track = Track::Gradient(SATURATION);
            percent_row(ui, &saturation, None, &mut v.saturation, track);
            gap_between_blocks(ui);
            help(ui, &t!("dialog-vibrance-hint"));
        }
        Adjustment::HueSaturation(hs) => {
            let (hue, saturation, lightness) = (
                t!("dialog-adjust-hue"),
                t!("dialog-adjust-saturation"),
                t!("dialog-adjust-lightness"),
            );
            if hs.colorize {
                let track = Track::Gradient(&HUES);
                scaled_row(ui, &hue, &mut hs.hue, 1.0, 0.0..=360.0, "°", track);
                gap_between_blocks(ui);
                let sat = &mut hs.saturation;
                let track = Track::Gradient(SATURATION);
                scaled_row(ui, &saturation, sat, 100.0, 0.0..=100.0, "", track);
            } else {
                let track = Track::Gradient(&HUES_CENTERED);
                scaled_row(ui, &hue, &mut hs.hue, 1.0, -180.0..=180.0, "°", track);
                gap_between_blocks(ui);
                let track = Track::Gradient(SATURATION);
                percent_row(ui, &saturation, None, &mut hs.saturation, track);
            }
            gap_between_blocks(ui);
            let track = Track::Gradient(LIGHTNESS);
            percent_row(ui, &lightness, None, &mut hs.lightness, track);
            gap_between_blocks(ui);
            hue_bars(ui, hs);
            gap_between_blocks(ui);
            let was = hs.colorize;
            ui_kit::checkbox(ui, &mut hs.colorize, &t!("dialog-adjust-colorize"))
                .on_hover_text(t!("dialog-adjust-colorize-tip"));
            if hs.colorize != was {
                // Like Photoshop: colorize starts from the hue of the
                // foreground color (red if it is a gray), saturation 25;
                // unchecking resets everything.
                *hs = if hs.colorize {
                    HueSaturation {
                        hue: crate::pixel_ops::hue_of([primary[0], primary[1], primary[2]])
                            .unwrap_or(0.0)
                            .round(),
                        saturation: crate::pixel_ops::COLORIZE_SATURATION,
                        lightness: 0.0,
                        colorize: true,
                    }
                } else {
                    HueSaturation::default()
                };
            }
        }
        Adjustment::BlackWhite(bw) => {
            let ranges = [
                (t!("core-bw-reds"), RED),
                (t!("core-bw-yellows"), YELLOW),
                (t!("core-bw-greens"), GREEN),
                (t!("core-bw-cyans"), CYAN),
                (t!("core-bw-blues"), BLUE),
                (t!("core-bw-magentas"), MAGENTA),
            ];
            let defaults = crate::pixel_ops::BlackWhite::default().weights;
            let unit = i18n::percent("");
            for (i, (name, tint)) in ranges.iter().enumerate() {
                if i > 0 {
                    gap_between_blocks(ui);
                }
                let track = Track::Gradient(&[Color32::BLACK, *tint]);
                let weight = &mut bw.weights[i];
                slider_row(
                    ui,
                    name,
                    None,
                    weight,
                    -200.0..=300.0,
                    &unit,
                    track,
                    defaults[i],
                );
            }
            gap_between_blocks(ui);
            ui_kit::checkbox(ui, &mut bw.tint, &t!("core-bw-tint"));
            if bw.tint {
                gap_between_blocks(ui);
                let hue = t!("dialog-adjust-hue");
                scaled_row(
                    ui,
                    &hue,
                    &mut bw.hue,
                    1.0,
                    0.0..=360.0,
                    "°",
                    Track::Gradient(&HUES),
                );
                gap_between_blocks(ui);
                let saturation = t!("dialog-adjust-saturation");
                let track = Track::Gradient(SATURATION);
                scaled_row(
                    ui,
                    &saturation,
                    &mut bw.saturation,
                    100.0,
                    0.0..=100.0,
                    &unit,
                    track,
                );
            }
        }
        Adjustment::PhotoFilter(f) => {
            ui.horizontal(|ui| {
                ui_kit::field_label(ui, &t!("core-photo-filter-color"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let [r, g, b] = f.color;
                    let mut c = Color32::from_rgb(r, g, b);
                    let tip = t!("core-photo-filter-color");
                    if ui_kit::swatch(ui, &mut c, 28.0, &tip).changed() {
                        f.color = [c.r(), c.g(), c.b()];
                    }
                });
            });
            gap_between_blocks(ui);
            let density = t!("core-photo-filter-density");
            let unit = i18n::percent("");
            scaled_row(
                ui,
                &density,
                &mut f.density,
                100.0,
                1.0..=100.0,
                &unit,
                Track::Accent,
            );
            gap_between_blocks(ui);
            let preserve = t!("dialog-adjust-preserve-luminosity");
            ui_kit::checkbox(ui, &mut f.preserve_luminosity, &preserve);
        }
        Adjustment::Invert => help(ui, &t!("core-adjust-no-settings")),
        Adjustment::Posterize(p) => {
            let mut levels = p.levels as f32;
            let label = t!("core-posterize-levels");
            slider_row(
                ui,
                &label,
                None,
                &mut levels,
                2.0..=255.0,
                "",
                Track::Accent,
                4.0,
            );
            p.levels = levels as u8;
        }
        Adjustment::Threshold(th) => {
            let mut level = th.level as f32;
            let label = t!("core-threshold-level");
            let track = Track::Gradient(LIGHTNESS);
            slider_row(ui, &label, None, &mut level, 1.0..=255.0, "", track, 128.0);
            th.level = level as u8;
        }
        Adjustment::GradientMap(g) => {
            super::gradient_editor::gradient_field(ui, &mut g.gradient);
            gap_between_blocks(ui);
            ui_kit::checkbox(ui, &mut g.reverse, &t!("core-gradient-reverse"));
        }
    }
}

/// Whether a menu or drop-down was open when this frame began: the Esc or
/// Enter that closed it is not also the dialog's.
fn popup_open_at_frame_start(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(popup_start_id()).unwrap_or(false))
}

/// Notes, at the start of each frame, whether a menu or drop-down is open.
pub(super) fn note_popups_at_frame_start(ctx: &egui::Context) {
    let open = egui::Popup::is_any_open(ctx);
    ctx.data_mut(|d| d.insert_temp(popup_start_id(), open));
}

fn popup_start_id() -> egui::Id {
    egui::Id::new("popup-open-at-frame-start")
}
