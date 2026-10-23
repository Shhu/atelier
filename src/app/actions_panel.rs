//! Actions panel (Window › Actions, Alt+F9), like Photoshop's: the sets
//! and their actions in a tree, the steps of an action with their settings;
//! a step turned off, or played with its dialog (modal control); Stop,
//! Record and Play (from the selected step); the panel menu (Button Mode,
//! New Action / Set, Duplicate, Delete, Insert Stop, Action Options, Load,
//! Save, Replace and Reset Actions); the function keys of the actions.
//!
//! Commands record themselves where they are committed, with their final
//! settings (`record`, one call at each place). Playing runs the same code
//! with those settings, one step per frame: it waits for the dialogs of
//! the steps with modal control and for the work done in the background,
//! and says which step could not be played, and why.

use std::collections::VecDeque;

use super::dialogs::{DialogSpec, block, floating_dialog, gap_between_blocks, help};
use super::select_menu::ModifyKind;
use super::*;
use crate::actions::{
    self as act, Combine, Contents, FnKey, Library, MenuItem, Modify, Recorded, Resample, Sequence,
    Set, Shape, Step,
};
use egui::{Align2, FontId, Response};

/// Window › Actions: Alt+F9, like Photoshop.
pub(super) const SHORTCUT_ACTIONS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::ALT, Key::F9);

/// Rows of the tree.
const ROW: f32 = 26.0;
/// Width of the two toggle columns (step on, dialog).
const TOGGLE: f32 = 20.0;

/// File › Automate and Scripts, Window › Actions, Quick Export.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Command {
    /// Shows or hides the Actions panel.
    Panel,
    Batch,
    ImageProcessor,
    FitImage,
    QuickExport,
}

/// A row of the tree: a set, an action, a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    Set(usize),
    Action(usize, usize),
    Step(usize, usize, usize),
}

impl Row {
    fn set(self) -> usize {
        match self {
            Row::Set(s) | Row::Action(s, _) | Row::Step(s, _, _) => s,
        }
    }

    fn action(self) -> Option<(usize, usize)> {
        match self {
            Row::Set(_) => None,
            Row::Action(s, a) | Row::Step(s, a, _) => Some((s, a)),
        }
    }
}

/// The panel's windows.
enum PanelDialog {
    /// New Action (`editing: None`; recording starts on OK) or Action
    /// Options: name, set, function key (0: none).
    Action {
        name: String,
        set: usize,
        key: u8,
        shift: bool,
        ctrl: bool,
        editing: Option<(usize, usize)>,
    },
    /// New Set, or Set Options (renaming it).
    Set {
        name: String,
        editing: Option<usize>,
    },
    /// Insert Stop: its message, and whether playing may go on.
    Stop {
        message: String,
        allow_continue: bool,
    },
    /// Delete the selected set, action or step?
    Delete(Row),
    /// Replace the actions with the default ones (or add them)?
    Reset,
    /// Delete every set?
    ClearAll,
}

/// What stops the playing: a Stop step's message, or a step that failed.
enum Message {
    Stop { text: String, allow_continue: bool },
    Error { text: String },
}

/// A step to play, and the action it belongs to.
struct Queued {
    action: String,
    step: Step,
    /// Where it is in the panel.
    row: Row,
}

/// The step being played: its dialog open (`dialog`), or its work going
/// on; the document's state before it, to tell whether it did anything.
struct Current {
    title: String,
    action: String,
    row: Row,
    dialog: bool,
    /// Its dialog's OK came (the command was committed).
    committed: bool,
    edits: bool,
    before: Option<Before>,
}

/// A document's state before a step.
struct Before {
    doc: u64,
    status: String,
    depth: usize,
    step: u64,
}

/// Actions being played.
pub(super) struct Player {
    queue: VecDeque<Queued>,
    current: Option<Current>,
    /// Played by Batch: an error ends the playing and is handed to it.
    batch: bool,
    /// What went wrong (Batch reads it).
    pub(super) error: Option<String>,
    /// Name shown when done.
    name: String,
}

/// The Actions panel and what it records and plays.
pub(super) struct ActionsState {
    pub open: bool,
    pub library: Library,
    selected: Option<Row>,
    /// Action being recorded, and where the next step goes in it.
    recording: Option<(usize, usize)>,
    insert_at: usize,
    /// Commands run by other commands aren't recorded on their own.
    mute: u32,
    pub(super) player: Option<Player>,
    /// The actions as buttons.
    button_mode: bool,
    dialog: Option<PanelDialog>,
    message: Option<Message>,
    /// Record Again: the step whose dialog is open, to take its new
    /// settings.
    again: Option<Row>,
    /// Why the last action played by Batch stopped.
    batch_error: Option<String>,
    /// The actions file may be written (it was read, or set aside).
    saving: bool,
    /// For the status bar: a problem with the actions file.
    notice: Option<String>,
}

impl Default for ActionsState {
    fn default() -> Self {
        let load = Library::load();
        Self {
            open: false,
            library: load.library,
            saving: load.saving,
            notice: load.notice,
            selected: None,
            recording: None,
            insert_at: 0,
            mute: 0,
            player: None,
            button_mode: false,
            dialog: None,
            message: None,
            again: None,
            batch_error: None,
        }
    }
}

impl ActionsState {
    fn action(&self, (s, a): (usize, usize)) -> Option<&Sequence> {
        self.library.sets.get(s)?.actions.get(a)
    }

    fn action_mut(&mut self, (s, a): (usize, usize)) -> Option<&mut Sequence> {
        self.library.sets.get_mut(s)?.actions.get_mut(a)
    }

    /// The selection still names a row (after a change of the tree).
    fn valid(&self, row: Row) -> bool {
        let sets = &self.library.sets;
        match row {
            Row::Set(s) => s < sets.len(),
            Row::Action(s, a) => sets.get(s).is_some_and(|x| a < x.actions.len()),
            Row::Step(s, a, i) => self.action((s, a)).is_some_and(|x| i < x.steps.len()),
        }
    }

    fn changed(&mut self) {
        if self.selected.is_some_and(|r| !self.valid(r)) {
            self.selected = None;
        }
        if self.recording.is_some_and(|r| self.action(r).is_none()) {
            self.recording = None;
        }
        if self.saving
            && let Err(e) = self.library.save()
        {
            self.notice = Some(t!("smart3-actions-not-saved", error = e.to_string()));
        }
    }

    pub(super) fn playing(&self) -> bool {
        self.player.is_some()
    }

    /// `row` was removed (or a copy of it added after it): the recording
    /// stops if the action it records may have moved; within it, it goes
    /// on where it was.
    fn after_change(&mut self, row: Row, removed: bool) {
        match (row, self.recording) {
            (Row::Step(s, i, k), Some(r)) if r == (s, i) && k < self.insert_at => {
                if removed {
                    self.insert_at -= 1;
                } else {
                    self.insert_at += 1;
                }
            }
            (Row::Step(..), _) | (_, None) => {}
            _ => self.recording = None,
        }
    }

    /// Steps of the action being recorded (`None`: not recording).
    pub(super) fn recording_len(&self) -> Option<usize> {
        self.action(self.recording?).map(|a| a.steps.len())
    }

    pub(super) fn stop_recording(&mut self) {
        self.recording = None;
    }

    /// Records into action `at`, after its last step.
    #[cfg(test)]
    pub(super) fn start_recording(&mut self, at: (usize, usize)) {
        self.insert_at = self.action(at).map_or(0, |a| a.steps.len());
        self.recording = Some(at);
    }

    /// Why the last action played by Batch stopped.
    pub(super) fn take_batch_error(&mut self) -> Option<String> {
        self.batch_error.take()
    }
}

/// Steps with a dialog to show (modal control).
fn has_dialog(r: &Recorded) -> bool {
    match r {
        Recorded::Filter { filter } => filter.kind().has_settings(),
        Recorded::CameraRaw { .. }
        | Recorded::ImageSize { .. }
        | Recorded::FitImage { .. }
        | Recorded::CanvasSize { .. }
        | Recorded::RotateCanvas { .. }
        | Recorded::Adjust { .. }
        | Recorded::Fill { .. }
        | Recorded::ModifySelection { .. }
        | Recorded::Transform { .. }
        | Recorded::Place { .. }
        | Recorded::Export { .. }
        | Recorded::Liquify { .. }
        | Recorded::ConvertToProfile { .. }
        | Recorded::AssignProfile { .. }
        | Recorded::Grayscale => true,
        _ => false,
    }
}

/// A Free Transform's corners when nothing moves.
const IDENTITY_FRAME: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

/// Steps that change the document (not only the selection).
fn edits(r: &Recorded) -> bool {
    !matches!(
        r,
        Recorded::Select { .. }
            | Recorded::SelectAll
            | Recorded::Deselect
            | Recorded::Reselect
            | Recorded::InvertSelection
            | Recorded::ModifySelection { .. }
            | Recorded::Export { .. }
            | Recorded::Stop { .. }
            | Recorded::Menu {
                item: MenuItem::Copy
            }
    )
}

impl MenuItem {
    fn action(self) -> Action {
        match self {
            MenuItem::NewLayer => Action::NewLayer,
            MenuItem::DuplicateLayer => Action::Duplicate,
            MenuItem::DeleteLayer => Action::Delete,
            MenuItem::LayerViaCopy => Action::LayerFromSelection,
            MenuItem::MergeDown => Action::MergeDown,
            MenuItem::MergeVisible => Action::MergeVisible,
            MenuItem::FlattenImage => Action::FlattenImage,
            MenuItem::BringForward => Action::Raise,
            MenuItem::SendBackward => Action::Lower,
            MenuItem::BringToFront => Action::BringToFront,
            MenuItem::SendToBack => Action::SendToBack,
            MenuItem::NewGroup => Action::NewGroup,
            MenuItem::GroupLayers => Action::GroupLayers,
            MenuItem::UngroupLayers => Action::Ungroup,
            MenuItem::ClippingMask => Action::ToggleClipping,
            MenuItem::RevealAllMask => Action::AddMask(NewMask::RevealAll),
            MenuItem::HideAllMask => Action::AddMask(NewMask::HideAll),
            MenuItem::RevealSelectionMask => Action::AddMask(NewMask::RevealSelection),
            MenuItem::HideSelectionMask => Action::AddMask(NewMask::HideSelection),
            MenuItem::DeleteMask => Action::DeleteMask,
            MenuItem::ApplyMask => Action::ApplyMask,
            MenuItem::Rasterize => Action::Rasterize,
            MenuItem::Copy => Action::Copy,
            MenuItem::Cut => Action::Cut,
            MenuItem::Paste => Action::Paste,
            MenuItem::Clear => Action::ClearSelection,
            MenuItem::CropToSelection => Action::CropToSelection,
            MenuItem::CropToContent => Action::CropToContent,
            MenuItem::ConvertToSmartObject => Action::Smart(smart_ui::Command::Convert),
            MenuItem::ConvertForSmartFilters => Action::Smart(smart_ui::Command::ConvertForFilters),
        }
    }

    /// The menu item a recorded command comes from.
    fn of(action: Action) -> Option<MenuItem> {
        Some(match action {
            Action::NewLayer => MenuItem::NewLayer,
            Action::Duplicate => MenuItem::DuplicateLayer,
            Action::Delete => MenuItem::DeleteLayer,
            Action::LayerFromSelection => MenuItem::LayerViaCopy,
            Action::MergeDown => MenuItem::MergeDown,
            Action::MergeVisible => MenuItem::MergeVisible,
            Action::FlattenImage => MenuItem::FlattenImage,
            Action::Raise => MenuItem::BringForward,
            Action::Lower => MenuItem::SendBackward,
            Action::BringToFront => MenuItem::BringToFront,
            Action::SendToBack => MenuItem::SendToBack,
            Action::NewGroup => MenuItem::NewGroup,
            Action::GroupLayers => MenuItem::GroupLayers,
            Action::Ungroup => MenuItem::UngroupLayers,
            Action::ToggleClipping => MenuItem::ClippingMask,
            Action::AddMask(NewMask::RevealAll) => MenuItem::RevealAllMask,
            Action::AddMask(NewMask::HideAll) => MenuItem::HideAllMask,
            Action::AddMask(NewMask::RevealSelection) => MenuItem::RevealSelectionMask,
            Action::AddMask(NewMask::HideSelection) => MenuItem::HideSelectionMask,
            Action::DeleteMask => MenuItem::DeleteMask,
            Action::ApplyMask => MenuItem::ApplyMask,
            Action::Rasterize => MenuItem::Rasterize,
            // A cut is recorded as a copy, then the clear it runs.
            Action::Copy | Action::Cut => MenuItem::Copy,
            Action::Paste => MenuItem::Paste,
            Action::ClearSelection => MenuItem::Clear,
            Action::CropToSelection => MenuItem::CropToSelection,
            Action::CropToContent => MenuItem::CropToContent,
            Action::Smart(smart_ui::Command::Convert) => MenuItem::ConvertToSmartObject,
            Action::Smart(smart_ui::Command::ConvertForFilters) => MenuItem::ConvertForSmartFilters,
            _ => return None,
        })
    }

    /// Needs a selection.
    fn selects(self) -> bool {
        matches!(
            self,
            MenuItem::Clear
                | MenuItem::CropToSelection
                | MenuItem::RevealSelectionMask
                | MenuItem::HideSelectionMask
        )
    }

