//! Filter › Camera Raw Filter… (Shift+Ctrl+A), and camera RAW files, which
//! open in the same window: a large preview (zoom, pan, before / after),
//! the histogram and its clipping warnings, the settings' panels on the
//! right. The preview is developed off the UI thread at the screen's
//! resolution; OK develops at full resolution, in one undo step (a RAW
//! file: a new document).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;

use image::RgbaImage;

use super::actions_panel::Stopped;
use super::dialogs::{
    DialogSpec, Steps, Track, block, channel_selector, curves_editor, dialog_body_height,
    floating_dialog, full_segmented, gap_between_blocks, help, level_track, slider_row_steps,
};
use super::*;
use crate::actions::Recorded;
use crate::camera_raw::{
    self, Analysis, BUILT_IN_PRESETS, BuiltIn, Frame, Preset, Profile, Settings, Source,
    VignetteStyle, Visual, WhiteBalance,
};
use crate::document::{FileError, SmartFilterKind};
use crate::i18n::t;
use ui_kit::color;

/// Camera Raw Filter: Shift+Ctrl+A, like Photoshop.
pub(super) const SHORTCUT_CAMERA_RAW: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::A);

/// Zoom levels of the preview (screen pixels per image pixel).
const ZOOMS: [f32; 13] = [
    0.0625, 0.125, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0,
];
/// Width of the panels' column.
const SIDE: f32 = 300.0;
/// Longest side of the development the histogram is counted on.
const HISTOGRAM_SIDE: f32 = 640.0;

/// Camera Raw in the session: its window, a RAW file being read, a
/// development being done, the user's presets.
#[derive(Default)]
pub(super) struct CameraRaw {
    window: Option<Box<RawWindow>>,
    loading: Option<Loading>,
    developing: Option<Developing>,
    /// Read when first shown.
    presets: Option<Vec<Preset>>,
}

impl CameraRaw {
    /// The window, a file being read or a development keep the keyboard
    /// and the image.
    pub(super) fn busy(&self) -> bool {
        self.window.is_some() || self.loading.is_some() || self.developing.is_some()
    }
}

/// What the window develops for.
#[derive(Clone)]
enum Purpose {
    /// The active layer (or its mask) of a document, in the selection.
    Filter { doc: u64, layer: u64 },
    /// A smart filter of the active smart object of a document: the one
    /// re-edited, or a new one.
    SmartFilter { doc: u64, layer: u64 },
    /// A RAW file to open; `preview_only`: only its embedded preview could
    /// be read.
    Open { path: PathBuf, preview_only: bool },
}

/// Pixels being prepared for the window (a RAW file being read).
struct Loading {
    purpose: Purpose,
    /// What the window opens with (else the defaults).
    settings: Option<Settings>,
    started: f64,
    rx: mpsc::Receiver<Result<Loaded, FileError>>,
}

struct Loaded {
    source: Source,
    analysis: Analysis,
    preview_only: bool,
}

struct Developing {
    destination: Destination,
    started: f64,
    progress: Arc<AtomicU32>,
    rx: mpsc::Receiver<RgbaImage>,
    /// Set to stop the development (cancelled).
    cancel: Arc<AtomicBool>,
}

/// Where a development goes.
enum Destination {
    /// The pixels of a document's layer; `rasterized`: it was an editable
    /// text.
    Layer {
        doc: u64,
        layer: u64,
        rasterized: bool,
        /// Remembered (Last Filter, an action) once developed.
        settings: Box<Settings>,
    },
    /// A new document, from a RAW file.
    Document { path: PathBuf, preview_only: bool },
}

/// The panels, in Camera Raw's order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    Basic,
    Curve,
    Mixer,
    Grading,
    Detail,
    Effects,
    Lens,
    Presets,
}

const PANELS: [Panel; 8] = [
    Panel::Basic,
    Panel::Curve,
    Panel::Mixer,
    Panel::Grading,
    Panel::Detail,
    Panel::Effects,
    Panel::Lens,
    Panel::Presets,
];

impl Panel {
    fn title(self) -> String {
        match self {
            Panel::Basic => t!("raw-panel-basic"),
            Panel::Curve => t!("raw-panel-curve"),
            Panel::Mixer => t!("raw-panel-mixer"),
            Panel::Grading => t!("raw-panel-grading"),
            Panel::Detail => t!("raw-panel-detail"),
            Panel::Effects => t!("raw-panel-effects"),
            Panel::Lens => t!("raw-panel-lens"),
            Panel::Presets => t!("raw-panel-presets"),
        }
    }

    /// Whether `a` and `b` differ in what this panel sets.
    fn differs(self, a: &Settings, b: &Settings) -> bool {
        match self {
            Panel::Basic => {
                let mut x = a.clone();
                x.curve = b.curve.clone();
                x.mixer = b.mixer.clone();
                x.grading = b.grading.clone();
                x.detail = b.detail.clone();
                x.effects = b.effects.clone();
                x.lens = b.lens.clone();
                x != *b
            }
            Panel::Curve => a.curve != b.curve,
            Panel::Mixer => a.mixer != b.mixer,
            Panel::Grading => a.grading != b.grading,
            Panel::Detail => a.detail != b.detail,
            Panel::Effects => a.effects != b.effects,
            Panel::Lens => a.lens != b.lens,
            Panel::Presets => false,
        }
    }

    /// What this panel sets, back to `defaults`.
    fn reset(self, s: &mut Settings, defaults: &Settings) {
        match self {
            Panel::Basic => {
                *s = Settings {
                    curve: s.curve.clone(),
                    mixer: s.mixer.clone(),
                    grading: s.grading.clone(),
                    detail: s.detail.clone(),
                    effects: s.effects.clone(),
                    lens: s.lens.clone(),
                    ..defaults.clone()
                }
            }
            Panel::Curve => s.curve = defaults.curve.clone(),
            Panel::Mixer => s.mixer = defaults.mixer.clone(),
            Panel::Grading => s.grading = defaults.grading.clone(),
            Panel::Detail => s.detail = defaults.detail.clone(),
            Panel::Effects => s.effects = defaults.effects.clone(),
            Panel::Lens => s.lens = defaults.lens.clone(),
            Panel::Presets => {}
        }
    }
}

/// The open window.
struct RawWindow {
    purpose: Purpose,
    source: Arc<Source>,
    analysis: Arc<Analysis>,
    /// The canvas's color table (a filter): the preview looks as the
    /// canvas does.
    lut: Option<Arc<crate::cms::Lut>>,
    /// The settings a reset goes back to, those the window opened with
    /// (Before), those being edited.
    defaults: Settings,
    opened: Settings,
    settings: Settings,
    view: View,
    open_panels: Vec<Panel>,
    /// Color Mixer: hue, saturation or luminance shown.
    mixer_tab: usize,
    /// Tone Curve: point curve shown (else parametric), its channel and
    /// selected point.
    point_curve: bool,
    curve_channel: usize,
    curve_point: Option<usize>,
    /// Color Grading: shadows, midtones, highlights or global shown.
    grading_tab: usize,
    /// A preset name being typed.
    preset_name: Option<String>,
    preview: Preview,
}

/// How the preview is seen.
struct View {
    /// Screen pixels per image pixel; `None`: fit in the view.
    zoom: Option<f32>,
    /// The zoom that fits, last computed.
    fit: f32,
    /// Image point at the center of the view.
    center: Pos2,
    /// The settings the window opened with are shown (\).
    before: bool,
    /// Before and after side by side (Y).
    side_by_side: bool,
    clip_shadows: bool,
    clip_highlights: bool,
    /// White Balance tool (I): a click on the image makes it neutral.
    sampling: bool,
    /// Alt held on a sharpening slider.
    visual: Visual,
    /// Values under the pointer.
    hover: Option<[u8; 3]>,
}

#[derive(Default)]
struct Preview {
    /// The source reduced for the last scale developed.
    scaled: Option<(u32, Arc<Source>)>,
    /// After, before.
    panes: [Pane; 2],
    /// Of the whole image developed (small), for the settings of a key.
    histogram: Option<Box<[[u32; 256]; 3]>>,
    histogram_key: u64,
    histogram_job: Option<(u64, mpsc::Receiver<Developed>)>,
    /// The source reduced for the histogram.
    small: Option<(u32, Arc<Source>)>,
    /// Key of a histogram whose computation failed (not tried again).
    histogram_failed: Option<u64>,
    /// A computation failed since the last frame (to say so).
    failure: bool,
}

#[derive(Default)]
struct Pane {
    shown: Option<Shown>,
    job: Option<(u64, mpsc::Receiver<Developed>)>,
    /// Key of a development that failed (not tried again).
    failed: Option<u64>,
}

/// Developed pixels on screen.
struct Shown {
    key: u64,
    /// Region of the image they show (full-resolution pixels).
    region: Rect,
    texture: TextureHandle,
    image: Arc<RgbaImage>,
    /// Clipping warnings (shadows, highlights) drawn over them.
    overlay: Option<((bool, bool), TextureHandle)>,
}

struct Developed {
    image: RgbaImage,
    region: Rect,
    scaled: (u32, Arc<Source>),
    histogram: Box<[[u32; 256]; 3]>,
}

/// What a pane must show: zoom, center, region of the image (full
/// resolution) and the scale it develops at.
struct Wanted {
    zoom: f32,
    center: Pos2,
    region: [u32; 4],
    scale: f32,
}

