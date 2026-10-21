//! Options bar: the active tool's name and settings, in a row under the title
//! bar. When the window is too narrow, hints go first, then the row wraps.

use super::keys;
use super::select_tools::MarqueeStyle;
use super::*;
use crate::i18n::t;
use egui::FontId;
use egui::text::LayoutJob;
use ui_kit::color;

mod artboard_options;
mod paint_options;
mod retouch_options;
mod transform_options;
mod type_options;
pub(super) mod vector_options;

/// Element sizes offered by the Scale menu of the Move tool (%).
const SCALE_PRESETS: [f32; 6] = [25.0, 50.0, 75.0, 100.0, 150.0, 200.0];
/// Zoom levels of the Hand and Zoom tools (%).
const ZOOM_PRESETS: [u32; 3] = [50, 100, 200];
/// Bounds of the X and Y fields (px).
const POSITION_LIMIT: i64 = 1_000_000;

impl AtelierApp {
    /// Options of the active tool.
    pub(super) fn options_bar(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.label(
            egui::RichText::new(self.tool.name())
                .font(ui_kit::bold(ui_kit::TEXT_SIZE))
                .color(color::TEXT),
        );
        if let Some(key) = self.tool.shortcut() {
            ui_kit::kbd(ui, key.name());
        }
        sep(ui);
        if self.doc.is_none() {
            ui.label(egui::RichText::new(t!("optbar-no-document")).color(color::MUTED));
            return;
        }
        if self.sel.transform.is_some() {
            return self.selection_transform_options(ui);
        }
        match self.tool {
            Tool::Transform => self.transform_options(ui),
            Tool::Artboard => self.artboard_options(ui),
            Tool::Select
            | Tool::Ellipse
            | Tool::FreeLasso
            | Tool::PolyLasso
            | Tool::Lasso
            | Tool::ObjectSelect
            | Tool::MagicWand => self.select_options(ui),
            Tool::Brush
            | Tool::Eraser
            | Tool::Dodge
            | Tool::Burn
            | Tool::Sponge
            | Tool::Blur
            | Tool::Sharpen
            | Tool::Smudge => self.brush_options(ui),
            Tool::Gradient => self.gradient_options(ui),
            Tool::CloneStamp
            | Tool::SpotHealing
            | Tool::HealingBrush
            | Tool::Patch
            | Tool::RedEye => self.retouch_options(ui),
            Tool::Bucket => self.bucket_options(ui),
            Tool::Eyedropper => self.eyedropper_options(ui),
            Tool::Text => self.text_options(ui),
            Tool::Canvas => self.canvas_options(ui),
            Tool::Hand | Tool::Zoom => self.view_options(ui),
            Tool::Pen(_) | Tool::PathSelect | Tool::DirectSelect | Tool::Shape(_) => {
                self.vector_options(ui)
            }
        }
    }

    fn transform_options(&mut self, ui: &mut egui::Ui) {
        if self.active_locked() {
            locked_notice(
                ui,
                &t!("options-layer-locked"),
                &t!("options-layer-locked-hint"),
            );
            return;
        }
        if self.warp_options(ui) {
            return;
        }
        let (Some(full), Some(src)) = (self.full_rect(), self.transform_source()) else {
            return;
        };
        self.reference_locator(ui);
        sep(ui);
        // A distorted frame: its box, not editable as numbers (same bar,
        // so that the canvas doesn't move when it starts).
        let distorted = self.transform_distorted();
        let mut r = match self.transform_reach() {
            Some(reach) => reach,
            None => self.transform_rect.unwrap_or(src),
        };
        let ratio = src.width() / src.height();

        // As applied: the frame's edges rounded to pixels.
        let shown = round_rect(r);
        let mut x = shown.min.x as i64;
        let mut y = shown.min.y as i64;
        let mut w = shown.width().max(1.0) as u32;
        let mut h = shown.height().max(1.0) as u32;
        let limits = -POSITION_LIMIT..=POSITION_LIMIT;
        let editable = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui) -> bool| {
            ui.add_enabled_ui(!distorted, |ui| add(ui)).inner
        };
        let x_changed = editable(ui, &mut |ui| {
            ui_kit::number_field(ui, "X", &mut x, limits.clone(), "", 86.0).changed()
        });
        let y_changed = editable(ui, &mut |ui| {
            ui_kit::number_field(ui, "Y", &mut y, limits.clone(), "", 86.0).changed()
        });
        let w_changed = editable(ui, &mut |ui| {
            size_field(ui, &t!("optbar-width-short"), &t!("options-width"), &mut w)
        });
        ui_kit::icon_toggle(
            ui,
            icon::LINK_SIMPLE,
            &t!("options-constrain-proportions"),
            &mut self.keep_ratio,
        );
        let h_changed = editable(ui, &mut |ui| {
            size_field(
                ui,
                &t!("optbar-height-short"),
                &t!("options-height"),
                &mut h,
            )
        });
        if w_changed && self.keep_ratio {
            h = ((w as f32 / ratio).round() as u32).max(1);
        }
        if h_changed && self.keep_ratio {
            w = ((h as f32 * ratio).round() as u32).max(1);
        }
        if x_changed || y_changed {
            r = Rect::from_min_size(pos2(x as f32, y as f32), r.size());
            self.transform_rect = Some(r);
        }
        let mut new_size = (w_changed || h_changed).then_some((w as f32, h as f32));
        sep(ui);

