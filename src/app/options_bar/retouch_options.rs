//! Options of the retouching tools: the brush and the source of the Clone
//! Stamp and the healing brushes, the Patch tool's mode, the Red Eye tool's
//! amounts.

use super::*;
use crate::app::retouch_tools::{PatchMode, Sample, SpotType};

impl AtelierApp {
    pub(super) fn retouch_options(&mut self, ui: &mut egui::Ui) {
        let percent = |v: f32| i18n::percent(format!("{:.0}", v * 100.0));
        if self.tool.retouch_brush() {
            self.brush_picker(ui);
            sep(ui);
        }
        let r = &mut self.retouch;
        let sample = |ui: &mut egui::Ui, id: &str, value: &mut Sample| {
            ui_kit::combo(
                ui,
                id,
                Some(&t!("retouch-sample")),
                &value.label(),
                220.0,
                |ui| {
                    for s in Sample::ALL {
                        ui.selectable_value(value, s, s.label());
                    }
                },
            );
        };
        let pressure_size = |ui: &mut egui::Ui, on: &mut bool| {
            ui_kit::icon_toggle(ui, icon::PENCIL_CIRCLE, &t!("paint-pressure-size-tip"), on);
        };
        let hint = match self.tool {
            Tool::CloneStamp => {
                let text = percent(r.clone_opacity);
                ui_kit::scrub(
                    ui,
                    &t!("options-opacity"),
                    &mut r.clone_opacity,
                    0.01..=1.0,
                    &text,
                    150.0,
                );
                let p = &mut self.paint;
                ui_kit::icon_toggle(
                    ui,
                    icon::PEN_NIB,
                    &t!("paint-pressure-opacity-tip"),
                    &mut p.pressure_opacity,
                );
                let text = percent(r.clone_flow);
                ui_kit::scrub(
                    ui,
                    &t!("paint-flow"),
                    &mut r.clone_flow,
                    0.01..=1.0,
                    &text,
                    150.0,
                );
                ui_kit::checkbox(ui, &mut r.clone_aligned, &t!("retouch-aligned"))
                    .on_hover_text(t!("retouch-aligned-tip"));
                sample(ui, "clone_sample", &mut r.clone_sample);
                pressure_size(ui, &mut p.pressure_size);
                t!(
                    "input3-clone-hint",
                    alt_click = keys::click(ui.ctx(), Modifiers::ALT)
                )
            }
            Tool::HealingBrush => {
                ui_kit::checkbox(ui, &mut r.heal_aligned, &t!("retouch-aligned"))
                    .on_hover_text(t!("retouch-aligned-tip"));
                sample(ui, "heal_sample", &mut r.heal_sample);
                pressure_size(ui, &mut self.paint.pressure_size);
                t!(
                    "input3-healing-hint",
                    alt_click = keys::click(ui.ctx(), Modifiers::ALT)
                )
            }
            Tool::SpotHealing => {
                ui_kit::field_label(ui, &t!("retouch-type"));
                ui_kit::segmented(
                    ui,
                    &mut r.spot_type,
                    &[
                        (
                            SpotType::ContentAware,
                            &t!("retouch-content-aware"),
                            &t!("retouch-content-aware-tip"),
                        ),
                        (
                            SpotType::ProximityMatch,
                            &t!("retouch-proximity-match"),
                            &t!("retouch-proximity-match-tip"),
                        ),
                    ],
                );
                ui_kit::checkbox(ui, &mut r.spot_sample_all, &t!("options-sample-all-layers"))
                    .on_hover_text(t!("retouch-spot-sample-all-tip"));
                pressure_size(ui, &mut self.paint.pressure_size);
                t!("retouch-spot-hint")
            }
            Tool::Patch => {
                ui_kit::field_label(ui, &t!("retouch-patch"));
                ui_kit::segmented(
                    ui,
                    &mut r.patch_mode,
                    &[
                        (
                            PatchMode::Source,
                            &t!("retouch-patch-source"),
                            &t!("retouch-patch-source-tip"),
                        ),
                        (
                            PatchMode::Destination,
                            &t!("retouch-patch-destination"),
                            &t!("retouch-patch-destination-tip"),
                        ),
                    ],
                );
                t!("retouch-patch-hint")
            }
            _ => {
                let text = percent(r.pupil);
                ui_kit::scrub(
                    ui,
                    &t!("retouch-pupil-size"),
                    &mut r.pupil,
                    0.01..=1.0,
                    &text,
                    170.0,
                );
                let text = percent(r.darken);
                ui_kit::scrub(
                    ui,
                    &t!("retouch-darken-amount"),
                    &mut r.darken,
                    0.01..=1.0,
                    &text,
                    190.0,
                );
                t!("retouch-red-eye-hint")
            }
        };
        trailing_hint(ui, &hint);
    }
}