impl View {
    fn wanted(&mut self, pane: Vec2, size: (u32, u32)) -> Wanted {
        let (w, h) = (size.0 as f32, size.1 as f32);
        let fit = (pane.x / w).min(pane.y / h).max(1e-4);
        self.fit = fit;
        let zoom = self.zoom.unwrap_or(fit);
        let half = pane / zoom / 2.0;
        let along = |c: f32, half: f32, len: f32| {
            if half * 2.0 >= len {
                len / 2.0
            } else {
                c.clamp(half, len - half)
            }
        };
        self.center = pos2(
            along(self.center.x, half.x, w),
            along(self.center.y, half.y, h),
        );
        let region = [
            (self.center.x - half.x).floor().max(0.0) as u32,
            (self.center.y - half.y).floor().max(0.0) as u32,
            ((self.center.x + half.x).ceil().min(w) as u32).max(1),
            ((self.center.y + half.y).ceil().min(h) as u32).max(1),
        ];
        Wanted {
            zoom,
            center: self.center,
            region,
            scale: zoom.min(1.0),
        }
    }
}

/// Develops `region` (full-resolution pixels) of `source` at `scale`, with
/// the surroundings that local operations need.
fn develop_region(
    source: &Arc<Source>,
    cached: Option<(u32, Arc<Source>)>,
    analysis: &Analysis,
    settings: &Settings,
    region: [u32; 4],
    scale: f32,
    visual: Visual,
) -> Developed {
    let key = scale.to_bits();
    let scaled = match cached {
        Some((k, s)) if k == key => s,
        _ if scale >= 1.0 => source.clone(),
        _ => Arc::new(source.downscaled(scale)),
    };
    let (sw, sh) = (scaled.width, scaled.height);
    let k = sw as f32 / source.width as f32;
    let [x0, y0, x1, y1] = region;
    let r = [
        ((x0 as f32 * k).floor() as u32).min(sw - 1),
        ((y0 as f32 * k).floor() as u32).min(sh - 1),
        ((x1 as f32 * k).ceil() as u32).clamp(1, sw),
        ((y1 as f32 * k).ceil() as u32).clamp(1, sh),
    ];
    let r = [r[0], r[1], r[2].max(r[0] + 1), r[3].max(r[1] + 1)];
    let whole = [0, 0, sw, sh];
    let margin = if r == whole {
        0
    } else {
        let long = sw.max(sh) as f32;
        let diagonal = ((sw * sw + sh * sh) as f32).sqrt();
        (0.06 * long + 8.0 + settings.lens.distortion.abs() / 100.0 * 0.08 * diagonal).ceil() as u32
    };
    let ext = [
        r[0].saturating_sub(margin),
        r[1].saturating_sub(margin),
        (r[2] + margin).min(sw),
        (r[3] + margin).min(sh),
    ];
    let part = if ext == whole {
        None
    } else {
        Some(scaled.crop(ext))
    };
    let frame = Frame {
        size: (sw, sh),
        origin: (ext[0], ext[1]),
        scale: k,
    };
    let developed = camera_raw::render(
        part.as_ref().unwrap_or(&scaled),
        settings,
        &frame,
        analysis,
        visual,
        None,
    );
    let image = if ext == r {
        developed
    } else {
        image::imageops::crop_imm(
            &developed,
            r[0] - ext[0],
            r[1] - ext[1],
            r[2] - r[0],
            r[3] - r[1],
        )
        .to_image()
    };
    let mut histogram = Box::new([[0u32; 256]; 3]);
    for p in image.pixels().filter(|p| p[3] > 0) {
        for c in 0..3 {
            histogram[c][p.0[c] as usize] += 1;
        }
    }
    Developed {
        image,
        region: Rect::from_min_max(
            pos2(r[0] as f32 / k, r[1] as f32 / k),
            pos2(r[2] as f32 / k, r[3] as f32 / k),
        ),
        scaled: (key, scaled),
        histogram,
    }
}

