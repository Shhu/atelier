//! Smart objects in the app: Layer › Smart Objects, File › Place Embedded
//! and Place Linked (drag and drop and pasted files too), Edit Contents in
//! a tab of its own whose Save updates the smart object, linked files
//! followed when the window gets the focus back, and the smart filters:
//! the filters of the Filter menu applied to a smart object, re-edited
//! from the Layers panel, with their Blending Options.

use std::sync::mpsc;

use super::actions_panel::Stopped;
use super::dialogs::{DialogSpec, Track, floating_dialog, gap_between_blocks, slider_row};
use super::*;
use crate::document::{
    EmbeddedIn, LinkFresh, LinkRead, SmartFilter, SmartFilterKind, SmartObject, SmartRender,
};
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
    /// The preview of a dialog being computed (its key in the job).
    preview: Option<filters_ui::Job<SmartRender>>,
    /// The key of a dialog preview whose computation failed.
    failed: Option<u64>,
    /// The last preview computed (its key), its pixels shown (taken out
    /// into `preview_shown`): OK puts it in instead of computing it again.
    ready: Option<(u64, SmartRender)>,
    /// A file being placed.
    placing: Option<Placing>,
    /// Smart filter row being dragged: its layer and index.
    pub(super) drag: Option<(u64, usize)>,
    /// The window had the focus at the last frame.
    pub(super) focused: bool,
    /// Document whose links were last looked at.
    links_of: u64,
    /// Linked files being read again, by document.
    links: Vec<(u64, mpsc::Receiver<Vec<LinkFresh>>)>,
    /// Those reads were asked for (Update Modified Content): every
    /// missing file said again.
    links_asked: bool,
    /// Documents of linked image files opened by Edit Contents: Save
    /// writes them in their format.
    link_files: Vec<u64>,
    /// Save of such a file that merges its layers or loses something:
    /// asked first.
    pub(super) link_save: Option<LinkSave>,
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

/// "Save logo.png?" for a linked file opened by Edit Contents whose format
/// can't hold all the tab holds.
pub(super) struct LinkSave {
    doc: u64,
    text: String,
    /// The close that asked to save it, done once saved.
    pub(super) then: Option<Confirm>,
}

struct Blending {
    layer: u64,
    index: usize,
    opened: SmartFilter,
    filter: SmartFilter,
    preview: bool,
}

/// A file just placed, its Free Transform being prepared.
struct Placing {
    path: PathBuf,
    linked: bool,
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
    /// The history step the change made: cancelling it undoes it.
    step: u64,
    name: String,
    started: f64,
    job: filters_ui::Job<SmartRender>,
    /// Done once it is in (the step recorded in an action…).
    then: Option<Then>,
}

/// What follows a smart object's change once rendered.
type Then = Box<dyn FnOnce(&mut AtelierApp)>;

