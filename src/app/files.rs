//! Document files, like Photoshop: Open and Import, Save / Save As (Atelier
//! documents, every layer kept), drag and drop, the clipboard, and what
//! counts as unsaved. Export As is in `export_as`.

use super::*;
use crate::document::is_native;

/// What Open and Import as Layer offer.
pub(super) const OPEN_EXTENSIONS: &[&str] = &[
    "atelier", "psd", "psb", "png", "jpg", "jpeg", "jpe", "jfif", "bmp", "gif", "webp", "tif",
    "tiff", "tga", "ico", "qoi",
];

impl AtelierApp {
    pub(super) fn open_path(&mut self, path: &Path) {
        // Already open: show its tab, like Photoshop.
        if let Some(i) = self.tab_of(path) {
            self.switch_tab(i);
            return;
        }
        // A camera RAW file opens in Camera Raw first.
        if crate::camera_raw::raw::is_raw(path) {
            self.open_raw(path);
            return;
        }
        let shown = path.display().to_string();
        let started = std::time::Instant::now();
        let opened = Document::open(path);
        crate::diag::record(format!("Open: {}", format_of(path)), started.elapsed());
        match opened {
            Ok(opened) => {
                self.status = if opened.animated {
                    t!("file-status-opened-first-frame", path = shown)
                } else if !opened.notes.is_empty() {
                    let notes = opened.notes.join(" · ");
                    crate::diag::warn(tm!(
                        "xchg-status-opened-simplified",
                        path = file_name(path),
                        notes = notes.clone()
                    ));
                    t!("xchg-status-opened-simplified", path = shown, notes = notes)
                } else {
                    t!("status-opened", path = shown)
                };
                // Batch's files don't push the user's out of Open Recent.
                if !self.automate.running() {
                    self.recent.add(path);
                }
                self.add_tab(opened.doc);
            }
            Err(e) => {
                // The name only, so that the cause stays in view.
                let name = file_name(path);
                self.report_error(tm!(
                    "status-open-failed",
                    path = name,
                    error = e.to_string()
                ));
            }
        }
    }

    /// Adds the image as a layer, or opens it if there is no document yet.
    pub(super) fn pick_image(&self, title: &str) -> Option<PathBuf> {
        use crate::camera_raw::raw::EXTENSIONS as RAW;
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(
                t!("file-filter-supported"),
                &[OPEN_EXTENSIONS, RAW].concat(),
            )
            .add_filter(t!("raw-file-filter"), RAW)
            .add_filter(t!("app-file-filter-all"), &["*"]);
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        dialog.pick_file()
    }

    /// Save (Ctrl+S): to the document's file if it is an Atelier or
    /// Photoshop document, otherwise Save As (an opened PNG or JPEG becomes
    /// an Atelier document, as Photoshop makes it a PSD). A Photoshop file
    /// is never silently replaced by one holding less: Save As then
    /// proposes an Atelier document, and says why.
    pub(super) fn save(&mut self) {
        // A smart object's contents (Edit Contents) go back to it.
        if self.save_embedded() || self.save_linked_file() {
            return;
        }
        let Some(doc) = &self.doc else { return };
        match doc.path.clone() {
            Some(path) if is_native(&path) => self.save_to(&path),
            Some(path) if has_extension(&path, "psd") => {
                let losses = doc.psd_losses();
                if losses.is_empty() {
                    self.save_to(&path);
                } else {
                    let notes = losses.join(" · ");
                    let why = t!("files3-status-psd-would-lose", notes = notes);
                    // Cancelled: the reason stays in view.
                    if !self.save_as_asked() {
                        self.status = why;
                    }
                }
            }
            _ => self.save_as_dialog(),
        }
    }

    /// Save As: an Atelier document, or a Photoshop document (the format
    /// of the document's file first, unless such a file would lose
    /// something).
    pub(super) fn save_as_dialog(&mut self) {
        self.save_as_asked();
    }