impl AtelierApp {
    /// Filter › Camera Raw Filter…: the window on the active layer (or
    /// its mask) in the selection, or for a smart filter of a smart object;
    /// opening with `settings` (a smart filter re-edited, an action's step).
    pub(super) fn camera_raw_filter(&mut self, settings: Option<Settings>) {
        if self.refuse_hidden_target() {
            return;
        }
        if self.smart_filters_apply() {
            let Some(target) = self.smart_filter_input() else {
                return;
            };
            let Some(doc) = &self.doc else { return };
            let purpose = Purpose::SmartFilter {
                doc: doc.uid(),
                layer: doc.active_layer().id,
            };
            self.load(purpose, settings, move || {
                Ok((Source::from_rgba(&target.image), false))
            });
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        if !doc.can_paint() {
            self.status = t!("filter-no-pixels");
            return;
        }
        // Developed over the whole layer, like the result.
        self.enlarge_for_dialog();
        let Some(doc) = &mut self.doc else { return };
        let Some(target) = filters_ui::active_target(doc, self.selection.as_ref()) else {
            // The layer enlarged above is put back as it was.
            self.shrink_after_preview();
            self.status = t!("status-selection-empty");
            return;
        };
        let purpose = Purpose::Filter {
            doc: doc.uid(),
            layer: doc.active_layer().id,
        };
        self.load(purpose, settings, move || {
            let [x0, y0, x1, y1] = target.area();
            let pixels = image::imageops::crop_imm(&*target.image, x0, y0, x1 - x0, y1 - y0);
            Ok((Source::from_rgba(&pixels.to_image()), false))
        });
    }

    /// Opens a RAW file: read on another thread, then Camera Raw's window.
    pub(super) fn open_raw(&mut self, path: &Path) {
        let purpose = Purpose::Open {
            path: path.to_path_buf(),
            preview_only: false,
        };
        let file = path.to_path_buf();
        self.load(purpose, None, move || {
            camera_raw::raw::read(&file).map(|raw| (raw.source, raw.preview_only))
        });
    }

    /// Prepares the window's pixels on another thread (`read`: the source
    /// and whether it is only a RAW file's preview); it opens with
    /// `settings`.
    fn load(
        &mut self,
        purpose: Purpose,
        settings: Option<Settings>,
        read: impl FnOnce() -> Result<(Source, bool), FileError> + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Camera Raw: read");
            let loaded = read().map(|(source, preview_only)| Loaded {
                analysis: Analysis::of(&source),
                source,
                preview_only,
            });
            let _ = tx.send(loaded);
            ctx.request_repaint();
        });
        self.camera_raw.loading = Some(Loading {
            purpose,
            settings,
            started: self.ctx.input(|i| i.time),
            rx,
        });
    }

    fn open_camera_raw(
        &mut self,
        purpose: Purpose,
        source: Source,
        analysis: Analysis,
        settings: Option<Settings>,
    ) {
        let defaults = Settings::for_source(&source);
        let opened = settings.unwrap_or_else(|| defaults.clone());
        let center = pos2(source.width as f32 / 2.0, source.height as f32 / 2.0);
        let lut = match purpose {
            Purpose::Open { .. } => None,
            _ => self.color.shown().1,
        };
        self.camera_raw.window = Some(Box::new(RawWindow {
            purpose,
            source: Arc::new(source),
            analysis: Arc::new(analysis),
            lut,
            settings: opened.clone(),
            opened,
            defaults,
            view: View {
                zoom: None,
                fit: 1.0,
                center,
                before: false,
                side_by_side: false,
                clip_shadows: false,
                clip_highlights: false,
                sampling: false,
                visual: Visual::Image,
                hover: None,
            },
            open_panels: vec![Panel::Basic],
            mixer_tab: 0,
            point_curve: false,
            curve_channel: 0,
            curve_point: None,
            grading_tab: 0,
            preset_name: None,
            preview: Preview::default(),
        }));
    }

    /// The window, a RAW file being read, a development in progress.
    pub(super) fn camera_raw_window(&mut self, ctx: &egui::Context) {
        self.poll_loading();
        self.poll_development();
        if let Some(l) = &self.camera_raw.loading {
            let name = match &l.purpose {
                Purpose::Filter { .. } | Purpose::SmartFilter { .. } => t!("raw-filter-title"),
                Purpose::Open { path, .. } => file_label(path),
            };
            filters_ui::busy_indicator(ctx, &name, l.started);
            return;
        }
        if let Some(d) = &self.camera_raw.developing {
            let fraction = d.progress.load(Ordering::Relaxed) as f32 / 1000.0;
            let name = t!("raw-developing");
            if filters_ui::cancellable_indicator(
                ctx,
                "camera_raw_busy",
                &name,
                d.started,
                Some(fraction),
            ) {
                self.cancel_development();
            }
            return;
        }
        let Some(mut window) = self.camera_raw.window.take() else {
            return;
        };
        if self.camera_raw.presets.is_none() {
            self.camera_raw.presets = Some(camera_raw::load_presets());
        }
        window.keys(ctx);
        window.update_preview(ctx);
        let title = match &window.purpose {
            Purpose::Filter { .. } | Purpose::SmartFilter { .. } => t!("raw-filter-title"),
            Purpose::Open { path, .. } => {
                format!("{} — {}", t!("raw-window-title"), file_label(path))
            }
        };
        let (ok, note) = match &window.purpose {
            Purpose::Filter { .. } | Purpose::SmartFilter { .. } => (t!("dialog-ok"), None),
            Purpose::Open { preview_only, .. } => (
                t!("raw-open"),
                preview_only.then(|| t!("raw-note-preview-only")),
            ),
        };
        let screen = ctx.content_rect();
        let width = (screen.width() - 16.0).min(1680.0);
        let mut status = None;
        let presets = self.camera_raw.presets.get_or_insert_with(Vec::new);
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width,
                reset: Some(window.settings != window.opened),
                ok,
                note,
                ..DialogSpec::new("camera_raw", title, screen.center())
            },
            |ui| window.body(ui, presets, &mut status),
        );
        if let Some(status) = status {
            self.status = status;
        }
        if std::mem::take(&mut window.preview.failure) {
            self.status_failed(tm!("fx4-preview-failed-any"));
        }
        if out.reset {
            window.settings = window.opened.clone();
            window.curve_point = None;
        }
        if out.ok {
            self.develop(*window);
        } else if out.cancel {
            self.shrink_after_preview();
        } else {
            self.camera_raw.window = Some(window);
        }
    }

    /// The pixels are ready: the window opens (a RAW file that could not
    /// be read says why).
    fn poll_loading(&mut self) {
        let Some(loading) = &self.camera_raw.loading else {
            return;
        };
        let result = match loading.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(FileError::Damaged),
        };
        let Some(loading) = self.camera_raw.loading.take() else {
            return;
        };
        let settings = loading.settings;
        match (result, loading.purpose) {
            (Ok(loaded), Purpose::Open { path, .. }) => {
                let purpose = Purpose::Open {
                    path,
                    preview_only: loaded.preview_only,
                };
                self.open_camera_raw(purpose, loaded.source, loaded.analysis, settings);
            }
            (Ok(loaded), purpose) => {
                self.open_camera_raw(purpose, loaded.source, loaded.analysis, settings)
            }
            (Err(e), Purpose::Open { path, .. }) => {
                let path = files::file_name(&path);
                self.report_error(tm!(
                    "status-open-failed",
                    path = path,
                    error = crate::i18n::Message::of(|| e.to_string())
                ));
            }
            // The Camera Raw Filter's pixels could not be read (it
            // panicked): nothing opens, the layer it enlarged goes back.
            (Err(_), _) => {
                self.shrink_after_preview();
                let name = crate::i18n::Message::of(|| t!("raw-filter-title"));
                let text = tm!("fx3-filter-failed", name = name);
                self.actions
                    .step_stopped(Stopped::Failed(text.shown.clone()));
                self.status_failed(text);
            }
        }
    }

    /// OK: the development at full resolution on another thread, or the
    /// smart filter.
    fn develop(&mut self, window: RawWindow) {
        let settings = window.settings;
        match window.purpose {
            Purpose::Filter { doc, layer } => self.develop_layer(settings, (doc, layer)),
            Purpose::SmartFilter { doc, layer } => {
                let active = self.doc.as_ref().map(|d| (d.uid(), d.active_layer().id));
                if active == Some((doc, layer)) {
                    self.add_camera_raw_smart_filter(settings);
                }
            }
            Purpose::Open { path, preview_only } => {
                let (source, analysis) = (window.source, window.analysis);
                let destination = Destination::Document { path, preview_only };
                self.spawn_development(destination, move |progress| {
                    let frame = Frame::whole(&source);
                    let visual = Visual::Image;
                    camera_raw::render(
                        &source,
                        &settings,
                        &frame,
                        &analysis,
                        visual,
                        Some(progress),
                    )
                });
            }
        }
    }

    /// Last Filter, or an action's step played without its dialog:
    /// `settings` again, on the active layer or as a smart filter.
    pub(super) fn camera_raw_again(&mut self, settings: Settings) {
        if self.refuse_hidden_target() {
            return;
        }
        if self.smart_filters_apply() {
            self.add_camera_raw_smart_filter(settings);
            return;
        }
        let Some(doc) = &self.doc else { return };
        let active = (doc.uid(), doc.active_layer().id);
        self.develop_layer(settings, active);
    }

    fn add_camera_raw_smart_filter(&mut self, settings: Settings) {
        let filter = SmartFilterKind::CameraRaw(Arc::new(settings.clone()));
        self.add_smart_filter(filter, move |app| app.camera_raw_applied(settings));
    }

    /// Camera Raw applied with `settings` (its result in, not cancelled):
    /// Last Filter repeats it, an action being recorded gets it.
    fn camera_raw_applied(&mut self, settings: Settings) {
        self.filters.remember_camera_raw(settings.clone());
        self.record(Recorded::CameraRaw { settings });
    }

    /// `settings` developed at full resolution on the active layer (`active`:
    /// its document and id), inside the selection: one undo step once done.
    fn develop_layer(&mut self, settings: Settings, (uid, layer): (u64, u64)) {
        let Some(doc) = &mut self.doc else { return };
        if (doc.uid(), doc.active_layer().id) != (uid, layer) {
            return;
        }
        let rasterized = doc.active_layer().text.is_some() && !doc.mask_targeted();
        doc.checkpoint(t!("raw-filter-title"));
        if !doc.begin_paint() {
            doc.discard_step();
            self.status = t!("filter-no-pixels");
            return;
        }
        let Some(target) = filters_ui::active_target(doc, self.selection.as_ref()) else {
            doc.end_paint();
            doc.discard_step();
            self.status = t!("status-selection-empty");
            return;
        };
        let destination = Destination::Layer {
            doc: uid,
            layer,
            rasterized,
            settings: Box::new(settings.clone()),
        };
        self.spawn_development(destination, move |progress| {
            target.process(0, |image, _| {
                let source = Source::from_rgba(image);
                let analysis = Analysis::of(&source);
                let frame = Frame::whole(&source);
                let visual = Visual::Image;
                *image = camera_raw::render(
                    &source,
                    &settings,
                    &frame,
                    &analysis,
                    visual,
                    Some(progress),
                );
            })
        });
    }

    /// Runs `work` on another thread, with its progress.
    fn spawn_development(
        &mut self,
        destination: Destination,
        work: impl FnOnce(&AtomicU32) -> RgbaImage + Send + 'static,
    ) {
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let (done, flag) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let _job = crate::diag::job("Camera Raw: develop");
            let image = crate::filters::cancellable(Some(flag), || work(&done));
            let _ = tx.send(image);
            ctx.request_repaint();
        });
        self.camera_raw.developing = Some(Developing {
            destination,
            started: self.ctx.input(|i| i.time),
            progress,
            rx,
            cancel,
        });
    }

    /// Cancel of the development: it stops, the layer stays as it was.
    fn cancel_development(&mut self) {
        let Some(d) = self.camera_raw.developing.take() else {
            return;
        };
        d.cancel.store(true, Ordering::Relaxed);
        let name = match d.destination {
            Destination::Layer {
                doc: uid, layer, ..
            } => {
                if let Some(doc) = &mut self.doc
                    && (doc.uid(), doc.active_layer().id) == (uid, layer)
                {
                    doc.end_paint();
                    doc.discard_step();
                }
                self.shrink_after_preview();
                self.end_layer_preview();
                self.reset_pending();
                self.actions.step_stopped(Stopped::Cancelled);
                t!("raw-filter-title")
            }
            Destination::Document { path, .. } => file_label(&path),
        };
        self.status = t!("retouch-status-cancelled", name = name.as_str());
    }

    /// The development is done: the layer's new pixels, or a new document.
    fn poll_development(&mut self) {
        let Some(developing) = &self.camera_raw.developing else {
            return;
        };
        let result = match developing.rx.try_recv() {
            Ok(image) => Some(image),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => None,
        };
        let Some(d) = self.camera_raw.developing.take() else {
            return;
        };
        match d.destination {
            Destination::Layer {
                doc: uid,
                layer,
                rasterized,
                settings,
            } => {
                let name = t!("raw-filter-title");
                if let Some(doc) = &mut self.doc
                    && (doc.uid(), doc.active_layer().id) == (uid, layer)
                {
                    match result {
                        Some(image) => {
                            doc.active_layer_mut().replace_pixels(image);
                            doc.end_paint();
                            self.status = t!("filter-applied", name = name.as_str());
                            if rasterized {
                                let layer = doc.active_layer().name.as_str();
                                let note = t!("canvas-status-text-rasterized", name = layer);
                                self.status = format!("{} — {note}", self.status);
                            }
                            self.camera_raw_applied(*settings);
                        }
                        None => {
                            doc.end_paint();
                            doc.discard_step();
                            // The name in English too, for the log and the report.
                            let name = crate::i18n::Message::of(|| t!("raw-filter-title"));
                            let text = tm!("fx3-filter-failed", name = name);
                            self.actions
                                .step_stopped(Stopped::Failed(text.shown.clone()));
                            self.status_failed(text);
                            self.shrink_after_preview();
                        }
                    }
                }
                self.end_layer_preview();
                self.reset_pending();
            }
            Destination::Document { path, preview_only } => {
                let Some(image) = result else {
                    let name = file_label(&path);
                    self.report_error(tm!("fx4-raw-open-failed", name = name.as_str()));
                    return;
                };
                let shown = path.display().to_string();
                self.status = if preview_only {
                    let notes = t!("raw-note-preview-only");
                    t!("xchg-status-opened-simplified", path = shown, notes = notes)
                } else {
                    t!("status-opened", path = shown)
                };
                if !self.automate.running() {
                    self.recent.add(&path);
                }
                self.add_tab(Document::new(image, Some(path)));
            }
        }
    }
}

