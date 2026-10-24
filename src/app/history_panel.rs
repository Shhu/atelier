//! History panel (Window › History), like Photoshop's: the document as it
//! opened (the oldest state kept), then each step by name. A click on a
//! step goes back (or forward) to it; the steps after it stay, greyed,
//! until a new action replaces them.

use super::dialogs::{DialogSpec, floating_dialog};
use super::*;
use egui::{Align2, FontId, Response};

/// Row of the list.
const ROW: f32 = 28.0;

#[derive(Default)]
pub(super) struct HistoryPanel {
    pub open: bool,
    /// History position last shown (the current step is scrolled to when
    /// it changes).
    shown: Option<(u64, u64)>,
    /// Active document, its history position and the number of tabs when
    /// the histories were last trimmed (see `trim_histories`).
    pub budget_key: Option<(u64, u64, usize)>,
}

impl AtelierApp {
    pub(super) fn history_window(&mut self, ctx: &egui::Context) {
        if !self.history.open {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let (names, done) = doc.history();
        let position = (doc.uid(), doc.history_step());
        let scroll = self.history.shown != Some(position);
        // The opened state, unless steps were forgotten since.
        let title = match doc.history_forgotten() {
            false => doc.display_name(),
            true => t!("hist3-oldest-state"),
        };
        let mut target = None;
        let pos = dialogs::panel_pos(ctx, "history");
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                resize: Some(ROW * 8.0),
                ..DialogSpec::new("history", t!("xchg-history-title"), pos)
            },
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let rows = std::iter::once((icon::IMAGE, title.as_str()))
                            .chain(names.iter().map(|n| (icon::CARET_RIGHT, *n)));
                        for (state, (glyph, name)) in rows.enumerate() {
                            let row = history_row(ui, glyph, name, state == done, state > done);
                            if state == done && scroll {
                                row.scroll_to_me(Some(egui::Align::Center));
                            }
                            if row.clicked() {
                                target = Some(state);
                            }
                        }
                    });
            },
        );
        self.history.shown = Some(position);
        if out.cancel {
            self.history.open = false;
        }
        if let Some(done) = target {
            self.go_to_history(done);
        }
    }

    /// The state after `done` steps (a click in the History panel), like
    /// as many undos or redos.
    pub(super) fn go_to_history(&mut self, done: usize) {
        let _measured = crate::diag::timer("History: go to step");
        // The steps up to the one clicked, by number: what ends first may
        // add steps, or remove one (an empty text given up).
        let wanted = self.doc.as_ref().map_or_else(Vec::new, |d| {
            let numbers = d.history_numbers();
            numbers[..done.min(numbers.len())].to_vec()
        });
        // A greyed row (undone) clicked, to redo up to it.
        let redo_row = self
            .doc
            .as_ref()
            .filter(|d| done > d.history().1)
            .and(wanted.last().copied());
        // A selection still being computed applies first, to the state it
        // was computed from.
        self.wait_select_job();
        self.finish_stroke();
        // A text being typed is committed first, as by Undo: its own steps
        // are then ordinary ones.
        self.commit_text();
        if self.cancel_select_move() {
            return;
        }
        self.reset_pending();
        self.settle_selection();
        let Some(doc) = &mut self.doc else { return };
        let numbers = doc.history_numbers();
        // What ended first dropped the redo list (a new selection): the
        // row clicked is gone, the state stays the new one.
        if redo_row.is_some_and(|n| !numbers.contains(&n)) {
            return;
        }
        let done = wanted
            .iter()
            .rev()
            .find_map(|n| numbers.iter().position(|m| m == n))
            .map_or(0, |i| i + 1);
        if doc.go_to_step(done) {
            self.reset_pending();
            self.pull_selection();
        }
    }

    // ------------------------------------------------------------ selection

    /// Records the selection as it now is as a history step (like
    /// Photoshop, every selection change is one), named after `name`, or
    /// after what made it: the tool, or Deselect when nothing is left.
    pub(super) fn commit_selection(&mut self, name: Option<String>) {
        let hint = self.sel.step_name.take();
        let Some(doc) = &mut self.doc else { return };
        let name = name.or(hint).unwrap_or_else(|| match &self.selection {
            None => t!("hist3-deselect"),
            Some(_) if self.tool.selects() => self.tool.name(),
            Some(_) => t!("hist3-selection"),
        });
        if doc.record_selection(self.selection.clone(), name) {
            self.prop_session = None;
        }
    }

    /// The selection changed as part of a step already recorded (a crop, a
    /// move of pixels): the history's copy follows, without a step.
    pub(super) fn sync_selection(&mut self) {
        if let Some(doc) = &mut self.doc {
            doc.sync_selection(self.selection.clone());
        }
    }

    /// After an undo, a redo or a step given up: the selection as the
    /// history has it, within the canvas.
    pub(super) fn pull_selection(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        let canvas = doc.full_rect();
        self.selection = doc.selection().cloned().and_then(|s| s.within(canvas));
        doc.sync_selection(self.selection.clone());
    }

    /// End of a frame (and before going through the history): a selection
    /// changed by a gesture that is over, and recorded by nothing, is known
    /// to the history from now on. The gestures (marquee, moving the
    /// outline) record their step when they end.
    pub(super) fn settle_selection(&mut self) {
        let held = self.ctx.input(|i| i.pointer.any_down());
        let gesture = self.marquee.is_some() || self.select_move.is_some();
        if !held && !gesture {
            self.sync_selection();
        }
    }
}

/// A step of the list: its icon and name; the current one highlighted,
/// those undone greyed.
fn history_row(
    ui: &mut egui::Ui,
    glyph: &str,
    name: &str,
    current: bool,
    undone: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let painter = ui.painter();
    if current {
        painter.rect_filled(rect, 6.0, ui_kit::color::ACCENT_SOFT);
    } else if response.hovered() {
        painter.rect_filled(rect, 6.0, ui_kit::color::BG3);
    }
    let color = if undone {
        ui_kit::color::DISABLED
    } else {
        ui_kit::color::TEXT
    };
    painter.text(
        pos2(rect.left() + 16.0, rect.center().y),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(13.0),
        if undone {
            ui_kit::color::DISABLED
        } else {
            ui_kit::color::MUTED
        },
    );
    // Too long, the name ends with "…" and shows whole on hover.
    let elided = ui_kit::elided_text(
        painter,
        pos2(rect.left() + 32.0, rect.center().y),
        name,
        FontId::proportional(ui_kit::TEXT_SIZE),
        color,
        rect.right() - 6.0 - (rect.left() + 32.0),
    );
    let response = if elided {
        response.on_hover_text(name)
    } else {
        response
    };
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, current, name));
    response
}