        // The angle and its two quarter-turn buttons wrap together.
        if room(ui) < 78.0 + 2.0 * (8.0 + 28.0) {
            ui.end_row();
        }
        let mut angle = self.transform_angle;
        editable(ui, &mut |ui| {
            ui_kit::number_field(
                ui,
                icon::ARROW_CLOCKWISE,
                &mut angle,
                -180.0..=180.0,
                "°",
                78.0,
            )
            .on_hover_text(t!("options-rotation"))
            .changed()
        });
        let mut turn = None;
        if ui_kit::icon_button(
            ui,
            icon::ARROW_COUNTER_CLOCKWISE,
            &t!("optbar-rotate-ccw"),
            true,
        )
        .clicked()
        {
            turn = Some((-90.0, transform::Command::RotateCcw));
        }
        if ui_kit::icon_button(ui, icon::ARROW_CLOCKWISE, &t!("optbar-rotate-cw"), true).clicked() {
            turn = Some((90.0, transform::Command::RotateCw));
        }
        match turn {
            Some((_, cmd)) if distorted => self.transform_command(cmd),
            Some((degrees, _)) => angle += degrees,
            None => {}
        }
        sep(ui);

        let sx = (r.width() / src.width() * 100.0).round();
        let sy = (r.height() / src.height() * 100.0).round();
        let scale = if sx == sy {
            i18n::percent(sx)
        } else {
            i18n::percent(format!("{sx} × {sy}"))
        };
        ui.add_enabled_ui(!distorted, |ui| {
            dropdown(
                ui,
                "scale_presets",
                Some(&t!("optbar-scale")),
                &scale,
                true,
                0.0,
                |ui| {
                    for pct in SCALE_PRESETS {
                        if ui
                            .selectable_label(sx == pct && sy == pct, i18n::percent(pct))
                            .clicked()
                        {
                            let s = pct / 100.0;
                            new_size = Some((
                                (src.width() * s).round().max(1.0),
                                (src.height() * s).round().max(1.0),
                            ));
                            ui.close();
                        }
                    }
                },
            );
        });
        if let Some((w, h)) = new_size {
            r = Rect::from_center_size(r.center(), vec2(w, h));
            self.transform_rect = Some(r);
        }
        self.resample_dropdown(ui);
        ui_kit::checkbox(ui, &mut self.fit_canvas, &t!("options-fit-canvas"))
            .on_hover_text(t!("options-fit-canvas-tip"));
        if ui_kit::icon_button(
            ui,
            icon::CROSSHAIR_SIMPLE,
            &t!("options-center-on-canvas"),
            true,
        )
        .clicked()
        {
            if distorted {
                self.nudge_transform(full.center() - r.center());
            }
            r = Rect::from_center_size(full.center(), r.size());
            self.transform_rect = Some(r);
        }
        self.warp_switch(ui);
        if distorted {
            let (apply, cancel) = apply_cancel(ui, true);
            if apply {
                self.apply_transform();
            }
            if cancel {
                self.reset_pending();
            }
            return;
        }