/// The file's name, for titles.
fn file_label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl RawWindow {
    fn is_raw(&self) -> bool {
        self.source.is_raw()
    }

    /// The window's keys (outside a field being typed in): \ before /
    /// after, Y side by side, U and O the clipping warnings, I the White
    /// Balance tool, Ctrl+0 fit, Ctrl+Alt+0 100 %, Ctrl++ / Ctrl+− zoom.
    fn keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let v = &mut self.view;
        ctx.input_mut(|i| {
            let none = Modifiers::NONE;
            if i.consume_key(none, Key::Backslash) {
                v.before = !v.before;
                v.side_by_side = false;
            }
            if i.consume_key(none, Key::Y) {
                v.side_by_side = !v.side_by_side;
            }
            if i.consume_key(none, Key::U) {
                v.clip_shadows = !v.clip_shadows;
            }
            if i.consume_key(none, Key::O) {
                v.clip_highlights = !v.clip_highlights;
            }
            if i.consume_key(none, Key::I) {
                v.sampling = !v.sampling;
            }
            let ctrl_alt = Modifiers::COMMAND.plus(Modifiers::ALT);
            if i.consume_key(ctrl_alt, Key::Num0) {
                v.zoom = Some(1.0);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Num0) {
                v.zoom = None;
            }
            if i.consume_key(Modifiers::COMMAND, Key::Plus)
                || i.consume_key(Modifiers::COMMAND, Key::Equals)
            {
                v.step_zoom(1);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Minus) {
                v.step_zoom(-1);
            }
        });
    }

    /// Starts developing what the panes must show, and takes what was
    /// developed.
    fn update_preview(&mut self, ctx: &egui::Context) {
        for (i, pane) in self.preview.panes.iter_mut().enumerate() {
            let Some((key, rx)) = &pane.job else { continue };
            let key = *key;
            let developed = match rx.try_recv() {
                Ok(developed) => developed,
                Err(mpsc::TryRecvError::Empty) => continue,
                // It panicked: not asked again for the same settings.
                Err(mpsc::TryRecvError::Disconnected) => {
                    pane.job = None;
                    pane.failed = Some(key);
                    self.preview.failure = true;
                    continue;
                }
            };
            pane.job = None;
            let size = [
                developed.image.width() as usize,
                developed.image.height() as usize,
            ];
            let mut shown = developed.image.as_raw().clone();
            color_ui::show_through(self.lut.as_deref(), &mut shown, false);
            let pixels = ColorImage::from_rgba_unmultiplied(size, &shown);
            let texture = ctx.load_texture(format!("camera_raw_{i}"), pixels, IMAGE_TEX_OPTIONS);
            pane.shown = Some(Shown {
                key,
                region: developed.region,
                texture,
                image: Arc::new(developed.image),
                overlay: None,
            });
            self.preview.scaled = Some(developed.scaled);
        }
    }

    /// The histogram: of the whole image developed small with the current
    /// settings, whatever the panes show (zoomed in, Alt held on a
    /// sharpening slider, before).
    fn update_histogram(&mut self, ctx: &egui::Context) {
        let p = &mut self.preview;
        if let Some((key, rx)) = &p.histogram_job {
            match rx.try_recv() {
                Ok(developed) => {
                    p.histogram_key = *key;
                    p.histogram = Some(developed.histogram);
                    p.small = Some(developed.scaled);
                    p.histogram_job = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    p.histogram_failed = Some(*key);
                    p.histogram_job = None;
                    p.failure = true;
                }
            }
        }
        let key = hash_of(format!("{:?}", self.settings));
        if (p.histogram.is_some() && p.histogram_key == key)
            || p.histogram_job.is_some()
            || p.histogram_failed == Some(key)
        {
            return;
        }
        let (source, analysis) = (self.source.clone(), self.analysis.clone());
        let (settings, cached) = (self.settings.clone(), p.small.clone());
        let (w, h) = (source.width, source.height);
        let scale = (HISTOGRAM_SIDE / w.max(h) as f32).min(1.0);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let region = [0, 0, w, h];
            let developed = develop_region(
                &source,
                cached,
                &analysis,
                &settings,
                region,
                scale,
                Visual::Image,
            );
            let _ = tx.send(developed);
            ctx.request_repaint();
        });
        p.histogram_job = Some((key, rx));
    }

    /// Asks pane `i` for `wanted` with `settings`.
    fn request(&mut self, ctx: &egui::Context, i: usize, wanted: &Wanted, settings: &Settings) {
        let visual = if i == 0 {
            self.view.visual
        } else {
            Visual::Image
        };
        let key = hash_of((
            format!("{settings:?}"),
            wanted.region,
            wanted.scale.to_bits(),
            visual,
        ));
        let pane = &mut self.preview.panes[i];
        if pane.shown.as_ref().is_some_and(|s| s.key == key)
            || pane.job.is_some()
            || pane.failed == Some(key)
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (source, analysis) = (self.source.clone(), self.analysis.clone());
        let cached = self.preview.scaled.clone();
        let (settings, region, scale) = (settings.clone(), wanted.region, wanted.scale);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Camera Raw: preview");
            let developed =
                develop_region(&source, cached, &analysis, &settings, region, scale, visual);
            let _ = tx.send(developed);
            ctx.request_repaint();
        });
        pane.job = Some((key, rx));
    }

    fn body(&mut self, ui: &mut egui::Ui, presets: &mut Vec<Preset>, status: &mut Option<String>) {
        let height = dialog_body_height(ui.ctx());
        ui.horizontal_top(|ui| {
            let preview_w = (ui.available_width() - SIDE - 16.0).max(200.0);
            ui.vertical(|ui| {
                ui.set_width(preview_w);
                let size = vec2(preview_w, (height - 44.0).max(100.0));
                self.preview_area(ui, size);
                self.preview_bar(ui);
            });
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.set_width(SIDE);
                self.histogram(ui);
                gap_between_blocks(ui);
                egui::ScrollArea::vertical()
                    .id_salt("camera_raw_panels")
                    .max_height((height - 160.0).max(100.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(SIDE - 12.0);
                        self.panels(ui, presets, status);
                    });
            });
        });
    }

    /// The image (or before and after side by side): drag to move around,
    /// double-click for 100 % / fit, a click with the White Balance tool
    /// makes a point neutral.
    fn preview_area(&mut self, ui: &mut egui::Ui, size: Vec2) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, color::BG0);
        let panes: Vec<(usize, Rect)> = if self.view.side_by_side {
            let half = (rect.width() - 4.0) / 2.0;
            vec![
                (1, Rect::from_min_size(rect.min, vec2(half, rect.height()))),
                (
                    0,
                    Rect::from_min_size(
                        rect.min + vec2(half + 4.0, 0.0),
                        vec2(half, rect.height()),
                    ),
                ),
            ]
        } else {
            vec![(usize::from(self.view.before), rect)]
        };
        let image_size = (self.source.width, self.source.height);
        let wanted = self.view.wanted(panes[0].1.size() * ppp, image_size);
        let after = self.settings.clone();
        let before = self.opened.clone();
        for &(i, _) in &panes {
            let settings = if i == 0 { &after } else { &before };
            self.request(&ctx, i, &wanted, settings);
        }
        self.update_histogram(&ctx);
        for &(i, pane_rect) in &panes {
            let to_screen = |p: Pos2| pane_rect.center() + (p - wanted.center) * wanted.zoom / ppp;
            let painter = ui.painter_at(pane_rect);
            let Some(shown) = &mut self.preview.panes[i].shown else {
                continue;
            };
            let shown_rect =
                Rect::from_min_max(to_screen(shown.region.min), to_screen(shown.region.max));
            draw_checker(&painter, shown_rect);
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            painter.image(shown.texture.id(), shown_rect, uv, Color32::WHITE);
            let flags = (self.view.clip_shadows, self.view.clip_highlights);
            if flags != (false, false) {
                if shown.overlay.as_ref().is_none_or(|o| o.0 != flags) {
                    let overlay = clipping_overlay(&shown.image, flags);
                    let texture = ctx.load_texture(
                        format!("camera_raw_clip_{i}"),
                        overlay,
                        IMAGE_TEX_OPTIONS,
                    );
                    shown.overlay = Some((flags, texture));
                }
                if let Some((_, texture)) = &shown.overlay {
                    painter.image(texture.id(), shown_rect, uv, Color32::WHITE);
                }
            }
            if self.view.side_by_side {
                let label = if i == 0 {
                    t!("raw-after")
                } else {
                    t!("raw-before")
                };
                ui_kit::pill(
                    &painter,
                    pane_rect.left_top() + vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    &label,
                );
            } else if self.view.before {
                ui_kit::pill(
                    &painter,
                    pane_rect.left_top() + vec2(8.0, 8.0),
                    egui::Align2::LEFT_TOP,
                    &t!("raw-before"),
                );
            }
        }
        if self.preview.panes.iter().any(|p| p.job.is_some()) {
            let spinner =
                Rect::from_min_size(rect.right_bottom() - vec2(24.0, 24.0), Vec2::splat(16.0));
            // Painted, not added, so the fields below keep their ids.
            egui::Spinner::new()
                .size(16.0)
                .color(color::MUTED)
                .paint_at(ui, spinner);
        }
        painter.rect_stroke(
            rect,
            4.0,
            UiStroke::new(1.0, color::LINE),
            StrokeKind::Inside,
        );

        // Pointer: values under it, White Balance tool, pan, zoom.
        let pane_at = |p: Pos2| panes.iter().find(|(_, r)| r.contains(p)).copied();
        let to_image =
            |p: Pos2, pane: Rect| wanted.center + (p - pane.center()) * ppp / wanted.zoom;
        self.view.hover = None;
        if let Some(p) = response.hover_pos()
            && let Some((i, pane)) = pane_at(p)
            && let Some(shown) = &self.preview.panes[i].shown
        {
            let q = to_image(p, pane);
            let (iw, ih) = shown.image.dimensions();
            let fx = (q.x - shown.region.min.x) / shown.region.width() * iw as f32;
            let fy = (q.y - shown.region.min.y) / shown.region.height() * ih as f32;
            if fx >= 0.0 && fy >= 0.0 && (fx as u32) < iw && (fy as u32) < ih {
                let px = shown.image.get_pixel(fx as u32, fy as u32).0;
                self.view.hover = Some([px[0], px[1], px[2]]);
            }
            ctx.set_cursor_icon(if self.view.sampling {
                CursorIcon::Crosshair
            } else if response.dragged() {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            });
        }
        if response.clicked()
            && self.view.sampling
            && let Some(p) = response.interact_pointer_pos()
            && let Some((_, pane)) = pane_at(p)
        {
            let q = to_image(p, pane);
            if let Some(rgb) = self.sample_source(q) {
                (self.settings.temperature, self.settings.tint) = self.source.neutral_balance(rgb);
                self.settings.white_balance = WhiteBalance::Custom;
            }
        } else if response.double_clicked()
            && let Some(p) = response.interact_pointer_pos()
            && let Some((_, pane)) = pane_at(p)
        {
            if self.view.zoom.is_some_and(|z| z >= 1.0) {
                self.view.zoom = None;
            } else {
                self.view.center = to_image(p, pane);
                self.view.zoom = Some(1.0);
            }
        }
        if response.dragged() && !self.view.sampling {
            self.view.center -= response.drag_delta() * ppp / wanted.zoom;
        }
        response.on_hover_text(if self.view.sampling {
            t!("raw-wb-tool-tip")
        } else {
            t!("raw-preview-tip")
        });
    }

    /// Source value around an image point (3 × 3 average), before any
    /// setting: what the White Balance tool makes neutral.
    fn sample_source(&self, p: Pos2) -> Option<[f32; 3]> {
        let (w, h) = (self.source.width as i64, self.source.height as i64);
        let (x, y) = (p.x.floor() as i64, p.y.floor() as i64);
        if x < 0 || y < 0 || x >= w || y >= h {
            return None;
        }
        let mut sum = [0.0f32; 3];
        let mut n = 0.0;
        for sy in (y - 1).max(0)..=(y + 1).min(h - 1) {
            for sx in (x - 1).max(0)..=(x + 1).min(w - 1) {
                let v = self.source.rgb[(sy * w + sx) as usize];
                for c in 0..3 {
                    sum[c] += v[c];
                }
                n += 1.0;
            }
        }
        Some(sum.map(|v| v / n))
    }

    /// Under the preview: zoom, before / after.
    fn preview_bar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let v = &mut self.view;
            let zoom_tip = t!("menu-view-zoom-out");
            if ui_kit::icon_button(ui, icon::MINUS, &zoom_tip, true).clicked() {
                v.step_zoom(-1);
            }
            let label = match v.zoom {
                None => t!("raw-zoom-fit"),
                Some(z) => i18n::percent(i18n::short_decimal(f64::from(z) * 100.0)),
            };
            let text = egui::RichText::new(label)
                .font(ui_kit::mono(12.0))
                .color(color::MUTED);
            ui.add_sized([64.0, 28.0], egui::Label::new(text));
            if ui_kit::icon_button(ui, icon::PLUS, &t!("menu-view-zoom-in"), true).clicked() {
                v.step_zoom(1);
            }
            let fit_tip = format!(
                "{} ({})",
                t!("raw-zoom-fit-tip"),
                keys::combo(ui.ctx(), Modifiers::COMMAND, &keys::key(Key::Num0))
            );
            if ui_kit::icon_button(ui, icon::CORNERS_OUT, &fit_tip, v.zoom.is_some()).clicked() {
                v.zoom = None;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut side = v.side_by_side;
                let tip = format!("{} ({})", t!("raw-side-by-side-tip"), keys::key(Key::Y));
                if ui_kit::icon_toggle(ui, icon::COLUMNS, &tip, &mut side).clicked() {
                    v.side_by_side = side;
                }
                let mut before = v.before && !v.side_by_side;
                let tip = format!("{} ({})", t!("raw-before-tip"), keys::key(Key::Backslash));
                if ui_kit::icon_toggle(ui, icon::CIRCLE_HALF, &tip, &mut before).clicked() {
                    v.before = before;
                    v.side_by_side = false;
                }
                let mut sampling = v.sampling;
                let tip = format!("{} ({})", t!("raw-wb-tool"), keys::key(Key::I));
                if ui_kit::icon_toggle(ui, icon::EYEDROPPER, &tip, &mut sampling).clicked() {
                    v.sampling = sampling;
                }
            });
        });
    }

    /// Histogram of the image (channels over each other), clipping warnings
    /// in its top corners (U, O), values under the pointer.
    fn histogram(&mut self, ui: &mut egui::Ui) {
        let (rect, _) = ui.allocate_exact_size(vec2(SIDE, 110.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 6.0, color::BG0);
        if let Some(h) = &self.preview.histogram {
            // Square root: a peak does not flatten the rest.
            let max = h
                .iter()
                .flat_map(|c| c[1..255].iter())
                .copied()
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            let bar = rect.width() / 256.0;
            let tints = [
                Color32::from_rgba_unmultiplied(230, 60, 60, 120),
                Color32::from_rgba_unmultiplied(60, 200, 80, 120),
                Color32::from_rgba_unmultiplied(70, 110, 240, 120),
            ];
            for (c, tint) in tints.iter().enumerate() {
                for (i, &n) in h[c].iter().enumerate() {
                    let height =
                        ((n as f32 / max).sqrt() * (rect.height() - 8.0)).min(rect.height() - 8.0);
                    if height > 0.0 {
                        let x = rect.left() + i as f32 * bar;
                        let r = Rect::from_min_max(
                            pos2(x, rect.bottom() - height),
                            pos2(x + bar.max(1.0), rect.bottom()),
                        );
                        painter.rect_filled(r, 0.0, *tint);
                    }
                }
            }
        }
        painter.rect_stroke(
            rect,
            6.0,
            UiStroke::new(1.0, color::LINE),
            StrokeKind::Inside,
        );
        // Clipping warnings: lit in the colors of the clipped channels.
        let clipped = |end: usize| {
            self.preview
                .histogram
                .as_ref()
                .map_or([false; 3], |h| [0, 1, 2].map(|c| h[c][end] > 0))
        };
        for (left, end) in [(true, 0usize), (false, 255)] {
            let corner = if left {
                rect.left_top() + vec2(6.0, 6.0)
            } else {
                rect.right_top() + vec2(-6.0, 6.0)
            };
            let dir = if left { 1.0 } else { -1.0 };
            let points = vec![
                corner,
                corner + vec2(12.0 * dir, 0.0),
                corner + vec2(0.0, 12.0),
            ];
            let hit = Rect::from_points(&points).expand(3.0);
            let id = ui.id().with(("clip", left));
            let response = ui.interact(hit, id, Sense::click());
            let on = if left {
                &mut self.view.clip_shadows
            } else {
                &mut self.view.clip_highlights
            };
            if response.clicked() {
                *on = !*on;
            }
            let [r, g, b] = clipped(end);
            let fill = if r || g || b {
                Color32::from_rgb(
                    if r { 255 } else { 60 },
                    if g { 255 } else { 60 },
                    if b { 255 } else { 60 },
                )
            } else {
                color::BG3
            };
            let stroke = UiStroke::new(
                1.0,
                if *on {
                    Color32::WHITE
                } else {
                    color::LINE_HOVER
                },
            );
            painter.add(egui::Shape::convex_polygon(points, fill, stroke));
            let (tip, key) = if left {
                (t!("raw-clip-shadows-tip"), Key::U)
            } else {
                (t!("raw-clip-highlights-tip"), Key::O)
            };
            ui_kit::tip_with_key(response, &tip, &keys::key(key));
        }
        // Values under the pointer.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            for (c, name) in ["R", "G", "B"].iter().enumerate() {
                let value = self.view.hover.map_or("—".to_owned(), |p| p[c].to_string());
                let text = egui::RichText::new(format!("{name} {value}"))
                    .font(ui_kit::mono(12.0))
                    .color(color::MUTED);
                ui.label(text);
            }
        });
    }

    fn panels(
        &mut self,
        ui: &mut egui::Ui,
        presets: &mut Vec<Preset>,
        status: &mut Option<String>,
    ) {
        // Profile: Color or Monochrome (above the panels, like Camera Raw).
        block(ui, |ui| {
            ui_kit::field_label(ui, &t!("raw-profile"));
            let (color_label, mono) = (t!("raw-profile-color"), t!("raw-profile-monochrome"));
            full_segmented(
                ui,
                &mut self.settings.profile,
                &[(Profile::Color, &color_label), (Profile::Monochrome, &mono)],
            );
        });
        gap_between_blocks(ui);
        self.view.visual = Visual::Image;
        for panel in PANELS {
            let open = self.open_panels.contains(&panel);
            let changed = panel.differs(&self.settings, &self.defaults);
            let (toggled, reset) = panel_header(ui, &panel.title(), open, changed);
            if toggled {
                if open {
                    self.open_panels.retain(|p| *p != panel);
                } else {
                    self.open_panels.push(panel);
                }
            }
            if reset {
                panel.reset(&mut self.settings, &self.defaults);
                self.curve_point = None;
            }
            if !self.open_panels.contains(&panel) {
                continue;
            }
            ui.add_space(4.0);
            // Panels share slider names (Highlights, Amount…).
            ui.push_id(panel.title(), |ui| match panel {
                Panel::Basic => self.basic(ui),
                Panel::Curve => self.tone_curve(ui),
                Panel::Mixer => self.mixer(ui),
                Panel::Grading => self.grading(ui),
                Panel::Detail => self.detail(ui),
                Panel::Effects => self.effects(ui),
                Panel::Lens => self.lens(ui),
                Panel::Presets => self.presets(ui, presets, status),
            });
            ui.add_space(10.0);
        }
    }

    fn basic(&mut self, ui: &mut egui::Ui) {
        let raw = self.is_raw();
        let d = self.defaults.clone();
        let s = &mut self.settings;
        // White balance: preset, tool, temperature and tint.
        ui.horizontal(|ui| {
            let choices: &[WhiteBalance] = if raw {
                &WhiteBalance::RAW
            } else {
                &WhiteBalance::IMAGE
            };
            let chosen = wb_label(s.white_balance);
            let mut picked = None;
            let width = ui.available_width() - 36.0;
            ui_kit::combo(
                ui,
                "camera_raw_wb",
                Some(&t!("raw-white-balance")),
                &chosen,
                width,
                |ui| {
                    for &wb in choices {
                        if ui
                            .selectable_label(s.white_balance == wb, wb_label(wb))
                            .clicked()
                        {
                            picked = Some(wb);
                        }
                    }
                },
            );
            if let Some(wb) = picked {
                s.white_balance = wb;
                match wb {
                    WhiteBalance::AsShot => (s.temperature, s.tint) = (d.temperature, d.tint),
                    WhiteBalance::Auto => (s.temperature, s.tint) = self.analysis.auto_balance,
                    WhiteBalance::Custom => {}
                    light => {
                        if let Some(values) = light.light() {
                            (s.temperature, s.tint) = values;
                        }
                    }
                }
            }
            let tip = format!("{} ({})", t!("raw-wb-tool"), keys::key(Key::I));
            ui_kit::icon_toggle(ui, icon::EYEDROPPER, &tip, &mut self.view.sampling);
        });
        gap_between_blocks(ui);
        const TEMPERATURE: &[Color32] = &[
            Color32::from_rgb(70, 110, 230),
            Color32::from_rgb(235, 200, 60),
        ];
        const TINT: &[Color32] = &[
            Color32::from_rgb(60, 190, 80),
            Color32::from_rgb(210, 70, 200),
        ];
        let (temperature, tint) = (t!("raw-temperature"), t!("raw-tint"));
        let wb_changed = if raw {
            let kelvins = Steps {
                decimals: 0,
                exponent: 2.5,
            };
            let a = slider(
                ui,
                &temperature,
                &mut s.temperature,
                camera_raw::TEMPERATURE_RANGE,
                d.temperature,
                kelvins,
                Track::Gradient(TEMPERATURE),
                " K",
            );
            let b = slider(
                ui,
                &tint,
                &mut s.tint,
                camera_raw::TINT_RANGE,
                d.tint,
                Steps::WHOLE,
                Track::Gradient(TINT),
                "",
            );
            a || b
        } else {
            let a = slider(
                ui,
                &temperature,
                &mut s.temperature,
                -100.0..=100.0,
                0.0,
                Steps::WHOLE,
                Track::Gradient(TEMPERATURE),
                "",
            );
            let b = slider(
                ui,
                &tint,
                &mut s.tint,
                -100.0..=100.0,
                0.0,
                Steps::WHOLE,
                Track::Gradient(TINT),
                "",
            );
            a || b
        };
        if wb_changed {
            s.white_balance = WhiteBalance::Custom;
        }
        gap_between_blocks(ui);
        let ev = Steps {
            decimals: 2,
            exponent: 1.0,
        };
        slider(
            ui,
            &t!("raw-exposure"),
            &mut s.exposure,
            -5.0..=5.0,
            0.0,
            ev,
            Track::Accent,
            "",
        );
        for (label, value) in [
            (t!("raw-contrast"), &mut s.contrast),
            (t!("raw-highlights"), &mut s.highlights),
            (t!("raw-shadows"), &mut s.shadows),
            (t!("raw-whites"), &mut s.whites),
            (t!("raw-blacks"), &mut s.blacks),
            (t!("raw-texture"), &mut s.texture),
            (t!("raw-clarity"), &mut s.clarity),
            (t!("raw-dehaze"), &mut s.dehaze),
            (t!("raw-vibrance"), &mut s.vibrance),
            (t!("raw-saturation"), &mut s.saturation),
        ] {
            centered(ui, &label, value);
        }
    }

    fn tone_curve(&mut self, ui: &mut egui::Ui) {
        let (parametric, point) = (t!("raw-curve-parametric"), t!("raw-curve-point"));
        full_segmented(
            ui,
            &mut self.point_curve,
            &[(false, &parametric), (true, &point)],
        );
        gap_between_blocks(ui);
        let curve = &mut self.settings.curve;
        let empty = [0u32; 256];
        if self.point_curve {
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("dialog-adjust-channel"));
                let before = self.curve_channel;
                channel_selector(ui, &mut self.curve_channel);
                if before != self.curve_channel {
                    self.curve_point = None;
                }
            });
            gap_between_blocks(ui);
            let c = self.curve_channel.min(3);
            let mut edited =
                crate::pixel_ops::Curve::from_points(&curve.points[c]).unwrap_or_default();
            let hist = self.preview.histogram.as_ref().map(|h| match c {
                // The composite: the channels' average.
                0 => std::array::from_fn(|i| (h[0][i] + h[1][i] + h[2][i]) / 3),
                _ => h[c - 1],
            });
            let tints = [
                color::MUTED,
                Color32::from_rgb(230, 72, 77),
                Color32::from_rgb(61, 184, 107),
                Color32::from_rgb(59, 123, 246),
            ];
            curves_editor(
                ui,
                &mut edited,
                hist.as_ref().unwrap_or(&empty),
                tints[c],
                &mut self.curve_point,
            );
            curve.points[c] = edited.points().to_vec();
        } else {
            // The resulting curve, over the histogram.
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 1.0, color::BG0);
            let grid = UiStroke::new(1.0, color::LINE);
            for k in 1..4 {
                let x = rect.left() + rect.width() * k as f32 / 4.0;
                painter.vline(x, rect.y_range(), grid);
                let y = rect.top() + rect.height() * k as f32 / 4.0;
                painter.hline(rect.x_range(), y, grid);
            }
            let samples = camera_raw::parametric_samples(curve, 65);
            let points: Vec<Pos2> = (samples.iter().enumerate())
                .map(|(i, y)| {
                    let v = i as f32 / 64.0;
                    pos2(
                        rect.left() + v * rect.width(),
                        rect.bottom() - y * rect.height(),
                    )
                })
                .collect();
            painter.add(egui::Shape::line(points, UiStroke::new(1.5, color::TEXT)));
            // Splits of the regions.
            let mut splits = curve.splits.map(|v| v * 2.55);
            if let Some(i) = level_track(ui, "camera_raw_splits", color::MUTED, &mut splits) {
                let lo = if i == 0 {
                    1.0
                } else {
                    splits[i - 1] / 2.55 + 1.0
                };
                let hi = if i == 2 {
                    99.0
                } else {
                    splits[i + 1] / 2.55 - 1.0
                };
                curve.splits[i] = (splits[i] / 2.55).round().clamp(lo, hi);
            }
            gap_between_blocks(ui);
            for (label, value) in [
                (t!("raw-highlights"), &mut curve.highlights),
                (t!("raw-curve-lights"), &mut curve.lights),
                (t!("raw-curve-darks"), &mut curve.darks),
                (t!("raw-shadows"), &mut curve.shadows),
            ] {
                centered(ui, &label, value);
            }
        }
    }

    fn mixer(&mut self, ui: &mut egui::Ui) {
        let monochrome = self.settings.profile == Profile::Monochrome;
        let m = &mut self.settings.mixer;
        let names = [
            t!("raw-hue-red"),
            t!("raw-hue-orange"),
            t!("raw-hue-yellow"),
            t!("raw-hue-green"),
            t!("raw-hue-aqua"),
            t!("raw-hue-blue"),
            t!("raw-hue-purple"),
            t!("raw-hue-magenta"),
        ];
        const CENTERS: [f32; 8] = [0.0, 30.0, 60.0, 120.0, 180.0, 240.0, 270.0, 300.0];
        let hue = |h: f32, s: f32, v: f32| -> Color32 {
            egui::ecolor::Hsva::new(h.rem_euclid(360.0) / 360.0, s, v, 1.0).into()
        };
        if monochrome {
            help(ui, &t!("raw-mixer-gray-hint"));
            gap_between_blocks(ui);
            for (i, name) in names.iter().enumerate() {
                let track = [Color32::from_gray(40), hue(CENTERS[i], 0.8, 0.9)];
                slider(
                    ui,
                    name,
                    &mut m.gray[i],
                    -100.0..=100.0,
                    0.0,
                    Steps::WHOLE,
                    Track::Gradient(&track),
                    "",
                );
            }
            return;
        }
        let (h, s, l) = (
            t!("raw-mixer-hue"),
            t!("raw-mixer-saturation"),
            t!("raw-mixer-luminance"),
        );
        full_segmented(ui, &mut self.mixer_tab, &[(0, &h), (1, &s), (2, &l)]);
        gap_between_blocks(ui);
        for (i, name) in names.iter().enumerate() {
            let c = CENTERS[i];
            let (value, track) = match self.mixer_tab {
                0 => (
                    &mut m.hue[i],
                    [
                        hue(c - 30.0, 0.8, 0.9),
                        hue(c, 0.8, 0.9),
                        hue(c + 30.0, 0.8, 0.9),
                    ],
                ),
                1 => (
                    &mut m.saturation[i],
                    [Color32::from_gray(128), hue(c, 0.5, 0.8), hue(c, 1.0, 0.95)],
                ),
                _ => (
                    &mut m.luminance[i],
                    [
                        Color32::from_gray(20),
                        hue(c, 0.8, 0.8),
                        Color32::from_gray(235),
                    ],
                ),
            };
            slider(
                ui,
                name,
                value,
                -100.0..=100.0,
                0.0,
                Steps::WHOLE,
                Track::Gradient(&track),
                "",
            );
        }
    }

    fn grading(&mut self, ui: &mut egui::Ui) {
        let g = &mut self.settings.grading;
        let names = [
            t!("raw-grading-shadows"),
            t!("raw-grading-midtones"),
            t!("raw-grading-highlights"),
            t!("raw-grading-global"),
        ];
        let wheels = [g.shadows, g.midtones, g.highlights, g.global];
        wheel_tabs(ui, &mut self.grading_tab, &names, &wheels);
        gap_between_blocks(ui);
        let wheel = match self.grading_tab {
            0 => &mut g.shadows,
            1 => &mut g.midtones,
            2 => &mut g.highlights,
            _ => &mut g.global,
        };
        ui.vertical_centered(|ui| color_wheel(ui, wheel, 168.0));
        gap_between_blocks(ui);
        const HUES: &[Color32] = &[
            Color32::from_rgb(255, 0, 0),
            Color32::from_rgb(255, 255, 0),
            Color32::from_rgb(0, 255, 0),
            Color32::from_rgb(0, 255, 255),
            Color32::from_rgb(0, 0, 255),
            Color32::from_rgb(255, 0, 255),
            Color32::from_rgb(255, 0, 0),
        ];
        let tint: Color32 = egui::ecolor::Hsva::new(wheel.hue / 360.0, 0.8, 0.9, 1.0).into();
        slider(
            ui,
            &t!("raw-grading-hue"),
            &mut wheel.hue,
            0.0..=359.0,
            0.0,
            Steps::WHOLE,
            Track::Gradient(HUES),
            "°",
        );
        slider(
            ui,
            &t!("raw-grading-saturation"),
            &mut wheel.saturation,
            0.0..=100.0,
            0.0,
            Steps::WHOLE,
            Track::Gradient(&[Color32::from_gray(128), tint]),
            "",
        );
        centered(ui, &t!("raw-grading-luminance"), &mut wheel.luminance);
        gap_between_blocks(ui);
        level(ui, &t!("raw-grading-blending"), &mut g.blending, 50.0);
        centered(ui, &t!("raw-grading-balance"), &mut g.balance);
    }

    fn detail(&mut self, ui: &mut egui::Ui) {
        let alt = ui.input(|i| i.modifiers.alt);
        let d = &mut self.settings.detail;
        let defaults = &self.defaults.detail;
        ui_kit::section(ui, &t!("raw-sharpening"));
        ui.add_space(4.0);
        let one = Steps {
            decimals: 1,
            exponent: 1.0,
        };
        let mut visual = None;
        for (label, value, range, default, steps, shows) in [
            (
                t!("raw-sharpen-amount"),
                &mut d.sharpen_amount,
                0.0..=150.0,
                defaults.sharpen_amount,
                Steps::WHOLE,
                Visual::SharpenAmount,
            ),
            (
                t!("raw-sharpen-radius"),
                &mut d.sharpen_radius,
                0.5..=3.0,
                defaults.sharpen_radius,
                one,
                Visual::SharpenRadius,
            ),
            (
                t!("raw-sharpen-detail"),
                &mut d.sharpen_detail,
                0.0..=100.0,
                defaults.sharpen_detail,
                Steps::WHOLE,
                Visual::SharpenDetail,
            ),
            (
                t!("raw-sharpen-masking"),
                &mut d.sharpen_masking,
                0.0..=100.0,
                defaults.sharpen_masking,
                Steps::WHOLE,
                Visual::SharpenMasking,
            ),
        ] {
            let rect = ui
                .scope(|ui| slider(ui, &label, value, range, default, steps, Track::Accent, ""))
                .response
                .rect;
            // Alt while dragging: what the slider does, in gray.
            let held = ui.input(|i| {
                i.pointer.primary_down()
                    && i.pointer.press_origin().is_some_and(|p| rect.contains(p))
            });
            if alt && held {
                visual = Some(shows);
            }
        }
        if let Some(v) = visual {
            self.view.visual = v;
        }
        let alt = keys::modifiers(ui.ctx(), Modifiers::ALT);
        help(ui, &t!("input3-raw-sharpen-hint", alt = alt));
        gap_between_blocks(ui);
        ui_kit::section(ui, &t!("raw-noise-reduction"));
        ui.add_space(4.0);
        for (label, value, default) in [
            (t!("raw-nr-luminance"), &mut d.luminance, defaults.luminance),
            (
                t!("raw-nr-luminance-detail"),
                &mut d.luminance_detail,
                defaults.luminance_detail,
            ),
            (t!("raw-nr-color"), &mut d.color, defaults.color),
            (
                t!("raw-nr-color-detail"),
                &mut d.color_detail,
                defaults.color_detail,
            ),
        ] {
            level(ui, &label, value, default);
        }
    }

    fn effects(&mut self, ui: &mut egui::Ui) {
        let e = &mut self.settings.effects;
        ui_kit::section(ui, &t!("raw-vignetting"));
        ui.add_space(4.0);
        let style_label = |s: VignetteStyle| match s {
            VignetteStyle::HighlightPriority => t!("raw-vignette-highlight-priority"),
            VignetteStyle::ColorPriority => t!("raw-vignette-color-priority"),
            VignetteStyle::PaintOverlay => t!("raw-vignette-paint-overlay"),
        };
        let width = ui.available_width();
        let chosen = style_label(e.vignette_style);
        ui_kit::combo(
            ui,
            "camera_raw_vignette",
            Some(&t!("raw-vignette-style")),
            &chosen,
            width,
            |ui| {
                for s in [
                    VignetteStyle::HighlightPriority,
                    VignetteStyle::ColorPriority,
                    VignetteStyle::PaintOverlay,
                ] {
                    ui.selectable_value(&mut e.vignette_style, s, style_label(s));
                }
            },
        );
        gap_between_blocks(ui);
        centered(ui, &t!("raw-amount"), &mut e.vignette);
        level(ui, &t!("raw-midpoint"), &mut e.midpoint, 50.0);
        centered(ui, &t!("raw-roundness"), &mut e.roundness);
        level(ui, &t!("raw-feather"), &mut e.feather, 50.0);
        if e.vignette_style != VignetteStyle::PaintOverlay {
            level(ui, &t!("raw-highlights"), &mut e.vignette_highlights, 0.0);
        }
        gap_between_blocks(ui);
        ui_kit::section(ui, &t!("raw-grain"));
        ui.add_space(4.0);
        ui.push_id("grain", |ui| {
            level(ui, &t!("raw-grain-amount"), &mut e.grain, 0.0);
            level(ui, &t!("raw-grain-size"), &mut e.grain_size, 25.0);
            level(ui, &t!("raw-grain-roughness"), &mut e.grain_roughness, 50.0);
        });
    }

    fn lens(&mut self, ui: &mut egui::Ui) {
        let l = &mut self.settings.lens;
        centered(ui, &t!("raw-lens-distortion"), &mut l.distortion);
        gap_between_blocks(ui);
        ui_kit::section(ui, &t!("raw-lens-vignetting"));
        ui.add_space(4.0);
        centered(ui, &t!("raw-amount"), &mut l.vignetting);
        level(ui, &t!("raw-midpoint"), &mut l.midpoint, 50.0);
    }

    /// Built-in and user presets: a click applies one (the white balance
    /// stays); the current settings can be saved as one.
    fn presets(
        &mut self,
        ui: &mut egui::Ui,
        presets: &mut Vec<Preset>,
        status: &mut Option<String>,
    ) {
        let mut apply = None;
        for preset in BUILT_IN_PRESETS {
            if preset_row(ui, &builtin_label(preset), false).0 {
                apply = Some(preset.settings(&self.settings));
            }
        }
        let mut delete = None;
        for (i, preset) in presets.iter().enumerate() {
            let (clicked, deleted) = preset_row(ui, &preset.name, true);
            if clicked {
                apply = Some(self.settings.with_look(&preset.settings));
            }
            if deleted {
                delete = Some(i);
            }
        }
        if let Some(s) = apply {
            self.settings = s;
            self.curve_point = None;
        }
        let mut save = false;
        if let Some(i) = delete {
            presets.remove(i);
            save = true;
        }
        ui.add_space(6.0);
        match &mut self.preset_name {
            None => {
                let label = format!("{}…", t!("raw-preset-save"));
                if ui_kit::button_with_icon(
                    ui,
                    Some(icon::FLOPPY_DISK),
                    &label,
                    Kind::Normal,
                    None,
                    true,
                )
                .clicked()
                {
                    self.preset_name = Some(String::new());
                }
            }
            Some(name) => {
                let id = egui::Id::new("camera_raw_preset_name");
                let out = ui_kit::text_field(ui, &t!("raw-preset-name"), name, id);
                if out.response.gained_focus() || name.is_empty() {
                    out.response.request_focus();
                }
                let typed = name.trim().to_owned();
                let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let mut close =
                    out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape));
                ui.horizontal(|ui| {
                    let ok = ui_kit::button(
                        ui,
                        &t!("raw-preset-save"),
                        Kind::Primary,
                        None,
                        !typed.is_empty(),
                    )
                    .clicked();
                    if (ok || enter) && !typed.is_empty() {
                        presets.retain(|p| p.name != typed);
                        presets.push(Preset {
                            name: typed.clone(),
                            settings: self.settings.clone(),
                        });
                        save = true;
                        close = true;
                    }
                    if ui_kit::button(ui, &t!("dialog-cancel"), Kind::Normal, None, true).clicked()
                    {
                        close = true;
                    }
                });
                if close {
                    self.preset_name = None;
                }
            }
        }
        if save && let Err(e) = camera_raw::save_presets(presets) {
            *status = Some(t!("raw-preset-save-failed", error = e.to_string()));
        }
    }
}

