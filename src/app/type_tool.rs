//! Type tool, like Photoshop's: a click starts a point text, a drag a
//! paragraph text in a box (resized by its handles, a "+" when the text
//! overflows it); the text is typed on the canvas with a caret and a
//! selection. Ctrl+Enter, Esc, another tool or a click elsewhere commits;
//! the Cancel button gives the edit up. Ctrl+drag moves a text.

use std::ops::Range;
use std::time::Duration;

use super::*;
use crate::document::HistoryMark;
use crate::mask_ops::Affine;
use crate::text::{
    self, AntiAlias, CharStyle, Layout, ParagraphStyle, TextAlign, TextData, TextEngine,
};

/// Settings of the next texts, and the text being edited.
pub(super) struct TypeTool {
    /// Character style of the next texts (their color: the foreground's).
    pub style: CharStyle,
    pub paragraph: ParagraphStyle,
    pub antialias: AntiAlias,
    pub edit: Option<TypeEdit>,
    drag: Option<TypeDrag>,
}

impl Default for TypeTool {
    fn default() -> Self {
        Self {
            style: CharStyle::default(),
            paragraph: ParagraphStyle::default(),
            antialias: AntiAlias::Sharp,
            edit: None,
            drag: None,
        }
    }
}

/// A text being typed on the canvas.
pub(super) struct TypeEdit {
    /// Its layer.
    pub layer: u64,
    /// Byte indices of the caret and of the other end of the selection.
    pub caret: usize,
    pub anchor: usize,
    /// Abscissa kept by Up and Down.
    goal_x: Option<f32>,
    /// Style chosen with nothing selected: what is typed next takes it.
    pub typing: Option<CharStyle>,
    /// The history before the edit (Cancel goes back to it).
    undo_len: HistoryMark,
    /// Its layer was created for it: dropped if it ends empty.
    created: bool,
    /// The text before the edit (of an existing layer): an edit that ends
    /// with it leaves no step.
    original: Option<TextData>,
    /// When the caret last moved: it shows steadily, then blinks.
    moved: f64,
    /// Ctrl+Z / Ctrl+Shift+Z while typing: the text and its caret and
    /// selection before each series of changes, and those undone.
    undo: Vec<TextState>,
    redo: Vec<TextState>,
    /// When the text last changed; a space or a line typed ends a series.
    changed: f64,
    new_series: bool,
    /// What an input method is composing (not in the text yet): shown at
    /// the caret, underlined.
    pub preedit: String,
}

/// A text as it was during an edit: its data, its caret and anchor.
type TextState = (TextData, usize, usize);

/// While typing, changes this far apart (s) are undone separately.
const TYPING_PAUSE: f64 = 1.0;
/// Series of changes that can be undone while typing, at most.
const TYPING_UNDO: usize = 200;

impl TypeEdit {
    fn new(layer: u64, caret: usize, undo_len: HistoryMark, created: bool, now: f64) -> Self {
        Self {
            layer,
            caret,
            anchor: caret,
            goal_x: None,
            typing: None,
            undo_len,
            original: None,
            created,
            moved: now,
            undo: Vec::new(),
            redo: Vec::new(),
            changed: f64::NEG_INFINITY,
            new_series: false,
            preedit: String::new(),
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.caret.min(self.anchor)..self.caret.max(self.anchor)
    }

    fn place(&mut self, i: usize, extend: bool) {
        self.caret = i;
        if !extend {
            self.anchor = i;
        }
        self.goal_x = None;
        self.typing = None;
    }
}

enum TypeDrag {
    /// A new text: a click makes a point text, a drag a box.
    New { start: Pos2, now: Pos2 },
    /// Selecting by dragging over the text.
    Select,
    /// A handle of the box (text coordinates), and where the box's origin
    /// was on the canvas.
    Frame { drag: RectDrag, origin: Pos2 },
}

/// Canvas point of the text point `(x, y)` of a text layer.
pub(super) fn text_to_canvas(offset: (i64, i64), t: &TextData, (x, y): (f32, f32)) -> Pos2 {
    let [a, b, c, d] = t.transform;
    pos2(
        offset.0 as f32 + t.anchor.0 + a * x + b * y,
        offset.1 as f32 + t.anchor.1 + c * x + d * y,
    )
}

/// Text point of the canvas point `p` of a text layer.
fn canvas_to_text(offset: (i64, i64), t: &TextData, p: Pos2) -> (f32, f32) {
    let [a, b, c, d] = t.transform;
    let inverse = Affine([a, b, 0.0, c, d, 0.0]).inverse();
    let (x, y) = (
        p.x - offset.0 as f32 - t.anchor.0,
        p.y - offset.1 as f32 - t.anchor.1,
    );
    inverse.map_or((x, y), |m| m.apply(x, y))
}

/// Selection highlight.
const SELECTION: Color32 = Color32::from_rgba_premultiplied(30, 70, 150, 110);

impl AtelierApp {
    pub(super) fn text_engine(&mut self) -> &mut TextEngine {
        if self.text_engine.is_none() {
            let engine = TextEngine::new();
            let style = &mut self.type_tool.style;
            if style.family.is_empty() {
                // A common font if available, otherwise the default font.
                let preferred = ["Arial", "Segoe UI", "DejaVu Sans", "Liberation Sans"];
                if let Some(f) = preferred
                    .iter()
                    .find(|f| engine.families().iter().any(|e| e == *f))
                {
                    style.family = f.to_string();
                }
            }
            self.text_engine = Some(engine);
        }
        self.text_engine
            .as_mut()
            .expect("text engine created above")
    }

    /// Whether a text is being typed on the canvas (it takes the keys).
    pub(super) fn typing_on_canvas(&self) -> bool {
        self.type_tool.edit.is_some()
    }

    /// The edit ends if its layer is no longer the active text (undo,
    /// another document…); its caret stays inside the text.
    fn sync_type_edit(&mut self) {
        let Some(edit) = &mut self.type_tool.edit else {
            return;
        };
        let text = self
            .doc
            .as_ref()
            .map(Document::active_layer)
            .filter(|l| l.id == edit.layer && !l.locked)
            .and_then(|l| l.text.as_ref());
        match text {
            Some(t) => {
                let clamp = |i: usize| {
                    let mut i = i.min(t.text.len());
                    while !t.text.is_char_boundary(i) {
                        i -= 1;
                    }
                    i
                };
                edit.caret = clamp(edit.caret);
                edit.anchor = clamp(edit.anchor);
            }
            None => {
                self.type_tool.edit = None;
                self.type_tool.drag = None;
            }
        }
    }

