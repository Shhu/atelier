//! The locks of the Layers panel: Lock Transparent Pixels, Lock Image
//! Pixels, Lock Position and Lock All, and what they refuse.

use super::*;
use crate::document::{LockedBy, Locks};

/// A lock of the Layers panel's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LockKind {
    Transparent,
    Pixels,
    Position,
    All,
}

impl LockKind {
    pub(super) const ROW: [Self; 4] = [Self::Transparent, Self::Pixels, Self::Position, Self::All];

    fn glyph(self) -> &'static str {
        match self {
            Self::Transparent => icon::CHECKERBOARD,
            Self::Pixels => icon::PAINT_BRUSH,
            Self::Position => icon::ARROWS_OUT_CARDINAL,
            Self::All => icon::LOCK_SIMPLE,
        }
    }

    pub(super) fn label(self) -> String {
        match self {
            Self::Transparent => t!("layers8-lock-transparent"),
            Self::Pixels => t!("layers8-lock-pixels"),
            Self::Position => t!("layers8-lock-position"),
            Self::All => t!("layers8-lock-all"),
        }
    }

    fn on(self, l: &Layer) -> bool {
        match self {
            Self::Transparent => l.locks.transparent,
            Self::Pixels => l.locks.pixels,
            Self::Position => l.locks.position,
            Self::All => l.locked,
        }
    }

    fn set(self, l: &mut Layer, on: bool) {
        match self {
            Self::Transparent => l.locks.transparent = on,
            Self::Pixels => l.locks.pixels = on,
            Self::Position => l.locks.position = on,
            Self::All => l.locked = on,
        }
    }

    fn applies(self, doc: &Document, i: usize) -> bool {
        match self {
            Self::Transparent | Self::Pixels => doc.lockable_pixels(i),
            Self::Position | Self::All => true,
        }
    }

    /// The innermost group holding layer `i` with this lock (or Lock All).
    fn group(self, doc: &Document, i: usize) -> Option<usize> {
        match self {
            Self::Transparent => doc.group_with_lock(i, |l| l.transparent),
            Self::Pixels => doc.group_with_lock(i, |l| l.pixels),
            Self::Position => doc.group_with_lock(i, |l| l.position),
            Self::All => doc.locking_group(i),
        }
    }

    /// As the panel's button and the menu show it for layer `i`: on, whether
    /// through Lock All or a group; and whether it can change.
    pub(super) fn state(self, doc: &Document, i: usize) -> (bool, bool) {
        let Some(l) = doc.layers.get(i) else {
            return (false, false);
        };
        // Every group that holds it counts, not only the nearest locked one.
        let inherited = self.group(doc, i).is_some();
        let all = l.locked || doc.locking_group(i).is_some();
        let on = self.on(l) || inherited || (all && self != Self::All);
        // Lock All covers the others; a lock from the group stays with it.
        let free = !inherited && (self == Self::All || !l.locked) && self.applies(doc, i);
        (on, free)
    }
}

/// A layer's own locks: Lock All, the partial ones.
type OwnLocks = (bool, Locks);

trait SetLock {
    fn set(&mut self, kind: LockKind, on: bool);
}

impl SetLock for OwnLocks {
    fn set(&mut self, kind: LockKind, on: bool) {
        match kind {
            LockKind::Transparent => self.1.transparent = on,
            LockKind::Pixels => self.1.pixels = on,
            LockKind::Position => self.1.position = on,
            LockKind::All => self.0 = on,
        }
    }
}

/// The lock Ctrl+/ puts on: the last one used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LastLock {
    pub(super) locks: Locks,
    pub(super) all: bool,
}

impl Default for LastLock {
    fn default() -> Self {
        Self {
            locks: Locks::default(),
            all: true,
        }
    }
}

impl LastLock {
    pub(super) const EVERY: Self = Self {
        locks: Locks::ALL,
        all: true,
    };

    pub(super) fn only(kind: LockKind) -> Self {
        Self {
            locks: Locks {
                transparent: kind == LockKind::Transparent,
                pixels: kind == LockKind::Pixels,
                position: kind == LockKind::Position,
            },
            all: kind == LockKind::All,
        }
    }

