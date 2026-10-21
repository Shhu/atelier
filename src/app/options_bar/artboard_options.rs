//! Options of the Artboard tool: the active artboard's size (presets, W
//! and H, orientation) and a button adding one next to it.

use super::*;
use crate::app::artboard_tool::{orientation_field, preset_combo};
use crate::document::Side;

impl AtelierApp {
    pub(super) fn artboard_options(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &self.doc else { return };
        let Some(a) = doc.artboard_of(doc.active) else {
            ui.label(egui::RichText::new(t!("artboard-options-hint")).color(color::MUTED));
            return;
        };
        let layer = &doc.layers[a];
        if layer.locked {
            return locked_notice(
                ui,
                &t!("options-layer-locked"),
                &t!("artboard-options-hint"),
            );
        }
        let (id, Some(r)) = (layer.id, layer.artboard_rect()) else {
            return;
        };
        let mut size = (r.w, r.h);
        ui.label(egui::RichText::new(t!("artboard-options-size")).color(color::LABEL));
        if let Some(s) = preset_combo(ui, "artboard_options_preset", size, 200.0) {
            size = s;
        }
        size_field(
            ui,
            &t!("optbar-width-short"),
            &t!("options-width"),
            &mut size.0,
        );
        size_field(
            ui,
            &t!("optbar-height-short"),
            &t!("options-height"),
            &mut size.1,
        );
        orientation_field(ui, &mut size);
        sep(ui);
        let add = ui_kit::button_with_icon(
            ui,
            Some(icon::PLUS),
            &t!("artboard-options-add"),
            Kind::Normal,
            None,
            true,
        )
        .on_hover_text(t!("artboard-options-add-tip"));
        if size != (r.w, r.h) {
            self.record_change(t!("artboard-hist-resize"), Some((id, "artboard-size")));
            if let Some(doc) = &mut self.doc {
                let rect = PixelRect {
                    w: size.0,
                    h: size.1,
                    ..r
                };
                doc.set_artboard_rect(a, rect);
            }
            self.refit_artboards();
        }
        if add.clicked()
            && let Some(doc) = &mut self.doc
            && doc.add_adjacent_artboard(a, Side::Right, false)
        {
            self.refit_artboards();
        }
    }
}