    /// New text at `origin` (canvas): point text, or paragraph text in a
    /// box of `frame`. It is typed right away.
    pub(super) fn create_text(&mut self, origin: Pos2, frame: Option<Vec2>) {
        // Load the fonts first: that is where the default font is chosen.
        self.text_engine();
        let style = CharStyle {
            color: self.primary.to_srgba_unmultiplied(),
            ..self.type_tool.style.clone()
        };
        let mut data = TextData::new(style, self.type_tool.paragraph);
        data.antialias = self.type_tool.antialias;
        data.frame = frame.map(|f| (f.x.max(1.0), f.y.max(1.0)));
        data.anchor = (origin.x.rem_euclid(1.0), origin.y.rem_euclid(1.0));
        let rendered = self.text_engine().render(&data);
        let now = self.ctx.input(|i| i.time);
        let Some(doc) = &mut self.doc else { return };
        let undo_len = doc.history_mark();
        doc.add_text_layer(data, rendered, (origin.x, origin.y));
        let id = doc.active_layer().id;
        // The creation is in the history: the typing joins it.
        self.prop_session = Some(((id, "text"), doc.history_step()));
        self.type_tool.edit = Some(TypeEdit::new(id, 0, undo_len, true, now));
        self.status = t!("type-status-typing");
    }

    /// Starts typing in the text layer `i`, the caret at the canvas point
    /// `at`.
    pub(super) fn edit_text_layer(&mut self, i: usize, at: Pos2) {
        self.select_layer(i);
        let now = self.ctx.input(|i| i.time);
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let Some(t) = layer.text.clone() else { return };
        let (id, offset) = (layer.id, layer.offset);
        let undo_len = doc.history_mark();
        let (x, y) = canvas_to_text(offset, &t, at);
        let caret = self.text_engine().layout(&t).hit(x, y);
        self.prop_session = None;
        let mut edit = TypeEdit::new(id, caret, undo_len, false, now);
        edit.original = Some(t);
        self.type_tool.edit = Some(edit);
        self.status = t!("type-status-typing");
    }

    /// Ends the edit, keeping the text. An empty text is removed.
    pub(super) fn commit_text(&mut self) {
        self.type_tool.drag = None;
        let Some(edit) = self.type_tool.edit.take() else {
            return;
        };
        self.prop_session = None;
        let Some(doc) = &mut self.doc else { return };
        let Some(i) = doc.layers.iter().position(|l| l.id == edit.layer) else {
            return;
        };
        let text = doc.layers[i].text.as_ref();
        if !text.is_some_and(|t| t.text.is_empty()) {
            // Back to the text it started with, by its own steps only: no
            // step is left.
            let (names, done) = doc.history();
            let typed = done > 0 && names[done - 1] == t!("xchg-hist-edit-type");
            if edit.original.is_some()
                && text == edit.original.as_ref()
                && typed
                && doc.steps_since(edit.undo_len) == 1
            {
                doc.rollback_to(edit.undo_len);
            }
            self.status = t!("type-status-committed");
            return;
        }
        if !edit.created || !doc.rollback_to(edit.undo_len) {
            // (A new layer whose creation the history no longer reaches.)
            delete_layer(doc, edit.layer);
        }
    }

    /// Gives the edit up: the text goes back to how it was before it.
    pub(super) fn cancel_text(&mut self) {
        self.type_tool.drag = None;
        let Some(edit) = self.type_tool.edit.take() else {
            return;
        };
        self.prop_session = None;
        if let Some(doc) = &mut self.doc
            && !doc.rollback_to(edit.undo_len)
            && edit.created
        {
            // (A new layer whose creation the history no longer reaches.)
            delete_layer(doc, edit.layer);
        }
        self.status = t!("type-status-cancelled");
    }

    /// Changes the active text layer (and the caret of its edit), as one
    /// undo step per series of edits.
    pub(super) fn change_text(&mut self, f: impl FnOnce(&mut TextData, Option<&mut TypeEdit>)) {
        self.sync_type_edit();
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            return;
        };
        let (Some(old), false) = (layer.text.clone(), layer.locked) else {
            return;
        };
        let id = layer.id;
        let mut data = old.clone();
        let now = self.ctx.input(|i| i.time);
        let edit = self.type_tool.edit.as_mut();
        let before = edit.as_ref().map(|e| (e.caret, e.anchor));
        f(&mut data, edit);
        if let Some(edit) = &mut self.type_tool.edit {
            edit.moved = now;
        }
        if data == old {
            return;
        }
        if let (Some(edit), Some((caret, anchor))) = (&mut self.type_tool.edit, before) {
            if edit.new_series || edit.undo.is_empty() || now - edit.changed > TYPING_PAUSE {
                edit.undo.push((old, caret, anchor));
                if edit.undo.len() > TYPING_UNDO {
                    edit.undo.remove(0);
                }
            }
            edit.redo.clear();
            (edit.changed, edit.new_series) = (now, false);
        }
        let rendered = self.text_engine().render(&data);
        self.record_change(t!("xchg-hist-edit-type"), Some((id, "text")));
        if let Some(doc) = &mut self.doc {
            doc.set_active_text(data, rendered);
        }
    }

