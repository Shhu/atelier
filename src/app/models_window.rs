//! AI models: the AI Models window (Edit › AI Models…), the question asked
//! before a feature's first download, and what runs once a download ends.

use super::dialogs::{DialogSpec, floating_dialog, gap_between_blocks, help};
use super::*;
use crate::ai::models::{self, DownloadError, Model};
use crate::ai::runtime::Device;
use crate::i18n::t;
use ui_kit::color;

/// What waits for a model: run once it is downloaded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum AfterDownload {
    SelectSubject,
    RemoveBackground,
    /// Object Selection tool: analysis of the image.
    ObjectSelection,
    /// Content-Aware Fill painted in by the AI model.
    ContentAwareFill,
}

impl AfterDownload {
    pub(super) fn model(self) -> Model {
        match self {
            AfterDownload::SelectSubject | AfterDownload::RemoveBackground => Model::Subject,
            AfterDownload::ObjectSelection => Model::Objects,
            AfterDownload::ContentAwareFill => Model::Inpaint,
        }
    }

    /// Name of the feature, for the question.
    fn feature(self) -> String {
        match self {
            AfterDownload::SelectSubject => t!("canvas-menu-select-subject"),
            AfterDownload::RemoveBackground => t!("menu-layer-remove-background"),
            AfterDownload::ObjectSelection => t!("ai-model-objects-purpose"),
            AfterDownload::ContentAwareFill => t!("retouch-content-aware-fill"),
        }
    }
}

/// Question asked before the first download of a model.
#[derive(Clone, Copy)]
pub(super) struct ModelPrompt {
    pub then: AfterDownload,
}

/// AI Models window, open.
#[derive(Default)]
pub(super) struct ModelsWindow {
    /// Model whose deletion awaits confirmation.
    confirm_delete: Option<Model>,
}

/// What a model's card asked for.
enum CardAction {
    Download,
    Cancel,
    Delete,
}

const GREEN: Color32 = Color32::from_rgb(0x3d, 0xb8, 0x6b);
const YELLOW: Color32 = Color32::from_rgb(0xe8, 0xc6, 0x3a);

impl AtelierApp {
    /// Is `then`'s model on the computer? If not, asks to download it (or,
    /// if it is already downloading, waits for it); `then` runs once it is
    /// there.
    pub(super) fn request_model(&mut self, then: AfterDownload) -> bool {
        let model = then.model();
        if model.ready() {
            return true;
        }
        if self.downloads.is_downloading(model) {
            self.wait_for_model(then);
        } else {
            self.model_prompt = Some(ModelPrompt { then });
        }
        false
    }

    /// `then` runs when its model's download ends well (the last request
    /// for a model replaces the previous one).
    fn wait_for_model(&mut self, then: AfterDownload) {
        self.after_download.retain(|t| t.model() != then.model());
        self.after_download.push(then);
    }

    /// The active document goes away (closed, or set aside for another
    /// tab): what was going to act on it is forgotten, so that it never
    /// lands on another document.
    pub(super) fn forget_document_jobs(&mut self) {
        self.model_prompt = None;
        self.subject_job = None;
        self.after_download.clear();
        self.object_select.leave_document();
    }

