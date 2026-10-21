//! Command palette (Ctrl+K): searches the tools and every menu command.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, FontId};

use super::keys;
use super::*;
use crate::i18n::t;

pub(super) const SHORTCUT_PALETTE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::K);

/// Results shown per group while typing.
const MAX_TOOLS: usize = 4;
const MAX_COMMANDS: usize = 8;
const WIDTH: f32 = 640.0;
const ROW_HEIGHT: f32 = 38.0;

/// Palette state: closed, or open with its query and highlighted row.
#[derive(Default)]
pub(super) struct Palette {
    open: bool,
    pub(super) query: String,
    /// Highlighted result, counted across the groups.
    active: usize,
    /// The highlight moved with the keyboard: scroll it into view.
    scroll: bool,
}

impl Palette {
    pub(super) fn open(&mut self) {
        *self = Self {
            open: true,
            ..Self::default()
        };
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }
}

/// A result: a tool or a menu command.
struct Entry {
    label: String,
    /// Where it lives: "Image › Adjustments", "Tool".
    path: String,
    shortcut: Option<String>,
    enabled: bool,
    action: Action,
}

impl AtelierApp {
    /// Draws the open palette and runs the chosen entry.
    pub(super) fn palette_window(&mut self, ctx: &egui::Context) {
        if !self.palette.open {
            return;
        }
        let query = self.palette.query.trim().to_owned();
        let groups = self.palette_groups(ctx, &query);
        let count = groups
            .iter()
            .map(|(_, entries)| entries.len())
            .sum::<usize>();

        // Keys first, so the search field doesn't see them.
        let (down, up, enter, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape) | i.consume_shortcut(&SHORTCUT_PALETTE),
            )
        });
        let p = &mut self.palette;
        if down {
            p.active += 1;
        }
        if up {
            p.active = p.active.saturating_sub(1);
        }
        p.active = p.active.min(count.saturating_sub(1));
        p.scroll |= down || up;
        let mut run = None;
        if enter {
            run = groups
                .iter()
                .flat_map(|(_, entries)| entries)
                .nth(p.active)
                .filter(|e| e.enabled)
                .map(|e| e.action);
        }

        let id = egui::Id::new("command_palette");
        let width = WIDTH.min(ctx.content_rect().width() - 32.0);
        let modal = egui::Modal::new(id)
            .area(egui::Modal::default_area(id).anchor(Align2::CENTER_TOP, vec2(0.0, 92.0)))
            .backdrop_color(Color32::from_rgba_unmultiplied(6, 7, 8, 140))
            .frame(
                egui::Frame::new()
                    .fill(ui_kit::color::BG2)
                    .stroke(UiStroke::new(1.0, ui_kit::color::WINDOW_LINE))
                    .corner_radius(14)
                    .shadow(egui::Shadow {
                        offset: [0, 30],
                        blur: 80,
                        spread: 0,
                        color: Color32::from_black_alpha(153),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                self.palette_search(ui);
                let clicked = self.palette_results(ui, &groups, &query);
                palette_footer(ui);
                clicked
            });
        run = run.or(modal.inner);
        if escape || run.is_some() || modal.should_close() {
            self.palette.open = false;
        }
        if let Some(action) = run {
            self.run(action);
        }
    }

    /// Tools and commands matching `query`, by group; suggestions when empty.
    fn palette_groups(&self, ctx: &egui::Context, query: &str) -> Vec<(String, Vec<Entry>)> {
        let mut commands = Vec::new();
        for (title, entries) in self.menus() {
            flatten(ctx, &entries, &title, &mut commands);
        }
        if query.is_empty() {
            let suggested = commands
                .into_iter()
                .filter(|e| suggested(e.action))
                .collect();
            return vec![(t!("palette-suggested"), suggested)];
        }
        let tools: Vec<Entry> = Tool::ALL
            .into_iter()
            .map(|tool| Entry {
                label: tool.name(),
                path: t!("palette-tool"),
                shortcut: tool.shortcut().map(|k| k.name().to_owned()),
                enabled: true,
                action: Action::Tool(tool),
            })
            .filter(|e| find(&e.label, query).is_some())
            .take(MAX_TOOLS)
            .collect();
        let commands: Vec<Entry> = commands
            .into_iter()
            .filter(|e| find(&e.label, query).is_some())
            .take(MAX_COMMANDS)
            .collect();
        [
            (t!("palette-tools"), tools),
            (t!("palette-commands"), commands),
        ]
        .into_iter()
        .filter(|(_, entries)| !entries.is_empty())
        .collect()
    }

    /// Search field row: icon, query (focused), Esc.
    fn palette_search(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 54.0), Sense::hover());
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            UiStroke::new(1.0, ui_kit::color::LINE),
        );
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(16.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 12.0;
        row.label(
            egui::RichText::new(icon::MAGNIFYING_GLASS)
                .size(18.0)
                .color(ui_kit::color::LABEL),
        );
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui_kit::kbd(ui, &keys::key(Key::Escape));
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.palette.query)
                    .id(egui::Id::new("command_palette_query"))
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .font(egui::FontId::proportional(15.0))
                    .text_color(ui_kit::color::TEXT)
                    .hint_text(
                        egui::RichText::new(t!("palette-placeholder")).color(ui_kit::color::LABEL),
                    )
                    .desired_width(ui.available_width()),
            );
            if edit.changed() {
                // The results were built before the field: show the new ones.
                self.palette.active = 0;
                ui.ctx().request_repaint();
            }
            // The field keeps the keyboard while the palette is open.
            edit.request_focus();
        });
    }

    /// Result groups (scrolling past 400 px); returns the clicked action.
    fn palette_results(
        &mut self,
        ui: &mut egui::Ui,
        groups: &[(String, Vec<Entry>)],
        query: &str,
    ) -> Option<Action> {
        let mut clicked = None;
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 6,
                right: 6,
                top: 4,
                bottom: 8,
            })
            .show(ui, |ui| {
                if groups.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(t!("palette-no-match", query = query))
                                .color(ui_kit::color::LABEL),
                        );
                    });
                    ui.add_space(24.0);
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(400.0 - 12.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let moved = ui.input(|i| i.pointer.delta() != Vec2::ZERO);
                        let mut index = 0;
                        for (title, entries) in groups {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                ui_kit::section(ui, title);
                            });
                            ui.add_space(4.0);
                            for entry in entries {
                                let active = index == self.palette.active;
                                let response = palette_row(ui, entry, query, active);
                                if response.hovered() && moved {
                                    self.palette.active = index;
                                }
                                if active && self.palette.scroll {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() && entry.enabled {
                                    clicked = Some(entry.action);
                                }
                                index += 1;
                            }
                        }
                    });
            });
        self.palette.scroll = false;
        clicked
    }
}

