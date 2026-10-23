//! Open documents as tabs. The active one lives in `AtelierApp`'s fields;
//! the others are set aside with their own view, selection and textures.

use super::*;
use crate::document::{HISTORY_BUDGET, TOTAL_HISTORY_BUDGET};

/// A document in a background tab, with what belongs to it.
pub(super) struct ParkedDoc {
    pub(super) doc: Document,
    thumbs: HashMap<(u64, bool), (u64, f64, egui::TextureHandle)>,
    layer_set: Vec<u64>,
    selection: Option<Selection>,
    last_selection: Option<Selection>,
    last_paint: Option<Pos2>,
    prop_session: Option<((u64, &'static str), u64)>,
    pub(super) zoom: f32,
    pan: Vec2,
    /// Not shown yet: fit it to the window when it is.
    fit_pending: Option<f32>,
}

impl AtelierApp {
    /// Number of open documents.
    pub(super) fn tab_count(&self) -> usize {
        usize::from(self.doc.is_some()) + self.parked.len()
    }

    /// Document of tab `i` and its zoom (the active one included).
    pub(super) fn tab(&self, i: usize) -> Option<(&Document, f32)> {
        use std::cmp::Ordering;
        match i.cmp(&self.active_tab) {
            Ordering::Less => self.parked.get(i).map(|p| (&p.doc, p.zoom)),
            Ordering::Equal => self.doc.as_ref().map(|d| (d, self.zoom)),
            Ordering::Greater => self.parked.get(i - 1).map(|p| (&p.doc, p.zoom)),
        }
    }

    /// Makes tab `i` the active one.
    pub(super) fn switch_tab(&mut self, i: usize) {
        if i == self.active_tab || i >= self.tab_count() {
            return;
        }
        let Some(current) = self.park() else { return };
        let mut all = std::mem::take(&mut self.parked);
        all.insert(self.active_tab, current);
        let target = all.remove(i);
        self.parked = all;
        self.active_tab = i;
        self.unpark(target);
    }

    /// Opens `doc` in a new tab, right after the active one.
    pub(super) fn add_tab(&mut self, doc: Document) {
        if let Some(current) = self.park() {
            self.parked.insert(self.active_tab, current);
            self.active_tab += 1;
        }
        self.doc = Some(doc);
        self.fit_pending = Some(1.0);
    }

    /// Tab already showing the file at `path`, if any.
    pub(super) fn tab_of(&self, path: &Path) -> Option<usize> {
        let same = |p: &Path| {
            p == path
                || std::fs::canonicalize(p)
                    .ok()
                    .is_some_and(|p| std::fs::canonicalize(path).is_ok_and(|q| p == q))
        };
        (0..self.tab_count()).find(|&i| {
            self.tab(i)
                .and_then(|(d, _)| d.path.as_deref())
                .is_some_and(same)
        })
    }

    /// After the active document was closed: the next tab (or the previous
    /// one) becomes active.
    pub(super) fn activate_neighbor_tab(&mut self) {
        if self.doc.is_some() || self.parked.is_empty() {
            self.active_tab = 0;
            return;
        }
        let i = self.active_tab.min(self.parked.len() - 1);
        let next = self.parked.remove(i);
        self.active_tab = i;
        self.unpark(next);
    }

    /// First tab with unsaved changes (the active one first, a pending
    /// transformation or crop counting as one).
    pub(super) fn first_modified_tab(&self) -> Option<usize> {
        if self.unsaved() {
            return Some(self.active_tab);
        }
        (0..self.tab_count()).find(|&i| self.tab(i).is_some_and(|(d, _)| d.modified))
    }

    /// Takes the active document out with its state, after applying what is
    /// being prepared and closing the windows that act on it.
    fn park(&mut self) -> Option<ParkedDoc> {
        self.doc.as_ref()?;
        self.commit_pending();
        self.sync_selection();
        self.leave_select_modes();
        self.adjust = None;
        self.transparency = None;
        self.split_open = false;
        self.fill_dialog = None;
        self.image_size = None;
        self.flatten_confirm = false;
        self.forget_document_jobs();
        self.end_layer_preview();
        let parked = ParkedDoc {
            doc: self.doc.take()?,
            thumbs: std::mem::take(&mut self.thumbs),
            layer_set: std::mem::take(&mut self.layer_set),
            selection: self.selection.take(),
            last_selection: self.last_selection.take(),
            last_paint: self.last_paint.take(),
            prop_session: self.prop_session.take(),
            zoom: self.zoom,
            pan: self.pan,
            fit_pending: self.fit_pending,
        };
        // Caches and gestures of the document that leaves.
        self.stroke = None;
        self.stroke_dirty = None;
        // The view stays: kept for the document's return (see `CanvasView`).
        self.lifted = None;
        self.blend_preview = None;
        self.mask_view = false;
        self.layer_style = None;
        self.marquee = None;
        self.marquee_preview = None;
        self.lasso = None;
        self.select_move = None;
        self.selection_outline = None;
        self.edges = Default::default();
        self.histogram = None;
        self.split_preview = None;
        self.preview_shown = None;
        self.list_drag = None;
        self.snap_targets = None;
        self.hover_pixel = None;
        self.last_view = None;
        self.fit_pending = None;
        Some(parked)
    }

    /// Brings a set-aside document back as the active one.
    fn unpark(&mut self, p: ParkedDoc) {
        self.doc = Some(p.doc);
        self.thumbs = p.thumbs;
        self.layer_set = p.layer_set;
        self.selection = p.selection;
        self.last_selection = p.last_selection;
        self.last_paint = p.last_paint;
        self.prop_session = p.prop_session;
        self.zoom = p.zoom;
        self.pan = p.pan;
        self.fit_pending = p.fit_pending;
        self.reset_pending();
    }

    /// The histories of all open documents together stay within
    /// [`TOTAL_HISTORY_BUDGET`]: the active one keeps its own budget, the
    /// documents set aside share what is left and lose their oldest steps.
    pub(super) fn trim_histories(&mut self) {
        let key = self
            .doc
            .as_ref()
            .map(|d| (d.uid(), d.history_step(), self.parked.len()));
        if self.parked.is_empty() || key == self.history.budget_key {
            return;
        }
        self.history.budget_key = key;
        let used = self.doc.as_ref().map_or(0, Document::history_bytes);
        let left = TOTAL_HISTORY_BUDGET.saturating_sub(used.min(HISTORY_BUDGET));
        let share = left / self.parked.len();
        for p in &mut self.parked {
            p.doc.trim_history(share);
        }
    }
}