impl View {
    fn step_zoom(&mut self, dir: i32) {
        let z = self.zoom.unwrap_or(self.fit);
        self.zoom = Some(if dir > 0 {
            ZOOMS
                .iter()
                .copied()
                .find(|&l| l > z * 1.01)
                .unwrap_or(ZOOMS[ZOOMS.len() - 1])
        } else {
            ZOOMS
                .iter()
                .rev()
                .copied()
                .find(|&l| l < z * 0.99)
                .unwrap_or(ZOOMS[0])
        });
    }
}

/// Panel header: caret, title, reset (when changed). Returns whether it
/// was clicked (to fold or unfold) and whether reset was.
fn panel_header(ui: &mut egui::Ui, title: &str, open: bool, changed: bool) -> (bool, bool) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 6.0, color::BG3);
    }
    painter.hline(rect.x_range(), rect.top(), UiStroke::new(1.0, color::LINE));
    let caret = if open {
        icon::CARET_DOWN
    } else {
        icon::CARET_RIGHT
    };
    painter.text(
        rect.left_center() + vec2(10.0, 0.0),
        egui::Align2::CENTER_CENTER,
        caret,
        egui::FontId::proportional(13.0),
        color::MUTED,
    );
    painter.text(
        rect.left_center() + vec2(24.0, 0.0),
        egui::Align2::LEFT_CENTER,
        title,
        ui_kit::bold(ui_kit::TEXT_SIZE),
        color::TEXT,
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(2.0, 2.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    let reset = changed
        && ui_kit::icon_button(
            &mut child,
            icon::ARROW_COUNTER_CLOCKWISE,
            &t!("raw-panel-reset-tip"),
            true,
        )
        .clicked();
    (response.clicked() && !reset, reset)
}

/// A preset in the list: its name, a delete button for the user's.
fn preset_row(ui: &mut egui::Ui, name: &str, deletable: bool) -> (bool, bool) {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, color::BG3);
    }
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(ui_kit::TEXT_SIZE),
        color::TEXT,
    );
    let mut deleted = false;
    if deletable {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        deleted =
            ui_kit::icon_button(&mut child, icon::TRASH, &t!("raw-preset-delete"), true).clicked();
    }
    (response.clicked() && !deleted, deleted)
}

