//! Crop tool: ratio or size, overlay and shield, Straighten, Delete Cropped
//! Pixels.

use super::super::crop_tool::{CropOverlay, CropPreset};
use super::*;

impl AtelierApp {
    pub(super) fn canvas_options(&mut self, ui: &mut egui::Ui) {
        let Some(full) = self.full_rect() else { return };
        let current = self.crop.preset;
        let mut picked = None;
        dropdown(ui, "crop_preset", None, &current.label(), false, 0.0, |ui| {
            for preset in CropPreset::ALL {
                if ui.selectable_label(current == preset, preset.label()).clicked() {
                    picked = Some(preset);
                    ui.close();
                }
            }
        });
        if let Some(preset) = picked {
            self.crop_pick_preset(preset, full);
        }
        self.crop_values(ui, full);
        let free = self.crop.preset == CropPreset::Ratio && self.crop.ratio().is_none();
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
        let resampled = self.crop.preset == CropPreset::Size
            && self.crop.ratio().is_some()
            && (self.crop.w, self.crop.h) != (full.width(), full.height());
        let pending = self.crop_box_drawn(full) || resampled;
        let (apply, cancel) = apply_cancel_icons(ui, pending);
        if apply {
            self.canvas_rect.get_or_insert(full);
            self.apply_crop_box();
            self.reset_pending();
        }
        if cancel {
            self.reset_pending();
        }
    }

    /// W, swap, H, and the resolution with W × H × Resolution.
    fn crop_values(&mut self, ui: &mut egui::Ui, full: Rect) {
        let size = self.crop.preset == CropPreset::Size;
        let suffix = if size { " px" } else { "" };
        let max = MAX_DIMENSION as f32;
        let mut edited = ui_kit::number_field(
            ui,
            &t!("optbar-width-short"),
            &mut self.crop.w,
            0.0..=max,
            suffix,
            82.0,
        )
        .on_hover_text(t!("options-width"))
        .changed();
        if ui_kit::icon_button(ui, icon::ARROWS_LEFT_RIGHT, &t!("crop8-swap"), true).clicked() {
            self.crop_swap(full);
        }
        edited |= ui_kit::number_field(
            ui,
            &t!("optbar-height-short"),
            &mut self.crop.h,
            0.0..=max,
            suffix,
            82.0,
        )
        .on_hover_text(t!("options-height"))
        .changed();
        if size {
            ui_kit::number_field(
                ui,
                &t!("crop8-resolution"),
                &mut self.crop.resolution,
                0.0..=100_000.0,
                " ppi",
                110.0,
            )
            .on_hover_text(t!("crop8-resolution-tip"));
        }
        if edited {
            if size {
                (self.crop.w, self.crop.h) = (self.crop.w.round(), self.crop.h.round());
            }
            self.crop_values_edited(full);
        }
    }

    /// Overlay, shield opacity, and the box around the visible pixels.
    fn crop_view_menu(&mut self, ui: &mut egui::Ui) {
        let name = t!("crop8-overlay");
        let mut fit = false;
        dropdown(ui, "crop_overlay", None, icon::GRID_NINE, false, 0.0, |ui| {
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
            ui.separator();
            fit = ui_kit::button(ui, &t!("options-fit-visible-pixels"), Kind::Normal, None, true)
                .on_hover_text(t!("options-fit-visible-pixels-tip"))
                .clicked();
        })
        .response
        .on_hover_text(name);
        if fit && let Some(b) = self.doc.as_ref().and_then(Document::content_bounds) {
            self.canvas_rect = Some(to_rect(b));
            self.crop.angle = 0.0;
        }
    }
}