        let angle = normalize_angle(angle);
        if angle != self.transform_angle {
            self.transform_angle = angle;
            self.transform_rect = Some(r);
        }
        let pending = r != src || self.transform_angle != 0.0 || (self.fit_canvas && r != full);
        let (apply, cancel) = apply_cancel(ui, pending);
        if apply {
            self.transform_rect = Some(r);
            self.apply_transform();
        }
        if cancel {
            self.reset_pending();
        }
    }

    /// Resampling of the transforms: the menu lists the full names, the
    /// button only the method.
    fn resample_dropdown(&mut self, ui: &mut egui::Ui) {
        let filter = self.resize_filter.label();
        let short = filter.split(" (").next().unwrap_or(&filter).to_owned();
        let label = t!("dialog-image-size-resample");
        dropdown(
            ui,
            "resize_filter",
            Some(&label),
            &short,
            false,
            0.0,
            |ui| {
                for f in ResizeFilter::ALL {
                    if ui
                        .selectable_label(self.resize_filter == f, f.label())
                        .clicked()
                    {
                        self.resize_filter = f;
                        ui.close();
                    }
                }
            },
        );
    }

    /// Selection tools: mode, the tool's settings, the selection's size, and
    /// the AI selections on the right.
    fn select_options(&mut self, ui: &mut egui::Ui) {
        ui_kit::segmented(
            ui,
            &mut self.select_mode,
            &[
                (
                    SelectMode::Replace,
                    icon::SELECTION,
                    &t!("optbar-select-new"),
                ),
                (
                    SelectMode::Add,
                    icon::UNITE_SQUARE,
                    &t!("options-select-add-hint"),
                ),
                (
                    SelectMode::Subtract,
                    icon::SUBTRACT_SQUARE,
                    &t!("options-select-subtract-hint"),
                ),
                (
                    SelectMode::Intersect,
                    icon::INTERSECT_SQUARE,
                    &t!("options-select-intersect-hint"),
                ),
            ],
        );
        if matches!(
            self.tool,
            Tool::Select | Tool::Ellipse | Tool::FreeLasso | Tool::PolyLasso | Tool::Lasso
        ) {
            self.shape_options(ui);
        }
        match self.tool {
            Tool::MagicWand => {
                tolerance_field(ui, &mut self.wand_tolerance);
                let edges = self.sel.shape.edges(Tool::MagicWand);
                ui_kit::checkbox(ui, &mut edges.antialias, &t!("sel-anti-alias"));
                ui_kit::checkbox(ui, &mut self.wand_contiguous, &t!("options-contiguous"));
                ui_kit::checkbox(
                    ui,
                    &mut self.wand_all_layers,
                    &t!("options-sample-all-layers"),
                );
            }
            Tool::Lasso => {
                let mut width = self.lasso_width.round() as u32;
                if ui_kit::number_field(
                    ui,
                    &t!("options-lasso-width"),
                    &mut width,
                    1..=40,
                    " px",
                    112.0,
                )
                .on_hover_text(t!("options-lasso-width-hint"))
                .changed()
                {
                    self.lasso_width = width as f32;
                }
                let mut frequency = self.lasso_frequency.round() as u32;
                if ui_kit::number_field(
                    ui,
                    &t!("options-lasso-frequency"),
                    &mut frequency,
                    0..=100,
                    "",
                    124.0,
                )
                .on_hover_text(t!("options-lasso-frequency-hint"))
                .changed()
                {
                    self.lasso_frequency = frequency as f32;
                }
            }
            _ => {}
        }

        let mask = t!("sel-menu-select-and-mask");
        if ui_kit::button(ui, &mask, Kind::Normal, None, self.selection.is_some())
            .on_hover_text(keys::shortcut(
                ui.ctx(),
                &select_menu::SHORTCUT_SELECT_AND_MASK,
            ))
            .clicked()
        {
            self.run(Action::Select(select_menu::Command::SelectAndMask));
        }

        let subject = t!("options-select-subject");
        let background = t!("options-remove-background");
        let right = button_width(ui, &subject, true)
            + button_width(ui, &background, true)
            + ui.spacing().item_spacing.x;
        // Size of the selection, if there is room for it.
        let (readout, tip) = match &self.selection {
            Some(s) => {
                let position = format!(
                    "{} {}, {}",
                    t!("options-position"),
                    signed(s.rect.x as f32),
                    signed(s.rect.y as f32)
                );
                let note = if s.inverted {
                    format!("\n{}", t!("options-selection-inverted-hint"))
                } else if s.mask.is_some() {
                    format!("\n{}", t!("options-selection-bounds-hint"))
                } else {
                    String::new()
                };
                let text = egui::RichText::new(format!("{} × {} px", s.rect.w, s.rect.h))
                    .font(ui_kit::mono(12.0));
                (text, Some(position + &note))
            }
            None => (egui::RichText::new(t!("options-no-selection")), None),
        };
        let readout = readout.color(color::LABEL);
        let readout_w = egui::WidgetText::from(readout.clone())
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
            .size()
            .x;
        // Object Selection: what to do (or the analysis going on), if it fits
        // with the size.
        if self.tool == Tool::ObjectSelect {
            let hint = self.object_select_hint();
            let width = ui
                .painter()
                .layout_no_wrap(hint.clone(), FontId::proportional(12.0), color::LABEL)
                .size()
                .x;
            if room(ui) >= width + readout_w + right + 3.0 * ui.spacing().item_spacing.x {
                ui_kit::hint(ui, hint);
            }
        }
        if room(ui) >= readout_w + right + 2.0 * ui.spacing().item_spacing.x {
            let response = ui.label(readout);
            if let Some(tip) = tip {
                response.on_hover_text(tip);
            }
        }
        let idle = self.subject_job.is_none();
        let mut action = None;
        right_aligned(ui, right, |ui| {
            let ai = |ui: &mut egui::Ui, label: &str| {
                ui_kit::button_with_icon(ui, Some(icon::SPARKLE), label, Kind::Ai, None, idle)
                    .clicked()
            };
            if ai(ui, &subject) {
                action = Some(Action::SelectSubject);
            }
            if ai(ui, &background) {
                action = Some(Action::RemoveBackground);
            }
        });
        if let Some(act) = action {
            self.run(act);
        }
    }

    fn bucket_options(&mut self, ui: &mut egui::Ui) {
        ui_kit::swatch(ui, &mut self.primary, 22.0, &t!("options-foreground"));
        tolerance_field(ui, &mut self.bucket_tolerance);
        ui_kit::checkbox(ui, &mut self.bucket_contiguous, &t!("options-contiguous"))
            .on_hover_text(t!("options-bucket-contiguous-tip"));
        trailing_hint(ui, &t!("options-bucket-hint"));
    }

    fn eyedropper_options(&mut self, ui: &mut egui::Ui) {
        let foreground = t!("options-foreground");
        let background = t!("optbar-background");
        for (label, swatch) in [
            (&foreground, &mut self.primary),
            (&background, &mut self.secondary),
        ] {
            ui.label(egui::RichText::new(label).color(color::LABEL));
            ui_kit::swatch(ui, swatch, 22.0, label);
        }
        trailing_hint(ui, &t!("options-eyedropper-hint"));
    }

    fn canvas_options(&mut self, ui: &mut egui::Ui) {
        let Some(full) = self.full_rect() else { return };
        let mut r = self.canvas_rect.unwrap_or(full);

        let mut w = r.width() as u32;
        let mut h = r.height() as u32;
        let w_changed = size_field(ui, &t!("optbar-width-short"), &t!("options-width"), &mut w);
        let h_changed = size_field(
            ui,
            &t!("optbar-height-short"),
            &t!("options-height"),
            &mut h,
        );
        if w_changed || h_changed {
            // Typed in: the area grows or shrinks around its center.
            r = round_rect(Rect::from_center_size(r.center(), vec2(w as f32, h as f32)));
            self.canvas_rect = Some(r);
        }
        let (x, y) = (signed(r.min.x), signed(r.min.y));
        ui.label(
            egui::RichText::new(t!("options-canvas-origin", x = x, y = y))
                .font(ui_kit::mono(12.0))
                .color(color::LABEL),
        );
        sep(ui);
        if ui_kit::button(
            ui,
            &t!("options-fit-visible-pixels"),
            Kind::Normal,
            None,
            true,
        )
        .on_hover_text(t!("options-fit-visible-pixels-tip"))
        .clicked()
            && let Some(b) = self.doc.as_ref().and_then(Document::content_bounds)
        {
            r = to_rect(b);
            self.canvas_rect = Some(r);
        }

        let (apply, cancel) = apply_cancel(ui, r != full);
        if apply {
            self.canvas_rect = Some(r);
            self.apply_canvas();
        }
        if cancel {
            self.reset_pending();
        }
    }

    /// Hand and Zoom tools: zoom presets and Fit on Screen.
    fn view_options(&mut self, ui: &mut egui::Ui) {
        let current = ZOOM_PRESETS
            .into_iter()
            .find(|&z| (self.zoom * 100.0 - z as f32).abs() < 0.01);
        let mut zoom = current.unwrap_or(0);
        let labels = ZOOM_PRESETS.map(i18n::percent);
        let actual = keys::shortcut(ui.ctx(), &SHORTCUT_ACTUAL);
        let options: Vec<(u32, &str, &str)> = ZOOM_PRESETS
            .iter()
            .zip(&labels)
            .map(|(&z, label)| {
                let tip = if z == 100 { actual.as_str() } else { "" };
                (z, label.as_str(), tip)
            })
            .collect();
        if ui_kit::segmented(ui, &mut zoom, &options) {
            if zoom == 100 {
                self.run(Action::ActualSize);
            } else {
                // Around the center of the view.
                self.set_zoom_around(zoom as f32 / 100.0, Pos2::ZERO, Pos2::ZERO);
            }
        }
        if ui_kit::button(ui, &t!("menu-view-fit-on-screen"), Kind::Normal, None, true)
            .on_hover_text(keys::shortcut(ui.ctx(), &SHORTCUT_FIT))
            .clicked()
        {
            self.run(Action::Fit);
        }
    }
}