    fn label(self) -> String {
        match self {
            MenuItem::NewLayer => t!("menu-layer-new"),
            MenuItem::DuplicateLayer => t!("menu-layer-duplicate"),
            MenuItem::DeleteLayer => t!("menu-layer-delete"),
            MenuItem::LayerViaCopy => t!("menu-layer-via-copy"),
            MenuItem::MergeDown => t!("menu-layer-merge-down"),
            MenuItem::MergeVisible => t!("menu-layer-merge-visible"),
            MenuItem::FlattenImage => t!("menu-layer-flatten"),
            MenuItem::BringForward => t!("menu-layer-bring-forward"),
            MenuItem::SendBackward => t!("menu-layer-send-backward"),
            MenuItem::BringToFront => t!("menu-layer-bring-to-front"),
            MenuItem::SendToBack => t!("menu-layer-send-to-back"),
            MenuItem::NewGroup => t!("core-menu-new-group"),
            MenuItem::GroupLayers => t!("core-menu-group-layers"),
            MenuItem::UngroupLayers => t!("core-menu-ungroup-layers"),
            MenuItem::ClippingMask => t!("core-menu-create-clipping"),
            MenuItem::RevealAllMask => t!("core-mask-reveal-all"),
            MenuItem::HideAllMask => t!("core-mask-hide-all"),
            MenuItem::RevealSelectionMask => t!("core-mask-reveal-selection"),
            MenuItem::HideSelectionMask => t!("core-mask-hide-selection"),
            MenuItem::DeleteMask => t!("core-mask-delete"),
            MenuItem::ApplyMask => t!("core-mask-apply"),
            MenuItem::Rasterize => t!("vector-menu-rasterize"),
            MenuItem::Copy => t!("menu-edit-copy"),
            MenuItem::Cut => t!("menu-edit-cut"),
            MenuItem::Paste => t!("menu-edit-paste"),
            MenuItem::Clear => t!("menu-edit-clear"),
            MenuItem::CropToSelection => t!("menu-image-crop"),
            MenuItem::CropToContent => t!("menu-image-trim"),
            MenuItem::ConvertToSmartObject => t!("smart-menu-convert"),
            MenuItem::ConvertForSmartFilters => t!("smart-menu-convert-for-filters"),
        }
    }
}

pub(super) fn resample(filter: ResizeFilter) -> Resample {
    match filter {
        ResizeFilter::Nearest => Resample::Nearest,
        ResizeFilter::Bilinear => Resample::Bilinear,
        ResizeFilter::Bicubic => Resample::Bicubic,
        ResizeFilter::Lanczos => Resample::Lanczos,
    }
}

pub(super) fn resize_filter(r: Resample) -> ResizeFilter {
    match r {
        Resample::Nearest => ResizeFilter::Nearest,
        Resample::Bilinear => ResizeFilter::Bilinear,
        Resample::Bicubic => ResizeFilter::Bicubic,
        Resample::Lanczos => ResizeFilter::Lanczos,
    }
}

pub(super) fn contents(c: FillContent) -> Contents {
    match c {
        FillContent::Foreground => Contents::Foreground,
        FillContent::Background => Contents::Background,
        FillContent::Color => Contents::Color,
        FillContent::ContentAware => Contents::ContentAware,
        FillContent::Black => Contents::Black,
        FillContent::Gray => Contents::Gray,
        FillContent::White => Contents::White,
    }
}

fn fill_content(c: Contents) -> FillContent {
    match c {
        Contents::Foreground => FillContent::Foreground,
        Contents::Background => FillContent::Background,
        Contents::Color => FillContent::Color,
        Contents::ContentAware => FillContent::ContentAware,
        Contents::Black => FillContent::Black,
        Contents::Gray => FillContent::Gray,
        Contents::White => FillContent::White,
    }
}

pub(super) fn combine(m: SelectMode) -> Combine {
    match m {
        SelectMode::Replace => Combine::New,
        SelectMode::Add => Combine::Add,
        SelectMode::Subtract => Combine::Subtract,
        SelectMode::Intersect => Combine::Intersect,
    }
}

fn select_mode(c: Combine) -> SelectMode {
    match c {
        Combine::New => SelectMode::Replace,
        Combine::Add => SelectMode::Add,
        Combine::Subtract => SelectMode::Subtract,
        Combine::Intersect => SelectMode::Intersect,
    }
}

pub(super) fn modify(k: ModifyKind) -> Modify {
    match k {
        ModifyKind::Border => Modify::Border,
        ModifyKind::Smooth => Modify::Smooth,
        ModifyKind::Expand => Modify::Expand,
        ModifyKind::Contract => Modify::Contract,
        ModifyKind::Feather => Modify::Feather,
    }
}

fn modify_kind(m: Modify) -> ModifyKind {
    match m {
        Modify::Border => ModifyKind::Border,
        Modify::Smooth => ModifyKind::Smooth,
        Modify::Expand => ModifyKind::Expand,
        Modify::Contract => ModifyKind::Contract,
        Modify::Feather => ModifyKind::Feather,
    }
}

pub(super) fn rgb(c: Color32) -> [u8; 3] {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    [r, g, b]
}