    /// [`Self::save_as_dialog`]; whether a file was chosen.
    fn save_as_asked(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        let lossy = doc.path.as_deref().is_some_and(|p| has_extension(p, "psd"))
            && !doc.psd_losses().is_empty();
        let title = if lossy {
            t!("files3-dialog-save-as-lossy")
        } else {
            t!("app-dialog-save-as")
        };
        let atelier = t!("file-filter-atelier");
        let atelier = (atelier.as_str(), &["atelier"][..]);
        let psd_filter = t!("xchg-file-filter-psd");
        let photoshop = (psd_filter.as_str(), &["psd"][..]);
        let psd = !lossy
            && self
                .doc
                .as_ref()
                .and_then(|d| d.path.as_deref())
                .is_some_and(|p| has_extension(p, "psd"));
        let filters = if psd {
            [photoshop, atelier]
        } else {
            [atelier, photoshop]
        };
        let stem = self.file_stem();
        match ask_save_path(&title, &filters, self.current_dir(), &stem) {
            Some(path) => {
                self.save_to(&path);
                true
            }
            None => false,
        }
    }

    fn save_to(&mut self, path: &Path) {
        self.commit_pending();
        // Photoshop refuses too: the other tab would show a file that is
        // no longer its own, and its Save would overwrite this one.
        if self.tab_of(path).is_some_and(|i| i != self.active_tab) {
            self.report_error(tm!(
                "file-status-open-in-another-tab",
                name = file_name(path)
            ));
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let started = std::time::Instant::now();
        let saved = doc.save(path);
        crate::diag::record(format!("Save: {}", format_of(path)), started.elapsed());
        let failed = saved.is_err();
        self.status = match saved {
            Ok(notes) => {
                // Saved as a file of its own: no longer a smart object's
                // contents.
                doc.embedded_in = None;
                self.recent.add(path);
                let shown = path.display().to_string();
                if notes.is_empty() {
                    t!("status-saved", path = shown)
                } else {
                    let what = notes.join(" · ");
                    crate::diag::warn(tm!(
                        "xchg-status-saved-simplified",
                        path = file_name(path),
                        notes = what
                    ));
                    t!(
                        "xchg-status-saved-simplified",
                        path = shown,
                        notes = notes.join(" · ")
                    )
                }
            }
            Err(e) => t!(
                "file-status-save-failed",
                name = file_name(path),
                error = e.to_string()
            ),
        };
        // Not saved: said where it can't be missed.
        if failed {
            self.report_error(self.status.clone());
        }
    }

    /// Name of the document without its extension, for the file dialogs.
    pub(super) fn file_stem(&self) -> String {
        self.doc
            .as_ref()
            .and_then(|d| d.path.as_deref())
            .and_then(Path::file_stem)
            .map_or_else(|| t!("doc-untitled"), |s| s.to_string_lossy().into_owned())
    }

    pub(super) fn current_dir(&self) -> Option<PathBuf> {
        self.doc
            .as_ref()?
            .path
            .as_ref()?
            .parent()
            .map(Path::to_path_buf)
    }

    /// The active document has changes not saved as an Atelier document,
    /// a transformation or crop being prepared included.
    pub(super) fn unsaved(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.modified)
            || (self.doc.is_some() && (self.canvas_rect.is_some() || self.transform_rect.is_some()))
    }

    /// Copies the visible pixels of the active layer (its targeted mask,
    /// in gray) to the clipboard. Returns whether they are on it.
    pub(super) fn copy_to_clipboard(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        let Some((image, rect)) = doc.copied(self.selection.as_ref()) else {
            self.status = t!("status-nothing-to-copy");
            return false;
        };
        let name = doc.active_layer().name.clone();
        self.put_on_clipboard(image, rect, t!("status-copied", name = name.as_str()))
    }

    /// Copy Merged: the visible layers together, inside the selection.
    pub(super) fn copy_merged(&mut self) -> bool {
        let Some(doc) = &self.doc else { return false };
        let Some((image, rect)) = doc.merged_pixels(self.selection.as_ref()) else {
            self.status = t!("status-nothing-to-copy");
            return false;
        };
        self.put_on_clipboard(image, rect, t!("files3-status-copied-merged"))
    }

