//! The locks of the Layers panel: Lock Transparent Pixels, Lock Image
//! Pixels, Lock Position and Lock All, and what they refuse.

use super::*;
use crate::document::Locks;

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

    /// As the panel's button and the menu show it for layer `i`: on, whether
    /// through Lock All or a group; and whether it can change.
    pub(super) fn state(self, doc: &Document, i: usize) -> (bool, bool) {
        let Some(l) = doc.layers.get(i) else {
            return (false, false);
        };
        let group = doc.lock_group(i).map(|g| &doc.layers[g]);
        let inherited = group.is_some_and(|g| g.locked || self.on(g));
        let all = l.locked || group.is_some_and(|g| g.locked);
        let on = self.on(l) || inherited || (all && self != Self::All);
        // Lock All covers the others; a lock from the group stays with it.
        let free = !inherited && (self == Self::All || !l.locked) && self.applies(doc, i);
        (on, free)
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

    /// Ctrl+/: a locked layer is unlocked; otherwise, the last lock used
    /// goes on the selected layers (Lock All at first). One step.
    pub(super) fn toggle_last_lock(&mut self) {
        let Some(doc) = &self.doc else { return };
        let active = doc.active_layer();
        let on = !active.locked && !active.locks.any();
        let last = self.last_lock;
        let picked = self.selected_indices();
        let name = active.name.clone();
        self.record_change(lock_step(on), None);
        let Some(doc) = &mut self.doc else { return };
        for i in picked {
            let can_hold_pixels = doc.lockable_pixels(i);
            let l = &mut doc.layers[i];
            if on {
                l.locked = last.all;
                l.locks.position = last.locks.position;
                l.locks.transparent = last.locks.transparent && can_hold_pixels;
                l.locks.pixels = last.locks.pixels && can_hold_pixels;
            } else {
                l.locked = false;
                l.locks = Locks::default();
            }
        }
        self.status = lock_status(on, &name);
    }

    /// A layer's own locks all off (the lock of its row in the panel).
    pub(super) fn unlock_layer(&mut self, i: usize) {
        self.record_change(lock_step(false), None);
        let Some(l) = self.doc.as_mut().and_then(|d| d.layers.get_mut(i)) else {
            return;
        };
        l.locked = false;
        l.locks = Locks::default();
        self.status = lock_status(false, &l.name.clone());
    }

    /// The row of locks above the list of layers.
    pub(super) fn lock_row(&mut self, ui: &mut egui::Ui) {
        let Some(doc) = &self.doc else { return };
        let states = LockKind::ROW.map(|k| (k, k.state(doc, doc.active)));
        let inherited = doc
            .lock_group(doc.active)
            .map(|g| doc.layers[g].name.clone());
        let mut clicked = None;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.label(
                egui::RichText::new(t!("layers8-lock-label"))
                    .size(12.0)
                    .color(ui_kit::color::LABEL),
            );
            ui.add_space(4.0);
            for (kind, (on, free)) in states {
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

    fn lock_refused(&mut self, message: i18n::Message) {
        let lock = icon::LOCK_SIMPLE;
        self.status_failed(i18n::Message::new(
            format!("{lock} {}", message.shown),
            format!("{lock} {}", message.english),
        ));
    }

    /// Lock Image Pixels on what is about to be edited (not a targeted
    /// mask, which isn't under it): says so and returns `true`.
    pub(super) fn refuse_pixels_locked(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        if !doc.pixels_locked() {
            return false;
        }
        let name = doc.active_layer().name.clone();
        self.lock_refused(tm!("layers8-status-pixels-locked", name = name.as_str()));
        true
    }

    /// Lock Transparent Pixels on what is about to take pixels away.
    pub(super) fn refuse_alpha_locked(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        if !doc.alpha_locked() {
            return false;
        }
        let name = doc.active_layer().name.clone();
        self.lock_refused(tm!(
            "layers8-status-transparent-locked",
            name = name.as_str()
        ));
        true
    }

    /// The active layer may not move (Lock Position, Lock All): says so and
    /// returns `true`.
    pub(super) fn refuse_position_locked(&mut self) -> bool {
        if self.refuse_if_locked() {
            return true;
        }
        let Some(doc) = &self.doc else { return false };
        if doc.can_move(doc.active) {
            return false;
        }
        let name = doc.active_layer().name.clone();
        self.lock_refused(tm!("layers8-status-position-locked", name = name.as_str()));
        true
    }

    /// What the partial locks refuse of `action`, beyond the checks of Lock
    /// All; `true` when it has been dealt with.
    pub(super) fn locks_refuse(&mut self, action: Action) -> bool {
        if self.in_quick_mask() {
            return false;
        }
        match action {
            Action::LayerViaCut | Action::Split => {
                self.refuse_pixels_locked() || self.refuse_alpha_locked()
            }
            Action::Rasterize => self.refuse_pixels_locked(),
            Action::MakeTransparent => self.refuse_alpha_locked(),
            Action::Transform(_) | Action::CenterElement => self.refuse_position_locked(),
            // Delete can't take the pixels away: it fills them, asking how.
            Action::ClearSelection
                if self.selection.is_some()
                    && self.doc.as_ref().is_some_and(Document::alpha_locked) =>
            {
                self.run(Action::FillDialog);
                true
            }
            _ => false,
        }
    }

    /// Under Lock Image Pixels, the Move tool only moves.
    pub(super) fn transform_frozen(&self) -> bool {
        !self.in_quick_mask() && self.doc.as_ref().is_some_and(Document::pixels_locked)
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
