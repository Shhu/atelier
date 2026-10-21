//! Smart objects in the app: Layer › Smart Objects, File › Place Embedded
//! and Place Linked (drag and drop and pasted files too), Edit Contents in
//! a tab of its own whose Save updates the smart object, linked files
//! followed when the window gets the focus back, and the smart filters:
//! the filters of the Filter menu applied to a smart object, re-edited
//! from the Layers panel, with their Blending Options.

use std::sync::mpsc;

use super::dialogs::{DialogSpec, Track, floating_dialog, gap_between_blocks, slider_row};
use super::*;
use crate::document::{EmbeddedIn, SmartFilter, SmartFilterKind, SmartObject, SmartRender};
use crate::filters::{Filter, Target};
use crate::i18n::t;
use crate::xform::{Homography, Map};

/// Pixels (filters counted) rendered at once; beyond, on another thread.
const ASYNC_PIXELS: u64 = 4 << 20;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Command {
    Convert,
    NewViaCopy,
    ResetTransform,
    UpdateModified,
    EditContents,
    ExportContents,
    EmbedLinked,
    ReplaceContents,
    PlaceLinked,
    /// Filter › Convert for Smart Filters.
    ConvertForFilters,
    // The smart filters of the active smart object (Layers panel).
    EditFilter(usize),
    FilterBlending(usize),
    ToggleFilter(usize),
    DeleteFilter(usize),
    /// A filter dragged to another place in the list (indices from the
    /// first applied).
    MoveFilter(usize, usize),
    ToggleFilters,
    ClearFilters,
    AddFilterMask,
    DeleteFilterMask,
    ToggleFilterMask,
    /// A click on the filter mask's thumbnail: the tools edit it.
    TargetFilterMask,
}

/// Smart objects' state of the session.
#[derive(Default)]
pub(super) struct SmartUi {
    /// The smart filter the filter dialog re-edits (its index).
    editing: Option<usize>,
    /// What the open filter dialog filters, for a layer, its revision and
    /// the index of the filter.
    input: Option<(InputKey, Target)>,
    /// Blending Options of a smart filter, open.
    blending: Option<Blending>,
    /// A rendering being computed after a change.
    rendering: Option<Rendering>,
    /// The preview of a dialog being computed, and for what.
    preview: Option<(u64, mpsc::Receiver<SmartRender>)>,
    /// A file being placed.
    placing: Option<Placing>,
    /// Smart filter row being dragged: its layer and index.
    pub(super) drag: Option<(u64, usize)>,
    /// The window had the focus at the last frame.
    focused: bool,
    /// Document whose links were last looked at.
    links_of: u64,
    /// Thumbnails of the filter masks: layer → fingerprint, texture.
    pub(super) thumbs: HashMap<u64, (u64, egui::TextureHandle)>,
}

impl SmartUi {
    /// A rendering or the Blending Options keep the keyboard and the image.
    pub(super) fn busy(&self) -> bool {
        self.rendering.is_some() || self.blending.is_some()
    }

    /// The smart filter as the open Blending Options set it.
    #[cfg(test)]
    pub(super) fn blending_mut(&mut self) -> Option<&mut SmartFilter> {
        self.blending.as_mut().map(|b| &mut b.filter)
    }
}

type InputKey = (u64, u64, Option<usize>);

struct Blending {
    layer: u64,
    index: usize,
    opened: SmartFilter,
    filter: SmartFilter,
    preview: bool,
}

/// A file just placed, its Free Transform being prepared.
struct Placing {
    doc: u64,
    /// The history right after the place.
    step: u64,
    name: String,
    /// Enter: the transform is being applied.
    committing: bool,
}

struct Rendering {
    doc: u64,
    layer: u64,
    name: String,
    started: f64,
    rx: mpsc::Receiver<SmartRender>,
}