    /// Ended downloads (once per frame): messages, and what was waiting for
    /// them. While a download runs, the status bar shows its progress.
    pub(super) fn poll_downloads(&mut self, ctx: &egui::Context) {
        for (model, result) in self.downloads.finished() {
            let waiting: Vec<AfterDownload> = self
                .after_download
                .iter()
                .copied()
                .filter(|t| t.model() == model)
                .collect();
            self.after_download.retain(|t| t.model() != model);
            match result {
                Ok(()) => {
                    self.status = t!("models-status-downloaded", name = model.name());
                    if model == Model::Objects {
                        // New files: an analysis that failed is tried again.
                        self.object_select.forget();
                    }
                    for then in waiting {
                        self.model_downloaded(then);
                    }
                }
                Err(e @ DownloadError::Cancelled) => self.status = e.to_string(),
                Err(e) => {
                    let error = e.to_string();
                    self.status = t!("models-status-failed", name = model.name(), error = error);
                }
            }
        }
        if !self.downloads.any() {
            return;
        }
        let downloading = Model::ALL
            .into_iter()
            .find_map(|m| self.downloads.progress(m).map(|f| (m, f)));
        if let Some((model, fraction)) = downloading {
            let percent = format!("{:.0}", fraction * 100.0);
            self.status = t!(
                "models-status-downloading",
                name = model.name(),
                percent = percent
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Continues what waited for a model, now downloaded.
    fn model_downloaded(&mut self, then: AfterDownload) {
        match then {
            AfterDownload::SelectSubject => self.find_subject(false),
            AfterDownload::RemoveBackground => self.find_subject(true),
            // The Object Selection tool starts its analysis here.
            AfterDownload::ObjectSelection => {}
            AfterDownload::ContentAwareFill => self.content_aware_downloaded(),
        }
    }

    /// Before a feature's first use: download its model (only once), or,
    /// for Select Subject and Remove Background, settle for the non-AI
    /// method.
    pub(super) fn model_prompt_window(&mut self, ctx: &egui::Context) {
        let Some(ModelPrompt { then }) = self.model_prompt else {
            return;
        };
        let model = then.model();
        let without_ai = matches!(model, Model::Subject | Model::Inpaint);
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 400.0,
                ok: t!("dialog-model-download"),
                other: without_ai.then(|| t!("dialog-model-no-ai")),
                ..DialogSpec::new("model_prompt", t!("models-prompt-title"), center)
            },
            |ui| {
                ui.label(t!(
                    "models-prompt-text",
                    feature = then.feature(),
                    name = model.name(),
                    size = size_text(model.size())
                ));
                gap_between_blocks(ui);
                if model == Model::Inpaint {
                    help(ui, &t!("retouch-model-hint"));
                } else if without_ai {
                    help(ui, &t!("dialog-model-hint"));
                } else {
                    help(ui, &t!("models-prompt-hint"));
                }
            },
        );
        if out.cancel {
            self.model_prompt = None;
        } else if out.ok {
            self.model_prompt = None;
            self.downloads.start(model);
            self.wait_for_model(then);
        } else if out.other {
            self.model_prompt = None;
            if then == AfterDownload::ContentAwareFill {
                self.content_aware_without_ai();
            } else {
                self.start_subject(then == AfterDownload::RemoveBackground, false);
            }
        }
    }

    /// The deletion confirmation is open: it keeps the keyboard.
    pub(super) fn models_modal(&self) -> bool {
        self.models_window
            .as_ref()
            .is_some_and(|w| w.confirm_delete.is_some())
    }

    /// AI Models window: each model's state, size, where it runs; download,
    /// cancel, delete.
    pub(super) fn models_window(&mut self, ctx: &egui::Context) {
        let Some(window) = &self.models_window else {
            return;
        };
        let confirm = window.confirm_delete;
        let total: u64 = Model::ALL.iter().map(|m| m.size_on_disk()).sum();
        let center = ctx.content_rect().center();
        let mut actions = Vec::new();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 480.0,
                modal: false,
                ok: t!("dialog-close-tip"),
                other: Some(t!("models-open-folder")),
                cancel: false,
                note: Some(t!("models-total", size = size_text(total))),
                ..DialogSpec::new("ai_models", t!("models-title"), center)
            },
            |ui| {
                help(ui, &t!("models-hint"));
                gap_between_blocks(ui);
                for model in Model::ALL {
                    if let Some(action) = self.model_card(ui, model) {
                        actions.push((model, action));
                    }
                }
            },
        );
        for (model, action) in actions {
            match action {
                CardAction::Download => self.downloads.start(model),
                CardAction::Cancel => {
                    self.downloads.cancel(model);
                    self.after_download.retain(|t| t.model() != model);
                }
                CardAction::Delete => {
                    if let Some(w) = &mut self.models_window {
                        w.confirm_delete = Some(model);
                    }
                }
            }
        }
        if out.other {
            let root = models::root();
            if let Err(e) = open_folder(&root) {
                self.status = t!("models-open-folder-failed", error = e.to_string());
            }
        }
        if out.ok || out.cancel {
            self.models_window = None;
            return;
        }
        if let Some(model) = confirm {
            self.confirm_delete_window(ctx, model);
        }
    }

    /// Is `model` being used (its files must stay)?
    fn model_in_use(&self, model: Model) -> bool {
        self.downloads.is_downloading(model)
            || match model {
                Model::Subject => self.subject_job.is_some(),
                Model::Objects => self.object_select.busy(),
                Model::Inpaint => self.retouch.ai_busy(),
            }
    }

    /// One model: name, state, purpose, sizes, where it runs, license; its
    /// download progress, and the button for what can be done.
    fn model_card(&self, ui: &mut egui::Ui, model: Model) -> Option<CardAction> {
        let progress = self.downloads.progress(model);
        let ready = model.ready();
        let on_disk = model.size_on_disk();
        let in_use = self.model_in_use(model);
        let mut action = None;
        egui::Frame::new()
            .fill(color::BG1)
            .stroke(UiStroke::new(1.0, color::LINE))
            .corner_radius(8)
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = vec2(8.0, 4.0);

                // Name, state; buttons on the right.
                let (row, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                let mut left = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(row)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                left.label(
                    egui::RichText::new(model.name())
                        .font(ui_kit::bold(ui_kit::TEXT_SIZE))
                        .color(color::TEXT),
                );
                let (state, tint) = if progress.is_some() {
                    (t!("models-state-downloading"), color::ACCENT_TEXT)
                } else if ready {
                    (t!("models-state-downloaded"), GREEN)
                } else if on_disk > 0 {
                    (t!("models-state-incomplete"), YELLOW)
                } else {
                    (t!("models-state-not-downloaded"), color::LABEL)
                };
                state_chip(&mut left, &state, tint);
                let mut right = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(row)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                if progress.is_some() {
                    let label = t!("dialog-cancel");
                    let b = ui_kit::button_with_icon(
                        &mut right,
                        Some(icon::X),
                        &label,
                        Kind::Normal,
                        None,
                        true,
                    );
                    if b.clicked() {
                        action = Some(CardAction::Cancel);
                    }
                } else {
                    if !ready {
                        let label = t!("dialog-model-download");
                        let glyph = Some(icon::DOWNLOAD_SIMPLE);
                        let b = ui_kit::button_with_icon(
                            &mut right,
                            glyph,
                            &label,
                            Kind::Primary,
                            None,
                            true,
                        );
                        if b.clicked() {
                            action = Some(CardAction::Download);
                        }
                    }
                    if on_disk > 0 {
                        let label = t!("models-delete");
                        let glyph = Some(icon::TRASH);
                        let b = ui_kit::button_with_icon(
                            &mut right,
                            glyph,
                            &label,
                            Kind::Normal,
                            None,
                            !in_use,
                        );
                        if in_use {
                            b.on_hover_text(t!("models-in-use"));
                        } else if b.clicked() {
                            action = Some(CardAction::Delete);
                        }
                    }
                }

                ui_kit::hint(ui, model.purpose());
                ui.add_space(6.0);

                // Details, in columns.
                let on_disk_text = if on_disk > 0 {
                    size_text(on_disk)
                } else {
                    "—".to_owned()
                };
                let details = [
                    (t!("models-download-size"), size_text(model.size()), 0.22),
                    (t!("models-on-disk"), on_disk_text, 0.22),
                    (t!("models-runs-on"), device_text(model), 0.34),
                    (t!("models-license"), model.license().to_owned(), 0.22),
                ];
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let width = ui.available_width();
                    for (label, value, share) in details {
                        detail(ui, &label, &value, width * share);
                    }
                });

                if let Some(fraction) = progress {
                    ui.add_space(6.0);
                    progress_bar(ui, fraction);
                }
            });
        action
    }

    /// "Delete { model }?": the files go, the model will be downloaded again
    /// when needed.
    fn confirm_delete_window(&mut self, ctx: &egui::Context, model: Model) {
        let center = ctx.content_rect().center();
        let in_use = self.model_in_use(model);
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 400.0,
                ok: t!("models-delete"),
                ok_enabled: !in_use,
                ..DialogSpec::new("models_delete", t!("models-delete-title"), center)
            },
            |ui| {
                ui.label(t!(
                    "models-delete-text",
                    name = model.name(),
                    size = size_text(model.size_on_disk())
                ));
                gap_between_blocks(ui);
                help(ui, &t!("models-delete-hint", feature = model.purpose()));
            },
        );
        if out.ok && !in_use {
            if model == Model::Objects {
                self.object_select.forget();
            }
            self.status = match model.delete() {
                Ok(()) => t!("models-status-deleted", name = model.name()),
                Err(e) => t!(
                    "models-status-delete-failed",
                    name = model.name(),
                    error = e
                ),
            };
        }
        if (out.ok || out.cancel)
            && let Some(w) = &mut self.models_window
        {
            w.confirm_delete = None;
        }
    }
}

