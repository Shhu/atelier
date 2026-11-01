//! Options of the Color Sampler and Ruler tools.

use super::*;
use crate::app::measure_tools::SampleSize;

impl AtelierApp {
    pub(in crate::app) fn sampler_options(&mut self, ui: &mut egui::Ui) {
        let mut size = self.measure.sample;
        ui_kit::combo(
            ui,
            "sampler-size",
            Some(&t!("panels8-sample-size")),
            &size.label(),
            150.0,
            |ui| {
                for s in SampleSize::ALL {
                    ui.selectable_value(&mut size, s, s.label());
                }
            },
        );
        self.measure.sample = size;
        let any = self.doc.as_ref().is_some_and(|d| !d.samplers.is_empty());
        let clear = ui_kit::button(
            ui,
            &t!("panels8-clear-all"),
            ui_kit::Kind::Normal,
            None,
            any,
        )
        .on_disabled_hover_text(t!("panels8-why-no-samplers"));
        if clear.clicked() {
            self.clear_samplers();
        }
        let alt_click = keys::click(ui.ctx(), Modifiers::ALT);
        wrapped_hint(ui, &t!("panels8-sampler-hint", alt_click = alt_click));
    }

    pub(in crate::app) fn ruler_options(&mut self, ui: &mut egui::Ui) {
        let values = self.measure_values();
        for (name, value) in values.iter().flatten() {
            ui.label(egui::RichText::new(name).color(color::LABEL));
            ui.label(
                egui::RichText::new(value)
                    .font(ui_kit::mono(12.0))
                    .color(color::TEXT),
            );
        }
        if values.is_some() {
            sep(ui);
        }
        let any = values.is_some();
        let alt = ui.input(|i| i.modifiers.alt);
        let quick = self.in_quick_mask();
        let straighten = ui_kit::button(
            ui,
            &t!("panels8-straighten-layer"),
            ui_kit::Kind::Normal,
            None,
            any && !quick,
        )
        .on_hover_text(t!("panels8-straighten-tip"))
        .on_disabled_hover_text(if quick {
            t!("panels8-status-straighten-quick-mask")
        } else {
            t!("panels8-why-no-line")
        });
        if straighten.clicked() {
            self.straighten_layer(alt);
        }
        let clear = ui_kit::button(ui, &t!("panels8-clear"), ui_kit::Kind::Normal, None, any)
            .on_disabled_hover_text(t!("panels8-why-no-line"));
        if clear.clicked()
            && let Some(doc) = &mut self.doc
        {
            doc.ruler = None;
        }
        let alt_drag = keys::combo(ui.ctx(), Modifiers::ALT, &t!("hint-key-drag"));
        wrapped_hint(ui, &t!("panels9-ruler-hint", alt_drag = alt_drag));
    }
}
