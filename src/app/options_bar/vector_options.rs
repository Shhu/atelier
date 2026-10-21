//! Options of the Pen, path selection and shape tools: tool mode (Shape or
//! Path), fill and stroke (of the new shapes, or of the active shape layer),
//! path operations, and each shape's settings.

use super::*;
use crate::app::gradient_editor::paint_ramp;
use crate::app::vector_tools::{PenTool, ShapeTool, ToolMode};
use crate::compose::gradient::GradientFill;
use crate::paint::gradient::Preset;
use crate::vector::{Align, Cap, Custom, Join, Paint, PathOp, ShapeStroke};

pub(in crate::app) fn op_label(op: Option<PathOp>) -> String {
    match op {
        None => t!("vector-op-new-layer"),
        Some(PathOp::Combine) => t!("vector-op-combine"),
        Some(PathOp::Subtract) => t!("vector-op-subtract"),
        Some(PathOp::Intersect) => t!("vector-op-intersect"),
        Some(PathOp::Exclude) => t!("vector-op-exclude"),
    }
}

fn op_icon(op: Option<PathOp>) -> &'static str {
    match op {
        None => icon::SQUARE,
        Some(PathOp::Combine) => icon::UNITE_SQUARE,
        Some(PathOp::Subtract) => icon::SUBTRACT_SQUARE,
        Some(PathOp::Intersect) => icon::INTERSECT_SQUARE,
        Some(PathOp::Exclude) => icon::EXCLUDE_SQUARE,
    }
}

pub(in crate::app) fn custom_name(c: Custom) -> String {
    match c {
        Custom::Arrow => t!("vector-custom-arrow"),
        Custom::Heart => t!("vector-custom-heart"),
        Custom::Star => t!("vector-custom-star"),
        Custom::Check => t!("vector-custom-check"),
        Custom::Bubble => t!("vector-custom-bubble"),
        Custom::Bolt => t!("vector-custom-bolt"),
    }
}

/// A custom shape drawn in `rect`.
pub(in crate::app) fn paint_custom(painter: &Painter, rect: Rect, c: Custom, color: Color32) {
    use crate::vector::{Bounds, Live, ShapeKind, pt};
    let live = Live::new(
        ShapeKind::Custom(c),
        Bounds::of(pt(rect.left(), rect.top()), pt(rect.right(), rect.bottom())),
    );
    for s in &live.path().subpaths {
        let points: Vec<Pos2> = s.flatten(0.3).into_iter().map(|q| pos2(q.x, q.y)).collect();
        painter.add(egui::Shape::closed_line(points, UiStroke::new(1.5, color)));
    }
}

/// Fill or stroke paint: None, a color, a gradient (`fg`, `bg`: the
/// foreground and background colors, for a new gradient). Returns whether
/// it changed.
pub(in crate::app) fn paint_field(
    ui: &mut egui::Ui,
    id: &str,
    tip: &str,
    paint: &mut Paint,
    fg: [u8; 3],
    bg: [u8; 3],
) -> bool {
    let before = paint.clone();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let painter = ui.painter();
    let border = if response.hovered() {
        color::MUTED
    } else {
        color::SWATCH_LINE
    };
    match paint {
        // None: white, struck out in red.
        Paint::None => {
            painter.rect_filled(rect, 4.0, Color32::WHITE);
            painter.line_segment(
                [rect.left_bottom(), rect.right_top()],
                UiStroke::new(2.0, Color32::from_rgb(220, 40, 40)),
            );
        }
        Paint::Color([r, g, b]) => {
            painter.rect_filled(rect, 4.0, Color32::from_rgb(*r, *g, *b));
        }
        Paint::Gradient(g) => paint_ramp(painter, rect, &g.gradient, fg, bg, true),
    }
    painter.rect_stroke(rect, 4.0, UiStroke::new(1.0, border), StrokeKind::Inside);
    let response = response.on_hover_text(tip);
    egui::Popup::menu(&response)
        .id(egui::Id::new(id))
        .show(|ui| {
            ui.set_min_width(180.0);
            if ui
                .selectable_label(paint.is_none(), t!("vector-paint-none"))
                .clicked()
            {
                *paint = Paint::None;
            }
            let color = match paint {
                Paint::Color(c) => *c,
                _ => fg,
            };
            if ui
                .selectable_label(matches!(paint, Paint::Color(_)), t!("vector-paint-color"))
                .clicked()
            {
                *paint = Paint::Color(color);
            }
            if ui
                .selectable_label(
                    matches!(paint, Paint::Gradient(_)),
                    t!("vector-paint-gradient"),
                )
                .clicked()
                && !matches!(paint, Paint::Gradient(_))
            {
                *paint = Paint::Gradient(GradientFill {
                    gradient: Preset::ForegroundToBackground.gradient().resolved(fg, bg),
                    ..Default::default()
                });
            }
            if let Paint::Color(c) = paint {
                ui.separator();
                let mut c32 = Color32::from_rgb(c[0], c[1], c[2]);
                if ui_kit::swatch(ui, &mut c32, 28.0, tip).changed() {
                    *c = [c32.r(), c32.g(), c32.b()];
                }
            }
        });
    *paint != before
}