    fn holds(self, kind: LockKind) -> bool {
        match kind {
            LockKind::Transparent => self.locks.transparent,
            LockKind::Pixels => self.locks.pixels,
            LockKind::Position => self.locks.position,
            LockKind::All => self.all,
        }
    }

    /// The names of these locks, for a message or a step.
    pub(super) fn names(self) -> String {
        let names: Vec<String> = LockKind::ROW
            .into_iter()
            .filter(|&k| self.holds(k))
            .map(LockKind::label)
            .collect();
        names.join(", ")
    }

    /// For the session: `transparent,pixels,position,all`.
    pub(super) fn to_bits(self) -> u64 {
        u64::from(self.locks.transparent)
            | u64::from(self.locks.pixels) << 1
            | u64::from(self.locks.position) << 2
            | u64::from(self.all) << 3
    }

    pub(super) fn from_bits(bits: u64) -> Self {
        let last = Self {
            locks: Locks {
                transparent: bits & 1 != 0,
                pixels: bits & 2 != 0,
                position: bits & 4 != 0,
            },
            all: bits & 8 != 0,
        };
        // Nothing would be no lock at all.
        if last.locks.any() || last.all {
            last
        } else {
            Self::default()
        }
    }
}

impl AtelierApp {
    /// A lock of the panel or menu on the selected layers, on or off as the
    /// active one isn't or is; one step.
    pub(super) fn toggle_lock_kind(&mut self, kind: LockKind) {
        let Some(doc) = &self.doc else { return };
        let (on, free) = kind.state(doc, doc.active);
        if !free {
            return;
        }
        let on = !on;
        let picked: Vec<usize> = self
            .selected_indices()
            .into_iter()
            .filter(|&i| kind.state(doc, i).1)
            .collect();
        let name = doc.active_layer().name.clone();
        self.record_change(lock_step(on), None);
        let Some(doc) = &mut self.doc else { return };
        for i in picked {
            kind.set(&mut doc.layers[i], on);
        }
        if on {
            let l = doc.active_layer();
            self.last_lock = LastLock {
                locks: l.locks,
                all: l.locked,
            };
        }
        self.status = lock_status(on, &name);
    }

    /// What Ctrl+/ would do: the locks concerned, on or off.
    pub(super) fn last_lock_change(&self) -> Option<(LastLock, bool)> {
        let active = self.doc.as_ref()?.active_layer();
        let on = !active.locked && !active.locks.any();
        Some((if on { self.last_lock } else { LastLock::EVERY }, on))
    }

