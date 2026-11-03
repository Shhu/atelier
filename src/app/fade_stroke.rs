//! Edit › Fade, Edit › Stroke and Image › Auto Tone, Auto Contrast and Auto
//! Color: their windows and their work on another thread.

use std::sync::{Arc, OnceLock};

use image::{GrayImage, RgbaImage};

use super::actions_panel::Stopped;
use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, full_segmented, gap_between_blocks, slider_row,
};
use super::filters_ui::{Job, active_target, cancellable_indicator};
use super::*;
use crate::actions::{MenuItem, Recorded};
use crate::i18n::{Message, t, tm};
use crate::pixel_ops::{AutoMode, Fade, StrokeLocation, StrokeSettings, auto_levels_within};

pub(super) const SHORTCUT_FADE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::F);
pub(super) const SHORTCUT_AUTO_TONE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::L);
pub(super) const SHORTCUT_AUTO_CONTRAST: KeyboardShortcut = KeyboardShortcut::new(
    Modifiers::COMMAND.plus(Modifiers::SHIFT).plus(Modifiers::ALT),
    Key::L,
);
pub(super) const SHORTCUT_AUTO_COLOR: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::B);

/// The Fade window.
pub(super) struct FadeDialog {
    pub(super) fade: Fade,
    pub(super) preview: bool,
}

/// What the Stroke window opens with, kept between sessions. The color
/// follows the foreground until one is chosen.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct StrokeOptions {
    pub(super) width: u32,
    pub(super) color: Option<[u8; 3]>,
    pub(super) location: StrokeLocation,
    pub(super) mode: BlendMode,
    pub(super) opacity: f32,
    pub(super) preserve_transparency: bool,
}

impl Default for StrokeOptions {
    fn default() -> Self {
        Self {
            width: 1,
            color: None,
            location: StrokeLocation::Center,
            mode: BlendMode::Normal,
            opacity: 1.0,
            preserve_transparency: false,
        }
    }
}

/// The name of an Auto command.
pub(super) fn auto_title(mode: AutoMode) -> String {
    match mode {
        AutoMode::Tone => t!("adjust8-auto-tone"),
        AutoMode::Contrast => t!("adjust8-auto-contrast"),
        AutoMode::Color => t!("adjust8-auto-color"),
    }
}

/// A command computed on another thread.
#[derive(Clone)]
enum Work {
    Auto(AutoMode),
    Fade(Fade),
    /// Its step is recorded and the layer ready for the result; the work
    /// leaves the band's coverage (canvas position) for Fade.
    Stroke(StrokeSettings, Arc<OnceLock<(GrayImage, (i64, i64))>>),
}

impl Work {
    fn title(&self) -> String {
        match self {
            Work::Auto(mode) => auto_title(*mode),
            Work::Fade(_) => t!("adjust8-fade-title"),
            Work::Stroke(..) => t!("adjust8-stroke-title"),
        }
    }
}

/// One under way: for the document, layer and target revision then.
pub(super) struct Processing {
    work: Work,
    doc: u64,
    layer: u64,
    rev: u64,
    started: f64,
    job: Job<RgbaImage>,
}

impl AtelierApp {
    fn start_processing(&mut self, work: Work, job: Job<RgbaImage>) {
        let Some(doc) = &self.doc else { return };
        self.filters.processing = Some(Processing {
            work,
            doc: doc.uid(),
            layer: doc.active_layer().id,
            rev: doc.target_rev(),
            started: self.ctx.input(|i| i.time),
            job,
        });
    }

    /// Image › Auto Tone, Auto Contrast, Auto Color, inside the selection.
    pub(super) fn start_auto(&mut self, mode: AutoMode) {
        if self.smart_filters_apply() {
            return self.refuse_on_smart();
        }
        let Some(doc) = &self.doc else { return };
        if doc.target_layer().is_none() {
            self.status_cannot(t!("adjust11-no-pixels"));
            return;
        }
        let Some(target) = active_target(doc, self.selection.as_ref()) else {
            self.status_cannot(t!("status-selection-empty"));
            return;
        };
        let job = Job::spawn(&self.ctx, 0, [0; 4], Some(format!("Auto {mode:?}")), move || {
            // Measured on the selected pixels, not the selection's box.
            let levels = auto_levels_within(&target.image, target.clip.as_ref(), mode);
            target.process(0, |image, _| crate::pixel_ops::levels(image, &levels))
        });
        self.start_processing(Work::Auto(mode), job);
    }

    /// Edit › Fade: its window, on what can be faded.
    pub(super) fn open_fade(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        match doc.fade_source(self.selection.as_ref()) {
            Some(fade) => self.filters.fade = Some(FadeDialog { fade, preview: true }),
            None => self.status_cannot(t!("adjust9-why-no-fade")),
        }
    }