/// Size in MB (GB from 1000 MB), decimal units like the download sizes.
pub(super) fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1e6;
    let (value, decimals) = if mb >= 1000.0 {
        (mb / 1000.0, 1)
    } else if mb >= 10.0 || bytes == 0 {
        (mb, 0)
    } else {
        (mb, 1)
    };
    let number = crate::i18n::decimal(value, decimals);
    if mb >= 1000.0 {
        t!("models-size-gb", size = number)
    } else {
        t!("models-size-mb", size = number)
    }
}

/// Where the model runs: known once it has run in this session (SAM's two
/// parts may run on different devices), otherwise what will be tried (the
/// graphics card, through DirectML, on Windows).
fn device_text(model: Model) -> String {
    match model.devices()[..] {
        [Device::Gpu] => t!("models-device-gpu"),
        [Device::Cpu] => t!("models-device-cpu"),
        [_, _] => t!("ai-device-gpu-and-cpu"),
        _ if cfg!(windows) => t!("models-device-gpu-if-available"),
        _ => t!("models-device-cpu"),
    }
}

/// Small label above its value.
fn detail(ui: &mut egui::Ui, label: &str, value: &str, width: f32) {
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        let label = egui::RichText::new(label).size(11.0).color(color::LABEL);
        ui.add(egui::Label::new(label).truncate());
        let value = egui::RichText::new(value).size(12.0).color(color::TEXT);
        ui.add(egui::Label::new(value).truncate());
    });
}