/// Name of a smart filter (Layers panel, history).
pub(super) fn filter_name(f: &SmartFilterKind) -> String {
    match f {
        SmartFilterKind::Filter(f) => filters_ui::title(f.kind()),
        SmartFilterKind::Liquify(_) => t!("liquify-title"),
        SmartFilterKind::CameraRaw(_) => t!("raw-filter-title"),
    }
}

impl AtelierApp {
    /// Layer › Smart Objects, in Photoshop's order.
    pub(super) fn smart_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let doc = self.doc.as_ref();
        let smart = doc.and_then(|d| d.active_layer().smart.as_deref());
        let is = smart.is_some();
        let linked = smart.is_some_and(|s| s.link.is_some());
        let item = |label: String, on: bool, cmd| Item(label, on, None, Action::Smart(cmd));
        vec![
            item(t!("smart-menu-convert"), doc.is_some(), Command::Convert),
            item(t!("smart-menu-new-via-copy"), is, Command::NewViaCopy),
            item(
                t!("smart-menu-reset-transform"),
                is,
                Command::ResetTransform,
            ),
            Separator,
            item(
                t!("smart-menu-update-modified"),
                linked,
                Command::UpdateModified,
            ),
            item(t!("smart-menu-edit-contents"), is, Command::EditContents),
            item(
                t!("smart-menu-export-contents"),
                is,
                Command::ExportContents,
            ),
            item(t!("smart-menu-embed-linked"), linked, Command::EmbedLinked),
            item(
                t!("smart-menu-replace-contents"),
                is,
                Command::ReplaceContents,
            ),
            Separator,
            Item(t!("smart-menu-rasterize"), is, None, Action::Rasterize),
        ]
    }

    /// Whether a filter applied now becomes a smart filter: the active
    /// layer is a smart object, its pixels targeted.
    pub(super) fn smart_filters_apply(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.active_layer().is_smart() && !d.mask_targeted())
    }

    /// Image › Adjustments on a smart object: refused, as in Photoshop (an
    /// adjustment layer clipped to it does it).
    pub(super) fn refuse_on_smart(&mut self) {
        self.status = t!("smart-status-no-adjustments");
    }

    /// Why the painting tools refuse the active layer.
    pub(super) fn nothing_to_paint(&self) -> String {
        if self.smart_filters_apply() {
            t!("smart-status-not-editable")
        } else {
            t!("paint-status-nothing-to-paint")
        }
    }

    pub(super) fn smart_command(&mut self, cmd: Command) {
        use Command::*;
        let modifies = !matches!(cmd, PlaceLinked | EditContents | ExportContents | Convert);
        if self.doc.is_none() || (modifies && self.refuse_if_locked()) {
            return;
        }
        match cmd {
            Convert | ConvertForFilters => {
                let picked = match cmd {
                    Convert => self.selected_indices(),
                    _ => self
                        .doc
                        .as_ref()
                        .map(|d| vec![d.active])
                        .unwrap_or_default(),
                };
                if let Some(doc) = &mut self.doc {
                    if doc.convert_to_smart(&picked) {
                        self.layer_set.clear();
                        let name = doc.active_layer().name.as_str();
                        self.status = t!("smart-status-converted", name = name);
                        self.reset_pending();
                    } else {
                        self.status = t!("smart-status-locked");
                    }
                }
            }
            NewViaCopy => {
                if let Some(doc) = &mut self.doc
                    && doc.new_smart_via_copy()
                {
                    self.reset_pending();
                }
            }
            ResetTransform => self.reset_smart_transform(),
            UpdateModified => self.update_links(true),
            EditContents => self.edit_contents(),
            ExportContents => self.export_contents(),
            EmbedLinked => {
                self.edit_smart(t!("smart-menu-embed-linked"), |s| s.link = None);
            }
            ReplaceContents => {
                let Some(path) = self.pick_image(&t!("smart-menu-replace-contents")) else {
                    return;
                };
                let shown = path.display().to_string();
                if let Some(doc) = &mut self.doc {
                    self.status = match doc.replace_contents(&path) {
                        Ok(()) => t!("smart-status-replaced", path = shown),
                        Err(e) => t!("status-import-failed", path = shown, error = e.to_string()),
                    };
                    self.reset_pending();
                }
            }
            PlaceLinked => {
                if let Some(path) = self.pick_image(&t!("smart-menu-place-linked")) {
                    self.place_path(&path, true);
                }
            }
            EditFilter(k) => self.edit_smart_filter(k),
            FilterBlending(k) => self.open_filter_blending(k),
            ToggleFilter(k) => {
                let on = self
                    .active_smart()
                    .and_then(|s| s.filters.get(k))
                    .is_some_and(|f| !f.visible);
                let name = if on {
                    t!("smart-hist-show-filter")
                } else {
                    t!("smart-hist-hide-filter")
                };
                self.edit_smart(name, move |s| {
                    if let Some(f) = s.filters.get_mut(k) {
                        f.visible = on;
                    }
                });
            }
            DeleteFilter(k) => {
                self.edit_smart(t!("smart-menu-delete-filter"), move |s| {
                    if k < s.filters.len() {
                        s.filters.remove(k);
                    }
                });
            }
            MoveFilter(from, to) => {
                let n = self.active_smart().map_or(0, |s| s.filters.len());
                if from != to && from < n && to < n {
                    self.edit_smart(t!("smart-hist-move-filter"), move |s| {
                        let f = s.filters.remove(from);
                        s.filters.insert(to, f);
                    });
                }
            }
            ToggleFilters => {
                let on = self.active_smart().is_some_and(|s| !s.filters_enabled);
                let name = if on {
                    t!("smart-menu-enable-filters")
                } else {
                    t!("smart-menu-disable-filters")
                };
                self.edit_smart(name, move |s| s.filters_enabled = on);
            }
            ClearFilters => {
                self.edit_smart(t!("smart-menu-clear-filters"), |s| {
                    s.filters.clear();
                    s.filter_mask = None;
                });
            }
            AddFilterMask => {
                let selection = self.selection.clone();
                if let Some(doc) = &mut self.doc {
                    doc.add_filter_mask(doc.active, selection.as_ref());
                    doc.target_filter_mask(true);
                }
            }
            DeleteFilterMask => {
                if let Some(doc) = &mut self.doc
                    && doc.filter_mask_targeted()
                {
                    doc.target_mask(false);
                }
                self.edit_smart(t!("smart-menu-delete-filter-mask"), |s| {
                    s.filter_mask = None
                });
            }
            ToggleFilterMask => {
                let on = self
                    .active_smart()
                    .and_then(|s| s.filter_mask.as_ref())
                    .is_some_and(|m| !m.enabled);
                let name = if on {
                    t!("smart-menu-enable-filter-mask")
                } else {
                    t!("smart-menu-disable-filter-mask")
                };
                self.edit_smart(name, move |s| {
                    if let Some(m) = &mut s.filter_mask {
                        m.enabled = on;
                    }
                });
            }
            TargetFilterMask => {
                if let Some(doc) = &mut self.doc {
                    doc.target_filter_mask(true);
                    self.mask_view = false;
                }
            }
        }
    }

    /// The active layer's smart object.
    fn active_smart(&self) -> Option<&SmartObject> {
        self.doc.as_ref()?.active_layer().smart.as_deref()
    }

    /// Changes the active smart object with `f` as the undo step `name`,
    /// then renders it again: here when small, else on another thread
    /// (its busy indicator shown).
    fn edit_smart(&mut self, name: String, f: impl FnOnce(&mut SmartObject)) {
        let Some(doc) = &mut self.doc else { return };
        let i = doc.active;
        if !doc.edit_smart_later(i, name.clone(), f) {
            return;
        }
        let Some(job) = doc.smart_job(i) else { return };
        let id = doc.layers[i].id;
        if job.size() < ASYNC_PIXELS {
            doc.put_smart(id, job.run());
            self.end_layer_preview();
            self.reset_pending();
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job.run());
            ctx.request_repaint();
        });
        self.smart.rendering = Some(Rendering {
            doc: doc.uid(),
            layer: id,
            name,
            started: self.ctx.input(|i| i.time),
            rx,
        });
    }

    /// Place Embedded / Place Linked: the file as a smart object, then
    /// Free Transform to place it, like Photoshop. Without a document, the
    /// file opens.
    pub(super) fn place_path(&mut self, path: &Path, linked: bool) {
        self.commit_pending();
        let Some(doc) = &mut self.doc else {
            self.open_path(path);
            return;
        };
        let shown = path.display().to_string();
        match doc.place_file(path, linked) {
            Ok(animated) => {
                self.status = if animated {
                    t!("file-status-imported-first-frame", path = shown)
                } else {
                    t!("smart-status-placed", path = shown)
                };
                self.reset_pending();
                self.run(Action::Transform(transform::Command::Free));
                if let Some(doc) = self.doc.as_ref().filter(|_| self.transform_pending()) {
                    let name = if linked {
                        t!("smart-hist-place-linked")
                    } else {
                        t!("smart-hist-place-embedded")
                    };
                    self.smart.placing = Some(Placing {
                        doc: doc.uid(),
                        step: doc.history_step(),
                        name,
                        committing: false,
                    });
                }
            }
            Err(e) => {
                self.status = t!("status-import-failed", path = shown, error = e.to_string());
            }
        }
    }

    /// Enter on the placing Free Transform: it applies.
    pub(super) fn commit_placing(&mut self) {
        if let Some(p) = &mut self.smart.placing {
            p.committing = true;
        }
    }

    /// The placing Free Transform applied: one step with the place, under
    /// its name.
    pub(super) fn join_placing(&mut self) {
        let Some(p) = self.smart.placing.take() else {
            return;
        };
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.history_step()) == (p.doc, p.step + 1)
        {
            doc.join_last_step();
            doc.rename_last_step(p.name);
        }
    }

    /// The placing Free Transform given up (Esc, Cancel): the placed layer
    /// goes, without a step left, like Photoshop.
    pub(super) fn cancel_placing(&mut self) {
        let Some(p) = self.smart.placing.take() else {
            return;
        };
        if p.committing {
            // Still being computed: joined once done.
            if self.transform_busy() {
                self.smart.placing = Some(p);
            }
            return;
        }
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.history_step()) == (p.doc, p.step)
        {
            doc.discard_step();
            self.status.clear();
        }
    }

    /// Edit Contents: the contents in a tab of their own (the one already
    /// showing them, if any); saving them updates the smart object.
    fn edit_contents(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some(contents) = doc.smart_contents(doc.active) else {
            return;
        };
        let wanted = contents.embedded_in.clone();
        let open = (0..self.tab_count()).find(|&i| {
            let shows = |e: &EmbeddedIn| {
                wanted
                    .as_ref()
                    .is_some_and(|w| (w.parent, w.contents) == (e.parent, e.contents))
            };
            self.tab(i)
                .and_then(|(d, _)| d.embedded_in.as_ref())
                .is_some_and(shows)
        });
        match open {
            Some(i) => self.switch_tab(i),
            None => self.add_tab(contents),
        }
    }

    /// Save of a smart object's contents (Edit Contents): its smart object
    /// shows them. Returns whether the active document is such contents.
    pub(super) fn save_embedded(&mut self) -> bool {
        self.commit_pending();
        let Some(doc) = &mut self.doc else {
            return false;
        };
        let Some(e) = doc.embedded_in.clone() else {
            return false;
        };
        let contents = doc.to_contents(e.contents, e.name.clone());
        let parent = self.parked.iter_mut().find(|p| p.doc.uid() == e.parent);
        match parent.map(|p| p.doc.update_contents(e.contents, contents)) {
            Some(true) => {
                doc.modified = false;
                self.status = t!("smart-status-updated", name = e.name.as_str());
            }
            // Its document is closed, or its smart object deleted: a file.
            _ => {
                doc.embedded_in = None;
                self.status = t!("smart-status-parent-gone");
                self.save_as_dialog();
            }
        }
        true
    }

    /// Export Contents: the contents saved as a document of their own.
    fn export_contents(&mut self) {
        let Some(doc) = &self.doc else { return };
        let Some(mut contents) = doc.smart_contents(doc.active) else {
            return;
        };
        contents.embedded_in = None;
        let atelier = t!("file-filter-atelier");
        let psd = t!("xchg-file-filter-psd");
        let filters = [
            (atelier.as_str(), &["atelier"][..]),
            (psd.as_str(), &["psd"][..]),
        ];
        let name = contents.display_name();
        let stem = Path::new(&name)
            .file_stem()
            .map_or(name.clone(), |s| s.to_string_lossy().into_owned());
        let dir = doc
            .path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let title = t!("smart-menu-export-contents");
        let Some(path) = files::ask_save_path(&title, &filters, dir, &stem) else {
            return;
        };
        let shown = path.display().to_string();
        self.status = match contents.save(&path) {
            Ok(_) => t!("status-saved", path = shown),
            Err(e) => t!(
                "file-status-save-failed",
                name = shown,
                error = e.to_string()
            ),
        };
    }

    /// Reset Transform: the contents at 100 %, unturned, centered where
    /// they are.
    fn reset_smart_transform(&mut self) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let Some(s) = layer.smart.as_deref() else {
            return;
        };
        let t = s.transform(layer.offset);
        let (w, h) = s.contents.size();
        let Some(b) = t.bounds([0.0, 0.0, w as f64, h as f64]) else {
            return;
        };
        let center = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        let (dx, dy) = (center[0] - w as f64 / 2.0, center[1] - h as f64 / 2.0);
        // On whole pixels, the contents show as they are.
        let (dx, dy) = (
            dx.round() - layer.offset.0 as f64,
            dy.round() - layer.offset.1 as f64,
        );
        let map = Map::Projective(Homography::affine([1.0, 0.0, dx, 0.0, 1.0, dy]));
        self.edit_smart(t!("smart-menu-reset-transform"), move |s| s.map = map);
    }

    /// Linked smart objects whose file changed are shown again (`all`: of
    /// every open document, else of the active one).
    fn update_links(&mut self, all: bool) {
        let mut report = crate::document::LinkReport::default();
        let mut merge = |r: crate::document::LinkReport| {
            report.updated.extend(r.updated);
            report.missing.extend(r.missing);
        };
        if let Some(doc) = &mut self.doc {
            merge(doc.update_links());
        }
        if all {
            for p in &mut self.parked {
                merge(p.doc.update_links());
            }
        }
        if !report.missing.is_empty() {
            self.status = t!(
                "smart-status-link-missing",
                names = report.missing.join(", ")
            );
        } else if !report.updated.is_empty() {
            self.status = t!(
                "smart-status-links-updated",
                names = report.updated.join(", ")
            );
        }
        if !report.updated.is_empty() {
            self.reset_pending();
        }
    }

    /// Every frame: the rendering being computed, linked files when the
    /// window gets the focus back (or another document shows).
    pub(super) fn poll_smart(&mut self, ctx: &egui::Context) {
        if let Some(r) = &self.smart.rendering {
            let done = match r.rx.try_recv() {
                Ok(done) => Some(done),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => None,
            };
            let Some(r) = self.smart.rendering.take() else {
                return;
            };
            if let (Some(done), Some(doc)) = (done, &mut self.doc)
                && doc.uid() == r.doc
            {
                doc.put_smart(r.layer, done);
            }
            self.end_layer_preview();
            self.reset_pending();
        }
        // The filter dialog closed: no smart filter re-edited any more.
        if !self.filters.busy() && !self.liquify.busy() && !self.camera_raw.busy() {
            self.smart.editing = None;
            self.smart.input = None;
        }
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let regained = focused && !self.smart.focused;
        self.smart.focused = focused;
        let uid = self.doc.as_ref().map_or(0, Document::uid);
        if (regained || uid != self.smart.links_of) && !self.modal_open() {
            self.smart.links_of = uid;
            self.update_links(regained);
        }
    }

    /// The busy indicator of a rendering, the Blending Options window.
    pub(super) fn smart_windows(&mut self, ctx: &egui::Context) {
        if let Some(r) = &self.smart.rendering {
            filters_ui::busy_indicator(ctx, &r.name, r.started);
            return;
        }
        self.filter_blending_window(ctx);
    }

    // ------------------------------------------------------ smart filters

    /// Re-edits smart filter `k` of the active smart object in its dialog
    /// or Liquify's workspace (not one without settings).
    fn edit_smart_filter(&mut self, k: usize) {
        let Some(f) = self.active_smart().and_then(|s| s.filters.get(k)) else {
            return;
        };
        match f.filter.clone() {
            SmartFilterKind::Filter(f) if f.kind().has_settings() => {
                self.open_filter_dialog(f);
                self.smart.editing = Some(k);
            }
            SmartFilterKind::Liquify(mesh) => {
                // What it distorts depends on the filter edited.
                self.smart.editing = Some(k);
                self.open_liquify_with(Some(&mesh));
            }
            SmartFilterKind::CameraRaw(settings) => {
                self.smart.editing = Some(k);
                self.camera_raw_filter(Some((*settings).clone()));
            }
            SmartFilterKind::Filter(_) => {}
        }
    }

    /// What the filter dialog filters for a smart object: the rendering
    /// through the filters below the one edited (all of them for a new one).
    pub(super) fn smart_filter_input(&mut self) -> Option<Target> {
        if !self.smart_filters_apply() {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let layer = doc.active_layer();
        let key = (layer.id, layer.rev(), self.smart.editing);
        if let Some((k, target)) = &self.smart.input
            && *k == key
        {
            return Some(target.clone());
        }
        let image = doc.smart_filter_input(doc.active, self.smart.editing)?;
        let target = Target {
            image,
            clip: None,
            offset: layer.offset,
        };
        self.smart.input = Some((key, target.clone()));
        Some(target)
    }

    /// The filters of the active smart object with `filter` applied: in
    /// the place of the one re-edited, else on top.
    fn with_filter(&self, filter: SmartFilterKind) -> Vec<SmartFilter> {
        let mut filters = self
            .active_smart()
            .map(|s| s.filters.clone())
            .unwrap_or_default();
        match self.smart.editing {
            Some(k) if k < filters.len() => filters[k].filter = filter,
            _ => filters.push(SmartFilter::new(filter)),
        }
        filters
    }

    /// The filter dialog's canvas preview on a smart object: rendered again
    /// with the filter in its place.
    pub(super) fn smart_filter_preview(&mut self, ctx: &egui::Context, filter: Filter, on: bool) {
        let filters = self.with_filter(filter.into());
        self.smart_preview(ctx, filters, on);
    }

    /// Shows the active smart object rendered with `filters` (computed on
    /// another thread), while `on`.
    fn smart_preview(&mut self, ctx: &egui::Context, filters: Vec<SmartFilter>, on: bool) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let key = hash_of((layer.id, layer.rev(), format!("{filters:?}")));
        if let Some((k, rx)) = &self.smart.preview
            && let Ok(done) = rx.try_recv()
        {
            let k = *k;
            self.smart.preview = None;
            if on {
                self.show_preview(LayerPreview::Filter(k), done.into_image());
            }
        }
        let shown = matches!(
            self.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(k)) if *k == key
        );
        if !on || shown || self.smart.preview.is_some() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let Some(job) = doc.smart_preview_job(doc.active, move |s| s.filters = filters) else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job.run());
            ctx.request_repaint();
        });
        self.smart.preview = Some((key, rx));
    }

    /// A filter applied to the active smart object (the filter dialog's
    /// OK, Liquify's, Last Filter): a smart filter, the one re-edited
    /// replaced. The first one, with a selection, gets its filter mask
    /// from it.
    pub(super) fn add_smart_filter(&mut self, filter: impl Into<SmartFilterKind>) {
        let filter = filter.into();
        let name = filter_name(&filter);
        let filters = self.with_filter(filter);
        self.smart.editing = None;
        self.smart.input = None;
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let first = layer
            .smart
            .as_ref()
            .is_some_and(|s| s.filters.is_empty() && s.filter_mask.is_none());
        let canvas = doc.full_rect();
        let mask = self.selection.as_ref().filter(|_| first).map(|sel| {
            let outside = if sel.inverted { 255 } else { 0 };
            crate::document::Mask::new(
                sel.to_mask(canvas),
                (canvas.x, canvas.y),
                outside,
                layer.offset,
            )
        });
        self.status = t!("filter-applied", name = name.as_str());
        self.edit_smart(name, move |s| {
            s.filters = filters;
            if mask.is_some() {
                s.filter_mask = mask;
            }
        });
    }

    /// Blending Options of smart filter `k` (double-click on its icon).
    fn open_filter_blending(&mut self, k: usize) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer().id;
        let Some(f) = self.active_smart().and_then(|s| s.filters.get(k)).cloned() else {
            return;
        };
        self.smart.blending = Some(Blending {
            layer,
            index: k,
            opened: f.clone(),
            filter: f,
            preview: true,
        });
    }

    /// Blending Options (filter name): mode and opacity, previewed.
    fn filter_blending_window(&mut self, ctx: &egui::Context) {
        let Some(mut b) = self.smart.blending.take() else {
            return;
        };
        let active = self.doc.as_ref().map(|d| d.active_layer().id);
        if active != Some(b.layer) {
            return self.end_layer_preview();
        }
        let title = t!("smart-blending-title", name = filter_name(&b.filter.filter));
        let mut preview = b.preview;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                reset: Some(b.filter != b.opened),
                preview: Some(&mut preview),
                ..DialogSpec::new("smart-blending", title, dialogs::tool_dialog_pos(ctx))
            },
            |ui| {
                ui_kit::field_label(ui, &t!("panel-blend-mode"));
                let width = ui.available_width();
                right_panel::blend_combo(
                    ui,
                    "smart-blending-mode",
                    &mut b.filter.blend,
                    false,
                    width,
                );
                gap_between_blocks(ui);
                let mut opacity = b.filter.opacity * 100.0;
                slider_row(
                    ui,
                    &t!("panel-opacity"),
                    None,
                    &mut opacity,
                    0.0..=100.0,
                    &i18n::percent(""),
                    Track::Accent,
                    100.0,
                );
                b.filter.opacity = opacity / 100.0;
            },
        );
        b.preview = preview;
        if out.reset {
            b.filter = b.opened.clone();
        }
        if out.ok {
            let (k, f) = (b.index, b.filter.clone());
            if f != b.opened {
                self.edit_smart(t!("smart-hist-blending"), move |s| {
                    if let Some(old) = s.filters.get_mut(k) {
                        *old = f;
                    }
                });
            }
            self.end_layer_preview();
            return;
        }
        if out.cancel {
            return self.end_layer_preview();
        }
        let mut filters = self
            .active_smart()
            .map(|s| s.filters.clone())
            .unwrap_or_default();
        if let Some(old) = filters.get_mut(b.index) {
            *old = b.filter.clone();
        }
        if b.preview {
            self.smart_preview(ctx, filters, true);
        } else {
            self.end_layer_preview();
        }
        self.smart.blending = Some(b);
    }
}