    /// The new own locks of the selected layers that `set` changes.
    fn lock_changes(&self, set: LastLock, on: bool) -> Vec<(usize, bool, Locks)> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let mut changes = Vec::new();
        for i in self.selected_indices() {
            // Under its group's Lock All, a layer's locks don't change.
            if doc.locking_group(i).is_some() {
                continue;
            }
            let l = &doc.layers[i];
            let mut next: OwnLocks = (l.locked, l.locks);
            for kind in LockKind::ROW.into_iter().filter(|&k| set.holds(k)) {
                // Nothing to add to what the group says, or what can't hold it.
                if !on || (kind.group(doc, i).is_none() && kind.applies(doc, i)) {
                    next.set(kind, on);
                }
            }
            if next != (l.locked, l.locks) {
                changes.push((i, next.0, next.1));
            }
        }
        changes
    }

    /// `set` on or off on the selected layers: one step. `false` when
    /// nothing changes.
    fn apply_locks(&mut self, set: LastLock, on: bool) -> bool {
        let changes = self.lock_changes(set, on);
        if changes.is_empty() {
            return false;
        }
        let name = self.doc.as_ref().map_or_else(String::new, |d| d.active_layer().name.clone());
        self.record_change(lock_step(on), None);
        let Some(doc) = &mut self.doc else {
            return false;
        };
        for (i, locked, locks) in changes {
            (doc.layers[i].locked, doc.layers[i].locks) = (locked, locks);
        }
        self.status = lock_status(on, &name);
        true
    }

    /// Ctrl+/: a locked layer is unlocked; otherwise, the last lock used
    /// goes on the selected layers (Lock All at first). One step, and none
    /// when nothing would change: the status says why.
    pub(super) fn toggle_last_lock(&mut self) {
        let Some((set, on)) = self.last_lock_change() else {
            return;
        };
        // The group's Lock All isn't this command's to undo.
        let held = self.doc.as_ref().is_some_and(|d| d.locking_group(d.active).is_some());
        if held {
            self.refuse_if_locked();
            return;
        }
        if self.apply_locks(set, on) {
            return;
        }
        // Already on through a group: nothing to do, and the group said.
        let doc = self.doc.as_ref().expect("a document has a lock change");
        let kinds = LockKind::ROW.into_iter().filter(|&k| set.holds(k));
        let groups: Option<Vec<usize>> = kinds.map(|k| k.group(doc, doc.active)).collect();
        match groups.as_deref() {
            Some([g, ..]) if on => {
                let group = doc.layers[*g].name.as_str();
                self.status = t!("locks11-lock-held-by-group", lock = set.names(), group = group);
            }
            _ => self.status_cannot(t!("layers8-lock-unavailable", lock = set.names())),
        }
    }

    /// A step of an action: `set` on or off. Already so: nothing to do.
    pub(super) fn play_locks(&mut self, set: LastLock, on: bool) -> Result<(), String> {
        if self.apply_locks(set, on) {
            return Ok(());
        }
        let Some(doc) = &self.doc else {
            return Err(t!("actions-error-no-document"));
        };
        let done = LockKind::ROW
            .into_iter()
            .filter(|&k| set.holds(k))
            .all(|k| k.state(doc, doc.active).0 == on);
        if done {
            Ok(())
        } else {
            Err(t!("layers8-lock-unavailable", lock = set.names()))
        }
    }

    /// A layer's own locks all off (the lock of its row in the panel).
    pub(super) fn unlock_layer(&mut self, i: usize) {
        let recording = self.actions.recording_len();
        let before = self.doc.as_ref().map(|d| (d.uid(), d.history_step()));
        let active = self.doc.as_ref().is_some_and(|d| d.active == i);
        self.record_change(lock_step(false), None);
        let Some(l) = self.doc.as_mut().and_then(|d| d.layers.get_mut(i)) else {
            return;
        };
        l.locked = false;
        l.locks = Locks::default();
        self.status = lock_status(false, &l.name.clone());
        // An action plays on the active layer: another one's can't be a step.
        if active {
            self.record(actions_panel::recorded_locks(LastLock::EVERY, false));
        } else {
            self.tell_unrecorded(Action::ToggleLock, recording, before);
        }
    }

    /// The locks of the Layers panel; `labelled`: after the word "Lock:".
    pub(super) fn lock_row(&mut self, ui: &mut egui::Ui, labelled: bool) {
        let Some(doc) = &self.doc else { return };
        let states = LockKind::ROW.map(|k| {
            let group = k.group(doc, doc.active).map(|g| doc.layers[g].name.clone());
            (k, k.state(doc, doc.active), group)
        });
        let mut clicked = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if labelled {
                ui.label(
                    egui::RichText::new(t!("layers8-lock-label"))
                        .size(12.0)
                        .color(ui_kit::color::LABEL),
                );
                ui.add_space(4.0);
            }
            for (kind, (on, free), inherited) in states {
                let mut shown = on;
                let label = kind.label();
                let tip = match (&inherited, free) {
                    (Some(g), false) if on => {
                        t!("tools6-panel-group-lock-tip", group = g.as_str())
                    }
                    (_, false) if !on => t!("layers8-lock-unavailable", lock = label.as_str()),
                    _ => label.clone(),
                };
                let response = ui
                    .add_enabled_ui(free, |ui| {
                        ui_kit::icon_toggle(ui, kind.glyph(), &label, &mut shown)
                    })
                    .inner;
                let response = response.on_disabled_hover_text(tip);
                if response.changed() {
                    clicked = Some(kind);
                }
            }
        });
        if let Some(kind) = clicked {
            self.run(Action::Lock(kind));
        }
    }

    /// Says that lock `kind` on layer `i` refuses what was asked, naming
    /// the layer or group that has it, where it can be turned off.
    pub(super) fn refuse_lock_of(&mut self, i: usize, kind: LockKind) {
        let Some(message) = self.doc.as_ref().and_then(|d| lock_refusal(d, i, kind)) else {
            return;
        };
        self.status_refused(message);
    }

    /// Says that a lock of layer `i` (whichever it has, Lock All first)
    /// refuses what was asked.
    pub(super) fn refuse_any_lock_of(&mut self, i: usize) {
        let Some(doc) = &self.doc else { return };
        let locks = doc.locks(i);
        let kind = if doc.is_locked(i) {
            LockKind::All
        } else if locks.position {
            LockKind::Position
        } else if locks.pixels {
            LockKind::Pixels
        } else {
            LockKind::Transparent
        };
        self.refuse_lock_of(i, kind);
    }

    /// What layer `i` holds (text, path, smart object) may not be edited:
    /// says so and returns `true`.
    pub(super) fn refuse_content_locked_at(&mut self, i: usize) -> bool {
        if !self.doc.as_ref().is_some_and(|d| d.content_locked(i)) {
            return false;
        }
        self.refuse_lock_of(i, LockKind::Pixels);
        true
    }

    /// A new text or shape layer would land in a group under Lock Image
    /// Pixels (or Lock All), where it could not be typed or drawn: says so
    /// and returns `true`.
    pub(super) fn refuse_new_layer_locked(&mut self) -> bool {
        match self.doc.as_ref().and_then(Document::landing_group) {
            Some(g) => self.refuse_content_locked_at(g),
            None => false,
        }
    }

    /// Lock Image Pixels on what is about to be edited (not a targeted
    /// mask, which isn't under it): says so and returns `true`.
    pub(super) fn refuse_pixels_locked(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        if !doc.pixels_locked() {
            return false;
        }
        self.refuse_lock_of(doc.active, LockKind::Pixels);
        true
    }

    /// What the active layer holds (text, path, smart object) may not be
    /// edited, a targeted mask or not: says so and returns `true`.
    pub(super) fn refuse_content_locked(&mut self) -> bool {
        if self.refuse_if_locked() {
            return true;
        }
        let Some(doc) = &self.doc else { return false };
        if !doc.content_locked(doc.active) {
            return false;
        }
        self.refuse_lock_of(doc.active, LockKind::Pixels);
        true
    }

    /// The one rule for what resamples or turns the active element (Free
    /// Transform, its commands, handles, Properties): says so and returns
    /// `true` when a lock on it, or on a layer its group holds, refuses.
    pub(super) fn refuse_transform_locked(&mut self) -> bool {
        if self.refuse_position_locked() {
            return true;
        }
        let Some(doc) = &self.doc else { return false };
        let Some((i, by)) = doc.transform_block(doc.active) else {
            return false;
        };
        let kind = match by {
            LockedBy::Position => LockKind::Position,
            LockedBy::Pixels => LockKind::Pixels,
        };
        self.refuse_lock_of(i, kind);
        true
    }

    /// A layer about to land in a group that won't let it move, when it
    /// has to be placed: says so and returns `true`.
    pub(super) fn refuse_new_layer_stuck(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        match doc.landing_group().filter(|&g| !doc.can_move(g)) {
            Some(g) => {
                self.refuse_lock_of(g, LockKind::Position);
                true
            }
            None => false,
        }
    }

    /// Lock Transparent Pixels on what is about to take pixels away.
    pub(super) fn refuse_alpha_locked(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        if !doc.alpha_locked() {
            return false;
        }
        self.refuse_lock_of(doc.active, LockKind::Transparent);
        true
    }

    /// The active layer may not move (Lock Position, Lock All): says so and
    /// returns `true`.
    pub(super) fn refuse_position_locked(&mut self) -> bool {
        if self.refuse_if_locked() {
            return true;
        }
        let Some(doc) = &self.doc else { return false };
        // A group none of whose layers can move doesn't move either.
        let by = match doc.can_move(doc.active) {
            true => doc.stuck_group_layer(doc.active),
            false => Some(doc.active),
        };
        let Some(i) = by else { return false };
        self.refuse_lock_of(i, LockKind::Position);
        true
    }

    /// What the partial locks refuse of `action`, beyond the checks of Lock
    /// All; `true` when it has been dealt with.
    pub(super) fn locks_refuse(&mut self, action: Action) -> bool {
        // Whatever is targeted, it writes into the layer's own pixels.
        if matches!(action, Action::ApplyMask)
            && let Some(doc) = &self.doc
        {
            let (active, locks) = (doc.active, doc.locks(doc.active));
            if locks.pixels || locks.transparent {
                let kind = if locks.pixels {
                    LockKind::Pixels
                } else {
                    LockKind::Transparent
                };
                self.refuse_lock_of(active, kind);
                return true;
            }
        }
        if self.in_quick_mask() {
            return false;
        }
        match action {
            Action::LayerViaCut | Action::Split => {
                self.refuse_pixels_locked() || self.refuse_alpha_locked()
            }
            Action::Rasterize => self.refuse_pixels_locked(),
            Action::MakeTransparent => self.refuse_alpha_locked(),
            Action::Transform(_) => self.refuse_transform_locked(),
            // Centered from the bounds of all it holds: refused when some of
            // them would stay.
            Action::CenterElement => {
                self.refuse_position_locked()
                    || match self.doc.as_ref().and_then(|d| d.held_member(d.active)) {
                        Some(j) => {
                            self.refuse_lock_of(j, LockKind::Position);
                            true
                        }
                        None => false,
                    }
            }
            // Delete can't take the pixels away: it fills them, asking how
            // (an action can't ask: the step fails).
            Action::ClearSelection
                if self.selection.is_some()
                    && self.doc.as_ref().is_some_and(Document::alpha_locked) =>
            {
                if self.actions.playing() {
                    return self.refuse_alpha_locked();
                }
                self.run(Action::FillDialog);
                true
            }
            _ => false,
        }
    }

    /// Under a lock that refuses transforms, the Move tool only moves.
    pub(super) fn transform_frozen(&self) -> bool {
        !self.in_quick_mask()
            && self.doc.as_ref().is_some_and(|d| d.transform_block(d.active).is_some())
    }
}