/// Colored dot and text.
fn state_chip(ui: &mut egui::Ui, text: &str, tint: Color32) {
    let (dot, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
    ui.painter().circle_filled(dot.center(), 3.0, tint);
    ui.add_space(-4.0);
    ui.label(egui::RichText::new(text).size(12.0).color(tint));
}

/// Download bar and its percentage.
fn progress_bar(ui: &mut egui::Ui, fraction: f32) {
    let fraction = fraction.clamp(0.0, 1.0);
    let percent = crate::i18n::percent(format!("{:.0}", fraction * 100.0));
    ui.horizontal(|ui| {
        let text_w = 40.0;
        let w = ui.available_width() - text_w - ui.spacing().item_spacing.x;
        let (rect, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
        let track = Rect::from_center_size(rect.center(), vec2(w, 6.0));
        ui.painter().rect_filled(track, 3.0, color::BG3);
        let mut done = track;
        done.set_width(w * fraction);
        ui.painter().rect_filled(done, 3.0, color::ACCENT);
        let (rect, _) = ui.allocate_exact_size(vec2(text_w, 18.0), Sense::hover());
        ui.painter().text(
            rect.right_center(),
            egui::Align2::RIGHT_CENTER,
            percent,
            ui_kit::mono(12.0),
            color::MUTED,
        );
    });
}

/// Shows `path` in the system's file manager (created if needed).
fn open_folder(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(program).arg(path).spawn()?;
    // Reaped in the background.
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
impl ModelsWindow {
    /// Open on the deletion confirmation of `model`.
    pub(super) fn confirming(model: Model) -> Self {
        Self {
            confirm_delete: Some(model),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    /// A red disc on blue: the non-AI search finds it.
    fn disc_image() -> image::RgbaImage {
        image::RgbaImage::from_fn(400, 300, |x, y| {
            let (dx, dy) = (x as f32 - 200.0, y as f32 - 150.0);
            if dx * dx + dy * dy < 75.0f32.powi(2) {
                image::Rgba([200, 30, 30, 255])
            } else {
                image::Rgba([30, 120, 200, 255])
            }
        })
    }

    fn grey_image() -> image::RgbaImage {
        image::RgbaImage::from_pixel(400, 300, image::Rgba([90, 90, 90, 255]))
    }

    /// Two documents of the same size (same layer ids): B, then A, active.
    fn two_tabs() -> Harness<'static, AtelierApp> {
        let mut harness = Harness::builder()
            .with_size([1400.0, 1000.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                app.doc = Some(Document::new(grey_image(), None));
                app.add_tab(Document::new(disc_image(), None));
                app
            });
        harness.run_steps(2);
        harness
    }

    /// Lets the subject search end (non-AI: a fraction of a second).
    fn finish_search(harness: &mut Harness<'_, AtelierApp>) {
        for _ in 0..100 {
            harness.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn untouched(app: &AtelierApp) -> bool {
        let doc = app.doc.as_ref().expect("a document");
        app.selection.is_none() && *doc.active_layer().image() == grey_image()
    }

    /// Select Subject or Remove Background on A, then A is closed: B,
    /// now active, gets neither A's subject nor A's background removal.
    #[test]
    fn subject_search_never_lands_on_the_next_tab() {
        for remove in [false, true] {
            let mut harness = two_tabs();
            let app = harness.state_mut();
            app.start_subject(remove, false);
            app.close_document();
            finish_search(&mut harness);
            assert!(untouched(harness.state()), "remove background: {remove}");
        }
    }

    /// Even if the document were replaced without forgetting the search,
    /// its result is only applied to the document searched.
    #[test]
    fn subject_result_checks_the_document() {
        for remove in [false, true] {
            let mut harness = two_tabs();
            let app = harness.state_mut();
            app.start_subject(remove, false);
            app.doc = Some(Document::new(grey_image(), None));
            finish_search(&mut harness);
            let app = harness.state();
            assert!(app.subject_job.is_none());
            assert!(untouched(app), "remove background: {remove}");
        }
    }

    /// The layer painted during Remove Background: it is left as it is.
    #[test]
    fn remove_background_skips_a_layer_changed_meanwhile() {
        let mut harness = two_tabs();
        let app = harness.state_mut();
        app.start_subject(true, false);
        let doc = app.doc.as_mut().unwrap();
        doc.active_layer_mut()
            .pixels_mut()
            .put_pixel(0, 0, image::Rgba([1, 2, 3, 255]));
        let painted = doc.active_layer().image().clone();
        finish_search(&mut harness);
        let app = harness.state();
        assert_eq!(*app.doc.as_ref().unwrap().active_layer().image(), painted);
        assert_eq!(app.status, t!("select-layer-changed"));
    }

    /// Cancel frees the model at once, even if the download's thread is
    /// stuck: the status says so, and it can be downloaded again.
    #[test]
    fn cancel_ends_a_download_at_once() {
        let mut downloads = downloads::Downloads::default();
        downloads.simulate(Model::Objects, 0.4);
        downloads.cancel(Model::Objects);
        assert!(!downloads.is_downloading(Model::Objects));
        let ended = downloads.finished();
        assert!(matches!(
            ended[..],
            [(Model::Objects, Err(DownloadError::Cancelled))]
        ));
        assert!(downloads.finished().is_empty());
    }
}
