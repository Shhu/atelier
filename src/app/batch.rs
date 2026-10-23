//! File › Automate › Batch and Fit Image, File › Scripts › Image
//! Processor, like Photoshop's. Batch plays an action on each image of a
//! folder (and its subfolders) or on the open documents, one document at a
//! time (the actions run as when played from the panel, their work in the
//! background included), then leaves them open, saves and closes them, or
//! saves them in another folder under names made of parts (document name,
//! serial number, date, extension…), never over an existing file unless
//! asked; errors stop it or are logged to a file. The Image Processor is a
//! Batch: JPEG, PSD and/or TIFF copies of each image, each resized to fit
//! a box if asked, in subfolders named after their format.

use std::collections::VecDeque;
use std::io::Write as _;

use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, gap_between_blocks, help, slider_row,
};
use super::*;
use crate::actions::{NamePart, Recorded, batch_name, today};
use crate::document::ExportFormat;
use crate::export::ExportOptions;

/// Formats Batch saves in.
const FORMATS: [ExportFormat; 6] = ExportFormat::ALL;

/// Parts of a name a slot of File Naming can hold (`Text`: typed).
const PARTS: [NamePart; 12] = [
    NamePart::DocumentName,
    NamePart::DocumentNameLower,
    NamePart::DocumentNameUpper,
    NamePart::Serial(1),
    NamePart::Serial(2),
    NamePart::Serial(3),
    NamePart::Serial(4),
    NamePart::SerialLetter,
    NamePart::SerialLetterUpper,
    NamePart::Date,
    NamePart::Extension,
    NamePart::ExtensionUpper,
];

fn part_label(p: &NamePart) -> String {
    match p {
        NamePart::DocumentName => t!("actions-name-document"),
        NamePart::DocumentNameLower => t!("actions-name-document-lower"),
        NamePart::DocumentNameUpper => t!("actions-name-document-upper"),
        NamePart::Serial(n) => t!("actions-name-serial", digits = *n as u32),
        NamePart::SerialLetter => t!("actions-name-letter-lower"),
        NamePart::SerialLetterUpper => t!("actions-name-letter-upper"),
        NamePart::Date => t!("actions-name-date"),
        NamePart::Extension => t!("actions-name-extension-lower"),
        NamePart::ExtensionUpper => t!("actions-name-extension-upper"),
        NamePart::Text(_) => t!("actions-name-text"),
    }
}

/// Fit Image: the box the image is scaled into.
#[derive(Clone, Copy)]
struct FitDialog {
    width: u32,
    height: u32,
    dont_enlarge: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Folder,
    Opened,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Destination {
    None,
    SaveAndClose,
    Folder,
}

/// Settings of the Batch window, kept for the session.
#[derive(Clone)]
struct BatchDialog {
    /// Set and action played.
    action: (usize, usize),
    source: Source,
    folder: Option<PathBuf>,
    subfolders: bool,
    destination: Destination,
    dest_folder: Option<PathBuf>,
    /// File Naming: four slots, `None` empty.
    naming: [Option<NamePart>; 4],
    serial: u32,
    format: ExportFormat,
    quality: u8,
    overwrite: bool,
    /// Log Errors to File (`Some`), else Stop for Errors.
    log: bool,
    log_file: Option<PathBuf>,
}

impl Default for BatchDialog {
    fn default() -> Self {
        Self {
            action: (0, 0),
            source: Source::Folder,
            folder: None,
            subfolders: false,
            destination: Destination::None,
            dest_folder: None,
            naming: [
                Some(NamePart::DocumentName),
                Some(NamePart::Text("_".into())),
                Some(NamePart::Serial(3)),
                Some(NamePart::Extension),
            ],
            serial: 1,
            format: ExportFormat::Jpeg,
            quality: 90,
            overwrite: false,
            log: false,
            log_file: None,
        }
    }
}

/// A format of the Image Processor: saved or not, resized to fit or not.
#[derive(Clone, Copy)]
struct ProcessorFormat {
    on: bool,
    fit: bool,
    width: u32,
    height: u32,
}

/// Settings of the Image Processor window, kept for the session.
#[derive(Clone)]
struct ProcessorDialog {
    opened: bool,
    folder: Option<PathBuf>,
    subfolders: bool,
    same_location: bool,
    dest: Option<PathBuf>,
    jpeg: ProcessorFormat,
    quality: u8,
    /// Convert Profile to sRGB (the JPEG files).
    srgb: bool,
    /// Use Proof Setup: JPEG and TIFF files in CMYK.
    proof: bool,
    psd: ProcessorFormat,
    tiff: ProcessorFormat,
    run_action: bool,
    action: (usize, usize),
    copyright: String,
    icc: bool,
}

impl Default for ProcessorDialog {
    fn default() -> Self {
        let format = |on| ProcessorFormat {
            on,
            fit: false,
            width: 1024,
            height: 1024,
        };
        Self {
            opened: true,
            folder: None,
            subfolders: false,
            same_location: true,
            dest: None,
            jpeg: format(true),
            quality: 80,
            srgb: true,
            proof: false,
            psd: format(false),
            tiff: format(false),
            run_action: false,
            action: (0, 0),
            copyright: String::new(),
            icc: true,
        }
    }
}

/// A document to process: a file, or an open document.
enum Item {
    File(PathBuf),
    Tab(u64),
}

/// A file each processed document is saved as.
#[derive(Clone)]
struct Output {
    options: ExportOptions,
    /// Scaled to fit this box first.
    fit: Option<(u32, u32)>,
    /// `None`: the document's folder.
    folder: Option<PathBuf>,
    subfolder: Option<&'static str>,
    /// File Naming; `None`: the document's name (made unique).
    naming: Option<Vec<NamePart>>,
    overwrite: bool,
}

/// The document being processed.
struct Current {
    uid: u64,
    name: String,
    source: Option<PathBuf>,
    /// Opened by the batch: closed after.
    opened: bool,
}

/// A batch running.
struct BatchRun {
    items: VecDeque<Item>,
    total: usize,
    done: usize,
    action: Option<(usize, usize)>,
    outputs: Vec<Output>,
    /// Save and Close.
    save_in_place: bool,
    quality: u8,
    /// Close the documents it opened.
    close: bool,
    /// Open documents are processed as copies, left untouched (Image
    /// Processor).
    copies: bool,
    /// Files written so far: two documents never go to the same file.
    written: std::collections::HashSet<PathBuf>,
    log: Option<PathBuf>,
    errors: usize,
    serial: u32,
    current: Option<Current>,
}

/// Batch, Image Processor and Fit Image windows, and the batch running.
#[derive(Default)]
pub(super) struct Automate {
    fit: Option<FitDialog>,
    batch: Option<BatchDialog>,
    batch_last: BatchDialog,
    processor: Option<ProcessorDialog>,
    processor_last: ProcessorDialog,
    run: Option<BatchRun>,
    /// Why the batch stopped.
    alert: Option<String>,
}

impl Automate {
    pub(super) fn modal(&self) -> bool {
        self.fit.is_some()
            || self.batch.is_some()
            || self.processor.is_some()
            || self.alert.is_some()
    }