    /// Ctrl+Z (`redo`: Ctrl+Shift+Z) while typing: the last series of
    /// changes of the edit is undone, the edit goes on. `false` if there
    /// is none.
    fn undo_typing(&mut self, redo: bool) -> bool {
        self.sync_type_edit();
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            return false;
        };
        let (Some(now), id) = (layer.text.clone(), layer.id) else {
            return false;
        };
        let Some(edit) = &mut self.type_tool.edit else {
            return false;
        };
        let current = (now, edit.caret, edit.anchor);
        let (from, to) = if redo {
            (&mut edit.redo, &mut edit.undo)
        } else {
            (&mut edit.undo, &mut edit.redo)
        };
        let Some((data, caret, anchor)) = from.pop() else {
            return false;
        };
        to.push(current);
        edit.place(caret, false);
        edit.anchor = anchor;
        edit.new_series = true;
        let rendered = self.text_engine().render(&data);
        self.record_change(t!("xchg-hist-edit-type"), Some((id, "text")));
        if let Some(doc) = &mut self.doc {
            doc.set_active_text(data, rendered);
        }
        true
    }

    /// Types `s` in place of the selection.
    pub(super) fn type_text(&mut self, s: &str) {
        self.type_text_in(s);
        // A word or a line typed: what follows is undone on its own.
        if s.chars().any(char::is_whitespace)
            && let Some(edit) = &mut self.type_tool.edit
        {
            edit.new_series = true;
        }
    }

    fn type_text_in(&mut self, s: &str) {
        self.change_text(|data, edit| {
            let Some(edit) = edit else { return };
            let range = edit.range();
            let style = match edit.typing.take() {
                Some(style) => style,
                // Replacing: the style of the first replaced character.
                None if !range.is_empty() => data.styles_in(range.clone())[0].clone(),
                None => data.style_at(range.start).clone(),
            };
            data.delete(range.clone());
            data.insert(range.start, s, &style);
            edit.place(range.start + s.len(), false);
        });
    }

    /// Deletes the selection, or what `motion` covers from the caret.
    fn type_delete(&mut self, motion: fn(&str, usize) -> usize) {
        self.change_text(|data, edit| {
            let Some(edit) = edit else { return };
            let mut range = edit.range();
            if range.is_empty() {
                let to = motion(&data.text, edit.caret);
                range = to.min(edit.caret)..to.max(edit.caret);
            }
            data.delete(range.clone());
            edit.place(range.start, false);
        });
    }

    fn selected_text(&self) -> Option<String> {
        let edit = self.type_tool.edit.as_ref()?;
        let t = self.doc.as_ref()?.active_layer().text.as_ref()?;
        let r = edit.range();
        (!r.is_empty()).then(|| t.text[r].to_owned())
    }

    /// The edited text and its layout.
    fn edited_layout(&mut self) -> Option<(TextData, (i64, i64), Arc<Layout>)> {
        let layer = self.doc.as_ref()?.active_layer();
        let t = layer.text.clone()?;
        let offset = layer.offset;
        let layout = self.text_engine().layout(&t);
        Some((t, offset, layout))
    }

    /// Keys while typing on the canvas: they all go to the text, but those
    /// of the File menu (which commit it first).
    pub(super) fn type_keys(&mut self, ctx: &egui::Context) {
        self.sync_type_edit();
        if let Some(action) = ctx.input_mut(|i| take_shortcut(i, FILE_SHORTCUTS)) {
            self.commit_text();
            self.run(action);
            return;
        }
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                egui::Event::Text(s) => {
                    let s: String = s.chars().filter(|c| !c.is_control()).collect();
                    if !s.is_empty() {
                        self.type_text(&s);
                    }
                }
                egui::Event::Paste(s) => {
                    let s = s.replace("\r\n", "\n").replace('\r', "\n");
                    self.type_text(&s);
                }
                // What an input method (Chinese, Japanese, Korean…) is
                // composing, then composed.
                egui::Event::Ime(egui::ImeEvent::Preedit { text, .. }) => {
                    if let Some(edit) = &mut self.type_tool.edit {
                        edit.preedit = text;
                    }
                }
                egui::Event::Ime(egui::ImeEvent::Commit(s)) => {
                    if let Some(edit) = &mut self.type_tool.edit {
                        edit.preedit.clear();
                    }
                    if !s.is_empty() {
                        self.type_text(&s);
                    }
                }
                egui::Event::Copy | egui::Event::Cut => {
                    if let Some(s) = self.selected_text() {
                        ctx.copy_text(s);
                        if event == egui::Event::Cut {
                            self.type_delete(|_, i| i);
                        }
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => self.type_key(key, modifiers),
                _ => {}
            }
            if self.type_tool.edit.is_none() {
                break;
            }
        }
    }

    fn type_key(&mut self, key: Key, m: Modifiers) {
        let (ctrl, shift, alt) = (m.command, m.shift, m.alt);
        match key {
            Key::Escape => self.commit_text(),
            Key::Enter if ctrl => self.commit_text(),
            Key::Enter => self.type_text("\n"),
            Key::Tab if !ctrl => self.type_text("\t"),
            Key::Backspace if ctrl => self.type_delete(text::word_left),
            Key::Backspace => self.type_delete(text::prev_char),
            Key::Delete if ctrl => self.type_delete(text::word_right),
            Key::Delete => self.type_delete(text::next_char),
            Key::A if ctrl && !shift => {
                let len = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.active_layer().text.as_ref())
                    .map_or(0, |t| t.text.len());
                if let Some(e) = &mut self.type_tool.edit {
                    e.anchor = 0;
                    e.place(len, true);
                }
            }
            // The edit's own changes first, like Photoshop: the edit goes on.
            Key::Z if ctrl && self.undo_typing(shift) => {}
            Key::Z if ctrl && !shift => {
                // A new text with nothing typed: it goes, nothing else.
                let empty_new = self.type_tool.edit.as_ref().is_some_and(|e| e.created)
                    && self
                        .doc
                        .as_ref()
                        .and_then(|d| d.active_layer().text.as_ref())
                        .is_some_and(|t| t.text.is_empty());
                self.commit_text();
                if !empty_new {
                    self.run(Action::Undo);
                }
            }
            // Nothing typed to redo: the edit goes on (a step undone before
            // it stays undone).
            Key::Z if ctrl => {}
            // Proof Colors and Gamut Warning, the edit going on (redo is
            // Shift+Ctrl+Z).
            Key::Y if ctrl => self.color_command(if shift {
                color_ui::Command::GamutWarning
            } else {
                color_ui::Command::ProofColors
            }),
            Key::ArrowLeft | Key::ArrowRight if alt => {
                // Tracking, like Photoshop: 20/1000 em (Ctrl: 100).
                let step = if ctrl { 100.0 } else { 20.0 };
                let d = if key == Key::ArrowLeft { -step } else { step };
                self.restyle_selection(|s| s.tracking = (s.tracking + d).clamp(-1000.0, 10_000.0));
            }
            Key::ArrowUp | Key::ArrowDown if alt => {
                let d = if key == Key::ArrowUp { -2.0 } else { 2.0 };
                if shift {
                    // Baseline shift, up with Up.
                    self.restyle_selection(|s| s.baseline_shift -= d);
                } else {
                    self.restyle_selection(|s| {
                        s.leading = Some((s.line_height() + d).max(0.0));
                    });
                }
            }
            Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::Home
            | Key::End => self.move_caret(key, ctrl, shift),
            _ if ctrl && shift => self.type_style_key(key),
            _ => {}
        }
    }

    /// Ctrl+Shift shortcuts of the Character and Paragraph panels.
    fn type_style_key(&mut self, key: Key) {
        let align = match key {
            Key::L => Some(TextAlign::Left),
            Key::C => Some(TextAlign::Center),
            Key::R => Some(TextAlign::Right),
            Key::J => Some(TextAlign::Justify),
            _ => None,
        };
        if let Some(align) = align {
            self.restyle_paragraphs(|p| p.align = align);
            return;
        }
        let toggle: Option<fn(&mut CharStyle) -> &mut bool> = match key {
            Key::B => Some(|s| &mut s.faux_bold),
            Key::I => Some(|s| &mut s.faux_italic),
            Key::K => Some(|s| &mut s.all_caps),
            Key::H => Some(|s| &mut s.small_caps),
            Key::U => Some(|s| &mut s.underline),
            Key::Slash => Some(|s| &mut s.strikethrough),
            _ => None,
        };
        if let Some(field) = toggle {
            let on = !*field(&mut self.shown_char_style());
            self.restyle_selection(move |s| *field(s) = on);
            return;
        }
        let step = match key {
            Key::Period => 2.0,
            Key::Comma => -2.0,
            _ => return,
        };
        self.restyle_selection(move |s| s.size = (s.size + step).clamp(1.0, 5000.0));
    }

    fn move_caret(&mut self, key: Key, ctrl: bool, shift: bool) {
        let Some((t, _, layout)) = self.edited_layout() else {
            return;
        };
        let Some(edit) = &mut self.type_tool.edit else {
            return;
        };
        let range = edit.range();
        let text = &t.text;
        let i = edit.caret;
        let to = match key {
            // A selection collapses to its side.
            Key::ArrowLeft if !shift && !range.is_empty() => range.start,
            Key::ArrowRight if !shift && !range.is_empty() => range.end,
            Key::ArrowLeft if ctrl => text::word_left(text, i),
            Key::ArrowRight if ctrl => text::word_right(text, i),
            Key::ArrowLeft => text::prev_char(text, i),
            Key::ArrowRight => text::next_char(text, i),
            Key::Home if ctrl => 0,
            Key::End if ctrl => text.len(),
            Key::Home => layout.line_bounds(i).start,
            Key::End => layout.line_bounds(i).end,
            _ => {
                let x = edit.goal_x.unwrap_or(layout.caret(i).0);
                let up = key == Key::ArrowUp;
                let to = layout
                    .vertical(i, x, up)
                    .unwrap_or(if up { 0 } else { text.len() });
                edit.place(to, shift);
                edit.goal_x = Some(x);
                edit.moved = self.ctx.input(|i| i.time);
                return;
            }
        };
        edit.place(to, shift);
        edit.moved = self.ctx.input(|i| i.time);
    }

    /// The character style shown in the panels: of what is typed next, of
    /// the first selected character, of the text's first character, or of
    /// the next texts.
    pub(super) fn shown_char_style(&self) -> CharStyle {
        let text = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().text.as_ref());
        match (text, &self.type_tool.edit) {
            (
                Some(_),
                Some(TypeEdit {
                    typing: Some(s), ..
                }),
            ) => s.clone(),
            (Some(t), Some(e)) => t.styles_in(e.range())[0].clone(),
            (Some(t), None) => t.style_at(0).clone(),
            (None, _) => CharStyle {
                color: self.primary.to_srgba_unmultiplied(),
                ..self.type_tool.style.clone()
            },
        }
    }

    /// The paragraph style shown: of the caret's paragraph, the text's
    /// first, or of the next texts.
    pub(super) fn shown_paragraph_style(&self) -> ParagraphStyle {
        let text = self
            .doc
            .as_ref()
            .and_then(|d| d.active_layer().text.as_ref());
        match (text, &self.type_tool.edit) {
            (Some(t), Some(e)) => t.paragraphs[t.paragraph_at(e.range().start)],
            (Some(t), None) => t.paragraphs[0],
            (None, _) => self.type_tool.paragraph,
        }
    }

    /// Applies `f` to the selected characters (with nothing selected, to
    /// what is typed next; outside an edit, to the whole text layer;
    /// without a text layer, to the next texts).
    pub(super) fn restyle_selection(&mut self, f: impl Fn(&mut CharStyle)) {
        let is_text = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active_layer().text.is_some());
        if !is_text {
            let mut style = self.shown_char_style();
            f(&mut style);
            style.sanitize();
            self.primary = Color32::from_rgba_unmultiplied(
                style.color[0],
                style.color[1],
                style.color[2],
                255,
            );
            self.type_tool.style = style;
            return;
        }
        let typing = self
            .type_tool
            .edit
            .as_ref()
            .filter(|e| e.range().is_empty())
            .map(|_| self.shown_char_style());
        self.change_text(|data, edit| {
            let range = match &edit {
                Some(e) => e.range(),
                None => 0..data.text.len(),
            };
            if let (Some(mut style), Some(edit)) = (typing, edit)
                && !data.text.is_empty()
            {
                f(&mut style);
                style.sanitize();
                edit.typing = Some(style);
                return;
            }
            data.restyle(range, |s| {
                f(s);
                s.sanitize();
            });
        });
        // The next texts take this style too.
        let mut style = self.shown_char_style();
        style.color = self.type_tool.style.color;
        self.type_tool.style = style;
    }

    /// Applies `f` to the paragraphs of the selection (outside an edit, to
    /// all of them; without a text layer, to the next texts).
    pub(super) fn restyle_paragraphs(&mut self, f: impl Fn(&mut ParagraphStyle)) {
        let is_text = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active_layer().text.is_some());
        if is_text {
            self.change_text(|data, edit| {
                let range = edit.map_or(0..data.text.len(), |e| e.range());
                data.restyle_paragraphs(range, |p| {
                    f(p);
                    p.sanitize();
                });
            });
        }
        let mut p = self.shown_paragraph_style();
        if !is_text {
            f(&mut p);
            p.sanitize();
        }
        self.type_tool.paragraph = p;
    }

    /// Anti-aliasing of the active text layer, or of the next texts.
    pub(super) fn set_antialias(&mut self, aa: AntiAlias) {
        self.type_tool.antialias = aa;
        self.change_text(|data, _| data.antialias = aa);
    }

    // ---------------------------------------------------------------- mouse

    /// Type tool on the canvas.
    pub(super) fn type_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) -> CursorIcon {
        self.sync_type_edit();
        let (ctrl, shift) = self.ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        // Ctrl: the Move tool, for texts.
        let dragging = self.layer_drag.is_some() || self.xform.quick.dragging();
        if dragging || (ctrl && self.type_tool.drag.is_none()) {
            if let Some(cursor) = self.drag_layer_input(view, pointer, start, down, true) {
                return cursor;
            }
            if ctrl {
                return CursorIcon::Default;
            }
        }
        let Some(p) = pointer else {
            return CursorIcon::Text;
        };
        let ip = view.to_image(p);
        if let Some(drag) = self.type_tool.drag.take() {
            return self.type_drag(drag, view, ip, down, shift);
        }

        // Over the text being edited: its box's handles, its characters.
        let editing = self.type_tool.edit.is_some();
        if editing && let Some((t, offset, layout)) = self.edited_layout() {
            let q = canvas_to_text(offset, &t, ip);
            let scale = view.zoom
                * (t.transform[0] * t.transform[3] - t.transform[1] * t.transform[2])
                    .abs()
                    .sqrt();
            if let Some((w, h)) = t.frame {
                let r = Rect::from_min_size(Pos2::ZERO, vec2(w, h) * scale);
                let handle = hit_test(r, pos2(q.0, q.1) * scale).filter(|&h| h != Handle::Move);
                if let Some(handle) = handle {
                    if start {
                        let origin = text_to_canvas(offset, &t, (0.0, 0.0));
                        let drag = RectDrag::new(
                            handle,
                            Rect::from_min_size(Pos2::ZERO, vec2(w, h)),
                            pos2(q.0, q.1),
                        );
                        self.type_tool.drag = Some(TypeDrag::Frame { drag, origin });
                    }
                    let angle = t.transform[2].atan2(t.transform[0]).to_degrees();
                    return handle.cursor(angle);
                }
            }
            let margin = 8.0 / scale.max(1e-3);
            let [x0, y0, x1, y1] = layout.extent;
            let inside = q.0 >= x0 - margin
                && q.0 <= x1 + margin
                && q.1 >= y0 - margin
                && q.1 <= y1 + margin;
            if inside {
                if start {
                    let i = layout.hit(q.0, q.1);
                    let now = self.ctx.input(|i| i.time);
                    let edit = self.type_tool.edit.as_mut().expect("editing");
                    edit.place(i, shift);
                    edit.moved = now;
                    self.type_tool.drag = Some(TypeDrag::Select);
                }
                return CursorIcon::Text;
            }
            if !start {
                return CursorIcon::Text;
            }
            // A click elsewhere commits; on another text, edits it.
            self.commit_text();
            match self.text_layer_at(ip) {
                Some(i) if !shift => {
                    self.edit_text_layer(i, ip);
                    self.type_tool.drag = Some(TypeDrag::Select);
                }
                _ => {}
            }
            return CursorIcon::Text;
        }

        if start {
            let target = (!shift).then(|| self.text_layer_at(ip)).flatten();
            match target {
                Some(i) => {
                    self.edit_text_layer(i, ip);
                    self.type_tool.drag = Some(TypeDrag::Select);
                }
                None => self.type_tool.drag = Some(TypeDrag::New { start: ip, now: ip }),
            }
        }
        CursorIcon::Text
    }

    /// Topmost visible, unlocked text layer under the canvas point.
    fn text_layer_at(&self, ip: Pos2) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        (0..doc.layers.len()).rev().find(|&i| {
            let l = &doc.layers[i];
            l.visible && !l.locked && l.text.is_some() && to_rect(l.rect()).contains(ip)
        })
    }

    fn type_drag(
        &mut self,
        drag: TypeDrag,
        view: View,
        ip: Pos2,
        down: bool,
        shift: bool,
    ) -> CursorIcon {
        match drag {
            TypeDrag::New { start, .. } if !down => {
                let moved = view.to_screen(ip).distance(view.to_screen(start)) >= 4.0;
                if moved {
                    let r = Rect::from_two_pos(start, ip);
                    self.create_text(r.min, Some(r.size()));
                } else {
                    self.create_text(start, None);
                }
                CursorIcon::Text
            }
            TypeDrag::New { start, .. } => {
                self.type_tool.drag = Some(TypeDrag::New { start, now: ip });
                CursorIcon::Crosshair
            }
            TypeDrag::Select => {
                let (double, triple) = self.ctx.input(|i| {
                    (
                        i.pointer.button_double_clicked(PointerButton::Primary),
                        i.pointer.button_triple_clicked(PointerButton::Primary),
                    )
                });
                if let Some((t, offset, layout)) = self.edited_layout()
                    && let Some(edit) = &mut self.type_tool.edit
                {
                    let q = canvas_to_text(offset, &t, ip);
                    let i = layout.hit(q.0, q.1);
                    // Released after a double click: the word; a triple
                    // click: the paragraph.
                    let whole = if triple {
                        Some(t.paragraph_ranges()[t.paragraph_at(i)].clone())
                    } else if double {
                        Some(text::word_at(&t.text, i))
                    } else {
                        None
                    };
                    match whole {
                        Some(r) if !down => {
                            edit.anchor = r.start;
                            edit.caret = r.end;
                        }
                        _ => edit.caret = i,
                    }
                }
                if down {
                    self.type_tool.drag = Some(TypeDrag::Select);
                }
                CursorIcon::Text
            }
            TypeDrag::Frame { drag, origin } => {
                let Some((t, offset, _)) = self.edited_layout() else {
                    return CursorIcon::Default;
                };
                // In the box's coordinates at the start of the drag.
                let [a, b, c, d] = t.transform;
                let v = ip - origin;
                let q = Affine([a, b, 0.0, c, d, 0.0])
                    .inverse()
                    .map_or((v.x, v.y), |m| m.apply(v.x, v.y));
                let r = drag.update(pos2(q.0, q.1), shift);
                let moved = pos2(
                    origin.x + a * r.min.x + b * r.min.y,
                    origin.y + c * r.min.x + d * r.min.y,
                );
                self.change_text(|data, _| {
                    data.frame = Some((r.width().max(1.0), r.height().max(1.0)));
                    data.anchor = (moved.x - offset.0 as f32, moved.y - offset.1 as f32);
                });
                let angle = c.atan2(a).to_degrees();
                if down {
                    self.type_tool.drag = Some(TypeDrag::Frame { drag, origin });
                }
                drag.handle.cursor(angle)
            }
        }
    }

    // ---------------------------------------------------------------- drawing

    /// The box of the active text, the caret and the selection being typed,
    /// the box being drawn.
    pub(super) fn draw_type_overlay(&mut self, painter: &Painter, view: View) {
        if self.tool != Tool::Text {
            return;
        }
        let accent = ui_kit::color::ACCENT;
        if let Some(TypeDrag::New { start, now }) = &self.type_tool.drag
            && view.to_screen(*now).distance(view.to_screen(*start)) >= 4.0
        {
            let r = view.rect_to_screen(Rect::from_two_pos(*start, *now));
            ui_kit::dashed_rect(painter, r, UiStroke::new(1.0, accent), 6.0, 4.0);
        }
        self.sync_type_edit();
        let Some((t, offset, layout)) = self.edited_layout() else {
            return;
        };
        let screen = |q: (f32, f32)| view.to_screen(text_to_canvas(offset, &t, q));
        let quad = |[x0, y0, x1, y1]: [f32; 4]| {
            vec![
                screen((x0, y0)),
                screen((x1, y0)),
                screen((x1, y1)),
                screen((x0, y1)),
            ]
        };
        let Some(edit) = &self.type_tool.edit else {
            // The active text's box.
            let mut points = quad(layout.extent);
            points.push(points[0]);
            let stroke = UiStroke::new(1.0, accent);
            for pair in points.windows(2) {
                painter.add(egui::Shape::dashed_line(pair, stroke, 6.0, 4.0));
            }
            return;
        };
        for r in layout.selection(edit.range()) {
            painter.add(egui::Shape::convex_polygon(
                quad(r),
                SELECTION,
                UiStroke::NONE,
            ));
        }
        let stroke = UiStroke::new(1.0, accent);
        match t.frame {
            Some((w, h)) => {
                painter.add(egui::Shape::closed_line(quad([0.0, 0.0, w, h]), stroke));
                let handles = [
                    (0.0, 0.0),
                    (w / 2.0, 0.0),
                    (w, 0.0),
                    (0.0, h / 2.0),
                    (w, h / 2.0),
                    (0.0, h),
                    (w / 2.0, h),
                    (w, h),
                ];
                for q in handles {
                    let r = Rect::from_center_size(screen(q), Vec2::splat(7.0));
                    painter.rect(r, 1.0, Color32::WHITE, stroke, StrokeKind::Inside);
                }
                if layout.overflow {
                    // A "+" in the bottom-right handle: more text than fits.
                    let c = screen((w, h));
                    let r = Rect::from_center_size(c, Vec2::splat(11.0));
                    painter.rect(r, 1.0, Color32::WHITE, stroke, StrokeKind::Inside);
                    let plus = UiStroke::new(1.5, accent);
                    painter.line_segment([c - vec2(3.0, 0.0), c + vec2(3.0, 0.0)], plus);
                    painter.line_segment([c - vec2(0.0, 3.0), c + vec2(0.0, 3.0)], plus);
                }
            }
            None => {
                let mut points = quad(layout.extent);
                points.push(points[0]);
                for pair in points.windows(2) {
                    painter.add(egui::Shape::dashed_line(pair, stroke, 4.0, 3.0));
                }
            }
        }
        // Input methods on, their candidates next to the caret.
        let (x, top, bottom) = layout.caret(edit.caret);
        let cursor_rect = Rect::from_two_pos(screen((x, top)), screen((x, bottom)));
        self.ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect: painter.clip_rect(),
                cursor_rect,
                should_interrupt_composition: false,
            });
        });
        // The text being composed, at the caret, underlined.
        if !edit.preedit.is_empty() {
            let size = (cursor_rect.height() * 0.8).clamp(8.0, 200.0);
            let galley = painter.layout_no_wrap(
                edit.preedit.clone(),
                egui::FontId::proportional(size),
                Color32::BLACK,
            );
            let at = pos2(
                cursor_rect.left(),
                cursor_rect.center().y - galley.size().y / 2.0,
            );
            let r = Rect::from_min_size(at, galley.size()).expand(2.0);
            painter.rect_filled(r, 2.0, Color32::WHITE);
            painter.line_segment(
                [
                    pos2(r.left() + 2.0, r.bottom() - 1.0),
                    pos2(r.right() - 2.0, r.bottom() - 1.0),
                ],
                UiStroke::new(1.0, Color32::BLACK),
            );
            painter.galley(at, galley, Color32::BLACK);
        }
        // The caret: shown for a while after it moved, then blinking.
        let now = self.ctx.input(|i| i.time);
        let phase = (now - edit.moved).max(0.0) % 1.0;
        if edit.range().is_empty() && phase < 0.55 {
            let (x, top, bottom) = layout.caret(edit.caret);
            let (a, b) = (screen((x, top)), screen((x, bottom)));
            painter.line_segment([a, b], UiStroke::new(3.0, Color32::WHITE));
            painter.line_segment([a, b], UiStroke::new(1.0, Color32::BLACK));
        }
        let next = if phase < 0.55 {
            0.55 - phase
        } else {
            1.0 - phase
        };
        self.ctx
            .request_repaint_after(Duration::from_secs_f64(next.max(0.01)));
    }

    // ---------------------------------------------------------- transforms

    /// Text layer `i` moved by the canvas map `map` (scale, rotation, skew):
    /// it stays a text, re-rendered sharp. A uniform scale changes its
    /// sizes, the rest goes to its transformation. Not recorded in the
    /// history (the caller does).
    pub(super) fn transform_text(&mut self, i: usize, map: Affine) -> bool {
        let Some(doc) = &self.doc else { return false };
        let layer = &doc.layers[i];
        let Some(t) = layer.text.clone() else {
            return false;
        };
        let offset = layer.offset;
        let [a, b, _, c, d, _] = map.0;
        let origin = text_to_canvas(offset, &t, (0.0, 0.0));
        let moved = map.apply(origin.x, origin.y);
        // Uniform scale (rotations included): the sizes; the rest stays.
        let det = a * d - b * c;
        let s = det.abs().sqrt();
        let uniform = (a - d).abs() < 1e-3 * s && (b + c).abs() < 1e-3 * s && det > 0.0;
        let mut data = if uniform { t.scaled(s) } else { t.clone() };
        let lin = if uniform {
            [a / s, b / s, c / s, d / s]
        } else {
            [a, b, c, d]
        };
        let [m0, m1, m2, m3] = t.transform;
        data.transform = [
            lin[0] * m0 + lin[1] * m2,
            lin[0] * m1 + lin[1] * m3,
            lin[2] * m0 + lin[3] * m2,
            lin[2] * m1 + lin[3] * m3,
        ];
        // Close to the identity: exactly it (sharp hinted text again).
        if data
            .transform
            .iter()
            .zip(text::IDENTITY)
            .all(|(v, i)| (v - i).abs() < 1e-4)
        {
            data.transform = text::IDENTITY;
        }
        data.anchor = (moved.0 - offset.0 as f32, moved.1 - offset.1 as f32);
        let rendered = self.text_engine().render(&data);
        let Some(doc) = &mut self.doc else {
            return false;
        };
        let active = doc.active;
        doc.active = i;
        doc.set_active_text(data, rendered);
        doc.active = active;
        true
    }

    /// Image Size: the texts (already scaled) are rendered again at their
    /// new sizes, to stay sharp.
    pub(super) fn rerender_texts(&mut self) {
        let Some(doc) = &self.doc else { return };
        let texts: Vec<(usize, TextData)> = doc
            .layers
            .iter()
            .enumerate()
            .filter_map(|(i, l)| l.text.clone().map(|t| (i, t)))
            .collect();
        for (i, t) in texts {
            let rendered = self.text_engine().render(&t);
            if let Some(doc) = &mut self.doc {
                let active = doc.active;
                doc.active = i;
                doc.set_active_text(t, rendered);
                doc.active = active;
            }
        }
    }
}