impl AtelierApp {
    /// Marquees and lassos: Feather, Anti-alias (not for the rectangle),
    /// and the marquees' Style.
    fn shape_options(&mut self, ui: &mut egui::Ui) {
        let label = t!("sel-feather");
        let label_w = ui
            .painter()
            .layout_no_wrap(label.clone(), ui_kit::medium(11.0), color::LABEL)
            .size()
            .x;
        let edges = self.sel.shape.edges(self.tool);
        ui_kit::number_field(
            ui,
            &label,
            &mut edges.feather,
            0.0..=1000.0,
            " px",
            label_w + 64.0,
        )
        .on_hover_text(t!("sel-feather-tip"));
        if self.tool != Tool::Select {
            ui_kit::checkbox(ui, &mut edges.antialias, &t!("sel-anti-alias"));
        }
        let o = &mut self.sel.shape;
        if !matches!(self.tool, Tool::Select | Tool::Ellipse) {
            return;
        }
        let style = o.style.label();
        dropdown(
            ui,
            "marquee_style",
            Some(&t!("sel-style")),
            &style,
            false,
            0.0,
            |ui| {
                for s in MarqueeStyle::ALL {
                    if ui.selectable_label(o.style == s, s.label()).clicked() {
                        o.style = s;
                        ui.close();
                    }
                }
            },
        );
        let (w, h) = (t!("optbar-width-short"), t!("optbar-height-short"));
        match o.style {
            MarqueeStyle::Normal => {}
            MarqueeStyle::FixedRatio => {
                let (rw, rh) = &mut o.ratio;
                ui_kit::number_field(ui, &w, rw, 0.001..=999.0, "", 86.0);
                if ui_kit::icon_button(ui, icon::ARROWS_LEFT_RIGHT, &t!("sel-swap-size"), true)
                    .clicked()
                {
                    std::mem::swap(rw, rh);
                }
                ui_kit::number_field(ui, &h, rh, 0.001..=999.0, "", 86.0);
            }
            MarqueeStyle::FixedSize => {
                let (sw, sh) = &mut o.size;
                ui_kit::number_field(ui, &w, sw, 1..=MAX_DIMENSION, " px", 96.0);
                if ui_kit::icon_button(ui, icon::ARROWS_LEFT_RIGHT, &t!("sel-swap-size"), true)
                    .clicked()
                {
                    std::mem::swap(sw, sh);
                }
                ui_kit::number_field(ui, &h, sh, 1..=MAX_DIMENSION, " px", 96.0);
            }
        }
    }