/// The function key pressed, if an action could have it.
pub(super) fn fn_key_press(input: &mut egui::InputState) -> Option<FnKey> {
    const KEYS: [Key; 11] = [
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    let m = input.modifiers;
    if m.alt {
        return None;
    }
    let (i, _) = KEYS
        .iter()
        .enumerate()
        .find(|(_, k)| input.key_pressed(**k))?;
    Some(FnKey {
        number: i as u8 + 2,
        shift: m.shift,
        ctrl: m.command,
    })
}

/// "Shift+F5", as the menus write it.
fn key_label(ctx: &egui::Context, key: FnKey) -> String {
    let modifiers = Modifiers {
        shift: key.shift,
        command: key.ctrl,
        ..Modifiers::NONE
    };
    keys::combo(ctx, modifiers, &format!("F{}", key.number))
}

/// Function keys the app's own commands use.
fn key_taken(key: FnKey) -> bool {
    key.shift && !key.ctrl && matches!(key.number, 5 | 6)
}

impl AtelierApp {
    // ------------------------------------------------------------ Recording

    /// A command committed with its final settings: added to the action
    /// being recorded. While playing, it tells that the dialog of the step
    /// played was confirmed.
    pub(super) fn record(&mut self, command: Recorded) {
        let a = &mut self.actions;
        if a.mute > 0 {
            return;
        }
        if let Some(p) = &mut a.player {
            if let Some(c) = &mut p.current {
                c.committed = true;
            }
            return;
        }
        if let Some(Row::Step(s, i, k)) = a.again.take() {
            if let Some(step) = a.action_mut((s, i)).and_then(|x| x.steps.get_mut(k)) {
                step.command = command;
            }
            return a.changed();
        }
        let Some(at) = a.recording else { return };
        let insert = a.insert_at;
        let Some(action) = a.action_mut(at) else {
            a.recording = None;
            return;
        };
        let i = insert.min(action.steps.len());
        action.steps.insert(i, Step::new(command));
        action.expanded = true;
        a.insert_at = i + 1;
        a.selected = Some(Row::Step(at.0, at.1, i));
        a.changed();
    }

    /// The step a menu command without settings records (see
    /// [`MenuItem`]), taken before it runs.
    pub(super) fn recorded_action(&self, action: Action) -> Option<Recorded> {
        let command = match action {
            Action::SelectAll => Recorded::SelectAll,
            Action::Deselect => Recorded::Deselect,
            Action::Reselect => Recorded::Reselect,
            Action::InvertSelection => Recorded::InvertSelection,
            Action::FlipCanvas(horizontal) => Recorded::FlipCanvas { horizontal },
            Action::Adjust(AdjustKind::Invert) => Recorded::Adjust {
                adjustment: Adjustment::Invert,
            },
            Action::FillPrimary
            | Action::FillSecondary
            | Action::FillPrimaryKeep
            | Action::FillSecondaryKeep => Recorded::Fill {
                contents: if matches!(action, Action::FillPrimary | Action::FillPrimaryKeep) {
                    Contents::Foreground
                } else {
                    Contents::Background
                },
                color: [0, 0, 0],
                opacity: 1.0,
                preserve_transparency: matches!(
                    action,
                    Action::FillPrimaryKeep | Action::FillSecondaryKeep
                ),
            },
            // Without a selection, it duplicates the layer (recorded so).
            Action::LayerFromSelection if self.selection.is_none() => return None,
            _ => Recorded::Menu {
                item: MenuItem::of(action)?,
            },
        };
        self.doc.is_some().then_some(command)
    }

    /// A command that changed the document while recording without
    /// recording anything (`len`: the steps before it): the status says
    /// it isn't in the action.
    pub(super) fn tell_unrecorded(
        &mut self,
        action: Action,
        len: Option<usize>,
        before: Option<(u64, u64)>,
    ) {
        let after = self.doc.as_ref().map(|d| (d.uid(), d.history_step()));
        let changed = after != before && after.map(|a| a.0) == before.map(|b| b.0);
        // Undo and Redo aren't commands; a transform records when applied.
        let pending = matches!(action, Action::Undo | Action::Redo) || self.transform_pending();
        if len.is_some() && changed && !pending && self.actions.recording_len() == len {
            self.status = t!("smart3-actions-not-recorded");
        }
    }

    /// Records `command`, run from the history step `before` (document,
    /// step): a command that edits only once it changed the document.
    pub(super) fn record_done(&mut self, command: Recorded, before: Option<(u64, u64)>) {
        let after = self.doc.as_ref().map(|d| (d.uid(), d.history_step()));
        // Played: the step's failure is told by the player.
        if !edits(&command) || after != before || self.actions.playing() {
            self.record(command);
        }
    }

    pub(super) fn record_image_size(&mut self, d: ImageSizeDialog) {
        self.record(Recorded::ImageSize {
            width: d.width,
            height: d.height,
            resample: resample(d.filter),
        });
    }

    /// The Crop tool's frame, about to be applied.
    pub(super) fn record_crop(&mut self) {
        let (Some(r), Some(doc)) = (self.canvas_rect, &self.doc) else {
            return;
        };
        let r = to_pixel_rect(round_rect(r));
        if r != doc.full_rect() {
            self.record(Recorded::Crop {
                rect: [r.x, r.y, r.w as i64, r.h as i64],
            });
        }
    }

    /// A marquee's shape (`None`: the plain rectangle, already the
    /// selection), about to be made.
    pub(super) fn record_marquee(
        &mut self,
        ellipse: bool,
        mode: SelectMode,
        shape: Option<PixelRect>,
    ) {
        let edges = *self.sel.shape.edges(self.tool);
        let rect = match shape {
            Some(r) => r,
            None => match &self.selection {
                Some(s) => s.rect,
                None => return self.record(Recorded::Deselect),
            },
        };
        self.record(Recorded::Select {
            shape: if ellipse {
                Shape::Ellipse
            } else {
                Shape::Rectangle
            },
            rect: [rect.x, rect.y, rect.w as i64, rect.h as i64].map(|v| v as f64),
            percent: false,
            mode: combine(mode),
            feather: if shape.is_some() { edges.feather } else { 0.0 },
            anti_alias: edges.antialias,
        });
    }

    /// Free Transform, about to be applied.
    pub(super) fn record_transform(&mut self) {
        // A file being placed: the place and its transform, one step.
        if let Some((path, linked)) = self.placing_file() {
            let corners = self.recorded_frame().unwrap_or(IDENTITY_FRAME);
            return self.record(Recorded::Place {
                path,
                linked,
                corners,
            });
        }
        match self.recorded_frame() {
            Some(corners) => self.record(Recorded::Transform { corners }),
            // Applied as it was: still the OK of a played step's transform.
            None if self.actions.playing() => self.record(Recorded::Transform {
                corners: IDENTITY_FRAME,
            }),
            None => {}
        }
    }

    /// Runs `f` without recording what it commits.
    pub(super) fn muted(&mut self, f: impl FnOnce(&mut Self)) {
        self.actions.mute += 1;
        f(self);
        self.actions.mute -= 1;
    }

    // ------------------------------------------------------------ Playing

    /// Plays from `row`: a set's actions, an action, or an action from one
    /// of its steps.
    pub(super) fn play_row(&mut self, row: Row) {
        let a = &self.actions;
        let mut queue = VecDeque::new();
        let mut add = |s: usize, i: usize, from: usize| {
            let Some(action) = a.action((s, i)) else {
                return;
            };
            for (k, step) in action.steps.iter().enumerate().skip(from) {
                queue.push_back(Queued {
                    action: action.name.clone(),
                    step: step.clone(),
                    row: Row::Step(s, i, k),
                });
            }
        };
        let name = match row {
            Row::Set(s) => {
                let n = a.library.sets.get(s).map_or(0, |set| set.actions.len());
                (0..n).for_each(|i| add(s, i, 0));
                a.library.sets.get(s).map(|set| set.name.clone())
            }
            Row::Action(s, i) => {
                add(s, i, 0);
                a.action((s, i)).map(|x| x.name.clone())
            }
            Row::Step(s, i, k) => {
                add(s, i, k);
                a.action((s, i)).map(|x| x.name.clone())
            }
        };
        let Some(name) = name else { return };
        self.start_playing(queue, name, false);
    }

    fn start_playing(&mut self, queue: VecDeque<Queued>, name: String, batch: bool) {
        self.commit_pending();
        self.actions.player = Some(Player {
            queue,
            current: None,
            batch,
            error: None,
            name,
        });
    }

    /// The action selected in the panel.
    pub(super) fn actions_selected_action(&self) -> Option<(usize, usize)> {
        self.actions.selected.and_then(Row::action)
    }

    /// Plays an action for Batch (errors are handed to it).
    pub(super) fn play_for_batch(&mut self, at: (usize, usize)) -> bool {
        let Some(action) = self.actions.action(at) else {
            return false;
        };
        let name = action.name.clone();
        let queue = action
            .steps
            .iter()
            .enumerate()
            .map(|(k, step)| Queued {
                action: name.clone(),
                step: step.clone(),
                row: Row::Step(at.0, at.1, k),
            })
            .collect();
        self.start_playing(queue, name, true);
        true
    }

    /// The action with function key `key`.
    pub(super) fn play_key(&mut self, key: FnKey) {
        if self.actions.playing() {
            return;
        }
        if let Some((s, a)) = self.actions.library.with_key(key) {
            self.play_row(Row::Action(s, a));
        }
    }

    pub(super) fn stop_playing(&mut self) {
        self.actions.player = None;
        self.actions.message = None;
    }

    /// What a step waits for: a dialog, work in the background, a frame
    /// being prepared (a transform with its dialog).
    pub(super) fn actions_wait(&self) -> bool {
        self.modal_open()
            || self.select_busy()
            || self.subject_job.is_some()
            || (self.transform_pending()
                && self
                    .actions
                    .player
                    .as_ref()
                    .and_then(|p| p.current.as_ref())
                    .is_some_and(|c| c.dialog))
    }

    /// Plays the next step, once the previous one is done (every frame).
    pub(super) fn play_actions(&mut self) {
        if let Some(notice) = self.actions.notice.take() {
            self.status = notice;
        }
        // Record Again's dialog closed without OK.
        if self.actions.again.is_some() && !self.actions_wait() && !self.transform_pending() {
            self.actions.again = None;
        }
        // Esc stops it, like Photoshop (a dialog takes it for itself).
        if self.actions.playing()
            && !self.actions.player.as_ref().is_some_and(|p| p.batch)
            && !self.modal_open()
            && self.ctx.input(|i| i.key_pressed(Key::Escape))
        {
            let name = self.actions.player.as_ref().map(|p| p.name.clone());
            self.stop_playing();
            self.status = t!("actions-status-stopped", name = name.unwrap_or_default());
            return;
        }
        loop {
            if self.actions.message.is_some() || self.actions_wait() {
                return;
            }
            let Some(p) = &mut self.actions.player else {
                return;
            };
            if let Some(c) = p.current.take() {
                if let Some(error) = self.step_failed(&c) {
                    return self.step_error(&c.title, &c.action, c.row, error);
                }
                if c.dialog && !c.committed {
                    self.status = t!("actions-status-stopped", name = c.action.as_str());
                    return self.stop_playing();
                }
                // A frame to show the result.
                return;
            }
            let Some(q) = self
                .actions
                .player
                .as_mut()
                .and_then(|p| p.queue.pop_front())
            else {
                let Some(p) = self.actions.player.take() else {
                    return;
                };
                if p.batch {
                    self.actions.batch_error = p.error;
                } else {
                    self.status = t!("actions-status-played", name = p.name.as_str());
                }
                return;
            };
            if !q.step.enabled {
                continue;
            }
            let title = step_title(&q.step.command);
            let dialog = q.step.dialog && has_dialog(&q.step.command);
            let before = self.doc.as_ref().map(|d| Before {
                doc: d.uid(),
                status: self.status.clone(),
                depth: d.history().1,
                step: d.history_step(),
            });
            self.actions.selected = Some(q.row);
            match self.play_step(&q.step.command, dialog) {
                Ok(()) => {
                    if let Some(p) = &mut self.actions.player {
                        p.current = Some(Current {
                            title,
                            action: q.action,
                            row: q.row,
                            dialog,
                            committed: false,
                            edits: edits(&q.step.command) && !dialog,
                            before,
                        });
                    }
                }
                Err(error) => return self.step_error(&title, &q.action, q.row, error),
            }
            return;
        }
    }

    /// A step that changed nothing while saying why (a locked layer, no
    /// pixels…): its message.
    fn step_failed(&self, c: &Current) -> Option<String> {
        let (true, Some(b), Some(doc)) = (c.edits, &c.before, &self.doc) else {
            return None;
        };
        let depth = doc.history().1;
        let done = depth > b.depth || (depth == b.depth && doc.history_step() != b.step);
        (doc.uid() == b.doc && !done && self.status != b.status).then(|| self.status.clone())
    }

    /// Stops at a step that could not be played: says so (Batch takes the
    /// message instead).
    fn step_error(&mut self, title: &str, action: &str, row: Row, error: String) {
        let text = t!(
            "actions-error-step",
            step = title,
            action = action,
            error = error.as_str()
        );
        let Some(p) = &mut self.actions.player else {
            return;
        };
        if p.batch {
            p.error = Some(text);
            p.queue.clear();
            return;
        }
        self.actions.selected = Some(row);
        self.actions.message = Some(Message::Error { text });
    }

    /// Runs a step: at once, or its dialog filled with its settings.
    fn play_step(&mut self, command: &Recorded, dialog: bool) -> Result<(), String> {
        if let Recorded::Stop {
            message,
            allow_continue,
        } = command
        {
            self.actions.message = Some(Message::Stop {
                text: message.clone(),
                allow_continue: *allow_continue,
            });
            return Ok(());
        }
        let Some(doc) = &self.doc else {
            return Err(t!("actions-error-no-document"));
        };
        let has_selection = self.selection.is_some();
        let locked = doc.active_layer().locked;
        // A smart object takes filters as smart filters.
        let can_paint = doc.can_paint();
        let filterable = can_paint || self.smart_filters_apply();
        let canvas = doc.full_rect();
        let lock_error = || {
            t!(
                "status-layer-is-locked",
                name = doc.active_layer().name.as_str()
            )
        };
        match command {
            Recorded::ImageSize {
                width,
                height,
                resample,
            } => {
                crate::document::check_size(*width, *height).map_err(|e| e.to_string())?;
                let d = ImageSizeDialog {
                    width: *width,
                    height: *height,
                    keep_ratio: false,
                    filter: resize_filter(*resample),
                };
                if dialog {
                    self.image_size = Some(d);
                } else {
                    self.apply_image_size(d);
                }
            }
            Recorded::FitImage {
                width,
                height,
                dont_enlarge,
            } => {
                crate::document::check_size(*width, *height).map_err(|e| e.to_string())?;
                if dialog {
                    self.open_fit_image(*width, *height, *dont_enlarge);
                } else {
                    self.fit_image(*width, *height, *dont_enlarge);
                }
            }
            Recorded::CanvasSize {
                width,
                height,
                relative,
                anchor,
                extension,
            } => {
                let d = CanvasSizeDialog {
                    width: *width,
                    height: *height,
                    relative: *relative,
                    anchor: (anchor[0].min(2), anchor[1].min(2)),
                    extension: extension.map(fill_content),
                };
                if dialog {
                    self.canvas_size = Some(d);
                } else {
                    self.apply_canvas_size(d);
                }
            }
            Recorded::RotateCanvas { angle } => {
                if dialog {
                    self.rotate_dialog = Some(RotateDialog {
                        angle: angle.abs().rem_euclid(360.0),
                        clockwise: *angle >= 0.0,
                    });
                } else {
                    self.rotate_canvas(*angle);
                }
            }
            Recorded::FlipCanvas { horizontal } => self.run(Action::FlipCanvas(*horizontal)),
            Recorded::Crop { rect: [x, y, w, h] } => {
                if *w <= 0 || *h <= 0 {
                    return Err(t!("status-nothing-to-crop"));
                }
                let r = PixelRect {
                    x: *x,
                    y: *y,
                    w: *w as u32,
                    h: *h as u32,
                };
                if crate::document::intersect(r, canvas).is_none() {
                    return Err(t!("status-nothing-to-crop"));
                }
                self.canvas_rect = Some(to_rect(r));
                self.apply_canvas();
            }
            Recorded::Adjust { adjustment } => {
                if locked {
                    return Err(lock_error());
                }
                if !can_paint && self.smart_filters_apply() {
                    return Err(t!("smart-status-no-adjustments"));
                }
                if !can_paint {
                    return Err(t!("filter-no-pixels"));
                }
                if dialog {
                    self.adjust = Some(AdjustDialog {
                        adjustment: adjustment.clone(),
                        range: usize::from(adjustment.kind() == AdjustKind::ColorBalance),
                        point: None,
                        preview: true,
                    });
                } else {
                    let selection = self.selection.as_ref();
                    if let Some(doc) = &mut self.doc
                        && doc.adjust_active(adjustment, selection)
                    {
                        self.status = t!("dialog-adjust-applied", name = adjustment.title());
                        self.reset_pending();
                    }
                }
            }
            Recorded::Filter { filter } => {
                if locked {
                    return Err(lock_error());
                }
                if !filterable {
                    return Err(t!("filter-no-pixels"));
                }
                if dialog {
                    self.filters.remember(*filter);
                    self.open_filter(filter.kind());
                } else {
                    let filter = self.filter_ready(*filter);
                    self.start_filter(filter);
                }
            }
            Recorded::CameraRaw { settings } => {
                if locked {
                    return Err(lock_error());
                }
                if dialog {
                    self.camera_raw_filter(Some(settings.clone()));
                } else {
                    self.camera_raw_again(settings.clone());
                }
            }
            Recorded::Fill {
                contents,
                color,
                opacity,
                preserve_transparency,
            } => {
                if locked {
                    return Err(lock_error());
                }
                if *contents == Contents::ContentAware && !has_selection {
                    return Err(t!("retouch-needs-selection"));
                }
                let d = FillDialog {
                    content: fill_content(*contents),
                    color: Color32::from_rgb(color[0], color[1], color[2]),
                    opacity: opacity.clamp(0.0, 1.0),
                    preserve_transparency: *preserve_transparency,
                    adapt: true,
                };
                if dialog {
                    self.fill_dialog = Some(d);
                } else {
                    self.apply_fill(d);
                }
            }
            Recorded::Select {
                shape,
                rect,
                percent,
                mode,
                feather,
                anti_alias,
            } => {
                let r = if *percent {
                    let (w, h) = (canvas.w as f64 / 100.0, canvas.h as f64 / 100.0);
                    [rect[0] * w, rect[1] * h, rect[2] * w, rect[3] * h]
                } else {
                    *rect
                };
                let r = PixelRect {
                    x: r[0].round() as i64,
                    y: r[1].round() as i64,
                    w: r[2].round().max(0.0) as u32,
                    h: r[3].round().max(0.0) as u32,
                };
                if r.w == 0 || r.h == 0 {
                    return Err(t!("status-selection-empty"));
                }
                let mode = if has_selection {
                    select_mode(*mode)
                } else {
                    SelectMode::Replace
                };
                let ellipse = *shape == Shape::Ellipse;
                self.select_shape(r, ellipse, mode, feather.max(0.0), *anti_alias);
            }
            Recorded::SelectAll => self.run(Action::SelectAll),
            Recorded::Deselect => self.run(Action::Deselect),
            Recorded::Reselect => self.run(Action::Reselect),
            Recorded::InvertSelection => self.run(Action::InvertSelection),
            Recorded::ModifySelection {
                modify,
                amount,
                at_bounds,
            } => {
                if !has_selection {
                    return Err(t!("actions-error-no-selection"));
                }
                self.modify_selection_step(modify_kind(*modify), *amount, *at_bounds, dialog);
            }
            Recorded::Transform { corners } => {
                if !self.play_transform(*corners, dialog) {
                    return Err(self.status.clone());
                }
            }
            Recorded::Place {
                path,
                linked,
                corners,
            } => {
                self.place_path(path, *linked);
                if !self.transform_pending() || !self.set_transform_frame(*corners) {
                    return Err(self.status.clone());
                }
                if !dialog {
                    self.apply_transform();
                }
            }
            Recorded::Menu { item } => {
                if item.selects() && !has_selection {
                    return Err(t!("actions-error-no-selection"));
                }
                let action = item.action();
                if action.modifies_active() && locked {
                    return Err(lock_error());
                }
                self.run(action);
            }
            Recorded::Export { options, folder } => {
                if dialog {
                    self.open_export(options.clone());
                } else {
                    self.export_into(options, folder.as_deref())?;
                }
            }
            Recorded::Liquify { mesh } => {
                let mesh = crate::liquify::Mesh::from_bytes(mesh)
                    .ok_or_else(|| t!("liquify-status-not-a-mesh"))?;
                if locked {
                    return Err(lock_error());
                }
                if !filterable {
                    return Err(t!("filter-no-pixels"));
                }
                if dialog {
                    self.open_liquify_with(Some(&mesh));
                } else {
                    self.start_liquify(Arc::new(mesh), None);
                }
            }
            Recorded::ConvertToProfile {
                profile,
                intent,
                bpc,
            } => {
                let to = crate::cms::Profile::from_icc(Arc::new(profile.clone()))
                    .ok_or_else(|| t!("color-status-not-a-profile"))?;
                self.convert_to_profile(to, *intent, *bpc, dialog);
            }
            Recorded::AssignProfile { profile } => {
                let profile = match profile {
                    Some(icc) => Some(
                        crate::cms::Profile::from_icc(Arc::new(icc.clone()))
                            .ok_or_else(|| t!("color-status-not-a-profile"))?,
                    ),
                    None => None,
                };
                self.assign_profile(profile, dialog);
            }
            Recorded::Grayscale => self.grayscale_mode(dialog),
            Recorded::Stop { .. } => unreachable!("shown above"),
            Recorded::Unknown(_) => return Err(t!("actions-error-unknown")),
        }
        Ok(())
    }

    /// Window › Actions and the commands of File › Automate and Scripts.
    pub(super) fn automate_command(&mut self, command: Command) {
        match command {
            Command::Panel => self.actions.open = !self.actions.open,
            Command::Batch => self.open_batch(),
            Command::ImageProcessor => self.open_image_processor(),
            Command::FitImage => {
                if let Some(doc) = &self.doc {
                    let (w, h) = (doc.width(), doc.height());
                    self.open_fit_image(w, h, false);
                }
            }
            Command::QuickExport => self.quick_export(),
        }
    }

    // ------------------------------------------------------------ Panel

    /// The panel's windows block the rest, like the other dialogs.
    pub(super) fn actions_modal(&self) -> bool {
        self.actions.dialog.is_some() || self.actions.message.is_some()
    }

    pub(super) fn actions_windows(&mut self, ctx: &egui::Context) {
        self.actions_message_window(ctx);
        self.actions_dialog_window(ctx);
        if self.actions.open {
            self.actions_panel_window(ctx);
        }
    }

    fn actions_panel_window(&mut self, ctx: &egui::Context) {
        let mut click: Option<PanelClick> = None;
        let pos = dialogs::panel_pos(ctx, "actions");
        let recording = self.actions.recording;
        let playing = self.actions.playing();
        let has_doc = self.doc.is_some();
        let button_mode = self.actions.button_mode;
        let selected = self.actions.selected;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 300.0,
                resize: Some(ROW * 12.0 + 34.0),
                ..DialogSpec::new("actions", t!("actions-title"), pos)
            },
            |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if button_mode {
                    action_buttons(ui, &self.actions.library, has_doc && !playing, &mut click);
                } else {
                    // The buttons below keep their place.
                    egui::ScrollArea::vertical()
                        .max_height(ui.available_height() - 34.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            tree(ui, &self.actions.library, selected, recording, &mut click);
                        });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let can_record = selected.is_some() && !playing;
                    let buttons = [
                        (
                            icon::STOP,
                            t!("actions-stop-tip"),
                            PanelClick::Stop,
                            recording.is_some() || playing,
                            false,
                        ),
                        (
                            icon::RECORD,
                            t!("actions-record-tip"),
                            PanelClick::Record,
                            can_record && recording.is_none(),
                            recording.is_some(),
                        ),
                        (
                            icon::PLAY,
                            t!("actions-play-tip"),
                            PanelClick::Play,
                            has_doc && selected.is_some() && !playing && recording.is_none(),
                            playing,
                        ),
                        (
                            icon::FOLDER_SIMPLE_PLUS,
                            t!("actions-new-set-tip"),
                            PanelClick::NewSet,
                            !playing,
                            false,
                        ),
                        (
                            icon::PLUS_SQUARE,
                            t!("actions-new-action-tip"),
                            PanelClick::NewAction,
                            !playing,
                            false,
                        ),
                        (
                            icon::TRASH,
                            t!("actions-delete-tip"),
                            PanelClick::Delete,
                            selected.is_some() && !playing,
                            false,
                        ),
                    ];
                    for (glyph, tip, c, enabled, on) in buttons {
                        let mut on = on;
                        let r = if enabled {
                            ui_kit::icon_toggle(ui, glyph, &tip, &mut on)
                        } else {
                            ui_kit::icon_button(ui, glyph, &tip, false)
                        };
                        if enabled && r.clicked() {
                            click = Some(c);
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let menu =
                            ui_kit::icon_button(ui, icon::LIST, &t!("actions-menu-tip"), true);
                        egui::Popup::menu(&menu).show(|ui| {
                            panel_menu(ui, button_mode, selected, recording, playing, &mut click);
                        });
                    });
                });
            },
        );
        if out.cancel {
            self.actions.open = false;
        }
        if let Some(click) = click {
            self.panel_click(click);
        }
    }

    fn panel_click(&mut self, click: PanelClick) {
        let a = &mut self.actions;
        let selected = a.selected;
        match click {
            PanelClick::Select(row) => {
                a.selected = Some(row);
                if let Some((s, i)) = row.action()
                    && a.recording == Some((s, i))
                {
                    a.insert_at = match row {
                        Row::Step(_, _, k) => k + 1,
                        _ => a.action((s, i)).map_or(0, |x| x.steps.len()),
                    };
                }
            }
            PanelClick::Expand(row) => match row {
                Row::Set(s) => {
                    if let Some(set) = a.library.sets.get_mut(s) {
                        set.expanded = !set.expanded;
                    }
                }
                Row::Action(s, i) => {
                    if let Some(x) = a.action_mut((s, i)) {
                        x.expanded = !x.expanded;
                    }
                }
                Row::Step(s, i, k) => {
                    if let Some(step) = a.action_mut((s, i)).and_then(|x| x.steps.get_mut(k)) {
                        step.expanded = !step.expanded;
                    }
                }
            },
            PanelClick::Toggle(row, dialog) => {
                toggle(&mut a.library, row, dialog);
                a.changed();
            }
            PanelClick::Options(row) => self.row_options(row),
            PanelClick::PlayAction(s, i) => self.play_row(Row::Action(s, i)),
            PanelClick::Stop => {
                if a.recording.take().is_none() {
                    self.stop_playing();
                }
            }
            PanelClick::Record => match selected.and_then(Row::action) {
                Some(at) => {
                    a.insert_at = match selected {
                        Some(Row::Step(_, _, k)) => k + 1,
                        _ => a.action(at).map_or(0, |x| x.steps.len()),
                    };
                    a.recording = Some(at);
                }
                None => self.new_action_dialog(),
            },
            PanelClick::Play => {
                if let Some(row) = selected {
                    self.play_row(row);
                }
            }
            PanelClick::NewSet => {
                let n = a.library.sets.len() + 1;
                a.dialog = Some(PanelDialog::Set {
                    name: t!("actions-set-default-name", n = n),
                    editing: None,
                });
            }
            PanelClick::NewAction => self.new_action_dialog(),
            PanelClick::Delete => {
                if let Some(row) = selected {
                    a.dialog = Some(PanelDialog::Delete(row));
                }
            }
            PanelClick::Duplicate => {
                if let Some(row) = selected {
                    duplicate(&mut a.library, row);
                    a.after_change(row, false);
                    a.changed();
                }
            }
            PanelClick::InsertStop => {
                a.dialog = Some(PanelDialog::Stop {
                    message: String::new(),
                    allow_continue: true,
                });
            }
            PanelClick::ButtonMode => a.button_mode = !a.button_mode,
            PanelClick::ClearAll => a.dialog = Some(PanelDialog::ClearAll),
            PanelClick::Reset => a.dialog = Some(PanelDialog::Reset),
            PanelClick::Load(replace) => self.load_actions(replace),
            PanelClick::Save => self.save_actions(),
        }
    }

    fn new_action_dialog(&mut self) {
        let a = &mut self.actions;
        if a.library.sets.is_empty() {
            a.library.sets.push(Set {
                name: t!("actions-set-default-name", n = 1),
                actions: Vec::new(),
                expanded: true,
            });
        }
        let set = a.selected.map_or(0, Row::set);
        let n = a.library.sets[set].actions.len() + 1;
        a.dialog = Some(PanelDialog::Action {
            name: t!("actions-action-default-name", n = n),
            set,
            key: 0,
            shift: false,
            ctrl: false,
            editing: None,
        });
    }

    /// Double-click: Action Options, Set Options, or the step's dialog
    /// played alone (like Photoshop, to change its settings).
    fn row_options(&mut self, row: Row) {
        let a = &mut self.actions;
        match row {
            Row::Set(s) => {
                if let Some(set) = a.library.sets.get(s) {
                    a.dialog = Some(PanelDialog::Set {
                        name: set.name.clone(),
                        editing: Some(s),
                    });
                }
            }
            Row::Action(s, i) => {
                if let Some(x) = a.action((s, i)) {
                    let key = x.key;
                    a.dialog = Some(PanelDialog::Action {
                        name: x.name.clone(),
                        set: s,
                        key: key.map_or(0, |k| k.number),
                        shift: key.is_some_and(|k| k.shift),
                        ctrl: key.is_some_and(|k| k.ctrl),
                        editing: Some((s, i)),
                    });
                }
            }
            Row::Step(s, i, k) => self.record_again(Row::Step(s, i, k)),
        }
    }

    /// Record Again (double-click on a step): its dialog, filled with its
    /// settings; confirmed, they replace the step's.
    fn record_again(&mut self, row: Row) {
        let Row::Step(s, i, k) = row else { return };
        let Some(command) = self
            .actions
            .action((s, i))
            .and_then(|x| x.steps.get(k))
            .map(|st| st.command.clone())
            .filter(has_dialog)
        else {
            return;
        };
        self.commit_pending();
        self.actions.again = Some(row);
        if let Err(error) = self.play_step(&command, true) {
            self.actions.again = None;
            self.status = error;
        }
    }

    fn load_actions(&mut self, replace: bool) {
        let title = if replace {
            t!("actions-menu-replace")
        } else {
            t!("actions-menu-load")
        };
        let filter = t!("actions-file-filter");
        let Some(path) = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(filter, &["json"])
            .pick_file()
        else {
            return;
        };
        match act::read(&path) {
            Ok(sets) => {
                let a = &mut self.actions;
                if replace {
                    a.library.sets = sets;
                    a.recording = None;
                } else {
                    a.library.sets.extend(sets);
                }
                a.changed();
                self.status = t!("actions-status-loaded", path = path.display().to_string());
            }
            Err(e) => self.status = t!("actions-status-load-failed", error = e.to_string()),
        }
    }

    /// Save Actions: the selected set, in a file of its own.
    fn save_actions(&mut self) {
        let Some(set) = self
            .actions
            .selected
            .and_then(|r| self.actions.library.sets.get(r.set()))
            .cloned()
        else {
            return;
        };
        let filter = t!("actions-file-filter");
        let Some(path) = rfd::FileDialog::new()
            .set_title(t!("actions-menu-save"))
            .add_filter(filter, &["json"])
            .set_file_name(format!("{}.json", set.name))
            .save_file()
        else {
            return;
        };
        self.status = match act::write(&path, &[set]) {
            Ok(()) => t!("actions-status-saved", path = path.display().to_string()),
            Err(e) => t!("actions-status-save-failed", error = e.to_string()),
        };
    }

    /// A Stop step's message, or the step that could not be played.
    fn actions_message_window(&mut self, ctx: &egui::Context) {
        let Some(message) = &self.actions.message else {
            return;
        };
        let (text, can_continue) = match message {
            Message::Stop {
                text,
                allow_continue,
            } => (text.clone(), *allow_continue),
            Message::Error { text } => (text.clone(), true),
        };
        let error = matches!(message, Message::Error { .. });
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: Align2::CENTER_CENTER,
                ok: if can_continue {
                    t!("actions-continue")
                } else {
                    t!("actions-stop")
                },
                other: can_continue.then(|| t!("actions-stop")),
                cancel: false,
                ..DialogSpec::new("actions_message", t!("actions-title"), center)
            },
            |ui| {
                ui.horizontal(|ui| {
                    let glyph = if error { icon::WARNING } else { icon::HAND };
                    ui.label(
                        egui::RichText::new(glyph)
                            .size(22.0)
                            .color(color_for(error)),
                    );
                    ui.add(egui::Label::new(text.as_str()).wrap());
                });
            },
        );
        if out.ok && can_continue {
            self.actions.message = None;
        } else if out.ok || out.other || out.cancel {
            // Stopped: Play goes on from the next step.
            let next = self
                .actions
                .player
                .as_ref()
                .and_then(|p| p.queue.front())
                .map(|q| q.row);
            self.stop_playing();
            if let Some(row) = next {
                self.actions.selected = Some(row);
            }
        }
    }

    fn actions_dialog_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.actions.dialog.take() else {
            return;
        };
        let center = ctx.content_rect().center();
        let sets: Vec<String> = self
            .actions
            .library
            .sets
            .iter()
            .map(|s| s.name.clone())
            .collect();
        let (title, ok) = match &dialog {
            PanelDialog::Action { editing: None, .. } => {
                (t!("actions-new-action-title"), t!("actions-record"))
            }
            PanelDialog::Action { .. } => (t!("actions-options-title"), t!("dialog-ok")),
            PanelDialog::Set { editing: None, .. } => {
                (t!("actions-new-set-title"), t!("dialog-ok"))
            }
            PanelDialog::Set { .. } => (t!("actions-set-options-title"), t!("dialog-ok")),
            PanelDialog::Stop { .. } => (t!("actions-insert-stop-title"), t!("dialog-ok")),
            PanelDialog::Delete(_) | PanelDialog::Reset | PanelDialog::ClearAll => {
                (t!("actions-title"), t!("dialog-ok"))
            }
        };
        let taken = |key: u8, shift: bool, ctrl: bool| {
            key_taken(FnKey {
                number: key,
                shift,
                ctrl,
            })
        };
        let ok_enabled = match &dialog {
            PanelDialog::Action {
                name,
                key,
                shift,
                ctrl,
                ..
            } => !name.trim().is_empty() && !taken(*key, *shift, *ctrl),
            PanelDialog::Set { name, .. } => !name.trim().is_empty(),
            _ => true,
        };
        let other = matches!(dialog, PanelDialog::Reset).then(|| t!("actions-append"));
        let delete_text = match &dialog {
            PanelDialog::Delete(row) => Some(self.delete_question(*row)),
            _ => None,
        };
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: Align2::CENTER_CENTER,
                ok,
                ok_enabled,
                other,
                ..DialogSpec::new("actions_dialog", title, center)
            },
            |ui| match &mut dialog {
                PanelDialog::Action {
                    name,
                    set,
                    key,
                    shift,
                    ctrl,
                    ..
                } => {
                    let id = egui::Id::new("action_name");
                    let edit = ui_kit::text_field(ui, &t!("actions-name"), name, id);
                    if ui.memory(|m| m.focused().is_none()) {
                        edit.response.request_focus();
                    }
                    gap_between_blocks(ui);
                    let width = ui.available_width();
                    block(ui, |ui| {
                        ui_kit::field_label(ui, &t!("actions-set"));
                        let shown = sets.get(*set).cloned().unwrap_or_default();
                        ui_kit::combo(ui, "action_set", None, &shown, width, |ui| {
                            for (i, s) in sets.iter().enumerate() {
                                ui.selectable_value(set, i, s);
                            }
                        });
                    });
                    gap_between_blocks(ui);
                    block(ui, |ui| {
                        ui_kit::field_label(ui, &t!("actions-function-key"));
                        let none = t!("actions-key-none");
                        let shown = if *key == 0 {
                            none.clone()
                        } else {
                            format!("F{key}")
                        };
                        ui.horizontal(|ui| {
                            ui_kit::combo(ui, "action_key", None, &shown, 96.0, |ui| {
                                ui.selectable_value(key, 0, none.as_str());
                                for k in 2..=12u8 {
                                    ui.selectable_value(key, k, format!("F{k}"));
                                }
                            });
                            if *key != 0 {
                                ui_kit::checkbox(
                                    ui,
                                    shift,
                                    &keys::modifiers(ui.ctx(), Modifiers::SHIFT),
                                );
                                ui_kit::checkbox(
                                    ui,
                                    ctrl,
                                    &keys::modifiers(ui.ctx(), Modifiers::COMMAND),
                                );
                            }
                        });
                        if *key != 0 && taken(*key, *shift, *ctrl) {
                            help(ui, &t!("actions-key-taken"));
                        }
                    });
                }
                PanelDialog::Set { name, .. } => {
                    let id = egui::Id::new("set_name");
                    let edit = ui_kit::text_field(ui, &t!("actions-name"), name, id);
                    if ui.memory(|m| m.focused().is_none()) {
                        edit.response.request_focus();
                    }
                }
                PanelDialog::Stop {
                    message,
                    allow_continue,
                } => {
                    ui_kit::field_label(ui, &t!("actions-stop-message"));
                    ui.add(
                        egui::TextEdit::multiline(message)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    );
                    gap_between_blocks(ui);
                    ui_kit::checkbox(ui, allow_continue, &t!("actions-allow-continue"));
                }
                PanelDialog::Delete(_) => {
                    ui.label(delete_text.clone().unwrap_or_default());
                }
                PanelDialog::Reset => {
                    ui.label(t!("actions-reset-question"));
                }
                PanelDialog::ClearAll => {
                    ui.label(t!("smart3-actions-clear-all-question"));
                }
            },
        );
        if !out.ok && !out.other && !out.cancel {
            self.actions.dialog = Some(dialog);
            return;
        }
        if out.cancel {
            return;
        }
        let a = &mut self.actions;
        match dialog {
            PanelDialog::Action {
                name,
                set,
                key,
                shift,
                ctrl,
                editing,
            } => {
                let key = (key != 0).then_some(FnKey {
                    number: key,
                    shift,
                    ctrl,
                });
                // A key taken by another action moves to this one.
                if let Some(key) = key {
                    for x in a.library.sets.iter_mut().flat_map(|s| s.actions.iter_mut()) {
                        if x.key == Some(key) {
                            x.key = None;
                        }
                    }
                }
                let name = name.trim().to_owned();
                let set = set.min(a.library.sets.len().saturating_sub(1));
                match editing {
                    Some(at) => {
                        if let Some(x) = a.action_mut(at) {
                            x.name = name;
                            x.key = key;
                        }
                        // Moved to another set.
                        if at.0 != set
                            && let Some(x) =
                                a.library.sets.get_mut(at.0).map(|s| s.actions.remove(at.1))
                        {
                            a.library.sets[set].actions.push(x);
                            a.selected =
                                Some(Row::Action(set, a.library.sets[set].actions.len() - 1));
                            a.recording = None;
                        }
                    }
                    None => {
                        let Some(target) = a.library.sets.get_mut(set) else {
                            return;
                        };
                        // After the selected action, like Photoshop.
                        let at = match a.selected {
                            Some(Row::Action(s, i) | Row::Step(s, i, _)) if s == set => i + 1,
                            _ => target.actions.len(),
                        };
                        target.actions.insert(
                            at,
                            Sequence {
                                name,
                                key,
                                steps: Vec::new(),
                                expanded: true,
                            },
                        );
                        target.expanded = true;
                        a.selected = Some(Row::Action(set, at));
                        a.recording = Some((set, at));
                        a.insert_at = 0;
                    }
                }
            }
            PanelDialog::Set { name, editing } => {
                let name = name.trim().to_owned();
                match editing.and_then(|s| a.library.sets.get_mut(s)) {
                    Some(set) => set.name = name,
                    None => {
                        a.library.sets.push(Set {
                            name,
                            actions: Vec::new(),
                            expanded: true,
                        });
                        a.selected = Some(Row::Set(a.library.sets.len() - 1));
                    }
                }
            }
            PanelDialog::Stop {
                message,
                allow_continue,
            } => {
                let step = Step::new(Recorded::Stop {
                    message,
                    allow_continue,
                });
                let Some(row) = a.selected else { return };
                let Some(at) = row.action() else { return };
                let i = match row {
                    Row::Step(_, _, k) => k + 1,
                    _ => a.action(at).map_or(0, |x| x.steps.len()),
                };
                if let Some(x) = a.action_mut(at) {
                    x.steps.insert(i.min(x.steps.len()), step);
                    x.expanded = true;
                    a.selected = Some(Row::Step(at.0, at.1, i));
                    if a.recording == Some(at) {
                        a.insert_at = i + 1;
                    }
                }
            }
            PanelDialog::Delete(row) => {
                delete(&mut a.library, row);
                a.after_change(row, true);
                a.selected = None;
            }
            PanelDialog::Reset => {
                let defaults = Library::defaults().sets;
                if out.other {
                    a.library.sets.extend(defaults);
                } else {
                    a.library.sets = defaults;
                    a.recording = None;
                }
            }
            PanelDialog::ClearAll => {
                a.library.sets.clear();
                a.selected = None;
            }
        }
        a.changed();
    }

    fn delete_question(&self, row: Row) -> String {
        let a = &self.actions;
        match row {
            Row::Set(s) => {
                let name = a.library.sets.get(s).map_or("", |x| x.name.as_str());
                t!("actions-delete-set-question", name = name)
            }
            Row::Action(s, i) => {
                let name = a.action((s, i)).map_or("", |x| x.name.as_str());
                t!("actions-delete-action-question", name = name)
            }
            Row::Step(s, i, k) => {
                let title = a
                    .action((s, i))
                    .and_then(|x| x.steps.get(k))
                    .map(|st| step_title(&st.command))
                    .unwrap_or_default();
                t!("actions-delete-step-question", name = title.as_str())
            }
        }
    }
}