/// Slider in Camera Raw's units; a double-click (on the label or the
/// track) resets it.
#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    steps: Steps,
    track: Track,
    unit: &str,
) -> bool {
    slider_row_steps(ui, label, None, value, range, unit, track, default, steps)
}

/// Slider from 0 to 100.
fn level(ui: &mut egui::Ui, label: &str, value: &mut f32, default: f32) -> bool {
    slider(
        ui,
        label,
        value,
        0.0..=100.0,
        default,
        Steps::WHOLE,
        Track::Accent,
        "",
    )
}

/// Slider from −100 to +100, neutral at 0.
fn centered(ui: &mut egui::Ui, label: &str, value: &mut f32) -> bool {
    slider(
        ui,
        label,
        value,
        -100.0..=100.0,
        0.0,
        Steps::WHOLE,
        Track::Accent,
        "",
    )
}

fn wb_label(wb: WhiteBalance) -> String {
    match wb {
        WhiteBalance::AsShot => t!("raw-wb-as-shot"),
        WhiteBalance::Auto => t!("raw-wb-auto"),
        WhiteBalance::Daylight => t!("raw-wb-daylight"),
        WhiteBalance::Cloudy => t!("raw-wb-cloudy"),
        WhiteBalance::Shade => t!("raw-wb-shade"),
        WhiteBalance::Tungsten => t!("raw-wb-tungsten"),
        WhiteBalance::Fluorescent => t!("raw-wb-fluorescent"),
        WhiteBalance::Flash => t!("raw-wb-flash"),
        WhiteBalance::Custom => t!("raw-wb-custom"),
    }
}