    /// Transform Selection in progress: what to do, Cancel and Apply.
    fn selection_transform_options(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(t!("sel-menu-transform")).color(color::TEXT));
        trailing_hint(ui, &t!("sel-transform-hint"));
        let (apply, cancel) = apply_cancel(ui, true);
        if apply {
            self.apply_selection_transform();
        }
        if cancel {
            self.sel.transform = None;
        }
    }
}

/// Room left on the current row.
fn room(ui: &egui::Ui) -> f32 {
    ui.available_size_before_wrap().x
}

/// Separator between groups; at the end of a full row, starts the next one.
fn sep(ui: &mut egui::Ui) {
    if room(ui) < 80.0 {
        ui.end_row();
        return;
    }
    ui.add_space(2.0);
    ui_kit::separator(ui);
    ui.add_space(2.0);
}

/// Hint at the end of the row, after a separator: left out when it doesn't fit.
fn trailing_hint(ui: &mut egui::Ui, text: &str) {
    let width = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(12.0), color::LABEL)
        .size()
        .x;
    if room(ui) >= width + 5.0 + 2.0 * ui.spacing().item_spacing.x {
        sep(ui);
        ui_kit::hint(ui, text);
    }
}

/// Lock and what is locked, then how to unlock it if there is room.
fn locked_notice(ui: &mut egui::Ui, label: &str, hint: &str) {
    ui.label(egui::RichText::new(format!("{} {label}", icon::LOCK_SIMPLE)).color(color::MUTED));
    trailing_hint(ui, hint);
}