fn color_for(error: bool) -> Color32 {
    if error {
        Color32::from_rgb(0xe5, 0xa0, 0x3c)
    } else {
        ui_kit::color::ACCENT_TEXT
    }
}

/// What a click in the panel asks for.
#[derive(Clone, Copy, Debug)]
enum PanelClick {
    Select(Row),
    Expand(Row),
    /// The step on / off (`false`), or its dialog (`true`).
    Toggle(Row, bool),
    /// Double-click on a row.
    Options(Row),
    PlayAction(usize, usize),
    Stop,
    Record,
    Play,
    NewSet,
    NewAction,
    Delete,
    Duplicate,
    InsertStop,
    ButtonMode,
    ClearAll,
    Reset,
    /// Load Actions, or Replace Actions (`true`).
    Load(bool),
    Save,
}

/// The panel menu.
fn panel_menu(
    ui: &mut egui::Ui,
    button_mode: bool,
    selected: Option<Row>,
    recording: Option<(usize, usize)>,
    playing: bool,
    click: &mut Option<PanelClick>,
) {
    let idle = !playing;
    let action = selected.and_then(Row::action).is_some();
    let mut mode = button_mode;
    if ui
        .checkbox(&mut mode, t!("actions-menu-button-mode"))
        .clicked()
    {
        *click = Some(PanelClick::ButtonMode);
    }
    ui.separator();
    let mut item = |ui: &mut egui::Ui, label: String, enabled: bool, c: PanelClick| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            *click = Some(c);
        }
    };
    item(
        ui,
        t!("actions-menu-new-action"),
        idle,
        PanelClick::NewAction,
    );
    item(ui, t!("actions-menu-new-set"), idle, PanelClick::NewSet);
    item(
        ui,
        t!("actions-menu-duplicate"),
        idle && selected.is_some(),
        PanelClick::Duplicate,
    );
    item(
        ui,
        t!("actions-menu-delete"),
        idle && selected.is_some(),
        PanelClick::Delete,
    );
    item(
        ui,
        t!("actions-menu-play"),
        idle && selected.is_some(),
        PanelClick::Play,
    );
    ui.separator();
    item(
        ui,
        t!("actions-menu-record"),
        idle && action && recording.is_none(),
        PanelClick::Record,
    );
    let again = selected.is_some_and(|row| matches!(row, Row::Step(..)));
    item(
        ui,
        t!("actions-menu-record-again"),
        idle && again && recording.is_none(),
        PanelClick::Options(selected.unwrap_or(Row::Set(0))),
    );
    item(
        ui,
        t!("actions-menu-insert-stop"),
        idle && action,
        PanelClick::InsertStop,
    );
    ui.separator();
    let options = matches!(selected, Some(Row::Set(_) | Row::Action(..)));
    let row = selected.unwrap_or(Row::Set(0));
    item(
        ui,
        t!("actions-menu-options"),
        idle && options,
        PanelClick::Options(row),
    );
    ui.separator();
    item(ui, t!("actions-menu-clear-all"), idle, PanelClick::ClearAll);
    item(ui, t!("actions-menu-reset"), idle, PanelClick::Reset);
    item(ui, t!("actions-menu-load"), idle, PanelClick::Load(false));
    item(ui, t!("actions-menu-replace"), idle, PanelClick::Load(true));
    item(
        ui,
        t!("actions-menu-save"),
        idle && selected.is_some(),
        PanelClick::Save,
    );
}

