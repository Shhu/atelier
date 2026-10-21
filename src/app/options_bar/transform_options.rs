//! Options of Free Transform beyond the turned rectangle's: the reference
//! point's locator, the switch to Warp, and the warp's options (style,
//! orientation, bend).

use super::*;
use crate::xform::WarpStyle;

fn style_name(style: WarpStyle) -> String {
    match style {
        WarpStyle::Custom => t!("xform-style-custom"),
        WarpStyle::Arc => t!("xform-style-arc"),
        WarpStyle::Arch => t!("xform-style-arch"),
        WarpStyle::Bulge => t!("xform-style-bulge"),
        WarpStyle::Flag => t!("xform-style-flag"),
        WarpStyle::Wave => t!("xform-style-wave"),
    }
}

impl AtelierApp {
    /// The reference point's locator: its 9 places in the frame.
    pub(super) fn reference_locator(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
        let response = response.on_hover_text(t!("xform-reference"));
        let at = |c: u8, r: u8| rect.min + vec2(6.0 + 8.0 * c as f32, 6.0 + 8.0 * r as f32);
        let painter = ui.painter();
        let line = UiStroke::new(1.0, color::LINE_HOVER);
        painter.rect_stroke(
            Rect::from_min_max(at(0, 0), at(2, 2)),
            0.0,
            line,
            StrokeKind::Middle,
        );
        let selected = self.reference_cell();
        for k in 0..9u8 {
            let cell = (k % 3, k / 3);
            let r = Rect::from_center_size(at(cell.0, cell.1), Vec2::splat(5.0));
            if selected == Some(cell) {
                painter.rect_filled(r, 1.0, color::ACCENT);
            } else {
                let fill = if response.hovered() {
                    color::BG3
                } else {
                    color::BG0
                };
                painter.rect(
                    r,
                    1.0,
                    fill,
                    UiStroke::new(1.0, color::MUTED),
                    StrokeKind::Inside,
                );
            }
        }
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let cell = |v: f32| ((v - 6.0) / 8.0).round().clamp(0.0, 2.0) as u8;
            let d = p - rect.min;
            self.set_reference_cell((cell(d.x), cell(d.y)));
        }
    }

    /// Switch between Free Transform and Warp, like Photoshop's.
    pub(super) fn warp_switch(&mut self, ui: &mut egui::Ui) {
        let mut warping = self.transform_warping();
        if ui_kit::icon_toggle(ui, icon::GRID_NINE, &t!("xform-toggle-warp"), &mut warping)
            .clicked()
        {
            self.toggle_warp();
        }
    }

    /// Options of a warp; `false` for the frame's.
    pub(super) fn warp_options(&mut self, ui: &mut egui::Ui) -> bool {
        if !self.transform_warping() {
            return false;
        }
        self.reference_locator(ui);
        sep(ui);
        let mut style = self.warp_style();
        let mut restyle = false;
        dropdown(
            ui,
            "warp_style",
            Some(&t!("xform-warp")),
            &style_name(style.0),
            false,
            0.0,
            |ui| {
                for s in WarpStyle::ALL {
                    if ui.selectable_label(style.0 == s, style_name(s)).clicked() {
                        style.0 = s;
                        restyle = true;
                        ui.close();
                    }
                }
            },
        );
        let bends = style.0 != WarpStyle::Custom;
        ui.add_enabled_ui(bends, |ui| {
            restyle |= ui_kit::segmented(
                ui,
                &mut style.2,
                &[
                    (false, icon::ARROWS_HORIZONTAL, &t!("xform-horizontal")),
                    (true, icon::ARROWS_VERTICAL, &t!("xform-vertical")),
                ],
            );
            restyle |= ui_kit::number_field(
                ui,
                &t!("xform-bend"),
                &mut style.1,
                -100.0..=100.0,
                "%",
                120.0,
            )
            .changed();
        });
        if restyle {
            self.set_warp_style(style);
        }
        sep(ui);
        self.resample_dropdown(ui);
        ui_kit::checkbox(ui, &mut self.fit_canvas, &t!("options-fit-canvas"))
            .on_hover_text(t!("options-fit-canvas-tip"));
        self.warp_switch(ui);
        let (apply, cancel) = apply_cancel(ui, true);
        if apply {
            self.apply_transform();
        }
        if cancel {
            self.reset_pending();
        }
        true
    }
}
