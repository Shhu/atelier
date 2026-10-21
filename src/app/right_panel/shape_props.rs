//! Properties of a shape layer, like Photoshop's Live Shape Properties: the
//! live shape's box, corner radii, sides and star ratio, line weight and
//! arrowheads; its appearance (fill, stroke and its options).

use super::*;
use crate::app::options_bar::vector_options::{paint_field, stroke_fields};
use crate::vector::{Bounds, Live, Paint, ShapeKind, pt};

impl AtelierApp {
    /// Sections of the active shape layer: its live shape (if it is one)
    /// and its appearance. An edit renders it again, one undo step per
    /// series of edits.
    pub(super) fn shape_section(&mut self, ui: &mut Ui, locked: bool) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let Some(old) = layer.shape.as_deref().cloned() else {
            return;
        };
        let (id, offset) = (layer.id, layer.offset);
        let mut shape = old.clone();
        let (fg, bg) = (self.primary, self.secondary);
        let (fg, bg) = ([fg.r(), fg.g(), fg.b()], [bg.r(), bg.g(), bg.b()]);
        if let Some(live) = &mut shape.live {
            section(ui, Some(&t!("vector-live-shape")), |ui| {
                ui.add_enabled_ui(!locked, |ui| live_fields(ui, id, live, offset));
            });
        }
        section(ui, Some(&t!("vector-appearance")), |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui_kit::field_label(ui, &t!("vector-fill"));
                    paint_field(
                        ui,
                        "side_shape_fill",
                        &t!("vector-fill-tip"),
                        &mut shape.fill,
                        fg,
                        bg,
                    );
                });
                if let Paint::Gradient(g) = &mut shape.fill {
                    gradient_fill_fields(ui, g);
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui_kit::field_label(ui, &t!("vector-stroke"));
                    let tip = t!("vector-stroke-tip");
                    paint_field(
                        ui,
                        "side_shape_stroke",
                        &tip,
                        &mut shape.stroke.paint,
                        fg,
                        bg,
                    );
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let half = (ui.available_width() - GAP) / 2.0;
                    stroke_fields(ui, "side_stroke_options", &mut shape.stroke, half, half);
                });
                if let Paint::Gradient(g) = &mut shape.stroke.paint {
                    gradient_fill_fields(ui, g);
                }
            });
        });
        if shape == old {
            return;
        }
        if shape.live != old.live {
            shape.rebuild();
        }
        self.record_change(t!("vector-hist-edit-shape"), Some((id, "shape")));
        if let Some(doc) = &mut self.doc {
            let i = doc.active;
            doc.set_shape(i, shape);
        }
    }
}

/// The live shape's settings; its box in canvas coordinates (the layer at
/// `offset`).
fn live_fields(ui: &mut Ui, id: u64, live: &mut Live, offset: (i64, i64)) {
    let half = (ui.available_width() - GAP) / 2.0;
    let o = pt(offset.0 as f32, offset.1 as f32);
    let old = Bounds::of(live.rect.min, live.rect.max);
    let (mut x, mut y) = (old.min.x + o.x, old.min.y + o.y);
    let (mut w, mut h) = (old.width(), old.height());
    let limit = MAX_OFFSET as f32;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui_kit::number_field(ui, "X", &mut x, -limit..=limit, "", half);
        ui_kit::number_field(ui, "Y", &mut y, -limit..=limit, "", half);
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let (w_label, h_label) = (t!("optbar-width-short"), t!("optbar-height-short"));
        ui_kit::number_field(ui, &w_label, &mut w, 0.0..=limit, "", half);
        ui_kit::number_field(ui, &h_label, &mut h, 0.0..=limit, "", half);
    });
    let new = Bounds::of(pt(x, y) - o, pt(x + w, y + h) - o);
    if new != old {
        // The box moved or resized; a line keeps its direction in it.
        let map = |p: crate::vector::Pt| {
            let fx = if old.width() > 0.0 {
                (p.x - old.min.x) / old.width()
            } else {
                0.0
            };
            let fy = if old.height() > 0.0 {
                (p.y - old.min.y) / old.height()
            } else {
                0.0
            };
            pt(new.min.x + fx * new.width(), new.min.y + fy * new.height())
        };
        live.rect = Bounds {
            min: map(live.rect.min),
            max: map(live.rect.max),
        };
    }
    match live.kind {
        ShapeKind::Rectangle => {
            let link_id = Id::new(("shape_radii_linked", id));
            let mut linked = ui.data(|d| d.get_temp::<bool>(link_id)).unwrap_or(true);
            let before = live.radii;
            let field = (ui.available_width() - 2.0 * GAP - 28.0) / 2.0;
            // Radii index: top left, top right, bottom right, bottom left.
            let rows = [
                [
                    (0, icon::ARROW_UP_LEFT, t!("vector-radius-top-left")),
                    (1, icon::ARROW_UP_RIGHT, t!("vector-radius-top-right")),
                ],
                [
                    (3, icon::ARROW_DOWN_LEFT, t!("vector-radius-bottom-left")),
                    (2, icon::ARROW_DOWN_RIGHT, t!("vector-radius-bottom-right")),
                ],
            ];
            for (row, fields) in rows.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for (i, glyph, tip) in fields {
                        ui_kit::number_field(
                            ui,
                            glyph,
                            &mut live.radii[*i],
                            0.0..=limit,
                            "",
                            field,
                        )
                        .on_hover_text(tip);
                    }
                    if row == 0 {
                        ui_kit::icon_toggle(
                            ui,
                            icon::LINK,
                            &t!("vector-radii-linked"),
                            &mut linked,
                        );
                    }
                });
            }
            if linked && let Some(i) = (0..4).find(|&i| live.radii[i] != before[i]) {
                live.radii = [live.radii[i]; 4];
            }
            ui.data_mut(|d| d.insert_temp(link_id, linked));
        }
        ShapeKind::Triangle | ShapeKind::Polygon => {
            if live.kind == ShapeKind::Polygon {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui_kit::number_field(
                        ui,
                        &t!("vector-sides"),
                        &mut live.sides,
                        3..=100,
                        "",
                        half,
                    );
                    let mut star = (live.star * 100.0).round();
                    let unit = i18n::percent("");
                    if ui_kit::number_field(
                        ui,
                        &t!("vector-star-ratio"),
                        &mut star,
                        1.0..=100.0,
                        &unit,
                        half,
                    )
                    .on_hover_text(t!("vector-star-ratio-tip"))
                    .changed()
                    {
                        live.star = star / 100.0;
                    }
                });
            }
            let width = ui.available_width();
            ui_kit::number_field(
                ui,
                icon::CORNERS_OUT,
                &mut live.radii[0],
                0.0..=limit,
                " px",
                width,
            )
            .on_hover_text(t!("vector-corner-radius"));
        }
        ShapeKind::Line => {
            let width = ui.available_width();
            ui_kit::number_field(
                ui,
                &t!("vector-weight"),
                &mut live.weight,
                0.1..=1000.0,
                " px",
                width,
            )
            .on_hover_text(t!("vector-weight-tip"));
            ui.horizontal(|ui| {
                ui_kit::checkbox(ui, &mut live.arrows[0], &t!("vector-arrow-start"));
                ui_kit::checkbox(ui, &mut live.arrows[1], &t!("vector-arrow-end"));
            });
        }
        ShapeKind::Ellipse | ShapeKind::Custom(_) => {}
    }
}