/// Button Mode: one button per action, playing it.
fn action_buttons(
    ui: &mut egui::Ui,
    library: &Library,
    enabled: bool,
    click: &mut Option<PanelClick>,
) {
    let width = (ui.available_width() - 4.0) / 2.0;
    let actions: Vec<_> = library
        .sets
        .iter()
        .enumerate()
        .flat_map(|(s, set)| set.actions.iter().enumerate().map(move |(i, a)| (s, i, a)))
        .collect();
    if actions.is_empty() {
        ui_kit::hint(ui, t!("actions-empty"));
    }
    for pair in actions.chunks(2) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for &(s, i, a) in pair {
                // The shortcut in the tooltip, with the whole name.
                let key = a.key.map_or(String::new(), |k| key_label(ui.ctx(), k));
                let button = ui_kit::keyed_button(
                    ui,
                    None,
                    &a.name,
                    &key,
                    Kind::Normal,
                    Some(width),
                    enabled,
                );
                if button.clicked() {
                    *click = Some(PanelClick::PlayAction(s, i));
                }
            }
        });
    }
}

/// The sets, their actions and the actions' steps.
fn tree(
    ui: &mut egui::Ui,
    library: &Library,
    selected: Option<Row>,
    recording: Option<(usize, usize)>,
    click: &mut Option<PanelClick>,
) {
    if library.sets.is_empty() {
        ui_kit::hint(ui, t!("actions-empty"));
    }
    for (s, set) in library.sets.iter().enumerate() {
        let steps = || set.actions.iter().flat_map(|a| a.steps.iter());
        let row = Row::Set(s);
        let r = tree_row(
            ui,
            TreeRow {
                depth: 0,
                glyph: icon::FOLDER_SIMPLE,
                name: &set.name,
                key: None,
                expanded: Some(set.expanded),
                on: state(steps().map(|st| st.enabled)),
                dialog: dialogs(steps()),
                selected: selected == Some(row),
                recording: false,
            },
        );
        row_clicks(&r, row, click);
        if !set.expanded {
            continue;
        }
        for (i, action) in set.actions.iter().enumerate() {
            let row = Row::Action(s, i);
            let key = action.key.map(|k| key_label(ui.ctx(), k));
            let r = tree_row(
                ui,
                TreeRow {
                    depth: 1,
                    glyph: icon::PLAY,
                    name: &action.name,
                    key: key.as_deref(),
                    expanded: Some(action.expanded),
                    on: state(action.steps.iter().map(|st| st.enabled)),
                    dialog: dialogs(action.steps.iter()),
                    selected: selected == Some(row),
                    recording: recording == Some((s, i)),
                },
            );
            row_clicks(&r, row, click);
            if !action.expanded {
                continue;
            }
            for (k, step) in action.steps.iter().enumerate() {
                let row = Row::Step(s, i, k);
                let details = step_details(&step.command);
                let title = step_title(&step.command);
                let r = tree_row(
                    ui,
                    TreeRow {
                        depth: 2,
                        glyph: "",
                        name: &title,
                        key: None,
                        expanded: (!details.is_empty()).then_some(step.expanded),
                        on: Some(step.enabled),
                        dialog: has_dialog(&step.command).then_some(Some(step.dialog)),
                        selected: selected == Some(row),
                        recording: false,
                    },
                );
                row_clicks(&r, row, click);
                if step.expanded {
                    for line in details {
                        detail_row(ui, &line);
                    }
                }
            }
        }
    }
}

/// Red: some steps off (or some dialogs), the action being recorded.
const MIXED: Color32 = Color32::from_rgb(0xe0, 0x5a, 0x4a);

/// The dialogs of `steps` (those that have one): shown, not, some only;
/// `None` without any.
fn dialogs<'a>(steps: impl Iterator<Item = &'a Step>) -> Option<Option<bool>> {
    let shown: Vec<bool> = steps
        .filter(|s| has_dialog(&s.command))
        .map(|s| s.dialog)
        .collect();
    (!shown.is_empty()).then(|| state(shown.into_iter()))
}