    /// Puts `image`, from `rect` of the canvas, on the clipboard.
    fn put_on_clipboard(&mut self, image: image::RgbaImage, rect: PixelRect, done: String) -> bool {
        let copied = CopiedFrom::new(&image, (rect.x, rect.y));
        let data = arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Owned(image.into_raw()),
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_image(data)) {
            Ok(()) => {
                self.last_copy = Some(copied);
                self.status = done;
                true
            }
            Err(e) => {
                self.status_failed(tm!("status-copy-failed", error = e.to_string()));
                false
            }
        }
    }

    /// Pastes the clipboard image as a new layer (`mode`), or as a new
    /// document, unsaved, when none is open.
    pub(super) fn paste_from_clipboard(&mut self, mode: PasteMode) {
        // Files copied in the file manager: placed, like Photoshop.
        let files = arboard::Clipboard::new()
            .and_then(|mut c| c.get().file_list())
            .unwrap_or_default();
        if !files.is_empty() && self.doc.is_some() {
            for path in files {
                self.place_path(&path, false);
            }
            return;
        }
        let image = arboard::Clipboard::new()
            .and_then(|mut c| c.get_image())
            .ok()
            .and_then(|data| {
                image::RgbaImage::from_raw(
                    data.width as u32,
                    data.height as u32,
                    data.bytes.into_owned(),
                )
            });
        match image {
            Some(image) => self.paste_image(image, mode),
            None => self.status = t!("status-clipboard-no-image"),
        }
    }

    fn paste_image(&mut self, image: image::RgbaImage, mode: PasteMode) {
        let (w, h) = image.dimensions();
        if let Err(e) = crate::document::check_size(w, h) {
            self.status_failed(tm!("file-status-paste-failed", error = e.to_string()));
            return;
        }
        if mode == PasteMode::Into && self.selection.is_none() {
            self.status = t!("files3-status-paste-into-no-selection");
            return;
        }
        self.commit_pending();
        // Paste in Place: where the same pixels were copied from (as
        // Paste otherwise): centered on the selection, else on the part
        // of the canvas in view.
        let origin = (self.last_copy.as_ref())
            .filter(|c| mode == PasteMode::InPlace && c.matches(&image))
            .map(|c| c.origin)
            .or_else(|| {
                let (cx, cy) = self.paste_center()?;
                let x = (cx - w as f32 / 2.0).round() as i64;
                Some((x, (cy - h as f32 / 2.0).round() as i64))
            });
        let step = match mode {
            PasteMode::Centered => t!("menu-edit-paste"),
            PasteMode::InPlace => t!("files3-menu-paste-in-place"),
            PasteMode::Into => t!("files3-menu-paste-into"),
        };
        let name = t!("app-pasted-layer-name");
        match &mut self.doc {
            Some(doc) => match (mode, &self.selection, origin) {
                (PasteMode::Into, Some(sel), _) => doc.paste_into(step, name, image, sel),
                (_, _, Some(origin)) => doc.paste_at(step, name, image, origin),
                _ => doc.add_centered_layer(step, name, image),
            },
            None => {
                let mut doc = Document::new(image, None);
                doc.mark_modified();
                self.add_tab(doc);
            }
        }
        // Paste Into works inside the selection: it ends, as in Photoshop.
        if mode == PasteMode::Into {
            self.deselect();
        }
        self.status = t!("status-pasted", width = w, height = h);
        self.reset_pending();
    }

    /// Where Paste centers an image (canvas coordinates): on the
    /// selection, else on the part of the canvas in view. `None` before
    /// the canvas is shown (then the canvas center).
    fn paste_center(&self) -> Option<(f32, f32)> {
        let doc = self.doc.as_ref()?;
        let canvas = doc.full_rect();
        if let Some(r) = self.selection.as_ref().and_then(|s| s.bounds(canvas)) {
            return Some((r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0));
        }
        let view = self.last_view?;
        let area = (self.ctx).data(|d| d.get_temp::<Rect>(egui::Id::new(DROP_AREA)))?;
        let seen = Rect::from_min_max(view.to_image(area.min), view.to_image(area.max));
        let visible = seen.intersect(to_rect(canvas));
        (visible.is_positive()).then(|| (visible.center().x, visible.center().y))
    }

    /// Files dropped on the window: placed as smart objects (Place
    /// Embedded), or each opened in its own tab when no document is open
    /// (Atelier documents always open). They wait while a modal window is
    /// open.
    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        // On the canvas, an image is placed in the document; on the tabs
        // or the panels, it opens in a tab of its own.
        let canvas = ctx.data(|d| d.get_temp::<Rect>(egui::Id::new(DROP_AREA)));
        ctx.input(|i| {
            let at = i.pointer.latest_pos();
            let on_canvas = match (at, canvas) {
                (Some(p), Some(area)) => area.contains(p),
                _ => true,
            };
            let paths = i
                .raw
                .dropped_files
                .iter()
                .map(|f| (f.path().to_path_buf(), on_canvas));
            self.dropped.extend(paths);
        });
        if self.dropped.is_empty() || self.modal_open() {
            return;
        }
        let open = self.doc.is_none();
        for (path, on_canvas) in std::mem::take(&mut self.dropped) {
            if open || !on_canvas || is_native(&path) {
                self.open_path(&path);
            } else {
                self.place_path(&path, false);
            }
        }
    }
}

