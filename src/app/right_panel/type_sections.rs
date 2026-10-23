//! Character and Paragraph sections of the Properties panel, for text
//! layers: the attributes of the selected characters (of the whole text
//! outside an edit) and of their paragraphs. The text itself can be typed
//! there too.

use super::*;
use crate::app::type_tool::{
    ANTIALIAS, align_options, antialias_name, face_label, face_list, family_label, family_list,
};
use crate::text::Kerning;

/// Width of a field in a row of two.
fn half(ui: &Ui) -> f32 {
    (ui.available_width() - GAP) / 2.0
}

/// A percentage field over a ratio (1 = 100 %).
fn percent_field(ui: &mut Ui, label: &str, ratio: &mut f32, width: f32, tip: String) {
    let mut v = *ratio * 100.0;
    let suffix = crate::i18n::percent("");
    if ui_kit::number_field(ui, label, &mut v, 0.0..=1000.0, &suffix, width)
        .on_hover_text(tip)
        .changed()
    {
        *ratio = (v / 100.0).max(0.01);
    }
}

impl AtelierApp {
    pub(super) fn character_section(&mut self, ui: &mut Ui, locked: bool) {
        let Some(content) = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().text.as_ref())
            .map(|t| t.text.clone())
        else {
            return;
        };
        let old = self.shown_char_style();
        let mut s = old.clone();
        let mut typed = content.clone();
        let families = self.text_engine().families().to_vec();
        let faces = self.text_engine().faces(&s.family);
        let antialias = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().text.as_ref())
            .map_or(self.type_tool.antialias, |t| t.antialias);
        let mut aa = antialias;
        let filter = &mut self.font_filter;
        section(ui, Some(&t!("side-character")), |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                egui::TextEdit::multiline(&mut typed)
                    .id(Id::new("text_layer_content"))
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .margin(vec2(9.0, 7.0))
                    .font(FontId::proportional(ui_kit::TEXT_SIZE))
                    .show(ui);

                let width = ui.available_width();
                ui_kit::combo(
                    ui,
                    "side_font",
                    None,
                    &family_label(&s.family),
                    width,
                    |ui| {
                        family_list(ui, &families, filter, &mut s.family);
                    },
                )
                .response
                .on_hover_text(t!("options-font"));
                ui_kit::combo(
                    ui,
                    "side_font_style",
                    None,
                    &face_label(&faces, &s),
                    width,
                    |ui| {
                        face_list(ui, &faces, &mut s);
                    },
                )
                .response
                .on_hover_text(t!("type-font-style-tip"));

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let w = half(ui);
                    ui_kit::number_field(
                        ui,
                        &t!("panel-size"),
                        &mut s.size,
                        1.0..=5000.0,
                        " px",
                        w,
                    )
                    .on_hover_text(t!("type-size-tip"));
                    let mut leading = s.line_height();
                    if ui_kit::number_field(
                        ui,
                        &t!("type-leading"),
                        &mut leading,
                        0.0..=10_000.0,
                        " px",
                        w,
                    )
                    .on_hover_text(t!("type-leading-tip"))
                    .changed()
                    {
                        s.leading = Some(leading);
                    }
                });
                let mut auto = s.leading.is_none();
                if ui_kit::checkbox(ui, &mut auto, &t!("type-leading-auto"))
                    .on_hover_text(t!("type-leading-auto-tip"))
                    .changed()
                {
                    s.leading = (!auto).then(|| s.line_height());
                }
                let kerning = |k: Kerning| match k {
                    Kerning::Metrics => t!("type-kerning-metrics"),
                    Kerning::Zero => "0".to_owned(),
                };
                ui_kit::combo(
                    ui,
                    "side_kerning",
                    Some(&t!("type-kerning")),
                    &kerning(s.kerning),
                    width,
                    |ui| {
                        for k in [Kerning::Metrics, Kerning::Zero] {
                            ui.selectable_value(&mut s.kerning, k, kerning(k));
                        }
                    },
                )
                .response
                .on_hover_text(t!("type-kerning-tip"));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let w = half(ui);
                    ui_kit::number_field(
                        ui,
                        &t!("type-tracking"),
                        &mut s.tracking,
                        -1000.0..=10_000.0,
                        "",
                        w,
                    )
                    .on_hover_text(t!("type-tracking-tip"));
                    ui_kit::number_field(
                        ui,
                        &t!("type-baseline"),
                        &mut s.baseline_shift,
                        -10_000.0..=10_000.0,
                        " px",
                        w,
                    )
                    .on_hover_text(t!("type-baseline-tip"));
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let w = half(ui);
                    percent_field(
                        ui,
                        &t!("type-vscale"),
                        &mut s.vertical_scale,
                        w,
                        t!("type-vscale-tip"),
                    );
                    percent_field(
                        ui,
                        &t!("type-hscale"),
                        &mut s.horizontal_scale,
                        w,
                        t!("type-hscale-tip"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let glyph = FontId::proportional(15.0);
                    let caps = FontId::proportional(12.0);
                    let tips = [
                        t!("type-faux-bold"),
                        t!("type-faux-italic"),
                        t!("type-all-caps"),
                        t!("type-small-caps"),
                        t!("type-underline"),
                        t!("type-strikethrough"),
                    ];
                    ui_kit::toggles(
                        ui,
                        &mut [
                            (&mut s.faux_bold, icon::TEXT_B, glyph.clone(), &tips[0]),
                            (
                                &mut s.faux_italic,
                                icon::TEXT_ITALIC,
                                glyph.clone(),
                                &tips[1],
                            ),
                            (&mut s.all_caps, "TT", caps.clone(), &tips[2]),
                            (&mut s.small_caps, "Tt", caps, &tips[3]),
                            (
                                &mut s.underline,
                                icon::TEXT_UNDERLINE,
                                glyph.clone(),
                                &tips[4],
                            ),
                            (
                                &mut s.strikethrough,
                                icon::TEXT_STRIKETHROUGH,
                                glyph,
                                &tips[5],
                            ),
                        ],
                    );
                    let [r, g, b, a] = s.color;
                    let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
                    if ui_kit::swatch(ui, &mut c, 28.0, &t!("side-text-color-tip")).changed() {
                        s.color = c.to_srgba_unmultiplied();
                    }
                });
                ui_kit::combo(
                    ui,
                    "side_antialias",
                    Some(&t!("type-aa")),
                    &antialias_name(aa),
                    width,
                    |ui| {
                        for option in ANTIALIAS {
                            ui.selectable_value(&mut aa, option, antialias_name(option));
                        }
                    },
                )
                .response
                .on_hover_text(t!("type-aa-tip"));
            });
        });
        if typed != content {
            self.replace_text(&typed);
        }
        if aa != antialias {
            self.set_antialias(aa);
        }
        self.apply_char_edit(&old, &s);
    }

    pub(super) fn paragraph_section(&mut self, ui: &mut Ui, locked: bool) {
        let old = self.shown_paragraph_style();
        let mut p = old;
        section(ui, Some(&t!("type-paragraph")), |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                let aligns = align_options();
                let options: Vec<_> = aligns
                    .iter()
                    .map(|(a, glyph, tip)| (*a, *glyph, tip.as_str()))
                    .collect();
                ui_kit::segmented(ui, &mut p.align, &options);
                let field = |ui: &mut Ui, label: String, value: &mut f32, w: f32, tip: String| {
                    ui_kit::number_field(ui, &label, value, -10_000.0..=10_000.0, " px", w)
                        .on_hover_text(tip);
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let w = half(ui);
                    field(
                        ui,
                        t!("type-indent-left"),
                        &mut p.indent_left,
                        w,
                        t!("type-indent-left-tip"),
                    );
                    field(
                        ui,
                        t!("type-indent-right"),
                        &mut p.indent_right,
                        w,
                        t!("type-indent-right-tip"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let w = half(ui);
                    field(
                        ui,
                        t!("type-indent-first"),
                        &mut p.indent_first,
                        w,
                        t!("type-indent-first-tip"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let w = half(ui);
                    field(
                        ui,
                        t!("type-space-before"),
                        &mut p.space_before,
                        w,
                        t!("type-space-before-tip"),
                    );
                    field(
                        ui,
                        t!("type-space-after"),
                        &mut p.space_after,
                        w,
                        t!("type-space-after-tip"),
                    );
                });
            });
        });
        self.apply_paragraph_edit(&old, &p);
    }
}