    /// The last result faded at once (an action played).
    pub(super) fn fade_with(&mut self, opacity: f32, mode: BlendMode) -> Result<(), String> {
        // The command before it changed nothing on this image.
        if self.doc.as_ref().is_some_and(Document::fade_of_nothing) {
            return Ok(());
        }
        let selection = self.selection.as_ref();
        let fade = self.doc.as_mut().and_then(|d| d.fade_source(selection));
        let Some(fade) = fade else {
            return Err(t!("adjust9-why-no-fade"));
        };
        self.apply_fade(Fade {
            opacity,
            mode,
            ..fade
        });
        Ok(())
    }

    /// OK of the Fade window. Its pixels are all faded, the selection long
    /// applied: the preview shown is the result, or it is computed.
    fn apply_fade(&mut self, fade: Fade) {
        if fade.opacity >= 1.0 && fade.mode == BlendMode::Normal {
            return self.record(recorded_fade(&fade));
        }
        let Some(doc) = &self.doc else { return };
        let (id, rev) = (doc.active_layer().id, doc.target_rev());
        let wanted = LayerPreview::Fade(fade.clone());
        if let Some(p) = self
            .preview_shown
            .take_if(|p| (&p.preview, p.layer, p.rev) == (&wanted, id, rev))
        {
            return self.put_fade(&fade, Arc::unwrap_or_clone(p.image));
        }
        let Some(target) = active_target(doc, None) else {
            return;
        };
        let f = fade.clone();
        let job = Job::spawn(&self.ctx, 0, [0; 4], Some("Fade".to_owned()), move || {
            target.process(0, |image, _| f.apply(image))
        });
        self.start_processing(Work::Fade(fade), job);
    }

    fn put_fade(&mut self, fade: &Fade, image: RgbaImage) {
        let Some(doc) = &mut self.doc else { return };
        let Some(name) = doc.fade_name().map(str::to_owned) else {
            return;
        };
        let step = t!("adjust8-hist-fade", name = name.as_str());
        if doc.fade_put(step.clone(), image) {
            self.status.set(step);
            self.record(recorded_fade(fade));
            self.reset_pending();
        }
    }