/// Where the canvas was last drawn (below the tabs): files dropped there
/// are placed.
pub(super) const DROP_AREA: &str = "canvas-drop-area";

/// Where a pasted image goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PasteMode {
    /// Centered on the canvas (Paste).
    Centered,
    /// Where it was copied from, when it is the last copy (Paste in Place).
    InPlace,
    /// Into the selection, through a layer mask (Paste Into).
    Into,
}

/// The last image copied, to know it again on the clipboard: its origin
/// on the canvas, its size, and a fingerprint of its transparency (what
/// survives every clipboard).
pub(super) struct CopiedFrom {
    origin: (i64, i64),
    size: (u32, u32),
    print: u64,
}

impl CopiedFrom {
    fn new(image: &image::RgbaImage, origin: (i64, i64)) -> Self {
        Self {
            origin,
            size: image.dimensions(),
            print: print(image),
        }
    }

    fn matches(&self, image: &image::RgbaImage) -> bool {
        self.size == image.dimensions() && self.print == print(image)
    }
}

/// Fingerprint of an image's alpha, and of the colors of its opaque
/// pixels (the others may come back premultiplied), from a few thousand
/// samples (fast on any size).
fn print(image: &image::RgbaImage) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let n = (image.width() as usize * image.height() as usize).max(1);
    for p in image.as_raw().chunks_exact(4).step_by(n.div_ceil(4096)) {
        match p[3] {
            255 => p.hash(&mut h),
            a => a.hash(&mut h),
        }
    }
    h.finish()
}

/// Asks where to save with the system dialog, proposing `stem` with the
/// first of `extensions`. A name typed without one of them gets it.
/// Where to save: the system dialog with `filters` (name, extensions),
/// the first one's first extension given to a name typed without one.
pub(super) fn ask_save_path(
    title: &str,
    filters: &[(&str, &[&str])],
    dir: Option<PathBuf>,
    stem: &str,
) -> Option<PathBuf> {
    #[cfg(test)]
    {
        let _ = (dir, stem);
        SAVE_DIALOG.with_borrow_mut(|d| {
            d.asked
                .push((title.to_string(), filters[0].1[0].to_string()));
            d.answer.take()
        })
    }
    #[cfg(not(test))]
    system_save_dialog(title, filters, dir, stem)
}

