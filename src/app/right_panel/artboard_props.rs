//! Properties of an artboard, like Photoshop's: its position (moving it
//! moves its layers), its size, its background.

use super::*;
use crate::document::ArtboardBackground;

/// The background choices, "Other" holding a color.
fn background_label(b: ArtboardBackground) -> String {
    match b {
        ArtboardBackground::White => t!("app-fill-white"),
        ArtboardBackground::Black => t!("app-fill-black"),
        ArtboardBackground::Transparent => t!("dialog-canvas-size-transparent"),
        ArtboardBackground::Custom(_) => t!("artboard-background-other"),
    }
}

impl AtelierApp {
    pub(super) fn artboard_section(&mut self, ui: &mut Ui, locked: bool) {
        let Some(doc) = &self.doc else { return };
        let (i, layer) = (doc.active, doc.active_layer());
        let (Some(board), Some(r)) = (layer.artboard, layer.artboard_rect()) else {
            return;
        };
        let id = layer.id;
        let (mut x, mut y, mut w, mut h) = (r.x, r.y, r.w, r.h);
        let mut background = board.background;
        section(ui, Some(&t!("artboard-kind")), |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                let half = (ui.available_width() - GAP) / 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui_kit::number_field(ui, "X", &mut x, -MAX_OFFSET..=MAX_OFFSET, "", half);
                    ui_kit::number_field(ui, "Y", &mut y, -MAX_OFFSET..=MAX_OFFSET, "", half);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let (wl, hl) = (t!("optbar-width-short"), t!("optbar-height-short"));
                    ui_kit::number_field(ui, &wl, &mut w, 1..=MAX_DIMENSION, "", half);
                    ui_kit::number_field(ui, &hl, &mut h, 1..=MAX_DIMENSION, "", half);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui_kit::field_label(ui, &t!("artboard-background"));
                    let custom = matches!(background, ArtboardBackground::Custom(_));
                    let width = ui.available_width() - if custom { 28.0 + GAP } else { 0.0 };
                    let shown = background_label(background);
                    let gray = ArtboardBackground::Custom([128; 3]);
                    ui_kit::combo(ui, "side_artboard_background", None, &shown, width, |ui| {
                        for b in [
                            ArtboardBackground::White,
                            ArtboardBackground::Black,
                            ArtboardBackground::Transparent,
                            gray,
                        ] {
                            let on =
                                std::mem::discriminant(&b) == std::mem::discriminant(&background);
                            if ui.selectable_label(on, background_label(b)).clicked() && !on {
                                background = b;
                            }
                        }
                    });
                    if let ArtboardBackground::Custom([r, g, b]) = background {
                        let mut c = Color32::from_rgb(r, g, b);
                        let tip = t!("artboard-background-tip");
                        if ui_kit::swatch(ui, &mut c, 28.0, &tip).changed() {
                            background = ArtboardBackground::Custom([c.r(), c.g(), c.b()]);
                        }
                    }
                });
            });
        });
        let moved = (x - r.x, y - r.y);
        if moved != (0, 0) || (w, h) != (r.w, r.h) {
            self.record_change(t!("artboard-hist-resize"), Some((id, "artboard-rect")));
            if let Some(doc) = &mut self.doc {
                doc.move_artboard(i, moved.0, moved.1);
                doc.set_artboard_rect(i, PixelRect { x, y, w, h });
            }
            self.refit_artboards();
        }
        if background != board.background {
            let step = t!("artboard-hist-background");
            self.record_change(step, Some((id, "artboard-background")));
            if let Some(a) = self
                .doc
                .as_mut()
                .and_then(|d| d.layers[i].artboard.as_mut())
            {
                a.background = background;
            }
        }
    }
}