/// On if all are, off if none is, `None` if mixed (or empty: on).
fn state(values: impl Iterator<Item = bool>) -> Option<bool> {
    let (mut on, mut off) = (false, false);
    for v in values {
        on |= v;
        off |= !v;
    }
    match (on, off) {
        (true, true) => None,
        (false, true) => Some(false),
        _ => Some(true),
    }
}

fn row_clicks(r: &TreeResponse, row: Row, click: &mut Option<PanelClick>) {
    if r.on.clicked() {
        *click = Some(PanelClick::Toggle(row, false));
    } else if r.dialog.as_ref().is_some_and(Response::clicked) {
        *click = Some(PanelClick::Toggle(row, true));
    } else if r.caret.as_ref().is_some_and(Response::clicked) {
        *click = Some(PanelClick::Expand(row));
    } else if r.row.double_clicked() {
        *click = Some(PanelClick::Options(row));
    } else if r.row.clicked() {
        *click = Some(PanelClick::Select(row));
    }
}

struct TreeRow<'a> {
    depth: usize,
    glyph: &'a str,
    name: &'a str,
    key: Option<&'a str>,
    /// Has a caret, open or not.
    expanded: Option<bool>,
    /// The step on (or all of those inside); `None`: some only.
    on: Option<bool>,
    /// Its dialog shown (`Some(None)`: some of those inside only); no
    /// column without one.
    dialog: Option<Option<bool>>,
    selected: bool,
    recording: bool,
}

struct TreeResponse {
    row: Response,
    on: Response,
    dialog: Option<Response>,
    caret: Option<Response>,
}

/// A row: the on / off column, the dialog column, the caret, the name.
fn tree_row(ui: &mut egui::Ui, r: TreeRow) -> TreeResponse {
    use ui_kit::color;
    let width = ui.available_width();
    let (rect, row) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
    let painter = ui.painter().clone();
    if r.selected {
        painter.rect_filled(rect, 6.0, color::ACCENT_SOFT);
    } else if row.hovered() {
        painter.rect_filled(rect, 6.0, color::BG3);
    }
    let cell = |i: f32| Rect::from_min_size(rect.min + vec2(i * TOGGLE, 0.0), vec2(TOGGLE, ROW));
    let id = row.id;
    let on_rect = cell(0.0);
    let on = ui.interact(on_rect, id.with("on"), Sense::click());
    let mark = match r.on {
        Some(true) => (icon::CHECK, color::TEXT),
        Some(false) => ("", color::TEXT),
        // Some steps off: the mark in red, like Photoshop's.
        None => (icon::CHECK, MIXED),
    };
    painter.text(
        on_rect.center(),
        Align2::CENTER_CENTER,
        mark.0,
        FontId::proportional(13.0),
        mark.1,
    );
    let on = on.on_hover_text(t!("actions-toggle-tip"));
    let dialog_rect = cell(1.0);
    let dialog = r.dialog.map(|shown| {
        let response = ui.interact(dialog_rect, id.with("dialog"), Sense::click());
        let fg = match shown {
            Some(true) => color::TEXT,
            Some(false) => color::DISABLED,
            None => MIXED,
        };
        painter.text(
            dialog_rect.center(),
            Align2::CENTER_CENTER,
            icon::APP_WINDOW,
            FontId::proportional(13.0),
            fg,
        );
        response.on_hover_text(t!("actions-dialog-tip"))
    });
    let x = rect.left() + 2.0 * TOGGLE + 4.0 + r.depth as f32 * 14.0;
    let caret = r.expanded.map(|open| {
        let c = Rect::from_min_size(pos2(x, rect.top()), vec2(16.0, ROW));
        let response = ui.interact(c, id.with("caret"), Sense::click());
        let glyph = if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        };
        painter.text(
            c.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(11.0),
            color::MUTED,
        );
        response
    });
    let mut text_x = x + 18.0;
    if !r.glyph.is_empty() {
        let glyph = if r.recording { icon::RECORD } else { r.glyph };
        let fg = if r.recording { MIXED } else { color::MUTED };
        painter.text(
            pos2(text_x + 7.0, rect.center().y),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(13.0),
            fg,
        );
        text_x += 20.0;
    }
    let key = r.key.map(|key| {
        let galley = painter.layout_no_wrap(key.to_owned(), ui_kit::mono(11.0), color::MUTED);
        let at = pos2(
            rect.right() - 8.0 - galley.size().x,
            rect.center().y - galley.size().y / 2.0,
        );
        (galley, at)
    });
    let right = key
        .as_ref()
        .map_or(rect.right() - 6.0, |(_, at)| at.x - 8.0);
    let clip = Rect::from_min_max(pos2(text_x, rect.top()), pos2(right, rect.bottom()));
    let text = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
    text.text(
        pos2(text_x, rect.center().y),
        Align2::LEFT_CENTER,
        r.name,
        FontId::proportional(ui_kit::TEXT_SIZE),
        if r.on == Some(false) {
            color::DISABLED
        } else {
            color::TEXT
        },
    );
    if let Some((galley, at)) = key {
        painter.galley(at, galley, color::MUTED);
    }
    let name = r.name.to_owned();
    row.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, r.selected, &name)
    });
    TreeResponse {
        row,
        on,
        dialog,
        caret,
    }
}

/// A setting of an open step.
fn detail_row(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    let x = rect.left() + 2.0 * TOGGLE + 4.0 + 3.0 * 14.0 + 18.0;
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(12.0),
        ui_kit::color::MUTED,
    );
}

/// Turns the steps of `row` on or off (`dialog`: their dialogs).
fn toggle(library: &mut Library, row: Row, dialog: bool) {
    let mut steps: Vec<&mut Step> = match row {
        Row::Set(s) => library
            .sets
            .get_mut(s)
            .map(|set| {
                set.actions
                    .iter_mut()
                    .flat_map(|a| a.steps.iter_mut())
                    .collect()
            })
            .unwrap_or_default(),
        Row::Action(s, a) => library
            .sets
            .get_mut(s)
            .and_then(|set| set.actions.get_mut(a))
            .map(|a| a.steps.iter_mut().collect())
            .unwrap_or_default(),
        Row::Step(s, a, k) => library
            .sets
            .get_mut(s)
            .and_then(|set| set.actions.get_mut(a))
            .and_then(|a| a.steps.get_mut(k))
            .into_iter()
            .collect(),
    };
    if dialog {
        steps.retain(|st| has_dialog(&st.command));
    }
    let value = |st: &Step| if dialog { st.dialog } else { st.enabled };
    // All on → all off; otherwise all on.
    let to = !steps.iter().all(|st| value(st));
    for st in steps {
        if dialog {
            st.dialog = to;
        } else {
            st.enabled = to;
        }
    }
}

/// A copy of the row, after it.
fn duplicate(library: &mut Library, row: Row) {
    match row {
        Row::Set(s) => {
            if let Some(set) = library.sets.get(s) {
                let mut copy = set.clone();
                copy.name = t!("actions-copy-name", name = set.name.as_str());
                // A function key plays one action only.
                for action in &mut copy.actions {
                    action.key = None;
                }
                library.sets.insert(s + 1, copy);
            }
        }
        Row::Action(s, a) => {
            if let Some(set) = library.sets.get_mut(s)
                && let Some(action) = set.actions.get(a)
            {
                let mut copy = action.clone();
                copy.name = t!("actions-copy-name", name = action.name.as_str());
                copy.key = None;
                set.actions.insert(a + 1, copy);
            }
        }
        Row::Step(s, a, k) => {
            if let Some(action) = library
                .sets
                .get_mut(s)
                .and_then(|set| set.actions.get_mut(a))
                && let Some(step) = action.steps.get(k)
            {
                let copy = step.clone();
                action.steps.insert(k + 1, copy);
            }
        }
    }
}

fn delete(library: &mut Library, row: Row) {
    match row {
        Row::Set(s) if s < library.sets.len() => {
            library.sets.remove(s);
        }
        Row::Action(s, a) => {
            if let Some(set) = library.sets.get_mut(s)
                && a < set.actions.len()
            {
                set.actions.remove(a);
            }
        }
        Row::Step(s, a, k) => {
            if let Some(action) = library
                .sets
                .get_mut(s)
                .and_then(|set| set.actions.get_mut(a))
                && k < action.steps.len()
            {
                action.steps.remove(k);
            }
        }
        Row::Set(_) => {}
    }
}

// ------------------------------------------------------------ Names

/// A step's name in the panel (and in its messages).
pub(super) fn step_title(r: &Recorded) -> String {
    match r {
        Recorded::ImageSize { .. } => t!("dialog-image-size-title"),
        Recorded::FitImage { .. } => t!("actions-fit-image-title"),
        Recorded::CanvasSize { .. } => t!("dialog-canvas-size-title"),
        Recorded::RotateCanvas { .. } => t!("actions-step-rotate"),
        Recorded::FlipCanvas { horizontal: true } => t!("menu-image-flip-horizontal"),
        Recorded::FlipCanvas { horizontal: false } => t!("menu-image-flip-vertical"),
        Recorded::Crop { .. } => t!("actions-step-crop"),
        Recorded::Adjust { adjustment } => adjustment.title(),
        Recorded::Filter { filter } => filters_ui::title(filter.kind()),
        Recorded::CameraRaw { .. } => t!("raw-filter-title"),
        Recorded::Fill { .. } => t!("dialog-fill-title"),
        Recorded::Select { .. } => t!("actions-step-select"),
        Recorded::SelectAll => t!("actions-step-select-all"),
        Recorded::Deselect => t!("menu-select-deselect"),
        Recorded::Reselect => t!("menu-select-reselect"),
        Recorded::InvertSelection => t!("actions-step-inverse"),
        Recorded::ModifySelection { modify, .. } => modify_kind(*modify)
            .menu_label()
            .trim_end_matches('…')
            .to_owned(),
        Recorded::Transform { .. } => t!("menu-edit-free-transform"),
        Recorded::Place { linked: true, .. } => t!("smart-hist-place-linked"),
        Recorded::Place { .. } => t!("smart-hist-place-embedded"),
        Recorded::Menu { item } => item.label(),
        Recorded::Export { .. } => t!("actions-step-export"),
        Recorded::Liquify { .. } => t!("liquify-title"),
        Recorded::ConvertToProfile { .. } => t!("color-convert-title"),
        Recorded::AssignProfile { .. } => t!("color-assign-title"),
        Recorded::Grayscale => t!("color-menu-mode-grayscale"),
        Recorded::Stop { .. } => t!("actions-step-stop"),
        Recorded::Unknown(value) => {
            let name = value.get("command").and_then(|c| c.as_str()).unwrap_or("?");
            t!("actions-step-unknown", name = name)
        }
    }
}

fn px(v: impl std::fmt::Display) -> String {
    format!("{v} px")
}

fn pct(v: f64) -> String {
    i18n::percent(i18n::short_decimal(v))
}

fn deg(v: f64) -> String {
    format!("{}°", i18n::short_decimal(v))
}

fn line(label: &str, value: impl Into<String>) -> String {
    t!("words3-label-value", label = label, value = value.into())
}

fn yes_no(on: bool) -> String {
    if on {
        t!("actions-yes")
    } else {
        t!("actions-no")
    }
}

/// A step's settings, one per line.
pub(super) fn step_details(r: &Recorded) -> Vec<String> {
    let width = t!("optbar-width-short");
    let height = t!("optbar-height-short");
    match r {
        Recorded::ImageSize {
            width: w,
            height: h,
            resample,
        } => vec![
            line(&width, px(w)),
            line(&height, px(h)),
            line(
                &t!("dialog-image-size-resample"),
                resize_filter(*resample).label(),
            ),
        ],
        Recorded::FitImage {
            width: w,
            height: h,
            dont_enlarge,
        } => vec![
            line(&width, px(w)),
            line(&height, px(h)),
            line(&t!("actions-dont-enlarge"), yes_no(*dont_enlarge)),
        ],
        Recorded::CanvasSize {
            width: w,
            height: h,
            relative,
            ..
        } => vec![
            line(&width, px(w)),
            line(&height, px(h)),
            line(&t!("dialog-canvas-size-relative"), yes_no(*relative)),
        ],
        Recorded::RotateCanvas { angle } => {
            vec![line(&t!("dialog-rotate-angle"), deg(f64::from(*angle)))]
        }
        Recorded::Crop { rect: [x, y, w, h] } => {
            vec![format!("{x}, {y} · {w} × {h} px")]
        }
        Recorded::Adjust { adjustment } => adjustment_details(adjustment),
        Recorded::Filter { filter } => filter_details(filter),
        Recorded::Fill {
            contents,
            opacity,
            preserve_transparency,
            ..
        } => vec![
            line(&t!("dialog-fill-contents"), fill_content(*contents).label()),
            line(&t!("dialog-fill-opacity"), pct(f64::from(*opacity) * 100.0)),
            line(
                &t!("dialog-fill-preserve-transparency"),
                yes_no(*preserve_transparency),
            ),
        ],
        Recorded::Select {
            shape,
            rect,
            percent,
            feather,
            ..
        } => {
            let shape = match shape {
                Shape::Rectangle => t!("tool-select"),
                Shape::Ellipse => t!("sel-tool-ellipse"),
            };
            let v = |v: f64| {
                if *percent {
                    pct(v)
                } else {
                    px(i18n::short_decimal(v))
                }
            };
            vec![
                shape,
                format!(
                    "{}, {} · {} × {}",
                    v(rect[0]),
                    v(rect[1]),
                    v(rect[2]),
                    v(rect[3])
                ),
                line(
                    &t!("sel-feather-radius"),
                    px(i18n::short_decimal(f64::from(*feather))),
                ),
            ]
        }
        Recorded::ModifySelection {
            amount, at_bounds, ..
        } => vec![
            px(i18n::short_decimal(f64::from(*amount))),
            line(&t!("sel-modify-at-bounds"), yes_no(*at_bounds)),
        ],
        Recorded::Transform { corners } | Recorded::Place { corners, .. } => {
            let d = |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]).hypot(b[1] - a[1]);
            let angle = (corners[1][1] - corners[0][1])
                .atan2(corners[1][0] - corners[0][0])
                .to_degrees();
            let mut lines = vec![
                line(&width, pct(d(corners[0], corners[1]) * 100.0)),
                line(&height, pct(d(corners[0], corners[3]) * 100.0)),
                line(&t!("dialog-rotate-angle"), deg(angle)),
            ];
            if let Recorded::Place { path, .. } = r {
                lines.insert(0, path.display().to_string());
            }
            lines
        }
        Recorded::Export { options, folder } => {
            let mut lines = vec![
                line(&t!("file-export-format"), options.format.name()),
                line(&t!("actions-scale"), pct(f64::from(options.scale))),
            ];
            if options.format == crate::document::ExportFormat::Jpeg {
                lines.push(line(
                    &t!("file-export-quality"),
                    options.quality.to_string(),
                ));
            }
            let folder = folder.as_ref().map_or_else(
                || t!("actions-document-folder"),
                |f| f.display().to_string(),
            );
            lines.push(line(&t!("actions-folder"), folder));
            lines
        }
        Recorded::ConvertToProfile {
            profile,
            intent,
            bpc,
        } => vec![
            profile_name(Some(profile)),
            intent.label(),
            line(&t!("color-bpc"), yes_no(*bpc)),
        ],
        Recorded::AssignProfile { profile } => vec![profile_name(profile.as_ref())],
        Recorded::Stop { message, .. } => vec![message.clone()],
        _ => Vec::new(),
    }
}