    pub(super) fn fade_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.filters.fade.take() else {
            return;
        };
        let name = self.doc.as_ref().and_then(Document::fade_name);
        let title = name.map_or_else(
            || t!("adjust8-fade-title"),
            |name| t!("adjust8-hist-fade", name = name),
        );
        let changed = d.fade.opacity < 1.0 || d.fade.mode != BlendMode::Normal;
        let mut preview = d.preview;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                reset: Some(changed),
                preview: Some(&mut preview),
                ..DialogSpec::new("fade", title, dialogs::tool_dialog_pos(ctx))
            },
            |ui| {
                let mut opacity = d.fade.opacity * 100.0;
                slider_row(
                    ui,
                    &t!("dialog-fill-opacity"),
                    None,
                    &mut opacity,
                    0.0..=100.0,
                    &i18n::percent(""),
                    Track::Accent,
                    100.0,
                );
                d.fade.opacity = opacity / 100.0;
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("adjust8-mode"));
                    let width = ui.available_width();
                    right_panel::blend_combo(ui, "fade_mode", &mut d.fade.mode, false, width);
                });
            },
        );
        d.preview = preview;
        if out.reset {
            d.fade.opacity = 1.0;
            d.fade.mode = BlendMode::Normal;
        }
        let done = out.ok || out.cancel;
        if out.ok {
            self.apply_fade(d.fade.clone());
        }
        if done {
            self.forget_adjust_preview();
            self.end_layer_preview();
            if let Some(doc) = &self.doc {
                doc.release_stand_in();
            }
        } else {
            self.filters.fade = Some(d);
        }
    }

    /// Whether the target has pixels a stroke can go on (a text or shape
    /// layer: rasterized first).
    pub(super) fn can_stroke(&self) -> bool {
        self.doc
            .as_ref()
            .is_some_and(|d| d.quick_mask().is_some() || d.can_paint())
    }

    /// Edit › Stroke: its window, as it was last used.
    pub(super) fn open_stroke(&mut self) {
        if self.smart_filters_apply() {
            return self.refuse_on_smart();
        }
        if self.doc.is_none() {
            return;
        }
        if self.can_stroke() {
            self.filters.stroke_open = true;
        } else {
            self.status_cannot(t!("adjust11-no-pixels"));
        }
    }

    pub(super) fn stroke_window(&mut self, ctx: &egui::Context) {
        if !self.filters.stroke_open {
            return;
        }
        let mut o = self.filters.stroke_options;
        let [r, g, b, _] = self.primary.to_srgba_unmultiplied();
        let [r, g, b] = o.color.unwrap_or([r, g, b]);
        let mut color = Color32::from_rgb(r, g, b);
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new(
                    "stroke",
                    t!("adjust8-stroke-title"),
                    ctx.content_rect().center(),
                )
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("adjust8-stroke-section"));
                    ui.horizontal(|ui| {
                        ui_kit::number_field(
                            ui,
                            &t!("adjust8-stroke-width"),
                            &mut o.width,
                            1..=StrokeSettings::MAX_WIDTH,
                            " px",
                            120.0,
                        );
                        let was = color;
                        ui_kit::swatch(ui, &mut color, 28.0, &t!("adjust8-stroke-color-tip"));
                        if color != was {
                            let [r, g, b, _] = color.to_srgba_unmultiplied();
                            o.color = Some([r, g, b]);
                        }
                    });
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("adjust8-stroke-location"));
                    let (inside, center, outside) = (
                        t!("adjust8-stroke-inside"),
                        t!("adjust8-stroke-center"),
                        t!("adjust8-stroke-outside"),
                    );
                    full_segmented(
                        ui,
                        &mut o.location,
                        &[
                            (StrokeLocation::Inside, &inside),
                            (StrokeLocation::Center, &center),
                            (StrokeLocation::Outside, &outside),
                        ],
                    );
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("adjust8-mode"));
                    let width = ui.available_width();
                    right_panel::blend_combo(ui, "stroke_mode", &mut o.mode, false, width);
                });
                gap_between_blocks(ui);
                let mut opacity = o.opacity * 100.0;
                slider_row(
                    ui,
                    &t!("dialog-fill-opacity"),
                    None,
                    &mut opacity,
                    0.0..=100.0,
                    &i18n::percent(""),
                    Track::Accent,
                    100.0,
                );
                o.opacity = opacity / 100.0;
                gap_between_blocks(ui);
                ui_kit::checkbox(
                    ui,
                    &mut o.preserve_transparency,
                    &t!("dialog-fill-preserve-transparency"),
                );
            },
        );
        self.filters.stroke_options = o;
        if out.ok || out.cancel {
            self.filters.stroke_open = false;
        }
        if out.ok {
            let [color_r, color_g, color_b, _] = color.to_srgba_unmultiplied();
            self.apply_stroke(StrokeSettings {
                width: o.width,
                color: [color_r, color_g, color_b],
                location: o.location,
                mode: o.mode,
                opacity: o.opacity,
                preserve_transparency: o.preserve_transparency,
            });
        }
    }

    /// Strokes the edge of the selection (else of the layer's visible
    /// pixels) on the target, in one step, on another thread.
    pub(super) fn apply_stroke(&mut self, s: StrokeSettings) {
        if self.smart_filters_apply() {
            return self.refuse_on_smart();
        }
        if self.refuse_hidden_layer() || self.refuse_if_target_locked() {
            return;
        }
        // A text or shape layer: rasterized first, if the user agrees.
        if self.rasterize_then(move |app| app.apply_stroke(s)) {
            return;
        }
        let selection = self.selection.clone();
        let Some(doc) = &mut self.doc else { return };
        let canvas = doc.full_rect();
        if selection.as_ref().is_some_and(|sel| sel.bounds(canvas).is_none()) {
            self.status_cannot(t!("status-selection-empty"));
            return;
        }
        doc.checkpoint(t!("adjust8-stroke-title"));
        // The layer grows to the canvas, or its targeted mask stands in for
        // it, until the result is in.
        if !doc.begin_paint() {
            doc.discard_step();
            self.status_cannot(t!("adjust11-no-pixels"));
            return;
        }
        let Some((image, offset)) = doc.target_layer().map(|l| (l.shared_image(), l.offset)) else {
            doc.end_paint();
            doc.discard_step();
            self.status_cannot(t!("adjust11-no-pixels"));
            return;
        };
        let band = Arc::new(OnceLock::new());
        let covered = band.clone();
        let job = Job::spawn(&self.ctx, 0, [0; 4], Some("Stroke".to_owned()), move || {
            let (out, mask, at) =
                crate::pixel_ops::stroke_covering(&image, offset, selection.as_ref(), canvas, &s);
            let _ = covered.set((mask, at));
            out
        });
        self.start_processing(Work::Stroke(s, band), job);
    }

    /// The indicator of the work under way, then its result put in.
    pub(super) fn processing_window(&mut self, ctx: &egui::Context) {
        let Some(p) = &self.filters.processing else {
            return;
        };
        let result = match p.job.poll() {
            Ok(None) => {
                let name = p.work.title();
                if cancellable_indicator(ctx, "processing_busy", &name, p.started, None) {
                    let p = self.filters.processing.take().expect("checked above");
                    self.end_stroke_step(&p);
                    self.status.set(t!("retouch-status-cancelled", name = name.as_str()));
                    self.actions.step_stopped(Stopped::Cancelled);
                    self.reset_pending();
                }
                return;
            }
            Ok(Some(image)) => Some(image),
            Err(()) => None,
        };
        let p = self.filters.processing.take().expect("checked above");
        let current = self.doc.as_ref().is_some_and(|d| {
            (d.uid(), d.active_layer().id, d.target_rev()) == (p.doc, p.layer, p.rev)
        });
        match result {
            Some(image) if current => self.put_processed(&p.work, image),
            Some(_) => self.end_stroke_step(&p),
            None => {
                self.end_stroke_step(&p);
                let work = p.work.clone();
                let text = tm!("fx3-filter-failed", name = Message::of(|| work.title()));
                self.actions
                    .step_stopped(Stopped::Failed(text.shown.clone()));
                self.status_failed(text);
            }
        }
        self.reset_pending();
        if let Some(doc) = &self.doc {
            doc.release_stand_in();
        }
    }

    /// A stroke given up: its step goes.
    fn end_stroke_step(&mut self, p: &Processing) {
        if let (Work::Stroke(..), Some(doc)) = (&p.work, &mut self.doc)
            && (doc.uid(), doc.active_layer().id) == (p.doc, p.layer)
        {
            doc.end_paint();
            doc.discard_step();
        }
    }

    fn put_processed(&mut self, work: &Work, image: RgbaImage) {
        let Some(doc) = &mut self.doc else { return };
        match work {
            Work::Auto(mode) => {
                let same = doc.target_layer().is_some_and(|l| *l.image() == image);
                let name = auto_title(*mode);
                if same {
                    doc.mark_fade_nothing();
                } else if doc.put_processed_active(name.clone(), image) {
                    doc.mark_fade();
                    self.status.set(t!("dialog-adjust-applied", name = name.as_str()));
                }
                self.record(Recorded::Menu {
                    item: match mode {
                        AutoMode::Tone => MenuItem::AutoTone,
                        AutoMode::Contrast => MenuItem::AutoContrast,
                        AutoMode::Color => MenuItem::AutoColor,
                    },
                });
            }
            Work::Fade(fade) => self.put_fade(fade, image),
            Work::Stroke(s, band) => {
                if *doc.active_layer().image() == image {
                    doc.end_paint();
                    doc.discard_step();
                    doc.mark_fade_nothing();
                    let soft = self.selection.as_ref().is_some_and(barely_selected);
                    self.status.set(if soft {
                        t!("adjust9-stroke-soft")
                    } else {
                        t!("adjust9-stroke-nothing")
                    });
                } else {
                    doc.active_layer_mut().replace_pixels(image);
                    doc.end_paint();
                    // Fade blends over the band.
                    match band.get() {
                        Some((mask, at)) => doc.mark_fade_painted(mask.clone(), *at),
                        None => doc.mark_fade_changed(),
                    }
                    self.status.set(t!("adjust8-stroke-title"));
                }
                self.record(recorded_stroke(s));
            }
        }
    }
}

