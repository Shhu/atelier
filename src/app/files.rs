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
        match Document::open(path) {
            Ok(opened) => {
                self.status = if opened.animated {
                    t!("file-status-opened-first-frame", path = shown)
                } else if !opened.notes.is_empty() {
                    let notes = opened.notes.join(" · ");
                    t!("xchg-status-opened-simplified", path = shown, notes = notes)
                } else {
                    t!("status-opened", path = shown)
                };
                self.recent.add(path);
                self.add_tab(opened.doc);
            }
            Err(e) => {
                self.status = t!("status-open-failed", path = shown, error = e.to_string());
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
    /// an Atelier document, as Photoshop makes it a PSD).
    pub(super) fn save(&mut self) {
        // A smart object's contents (Edit Contents) go back to it.
        if self.save_embedded() {
            return;
        }
        match self.doc.as_ref().and_then(|d| d.path.clone()) {
            Some(path) if is_native(&path) || has_extension(&path, "psd") => self.save_to(&path),
            _ => self.save_as_dialog(),
        }
    }

    /// Save As: an Atelier document, or a Photoshop document (the format
    /// of the document's file first).
    pub(super) fn save_as_dialog(&mut self) {
        if self.doc.is_none() {
            return;
        }
        let title = t!("app-dialog-save-as");
        let atelier = t!("file-filter-atelier");
        let atelier = (atelier.as_str(), &["atelier"][..]);
        let psd_filter = t!("xchg-file-filter-psd");
        let photoshop = (psd_filter.as_str(), &["psd"][..]);
        let psd = self
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
        if let Some(path) = ask_save_path(&title, &filters, self.current_dir(), &stem) {
            self.save_to(&path);
        }
    }

    fn save_to(&mut self, path: &Path) {
        self.commit_pending();
        // Photoshop refuses too: the other tab would show a file that is
        // no longer its own, and its Save would overwrite this one.
        if self.tab_of(path).is_some_and(|i| i != self.active_tab) {
            self.status = t!("file-status-open-in-another-tab", name = file_name(path));
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        self.status = match doc.save(path) {
            Ok(notes) => {
                // Saved as a file of its own: no longer a smart object's
                // contents.
                doc.embedded_in = None;
                self.recent.add(path);
                let shown = path.display().to_string();
                if notes.is_empty() {
                    t!("status-saved", path = shown)
                } else {
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
        let Some(image) = doc.copied_pixels(self.selection.as_ref()) else {
            self.status = t!("status-nothing-to-copy");
            return false;
        };
        let data = arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: std::borrow::Cow::Owned(image.into_raw()),
        };
        match arboard::Clipboard::new().and_then(|mut c| c.set_image(data)) {
            Ok(()) => {
                self.status = t!("status-copied", name = doc.active_layer().name.as_str());
                true
            }
            Err(e) => {
                self.status = t!("status-copy-failed", error = e.to_string());
                false
            }
        }
    }

    /// Pastes the clipboard image as a new layer, or as a new document,
    /// unsaved, when none is open.
    pub(super) fn paste_from_clipboard(&mut self) {
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
            Some(image) => self.paste_image(image),
            None => self.status = t!("status-clipboard-no-image"),
        }
    }

    fn paste_image(&mut self, image: image::RgbaImage) {
        let (w, h) = image.dimensions();
        if let Err(e) = crate::document::check_size(w, h) {
            self.status = t!("file-status-paste-failed", error = e.to_string());
            return;
        }
        self.commit_pending();
        match &mut self.doc {
            Some(doc) => {
                doc.add_centered_layer(t!("menu-edit-paste"), t!("app-pasted-layer-name"), image)
            }
            None => {
                let mut doc = Document::new(image, None);
                doc.modified = true;
                self.add_tab(doc);
            }
        }
        self.status = t!("status-pasted", width = w, height = h);
        self.reset_pending();
    }

    /// Files dropped on the window: placed as smart objects (Place
    /// Embedded), or each opened in its own tab when no document is open
    /// (Atelier documents always open). They wait while a modal window is
    /// open.
    pub(super) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        ctx.input(|i| {
            let paths = i.raw.dropped_files.iter().map(|f| f.path().to_path_buf());
            self.dropped.extend(paths);
        });
        if self.dropped.is_empty() || self.modal_open() {
            return;
        }
        let open = self.doc.is_none();
        for path in std::mem::take(&mut self.dropped) {
            if open || is_native(&path) {
                self.open_path(&path);
            } else {
                self.place_path(&path, false);
            }
        }
    }
}

/// Asks where to save with the system dialog, proposing `stem` with the
/// first of `extensions`. A name typed without one of them gets it.
/// Where to save: the system dialog with `filters` (name, extensions),
/// the first one's first extension given to a name typed without one.
pub(super) fn ask_save_path(
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

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// `path` with the first of `extensions` added, if it has none of them.
fn has_extension(path: &Path, ext: &str) -> bool {
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
        harness.state_mut().paste_image(image([1, 2, 3, 255]));
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
}
