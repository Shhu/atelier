//! Crop tool: ratio or size, overlay and shield, Straighten, Delete Cropped
//! Pixels.

use super::super::crop_tool::{CropOverlay, CropPreset};
use super::*;

impl AtelierApp {
    pub(super) fn canvas_options(&mut self, ui: &mut egui::Ui) {
        let Some(full) = self.full_rect() else { return };
        self.crop_sync_original();
        let current = self.crop.preset;
        let mut picked = None;
        let list = dropdown(ui, "crop_preset", None, &current.label(), false, 0.0, |ui| {
            for preset in CropPreset::ALL {
                if ui.selectable_label(current == preset, preset.label()).clicked() {
                    picked = Some(preset);
                    ui.close();
                }
            }
        });
        ui_kit::name_control(ui, list.response.id, &t!("crop9-preset-name"), &current.label());
        if let Some(preset) = picked {
            self.crop_pick_preset(preset, full);
        }
        self.crop_values(ui, full);
        let free = self.crop.preset == CropPreset::Ratio && self.crop_ratio().is_none();
        if ui_kit::button(ui, &t!("crop8-clear"), Kind::Normal, None, !free)
            .on_hover_text(t!("crop8-clear-tip"))
            .clicked()
        {
            self.crop_clear();
        }
        sep(ui);
        ui_kit::icon_toggle(
            ui,
            icon::ANGLE,
            &t!("crop8-straighten-tip"),
            &mut self.crop.straighten,
        );
        if ui_kit::icon_button(ui, icon::SELECTION_ALL, &t!("options-fit-visible-pixels"), true)
            .clicked()
        {
            self.crop_fit_visible();
        }
        self.crop_view_menu(ui);
        ui_kit::checkbox(ui, &mut self.crop.delete_cropped, &t!("crop8-delete-cropped"))
            .on_hover_text(t!("crop8-delete-cropped-tip"));
        if self.crop.angle != 0.0 {
            let angle = i18n::short_decimal(f64::from((self.crop.angle * 10.0).round() / 10.0));
            ui.label(
                egui::RichText::new(format!("{angle}°"))
                    .font(ui_kit::mono(12.0))
                    .color(color::LABEL),
            );
        }
        // Icons, like the transform bars: the row doesn't wrap for them.
        // Lit when Enter applies, by the same rule.
        let (apply, cancel) = apply_cancel_icons(ui, self.crop_box_pending());
        if apply {
            self.apply_crop_box();
            self.reset_pending();
        }
        if cancel {
            self.reset_pending();
        }
    }

    /// W, swap, H, and the resolution with W × H × Resolution. Empty is
    /// shown empty; pixels say so, and a ratio reads as one.
    fn crop_values(&mut self, ui: &mut egui::Ui, full: Rect) {
        let size = self.crop.preset == CropPreset::Size;
        let suffix = if size { " px" } else { "" };
        let tip = |pixels: String| if size { pixels } else { t!("crop9-ratio-tip") };
        let max = MAX_DIMENSION as f32;
        let mut edited = ui_kit::blank_number_field(
            ui,
            &t!("optbar-width-short"),
            &mut self.crop.w,
            0.0..=max,
            suffix,
            82.0,
        )
        .on_hover_text(tip(t!("options-width")))
        .changed();
        if !size {
            ui.label(egui::RichText::new(":").color(color::LABEL));
        }
        if ui_kit::icon_button(ui, icon::ARROWS_LEFT_RIGHT, &t!("crop8-swap"), true).clicked() {
            self.crop_swap(full);
        }
        edited |= ui_kit::blank_number_field(
            ui,
            &t!("optbar-height-short"),
            &mut self.crop.h,
            0.0..=max,
            suffix,
            82.0,
        )
        .on_hover_text(tip(t!("options-height")))
        .changed();
        if size {
            let typed = ui_kit::blank_number_field(
                ui,
                &t!("crop8-resolution"),
                &mut self.crop.resolution,
                0.0..=crate::document::MAX_RESOLUTION,
                " ppi",
                110.0,
            )
            .on_hover_text(t!("crop8-resolution-tip"))
            .changed();
            if typed && self.canvas_rect.is_none() {
                self.canvas_rect = Some(self.crop_start_rect(full));
            }
        }
        if edited {
            if size {
                (self.crop.w, self.crop.h) = (self.crop.w.round(), self.crop.h.round());
            }
            self.crop_values_edited(full);
        }
    }

    /// Overlay and shield opacity.
    fn crop_view_menu(&mut self, ui: &mut egui::Ui) {
        let name = t!("crop8-overlay");
        let response = dropdown(ui, "crop_overlay", None, icon::GRID_NINE, false, 0.0, |ui| {
            ui.set_width(220.0);
            ui_kit::field_label(ui, &name);
            for overlay in CropOverlay::ALL {
                if ui
                    .selectable_label(self.crop.overlay == overlay, overlay.label())
                    .clicked()
                {
                    self.crop.overlay = overlay;
                }
            }
            ui.separator();
            let mut shield = (self.crop.shield * 100.0).round() as u32;
            if ui_kit::number_field(ui, &t!("crop8-shield"), &mut shield, 0..=100, " %", 200.0)
                .on_hover_text(t!("crop8-shield-tip"))
                .changed()
            {
                self.crop.shield = shield as f32 / 100.0;
            }
        })
        .response
        .on_hover_text(&name);
        // Painted as a glyph: said by its name and the overlay chosen.
        ui_kit::name_control(ui, response.id, &name, &self.crop.overlay.label());
    }
}