/// No pixel is more than 50% selected: a selection without an edge to stroke.
fn barely_selected(selection: &Selection) -> bool {
    !selection.inverted
        && selection
            .mask
            .as_ref()
            .is_some_and(|m| m.as_raw().iter().all(|&v| v < 128))
}

fn recorded_fade(fade: &Fade) -> Recorded {
    Recorded::Fade {
        opacity: fade.opacity,
        mode: format!("{:?}", fade.mode),
    }
}

fn recorded_stroke(s: &StrokeSettings) -> Recorded {
    Recorded::Stroke {
        width: s.width,
        color: s.color,
        location: format!("{:?}", s.location),
        mode: format!("{:?}", s.mode),
        opacity: s.opacity,
        preserve_transparency: s.preserve_transparency,
    }
}

/// A blend mode by its recorded name.
pub(super) fn blend_named(name: &str) -> BlendMode {
    BlendMode::GROUPS
        .iter()
        .flat_map(|g| g.iter().copied())
        .find(|m| format!("{m:?}") == name)
        .unwrap_or(BlendMode::Normal)
}

/// A stroke's location by its recorded name.
/// The name of a stroke's location, as the Stroke window shows it.
pub(super) fn location_label(location: StrokeLocation) -> String {
    match location {
        StrokeLocation::Inside => t!("adjust8-stroke-inside"),
        StrokeLocation::Center => t!("adjust8-stroke-center"),
        StrokeLocation::Outside => t!("adjust8-stroke-outside"),
    }
}

pub(super) fn location_named(name: &str) -> StrokeLocation {
    [
        StrokeLocation::Inside,
        StrokeLocation::Center,
        StrokeLocation::Outside,
    ]
    .into_iter()
    .find(|l| format!("{l:?}") == name)
    .unwrap_or_default()
}