/// The rendering of a change, from a dialog's preview of it.
enum SmartReady {
    Render(Box<SmartRender>),
    Job(filters_ui::Job<SmartRender>),
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
            UpdateModified => self.update_links(true, true),
            EditContents => self.edit_contents(),
            ExportContents => self.export_contents(),
            EmbedLinked => {
                if let Some(doc) = &mut self.doc
                    && doc.embed_linked(doc.active, t!("smart-menu-embed-linked"))
                {
                    self.reset_pending();
                }
            }
            ReplaceContents => {
                let Some(path) = self.pick_image(&t!("smart-menu-replace-contents")) else {
                    return;
                };
                let shown = path.display().to_string();
                if let Some(doc) = &mut self.doc {
                    match doc.replace_contents(&path) {
                        Ok(()) => self.status = t!("smart-status-replaced", path = shown),
                        Err(e) => self.report_error(tm!(
                            "status-import-failed",
                            path = shown,
                            error = crate::i18n::Message::of(|| e.to_string())
                        )),
                    }
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
    /// then renders it again: here when small and without filters (whose
    /// cost can't be told), else on another thread (its busy indicator
    /// shown).
    fn edit_smart(&mut self, name: String, f: impl FnOnce(&mut SmartObject)) {
        self.edit_smart_then(name, f, None, None);
    }

    /// [`Self::edit_smart`], then `then` once rendered (not if the
    /// rendering is cancelled or fails). `ready`: the rendering of the
    /// change, computed or being computed already (a dialog's preview).
    fn edit_smart_then(
        &mut self,
        name: String,
        f: impl FnOnce(&mut SmartObject),
        then: Option<Then>,
        ready: Option<SmartReady>,
    ) {
        // A rendering still being computed: the new one shows its change
        // too.
        if let Some(r) = self.smart.rendering.take() {
            r.job.stop();
            if let Some(then) = r.then {
                then(self);
            }
        }
        let Some(doc) = &mut self.doc else { return };
        let i = doc.active;
        if !doc.edit_smart_later(i, name.clone(), f) {
            return;
        }
        let id = doc.layers[i].id;
        let job = match ready {
            Some(SmartReady::Render(done)) => {
                doc.put_smart(id, *done);
                return self.smart_rendered(then);
            }
            Some(SmartReady::Job(job)) => job,
            None => {
                let Some(job) = doc.smart_job(i) else { return };
                if job.size() < ASYNC_PIXELS && !job.has_filters() {
                    doc.put_smart(id, job.run());
                    return self.smart_rendered(then);
                }
                // Stopped (its filters) when cancelled.
                let label = Some("Smart object rendering".to_owned());
                filters_ui::Job::spawn(&self.ctx, 0, [0; 4], label, move || job.run())
            }
        };
        self.smart.rendering = Some(Rendering {
            doc: doc.uid(),
            layer: id,
            step: doc.history_step(),
            name,
            started: self.ctx.input(|i| i.time),
            job,
            then,
        });
    }

    /// A smart object's change rendered and shown.
    fn smart_rendered(&mut self, then: Option<Then>) {
        self.end_layer_preview();
        self.reset_pending();
        if let Some(then) = then {
            then(self);
        }
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
                        path: path.to_path_buf(),
                        linked,
                        doc: doc.uid(),
                        step: doc.history_step(),
                        name,
                        committing: false,
                    });
                }
            }
            Err(e) => {
                self.report_error(tm!(
                    "status-import-failed",
                    path = shown,
                    error = crate::i18n::Message::of(|| e.to_string())
                ));
            }
        }
    }

    /// The file being placed (its Free Transform not applied yet) and
    /// whether it is linked.
    pub(super) fn placing_file(&self) -> Option<(PathBuf, bool)> {
        let p = self.smart.placing.as_ref().filter(|p| !p.committing)?;
        let doc = self.doc.as_ref().filter(|d| d.uid() == p.doc)?;
        (doc.history_step() == p.step).then(|| (p.path.clone(), p.linked))
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
    /// showing them, if any); saving them updates the smart object. A
    /// linked smart object's file opens itself: saved, it updates every
    /// smart object linked to it.
    fn edit_contents(&mut self) {
        let linked = self
            .active_smart()
            .and_then(|s| s.link.as_ref())
            .filter(|l| !l.missing && l.path.exists())
            .map(|l| l.path.clone());
        if let Some(path) = linked {
            self.open_path(&path);
            let opened = self
                .doc
                .as_ref()
                .filter(|d| d.path.as_deref() == Some(path.as_path()));
            if let Some(doc) = opened
                && !crate::document::is_native(&path)
                && !crate::document::is_psd(&path)
            {
                let uid = doc.uid();
                if !self.smart.link_files.contains(&uid) {
                    self.smart.link_files.push(uid);
                }
            }
            return;
        }
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
        // A link whose file is missing: embedded, or its file would
        // replace these edits once back.
        let linked = parent.as_ref().is_some_and(|p| {
            p.doc.layers.iter().any(|l| {
                l.smart
                    .as_ref()
                    .is_some_and(|s| s.contents.id == e.contents && s.link.as_ref().is_some_and(|k| k.gone()))
            })
        });
        match parent.map(|p| p.doc.update_contents(e.contents, contents)) {
            Some(true) => {
                doc.mark_saved();
                self.status = if linked {
                    t!("smart4-status-updated-embedded", name = e.name.as_str())
                } else {
                    t!("smart-status-updated", name = e.name.as_str())
                };
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

    /// Save of a linked image file opened by Edit Contents: written over
    /// it in its format, the smart objects linked to it then follow.
    /// Returns whether the active document is such a file.
    pub(super) fn save_linked_file(&mut self) -> bool {
        let open: Vec<u64> = (0..self.tab_count())
            .filter_map(|i| self.tab(i).map(|(d, _)| d.uid()))
            .collect();
        self.smart.link_files.retain(|d| open.contains(d));
        let Some(doc) = &self.doc else { return false };
        let Some(path) = doc.path.clone() else {
            return false;
        };
        if !self.smart.link_files.contains(&doc.uid()) || super::batch::format_of(&path).is_none() {
            return false;
        }
        self.commit_pending();
        let Some(doc) = &self.doc else { return true };
        let name = files::file_name(&path);
        // Its format can't hold the layers, the depth, the inks or the
        // metadata the tab holds: asked first, never done silently.
        let mut text = Vec::new();
        if doc.layers.len() > 1 {
            text.push(t!("smart5-link-save-merge", name = name.as_str()));
        }
        if !doc.file_notes.is_empty() {
            let notes = doc.file_notes.join(" · ");
            text.push(t!("smart5-link-save-lose", name = name.as_str(), notes = notes));
        }
        match (!text.is_empty()).then(|| text.join("\n\n")) {
            Some(text) => {
                self.smart.link_save = Some(LinkSave {
                    doc: doc.uid(),
                    text,
                    then: None,
                })
            }
            None => {
                self.write_linked_file();
            }
        }
        true
    }

    /// The linked file written over in its format (its layers merged, what
    /// it can't hold left out); whether it was.
    fn write_linked_file(&mut self) -> bool {
        let Some(doc) = &mut self.doc else {
            return false;
        };
        let Some(name) = doc.path.as_deref().map(files::file_name) else {
            return false;
        };
        let layered = doc.layers.len() > 1;
        // Agreed to: written without them (the file holds them no more).
        let notes = std::mem::take(&mut doc.file_notes);
        match self.save_in_place(crate::export::ExportOptions::default().quality) {
            Ok(()) => {
                self.status = if layered {
                    t!("smart4-status-link-saved-flat", name = name.as_str())
                } else {
                    t!("smart4-status-link-saved", name = name.as_str())
                };
                true
            }
            Err(e) => {
                if let Some(doc) = &mut self.doc {
                    doc.file_notes = notes;
                }
                self.report_error(e);
                false
            }
        }
    }

    /// "Save logo.png?": Save (merged, or without what it can't hold),
    /// Save As…, Cancel. A close waiting for it goes on once saved.
    fn link_save_window(&mut self, ctx: &egui::Context) {
        let Some(ask) = &self.smart.link_save else {
            return;
        };
        let Some(doc) = self.doc.as_ref().filter(|d| d.uid() == ask.doc) else {
            self.smart.link_save = None;
            return;
        };
        let title = t!("smart5-link-save-title", name = doc.display_name());
        let text = ask.text.clone();
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 420.0,
                ok: t!("smart5-link-save-ok"),
                other: Some(t!("smart5-link-save-as")),
                ..DialogSpec::new("link_save", title, center)
            },
            |ui| {
                ui.label(text);
            },
        );
        if !out.ok && !out.other && !out.cancel {
            return;
        }
        let Some(ask) = self.smart.link_save.take() else {
            return;
        };
        if out.cancel {
            return;
        }
        if out.ok {
            if !self.write_linked_file() {
                return;
            }
        } else {
            self.save_as_dialog();
        }
        if self.doc.as_ref().is_some_and(|d| d.modified) {
            return;
        }
        match ask.then {
            Some(Confirm::Close) => {
                self.close_document();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Some(Confirm::CloseDocument) => self.close_document(),
            None => {}
        }
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
        match contents.save(&path) {
            Ok(_) => self.status = t!("status-saved", path = shown),
            Err(e) => self.report_error(tm!(
                "file-status-save-failed",
                name = files::file_name(&path),
                error = crate::i18n::Message::of(|| e.to_string())
            )),
        }
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

    /// Linked smart objects whose file changed are read again on other
    /// threads, then shown (`all`: of every open document, else of the
    /// active one). Files on another machine only when asked (`network`:
    /// Update Modified Content), never just by showing a document.
    fn update_links(&mut self, all: bool, network: bool) {
        let mut docs: Vec<&mut Document> = self.doc.iter_mut().collect();
        if all {
            docs.extend(self.parked.iter_mut().map(|p| &mut p.doc));
        }
        for doc in docs {
            let uid = doc.uid();
            if self.smart.links.iter().any(|(d, _)| *d == uid) {
                continue;
            }
            let reads = doc.stale_links(network);
            if reads.is_empty() {
                continue;
            }
            let (tx, rx) = mpsc::channel();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                let _job = crate::diag::job("Linked files: read");
                let _ = tx.send(reads.into_iter().map(LinkRead::run).collect());
                ctx.request_repaint();
            });
            self.smart.links.push((uid, rx));
            self.smart.links_asked |= network;
        }
    }

    /// The linked files read again shown, each document's as one step
    /// (not while a dialog, a gesture or a transform is going on).
    fn put_links(&mut self, ctx: &egui::Context) {
        if self.smart.links.is_empty()
            || self.modal_open()
            || self.transform_pending()
            || ctx.input(|i| i.pointer.any_down())
        {
            return;
        }
        let mut report = crate::document::LinkReport::default();
        let mut active = false;
        let jobs = std::mem::take(&mut self.smart.links);
        for (uid, rx) in jobs {
            let fresh = match rx.try_recv() {
                Ok(fresh) => fresh,
                Err(mpsc::TryRecvError::Empty) => {
                    self.smart.links.push((uid, rx));
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => continue,
            };
            let doc = match self.doc.as_mut().filter(|d| d.uid() == uid) {
                Some(doc) => {
                    active = true;
                    Some(doc)
                }
                None => self
                    .parked
                    .iter_mut()
                    .map(|p| &mut p.doc)
                    .find(|d| d.uid() == uid),
            };
            if let Some(doc) = doc {
                let r = doc.put_links(fresh);
                report.updated.extend(r.updated);
                report.missing.extend(r.missing);
                report.still_missing.extend(r.still_missing);
                report.unreadable.extend(r.unreadable);
            }
        }
        // Said once per file gone (the Layers panel badge shows it), unless
        // Update Modified Content asked.
        if self.smart.links_asked {
            report.missing.append(&mut report.still_missing);
            self.smart.links_asked = !self.smart.links.is_empty();
        }
        if !report.missing.is_empty() {
            self.status = t!(
                "smart-status-link-missing",
                names = report.missing.join(", ")
            );
        } else if !report.unreadable.is_empty() {
            self.status = t!(
                "smart4-status-link-unreadable",
                names = report.unreadable.join(", ")
            );
        } else if !report.updated.is_empty() {
            self.status = t!(
                "smart-status-links-updated",
                names = report.updated.join(", ")
            );
        }
        if active && !report.updated.is_empty() {
            self.end_layer_preview();
            self.reset_pending();
        }
    }

    /// Linked files still being read.
    #[cfg(test)]
    pub(super) fn links_reading(&self) -> bool {
        !self.smart.links.is_empty()
    }

    /// Every frame: the rendering being computed, linked files when the
    /// window gets the focus back (or another document shows).
    pub(super) fn poll_smart(&mut self, ctx: &egui::Context) {
        if let Some(r) = &self.smart.rendering {
            let done = match r.job.poll() {
                Ok(Some(done)) => Some(done),
                Ok(None) => return,
                Err(()) => None,
            };
            let Some(r) = self.smart.rendering.take() else {
                return;
            };
            match done {
                Some(done) => {
                    if let Some(doc) = &mut self.doc
                        && doc.uid() == r.doc
                    {
                        doc.put_smart(r.layer, done);
                        if let Some(then) = r.then {
                            then(self);
                        }
                    }
                }
                // Failed: the change is undone (its rendering is missing).
                None => {
                    self.undo_smart_change(&r);
                    let text = tm!("fx3-filter-failed", name = r.name.as_str());
                    self.actions
                        .step_stopped(Stopped::Failed(text.shown.clone()));
                    self.status_failed(text);
                }
            }
            self.end_layer_preview();
            self.reset_pending();
        }
        // No dialog previews any more: its computation is stopped.
        if self.smart.preview.is_some()
            && !self.filters.busy()
            && !self.liquify.busy()
            && !self.camera_raw.busy()
            && self.smart.blending.is_none()
        {
            self.smart.preview = None;
            self.smart.ready = None;
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
            self.update_links(regained, false);
        }
        self.put_links(ctx);
    }

    /// The busy indicator of a rendering, the Blending Options window.
    pub(super) fn smart_windows(&mut self, ctx: &egui::Context) {
        if let Some(r) = &self.smart.rendering {
            if filters_ui::cancellable_indicator(ctx, "smart_busy", &r.name, r.started, None) {
                self.cancel_smart_rendering();
            }
            return;
        }
        self.link_save_window(ctx);
        self.filter_blending_window(ctx);
    }

    /// Cancel of the rendering being computed: the change it was for is
    /// undone.
    pub(super) fn cancel_smart_rendering(&mut self) {
        let Some(r) = self.smart.rendering.take() else {
            return;
        };
        r.job.stop();
        self.undo_smart_change(&r);
        self.status = t!("retouch-status-cancelled", name = r.name.as_str());
        self.actions.step_stopped(Stopped::Cancelled);
        self.end_layer_preview();
        self.reset_pending();
    }

    /// The change a rendering was for, undone (cancelled or failed), if
    /// it is still the last step of its document.
    fn undo_smart_change(&mut self, r: &Rendering) {
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.history_step()) == (r.doc, r.step)
        {
            doc.discard_step();
        }
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
        if let Some(job) = &self.smart.preview {
            match job.poll() {
                Ok(Some(done)) => {
                    let (k, stopped) = (job.key(), job.stopped());
                    self.smart.preview = None;
                    if on && !stopped {
                        let (image, rest) = done.take_image();
                        self.show_preview(LayerPreview::Filter(k), image);
                        let shown = self.preview_shown.as_ref().is_some_and(
                            |p| matches!(p.preview, LayerPreview::Filter(x) if x == k),
                        );
                        self.smart.ready = shown.then_some((k, rest));
                    }
                }
                Ok(None) => {}
                Err(()) => {
                    // Not started again for the same settings.
                    self.smart.failed = Some(job.key());
                    self.smart.preview = None;
                    self.end_layer_preview();
                    self.status_failed(tm!("fx4-preview-failed-any"));
                }
            }
        }
        // Settings changed, or no preview wanted: the one being computed
        // is stopped (it would only delay the next).
        if let Some(job) = &self.smart.preview
            && (!on || job.key() != key)
        {
            self.smart.preview = None;
        }
        let shown = matches!(
            self.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(k)) if *k == key
        );
        if !on || shown || self.smart.preview.is_some() || self.smart.failed == Some(key) {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let Some(job) = doc.smart_preview_job(doc.active, move |s| s.filters = filters) else {
            return;
        };
        let label = Some("Smart filter preview".to_owned());
        let job = filters_ui::Job::spawn(ctx, key, [0; 4], label, move || job.run());
        self.smart.preview = Some(job);
    }

    /// A filter applied to the active smart object (the filter dialog's
    /// OK, Liquify's, Last Filter): a smart filter, the one re-edited
    /// replaced. The first one, with a selection, gets its filter mask
    /// from it. Then `then` once rendered (not if the rendering is
    /// cancelled or fails): what Last Filter repeats, the step of an action
    /// being recorded.
    pub(super) fn add_smart_filter(
        &mut self,
        filter: impl Into<SmartFilterKind>,
        then: impl FnOnce(&mut AtelierApp) + 'static,
    ) {
        let filter = filter.into();
        let name = filter_name(&filter);
        let filters = self.with_filter(filter);
        self.smart.editing = None;
        self.smart.input = None;
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        // As `smart_preview` keys it.
        let key = hash_of((layer.id, layer.rev(), format!("{filters:?}")));
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
        // The dialog's preview of the same filters is the result (not with
        // a filter mask it didn't have).
        let ready = if mask.is_some() {
            None
        } else {
            self.preview_of_smart(key)
        };
        self.status = t!("filter-applied", name = name.as_str());
        let change = move |s: &mut SmartObject| {
            s.filters = filters;
            if mask.is_some() {
                s.filter_mask = mask;
            }
        };
        self.edit_smart_then(name, change, Some(Box::new(then)), ready);
    }

    /// The dialog's preview with key `key`, computed (and shown) or being
    /// computed.
    fn preview_of_smart(&mut self, key: u64) -> Option<SmartReady> {
        if let Some((k, rest)) = self.smart.ready.take()
            && k == key
            && let Some(p) = self.preview_shown.take_if(|p| {
                matches!(p.preview, LayerPreview::Filter(x) if x == key) && p.mask.is_none()
            })
        {
            let image = Arc::try_unwrap(p.image).unwrap_or_else(|a| (*a).clone());
            return Some(SmartReady::Render(Box::new(rest.with_image(image))));
        }
        let job = self.smart.preview.take()?;
        if job.key() == key && !job.stopped() {
            return Some(SmartReady::Job(job));
        }
        self.smart.preview = Some(job);
        None
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