/// Deletes the layer `id`, if it is still there.
fn delete_layer(doc: &mut Document, id: u64) {
    if let Some(i) = doc.layers.iter().position(|l| l.id == id) {
        doc.active = i;
        doc.delete_active();
    }
}

/// Canvas map of the Move tool's transform: `src` scaled to `frame`, then
/// turned by `angle` (degrees, clockwise) around its center.
pub(super) fn frame_map(src: Rect, frame: Rect, angle: f32) -> Affine {
    let (sx, sy) = (frame.width() / src.width(), frame.height() / src.height());
    let (s, c) = angle.to_radians().sin_cos();
    let center = frame.center();
    let v = frame.min - center - vec2(sx * src.min.x, sy * src.min.y);
    let t = center + vec2(c * v.x - s * v.y, s * v.x + c * v.y);
    Affine([c * sx, -s * sy, t.x, s * sx, c * sy, t.y])
}

// ------------------------------------------------------------ panel widgets

/// Copies into `s` the attributes that differ between `old` and `new` (an
/// edit in a panel: the other attributes of mixed characters stay).
pub(super) fn copy_changed(old: &CharStyle, new: &CharStyle, s: &mut CharStyle) {
    macro_rules! fields {
        ($($f:ident),*) => {
            $(if old.$f != new.$f {
                s.$f = new.$f.clone();
            })*
        };
    }
    fields!(
        family,
        weight,
        italic,
        stretch,
        size,
        leading,
        tracking,
        kerning,
        horizontal_scale,
        vertical_scale,
        baseline_shift,
        color,
        faux_bold,
        faux_italic,
        all_caps,
        small_caps,
        underline,
        strikethrough
    );
}

