//! Paths panel (Window › Paths), like Photoshop's: the active shape layer's
//! path, the Work Path and the saved paths; a path loaded as a selection,
//! filled, stroked with the brush; a selection made into a Work Path; and
//! the dialogs of those commands (Alt + their buttons).

use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, gap_between_blocks, panel_pos, slider_row,
};
use super::select_menu::Then;
use super::*;
use crate::document::{PathRef, SavedPath};
use crate::mask_ops::Modify;
use crate::paint::StrokeMode;
use crate::vector::raster::{fill_coverage, pixel_box};
use crate::vector::{Path as VPath, trace};

/// Row of the list.
const ROW: f32 = 40.0;

/// Commands of the Paths panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PathCommand {
    /// Load Path as a Selection (Ctrl+Enter, Ctrl + click on a path).
    MakeSelection,
    /// Fill Path with the foreground color.
    FillPath,
    /// Stroke Path with the brush.
    StrokePath,
    /// Make Work Path from the selection.
    MakeWorkPath,
    /// Create New Path (empty, saved).
    NewPath,
    /// Save Path: the Work Path becomes a saved path.
    SavePath,
    DeletePath,
}

/// Make Selection.
#[derive(Clone, Copy)]
pub(super) struct SelectionSettings {
    feather: f32,
    antialias: bool,
    mode: SelectMode,
}

/// Fill Path.
#[derive(Clone, Copy)]
pub(super) struct FillSettings {
    content: FillContent,
    color: Color32,
    opacity: f32,
    preserve: bool,
    feather: f32,
    antialias: bool,
}

/// Stroke Path: with the Brush or the Eraser, pressure simulated or not.
#[derive(Clone, Copy)]
pub(super) struct StrokeSettings {
    eraser: bool,
    pressure: bool,
}

/// A dialog of the panel, its settings being edited.
#[derive(Clone, Copy)]
pub(super) enum PathDialog {
    Selection(SelectionSettings),
    Fill(FillSettings),
    Stroke(StrokeSettings),
    /// Make Work Path: tolerance (px).
    WorkPath(f32),
}

/// The panel, its dialog, the last settings of each command.
pub(super) struct PathsPanel {
    pub open: bool,
    pub dialog: Option<PathDialog>,
    selection: SelectionSettings,
    fill: FillSettings,
    stroke: StrokeSettings,
    tolerance: f32,
    /// Saved path being renamed, and its new name.
    rename: Option<(usize, String)>,
}

impl Default for PathsPanel {
    fn default() -> Self {
        Self {
            open: false,
            dialog: None,
            selection: SelectionSettings {
                feather: 0.0,
                antialias: true,
                mode: SelectMode::Replace,
            },
            fill: FillSettings {
                content: FillContent::Foreground,
                color: Color32::BLACK,
                opacity: 1.0,
                preserve: false,
                feather: 0.0,
                antialias: true,
            },
            stroke: StrokeSettings {
                eraser: false,
                pressure: false,
            },
            tolerance: 2.0,
            rename: None,
        }
    }
}

/// The selection a path makes over `canvas`: its fill's coverage, feathered
/// by `feather` px, its edges anti-aliased or not.
pub(super) fn path_selection(
    path: &VPath,
    canvas: PixelRect,
    feather: f32,
    antialias: bool,
) -> Option<Selection> {
    let reach = Modify::Feather(feather).reach() as f32;
    let b = path
        .bounds()?
        .expand(if feather > 0.0 { reach } else { 1.0 });
    let area = crate::document::intersect(pixel_box(b), canvas)?;
    let mut mask = fill_coverage(path, area);
    if !antialias {
        for v in mask.iter_mut() {
            *v = if *v >= 128 { 255 } else { 0 };
        }
    }
    if feather > 0.0 {
        crate::mask_ops::modify(&mut mask, Modify::Feather(feather));
    }
    Selection::from_mask(&mask, (area.x, area.y))
}

impl AtelierApp {
    /// The path the commands work on (the selected one, or the active
    /// shape layer's), with a message when there is none.
    fn command_path(&mut self) -> Option<VPath> {
        let path = self.target_path().map(|(_, p)| p).filter(|p| !p.is_empty());
        if path.is_none() {
            self.status = t!("vector-status-no-path");
        }
        path
    }

