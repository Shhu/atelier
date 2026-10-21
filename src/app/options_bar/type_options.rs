//! Type tool options: font family and style, size, anti-aliasing,
//! alignment and color of the selected characters (or of the next texts),
//! Cancel and Commit while a text is typed.

use super::*;
use crate::app::type_tool::{
    ANTIALIAS, align_options, antialias_name, face_label, face_list, family_label, family_list,
};

impl AtelierApp {
    pub(super) fn text_options(&mut self, ui: &mut egui::Ui) {
        let is_text = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active_layer().text.is_some());
        if self.active_locked() && is_text {
            locked_notice(
                ui,
                &t!("options-text-locked"),
                &t!("options-text-locked-hint"),
            );
            return;
        }
        let old = self.shown_char_style();
        let mut style = old.clone();
        let families = self.text_engine().families().to_vec();
        let faces = self.text_engine().faces(&style.family);

        let filter = &mut self.font_filter;
        dropdown(
            ui,
            "font_family",
            None,
            &family_label(&style.family),
            false,
            0.0,
            |ui| {
                family_list(ui, &families, filter, &mut style.family);
            },
        )
        .response
        .on_hover_text(t!("options-font"));
        dropdown(
            ui,
            "font_style",
            None,
            &face_label(&faces, &style),
            false,
            0.0,
            |ui| {
                face_list(ui, &faces, &mut style);
            },
        )
        .response
        .on_hover_text(t!("type-font-style-tip"));
        ui_kit::number_field(
            ui,
            &t!("options-size"),
            &mut style.size,
            1.0..=5000.0,
            " px",
            96.0,
        )
        .on_hover_text(t!("type-size-tip"));

        let antialias = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().text.as_ref())
            .map_or(self.type_tool.antialias, |t| t.antialias);
        let mut aa = antialias;
        dropdown(
            ui,
            "text_antialias",
            None,
            &antialias_name(aa),
            false,
            0.0,
            |ui| {
                for option in ANTIALIAS {
                    if ui
                        .selectable_label(aa == option, antialias_name(option))
                        .clicked()
                    {
                        aa = option;
                        ui.close();
                    }
                }
            },
        )
        .response
        .on_hover_text(t!("type-aa-tip"));
        sep(ui);

        let old_paragraph = self.shown_paragraph_style();
        let mut paragraph = old_paragraph;
        let aligns = align_options();
        let options: Vec<_> = aligns
            .iter()
            .map(|(a, glyph, tip)| (*a, *glyph, tip.as_str()))
            .collect();
        ui_kit::segmented(ui, &mut paragraph.align, &options);
        sep(ui);

        if is_text {
            let [r, g, b, a] = style.color;
            let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
            if ui_kit::swatch(ui, &mut c, 22.0, &t!("options-color")).changed() {
                style.color = c.to_srgba_unmultiplied();
            }
            if ui_kit::button(ui, &t!("optbar-use-foreground"), Kind::Normal, None, true)
                .on_hover_text(t!("options-use-foreground-tip"))
                .clicked()
            {
                style.color = self.primary.to_srgba_unmultiplied();
            }
        } else {
            // New texts take the foreground color: this swatch is the foreground.
            ui_kit::swatch(ui, &mut self.primary, 22.0, &t!("options-text-color-hint"));
            style.color = old.color;
        }

        if self.typing_on_canvas() {
            let (commit, cancel) = apply_cancel(ui, true);
            if cancel {
                self.cancel_text();
                return;
            }
            if commit {
                self.commit_text();
                return;
            }
        } else if !is_text {
            trailing_hint(ui, &t!("type-click-hint"));
        }

        if aa != antialias {
            self.set_antialias(aa);
        }
        self.apply_paragraph_edit(&old_paragraph, &paragraph);
        self.apply_char_edit(&old, &style);
    }
}