/// The same for paragraph attributes.
pub(super) fn copy_changed_paragraph(
    old: &ParagraphStyle,
    new: &ParagraphStyle,
    p: &mut ParagraphStyle,
) {
    macro_rules! fields {
        ($($f:ident),*) => {
            $(if old.$f != new.$f {
                p.$f = new.$f;
            })*
        };
    }
    fields!(
        align,
        indent_left,
        indent_right,
        indent_first,
        space_before,
        space_after
    );
}

/// Name of the font family (empty: the default font).
pub(super) fn family_label(family: &str) -> String {
    if family.is_empty() {
        t!("options-font-default")
    } else {
        family.to_owned()
    }
}

/// Font families, filtered by a search field (inside a drop-down).
pub(super) fn family_list(
    ui: &mut egui::Ui,
    families: &[String],
    filter: &mut String,
    family: &mut String,
) {
    ui.set_min_width(240.0);
    let search = ui.add(egui::TextEdit::singleline(filter).hint_text(t!("options-font-search")));
    // Type right away to filter.
    if ui.memory(|m| m.focused().is_none()) {
        search.request_focus();
    }
    let needle = filter.to_lowercase();
    let default = t!("options-font-default");
    let fonts = std::iter::once((String::new(), default.as_str())).chain(
        families
            .iter()
            .filter(|f| f.to_lowercase().contains(&needle))
            .map(|f| (f.clone(), f.as_str())),
    );
    for (f, label) in fonts {
        if ui.selectable_label(*family == f, label).clicked() {
            *family = f;
            ui.close();
        }
    }
}