    pub(super) fn running(&self) -> bool {
        self.run.is_some()
    }
}

/// The images Atelier opens in `folder` (and its subfolders), by name:
/// each file once, even when links lead to a folder twice (or in a loop).
fn images_in(folder: &Path, subfolders: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut visited = std::collections::HashSet::new();
    let mut dirs = vec![folder.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if !visited.insert(std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone())) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if subfolders {
                    dirs.push(path);
                }
            } else if !seen.insert(std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone())) {
                continue;
            } else if crate::camera_raw::raw::is_raw(&path)
                || path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    files::OPEN_EXTENSIONS
                        .iter()
                        .any(|x| x.eq_ignore_ascii_case(e))
                })
            {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Export format of a file name's extension.
fn format_of(path: &Path) -> Option<ExportFormat> {
    let ext = path.extension()?.to_str()?;
    ExportFormat::ALL
        .into_iter()
        .find(|f| f.extensions().iter().any(|x| x.eq_ignore_ascii_case(ext)))
}

/// A folder chosen with the system dialog.
fn pick_folder(title: &str, from: Option<&Path>) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title(title);
    if let Some(dir) = from {
        dialog = dialog.set_directory(dir);
    }
    dialog.pick_folder()
}

/// "Choose…" and the folder chosen.
fn folder_row(ui: &mut egui::Ui, id: &str, folder: &mut Option<PathBuf>, title: &str) {
    ui.horizontal(|ui| {
        if ui_kit::button(ui, &t!("actions-choose"), Kind::Normal, None, true).clicked()
            && let Some(f) = pick_folder(title, folder.as_deref())
        {
            *folder = Some(f);
        }
        let shown = folder
            .as_ref()
            .map_or_else(|| t!("actions-no-folder"), |f| f.display().to_string());
        ui.add(egui::Label::new(egui::RichText::new(shown).color(ui_kit::color::MUTED)).truncate())
            .on_hover_text(id.to_owned());
    });
}

/// Set and action combos.
fn action_picker(
    ui: &mut egui::Ui,
    id: &str,
    library: &crate::actions::Library,
    at: &mut (usize, usize),
) {
    let width = (ui.available_width() - 8.0) / 2.0;
    let sets = &library.sets;
    at.0 = at.0.min(sets.len().saturating_sub(1));
    let actions = sets.get(at.0).map_or(&[][..], |s| s.actions.as_slice());
    at.1 = at.1.min(actions.len().saturating_sub(1));
    ui.horizontal(|ui| {
        let set = sets.get(at.0).map_or("", |s| s.name.as_str());
        ui_kit::combo(
            ui,
            (id, "set"),
            Some(&t!("actions-set")),
            set,
            width,
            |ui| {
                for (i, s) in sets.iter().enumerate() {
                    if ui.selectable_label(at.0 == i, &s.name).clicked() {
                        *at = (i, 0);
                    }
                }
            },
        );
        let action = actions.get(at.1).map_or("", |a| a.name.as_str());
        ui_kit::combo(
            ui,
            (id, "action"),
            Some(&t!("actions-action")),
            action,
            width,
            |ui| {
                for (i, a) in actions.iter().enumerate() {
                    ui.selectable_value(&mut at.1, i, &a.name);
                }
            },
        );
    });
}

impl AtelierApp {
    // ------------------------------------------------------------ Fit Image

    pub(super) fn open_fit_image(&mut self, width: u32, height: u32, dont_enlarge: bool) {
        self.automate.fit = Some(FitDialog {
            width,
            height,
            dont_enlarge,
        });
    }

    /// Fit Image: the image scaled to fit in `width` × `height`, its
    /// proportions kept (not enlarged with `dont_enlarge`).
    pub(super) fn fit_image(&mut self, width: u32, height: u32, dont_enlarge: bool) {
        let Some(doc) = &self.doc else { return };
        let (w, h) = (doc.width(), doc.height());
        self.record(Recorded::FitImage {
            width,
            height,
            dont_enlarge,
        });
        let Some(size) = fitted((w, h), (width, height), dont_enlarge) else {
            return;
        };
        let d = ImageSizeDialog {
            width: size.0,
            height: size.1,
            keep_ratio: true,
            filter: self.resize_filter,
        };
        self.muted(|app| app.apply_image_size(d));
    }