    /// Runs a command of the Paths panel; with `dialog`, its dialog first
    /// (Alt + its button, like Photoshop).
    pub(super) fn path_command(&mut self, command: PathCommand, dialog: bool) {
        if self.doc.is_none() {
            return;
        }
        let panel = &mut self.vector.panel;
        if dialog {
            panel.dialog = match command {
                PathCommand::MakeSelection => Some(PathDialog::Selection(panel.selection)),
                PathCommand::FillPath => Some(PathDialog::Fill(panel.fill)),
                PathCommand::StrokePath => Some(PathDialog::Stroke(panel.stroke)),
                PathCommand::MakeWorkPath if self.selection.is_some() => {
                    Some(PathDialog::WorkPath(panel.tolerance))
                }
                _ => None,
            };
            if panel.dialog.is_some() {
                return;
            }
        }
        match command {
            PathCommand::MakeSelection => {
                let s = self.vector.panel.selection;
                self.make_selection(s);
            }
            PathCommand::FillPath => {
                let s = self.vector.panel.fill;
                self.fill_path(s);
            }
            PathCommand::StrokePath => {
                let s = self.vector.panel.stroke;
                self.stroke_path(s);
            }
            PathCommand::MakeWorkPath => {
                let t = self.vector.panel.tolerance;
                self.make_work_path(t);
            }
            PathCommand::NewPath => {
                let Some(doc) = &mut self.doc else { return };
                let name = next_path_name(doc);
                doc.checkpoint(t!("vector-hist-new-path"));
                doc.paths.push(SavedPath {
                    name,
                    path: VPath::default(),
                });
                self.vector.selected = Some(PathRef::Saved(doc.paths.len() - 1));
            }
            PathCommand::SavePath => {
                let Some(doc) = &mut self.doc else { return };
                let Some(path) = doc.work_path.clone() else {
                    return;
                };
                let name = next_path_name(doc);
                doc.checkpoint(t!("vector-hist-save-path"));
                doc.paths.push(SavedPath { name, path });
                doc.work_path = None;
                self.vector.selected = Some(PathRef::Saved(doc.paths.len() - 1));
            }
            PathCommand::DeletePath => {
                let Some(doc) = &mut self.doc else { return };
                match self.vector.selected {
                    Some(PathRef::Work) if doc.work_path.is_some() => {
                        doc.checkpoint(t!("vector-hist-delete-path"));
                        doc.work_path = None;
                    }
                    Some(PathRef::Saved(i)) if i < doc.paths.len() => {
                        doc.checkpoint(t!("vector-hist-delete-path"));
                        doc.paths.remove(i);
                    }
                    _ => return,
                }
                self.vector.selected = None;
                self.vector.clear_selection();
            }
        }
    }

    /// Load Path as a Selection, combined with the current one by `s.mode`.
    fn make_selection(&mut self, s: SelectionSettings) {
        let Some(path) = self.command_path() else {
            return;
        };
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let pixels = path
            .bounds()
            .map_or(0, |b| (b.width() * b.height()).max(0.0) as u64);
        let mode = if self.selection.is_some() {
            s.mode
        } else {
            SelectMode::Replace
        };
        self.sel.step_name = Some(t!("hist3-make-selection"));
        self.select_later(Then::Select(mode), pixels, move || {
            path_selection(&path, canvas, s.feather, s.antialias)
        });
    }

    /// Fill Path: the path's area of the active layer filled.
    fn fill_path(&mut self, s: FillSettings) {
        if self.refuse_if_target_locked() {
            return;
        }
        let Some(path) = self.command_path() else {
            return;
        };
        let Some(doc) = &self.doc else { return };
        let Some(sel) = path_selection(&path, doc.full_rect(), s.feather, s.antialias) else {
            return;
        };
        let color = match s.content {
            FillContent::Foreground | FillContent::ContentAware => self.primary,
            FillContent::Background => self.secondary,
            FillContent::Color => s.color,
            FillContent::Black => Color32::BLACK,
            FillContent::Gray => Color32::from_gray(128),
            FillContent::White => Color32::WHITE,
        };
        let Some(doc) = &mut self.doc else { return };
        let rgba = color.to_srgba_unmultiplied();
        if !doc.fill_active(rgba, Some(&sel), s.opacity, s.preserve) {
            self.status = self.nothing_to_paint();
        }
    }