/// A recorded profile's name (`None`: Don't Color Manage).
fn profile_name(icc: Option<&Vec<u8>>) -> String {
    match icc {
        Some(icc) => crate::cms::Profile::from_icc(Arc::new(icc.clone()))
            .map_or_else(|| "?".to_owned(), |p| color_ui::shown_name(&p)),
        None => t!("color-assign-none"),
    }
}

fn adjustment_details(a: &Adjustment) -> Vec<String> {
    let n = |v: f32| i18n::short_decimal(f64::from((v * 100.0).round()));
    match a {
        Adjustment::BrightnessContrast(bc) => vec![
            line(&t!("dialog-adjust-brightness"), n(bc.brightness)),
            line(&t!("dialog-adjust-contrast"), n(bc.contrast)),
        ],
        Adjustment::HueSaturation(hs) => vec![
            line(&t!("dialog-adjust-hue"), deg(f64::from(hs.hue))),
            line(&t!("dialog-adjust-saturation"), n(hs.saturation)),
            line(&t!("dialog-adjust-lightness"), n(hs.lightness)),
            line(&t!("dialog-adjust-colorize"), yes_no(hs.colorize)),
        ],
        Adjustment::Vibrance(v) => vec![
            line(&t!("dialog-adjust-vibrance"), n(v.vibrance)),
            line(&t!("dialog-adjust-saturation"), n(v.saturation)),
        ],
        Adjustment::PhotoFilter(f) => vec![line(
            &t!("core-photo-filter-density"),
            pct(f64::from(f.density) * 100.0),
        )],
        Adjustment::Posterize(p) => vec![line(&t!("core-posterize-levels"), p.levels.to_string())],
        Adjustment::Threshold(th) => vec![line(&t!("core-threshold-level"), th.level.to_string())],
        _ => Vec::new(),
    }
}

fn filter_details(f: &crate::filters::Filter) -> Vec<String> {
    use crate::filters::Filter as F;
    let n = |v: f32| i18n::short_decimal(f64::from(v));
    let radius = |r: f32| line(&t!("filter-radius"), px(n(r)));
    let amount = |a: f32| line(&t!("filter-amount"), pct(f64::from(a)));
    let threshold = |v: f32| line(&t!("filter-threshold"), n(v));
    match *f {
        F::GaussianBlur { radius: r }
        | F::BoxBlur { radius: r }
        | F::Median { radius: r }
        | F::HighPass { radius: r } => vec![radius(r)],
        F::MotionBlur { angle, distance } => vec![
            line(&t!("filter-angle"), deg(f64::from(angle))),
            line(&t!("filter-distance"), px(n(distance))),
        ],
        F::SurfaceBlur {
            radius: r,
            threshold: th,
        }
        | F::DustAndScratches {
            radius: r,
            threshold: th,
        } => vec![radius(r), threshold(th)],
        F::UnsharpMask {
            amount: a,
            radius: r,
            threshold: th,
        } => vec![amount(a), radius(r), threshold(th)],
        F::SmartSharpen(s) => vec![amount(s.amount), radius(s.radius)],
        F::AddNoise(a) => vec![amount(a.amount)],
        F::ReduceNoise(r) => vec![line(&t!("filter-strength"), n(r.strength))],
        F::Offset(o) => vec![
            line(&t!("filter-offset-horizontal"), px(n(o.horizontal))),
            line(&t!("filter-offset-vertical"), px(n(o.vertical))),
        ],
        F::Minimum(m) | F::Maximum(m) => vec![radius(m.radius)],
        F::Emboss {
            angle,
            height,
            amount: a,
        } => vec![
            line(&t!("filter-angle"), deg(f64::from(angle))),
            line(&t!("filter-height"), px(n(height))),
            amount(a),
        ],
        F::Mosaic { cell } => vec![line(&t!("filter-cell-size"), px(n(cell)))],
        F::FindEdges | F::Clouds(_) => Vec::new(),
    }
}