    fn fit_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.automate.fit else {
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("fit_image", t!("actions-fit-image-title"), center)
            },
            |ui| {
                ui_kit::field_label(ui, &t!("actions-constrain-within"));
                let half = (ui.available_width() - 8.0) / 2.0;
                ui.horizontal(|ui| {
                    let size = 1..=MAX_DIMENSION;
                    let (w, h) = (t!("optbar-width-short"), t!("optbar-height-short"));
                    ui_kit::number_field(ui, &w, &mut d.width, size.clone(), " px", half);
                    ui_kit::number_field(ui, &h, &mut d.height, size, " px", half);
                });
                gap_between_blocks(ui);
                ui_kit::checkbox(ui, &mut d.dont_enlarge, &t!("actions-dont-enlarge"));
            },
        );
        self.automate.fit = (!out.ok && !out.cancel).then_some(d);
        if out.ok {
            self.fit_image(d.width, d.height, d.dont_enlarge);
        }
    }

    // ------------------------------------------------------------ Windows

    pub(super) fn open_batch(&mut self) {
        let mut d = self.automate.batch_last.clone();
        if let Some(sel) = self.actions_selected_action() {
            d.action = sel;
        }
        self.automate.batch = Some(d);
    }

    pub(super) fn open_image_processor(&mut self) {
        let mut d = self.automate.processor_last.clone();
        d.opened &= self.doc.is_some();
        self.automate.processor = Some(d);
    }

    pub(super) fn automate_windows(&mut self, ctx: &egui::Context) {
        self.fit_window(ctx);
        self.batch_window(ctx);
        self.processor_window(ctx);
        self.progress_window(ctx);
        self.alert_window(ctx);
    }

    fn batch_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.automate.batch.take() else {
            return;
        };
        let library = &self.actions.library;
        let has_action = library
            .sets
            .get(d.action.0)
            .is_some_and(|s| d.action.1 < s.actions.len());
        let ready = has_action
            && (d.source == Source::Opened || d.folder.is_some())
            && (d.destination != Destination::Folder || d.dest_folder.is_some())
            && (!d.log || d.log_file.is_some());
        let center = ctx.content_rect().center();
        let date = today();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 440.0,
                ok_enabled: ready,
                ..DialogSpec::new("batch", t!("actions-batch-title"), center)
            },
            |ui| {
                let width = ui.available_width();
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-batch-play"));
                    action_picker(ui, "batch_action", library, &mut d.action);
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-batch-source"));
                    let (folder, opened) =
                        (t!("actions-source-folder"), t!("actions-source-opened"));
                    dialogs::full_segmented(
                        ui,
                        &mut d.source,
                        &[(Source::Folder, &folder), (Source::Opened, &opened)],
                    );
                    if d.source == Source::Folder {
                        folder_row(
                            ui,
                            "batch_source",
                            &mut d.folder,
                            &t!("actions-batch-source"),
                        );
                        ui_kit::checkbox(ui, &mut d.subfolders, &t!("actions-subfolders"));
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-batch-destination"));
                    let label = |x: Destination| match x {
                        Destination::None => t!("actions-destination-none"),
                        Destination::SaveAndClose => t!("actions-destination-save-close"),
                        Destination::Folder => t!("actions-destination-folder"),
                    };
                    let shown = label(d.destination);
                    ui_kit::combo(ui, "batch_destination", None, &shown, width, |ui| {
                        for x in [
                            Destination::None,
                            Destination::SaveAndClose,
                            Destination::Folder,
                        ] {
                            ui.selectable_value(&mut d.destination, x, label(x));
                        }
                    });
                    if d.destination == Destination::Folder {
                        let title = t!("actions-batch-destination");
                        folder_row(ui, "batch_destination_folder", &mut d.dest_folder, &title);
                        gap_between_blocks(ui);
                        ui_kit::field_label(ui, &t!("actions-file-naming"));
                        naming_slots(ui, &mut d.naming);
                        let example: Vec<NamePart> = d.naming.iter().flatten().cloned().collect();
                        let ext = d.format.extensions()[0];
                        let shown =
                            batch_name(&example, &t!("actions-example-name"), d.serial, ext, date);
                        ui_kit::key_value(ui, &t!("actions-example"), &shown);
                        ui_kit::number_field(
                            ui,
                            &t!("actions-starting-serial"),
                            &mut d.serial,
                            1..=9999,
                            "",
                            width,
                        );
                    }
                    if d.destination != Destination::None {
                        gap_between_blocks(ui);
                        if d.destination == Destination::Folder {
                            ui_kit::combo(
                                ui,
                                "batch_format",
                                Some(&t!("file-export-format")),
                                d.format.name(),
                                width,
                                |ui| {
                                    for f in FORMATS {
                                        ui.selectable_value(&mut d.format, f, f.name());
                                    }
                                },
                            );
                        }
                        let mut quality = d.quality as f32;
                        let label = t!("file-export-quality");
                        slider_row(
                            ui,
                            &label,
                            None,
                            &mut quality,
                            1.0..=100.0,
                            "",
                            Track::Accent,
                            90.0,
                        );
                        d.quality = quality.round().clamp(1.0, 100.0) as u8;
                        if d.destination == Destination::Folder {
                            ui_kit::checkbox(ui, &mut d.overwrite, &t!("actions-overwrite"));
                        } else {
                            help(ui, &t!("actions-save-close-hint"));
                        }
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-batch-errors"));
                    let (stop, log) = (t!("actions-errors-stop"), t!("actions-errors-log"));
                    dialogs::full_segmented(ui, &mut d.log, &[(false, &stop), (true, &log)]);
                    if d.log {
                        ui.horizontal(|ui| {
                            if ui_kit::button(ui, &t!("actions-save-as"), Kind::Normal, None, true)
                                .clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .set_title(t!("actions-batch-errors"))
                                    .set_file_name("batch-errors.txt")
                                    .save_file()
                            {
                                d.log_file = Some(p);
                            }
                            let shown = d
                                .log_file
                                .as_ref()
                                .map_or_else(|| t!("actions-no-file"), |f| f.display().to_string());
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(shown).color(ui_kit::color::MUTED),
                                )
                                .truncate(),
                            );
                        });
                    }
                });
            },
        );
        self.automate.batch_last = d.clone();
        if out.ok && ready {
            self.start_batch(&d);
        } else if !out.cancel {
            self.automate.batch = Some(d);
        }
    }

    fn processor_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.automate.processor.take() else {
            return;
        };
        let has_doc = self.doc.is_some();
        let library = &self.actions.library;
        let has_action = library
            .sets
            .get(d.action.0)
            .is_some_and(|s| d.action.1 < s.actions.len());
        let ready = (d.opened || d.folder.is_some())
            && (d.same_location || d.dest.is_some())
            && (d.jpeg.on || d.psd.on || d.tiff.on)
            && (!d.run_action || has_action)
            && !(d.opened && d.same_location && !self.all_saved());
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 440.0,
                ok: t!("actions-run"),
                ok_enabled: ready,
                ..DialogSpec::new("image_processor", t!("actions-processor-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-processor-source"));
                    let (open, folder) = (t!("actions-source-opened"), t!("actions-source-folder"));
                    let mut opened = d.opened;
                    dialogs::full_segmented(ui, &mut opened, &[(true, &open), (false, &folder)]);
                    d.opened = opened && has_doc;
                    if !d.opened {
                        folder_row(
                            ui,
                            "processor_source",
                            &mut d.folder,
                            &t!("actions-processor-source"),
                        );
                        ui_kit::checkbox(ui, &mut d.subfolders, &t!("actions-subfolders"));
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-processor-location"));
                    let (same, other) = (t!("actions-same-location"), t!("actions-other-folder"));
                    dialogs::full_segmented(
                        ui,
                        &mut d.same_location,
                        &[(true, &same), (false, &other)],
                    );
                    if !d.same_location {
                        folder_row(
                            ui,
                            "processor_dest",
                            &mut d.dest,
                            &t!("actions-processor-location"),
                        );
                    }
                    help(ui, &t!("actions-processor-subfolders-hint"));
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-processor-file-type"));
                    format_options(ui, "JPEG", &mut d.jpeg);
                    if d.jpeg.on {
                        let mut quality = d.quality as f32;
                        let label = t!("file-export-quality");
                        slider_row(
                            ui,
                            &label,
                            None,
                            &mut quality,
                            1.0..=100.0,
                            "",
                            Track::Accent,
                            80.0,
                        );
                        d.quality = quality.round().clamp(1.0, 100.0) as u8;
                        ui_kit::checkbox(ui, &mut d.srgb, &t!("actions-convert-profile-srgb"));
                    }
                    format_options(ui, "PSD", &mut d.psd);
                    format_options(ui, "TIFF", &mut d.tiff);
                    if d.jpeg.on || d.tiff.on {
                        ui_kit::checkbox(ui, &mut d.proof, &t!("actions-use-proof"))
                            .on_hover_text(t!("actions-use-proof-tip"));
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("actions-processor-preferences"));
                    ui_kit::checkbox(ui, &mut d.run_action, &t!("actions-run-action"));
                    if d.run_action {
                        action_picker(ui, "processor_action", library, &mut d.action);
                    }
                    let id = egui::Id::new("processor_copyright");
                    ui_kit::text_field(ui, &t!("actions-copyright"), &mut d.copyright, id);
                    ui_kit::checkbox(ui, &mut d.icc, &t!("actions-include-icc"));
                });
                if d.opened && d.same_location && !self.all_saved() {
                    help(ui, &t!("actions-processor-unsaved"));
                }
            },
        );
        self.automate.processor_last = d.clone();
        if out.ok && ready {
            self.start_processor(&d);
        } else if !out.cancel {
            self.automate.processor = Some(d);
        }
    }

    /// Every open document has a file (for "Save in Same Location").
    fn all_saved(&self) -> bool {
        (0..self.tab_count()).all(|i| self.tab(i).is_some_and(|(d, _)| d.path.is_some()))
    }

    /// The open documents, from the first tab.
    fn open_items(&self) -> VecDeque<Item> {
        (0..self.tab_count())
            .filter_map(|i| self.tab(i).map(|(d, _)| Item::Tab(d.uid())))
            .collect()
    }

    fn start_batch(&mut self, d: &BatchDialog) {
        let items: VecDeque<Item> = match d.source {
            Source::Opened => self.open_items(),
            Source::Folder => d
                .folder
                .as_deref()
                .map(|f| images_in(f, d.subfolders))
                .unwrap_or_default()
                .into_iter()
                .map(Item::File)
                .collect(),
        };
        let outputs = match d.destination {
            Destination::Folder => vec![Output {
                options: ExportOptions {
                    format: d.format,
                    quality: d.quality,
                    srgb: false,
                    ..ExportOptions::default()
                },
                fit: None,
                folder: d.dest_folder.clone(),
                subfolder: None,
                naming: Some(d.naming.iter().flatten().cloned().collect()),
                overwrite: d.overwrite,
            }],
            _ => Vec::new(),
        };
        self.begin_run(BatchRun {
            total: items.len(),
            items,
            done: 0,
            action: Some(d.action),
            outputs,
            save_in_place: d.destination == Destination::SaveAndClose,
            quality: d.quality,
            close: d.destination != Destination::None,
            copies: false,
            written: Default::default(),
            log: d.log.then(|| d.log_file.clone()).flatten(),
            errors: 0,
            serial: d.serial,
            current: None,
        });
    }

    fn start_processor(&mut self, d: &ProcessorDialog) {
        let items: VecDeque<Item> = if d.opened {
            self.open_items()
        } else {
            d.folder
                .as_deref()
                .map(|f| images_in(f, d.subfolders))
                .unwrap_or_default()
                .into_iter()
                .map(Item::File)
                .collect()
        };
        let copyright = (!d.copyright.trim().is_empty()).then(|| d.copyright.trim().to_owned());
        let folder = (!d.same_location).then(|| d.dest.clone()).flatten();
        let output = |format: ExportFormat, f: &ProcessorFormat, quality: u8, name| Output {
            options: ExportOptions {
                format,
                quality,
                srgb: format == ExportFormat::Jpeg && d.srgb,
                use_proof: d.proof && format != ExportFormat::Psd,
                copyright: copyright.clone(),
                embed_profile: d.icc,
                ..ExportOptions::default()
            },
            fit: f.fit.then_some((f.width, f.height)),
            folder: folder.clone(),
            subfolder: Some(name),
            naming: None,
            overwrite: false,
        };
        let mut outputs = Vec::new();
        if d.jpeg.on {
            outputs.push(output(ExportFormat::Jpeg, &d.jpeg, d.quality, "JPEG"));
        }
        if d.psd.on {
            outputs.push(output(ExportFormat::Psd, &d.psd, 90, "PSD"));
        }
        if d.tiff.on {
            outputs.push(output(ExportFormat::Tiff, &d.tiff, 90, "TIFF"));
        }
        self.begin_run(BatchRun {
            total: items.len(),
            items,
            done: 0,
            action: d.run_action.then_some(d.action),
            outputs,
            save_in_place: false,
            quality: d.quality,
            close: true,
            copies: true,
            written: Default::default(),
            log: None,
            errors: 0,
            serial: 1,
            current: None,
        });
    }

    fn begin_run(&mut self, run: BatchRun) {
        self.commit_pending();
        self.actions.stop_recording();
        if run.items.is_empty() {
            self.automate.alert = Some(t!("actions-batch-nothing"));
            return;
        }
        self.automate.run = Some(run);
    }

    // ------------------------------------------------------------ Running

    /// One document at a time: opened, its action played (over frames),
    /// saved, closed (every frame).
    pub(super) fn run_batch(&mut self) {
        if self.automate.run.is_none() || self.actions.playing() || self.actions_wait() {
            return;
        }
        if let Some(cur) = self.automate.run.as_mut().and_then(|r| r.current.take()) {
            return self.finish_document(cur);
        }
        let Some(run) = &mut self.automate.run else {
            return;
        };
        let Some(item) = run.items.pop_front() else {
            return self.end_batch();
        };
        let (action, copies) = (run.action, run.copies);
        let (current, error) = match item {
            Item::Tab(uid) => match (0..self.tab_count())
                .find(|&i| self.tab(i).is_some_and(|(d, _)| d.uid() == uid))
            {
                Some(i) => {
                    self.switch_tab(i);
                    match self
                        .doc
                        .as_ref()
                        .filter(|_| copies)
                        .map(Document::duplicate)
                    {
                        Some(copy) => {
                            self.add_tab(copy);
                            (self.current_document(true), None)
                        }
                        None => (self.current_document(false), None),
                    }
                }
                // Closed meanwhile.
                None => (None, None),
            },
            // Already open: a copy of its own, the tab and its unsaved
            // changes left as they are.
            Item::File(path)
                if self.tab_of(&path).is_some() && !crate::camera_raw::raw::is_raw(&path) =>
            {
                match Document::open(&path) {
                    Ok(opened) => {
                        self.add_tab(opened.doc);
                        (self.current_document(true), None)
                    }
                    Err(e) => {
                        let shown = path.display().to_string();
                        let error = t!("status-open-failed", path = shown, error = e.to_string());
                        crate::diag::error(&error);
                        (None, Some(error))
                    }
                }
            }
            Item::File(path) => {
                let was_open = self.tab_of(&path).is_some();
                self.open_path(&path);
                if self.tab_of(&path).is_some() {
                    (self.current_document(!was_open), None)
                } else {
                    (None, Some(self.status.clone()))
                }
            }
        };
        if let Some(error) = error {
            if !self.batch_error(error) {
                return;
            }
            if let Some(run) = &mut self.automate.run {
                run.done += 1;
            }
            return;
        }
        let Some(current) = current else {
            if let Some(run) = &mut self.automate.run {
                run.done += 1;
            }
            return;
        };
        if let Some(run) = &mut self.automate.run {
            run.current = Some(current);
        }
        if let Some(at) = action {
            self.play_for_batch(at);
        }
    }

    fn current_document(&self, opened: bool) -> Option<Current> {
        let doc = self.doc.as_ref()?;
        Some(Current {
            uid: doc.uid(),
            name: doc.display_name(),
            source: doc.path.clone(),
            opened,
        })
    }

    /// After the action: the document's files saved, the document closed.
    fn finish_document(&mut self, cur: Current) {
        // The action failed: logged, and the document isn't saved.
        let failed = self.actions.take_batch_error();
        if let Some(error) = &failed
            && !self.batch_error(t!(
                "words3-label-value",
                label = cur.name.as_str(),
                value = error.as_str()
            ))
        {
            return;
        }
        let Some(run) = &self.automate.run else {
            return;
        };
        let (outputs, serial) = (run.outputs.clone(), run.serial);
        let (save_in_place, quality, close) = (run.save_in_place, run.quality, run.close);
        let on_document = self.doc.as_ref().is_some_and(|d| d.uid() == cur.uid);
        let mut saved = false;
        if on_document && failed.is_none() {
            for output in &outputs {
                if let Err(e) = self.save_output(output, &cur, serial)
                    && !self.batch_error(t!(
                        "words3-label-value",
                        label = cur.name.as_str(),
                        value = e.to_string()
                    ))
                {
                    return;
                }
            }
            if save_in_place {
                match self.save_in_place(quality) {
                    Ok(()) => saved = true,
                    Err(e) => {
                        if !self.batch_error(t!(
                            "words3-label-value",
                            label = cur.name.as_str(),
                            value = e.to_string()
                        )) {
                            return;
                        }
                    }
                }
            }
        }
        // A document the batch didn't open is closed only once saved: its
        // work is never lost.
        let unsaved = self
            .doc
            .as_ref()
            .is_some_and(|d| d.uid() == cur.uid && d.modified);
        if close && (cur.opened || (save_in_place && saved && !unsaved)) {
            self.close_by_uid(cur.uid);
        }
        if let Some(run) = &mut self.automate.run {
            run.done += 1;
            run.serial += 1;
        }
    }

    /// One of a document's files, named as asked, in its folder.
    fn save_output(&mut self, o: &Output, cur: &Current, serial: u32) -> Result<PathBuf, String> {
        let folder = match &o.folder {
            Some(f) => f.clone(),
            None => cur
                .source
                .as_deref()
                .and_then(Path::parent)
                .map(Path::to_path_buf)
                .ok_or_else(|| t!("actions-error-no-folder"))?,
        };
        let folder = match o.subfolder {
            Some(sub) => folder.join(sub),
            None => folder,
        };
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let stem = self.file_stem();
        let ext = o.options.format.extensions()[0];
        let path = match &o.naming {
            Some(parts) => folder.join(batch_name(parts, &stem, serial, ext, today())),
            None => export_as::unused_path(&folder, &stem, ext),
        };
        if self
            .automate
            .run
            .as_ref()
            .is_some_and(|r| r.written.contains(&path))
        {
            return Err(t!(
                "smart3-batch-written-twice",
                name = files::file_name(&path)
            ));
        }
        if path.exists() && !o.overwrite {
            return Err(t!("actions-error-exists", name = files::file_name(&path)));
        }
        let mut options = o.options.clone();
        let fit = o.fit.and_then(|(w, h)| {
            let doc = self.doc.as_ref()?;
            fitted((doc.width(), doc.height()), (w, h), true)
        });
        // Layers are kept (PSD): the document is resized, then back.
        let resized = match (fit, options.format) {
            (Some((w, _)), ExportFormat::Psd) => {
                let d = ImageSizeDialog {
                    width: w,
                    height: fit.map_or(1, |f| f.1),
                    keep_ratio: true,
                    filter: ResizeFilter::Bicubic,
                };
                self.muted(|app| app.apply_image_size(d));
                true
            }
            (Some((w, _)), _) => {
                let canvas = self.doc.as_ref().map_or(1, Document::width);
                options.scale = w as f32 * 100.0 / canvas as f32;
                false
            }
            (None, _) => false,
        };
        let result = self.write_export(&options, &path);
        if resized && let Some(doc) = &mut self.doc {
            doc.undo();
            self.reset_pending();
        }
        if result.is_ok()
            && let Some(run) = &mut self.automate.run
        {
            run.written.insert(path.clone());
        }
        result.map(|_| path)
    }

    /// Save and Close: the document over its own file, in its format.
    fn save_in_place(&mut self, quality: u8) -> Result<(), String> {
        let Some(doc) = &mut self.doc else {
            return Ok(());
        };
        let Some(path) = doc.path.clone() else {
            return Err(t!("actions-error-no-folder"));
        };
        // Never a Photoshop file replaced by one holding less.
        if crate::document::is_psd(&path) {
            let losses = doc.psd_losses();
            if !losses.is_empty() {
                return Err(t!(
                    "files3-status-psd-would-lose",
                    notes = losses.join(" · ")
                ));
            }
        }
        if crate::document::is_native(&path) || crate::document::is_psd(&path) {
            return doc.save(&path).map(|_| ()).map_err(|e| e.to_string());
        }
        let Some(format) = format_of(&path) else {
            return Err(t!("actions-error-format", name = files::file_name(&path)));
        };
        let options = ExportOptions {
            format,
            quality,
            ..ExportOptions::default()
        };
        self.write_export(&options, &path)?;
        if let Some(doc) = &mut self.doc {
            doc.mark_saved();
        }
        Ok(())
    }

    fn close_by_uid(&mut self, uid: u64) {
        if let Some(i) =
            (0..self.tab_count()).find(|&i| self.tab(i).is_some_and(|(d, _)| d.uid() == uid))
        {
            self.switch_tab(i);
            self.close_document();
        }
    }

    /// An error: logged (returns `true`: go on), or the batch stops with it.
    fn batch_error(&mut self, error: String) -> bool {
        let Some(run) = &mut self.automate.run else {
            return false;
        };
        run.errors += 1;
        if let Some(log) = &run.log {
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .and_then(|mut f| writeln!(f, "{error}"));
            if written.is_ok() {
                return true;
            }
        }
        self.automate.run = None;
        self.stop_playing();
        self.automate.alert = Some(t!("actions-batch-stopped", error = error.as_str()));
        false
    }

    fn end_batch(&mut self) {
        let Some(run) = self.automate.run.take() else {
            return;
        };
        self.status = match (&run.log, run.errors) {
            (Some(log), n) if n > 0 => t!(
                "actions-batch-done-errors",
                count = run.done as u32,
                errors = n as u32,
                path = log.display().to_string()
            ),
            _ => t!("actions-batch-done", count = run.done as u32),
        };
    }

    /// Batch's progress, and its Cancel: the document being processed is
    /// left as it is.
    fn progress_window(&mut self, ctx: &egui::Context) {
        let Some(run) = &self.automate.run else {
            return;
        };
        let name = run
            .current
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_default();
        let (done, total) = (run.done, run.total.max(1));
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok: t!("dialog-cancel"),
                cancel: false,
                ..DialogSpec::new("batch_progress", t!("actions-batch-title"), center)
            },
            |ui| {
                let shown = t!(
                    "actions-batch-progress",
                    n = (done + 1).min(total) as u32,
                    total = total as u32
                );
                ui.label(shown);
                ui.add(
                    egui::ProgressBar::new(done as f32 / total as f32)
                        .desired_width(ui.available_width()),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(name).color(ui_kit::color::MUTED))
                        .truncate(),
                );
            },
        );
        if out.ok || out.cancel {
            let done = run.done as u32;
            self.automate.run = None;
            self.stop_playing();
            self.status = t!("actions-batch-cancelled", count = done);
        }
    }

    fn alert_window(&mut self, ctx: &egui::Context) {
        let Some(text) = &self.automate.alert else {
            return;
        };
        let center = ctx.content_rect().center();
        let text = text.clone();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                cancel: false,
                ..DialogSpec::new("batch_alert", t!("actions-batch-title"), center)
            },
            |ui| {
                ui.add(egui::Label::new(text.as_str()).wrap());
            },
        );
        if out.ok || out.cancel {
            self.automate.alert = None;
        }
    }
}