    /// Stroke Path: the Brush (or the Eraser) along each component, with its
    /// current settings; Simulate Pressure tapers the ends.
    fn stroke_path(&mut self, s: StrokeSettings) {
        if self.refuse_if_target_locked() {
            return;
        }
        let Some(path) = self.command_path() else {
            return;
        };
        let p = &self.paint;
        let mut settings = BrushSettings::round(
            self.primary.to_srgba_unmultiplied(),
            self.brush_size,
            self.hardness,
            self.opacity,
        );
        (settings.tip, settings.spacing) = (p.tip.clone(), p.spacing);
        (settings.angle, settings.roundness, settings.flow) = (p.angle, p.roundness, p.flow);
        settings.pressure_size = s.pressure;
        let mode = if s.eraser {
            StrokeMode::Erase
        } else {
            StrokeMode::Paint
        };
        let Some(doc) = &mut self.doc else { return };
        doc.checkpoint(t!("vector-hist-stroke-path"));
        if !doc.begin_paint() {
            doc.discard_step();
            self.status = self.nothing_to_paint();
            return;
        }
        let mut painted = false;
        for sub in path.subpaths.iter().filter(|s| s.anchors.len() >= 2) {
            let mut points = sub.flatten(0.25);
            if sub.closed {
                points.push(points[0]);
            }
            let total: f32 = points.windows(2).map(|w| w[0].dist(w[1])).sum();
            let layer = doc.active_layer_mut();
            let (ox, oy) = (layer.offset.0 as f32, layer.offset.1 as f32);
            let mut stroke = Stroke::begin(layer.shared_image(), mode, settings.clone());
            let image = layer.pixels_mut();
            let mut along = 0.0;
            for (i, q) in points.iter().enumerate() {
                if i > 0 {
                    along += points[i - 1].dist(*q);
                }
                let t = if total > 0.0 { along / total } else { 0.5 };
                let pressure = (t * std::f32::consts::PI).sin().max(0.05);
                stroke.add_point(image, q.x - ox, q.y - oy, pressure);
            }
            painted |= stroke.painted();
        }
        doc.sync_paint(None);
        doc.end_paint();
        if !painted {
            doc.discard_step();
        }
    }

    /// Make Work Path: the selection's outline, within `tolerance` px.
    fn make_work_path(&mut self, tolerance: f32) {
        let (Some(sel), Some(doc)) = (self.selection.clone(), &mut self.doc) else {
            self.status = t!("vector-status-no-selection");
            return;
        };
        let canvas = doc.full_rect();
        let Some(area) = sel.bounds(canvas) else {
            return;
        };
        let path = trace::trace(&sel.to_mask(area), (area.x, area.y), tolerance);
        if path.is_empty() {
            self.status = t!("status-selection-empty");
            return;
        }
        doc.checkpoint(t!("vector-hist-make-work-path"));
        doc.work_path = Some(path);
        self.vector.selected = Some(PathRef::Work);
        self.vector.clear_selection();
        self.deselect();
        // In the Make Work Path step.
        self.sync_selection();
    }

