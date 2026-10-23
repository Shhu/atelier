//! Command palette (Ctrl+K): searches the tools and every menu command.

use std::ops::Range;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, FontId};

use super::keys;
use super::*;
use crate::i18n::t;

pub(super) const SHORTCUT_PALETTE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::K);

/// Results shown per group while typing (a line says how many more).
const MAX_TOOLS: usize = 4;
const MAX_COMMANDS: usize = 12;
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
    /// An option's state (View › Rulers, Window › History…).
    checked: Option<bool>,
    action: Action,
}

/// A group of results: its title, the results shown, how many more match.
type Group = (String, Vec<Entry>, usize);

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
            .map(|(_, entries, _)| entries.len())
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
        let active = groups
            .iter()
            .flat_map(|(_, entries, _)| entries)
            .nth(p.active);
        if enter {
            run = active.filter(|e| e.enabled).map(|e| e.action);
        }
        // A greyed result says why, instead of the keys' hints.
        let why = active
            .filter(|e| !e.enabled)
            .map(|e| self.why_unavailable(e.action));

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
                palette_footer(ui, why.as_deref());
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

    /// Tools and commands matching `query`, by group, the available ones
    /// and the best matches first; suggestions when empty.
    fn palette_groups(&self, ctx: &egui::Context, query: &str) -> Vec<Group> {
        let mut commands = Vec::new();
        for (title, entries) in self.menus() {
            flatten(ctx, &entries, &title, &mut commands);
        }
        if query.is_empty() {
            // With no image: what starts one.
            let no_doc = self.doc.is_none();
            let suggested = commands
                .into_iter()
                .filter(|e| e.enabled && suggested(e.action, no_doc))
                .collect();
            return vec![(t!("palette-suggested"), suggested, 0)];
        }
        let tools = Tool::ALL.into_iter().map(|tool| Entry {
            label: tool.name(),
            path: t!("palette-tool"),
            shortcut: tool.shortcut().map(|k| k.name().to_owned()),
            enabled: true,
            checked: None,
            action: Action::Tool(tool),
        });
        let tools = ranked(tools.collect(), query, MAX_TOOLS);
        let commands = ranked(commands, query, MAX_COMMANDS);
        [
            (t!("palette-tools"), tools.0, tools.1),
            (t!("palette-commands"), commands.0, commands.1),
        ]
        .into_iter()
        .filter(|(_, entries, _)| !entries.is_empty())
        .collect()
    }

    /// Why a greyed command can't run now, in a few words.
    pub(super) fn why_unavailable(&self, action: Action) -> String {
        unavailable_reason(self.doc.is_some(), self.selection.is_some(), action)
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
        groups: &[Group],
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
                        for (title, entries, more) in groups {
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                ui.add_space(10.0);
                                ui_kit::section(ui, title);
                            });
                            ui.add_space(4.0);
                            for entry in entries {
                                let active = index == self.palette.active;
                                let mut response = palette_row(ui, entry, query, active);
                                if !entry.enabled {
                                    response =
                                        response.on_hover_text(self.why_unavailable(entry.action));
                                }
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
                            if *more > 0 {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new(t!("ui3-palette-more", count = *more))
                                            .size(12.0)
                                            .color(ui_kit::color::LABEL),
                                    );
                                });
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
                checked: None,
                action: *action,
            }),
            MenuEntry::Check(label, enabled, checked, shortcut, action) => out.push(Entry {
                label: label.clone(),
                path: path.to_owned(),
                shortcut: shortcut.map(|s| keys::shortcut(ctx, s)),
                enabled: *enabled,
                checked: Some(*checked),
                action: *action,
            }),
            MenuEntry::Choice(label, selected, action) => out.push(Entry {
                label: label.clone(),
                path: path.to_owned(),
                shortcut: None,
                enabled: true,
                checked: Some(*selected),
                action: *action,
            }),
            MenuEntry::Unavailable(..) => {}
        }
    }
}

/// Why a greyed command can't run, knowing whether there is an image and
/// a selection.
pub(super) fn unavailable_reason(has_doc: bool, has_selection: bool, action: Action) -> String {
    let needs_selection = matches!(
        action,
        Action::Cut
            | Action::ClearSelection
            | Action::ContentAwareFill
            | Action::Deselect
            | Action::InvertSelection
            | Action::CropToSelection
            | Action::AddMask(NewMask::RevealSelection | NewMask::HideSelection)
    );
    if !has_doc {
        t!("ui3-why-no-image")
    } else if needs_selection && !has_selection {
        t!("ui3-why-no-selection")
    } else if matches!(action, Action::Undo) {
        t!("ui3-why-nothing-to-undo")
    } else if matches!(action, Action::Redo) {
        t!("ui3-why-nothing-to-redo")
    } else {
        t!("ui3-why-not-now")
    }
}