/// The size `size` takes to fit in `bounds`, its proportions kept; `None`
/// if it stays as it is.
fn fitted(size: (u32, u32), bounds: (u32, u32), dont_enlarge: bool) -> Option<(u32, u32)> {
    let k = (f64::from(bounds.0) / f64::from(size.0)).min(f64::from(bounds.1) / f64::from(size.1));
    if (dont_enlarge && k >= 1.0) || k == 1.0 {
        return None;
    }
    let side = |v: u32| ((f64::from(v) * k).round() as u32).clamp(1, MAX_DIMENSION);
    Some((side(size.0), side(size.1)))
}

/// A format of the Image Processor: Save as, Resize to Fit W × H.
fn format_options(ui: &mut egui::Ui, name: &str, f: &mut ProcessorFormat) {
    ui_kit::checkbox(ui, &mut f.on, &t!("actions-save-as-format", format = name));
    if !f.on {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(24.0);
        ui_kit::checkbox(ui, &mut f.fit, &t!("actions-resize-to-fit"));
    });
    if f.fit {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            let half = (ui.available_width() - 8.0) / 2.0;
            let size = 1..=MAX_DIMENSION;
            let (w, h) = (t!("optbar-width-short"), t!("optbar-height-short"));
            ui_kit::number_field(ui, &w, &mut f.width, size.clone(), " px", half);
            ui_kit::number_field(ui, &h, &mut f.height, size, " px", half);
        });
    }
}