fn builtin_label(preset: BuiltIn) -> String {
    match preset {
        BuiltIn::Vivid => t!("raw-preset-vivid"),
        BuiltIn::Matte => t!("raw-preset-matte"),
        BuiltIn::BlackWhite => t!("raw-preset-black-white"),
        BuiltIn::WarmFilm => t!("raw-preset-warm-film"),
    }
}

/// Clipped pixels: blue where black, red where white (all channels).
fn clipping_overlay(image: &RgbaImage, (shadows, highlights): (bool, bool)) -> ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image
        .pixels()
        .map(|p| {
            let [r, g, b, _] = p.0;
            if highlights && r == 255 && g == 255 && b == 255 {
                Color32::from_rgb(255, 0, 0)
            } else if shadows && r == 0 && g == 0 && b == 0 {
                Color32::from_rgb(0, 80, 255)
            } else {
                Color32::TRANSPARENT
            }
        })
        .collect();
    ColorImage::new(size, pixels)
}

/// The wheels to choose from: each one's color, its name below.
fn wheel_tabs(
    ui: &mut egui::Ui,
    tab: &mut usize,
    names: &[String; 4],
    wheels: &[camera_raw::Wheel; 4],
) {
    let width = ui.available_width();
    let (rect, group) = ui.allocate_exact_size(vec2(width, 52.0), Sense::hover());
    let cell = width / 4.0;
    for (i, (name, wheel)) in names.iter().zip(wheels).enumerate() {
        let r = Rect::from_min_size(rect.min + vec2(i as f32 * cell, 0.0), vec2(cell, 52.0));
        let response = ui.interact(r, group.id.with(i), Sense::click());
        let selected = *tab == i;
        if selected || response.hovered() {
            let fill = if selected {
                color::BG3
            } else {
                color::BUTTON_HOVER
            };
            ui.painter().rect_filled(r.shrink(1.0), 6.0, fill);
        }
        let center = r.center_top() + vec2(0.0, 16.0);
        let tint: Color32 =
            egui::ecolor::Hsva::new(wheel.hue / 360.0, wheel.saturation / 100.0, 0.85, 1.0).into();
        ui.painter().circle_filled(center, 9.0, tint);
        let ring = if selected {
            color::TEXT
        } else {
            color::LINE_HOVER
        };
        ui.painter()
            .circle_stroke(center, 9.0, UiStroke::new(1.0, ring));
        let fg = if selected { color::TEXT } else { color::MUTED };
        ui.painter().text(
            r.center_bottom() - vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            name,
            egui::FontId::proportional(11.0),
            fg,
        );
        if response.clicked() {
            *tab = i;
        }
    }
}