#[cfg(not(test))]
fn system_save_dialog(
    title: &str,
    filters: &[(&str, &[&str])],
    mut dir: Option<PathBuf>,
    stem: &str,
) -> Option<PathBuf> {
    let extensions: Vec<&str> = filters.iter().flat_map(|f| f.1.iter().copied()).collect();
    let mut name = format!("{stem}.{}", extensions[0]);
    loop {
        let mut dialog = rfd::FileDialog::new().set_title(title).set_file_name(&name);
        for (filter, exts) in filters {
            dialog = dialog.add_filter(*filter, exts);
        }
        if let Some(dir) = &dir {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.save_file()?;
        let Some(completed) = with_extension(&path, &extensions) else {
            return Some(path);
        };
        if !completed.exists() {
            return Some(completed);
        }
        // The system dialog (GTK) only checked the name as typed: ask again
        // with the full name, for it to confirm replacing that file.
        name = completed.file_name()?.to_string_lossy().into_owned();
        dir = completed.parent().map(Path::to_path_buf);
    }
}

/// The save dialog in tests: never shown; what it was asked (title, first
/// extension), and its answer.
#[cfg(test)]
#[derive(Default)]
pub(super) struct SaveDialog {
    pub(super) asked: Vec<(String, String)>,
    pub(super) answer: Option<PathBuf>,
}

#[cfg(test)]
thread_local! {
    pub(super) static SAVE_DIALOG: std::cell::RefCell<SaveDialog> = Default::default();
}

/// A file's format, as measured: its extension ("png"), lowercase.
pub(super) fn format_of(path: &Path) -> String {
    path.extension()
        .map_or_else(|| "?".to_owned(), |e| e.to_string_lossy().to_lowercase())
}

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `path` with the first of `extensions` added, if it has none of them.
pub(super) fn has_extension(path: &Path, ext: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn with_extension(path: &Path, extensions: &[&str]) -> Option<PathBuf> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if extensions.iter().any(|x| x.eq_ignore_ascii_case(ext)) {
        return None;
    }
    let mut completed = path.as_os_str().to_owned();
    completed.push(".");
    completed.push(extensions[0]);
    Some(completed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_names_get_the_format_extension() {
        let atelier = &["atelier"];
        assert_eq!(
            with_extension(Path::new("/a/photo"), atelier),
            Some("/a/photo.atelier".into())
        );
        assert_eq!(with_extension(Path::new("/a/photo.ATELIER"), atelier), None);
        assert_eq!(
            with_extension(Path::new("/a/photo.png"), atelier),
            Some("/a/photo.png.atelier".into())
        );
        let jpeg = crate::document::ExportFormat::Jpeg.extensions();
        assert_eq!(with_extension(Path::new("x.jpeg"), jpeg), None);
        assert_eq!(with_extension(Path::new("x.jfif"), jpeg), None);
        assert_eq!(with_extension(Path::new("x"), jpeg), Some("x.jpg".into()));
    }

    use egui_kittest::Harness;

    #[derive(Debug)]
    struct Dropped(PathBuf);

    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    fn harness(docs: Vec<Document>) -> Harness<'static, AtelierApp> {
        let mut harness = Harness::builder()
            .with_size([1400.0, 1000.0])
            .build_eframe(move |cc| {
                let mut app = AtelierApp::new(cc, None);
                app.recent = crate::recent::Recent::with_paths(Vec::new());
                for doc in docs {
                    app.add_tab(doc);
                }
                app
            });
        harness.run_steps(2);
        harness
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-files-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn image(color: [u8; 4]) -> image::RgbaImage {
        image::RgbaImage::from_pixel(16, 16, image::Rgba(color))
    }

    fn drop_file(harness: &mut Harness<'_, AtelierApp>, path: &Path) {
        let file = std::sync::Arc::new(Dropped(path.to_path_buf()));
        harness.input_mut().dropped_files.push(file);
    }

    fn close_window(harness: &mut Harness<'_, AtelierApp>) {
        let viewport = harness.input_mut().viewports.entry(egui::ViewportId::ROOT);
        viewport
            .or_default()
            .events
            .push(egui::ViewportEvent::Close);
    }

    #[test]
    fn a_pasted_document_asks_before_closing() {
        let mut harness = harness(Vec::new());
        harness
            .state_mut()
            .paste_image(image([1, 2, 3, 255]), PasteMode::Centered);
        harness.run_steps(2);
        assert!(harness.state().doc.as_ref().is_some_and(|d| d.modified));
        close_window(&mut harness);
        harness.run_steps(1);
        assert!(matches!(harness.state().confirm, Some(Confirm::Close)));
    }

    #[test]
    fn a_pending_transformation_asks_before_closing() {
        let mut harness = harness(vec![Document::new(image([9, 9, 9, 255]), None)]);
        let app = harness.state_mut();
        app.transform_rect = Some(Rect::from_min_size(pos2(2.0, 2.0), vec2(8.0, 8.0)));
        app.run(Action::CloseDocument);
        assert!(matches!(app.confirm, Some(Confirm::CloseDocument)));
        assert_eq!(app.first_modified_tab(), Some(0));
    }

    #[test]
    fn save_as_onto_a_document_open_in_another_tab_is_refused() {
        let dir = folder("save-onto-tab");
        let shared = dir.join("shared.atelier");
        let mut a = Document::new(image([0, 0, 255, 255]), None);
        a.save(&shared).unwrap();
        let before = std::fs::read(&shared).unwrap();
        let mut b = Document::new(image([255, 0, 0, 255]), None);
        b.modified = true;
        let mut harness = harness(vec![a, b]);
        let app = harness.state_mut();
        app.save_to(&shared);
        assert_eq!(std::fs::read(&shared).unwrap(), before);
        assert!(
            app.doc
                .as_ref()
                .is_some_and(|d| d.modified && d.path.is_none())
        );
        // The document's own file is saved as usual.
        app.switch_tab(0);
        app.save_to(&shared);
        assert!(app.doc.as_ref().is_some_and(|d| !d.modified));
    }

    #[test]
    fn ctrl_s_saves_an_atelier_document_in_place() {
        let dir = folder("save-in-place");
        let path = dir.join("doc.atelier");
        let mut doc = Document::new(image([0, 0, 255, 255]), None);
        doc.save(&path).unwrap();
        doc.add_empty_layer();
        assert!(doc.modified);
        let mut harness = harness(vec![doc]);
        harness.state_mut().save();
        assert!(harness.state().doc.as_ref().is_some_and(|d| !d.modified));
        assert_eq!(Document::open(&path).unwrap().doc.layers.len(), 2);
    }

    #[test]
    fn several_files_dropped_without_a_document_open_in_tabs() {
        let dir = folder("drop-several");
        let mut harness = harness(Vec::new());
        for (i, color) in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
            .iter()
            .enumerate()
        {
            let path = dir.join(format!("{i}.png"));
            image(*color).save(&path).unwrap();
            drop_file(&mut harness, &path);
        }
        harness.run_steps(2);
        let app = harness.state();
        assert_eq!(app.tab_count(), 3);
        assert!((0..3).all(|i| {
            app.tab(i)
                .is_some_and(|(d, _)| d.layers.len() == 1 && !d.modified)
        }));
    }

    #[test]
    fn files_dropped_during_a_modal_window_wait_for_it() {
        let dir = folder("drop-modal");
        let path = dir.join("layer.png");
        image([0, 0, 255, 255]).save(&path).unwrap();
        let mut harness = harness(vec![Document::new(image([9, 9, 9, 255]), None)]);
        harness
            .state_mut()
            .run(Action::Adjust(AdjustKind::BrightnessContrast));
        harness.run_steps(1);
        drop_file(&mut harness, &path);
        harness.run_steps(2);
        assert_eq!(harness.state().doc.as_ref().unwrap().layers.len(), 1);
        harness.state_mut().adjust = None;
        harness.run_steps(2);
        assert_eq!(harness.state().doc.as_ref().unwrap().layers.len(), 2);
    }

    #[test]
    fn ctrl_s_never_overwrites_a_psd_with_less() {
        let dir = folder("lossy-psd");
        let path = dir.join("gray16.psd");
        // 16-bit grayscale: opened as 8-bit RGB.
        let mut w = Vec::new();
        w.extend(b"8BPS");
        w.extend(1u16.to_be_bytes());
        w.extend([0; 6]);
        w.extend(1u16.to_be_bytes());
        w.extend(1u32.to_be_bytes());
        w.extend(2u32.to_be_bytes());
        w.extend(16u16.to_be_bytes());
        w.extend(1u16.to_be_bytes());
        w.extend([0; 12]);
        w.extend(0u16.to_be_bytes());
        w.extend([0xff, 0xff, 0x80, 0x00]);
        std::fs::write(&path, &w).unwrap();
        let mut harness = harness(Vec::new());
        let app = harness.state_mut();
        app.open_path(&path);
        app.doc.as_mut().unwrap().add_empty_layer();
        SAVE_DIALOG.with_borrow_mut(|d| d.asked.clear());
        app.save();
        // Save As instead, an Atelier document first, and why.
        let asked = SAVE_DIALOG.with_borrow_mut(|d| std::mem::take(&mut d.asked));
        assert_eq!(
            asked,
            [(t!("files3-dialog-save-as-lossy"), "atelier".to_string())]
        );
        assert_eq!(std::fs::read(&path).unwrap(), w);
        let losses = app.doc.as_ref().unwrap().psd_losses();
        assert!(losses.contains(&t!("xchg-psd-note-depth", bits = 16)));
        let why = t!("files3-status-psd-would-lose", notes = losses.join(" · "));
        assert_eq!(app.status, why);
        assert!(app.doc.as_ref().is_some_and(|d| d.modified));
        // Saved as an Atelier document: done.
        let atelier = dir.join("gray16.atelier");
        SAVE_DIALOG.with_borrow_mut(|d| d.answer = Some(atelier.clone()));
        app.save();
        let doc = app.doc.as_ref().unwrap();
        assert_eq!(doc.path.as_deref(), Some(atelier.as_path()));
        assert!(!doc.modified);
    }

    #[test]
    fn paste_in_place_and_paste_into() {
        let mut harness = harness(vec![Document::new(image([9, 9, 9, 255]), None)]);
        let app = harness.state_mut();
        let copied = image::RgbaImage::from_fn(4, 3, |x, _| image::Rgba([x as u8 * 60, 0, 0, 255]));
        app.last_copy = Some(CopiedFrom::new(&copied, (5, 6)));
        // The same pixels go back where they came from.
        app.paste_image(copied.clone(), PasteMode::InPlace);
        assert_eq!(app.doc.as_ref().unwrap().active_layer().offset, (5, 6));
        // Others are centered.
        let other = image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]));
        app.paste_image(other.clone(), PasteMode::InPlace);
        assert_eq!(app.doc.as_ref().unwrap().active_layer().offset, (6, 6));
        // Into the selection: masked by it, the selection ended.
        let rect = PixelRect {
            x: 2,
            y: 2,
            w: 6,
            h: 6,
        };
        app.paste_image(other.clone(), PasteMode::Into);
        assert!(
            app.status
                .contains(&t!("files3-status-paste-into-no-selection"))
        );
        app.selection = Some(rect.into());
        let layers = app.doc.as_ref().unwrap().layers.len();
        app.paste_image(other, PasteMode::Into);
        let doc = app.doc.as_ref().unwrap();
        assert_eq!(doc.layers.len(), layers + 1);
        let layer = doc.active_layer();
        assert_eq!(layer.offset, (3, 3));
        let mask = layer.mask.as_ref().expect("a layer mask");
        assert_eq!(mask.value_at(4, 4, layer.offset), 1.0);
        assert_eq!(mask.value_at(0, 0, layer.offset), 0.0);
        assert!(app.selection.is_none());
        // One step.
        app.doc.as_mut().unwrap().undo();
        assert_eq!(app.doc.as_ref().unwrap().layers.len(), layers);
    }

    /// Paste in Place knows its copy by its colors too: another opaque
    /// image of the same size is not it.
    #[test]
    fn paste_in_place_knows_its_copy_by_its_colors() {
        let copy =
            image::RgbaImage::from_fn(40, 30, |x, y| image::Rgba([x as u8, y as u8, 7, 255]));
        let from = CopiedFrom::new(&copy, (5, 6));
        assert!(from.matches(&copy));
        let shot = image::RgbaImage::from_pixel(40, 30, image::Rgba([1, 2, 3, 255]));
        assert!(!from.matches(&shot));
    }

    /// Paste centers the image on the selection, else on the part of the
    /// canvas in view (zoomed into a corner, it lands there).
    #[test]
    fn paste_lands_on_the_selection_or_in_view() {
        let canvas = image::RgbaImage::from_pixel(1000, 800, image::Rgba([9, 9, 9, 255]));
        let mut harness = harness(vec![Document::new(canvas, None)]);
        let pasted = image::RgbaImage::from_pixel(10, 10, image::Rgba([1, 2, 3, 255]));
        let app = harness.state_mut();
        app.selection = Some(
            PixelRect {
                x: 100,
                y: 100,
                w: 20,
                h: 20,
            }
            .into(),
        );
        app.paste_image(pasted.clone(), PasteMode::Centered);
        assert_eq!(app.doc.as_ref().unwrap().active_layer().offset, (105, 105));
        app.selection = None;
        // Zoomed into the top left corner.
        app.zoom = 8.0;
        app.pan = Vec2::splat(2000.0);
        harness.run_steps(2);
        let app = harness.state_mut();
        app.paste_image(pasted, PasteMode::Centered);
        let (x, y) = app.doc.as_ref().unwrap().active_layer().offset;
        let view = app.last_view.unwrap();
        let area = (app.ctx)
            .data(|d| d.get_temp::<Rect>(egui::Id::new(DROP_AREA)))
            .unwrap();
        let seen = Rect::from_min_max(view.to_image(area.min), view.to_image(area.max));
        let center = egui::pos2(x as f32 + 5.0, y as f32 + 5.0);
        assert!(
            seen.contains(center) && x < 300 && y < 300,
            "{x}, {y} in {seen:?}"
        );
    }
}