/// File Naming's slots: a part, typed text, or nothing.
fn naming_slots(ui: &mut egui::Ui, slots: &mut [Option<NamePart>; 4]) {
    let half = (ui.available_width() - 8.0) / 2.0;
    for (pair, row) in slots.chunks_mut(2).zip(0..) {
        ui.horizontal(|ui| {
            for (slot, col) in pair.iter_mut().zip(0..) {
                let id = ("batch_naming", row * 2 + col);
                match slot {
                    Some(NamePart::Text(text)) => {
                        let emptied = ui
                            .allocate_ui(vec2(half, 32.0), |ui| {
                                let id = egui::Id::new(id);
                                let edit =
                                    ui_kit::text_field(ui, &t!("actions-name-text"), text, id);
                                edit.response.lost_focus() && text.is_empty()
                            })
                            .inner;
                        // Emptied: the slot goes back to its list.
                        if emptied {
                            *slot = None;
                        }
                    }
                    _ => {
                        let shown = slot
                            .as_ref()
                            .map_or_else(|| t!("actions-name-none"), part_label);
                        ui_kit::combo(ui, id, None, &shown, half, |ui| {
                            ui.selectable_value(slot, None, t!("actions-name-none"));
                            for p in PARTS {
                                let label = part_label(&p);
                                ui.selectable_value(slot, Some(p), label);
                            }
                            ui.selectable_value(
                                slot,
                                Some(NamePart::Text("_".into())),
                                t!("actions-name-text"),
                            );
                        });
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Sequence, Set, Step};
    use crate::pixel_ops::Adjustment;
    use egui_kittest::Harness;
    use image::{Rgba, RgbaImage};

    /// No document; one action: Invert.
    fn app() -> Harness<'static, AtelierApp> {
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                app.recent = crate::recent::Recent::with_paths(Vec::new());
                app.actions.library.sets = vec![Set {
                    name: "S".into(),
                    actions: vec![Sequence {
                        name: "Invert".into(),
                        key: None,
                        steps: vec![Step::new(Recorded::Adjust {
                            adjustment: Adjustment::Invert,
                        })],
                        expanded: false,
                    }],
                    expanded: true,
                }];
                app
            });
        h.run_steps(2);
        h
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-batch-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(path: &Path, (w, h): (u32, u32), v: u8) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        RgbaImage::from_pixel(w, h, Rgba([v, v, v, 255]))
            .save(path)
            .unwrap();
    }

    fn run(h: &mut Harness<'static, AtelierApp>) {
        for _ in 0..500 {
            h.run_steps(1);
            if !h.state().automate.running() {
                return;
            }
        }
        panic!("batch not done");
    }

    fn batch(source: &Path, dest: &Path) -> BatchDialog {
        BatchDialog {
            folder: Some(source.to_path_buf()),
            subfolders: true,
            destination: Destination::Folder,
            dest_folder: Some(dest.to_path_buf()),
            naming: [
                Some(NamePart::DocumentName),
                Some(NamePart::Text("_".into())),
                Some(NamePart::Serial(2)),
                Some(NamePart::Extension),
            ],
            format: ExportFormat::Png,
            ..BatchDialog::default()
        }
    }

    fn value(path: &Path) -> u8 {
        image::open(path).unwrap().to_rgba8().get_pixel(0, 0).0[0]
    }

    #[test]
    fn camera_raw_files_are_among_the_images_of_a_folder() {
        let src = folder("raw-scan");
        for name in ["a.png", "b.CR2", "c.dng", "notes.txt"] {
            std::fs::write(src.join(name), b"").unwrap();
        }
        let names: Vec<String> = images_in(&src, false)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.png", "b.CR2", "c.dng"]);
    }

    #[test]
    fn batch_saves_each_image_under_its_new_name() {
        let (src, dest) = (folder("names-src"), folder("names-dest"));
        png(&src.join("a.png"), (8, 8), 10);
        png(&src.join("b.png"), (8, 8), 20);
        png(&src.join("sub/c.png"), (8, 8), 30);
        let mut h = app();
        let d = batch(&src, &dest);
        h.state_mut().start_batch(&d);
        run(&mut h);
        for (name, v) in [("a_01.png", 10), ("b_02.png", 20), ("c_03.png", 30)] {
            assert_eq!(value(&dest.join(name)), 255 - v, "{name}");
        }
        assert_eq!(value(&src.join("a.png")), 10, "the originals stay");
        assert_eq!(
            h.state().tab_count(),
            0,
            "the documents it opened are closed"
        );
        assert!(h.state().automate.alert.is_none());

        // Again: the files exist and aren't replaced; Stop for Errors.
        h.state_mut().start_batch(&d);
        run(&mut h);
        let alert = h.state().automate.alert.clone().expect("stopped");
        assert!(alert.contains("a_01.png"), "{alert}");
        // Logged: the batch goes on, one line per file.
        let log = dest.join("errors.txt");
        let logged = BatchDialog {
            log: true,
            log_file: Some(log.clone()),
            ..d.clone()
        };
        h.state_mut().automate.alert = None;
        h.state_mut().start_batch(&logged);
        run(&mut h);
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 3);
        assert!(h.state().automate.alert.is_none());
        // Allowed to replace them: done, without error.
        let replace = BatchDialog {
            overwrite: true,
            ..d
        };
        h.state_mut().start_batch(&replace);
        run(&mut h);
        assert!(h.state().automate.alert.is_none());
    }

    #[test]
    fn unreadable_files_stop_the_batch_or_are_logged() {
        let (src, dest) = (folder("errors-src"), folder("errors-dest"));
        std::fs::write(src.join("a-broken.png"), b"not an image").unwrap();
        png(&src.join("b.png"), (4, 4), 50);
        let mut h = app();
        let d = batch(&src, &dest);
        h.state_mut().start_batch(&d);
        run(&mut h);
        assert!(h.state().automate.alert.is_some());
        assert!(!dest.join("b_02.png").exists(), "stopped at the error");
        h.state_mut().automate.alert = None;
        let log = dest.join("log.txt");
        let logged = BatchDialog {
            log: true,
            log_file: Some(log.clone()),
            ..d
        };
        h.state_mut().start_batch(&logged);
        run(&mut h);
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("a-broken.png"), "{text}");
        // The serial number counts the documents processed.
        assert_eq!(value(&dest.join("b_01.png")), 205);
    }

    #[test]
    fn save_and_close_replaces_the_files_of_open_documents() {
        let src = folder("save-close");
        let path = src.join("x.png");
        png(&path, (4, 4), 40);
        let mut h = app();
        h.state_mut().open_path(&path);
        let d = BatchDialog {
            source: Source::Opened,
            destination: Destination::SaveAndClose,
            ..BatchDialog::default()
        };
        h.state_mut().start_batch(&d);
        run(&mut h);
        assert_eq!(value(&path), 215);
        assert_eq!(h.state().tab_count(), 0);
    }

    #[test]
    fn image_processor_saves_each_format_in_its_folder() {
        let (src, dest) = (folder("processor-src"), folder("processor-dest"));
        png(&src.join("photo.png"), (200, 100), 90);
        let mut h = app();
        let d = ProcessorDialog {
            opened: false,
            folder: Some(src.clone()),
            same_location: false,
            dest: Some(dest.clone()),
            jpeg: ProcessorFormat {
                on: true,
                fit: true,
                width: 50,
                height: 50,
            },
            tiff: ProcessorFormat {
                on: true,
                fit: false,
                width: 1,
                height: 1,
            },
            psd: ProcessorFormat {
                on: true,
                fit: true,
                width: 100,
                height: 20,
            },
            run_action: true,
            copyright: "Me".into(),
            ..ProcessorDialog::default()
        };
        h.state_mut().start_processor(&d);
        run(&mut h);
        let size = |p: PathBuf| image::image_dimensions(p).unwrap();
        assert_eq!(size(dest.join("JPEG/photo.jpg")), (50, 25));
        assert_eq!(size(dest.join("TIFF/photo.tif")), (200, 100));
        assert!(value(&dest.join("TIFF/photo.tif")) > 160, "the action ran");
        let psd = Document::open(&dest.join("PSD/photo.psd")).unwrap().doc;
        assert_eq!((psd.width(), psd.height()), (40, 20));
        let jpeg = std::fs::read(dest.join("JPEG/photo.jpg")).unwrap();
        assert!(jpeg.windows(2).any(|w| w == b"Me"), "the copyright");
        // Run again: new names, nothing replaced.
        h.state_mut().start_processor(&d);
        run(&mut h);
        assert!(dest.join("JPEG/photo-2.jpg").exists());
    }

    /// Save and Close with Log Errors: a document that can't be saved is
    /// logged and stays open, its work kept.
    #[test]
    fn save_and_close_keeps_documents_it_could_not_save() {
        let dir = folder("save-close-log");
        let mut h = app();
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([10, 10, 10, 255])), None);
        doc.modified = true;
        h.state_mut().add_tab(doc);
        let d = BatchDialog {
            source: Source::Opened,
            destination: Destination::SaveAndClose,
            log: true,
            log_file: Some(dir.join("log.txt")),
            ..BatchDialog::default()
        };
        h.state_mut().start_batch(&d);
        run(&mut h);
        let log = std::fs::read_to_string(dir.join("log.txt")).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert_eq!(h.state().tab_count(), 1, "the unsaved document stays open");
    }

    #[test]
    fn links_to_a_folder_twice_list_each_image_once() {
        let src = folder("links");
        png(&src.join("a.png"), (2, 2), 0);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&src, src.join("loop")).unwrap();
            std::fs::create_dir_all(src.join("sub")).unwrap();
            std::os::unix::fs::symlink(src.join("a.png"), src.join("sub/b.png")).unwrap();
        }
        assert_eq!(images_in(&src, true), [src.join("a.png")]);
    }

    /// A file of the folder already open with unsaved changes: the batch
    /// works on a copy of its own, the tab is left as it was.
    #[test]
    fn batch_leaves_an_open_copy_of_a_file_alone() {
        let src = folder("open-copy");
        let path = src.join("x.png");
        png(&path, (4, 4), 40);
        let mut h = app();
        h.state_mut().open_path(&path);
        h.state_mut().doc.as_mut().unwrap().modified = true;
        let uid = h.state().doc.as_ref().unwrap().uid();
        let d = BatchDialog {
            folder: Some(src.clone()),
            destination: Destination::SaveAndClose,
            ..BatchDialog::default()
        };
        h.state_mut().start_batch(&d);
        run(&mut h);
        assert_eq!(value(&path), 215, "the file was processed");
        assert_eq!(h.state().tab_count(), 1);
        let doc = h.state().doc.as_ref().unwrap();
        assert!(doc.uid() == uid && doc.modified);
        let pixel = doc.flatten().get_pixel(0, 0).0;
        assert_eq!(pixel[0], 40, "the tab is untouched");
    }

    /// The Image Processor on open documents works on copies: the
    /// documents keep their pixels, size and history.
    #[test]
    fn image_processor_leaves_open_documents_untouched() {
        let (src, dest) = (folder("processor-open-src"), folder("processor-open-dest"));
        let path = src.join("photo.png");
        png(&path, (200, 100), 90);
        let mut h = app();
        h.state_mut().open_path(&path);
        let d = ProcessorDialog {
            opened: true,
            same_location: false,
            dest: Some(dest.clone()),
            jpeg: ProcessorFormat {
                on: false,
                fit: false,
                width: 1,
                height: 1,
            },
            psd: ProcessorFormat {
                on: true,
                fit: true,
                width: 100,
                height: 100,
            },
            run_action: true,
            ..ProcessorDialog::default()
        };
        h.state_mut().start_processor(&d);
        run(&mut h);
        assert!(dest.join("PSD/photo.psd").exists());
        assert_eq!(h.state().tab_count(), 1);
        let doc = h.state().doc.as_ref().unwrap();
        assert_eq!((doc.width(), doc.height()), (200, 100));
        assert_eq!(doc.flatten().get_pixel(0, 0).0[0], 90);
        assert!(!doc.modified && !doc.can_undo());
    }

    /// Two images named alike, a naming without serial and Override: the
    /// second is logged instead of replacing the first.
    #[test]
    fn two_documents_never_go_to_the_same_file() {
        let (src, dest) = (folder("same-name-src"), folder("same-name-dest"));
        png(&src.join("a/x.png"), (4, 4), 10);
        png(&src.join("b/x.png"), (4, 4), 20);
        let log = dest.join("log.txt");
        let d = BatchDialog {
            naming: [
                Some(NamePart::DocumentName),
                Some(NamePart::Extension),
                None,
                None,
            ],
            overwrite: true,
            log: true,
            log_file: Some(log.clone()),
            ..batch(&src, &dest)
        };
        let mut h = app();
        h.state_mut().start_batch(&d);
        run(&mut h);
        assert_eq!(value(&dest.join("x.png")), 245, "the first one stays");
        assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
    }

    #[test]
    fn fit_image_keeps_the_proportions() {
        assert_eq!(fitted((200, 100), (50, 50), false), Some((50, 25)));
        assert_eq!(fitted((200, 100), (1000, 1000), true), None);
        assert_eq!(fitted((200, 100), (1000, 1000), false), Some((1000, 500)));
        assert_eq!(fitted((200, 100), (300, 100), false), None);
        let mut h = app();
        h.state_mut().doc = Some(Document::new(RgbaImage::new(200, 100), None));
        h.state_mut().fit_image(60, 60, true);
        let doc = h.state().doc.as_ref().unwrap();
        assert_eq!((doc.width(), doc.height()), (60, 30));
    }
}