fn lock_step(on: bool) -> String {
    if on {
        t!("xchg-hist-lock-layer")
    } else {
        t!("xchg-hist-unlock-layer")
    }
}

fn lock_status(on: bool, name: &str) -> String {
    if on {
        format!(
            "{} {}",
            icon::LOCK_SIMPLE,
            t!("status-layer-locked", name = name)
        )
    } else {
        format!(
            "{} {}",
            icon::LOCK_SIMPLE_OPEN,
            t!("status-layer-unlocked", name = name)
        )
    }
}

/// Why lock `kind` on layer `i` refuses what was asked, naming the layer
/// or group that has it, where it can be turned off.
pub(super) fn lock_refusal(doc: &Document, i: usize, kind: LockKind) -> Option<crate::i18n::Message> {
    let l = doc.layers.get(i)?;
    let name = l.name.as_str();
    let group = (!kind.on(l) && !l.locked)
        .then(|| kind.group(doc, i))
        .flatten()
        .map(|g| &doc.layers[g]);
    Some(match (kind, group) {
        (_, Some(g)) if g.locked => {
            tm!("tools6-status-group-locked", name = name, group = g.name.as_str())
        }
        (_, None) if l.locked => tm!("status-layer-is-locked", name = name),
        (LockKind::Pixels, Some(g)) => {
            tm!("locks9-status-group-pixels-locked", name = name, group = g.name.as_str())
        }
        (LockKind::Transparent, Some(g)) => {
            tm!("locks9-status-group-transparent-locked", name = name, group = g.name.as_str())
        }
        (LockKind::Position, Some(g)) => {
            tm!("locks9-status-group-position-locked", name = name, group = g.name.as_str())
        }
        (LockKind::Pixels, None) => tm!("layers8-status-pixels-locked", name = name),
        (LockKind::Transparent, None) => tm!("layers8-status-transparent-locked", name = name),
        (LockKind::Position | LockKind::All, None) => {
            tm!("layers8-status-position-locked", name = name)
        }
        (LockKind::All, Some(g)) => {
            tm!("tools6-status-group-locked", name = name, group = g.name.as_str())
        }
    })
}