/// Width of a `ui_kit::button` (with or without icon).
fn button_width(ui: &egui::Ui, label: &str, with_icon: bool) -> f32 {
    let text = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            ui_kit::medium(ui_kit::TEXT_SIZE),
            color::TEXT,
        )
        .size()
        .x;
    text + if with_icon { 23.0 } else { 0.0 } + 24.0
}

/// Group of `width` pushed to the right end of the row (of the next one if
/// it doesn't fit).
fn right_aligned(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    if room(ui) < width {
        ui.end_row();
    }
    ui.add_space((room(ui) - width).max(0.0));
    add(ui);
}

/// Cancel and Apply on the right, enabled while something is pending.
/// Returns the clicks (apply, cancel).
fn apply_cancel(ui: &mut egui::Ui, pending: bool) -> (bool, bool) {
    let (cancel, apply) = (t!("dialog-cancel"), t!("options-apply"));
    let width = button_width(ui, &cancel, false)
        + button_width(ui, &apply, true)
        + ui.spacing().item_spacing.x;
    let mut clicks = (false, false);
    // Short of room, the two buttons become icons rather than a new row.
    if room(ui) < width {
        clicks.1 = ui_kit::icon_button(ui, icon::X, &cancel, pending).clicked();
        clicks.0 = ui_kit::icon_button(ui, icon::CHECK, &apply, pending).clicked();
        return clicks;
    }
    right_aligned(ui, width, |ui| {
        clicks.1 = ui_kit::button(ui, &cancel, Kind::Normal, None, pending).clicked();
        clicks.0 =
            ui_kit::button_with_icon(ui, Some(icon::CHECK), &apply, Kind::Primary, None, pending)
                .clicked();
    });
    clicks
}

/// Width or height field (px); returns `true` if it changed.
fn size_field(ui: &mut egui::Ui, label: &str, tip: &str, value: &mut u32) -> bool {
    ui_kit::number_field(ui, label, value, 1..=MAX_DIMENSION, "", 92.0)
        .on_hover_text(tip)
        .changed()
}

/// Tolerance out of 255, like Photoshop (stored as 0..1).
fn tolerance_field(ui: &mut egui::Ui, tolerance: &mut f32) {
    let mut value = (*tolerance * 255.0).round() as u32;
    if ui_kit::number_field(ui, &t!("options-tolerance"), &mut value, 0..=255, "", 130.0).changed()
    {
        *tolerance = value as f32 / 255.0;
    }
}

/// Drop-down list in the bar: optional grey label, then the value (`mono`:
/// a number), `lead` px left free before it (for a drawn icon). The popup
/// stays open while it is used; items close it with `ui.close()`.
fn dropdown<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    label: Option<&str>,
    value: &str,
    mono: bool,
    lead: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let format = |font_id, color| egui::TextFormat {
        font_id,
        color,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    if let Some(label) = label {
        job.append(
            label,
            0.0,
            format(FontId::proportional(ui_kit::TEXT_SIZE), color::LABEL),
        );
    }
    let font = if mono {
        ui_kit::mono(12.0)
    } else {
        FontId::proportional(ui_kit::TEXT_SIZE)
    };
    let space = if label.is_some() { 8.0 } else { 0.0 };
    job.append(value, lead + space, format(font, color::TEXT));
    let text = ui.painter().layout_job(job.clone()).size().x;
    let s = ui.spacing();
    let width = text + 2.0 * s.button_padding.x + s.icon_width + s.icon_spacing + 2.0;
    if room(ui) < width {
        ui.end_row();
    }
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(job)
        .width(width)
        .height(320.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, add_contents)
}