/// Commands proposed before typing: what starts an image when there is
/// none, otherwise a few frequent ones.
fn suggested(action: Action, no_doc: bool) -> bool {
    if no_doc {
        return matches!(
            action,
            Action::Open
                | Action::NewDocument
                | Action::Paste
                | Action::Workspace(workspace::Command::OpenRecent(_))
        );
    }
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

/// Other words a command is looked for by ("resize" for Image Size), in
/// the language of the interface.
fn keywords(action: Action) -> String {
    match action {
        Action::ImageSize => t!("ui3-palette-keywords-image-size"),
        Action::CanvasSize => t!("ui3-palette-keywords-canvas-size"),
        Action::RotateCanvas(_) | Action::RotateArbitrary | Action::FlipCanvas(_) => {
            t!("ui3-palette-keywords-rotate")
        }
        Action::Tool(Tool::Canvas) | Action::CropToSelection | Action::CropToContent => {
            t!("ui3-palette-keywords-crop")
        }
        Action::Tool(Tool::Transform) => t!("ui3-palette-keywords-move"),
        Action::Tool(Tool::Text) => t!("ui3-palette-keywords-type"),
        Action::Adjust(AdjustKind::BlackWhite)
        | Action::NewAdjustmentLayer(AdjustKind::BlackWhite) => {
            t!("ui3-palette-keywords-black-white")
        }
        Action::ExportAs | Action::Automate(actions_panel::Command::QuickExport) => {
            t!("ui3-palette-keywords-export")
        }
        Action::Save | Action::SaveAs => t!("ui3-palette-keywords-save"),
        Action::RemoveBackground | Action::SelectSubject => {
            t!("ui3-palette-keywords-cut-out")
        }
        Action::Workspace(workspace::Command::Shortcuts) => t!("ui3-palette-keywords-shortcuts"),
        Action::Workspace(workspace::Command::About) => t!("ui3-palette-keywords-about"),
        Action::Workspace(workspace::Command::OpenRecent(_)) => t!("ui3-palette-keywords-recent"),
        _ => String::new(),
    }
}

/// The entries matching `query`, the available ones first, then the best
/// matches, at most `max`; and how many more match.
fn ranked(entries: Vec<Entry>, query: &str, max: usize) -> (Vec<Entry>, usize) {
    let mut scored: Vec<(u32, Entry)> = entries
        .into_iter()
        .filter_map(|e| {
            let score = score(&e.label, &e.path, &keywords(e.action), query)?;
            Some((score + if e.enabled { 1000 } else { 0 }, e))
        })
        .collect();
    // Stable: equal matches keep the menus' order.
    scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    let more = scored.len().saturating_sub(max);
    (scored.into_iter().take(max).map(|(_, e)| e).collect(), more)
}

/// Small words a query may hold that commands don't ("black and white").
const FILLERS: [&str; 12] = [
    "a", "and", "of", "the", "to", "et", "de", "des", "du", "la", "le", "les",
];

/// How well a command matches `query` (higher is better), or `None`: its
/// label starts with it; each word of it starts a word of the label; the
/// label holds it; each word starts a word of the menu path and label, or
/// of the keywords. Case and accents don't count.
fn score(label: &str, path: &str, keywords: &str, query: &str) -> Option<u32> {
    let query = fold(query);
    let label = fold(label);
    if label.starts_with(&query) {
        return Some(100);
    }
    let wanted: Vec<&str> = words(&query).filter(|w| !FILLERS.contains(w)).collect();
    if wanted.is_empty() {
        return label.contains(&query).then_some(60);
    }
    let all_found = |text: &str| {
        let have: Vec<&str> = words(text).collect();
        wanted
            .iter()
            .all(|q| have.iter().any(|w| starts_like(w, q)))
    };
    if all_found(&label) {
        Some(80)
    } else if label.contains(&query) {
        Some(60)
    } else if all_found(&format!("{} {label}", fold(path))) {
        Some(40)
    } else if all_found(&format!("{} {label} {}", fold(path), fold(keywords))) {
        Some(30)
    } else {
        None
    }
}

/// The words of folded text.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

/// `word` starts with `typed`, or with all of it but its last letter once
/// it is 5 letters long ("rotate" finds "rotation", "layers" "layer").
fn starts_like(word: &str, typed: &str) -> bool {
    if word.starts_with(typed) {
        return true;
    }
    let n = typed.chars().count();
    n >= 5 && {
        let stem: String = typed.chars().take(n - 1).collect();
        word.starts_with(&stem)
    }
}

/// `text` for searching: lower case, without accents ("é" → "e", "œ" →
/// "oe").
pub(super) fn fold(text: &str) -> String {
    folded_chars(text).into_iter().map(|(c, _)| c).collect()
}

/// The folded characters of `text`, each with the byte range of the
/// character it comes from.
fn folded_chars(text: &str) -> Vec<(char, Range<usize>)> {
    let mut out = Vec::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        let range = i..i + c.len_utf8();
        for lower in c.to_lowercase() {
            match base_letters(lower) {
                Some(base) => out.extend(base.chars().map(|b| (b, range.clone()))),
                None => out.push((lower, range.clone())),
            }
        }
    }
    out
}