/// Stroke width (a field `width` wide), then its options (alignment, caps,
/// corners, dashes) in a drop-down list `options` wide.
pub(in crate::app) fn stroke_fields(
    ui: &mut egui::Ui,
    id: &str,
    s: &mut ShapeStroke,
    width: f32,
    options: f32,
) {
    ui_kit::number_field(
        ui,
        icon::LINE_SEGMENT,
        &mut s.width,
        0.0..=1000.0,
        " px",
        width,
    )
    .on_hover_text(t!("vector-stroke-width"));
    let shown = match s.align {
        Align::Inside => t!("vector-align-inside"),
        Align::Center => t!("vector-align-center"),
        Align::Outside => t!("vector-align-outside"),
    };
    ui_kit::combo(ui, id, None, &shown, options, |ui| {
        ui.set_min_width(220.0);
        ui_kit::field_label(ui, &t!("vector-stroke-align"));
        ui_kit::segmented(
            ui,
            &mut s.align,
            &[
                (Align::Inside, &t!("vector-align-inside"), ""),
                (Align::Center, &t!("vector-align-center"), ""),
                (Align::Outside, &t!("vector-align-outside"), ""),
            ],
        );
        ui_kit::field_label(ui, &t!("vector-stroke-caps"));
        ui_kit::segmented(
            ui,
            &mut s.cap,
            &[
                (Cap::Butt, &t!("vector-cap-butt"), ""),
                (Cap::Round, &t!("vector-cap-round"), ""),
                (Cap::Square, &t!("vector-cap-square"), ""),
            ],
        );
        ui_kit::field_label(ui, &t!("vector-stroke-corners"));
        ui_kit::segmented(
            ui,
            &mut s.join,
            &[
                (Join::Miter, &t!("vector-join-miter"), ""),
                (Join::Round, &t!("vector-join-round"), ""),
                (Join::Bevel, &t!("vector-join-bevel"), ""),
            ],
        );
        let mut dashed = s.dash.is_some();
        if ui_kit::checkbox(ui, &mut dashed, &t!("vector-dashed-line")).changed() {
            s.dash = dashed.then_some([4.0, 2.0]);
        }
        if let Some([dash, gap]) = &mut s.dash {
            ui.horizontal(|ui| {
                ui_kit::number_field(ui, &t!("vector-dash"), dash, 0.0..=100.0, "", 100.0);
                ui_kit::number_field(ui, &t!("vector-gap"), gap, 0.0..=100.0, "", 100.0);
            });
        }
    })
    .response
    .on_hover_text(t!("vector-stroke-options"));
}