impl AtelierApp {
    /// Capture test: `what` open (the panel with an action opened, Batch…).
    #[cfg(test)]
    pub(super) fn capture_automate(&mut self, what: &str) {
        match what {
            "actions" | "actions-buttons" => {
                self.actions.open = true;
                self.actions.button_mode = what == "actions-buttons";
                let a = &mut self.actions;
                if let Some(action) = a.action_mut((0, 0)) {
                    action.expanded = true;
                    action.key = Some(FnKey {
                        number: 2,
                        shift: true,
                        ctrl: false,
                    });
                    action.steps[1].expanded = true;
                    action.steps[0].enabled = false;
                }
                a.selected = Some(Row::Step(0, 0, 1));
                a.recording = Some((0, 1));
            }
            "batch" => self.open_batch(),
            "image-processor" => self.open_image_processor(),
            "export-as" => self.run(Action::ExportAs),
            _ => self.automate_command(Command::FitImage),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::{Filter, FilterKind};
    use egui_kittest::Harness;
    use image::{Rgba, RgbaImage};

    /// A 120 × 80 gradient document.
    fn app() -> Harness<'static, AtelierApp> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                let image = RgbaImage::from_fn(120, 80, |x, y| {
                    Rgba([(x * 2) as u8, (y * 3) as u8, 200 - x as u8, 255])
                });
                app.doc = Some(Document::new(image, None));
                app
            });
        h.run_steps(2);
        h
    }

    fn flat(h: &Harness<'static, AtelierApp>) -> RgbaImage {
        h.state().doc.as_ref().unwrap().flatten()
    }

    /// Runs frames until `done` (work in the background, steps played).
    fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
        for _ in 0..1000 {
            h.run_steps(1);
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let message = match &h.state().actions.message {
            Some(Message::Error { text } | Message::Stop { text, .. }) => text.clone(),
            None => String::new(),
        };
        panic!("not done: {message} ({})", h.state().status);
    }

    fn idle(app: &AtelierApp) -> bool {
        !app.actions.playing() && !app.actions_wait()
    }

    /// One set with one action of `steps`, selected.
    fn with_action(h: &mut Harness<'static, AtelierApp>, steps: Vec<Step>) {
        let a = &mut h.state_mut().actions;
        a.library.sets = vec![Set {
            name: "Test".into(),
            actions: vec![Sequence {
                name: "A".into(),
                key: None,
                steps,
                expanded: true,
            }],
            expanded: true,
        }];
        a.selected = Some(Row::Action(0, 0));
    }

    fn play(h: &mut Harness<'static, AtelierApp>) {
        h.state_mut().panel_click(PanelClick::Play);
        settle(h, idle);
    }

    fn invert() -> Step {
        Step::new(Recorded::Adjust {
            adjustment: Adjustment::Invert,
        })
    }

    #[test]
    fn camera_raw_is_recorded_then_played() {
        let mut a = app();
        let original = flat(&a);
        with_action(&mut a, Vec::new());
        a.state_mut().panel_click(PanelClick::Record);
        let settings = crate::camera_raw::Settings {
            exposure: -1.0,
            ..Default::default()
        };
        a.state_mut().camera_raw_again(settings.clone());
        settle(&mut a, |app| !app.camera_raw.busy());
        a.state_mut().panel_click(PanelClick::Stop);
        let steps = &a.state().actions.library.sets[0].actions[0].steps;
        assert_eq!(steps[0].command, Recorded::CameraRaw { settings });
        let expected = flat(&a);
        assert_ne!(expected, original);
        a.state_mut().undo();
        play(&mut a);
        assert_eq!(flat(&a), expected);
    }

    #[test]
    fn recorded_steps_play_back_the_same_after_saving() {
        // Recorded while run as usual: Image Size, Hue/Saturation and
        // Gaussian Blur, from their windows.
        let mut a = app();
        with_action(&mut a, Vec::new());
        a.state_mut().panel_click(PanelClick::Record);
        a.state_mut().apply_image_size(ImageSizeDialog {
            width: 60,
            height: 40,
            keep_ratio: true,
            filter: ResizeFilter::Bicubic,
        });
        a.state_mut().run(Action::Adjust(AdjustKind::HueSaturation));
        a.state_mut().adjust.as_mut().unwrap().adjustment =
            Adjustment::HueSaturation(crate::pixel_ops::HueSaturation {
                hue: 40.0,
                saturation: -0.3,
                lightness: 0.1,
                colorize: false,
            });
        a.run_steps(1);
        a.key_press(egui::Key::Enter);
        a.run_steps(1);
        let blur = Filter::GaussianBlur { radius: 3.0 };
        a.state_mut().filters.remember(blur);
        a.state_mut().run(Action::Filter(FilterKind::GaussianBlur));
        a.run_steps(1);
        a.key_press(egui::Key::Enter);
        settle(&mut a, |app| !app.filters.busy());
        a.state_mut().panel_click(PanelClick::Stop);
        let steps = &a.state().actions.library.sets[0].actions[0].steps;
        assert!(matches!(
            steps.iter().map(|s| &s.command).collect::<Vec<_>>()[..],
            [
                Recorded::ImageSize {
                    width: 60,
                    height: 40,
                    ..
                },
                Recorded::Adjust { .. },
                Recorded::Filter {
                    filter: Filter::GaussianBlur { .. }
                }
            ]
        ));
        let expected = flat(&a);
        assert_eq!(expected.dimensions(), (60, 40));

        // Saved, read back, played on the same image.
        let dir = std::env::temp_dir().join("atelier-actions-play");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recorded.json");
        act::write(&path, &a.state().actions.library.sets).unwrap();
        let mut b = app();
        with_action(&mut b, Vec::new());
        b.state_mut().actions.library.sets = act::read(&path).unwrap();
        play(&mut b);
        assert_eq!(flat(&b), expected);
        assert!(b.state().actions.message.is_none());
        // One undo step each.
        assert_eq!(b.state().doc.as_ref().unwrap().history().1, 3);
    }

    #[test]
    fn steps_turned_off_are_skipped() {
        let mut h = app();
        let original = flat(&h);
        with_action(
            &mut h,
            vec![
                Step::new(Recorded::FlipCanvas { horizontal: true }),
                invert(),
            ],
        );
        h.state_mut()
            .panel_click(PanelClick::Toggle(Row::Step(0, 0, 0), false));
        play(&mut h);
        let mut inverted = original.clone();
        image::imageops::invert(&mut inverted);
        assert_eq!(flat(&h), inverted);
        // The action's check shows it partly on.
        let steps = &h.state().actions.library.sets[0].actions[0].steps;
        assert_eq!(state(steps.iter().map(|s| s.enabled)), None);
    }

    #[test]
    fn a_step_with_its_dialog_waits_for_ok_and_cancel_stops() {
        let mut h = app();
        let mut size = Step::new(Recorded::ImageSize {
            width: 30,
            height: 20,
            resample: Resample::Bilinear,
        });
        size.dialog = true;
        with_action(&mut h, vec![size, invert()]);
        h.state_mut().panel_click(PanelClick::Play);
        h.run_steps(2);
        let d = h.state().image_size.expect("its dialog, with its settings");
        assert_eq!((d.width, d.height), (30, 20));
        h.key_press(egui::Key::Escape);
        settle(&mut h, idle);
        assert_eq!(flat(&h).dimensions(), (120, 80), "cancelled: stopped");
        let before = flat(&h);
        // Again, confirmed: the rest is played.
        h.state_mut().actions.selected = Some(Row::Action(0, 0));
        h.state_mut().panel_click(PanelClick::Play);
        h.run_steps(2);
        assert!(h.state().image_size.is_some());
        h.key_press(egui::Key::Enter);
        settle(&mut h, idle);
        let after = flat(&h);
        assert_eq!(after.dimensions(), (30, 20));
        assert!(after.get_pixel(0, 0).0[2] > 255 - before.get_pixel(0, 0).0[2] - 20);
    }

    #[test]
    fn steps_that_cannot_play_say_why() {
        let mut h = app();
        let feather = Step::new(Recorded::ModifySelection {
            modify: Modify::Feather,
            amount: 5.0,
            at_bounds: false,
        });
        let unknown = Step::new(Recorded::Unknown(serde_json::json!({"command": "liquify"})));
        with_action(&mut h, vec![feather, unknown, invert()]);
        h.state_mut().panel_click(PanelClick::Play);
        settle(&mut h, |app| app.actions.message.is_some());
        let Some(Message::Error { text }) = &h.state().actions.message else {
            panic!("an error");
        };
        assert!(text.contains(&t!("actions-error-no-selection")), "{text}");
        assert_eq!(h.state().actions.selected, Some(Row::Step(0, 0, 0)));
        // Continue: the next step can't play either; Stop ends there.
        h.key_press(egui::Key::Enter);
        settle(&mut h, |app| app.actions.message.is_some());
        assert_eq!(h.state().actions.selected, Some(Row::Step(0, 0, 1)));
        h.state_mut().actions.message = None;
        h.state_mut().stop_playing();
        // A locked layer: refused with its message.
        h.state_mut()
            .doc
            .as_mut()
            .unwrap()
            .active_layer_mut()
            .locked = true;
        h.state_mut().actions.selected = Some(Row::Step(0, 0, 2));
        h.state_mut().panel_click(PanelClick::Play);
        settle(&mut h, |app| app.actions.message.is_some());
        let Some(Message::Error { text }) = &h.state().actions.message else {
            panic!("an error");
        };
        assert!(text.contains("🔒") || text.contains(&t!("actions-title")) || !text.is_empty());
    }

    #[test]
    fn stop_steps_show_their_message() {
        let mut h = app();
        let original = flat(&h);
        let stop = Step::new(Recorded::Stop {
            message: "Check the colors".into(),
            allow_continue: true,
        });
        with_action(&mut h, vec![stop, invert()]);
        h.state_mut().panel_click(PanelClick::Play);
        settle(&mut h, |app| app.actions.message.is_some());
        assert_eq!(flat(&h), original);
        h.key_press(egui::Key::Enter);
        settle(&mut h, idle);
        assert_ne!(flat(&h), original);
    }

    #[test]
    fn default_actions_play() {
        let mut h = app();
        let original = flat(&h);
        let a = &mut h.state_mut().actions;
        a.library = Library::defaults();
        // Vignette, its feather without its dialog (and for a small image).
        toggle(&mut a.library, Row::Step(0, 0, 3), true);
        let feather = &mut a.library.sets[0].actions[0].steps[3].command;
        if let Recorded::ModifySelection { amount, .. } = feather {
            *amount = 4.0;
        }
        a.selected = Some(Row::Action(0, 0));
        play(&mut h);
        let shaded = flat(&h);
        assert!(h.state().actions.message.is_none());
        let doc = h.state().doc.as_ref().unwrap();
        assert_eq!(doc.layers.len(), 2);
        assert!(h.state().selection.is_none());
        let dark = |p: Rgba<u8>| p.0[..3].iter().map(|&c| c as u32).sum::<u32>();
        assert!(dark(*shaded.get_pixel(0, 0)) < dark(*original.get_pixel(0, 0)) / 2 + 1);
        assert_eq!(shaded.get_pixel(60, 40), original.get_pixel(60, 40));
        // Resize to 1080 px: never enlarged.
        h.state_mut().actions.selected = Some(Row::Action(0, 2));
        play(&mut h);
        assert_eq!(flat(&h).dimensions(), (120, 80));
    }

    #[test]
    fn transforms_record_their_frame_and_play_it_on_other_sizes() {
        let mut a = app();
        with_action(&mut a, Vec::new());
        a.state_mut().panel_click(PanelClick::Record);
        a.state_mut()
            .run(Action::Transform(transform::Command::Free));
        // Half as wide, from the left edge.
        a.state_mut().transform_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(60.0, 80.0)));
        a.state_mut().commit_pending();
        settle(&mut a, |app| !app.transform_busy());
        let steps = &a.state().actions.library.sets[0].actions[0].steps;
        let Recorded::Transform { corners } = steps[0].command else {
            panic!("a transform");
        };
        assert_eq!(corners, [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]]);
        let expected = flat(&a);
        let mut b = app();
        with_action(
            &mut b,
            a.state().actions.library.sets[0].actions[0].steps.clone(),
        );
        play(&mut b);
        assert_eq!(flat(&b), expected);
    }

    #[test]
    fn marquees_record_their_shape() {
        let mut h = app();
        with_action(&mut h, Vec::new());
        h.state_mut().panel_click(PanelClick::Record);
        let app = h.state_mut();
        app.tool = Tool::Ellipse;
        app.marquee_preview = Some(PixelRect {
            x: 10,
            y: 5,
            w: 40,
            h: 30,
        });
        app.record_marquee(true, SelectMode::Replace, app.marquee_preview);
        app.selection = Some(
            PixelRect {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            }
            .into(),
        );
        app.record_marquee(false, SelectMode::Replace, None);
        app.run(Action::InvertSelection);
        let steps: Vec<Recorded> = app.actions.library.sets[0].actions[0]
            .steps
            .iter()
            .map(|s| s.command.clone())
            .collect();
        assert!(matches!(
            &steps[..],
            [
                Recorded::Select { shape: Shape::Ellipse, rect, mode: Combine::New, .. },
                Recorded::Select { shape: Shape::Rectangle, rect: r2, .. },
                Recorded::InvertSelection,
            ] if *rect == [10.0, 5.0, 40.0, 30.0] && *r2 == [1.0, 2.0, 3.0, 4.0]
        ));
    }

    #[test]
    fn record_again_takes_the_new_settings_of_a_step() {
        let mut h = app();
        let size = Recorded::ImageSize {
            width: 60,
            height: 40,
            resample: Resample::Bicubic,
        };
        with_action(&mut h, vec![Step::new(size), invert()]);
        // Its dialog, with its settings; cancelled: nothing changes.
        h.state_mut()
            .panel_click(PanelClick::Options(Row::Step(0, 0, 0)));
        h.run_steps(2);
        assert_eq!(h.state().image_size.map(|d| d.width), Some(60));
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.state().actions.again.is_none());
        assert_eq!(flat(&h).dimensions(), (120, 80));
        // Confirmed with other settings: the step takes them.
        h.state_mut()
            .panel_click(PanelClick::Options(Row::Step(0, 0, 0)));
        h.run_steps(2);
        let d = h.state_mut().image_size.as_mut().unwrap();
        (d.width, d.height) = (30, 20);
        h.key_press(egui::Key::Enter);
        h.run_steps(2);
        let step = &h.state().actions.library.sets[0].actions[0].steps[0];
        assert!(matches!(
            step.command,
            Recorded::ImageSize {
                width: 30,
                height: 20,
                ..
            }
        ));
        assert_eq!(flat(&h).dimensions(), (30, 20), "applied too");
    }

    #[test]
    fn other_areas_commands_record_and_play() {
        use crate::liquify::{Brush, Effect, Liquify, Mesh};
        // Liquify, Convert to Profile, Grayscale, Convert to Smart Object.
        let mut a = app();
        with_action(&mut a, Vec::new());
        a.state_mut().panel_click(PanelClick::Record);
        let mut liquify = Liquify::new(Mesh::new(120, 80));
        liquify.begin_stroke();
        let brush = Brush::default();
        liquify.stroke(Effect::Warp, &brush, [60.0, 40.0], [80.0, 40.0], 0.0, 1.0);
        liquify.end_stroke();
        a.state_mut()
            .start_liquify(Arc::new(liquify.mesh.clone()), None);
        settle(&mut a, |app| !app.liquify.busy());
        let adobe = crate::cms::WorkingRgb::AdobeRgb.profile().clone();
        a.state_mut()
            .convert_to_profile(adobe, crate::cms::Intent::Perceptual, true, false);
        a.state_mut().grayscale_mode(false);
        a.state_mut().run(Action::Smart(smart_ui::Command::Convert));
        a.state_mut().panel_click(PanelClick::Stop);
        let steps: Vec<Recorded> = a.state().actions.library.sets[0].actions[0]
            .steps
            .iter()
            .map(|s| s.command.clone())
            .collect();
        assert!(matches!(
            &steps[..],
            [
                Recorded::Liquify { .. },
                Recorded::ConvertToProfile { .. },
                Recorded::Grayscale,
                Recorded::Menu {
                    item: MenuItem::ConvertToSmartObject
                },
            ]
        ));
        let expected = flat(&a);
        let mut b = app();
        with_action(&mut b, steps.into_iter().map(Step::new).collect());
        play(&mut b);
        assert!(b.state().actions.message.is_none());
        assert_eq!(flat(&b), expected);
        let doc = b.state().doc.as_ref().unwrap();
        assert!(doc.active_layer().smart.is_some());
    }

    #[test]
    fn function_keys_play_their_action() {
        let mut h = app();
        let original = flat(&h);
        with_action(&mut h, vec![invert()]);
        h.state_mut().actions.library.sets[0].actions[0].key = Some(FnKey {
            number: 3,
            shift: true,
            ctrl: false,
        });
        h.key_press(egui::Key::F3);
        settle(&mut h, idle);
        assert_eq!(flat(&h), original, "Shift is part of the key");
        h.key_press_modifiers(Modifiers::SHIFT, egui::Key::F3);
        settle(&mut h, idle);
        assert_ne!(flat(&h), original);
        assert!(key_taken(FnKey {
            number: 5,
            shift: true,
            ctrl: false
        }));
    }

    #[test]
    fn panel_new_action_records_and_options_rename() {
        let mut h = app();
        h.state_mut().actions.library.sets.clear();
        h.state_mut().run(Action::Automate(Command::Panel));
        h.state_mut().panel_click(PanelClick::NewAction);
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        let app = h.state_mut();
        assert_eq!(app.actions.recording, Some((0, 0)));
        app.run(Action::SelectAll);
        app.run(Action::NewLayer);
        app.panel_click(PanelClick::Stop);
        let steps = &app.actions.library.sets[0].actions[0].steps;
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[1].command,
            Recorded::Menu {
                item: MenuItem::NewLayer
            }
        );
        // Duplicate and Delete.
        app.actions.selected = Some(Row::Action(0, 0));
        app.panel_click(PanelClick::Duplicate);
        assert_eq!(app.actions.library.sets[0].actions.len(), 2);
        app.panel_click(PanelClick::Delete);
        h.run_steps(2);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        assert_eq!(h.state().actions.library.sets[0].actions.len(), 1);
        h.run_steps(2);
    }

    #[test]
    fn clear_all_asks_first_and_duplicated_sets_drop_their_keys() {
        let mut h = app();
        let key = FnKey {
            number: 3,
            shift: false,
            ctrl: false,
        };
        let app = h.state_mut();
        app.actions.library.sets[0].actions[0].key = Some(key);
        app.actions.selected = Some(Row::Set(0));
        app.panel_click(PanelClick::Duplicate);
        assert_eq!(app.actions.library.sets.len(), 2);
        assert_eq!(app.actions.library.with_key(key), Some((0, 0)));
        let copied = &app.actions.library.sets[1].actions;
        assert!(copied.iter().all(|a| a.key.is_none()));
        // Clear All: only once confirmed.
        app.panel_click(PanelClick::ClearAll);
        h.run_steps(2);
        assert_eq!(h.state().actions.library.sets.len(), 2);
        h.key_press(egui::Key::Enter);
        h.run_steps(1);
        assert!(h.state().actions.library.sets.is_empty());
    }

    /// A command that changed nothing isn't recorded (Merge Down on the
    /// bottom layer); once it does something, it is.
    #[test]
    fn commands_are_recorded_once_done() {
        let mut h = app();
        with_action(&mut h, Vec::new());
        let app = h.state_mut();
        app.actions.start_recording((0, 0));
        app.run(Action::MergeDown);
        app.run(Action::NewLayer);
        app.run(Action::SelectAll);
        app.run(Action::MergeDown);
        let steps: Vec<Recorded> = app.actions.library.sets[0].actions[0]
            .steps
            .iter()
            .map(|s| s.command.clone())
            .collect();
        let menu = |item| Recorded::Menu { item };
        assert_eq!(
            steps,
            [
                menu(MenuItem::NewLayer),
                Recorded::SelectAll,
                menu(MenuItem::MergeDown)
            ]
        );
    }

    /// Place Embedded then scaled: one step, the file and its transform,
    /// played as such (not a transform of whatever layer is active).
    #[test]
    fn a_placed_file_is_recorded_with_its_transform() {
        let dir = std::env::temp_dir().join("atelier-actions-place");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.png");
        RgbaImage::from_pixel(20, 10, Rgba([0, 0, 255, 255]))
            .save(&path)
            .unwrap();
        let mut h = app();
        with_action(&mut h, Vec::new());
        h.state_mut().actions.start_recording((0, 0));
        h.state_mut().place_path(&path, false);
        h.run_steps(2);
        let frame = h.state().transform_rect.unwrap();
        h.state_mut().transform_rect = Some(Rect::from_min_size(frame.min, frame.size() * 2.0));
        h.key_press(egui::Key::Enter);
        let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
        settle(&mut h, applied);
        let steps = &h.state().actions.library.sets[0].actions[0].steps;
        assert_eq!(steps.len(), 1);
        let Recorded::Place {
            path: recorded,
            linked: false,
            corners,
        } = &steps[0].command
        else {
            panic!("{:?}", steps[0].command);
        };
        assert_eq!(recorded, &path);
        assert!((corners[2][0] - 2.0).abs() < 1e-6, "{corners:?}");
        // Played on another document: the file placed, scaled.
        let mut h2 = app();
        let steps = h.state().actions.library.sets[0].actions[0].steps.clone();
        with_action(&mut h2, steps);
        play(&mut h2);
        let doc = h2.state().doc.as_ref().unwrap();
        assert_eq!(doc.layers.len(), 2);
        assert!(doc.active_layer().is_smart());
        assert_eq!(doc.active_layer().rect().w, 40);
    }
}