/// Menu commands, submenus flattened ("Image › Adjustments").
fn flatten(ctx: &egui::Context, entries: &[MenuEntry], path: &str, out: &mut Vec<Entry>) {
    for entry in entries {
        match entry {
            MenuEntry::Separator => {}
            MenuEntry::Submenu(label, sub) => flatten(ctx, sub, &format!("{path} › {label}"), out),
            MenuEntry::Item(label, enabled, shortcut, action) => out.push(Entry {
                label: label.clone(),
                path: path.to_owned(),
                shortcut: shortcut.map(|s| keys::shortcut(ctx, s)),
                enabled: *enabled,
                action: *action,
            }),
            MenuEntry::Check(label, enabled, _, shortcut, action) => out.push(Entry {
                label: label.clone(),
                path: path.to_owned(),
                shortcut: shortcut.map(|s| keys::shortcut(ctx, s)),
                enabled: *enabled,
                action: *action,
            }),
            MenuEntry::Choice(label, _, action) => out.push(Entry {
                label: label.clone(),
                path: path.to_owned(),
                shortcut: None,
                enabled: true,
                action: *action,
            }),
            MenuEntry::Unavailable(..) => {}
        }
    }
}

/// Commands proposed before typing.
fn suggested(action: Action) -> bool {
    matches!(
        action,
        Action::RemoveBackground
            | Action::SelectSubject
            | Action::Tool(Tool::Canvas)
            | Action::ImageSize
            | Action::Split
            | Action::Fit
    )
}

/// Case-insensitive position of `query` in `text` (byte range in `text`).
fn find(text: &str, query: &str) -> Option<Range<usize>> {
    let lower = |c: char| c.to_lowercase();
    text.char_indices().find_map(|(start, _)| {
        let mut rest = text[start..].char_indices();
        let mut end = start;
        for q in query.chars() {
            let (i, c) = rest.next()?;
            if !lower(c).eq(lower(q)) {
                return None;
            }
            end = start + i + c.len_utf8();
        }
        Some(start..end)
    })
}