impl AtelierApp {
    /// Options of the vector tools.
    pub(super) fn vector_options(&mut self, ui: &mut egui::Ui) {
        let shape_tool = match self.tool {
            Tool::Shape(s) => Some(s),
            _ => None,
        };
        let pen = match self.tool {
            Tool::Pen(p) => Some(p),
            _ => None,
        };
        let draws = shape_tool.is_some() || matches!(pen, Some(PenTool::Pen | PenTool::Freeform));
        if draws {
            let mode = if shape_tool.is_some() {
                &mut self.vector.shape_mode
            } else {
                &mut self.vector.pen_mode
            };
            let shown = match mode {
                ToolMode::Shape => t!("vector-mode-shape"),
                ToolMode::Path => t!("vector-mode-path"),
            };
            dropdown(ui, "vector_mode", None, &shown, false, 0.0, |ui| {
                ui.selectable_value(mode, ToolMode::Shape, t!("vector-mode-shape"));
                ui.selectable_value(mode, ToolMode::Path, t!("vector-mode-path"));
            })
            .response
            .on_hover_text(t!("vector-mode-tip"));
            sep(ui);
        }
        let shape_mode = draws
            && (if shape_tool.is_some() {
                self.vector.shape_mode
            } else {
                self.vector.pen_mode
            }) == ToolMode::Shape;
        let active_shape = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().shape.as_deref().cloned());
        // Like Photoshop: with a shape layer active, its fill and stroke.
        let selects = matches!(self.tool, Tool::PathSelect | Tool::DirectSelect);
        if shape_mode || (active_shape.is_some() && selects) {
            self.paint_options(ui, active_shape);
            sep(ui);
        }
        if draws {
            let ops: Vec<Option<PathOp>> = (shape_mode.then_some(None).into_iter())
                .chain(PathOp::ALL.map(Some))
                .collect();
            let mut op = if !shape_mode && self.vector.op.is_none() {
                Some(PathOp::Combine)
            } else {
                self.vector.op
            };
            let labels: Vec<String> = ops.iter().map(|&o| op_label(o)).collect();
            let options: Vec<(Option<PathOp>, &str, &str)> = ops
                .iter()
                .zip(&labels)
                .map(|(&o, l)| (o, op_icon(o), l.as_str()))
                .collect();
            if ui_kit::segmented(ui, &mut op, &options) {
                self.vector.op = op;
            }
            sep(ui);
        }
        if matches!(self.tool, Tool::PathSelect | Tool::DirectSelect) {
            self.selected_components_op(ui);
        }
        let v = &mut self.vector;
        match shape_tool {
            Some(ShapeTool::Rectangle | ShapeTool::Triangle) => {
                ui_kit::number_field(
                    ui,
                    icon::CORNERS_OUT,
                    &mut v.radius,
                    0.0..=10000.0,
                    " px",
                    96.0,
                )
                .on_hover_text(t!("vector-corner-radius"));
            }
            Some(ShapeTool::Polygon) => {
                ui_kit::number_field(ui, &t!("vector-sides"), &mut v.sides, 3..=100, "", 96.0);
                let mut star = (v.star * 100.0).round();
                let unit = i18n::percent("");
                if ui_kit::number_field(
                    ui,
                    &t!("vector-star-ratio"),
                    &mut star,
                    1.0..=100.0,
                    &unit,
                    150.0,
                )
                .on_hover_text(t!("vector-star-ratio-tip"))
                .changed()
                {
                    v.star = star / 100.0;
                }
                ui_kit::number_field(
                    ui,
                    icon::CORNERS_OUT,
                    &mut v.radius,
                    0.0..=10000.0,
                    " px",
                    96.0,
                )
                .on_hover_text(t!("vector-corner-radius"));
            }
            Some(ShapeTool::Line) => {
                ui_kit::number_field(
                    ui,
                    &t!("vector-weight"),
                    &mut v.weight,
                    0.1..=1000.0,
                    " px",
                    120.0,
                )
                .on_hover_text(t!("vector-weight-tip"));
                ui_kit::checkbox(ui, &mut v.arrows[0], &t!("vector-arrow-start"));
                ui_kit::checkbox(ui, &mut v.arrows[1], &t!("vector-arrow-end"));
            }
            Some(ShapeTool::Custom) => {
                let shown = custom_name(v.custom);
                dropdown(
                    ui,
                    "custom_shape",
                    Some(&t!("vector-custom-label")),
                    &shown,
                    false,
                    0.0,
                    |ui| {
                        ui.set_min_width(200.0);
                        for c in Custom::ALL {
                            let (r, response) =
                                ui.allocate_exact_size(vec2(200.0, 30.0), Sense::click());
                            if response.hovered() || v.custom == c {
                                ui.painter().rect_filled(r, 6.0, color::BG3);
                            }
                            let icon_rect =
                                Rect::from_min_size(r.min + vec2(6.0, 5.0), vec2(20.0, 20.0));
                            paint_custom(ui.painter(), icon_rect, c, color::TEXT);
                            ui.painter().text(
                                pos2(icon_rect.right() + 10.0, r.center().y),
                                egui::Align2::LEFT_CENTER,
                                custom_name(c),
                                FontId::proportional(ui_kit::TEXT_SIZE),
                                color::TEXT,
                            );
                            if response.clicked() {
                                v.custom = c;
                                ui.close();
                            }
                        }
                    },
                );
            }
            Some(ShapeTool::Ellipse) | None => {}
        }
        if pen == Some(PenTool::Pen) {
            ui_kit::checkbox(ui, &mut v.auto_add, &t!("vector-auto-add-delete"))
                .on_hover_text(t!("vector-auto-add-delete-tip"));
        }
        let hint = match self.tool {
            Tool::Pen(PenTool::Pen) => t!("vector-pen-hint"),
            Tool::Pen(PenTool::Freeform) => t!("vector-freeform-hint"),
            Tool::Pen(_) => t!("vector-anchor-tools-hint"),
            Tool::PathSelect => t!("vector-path-select-hint"),
            Tool::DirectSelect => t!("vector-direct-select-hint"),
            _ => t!("vector-shape-hint"),
        };
        trailing_hint(ui, &hint);
    }

    /// Fill, stroke and its width and options: of `active` (the active
    /// shape layer) if given, and of the next shapes.
    fn paint_options(&mut self, ui: &mut egui::Ui, active: Option<crate::vector::Shape>) {
        let (fg, bg) = (self.primary, self.secondary);
        let (fg, bg) = ([fg.r(), fg.g(), fg.b()], [bg.r(), bg.g(), bg.b()]);
        let (mut fill, mut stroke) = match &active {
            Some(s) => (s.fill.clone(), s.stroke.clone()),
            None => (
                self.vector.fill.clone().unwrap_or(Paint::Color(fg)),
                self.vector.stroke.clone(),
            ),
        };
        ui.label(egui::RichText::new(t!("vector-fill")).color(color::LABEL));
        let fill_changed =
            paint_field(ui, "vector_fill", &t!("vector-fill-tip"), &mut fill, fg, bg);
        ui.label(egui::RichText::new(t!("vector-stroke")).color(color::LABEL));
        paint_field(
            ui,
            "vector_stroke",
            &t!("vector-stroke-tip"),
            &mut stroke.paint,
            fg,
            bg,
        );
        stroke_fields(ui, "vector_stroke_options", &mut stroke, 92.0, 110.0);
        match active {
            Some(old) => {
                if (fill != old.fill || stroke != old.stroke) && !self.refuse_if_locked() {
                    let Some(doc) = &self.doc else { return };
                    let id = doc.active_layer().id;
                    self.record_change(t!("vector-hist-edit-shape"), Some((id, "shape-paint")));
                    (self.vector.fill, self.vector.stroke) = (Some(fill.clone()), stroke.clone());
                    let shape = crate::vector::Shape {
                        fill,
                        stroke,
                        ..old
                    };
                    if let Some(doc) = &mut self.doc {
                        let i = doc.active;
                        doc.set_shape(i, shape);
                    }
                }
            }
            None => {
                if fill_changed {
                    self.vector.fill = Some(fill);
                }
                self.vector.stroke = stroke;
            }
        }
    }

    /// Path operation of the selected components (Path Selection).
    fn selected_components_op(&mut self, ui: &mut egui::Ui) {
        let Some((r, mut path)) = self.target_path() else {
            return;
        };
        let selected = self.vector.components.clone();
        let Some(&first) = selected.first() else {
            return;
        };
        let mut op = Some(path.subpaths[first].op);
        let labels: Vec<String> = PathOp::ALL.map(|o| op_label(Some(o))).to_vec();
        let options: Vec<(Option<PathOp>, &str, &str)> = PathOp::ALL
            .iter()
            .zip(&labels)
            .map(|(&o, l)| (Some(o), op_icon(Some(o)), l.as_str()))
            .collect();
        if ui_kit::segmented(ui, &mut op, &options)
            && let Some(op) = op
            && !(matches!(r, crate::document::PathRef::Layer(_)) && self.refuse_if_locked())
        {
            for &k in &selected {
                path.subpaths[k].op = op;
            }
            if let Some(doc) = &mut self.doc {
                doc.checkpoint(t!("vector-hist-path-operation"));
                doc.set_path(r, path);
            }
        }
        sep(ui);
    }
}