/// A color wheel: the point's angle is the hue, its distance from the
/// center the saturation. Drag it; double-click to reset.
fn color_wheel(ui: &mut egui::Ui, wheel: &mut camera_raw::Wheel, size: f32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let center = rect.center();
    let radius = size / 2.0 - 6.0;
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(center, Color32::from_gray(110));
    const STEPS: u32 = 72;
    for i in 0..=STEPS {
        let a = i as f32 / STEPS as f32 * std::f32::consts::TAU;
        let c: Color32 = egui::ecolor::Hsva::new(i as f32 / STEPS as f32, 0.75, 0.85, 1.0).into();
        mesh.colored_vertex(center + vec2(a.cos(), -a.sin()) * radius, c);
        if i > 0 {
            mesh.add_triangle(0, i, i + 1);
        }
    }
    ui.painter().add(mesh);
    ui.painter()
        .circle_stroke(center, radius, UiStroke::new(1.0, color::LINE_HOVER));
    if response.double_clicked() {
        wheel.hue = 0.0;
        wheel.saturation = 0.0;
    } else if let Some(p) = response.interact_pointer_pos()
        && (response.dragged() || response.clicked())
    {
        let d = p - center;
        wheel.hue = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0).round();
        wheel.saturation = (d.length() / radius * 100.0).clamp(0.0, 100.0).round();
    }
    let a = wheel.hue.to_radians();
    let point = center + vec2(a.cos(), -a.sin()) * radius * wheel.saturation / 100.0;
    ui.painter().circle_filled(point, 6.0, Color32::WHITE);
    ui.painter().circle_stroke(
        point,
        6.0,
        UiStroke::new(1.5, Color32::from_black_alpha(160)),
    );
    response.on_hover_text(t!("raw-wheel-tip"));
}

#[cfg(test)]
#[path = "camera_raw_tests.rs"]
mod tests;