/// Name of the style's face among `faces` (the nearest one).
pub(super) fn face_label(faces: &[text::Face], s: &CharStyle) -> String {
    let distance = |f: &&text::Face| {
        (f.italic != s.italic) as u32 * 10_000
            + (f.stretch as i32 - s.stretch as i32).unsigned_abs() * 1000
            + (f.weight as i32 - s.weight as i32).unsigned_abs()
    };
    faces
        .iter()
        .min_by_key(distance)
        .map_or_else(|| s.weight.to_string(), |f| f.name.clone())
}

/// The family's faces (inside a drop-down).
pub(super) fn face_list(ui: &mut egui::Ui, faces: &[text::Face], s: &mut CharStyle) {
    ui.set_min_width(160.0);
    let current = face_label(faces, s);
    for f in faces {
        if ui.selectable_label(f.name == current, &f.name).clicked() {
            (s.weight, s.italic, s.stretch) = (f.weight, f.italic, f.stretch);
            ui.close();
        }
    }
}

pub(super) fn antialias_name(aa: AntiAlias) -> String {
    match aa {
        AntiAlias::None => t!("type-aa-none"),
        AntiAlias::Sharp => t!("type-aa-sharp"),
        AntiAlias::Smooth => t!("type-aa-smooth"),
    }
}

