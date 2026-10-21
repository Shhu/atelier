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
        let title = doc.display_name();
        let mut target = None;
        let pos = dialogs::tool_dialog_pos(ctx) + vec2(-300.0, 0.0);
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
        self.finish_stroke();
        if self.cancel_select_move() {
            return;
        }
        self.reset_pending();
        if self.doc.as_mut().is_some_and(|d| d.go_to_step(done)) {
            self.reset_pending();
            self.clip_selection();
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
    let text = painter.with_clip_rect(rect.shrink2(vec2(6.0, 0.0)).intersect(painter.clip_rect()));
    text.text(
        pos2(rect.left() + 32.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        FontId::proportional(ui_kit::TEXT_SIZE),
        color,
    );
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, current, name));
    response
}