    /// The Paths panel and its dialog.
    pub(super) fn vector_windows(&mut self, ctx: &egui::Context) {
        self.path_dialog_window(ctx);
        if !self.vector.panel.open {
            return;
        }
        let Some(doc) = &self.doc else {
            return;
        };
        // The active shape layer's path, the Work Path, the saved paths.
        let mut rows: Vec<(PathRef, String, VPath)> = Vec::new();
        let layer = doc.active_layer();
        if let Some(p) = doc.path(PathRef::Layer(layer.id)) {
            let name = t!("vector-shape-path-name", name = layer.name.as_str());
            rows.push((PathRef::Layer(layer.id), name, p));
        }
        if let Some(p) = &doc.work_path {
            rows.push((PathRef::Work, t!("vector-work-path"), p.clone()));
        }
        for (i, p) in doc.paths.iter().enumerate() {
            rows.push((PathRef::Saved(i), p.name.clone(), p.path.clone()));
        }
        let current = self.path_target();
        let has_selection = self.selection.is_some();
        let mut clicked: Option<(PathRef, Modifiers, bool)> = None;
        let mut deselect = false;
        let mut command: Option<(PathCommand, bool)> = None;
        let mut renamed: Option<(usize, String)> = None;
        let rename = &mut self.vector.panel.rename;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                ..DialogSpec::new("paths", t!("vector-paths-title"), panel_pos(ctx, "paths"))
            },
            |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if rows.is_empty() {
                    ui_kit::hint(ui, t!("vector-paths-empty"));
                }
                for (r, name, path) in &rows {
                    let editing = match (r, rename.as_mut()) {
                        (PathRef::Saved(i), Some((j, text))) if i == j => Some(text),
                        _ => None,
                    };
                    if let Some(text) = editing {
                        let id = egui::Id::new("path_rename");
                        let edit = ui_kit::text_field(ui, &t!("vector-path-name-label"), text, id);
                        edit.response.request_focus();
                        if edit.response.lost_focus() {
                            renamed = rename.take();
                        }
                        continue;
                    }
                    let response = path_row(ui, name, path, Some(*r) == current, *r);
                    if response.double_clicked() {
                        match r {
                            PathRef::Work => command = Some((PathCommand::SavePath, false)),
                            PathRef::Saved(i) => *rename = Some((*i, name.clone())),
                            PathRef::Layer(_) => {}
                        }
                    } else if response.clicked() {
                        clicked = Some((*r, ui.input(|i| i.modifiers), false));
                    }
                }
                // A click below the paths: none selected.
                let size = vec2(ui.available_width(), 16.0);
                let (_, below) = ui.allocate_exact_size(size, Sense::click());
                if below.clicked() {
                    deselect = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let any = current.is_some();
                    let deletable = matches!(current, Some(PathRef::Work | PathRef::Saved(_)));
                    let alt = keys::modifiers(ui.ctx(), Modifiers::ALT);
                    let buttons = [
                        (
                            icon::PAINT_BUCKET,
                            t!("input3-fill-path-tip", alt = alt.clone()),
                            PathCommand::FillPath,
                            any,
                        ),
                        (
                            icon::PAINT_BRUSH,
                            t!("input3-stroke-path-tip", alt = alt.clone()),
                            PathCommand::StrokePath,
                            any,
                        ),
                        (
                            icon::SELECTION,
                            t!("input3-make-selection-tip", alt = alt.clone()),
                            PathCommand::MakeSelection,
                            any,
                        ),
                        (
                            icon::BEZIER_CURVE,
                            t!("input3-make-work-path-tip", alt = alt.clone()),
                            PathCommand::MakeWorkPath,
                            has_selection,
                        ),
                        (
                            icon::PLUS,
                            t!("vector-new-path-tip"),
                            PathCommand::NewPath,
                            true,
                        ),
                        (
                            icon::TRASH,
                            t!("vector-delete-path-tip"),
                            PathCommand::DeletePath,
                            deletable,
                        ),
                    ];
                    for (glyph, tip, cmd, enabled) in buttons {
                        if ui_kit::icon_button(ui, glyph, &tip, enabled).clicked() {
                            command = Some((cmd, ui.input(|i| i.modifiers.alt)));
                        }
                    }
                });
            },
        );
        if out.cancel {
            self.vector.panel.open = false;
        }
        if let Some((i, name)) = renamed
            && let Some(doc) = &mut self.doc
            && doc
                .paths
                .get(i)
                .is_some_and(|p| p.name != name && !name.trim().is_empty())
        {
            doc.checkpoint(t!("vector-hist-rename-path"));
            doc.paths[i].name = name.trim().to_owned();
        }
        if deselect {
            self.vector.selected = None;
            self.vector.clear_selection();
        }
        if let Some((r, m, _)) = clicked {
            if m.command {
                // Ctrl + click: loaded as a selection (Shift adds, Alt
                // subtracts, both intersect).
                if let Some(path) = self.doc.as_ref().and_then(|d| d.path(r)) {
                    let canvas = self
                        .doc
                        .as_ref()
                        .map(Document::full_rect)
                        .expect("document");
                    let mode = self.selection_mode(m.shift, m.alt);
                    let pixels = path.bounds().map_or(0, |b| (b.width() * b.height()) as u64);
                    self.sel.step_name = Some(t!("hist3-make-selection"));
                    self.select_later(Then::Select(mode), pixels, move || {
                        path_selection(&path, canvas, 0.0, true)
                    });
                }
            } else {
                self.vector.selected = (!matches!(r, PathRef::Layer(_))).then_some(r);
                self.vector.clear_selection();
            }
        }
        if let Some((cmd, dialog)) = command {
            self.path_command(cmd, dialog);
        }
    }

    /// The dialog of a command (Alt + its button).
    fn path_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.vector.panel.dialog else {
            return;
        };
        let title = match d {
            PathDialog::Selection(_) => t!("vector-make-selection-title"),
            PathDialog::Fill(_) => t!("vector-fill-path-title"),
            PathDialog::Stroke(_) => t!("vector-stroke-path-title"),
            PathDialog::WorkPath(_) => t!("vector-make-work-path-title"),
        };
        let center = ctx.content_rect().center();
        let has_selection = self.selection.is_some();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("path_dialog", title, center)
            },
            |ui| {
                let width = ui.available_width();
                let feather_field = |ui: &mut egui::Ui, feather: &mut f32, antialias: &mut bool| {
                    block(ui, |ui| {
                        ui_kit::field_label(ui, &t!("vector-rendering"));
                        let label = t!("sel-feather-radius");
                        ui_kit::number_field(ui, &label, feather, 0.0..=1000.0, " px", width);
                        ui_kit::checkbox(ui, antialias, &t!("sel-anti-alias"));
                    });
                };
                match &mut d {
                    PathDialog::Selection(s) => {
                        feather_field(ui, &mut s.feather, &mut s.antialias);
                        gap_between_blocks(ui);
                        block(ui, |ui| {
                            ui_kit::field_label(ui, &t!("vector-operation"));
                            let modes = [
                                (SelectMode::Replace, t!("vector-op-new-selection")),
                                (SelectMode::Add, t!("vector-op-add-selection")),
                                (SelectMode::Subtract, t!("vector-op-subtract-selection")),
                                (SelectMode::Intersect, t!("vector-op-intersect-selection")),
                            ];
                            for (mode, label) in modes {
                                let enabled = has_selection || mode == SelectMode::Replace;
                                ui.add_enabled_ui(enabled, |ui| {
                                    ui.radio_value(&mut s.mode, mode, label);
                                });
                            }
                        });
                    }
                    PathDialog::Fill(f) => {
                        block(ui, |ui| {
                            ui_kit::field_label(ui, &t!("dialog-fill-contents"));
                            let shown = f.content.label();
                            ui_kit::combo(ui, "path_fill_content", None, &shown, width, |ui| {
                                let contents = FillContent::ALL
                                    .into_iter()
                                    .filter(|&c| c != FillContent::ContentAware);
                                for c in contents {
                                    ui.selectable_value(&mut f.content, c, c.label());
                                }
                            });
                            if f.content == FillContent::Color {
                                ui_kit::swatch(
                                    ui,
                                    &mut f.color,
                                    28.0,
                                    &t!("dialog-fill-color-tip"),
                                );
                            }
                        });
                        gap_between_blocks(ui);
                        let mut opacity = f.opacity * 100.0;
                        let label = t!("dialog-fill-opacity");
                        let unit = i18n::percent("");
                        slider_row(
                            ui,
                            &label,
                            None,
                            &mut opacity,
                            0.0..=100.0,
                            &unit,
                            Track::Accent,
                            100.0,
                        );
                        f.opacity = opacity / 100.0;
                        ui_kit::checkbox(
                            ui,
                            &mut f.preserve,
                            &t!("dialog-fill-preserve-transparency"),
                        );
                        gap_between_blocks(ui);
                        feather_field(ui, &mut f.feather, &mut f.antialias);
                    }
                    PathDialog::Stroke(s) => {
                        block(ui, |ui| {
                            ui_kit::field_label(ui, &t!("vector-stroke-tool"));
                            let shown = if s.eraser {
                                t!("tool-eraser")
                            } else {
                                t!("tool-brush")
                            };
                            ui_kit::combo(ui, "path_stroke_tool", None, &shown, width, |ui| {
                                ui.selectable_value(&mut s.eraser, false, t!("tool-brush"));
                                ui.selectable_value(&mut s.eraser, true, t!("tool-eraser"));
                            });
                            ui_kit::checkbox(ui, &mut s.pressure, &t!("vector-simulate-pressure"))
                                .on_hover_text(t!("vector-simulate-pressure-tip"));
                        });
                    }
                    PathDialog::WorkPath(tolerance) => {
                        let label = t!("vector-tolerance");
                        ui_kit::number_field(ui, &label, tolerance, 0.5..=10.0, " px", width)
                            .on_hover_text(t!("vector-tolerance-tip"));
                    }
                }
            },
        );
        self.vector.panel.dialog = (!out.ok && !out.cancel).then_some(d);
        if !out.ok {
            return;
        }
        let panel = &mut self.vector.panel;
        let command = match d {
            PathDialog::Selection(s) => {
                panel.selection = s;
                PathCommand::MakeSelection
            }
            PathDialog::Fill(f) => {
                panel.fill = f;
                PathCommand::FillPath
            }
            PathDialog::Stroke(s) => {
                panel.stroke = s;
                PathCommand::StrokePath
            }
            PathDialog::WorkPath(t) => {
                panel.tolerance = t.clamp(0.5, 10.0);
                PathCommand::MakeWorkPath
            }
        };
        self.path_command(command, false);
    }
}