pub(super) const ANTIALIAS: [AntiAlias; 3] = [AntiAlias::None, AntiAlias::Sharp, AntiAlias::Smooth];

/// Alignment buttons of the Paragraph panel.
pub(super) fn align_options() -> [(TextAlign, &'static str, String); 4] {
    [
        (
            TextAlign::Left,
            icon::TEXT_ALIGN_LEFT,
            t!("options-align-left"),
        ),
        (
            TextAlign::Center,
            icon::TEXT_ALIGN_CENTER,
            t!("options-align-center"),
        ),
        (
            TextAlign::Right,
            icon::TEXT_ALIGN_RIGHT,
            t!("options-align-right"),
        ),
        (
            TextAlign::Justify,
            icon::TEXT_ALIGN_JUSTIFY,
            t!("type-align-justify"),
        ),
    ]
}

impl AtelierApp {
    /// Applies an edit of the shown character style (`old` → `new`).
    pub(super) fn apply_char_edit(&mut self, old: &CharStyle, new: &CharStyle) {
        if old != new {
            let (old, new) = (old.clone(), new.clone());
            self.restyle_selection(move |s| copy_changed(&old, &new, s));
        }
    }

    /// Applies an edit of the shown paragraph style.
    pub(super) fn apply_paragraph_edit(&mut self, old: &ParagraphStyle, new: &ParagraphStyle) {
        if old != new {
            let (old, new) = (*old, *new);
            self.restyle_paragraphs(move |p| copy_changed_paragraph(&old, &new, p));
        }
    }

    /// The text of the active text layer replaced by `new` (typed in the
    /// Properties panel): the changed middle takes the style around it.
    pub(super) fn replace_text(&mut self, new: &str) {
        self.change_text(|data, edit| {
            let old = &data.text;
            let prefix = old
                .char_indices()
                .zip(new.chars())
                .find(|((_, a), b)| a != b)
                .map_or(old.len().min(new.len()), |((i, _), _)| i);
            let suffix = old[prefix..]
                .chars()
                .rev()
                .zip(new[prefix..].chars().rev())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a.len_utf8())
                .sum::<usize>();
            let removed = prefix..old.len() - suffix;
            let added = &new[prefix..new.len() - suffix];
            let style = data.style_at(prefix).clone();
            data.delete(removed);
            data.insert(prefix, added, &style);
            if let Some(e) = edit {
                e.place(prefix + added.len(), false);
            }
        });
    }
}