/// One result row: label with the match highlighted, menu path, shortcut.
fn palette_row(ui: &mut egui::Ui, entry: &Entry, query: &str, active: bool) -> egui::Response {
    use ui_kit::color;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let painter = ui.painter_at(rect);
    if active {
        painter.rect_filled(rect, 7.0, color::ACCENT_SOFT);
    }
    let inner = rect.shrink2(vec2(10.0, 0.0));
    let mut right = inner.right();
    if let Some(key) = &entry.shortcut {
        right -= paint_kbd(
            &painter,
            pos2(right, rect.center().y),
            key,
            ui_kit::mono(11.0),
        ) + 12.0;
    }
    let path = painter.layout_no_wrap(entry.path.clone(), FontId::proportional(12.0), color::LABEL);
    right -= path.size().x;
    painter.galley(
        pos2(right, rect.center().y - path.size().y / 2.0),
        path,
        color::LABEL,
    );
    right -= 12.0;

    let text = if entry.enabled {
        color::TEXT
    } else {
        color::DISABLED
    };
    let format = |font_id, color| TextFormat {
        font_id,
        color,
        ..Default::default()
    };
    let mut job = LayoutJob::default();
    let hit = find(&entry.label, query).filter(|_| !query.is_empty());
    let (start, end) = hit.map_or((entry.label.len(), entry.label.len()), |r| (r.start, r.end));
    job.append(
        &entry.label[..start],
        0.0,
        format(FontId::proportional(13.0), text),
    );
    let strong = if entry.enabled {
        color::ACCENT_TEXT
    } else {
        text
    };
    job.append(
        &entry.label[start..end],
        0.0,
        format(ui_kit::bold(13.0), strong),
    );
    job.append(
        &entry.label[end..],
        0.0,
        format(FontId::proportional(13.0), text),
    );
    job.wrap = TextWrapping::truncate_at_width((right - inner.left()).max(0.0));
    let label = painter.layout_job(job);
    painter.galley(
        pos2(inner.left(), rect.center().y - label.size().y / 2.0),
        label,
        text,
    );
    response
}

/// Keyboard key ending at `right_center`; returns its width.
fn paint_kbd(painter: &egui::Painter, right_center: Pos2, key: &str, font: FontId) -> f32 {
    let galley = painter.layout_no_wrap(key.to_owned(), font, ui_kit::color::MUTED);
    let size = vec2(galley.size().x + 10.0, 18.0);
    let rect = Rect::from_min_size(right_center - vec2(size.x, size.y / 2.0), size);
    painter.rect(
        rect,
        4.0,
        ui_kit::color::BG1,
        UiStroke::new(1.0, ui_kit::color::KBD_LINE),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        ui_kit::color::MUTED,
    );
    size.x
}

/// Key hints at the bottom.
fn palette_footer(ui: &mut egui::Ui) {
    use ui_kit::color;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
    // Rounded at the bottom, like the palette.
    ui.painter().rect_filled(
        rect,
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 13,
            se: 13,
        },
        color::LAYERS,
    );
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        UiStroke::new(1.0, color::LINE),
    );
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(16.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    // Arrows are Phosphor icons, missing from the monospace font.
    let arrow = || FontId::proportional(11.0);
    let hint = |ui: &mut egui::Ui, keys: &[(&str, FontId)], text: String| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (key, font) in keys {
            let width = ui
                .painter()
                .layout_no_wrap(key.to_string(), font.clone(), color::MUTED)
                .size()
                .x;
            let (rect, _) = ui.allocate_exact_size(vec2(width + 10.0, 18.0), Sense::hover());
            paint_kbd(ui.painter(), rect.right_center(), key, font.clone());
        }
        ui.label(egui::RichText::new(text).size(12.0).color(color::LABEL));
        ui.add_space(10.0);
    };
    hint(
        &mut row,
        &[(icon::ARROW_UP, arrow()), (icon::ARROW_DOWN, arrow())],
        t!("palette-navigate"),
    );
    hint(
        &mut row,
        &[(&keys::key(Key::Enter), ui_kit::mono(11.0))],
        t!("palette-run"),
    );
    hint(
        &mut row,
        &[(&keys::key(Key::Escape), ui_kit::mono(11.0))],
        t!("palette-close"),
    );
}