/// "Path N" for the first number no path has.
fn next_path_name(doc: &Document) -> String {
    let name = |n: usize| t!("vector-path-name", number = n);
    (1..=doc.paths.len() + 1)
        .map(name)
        .find(|n| doc.paths.iter().all(|p| p.name != *n))
        .unwrap_or_else(|| name(1))
}

/// A row of the list: the path drawn small, its name (the Work Path and a
/// shape's path in italics, as in Photoshop).
fn path_row(
    ui: &mut egui::Ui,
    name: &str,
    path: &VPath,
    selected: bool,
    r: PathRef,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 6.0, ui_kit::color::ACCENT_SOFT);
    } else if response.hovered() {
        painter.rect_filled(rect, 6.0, ui_kit::color::BG3);
    }
    let thumb = Rect::from_min_size(rect.min + vec2(6.0, 5.0), vec2(40.0, 30.0));
    painter.rect_filled(thumb, 3.0, ui_kit::color::THUMB_EMPTY);
    if let Some(b) = path
        .bounds()
        .filter(|b| b.width() > 0.0 || b.height() > 0.0)
    {
        let k = ((thumb.width() - 6.0) / b.width().max(1e-3))
            .min((thumb.height() - 6.0) / b.height().max(1e-3));
        let c = b.center();
        let place = |q: crate::vector::Pt| thumb.center() + vec2((q.x - c.x) * k, (q.y - c.y) * k);
        let line = UiStroke::new(1.0, ui_kit::color::TEXT);
        for s in &path.subpaths {
            let points: Vec<Pos2> = s.flatten(0.5 / k).into_iter().map(place).collect();
            if s.closed {
                painter.add(egui::Shape::closed_line(points, line));
            } else {
                painter.add(egui::Shape::line(points, line));
            }
        }
    }
    let mut job = egui::text::LayoutJob::default();
    job.append(
        name,
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(ui_kit::TEXT_SIZE),
            color: ui_kit::color::TEXT,
            italics: !matches!(r, PathRef::Saved(_)),
            ..Default::default()
        },
    );
    // A long name ends with "…" at the row's end, whole on hover.
    let x = thumb.right() + 10.0;
    job.wrap = egui::text::TextWrapping::truncate_at_width((rect.right() - 8.0 - x).max(0.0));
    let galley = painter.layout_job(job);
    let elided = galley.elided;
    painter.galley(
        pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        ui_kit::color::TEXT,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    if elided {
        response.on_hover_text(name)
    } else {
        response
    }
}