/// [`frame_map`] for a text: a frame of its proportions, give or take a
/// pixel of rounding, scales it evenly (its sizes change, not its
/// transformation).
pub(super) fn text_frame_map(src: Rect, frame: Rect, angle: f32) -> Affine {
    let even = Rect::from_min_size(frame.min, src.size() * (frame.height() / src.height()));
    if (even.width() - frame.width()).abs() <= 1.0 {
        frame_map(src, even, angle)
    } else {
        frame_map(src, frame, angle)
    }
}

impl AtelierApp {
    /// The text layers before a change of the whole canvas (Image Rotation,
    /// Flip Canvas): (layer id, text, position).
    pub(super) fn texts_before(&self) -> Vec<(u64, TextData, (i64, i64))> {
        self.doc.as_ref().map_or_else(Vec::new, |d| {
            d.layers
                .iter()
                .filter_map(|l| Some((l.id, l.text.clone()?, l.offset)))
                .collect()
        })
    }

    /// After it: the texts moved by the canvas map `map` stay texts,
    /// rendered again (the change itself is the undo step).
    pub(super) fn texts_after(&mut self, texts: Vec<(u64, TextData, (i64, i64))>, map: Affine) {
        for (id, t, old) in texts {
            let Some(doc) = &mut self.doc else { return };
            let Some(i) = doc.layers.iter().position(|l| l.id == id) else {
                continue;
            };
            let layer = &mut doc.layers[i];
            // The origin where it was, from the layer's new position.
            let anchor = (
                (old.0 - layer.offset.0) as f32 + t.anchor.0,
                (old.1 - layer.offset.1) as f32 + t.anchor.1,
            );
            layer.text = Some(TextData { anchor, ..t });
            self.transform_text(i, map);
        }
    }
}

/// Canvas map of Image Rotation: `degrees` clockwise around the center of
/// a canvas of `old` size, into one of `new` size.
pub(super) fn canvas_rotation(old: (u32, u32), new: (u32, u32), degrees: f32) -> Affine {
    let (s, c) = degrees.to_radians().sin_cos();
    let (ox, oy) = (old.0 as f32 / 2.0, old.1 as f32 / 2.0);
    let (nx, ny) = (new.0 as f32 / 2.0, new.1 as f32 / 2.0);
    Affine([c, -s, nx - (c * ox - s * oy), s, c, ny - (s * ox + c * oy)])
}

/// Canvas map of Flip Canvas.
pub(super) fn canvas_flip((w, h): (u32, u32), horizontal: bool) -> Affine {
    Affine(if horizontal {
        [-1.0, 0.0, w as f32, 0.0, 1.0, 0.0]
    } else {
        [1.0, 0.0, 0.0, 0.0, -1.0, h as f32]
    })
}