/// The plain letters of an accented lower case letter.
fn base_letters(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
        'ç' => "c",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ñ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ý' | 'ÿ' => "y",
        'œ' => "oe",
        'æ' => "ae",
        'ß' => "ss",
        // Typographic apostrophes match the typed one.
        '’' | '‘' => "'",
        _ => return None,
    })
}

/// The part of `label` to highlight for `query` (byte range in `label`):
/// where it holds the whole query, otherwise the word the first typed
/// word starts.
fn highlight(label: &str, query: &str) -> Option<Range<usize>> {
    let chars = folded_chars(label);
    let query: Vec<char> = fold(query).chars().collect();
    if query.is_empty() {
        return None;
    }
    let span = |from: usize, len: usize| chars[from].1.start..chars[from + len - 1].1.end;
    if let Some(at) = chars
        .windows(query.len())
        .position(|w| w.iter().map(|(c, _)| *c).eq(query.iter().copied()))
    {
        return Some(span(at, query.len()));
    }
    let typed: String = query.iter().collect();
    let first: Vec<char> = words(&typed)
        .find(|w| !FILLERS.contains(w))?
        .chars()
        .collect();
    let len = first.len().min(chars.len());
    (0..chars.len()).find_map(|at| {
        let start_of_word = at == 0 || !chars[at - 1].0.is_alphanumeric();
        let n = (0..len.min(chars.len() - at))
            .take_while(|&k| chars[at + k].0 == first[k])
            .count();
        (start_of_word && n >= len.saturating_sub(1).max(1)).then(|| span(at, n))
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
    // An option shows its state first.
    if let Some(checked) = entry.checked {
        let (glyph, mark) = if checked {
            (icon::CHECK_SQUARE, color::ACCENT_TEXT)
        } else {
            (icon::SQUARE, color::LABEL)
        };
        job.append(glyph, 0.0, format(FontId::proportional(13.0), mark));
        job.append(" ", 0.0, format(FontId::proportional(13.0), text));
    }
    let hit = highlight(&entry.label, query);
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

/// Key hints at the bottom, or why the highlighted command is greyed.
fn palette_footer(ui: &mut egui::Ui, why: Option<&str>) {
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
    if let Some(why) = why {
        row.label(
            egui::RichText::new(format!("{}  {why}", icon::INFO))
                .size(12.0)
                .color(color::MUTED),
        );
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn app(with_doc: bool, lang: Lang) -> Harness<'static, AtelierApp> {
        i18n::set_lang(lang);
        let mut h = Harness::builder()
            .with_size([1280.0, 800.0])
            .build_eframe(move |cc| {
                let mut app = AtelierApp::new(cc, None);
                if with_doc {
                    let img =
                        image::RgbaImage::from_pixel(64, 64, image::Rgba([90, 120, 200, 255]));
                    app.doc = Some(Document::new(img, None));
                }
                app
            });
        h.run_steps(2);
        h
    }

    /// The labels found for `query`, all groups.
    fn found(h: &Harness<'static, AtelierApp>, query: &str) -> Vec<(String, Action)> {
        h.state()
            .palette_groups(&h.ctx, query)
            .into_iter()
            .flat_map(|(_, entries, _)| entries)
            .map(|e| (e.label, e.action))
            .collect()
    }

    #[test]
    fn folding_drops_case_and_accents() {
        assert_eq!(fold("Luminosité/Contraste"), "luminosite/contraste");
        assert_eq!(fold("Œil Rouge"), "oeil rouge");
        assert_eq!(highlight("Netteté optimisée…", "nettete"), Some(0..8));
        assert_eq!(highlight("Image Size…", "size"), Some(6..10));
    }

    #[test]
    fn menu_paths_and_word_prefixes_match() {
        let h = app(true, Lang::En);
        let first = |q: &str| found(&h, q).first().map(|(label, _)| label.clone());
        assert_eq!(first("select all"), Some(t!("menu-select-all")));
        assert_eq!(first("sel sub"), Some(t!("menu-select-subject")));
        let rotate = found(&h, "rotate");
        assert!(
            rotate
                .iter()
                .any(|(_, a)| matches!(a, Action::RotateCanvas(90)))
        );
        let mask = found(&h, "layer mask");
        assert!(
            mask.iter()
                .any(|(_, a)| matches!(a, Action::AddMask(NewMask::RevealAll)))
        );
        assert!(
            found(&h, "language")
                .iter()
                .any(|(_, a)| matches!(a, Action::SetLanguage(_)))
        );
        assert!(
            found(&h, "black and white")
                .iter()
                .any(|(_, a)| matches!(a, Action::Adjust(AdjustKind::BlackWhite)))
        );
        assert!(
            found(&h, "shortcuts")
                .iter()
                .any(|(_, a)| matches!(a, Action::Workspace(workspace::Command::Shortcuts)))
        );
    }

    /// Keywords find a command by another name; small words are skipped.
    #[test]
    fn keywords_and_fillers() {
        let keywords = "resize scale resample";
        assert!(score("Image Size…", "Image", keywords, "resize").is_some());
        assert!(score("Image Size…", "Image", "", "resize").is_none());
        assert!(
            score(
                "Black & White…",
                "Image › Adjustments",
                "",
                "black and white"
            )
            .is_some()
        );
        assert!(score("Image Rotation", "Image", "", "rotate").is_some());
        // The label's own words rank above the menu path's.
        let own = score("Select Subject", "Select", "", "subject").unwrap();
        let path = score("All", "Select", "", "select all").unwrap();
        assert!(own > path);
    }

    #[test]
    fn french_is_found_without_accents() {
        let h = app(true, Lang::Fr);
        let label = AdjustKind::BrightnessContrast;
        let found = found(&h, "luminosite");
        i18n::set_lang(Lang::En);
        assert!(
            found
                .iter()
                .any(|(_, a)| matches!(a, Action::Adjust(k) if *k == label))
        );
    }

    /// Available commands come first; a greyed one says why.
    #[test]
    fn available_results_come_first() {
        let h = app(true, Lang::En);
        let results = h.state().palette_groups(&h.ctx, "crop");
        let commands = &results.last().expect("commands").1;
        let first_greyed = commands.iter().position(|e| !e.enabled);
        if let Some(i) = first_greyed {
            assert!(commands[i..].iter().all(|e| !e.enabled));
        }
        assert_eq!(
            h.state().why_unavailable(Action::CropToSelection),
            t!("ui3-why-no-selection")
        );
    }

    /// With no image, the suggestions start one: none is greyed.
    #[test]
    fn suggestions_without_an_image_are_available() {
        let h = app(false, Lang::En);
        let groups = h.state().palette_groups(&h.ctx, "");
        let suggested = &groups[0].1;
        assert!(suggested.iter().any(|e| matches!(e.action, Action::Open)));
        assert!(
            suggested
                .iter()
                .any(|e| matches!(e.action, Action::NewDocument))
        );
        assert!(suggested.iter().all(|e| e.enabled));
    }

    /// Options show their state; cut lists say how many more match.
    #[test]
    fn toggles_show_their_state_and_cut_lists_say_so() {
        let mut h = app(true, Lang::En);
        h.state_mut().history.open = true;
        let groups = h.state().palette_groups(&h.ctx, &t!("xchg-menu-history"));
        let history = groups
            .iter()
            .flat_map(|(_, e, _)| e)
            .find(|e| matches!(e.action, Action::History))
            .expect("Window › History");
        assert_eq!(history.checked, Some(true));
        let groups = h.state().palette_groups(&h.ctx, "e");
        assert!(groups.iter().any(|(_, _, more)| *more > 0));
    }
}
