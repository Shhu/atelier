//! Filter menu: the filter dialogs (settings, preview square, live preview
//! on the canvas), Last Filter, and the filters computed off the UI thread.

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use image::RgbaImage;

use super::dialogs::{
    DialogSpec, Steps, Track, block, floating_dialog, full_segmented, gap_between_blocks,
    slider_row_steps, tool_dialog_pos,
};
use super::*;
use crate::filters::{
    Filter, FilterKind, Morphology, Remove, SmartSharpen, Target, ToneFade, Undefined,
};
use crate::i18n::t;
use ui_kit::color;

/// Last Filter: Alt+Ctrl+F, like Photoshop.
pub(super) const SHORTCUT_LAST_FILTER: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::F);

/// Submenus of the Filter menu, in Photoshop's order.
const GROUPS: [(Group, &[FilterKind]); 7] = [
    (
        Group::Blur,
        &[
            FilterKind::BoxBlur,
            FilterKind::GaussianBlur,
            FilterKind::MotionBlur,
            FilterKind::SurfaceBlur,
        ],
    ),
    (
        Group::Noise,
        &[
            FilterKind::AddNoise,
            FilterKind::DustAndScratches,
            FilterKind::Median,
            FilterKind::ReduceNoise,
        ],
    ),
    (Group::Pixelate, &[FilterKind::Mosaic]),
    (Group::Render, &[FilterKind::Clouds]),
    (
        Group::Sharpen,
        &[FilterKind::SmartSharpen, FilterKind::UnsharpMask],
    ),
    (Group::Stylize, &[FilterKind::Emboss, FilterKind::FindEdges]),
    (
        Group::Other,
        &[
            FilterKind::HighPass,
            FilterKind::Maximum,
            FilterKind::Minimum,
            FilterKind::Offset,
        ],
    ),
];

#[derive(Clone, Copy)]
enum Group {
    Blur,
    Noise,
    Pixelate,
    Render,
    Sharpen,
    Stylize,
    Other,
}

impl Group {
    fn label(self) -> String {
        match self {
            Group::Blur => t!("filter-group-blur"),
            Group::Noise => t!("filter-group-noise"),
            Group::Pixelate => t!("filter-group-pixelate"),
            Group::Render => t!("filter-group-render"),
            Group::Sharpen => t!("filter-group-sharpen"),
            Group::Stylize => t!("filter-group-stylize"),
            Group::Other => t!("filter-group-other"),
        }
    }
}

/// The filter whose `Debug` name is `name` (what Diagnostics measured).
pub(super) fn kind_named(name: &str) -> Option<FilterKind> {
    GROUPS
        .iter()
        .flat_map(|(_, kinds)| kinds.iter().copied())
        .find(|k| format!("{k:?}") == name)
}

/// Name of a filter (its dialog's title).
pub(super) fn title(kind: FilterKind) -> String {
    match kind {
        FilterKind::BoxBlur => t!("filter-box-blur"),
        FilterKind::GaussianBlur => t!("filter-gaussian-blur"),
        FilterKind::MotionBlur => t!("filter-motion-blur"),
        FilterKind::SurfaceBlur => t!("filter-surface-blur"),
        FilterKind::AddNoise => t!("filter-add-noise"),
        FilterKind::DustAndScratches => t!("filter-dust-and-scratches"),
        FilterKind::Median => t!("filter-median"),
        FilterKind::ReduceNoise => t!("filter-reduce-noise"),
        FilterKind::Mosaic => t!("filter-mosaic"),
        FilterKind::Clouds => t!("filter-clouds"),
        FilterKind::SmartSharpen => t!("filter-smart-sharpen"),
        FilterKind::UnsharpMask => t!("filter-unsharp-mask"),
        FilterKind::Emboss => t!("filter-emboss"),
        FilterKind::FindEdges => t!("filter-find-edges"),
        FilterKind::HighPass => t!("filter-high-pass"),
        FilterKind::Maximum => t!("filter-maximum"),
        FilterKind::Minimum => t!("filter-minimum"),
        FilterKind::Offset => t!("filter-offset"),
    }
}

/// Menu label: the name, followed by "…" when a dialog opens.
fn menu_label(kind: FilterKind) -> String {
    if kind.has_settings() {
        format!("{}…", title(kind))
    } else {
        title(kind)
    }
}

/// Filters of the session: settings, dialog, filter being applied.
#[derive(Default)]
pub(super) struct Filters {
    /// Settings last applied of each filter: its dialog opens with them.
    settings: HashMap<FilterKind, Filter>,
    /// Last filter applied (Last Filter).
    last: Option<Filter>,
    /// The Camera Raw Filter's settings, when it was applied after `last`.
    camera_raw: Option<crate::camera_raw::Settings>,
    dialog: Option<FilterDialog>,
    applying: Option<Applying>,
    /// The layer a dialog (a filter's, Liquify, Camera Raw) enlarged for
    /// its preview, as it was.
    enlarged: Option<Enlarged>,
    /// Preview of an adjustment (or Make Transparent) being computed: what
    /// for (the layer's id and revision), and the computation.
    adjust: Option<(LayerPreview, u64, u64, Job<RgbaImage>)>,
    /// The preview whose computation failed (for that layer and revision):
    /// not started again.
    adjust_failed: Option<(LayerPreview, u64, u64)>,
}

/// A layer enlarged to the canvas for a dialog's preview: its document and
/// id, its revision once enlarged, and its pixels, offset and revision
/// before.
struct Enlarged {
    doc: u64,
    layer: u64,
    rev: u64,
    before: (Arc<RgbaImage>, (i64, i64), u64),
}

impl Filters {
    /// A dialog or a filter being applied keeps the keyboard and the image.
    pub(super) fn busy(&self) -> bool {
        self.dialog.is_some() || self.applying.is_some()
    }

    /// The settings a filter's dialog opens with (an action's step).
    pub(super) fn remember(&mut self, filter: Filter) {
        self.settings.insert(filter.kind(), filter);
    }

    /// Liquify was applied after the last filter: Last Filter repeats it.
    pub(super) fn forget_last(&mut self) {
        self.last = None;
        self.camera_raw = None;
    }

    /// The Camera Raw Filter was applied: Last Filter repeats it.
    pub(super) fn remember_camera_raw(&mut self, settings: crate::camera_raw::Settings) {
        self.last = None;
        self.camera_raw = Some(settings);
    }
}

/// Filter dialog open.
struct FilterDialog {
    filter: Filter,
    /// Settings it opened with (Reset).
    opened: Filter,
    /// Result shown on the canvas.
    preview: bool,
    /// Preview square: zoom (1 = 100 %) and center, in layer coordinates.
    zoom: f32,
    center: Pos2,
    square: Square,
    canvas: Option<Job<RgbaImage>>,
    /// Key of a canvas preview whose computation failed (not tried again).
    failed: Option<u64>,
    /// Smart Sharpen: Shadows and Highlights settings shown.
    tones: bool,
}

/// What the preview square shows.
#[derive(Default)]
struct Square {
    /// Filtered and original pixels, for a key, and the region they show.
    shown: Option<(u64, [u32; 4], egui::TextureHandle, egui::TextureHandle)>,
    job: Option<Job<(ColorImage, ColorImage)>>,
}

/// Filter being applied to the layer.
struct Applying {
    filter: Filter,
    /// Document and layer it applies to.
    doc: u64,
    layer: u64,
    /// The layer was an editable text.
    rasterized: bool,
    started: f64,
    job: Job<RgbaImage>,
}

/// A filter's result the dialog's preview already has, or is computing.
enum Ready {
    Image(Arc<RgbaImage>),
    Job(Job<RgbaImage>),
}

/// Computation on another thread, and what it was started for. Dropping
/// it stops the filters it applies (a preview no longer wanted, a filter
/// cancelled).
pub(super) struct Job<T> {
    key: u64,
    region: [u32; 4],
    rx: mpsc::Receiver<T>,
    cancel: Arc<AtomicBool>,
}

impl<T: Send + 'static> Job<T> {
    /// `name`: what it is measured as (see `crate::diag`), if anything.
    pub(super) fn spawn(
        ctx: &egui::Context,
        key: u64,
        region: [u32; 4],
        name: Option<String>,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let _job = name.map(crate::diag::job);
            let _ = tx.send(crate::filters::cancellable(Some(flag), work));
            ctx.request_repaint();
        });
        Self {
            key,
            region,
            rx,
            cancel,
        }
    }

    /// Stops the filters it applies: what it gives is then meaningless.
    pub(super) fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// What it was started for.
    pub(super) fn key(&self) -> u64 {
        self.key
    }

    pub(super) fn stopped(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// What the computation gave: `Err(())` once it failed (its thread
    /// panicked), `Ok(None)` while it runs.
    pub(super) fn poll(&self) -> Result<Option<T>, ()> {
        match self.rx.try_recv() {
            Ok(out) => Ok(Some(out)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(()),
        }
    }
}

impl<T> Drop for Job<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Side of the preview square (the dialog's body width).
const SQUARE: f32 = 288.0;
/// Zoom levels of the preview square.
const SQUARE_ZOOMS: [f32; 14] = [
    0.0625, 0.125, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0,
];
/// Delay (s) before the busy indicator of a filter being applied shows.
const BUSY_DELAY: f64 = 0.3;

/// What a filter works on: the active layer's pixels, or its mask when
/// targeted (see [`Document::target_layer`]), and the area of the
/// selection. `None` without either, or if the selection does not touch it.
pub(super) fn active_target(doc: &Document, selection: Option<&Selection>) -> Option<Target> {
    let layer = doc.target_layer()?;
    let clip = match selection {
        Some(selection) => {
            let clip = selection.clip(&layer, doc.full_rect())?;
            let [x0, y0, x1, y1] = clip.rect;
            if x0 >= x1 || y0 >= y1 {
                return None;
            }
            Some(clip)
        }
        None => None,
    };
    Some(Target {
        image: layer.shared_image(),
        clip,
        offset: layer.offset,
    })
}

/// A dialog's result is previewed over the whole layer: a layer smaller
/// than the canvas is enlarged first, as it will be (its pixels don't
/// change; a text, a shape or a targeted mask is left as it is). Returns
/// what to put back if the dialog is cancelled.
fn enlarge_for_preview(doc: &mut Document) -> Option<Enlarged> {
    if doc.mask_targeted() || doc.active_layer().is_live() {
        return None;
    }
    let layer = doc.active_layer();
    let before = (layer.shared_image(), layer.offset, layer.rev());
    if !doc.expand_active_to_canvas() {
        return None;
    }
    let layer = doc.active_layer();
    Some(Enlarged {
        doc: doc.uid(),
        layer: layer.id,
        rev: layer.rev(),
        before,
    })
}

/// Seed of a new noise or clouds pattern.
fn fresh_seed() -> u32 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    hash_of(now.as_nanos()) as u32
}

impl AtelierApp {
    /// A filter was applied this session: Filter › Last Filter can repeat it.
    pub(super) fn has_last_filter(&self) -> bool {
        self.filters.last.is_some()
            || self.filters.camera_raw.is_some()
            || self.liquify.last.is_some()
    }

    /// The Filter menu (between Select and View, like Photoshop).
    pub(super) fn filter_menu(&self) -> (String, Vec<MenuEntry>) {
        use MenuEntry::{Item, Separator, Submenu};
        let has_doc = self.doc.is_some();
        let mut entries = vec![
            Item(
                t!("filter-last"),
                has_doc && self.has_last_filter(),
                Some(&SHORTCUT_LAST_FILTER),
                Action::LastFilter,
            ),
            Separator,
            Item(
                t!("smart-menu-convert-for-filters"),
                has_doc,
                None,
                Action::Smart(smart_ui::Command::ConvertForFilters),
            ),
            Separator,
            Item(
                format!("{}…", t!("raw-filter-title")),
                has_doc,
                Some(&camera_raw_ui::SHORTCUT_CAMERA_RAW),
                Action::CameraRawFilter,
            ),
            Item(
                format!("{}…", t!("liquify-title")),
                has_doc,
                Some(&liquify_ui::SHORTCUT_LIQUIFY),
                Action::Liquify,
            ),
            Separator,
        ];
        for (group, kinds) in GROUPS {
            let items = kinds
                .iter()
                .map(|&k| Item(menu_label(k), has_doc, None, Action::Filter(k)))
                .collect();
            entries.push(Submenu(group.label(), items));
        }
        (t!("filter-menu"), entries)
    }

    /// Filter menu item: its dialog, or the filter at once if it has no
    /// settings.
    pub(super) fn open_filter(&mut self, kind: FilterKind) {
        if !kind.has_settings() {
            let filter = self.filter_ready(kind.initial());
            self.start_filter(filter);
            return;
        }
        let filter = self
            .filters
            .settings
            .get(&kind)
            .copied()
            .unwrap_or_else(|| kind.initial());
        let filter = self.filter_ready(filter);
        self.open_filter_dialog(filter);
    }

    /// The dialog of `filter`, opening with its settings.
    pub(super) fn open_filter_dialog(&mut self, filter: Filter) {
        if self.refuse_hidden_target() {
            return;
        }
        let selection = self.selection.clone();
        // On a smart object: a smart filter.
        let smart = self.smart_filters_apply();
        let Some(doc) = &mut self.doc else { return };
        if !doc.can_paint() && !smart {
            self.status = t!("filter-no-pixels");
            return;
        }
        // The result is previewed over the whole layer: a layer smaller
        // than the canvas is enlarged first, as it will be (its pixels
        // don't change; a text or a shape stays editable until the filter
        // applies).
        let enlarged = enlarge_for_preview(doc);
        self.filters.enlarged = enlarged;
        let Some(doc) = &mut self.doc else { return };
        // The square shows the middle of the selection, or of the canvas.
        let canvas = doc.full_rect();
        let around = selection.filter(|s| !s.inverted).map_or(canvas, |s| s.rect);
        let Some(target) = doc.target_layer() else {
            return;
        };
        let center = pos2(
            (around.x - target.offset.0) as f32 + around.w as f32 / 2.0,
            (around.y - target.offset.1) as f32 + around.h as f32 / 2.0,
        );
        self.filters.dialog = Some(FilterDialog {
            filter,
            opened: filter,
            preview: true,
            zoom: 1.0,
            center,
            square: Square::default(),
            canvas: None,
            failed: None,
            tones: false,
        });
    }

    /// Last Filter (Alt+Ctrl+F): the last filter again, with its settings
    /// (Camera Raw's too; Liquify's: its mesh).
    pub(super) fn last_filter(&mut self) {
        match self.filters.last {
            Some(filter) => {
                let filter = self.filter_ready(filter);
                self.start_filter(filter);
            }
            None => match self.filters.camera_raw.clone() {
                Some(settings) => self.camera_raw_again(settings),
                None => self.liquify_again(),
            },
        }
    }

    /// A new pattern for noise and clouds, the clouds in the current colors.
    pub(super) fn filter_ready(&self, filter: Filter) -> Filter {
        match filter {
            Filter::AddNoise(n) => Filter::AddNoise(crate::filters::AddNoise {
                seed: fresh_seed(),
                ..n
            }),
            Filter::Clouds(_) => {
                let rgb = |c: Color32| {
                    let [r, g, b, _] = c.to_srgba_unmultiplied();
                    [r, g, b]
                };
                Filter::Clouds(crate::filters::Clouds {
                    foreground: rgb(self.primary),
                    background: rgb(self.secondary),
                    seed: fresh_seed(),
                })
            }
            f => f,
        }
    }

    /// Applies `filter` to the active layer (or its targeted mask), inside
    /// the selection, on another thread: one undo step once done.
    pub(super) fn start_filter(&mut self, filter: Filter) {
        self.start_filter_with(filter, None);
    }

    /// [`Self::start_filter`], the result being `ready` already: the
    /// dialog's canvas preview of the same settings, computed or being
    /// computed for the same pixels.
    fn start_filter_with(&mut self, filter: Filter, ready: Option<Ready>) {
        if self.refuse_hidden_target() {
            return;
        }
        if self.smart_filters_apply() {
            self.add_smart_filter(filter);
            self.filters.last = Some(filter);
            self.filters.settings.insert(filter.kind(), filter);
            self.record(crate::actions::Recorded::Filter { filter });
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let rasterized = doc.active_layer().text.is_some() && !doc.mask_targeted();
        let before = doc.target_layer().map(|l| l.image().dimensions());
        doc.checkpoint(title(filter.kind()));
        // The layer grows to the canvas, or its targeted mask stands in for
        // it, until the result is in (`poll_filter`).
        if !doc.begin_paint() {
            doc.discard_step();
            self.status = t!("filter-no-pixels");
            return;
        }
        let Some(target) = active_target(doc, self.selection.as_ref()) else {
            doc.end_paint();
            doc.discard_step();
            self.status = t!("status-selection-empty");
            return;
        };
        // A preview of other dimensions (a text, enlarged only now) would
        // not fit the layer any more.
        if self
            .preview_shown
            .as_ref()
            .is_some_and(|p| p.image.dimensions() != target.image.dimensions())
        {
            self.preview_shown = None;
        }
        // The preview is the result only if the target kept its size.
        let ready = ready.filter(|_| before == Some(target.image.dimensions()));
        let job = match ready {
            Some(Ready::Job(job)) => job,
            Some(Ready::Image(image)) => {
                // A job that gives at once what the preview holds.
                Job::spawn(&self.ctx, 0, [0; 4], None, move || (*image).clone())
            }
            None => {
                let reach = filter.reach().unwrap_or(0);
                let name = format!("Filter: {:?}", filter.kind());
                Job::spawn(&self.ctx, 0, [0; 4], Some(name), move || {
                    target.process(reach, |image, origin| filter.apply(image, origin))
                })
            }
        };
        self.filters.applying = Some(Applying {
            filter,
            doc: doc.uid(),
            layer: doc.active_layer().id,
            rasterized,
            started: self.ctx.input(|i| i.time),
            job,
        });
        self.record(crate::actions::Recorded::Filter { filter });
    }

    /// The filter being applied is done: its pixels replace the layer's.
    fn poll_filter(&mut self) {
        let Some(applying) = &self.filters.applying else {
            return;
        };
        let result = match applying.job.poll() {
            Ok(Some(image)) => Some(image),
            Ok(None) => return,
            Err(()) => None,
        };
        let Some(a) = self.filters.applying.take() else {
            return;
        };
        let name = title(a.filter.kind());
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.active_layer().id) == (a.doc, a.layer)
        {
            match result {
                Some(image) => {
                    doc.active_layer_mut().replace_pixels(image);
                    doc.end_paint();
                    self.status = t!("filter-applied", name = name.as_str());
                    if a.rasterized {
                        let layer = doc.active_layer().name.as_str();
                        let note = t!("canvas-status-text-rasterized", name = layer);
                        self.status = format!("{} — {note}", self.status);
                    }
                    self.filters.last = Some(a.filter);
                    self.filters.settings.insert(a.filter.kind(), a.filter);
                }
                // The computation failed: nothing changes.
                None => {
                    doc.end_paint();
                    doc.discard_step();
                    self.status_failed(tm!("fx3-filter-failed", name = name.as_str()));
                    self.shrink_after_preview();
                }
            }
        }
        self.end_layer_preview();
        self.reset_pending();
    }

    /// The preview of an adjustment dialog (or Make Transparent) on the
    /// canvas, computed on another thread: the last one stays shown until
    /// the next is ready, one computation at a time.
    pub(super) fn adjust_preview(&mut self, preview: LayerPreview) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let (id, rev) = (layer.id, layer.rev());
        if let Some((done, layer, rev_then, job)) = &self.filters.adjust {
            match job.poll() {
                Ok(None) => return,
                Ok(Some(image)) => {
                    let done = done.clone();
                    let current = (*layer, *rev_then) == (id, rev);
                    self.filters.adjust = None;
                    if current {
                        self.show_preview(done, image);
                    }
                }
                Err(()) => {
                    if let Some((failed, layer, rev, _)) = self.filters.adjust.take() {
                        let text = match &failed {
                            LayerPreview::Adjust(a) => {
                                let name = a.title();
                                t!("fx4-preview-failed", name = name.as_str())
                            }
                            LayerPreview::Transparency(_) => {
                                let name = t!("dialog-transparency-title");
                                t!("fx4-preview-failed", name = name.as_str())
                            }
                            LayerPreview::Filter(_) => t!("fx4-preview-failed-any"),
                        };
                        self.filters.adjust_failed = Some((failed, layer, rev));
                        self.status_failed(text);
                    }
                    return self.end_layer_preview();
                }
            }
        }
        if let Some(p) = &self.preview_shown
            && (&p.preview, p.layer, p.rev) == (&preview, id, rev)
        {
            return;
        }
        if let Some((failed, layer, rev_then)) = &self.filters.adjust_failed
            && (failed, *layer, *rev_then) == (&preview, id, rev)
        {
            return;
        }
        let Some(doc) = &self.doc else { return };
        // Like the result: only inside the selection.
        let Some(target) = active_target(doc, self.selection.as_ref()) else {
            return self.end_layer_preview();
        };
        let wanted = preview.clone();
        let name = match &preview {
            LayerPreview::Adjust(a) => format!("Adjustment preview: {:?}", a.kind()),
            LayerPreview::Transparency(_) => "Make Transparent preview".to_owned(),
            LayerPreview::Filter(_) => "Layer preview".to_owned(),
        };
        let job = Job::spawn(&self.ctx, 0, [0; 4], Some(name), move || {
            target.process(0, |image, _| wanted.apply(image))
        });
        self.filters.adjust = Some((preview, id, rev, job));
    }

    /// The adjustment dialog closed: its preview being computed is dropped.
    pub(super) fn forget_adjust_preview(&mut self) {
        self.filters.adjust = None;
    }

    /// A hidden layer is not filtered (its result could not be seen): says
    /// so. Returns `true` then.
    pub(super) fn refuse_hidden_target(&mut self) -> bool {
        self.refuse_hidden(true)
    }

    /// The same for the other commands that change the active layer's
    /// pixels (Fill, adjustments, Make Transparent, Content-Aware Fill).
    pub(super) fn refuse_hidden_layer(&mut self) -> bool {
        self.refuse_hidden(false)
    }

    fn refuse_hidden(&mut self, filter: bool) -> bool {
        match self.hidden_target(filter) {
            Some(text) => {
                self.status = text;
                true
            }
            None => false,
        }
    }

    /// Why the active layer can't be changed: it is hidden (or in a hidden
    /// group). `filter`: in a filter's words. In Quick Mask Mode, the mask
    /// is changed instead, always shown.
    pub(super) fn hidden_target(&self, filter: bool) -> Option<String> {
        let doc = self.doc.as_ref()?;
        if doc.shown(doc.active) || doc.quick_mask().is_some() {
            return None;
        }
        let name = doc.active_layer().name.as_str();
        Some(if filter {
            t!("fx3-filter-layer-hidden", name = name)
        } else {
            t!("fx4-layer-hidden", name = name)
        })
    }

    /// A dialog closed without changing the layer: the layer it enlarged
    /// for its preview is put back as it was, if unchanged since.
    pub(super) fn shrink_after_preview(&mut self) {
        let Some(e) = self.filters.enlarged.take() else {
            return;
        };
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.active_layer().id, doc.active_layer().rev())
                == (e.doc, e.layer, e.rev)
        {
            let (image, offset, rev) = e.before;
            doc.active_layer_mut().put_back_pixels(image, offset, rev);
        }
    }

    /// See [`enlarge_for_preview`]: the layer enlarged for the dialog
    /// opening, to put back if it is cancelled.
    pub(super) fn enlarge_for_dialog(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        self.filters.enlarged = enlarge_for_preview(doc);
    }

    /// Cancel of the filter being applied (its computation stops): the
    /// layer stays as it was.
    fn cancel_filter(&mut self) {
        let Some(a) = self.filters.applying.take() else {
            return;
        };
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.active_layer().id) == (a.doc, a.layer)
        {
            doc.end_paint();
            doc.discard_step();
        }
        // A layer the dialog enlarged is put back as it was.
        self.shrink_after_preview();
        let name = title(a.filter.kind());
        self.status = t!("retouch-status-cancelled", name = name.as_str());
        self.end_layer_preview();
        self.reset_pending();
    }

    /// The filter dialog, or the indicator of a filter being applied.
    pub(super) fn filter_window(&mut self, ctx: &egui::Context) {
        self.poll_filter();
        if let Some(a) = &self.filters.applying {
            let name = title(a.filter.kind());
            if cancellable_indicator(ctx, "filter_busy", &name, a.started, None) {
                self.cancel_filter();
            }
            return;
        }
        let Some(mut dialog) = self.filters.dialog.take() else {
            return;
        };
        // A smart filter filters what the ones below it give.
        let smart = self.smart_filter_input();
        let Some(doc) = &self.doc else { return };
        let target = smart.or_else(|| active_target(doc, self.selection.as_ref()));
        let layer = doc.active_layer();
        let (layer_id, layer_rev, size) = (layer.id, layer.rev(), layer.image().dimensions());
        let fingerprint = hash_of(format!("{:?}", dialog.filter));
        if let Some(target) = &target {
            self.update_square(ctx, &mut dialog, target, (layer_id, layer_rev, fingerprint));
            let key = hash_of((layer_id, layer_rev, fingerprint));
            self.update_canvas_preview(ctx, &mut dialog, target, key);
        }

        let kind = dialog.filter.kind();
        let mut preview = dialog.preview;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                reset: Some(dialog.filter != dialog.opened),
                preview: Some(&mut preview),
                ok_enabled: target.is_some(),
                note: target.is_none().then(|| t!("status-selection-empty")),
                ..DialogSpec::new("filter", title(kind), tool_dialog_pos(ctx))
            },
            |ui| {
                preview_square(ui, &mut dialog);
                gap_between_blocks(ui);
                settings(ui, &mut dialog.filter, &mut dialog.tones, size);
            },
        );
        if preview != dialog.preview {
            dialog.preview = preview;
            if !preview {
                self.end_layer_preview();
            }
        }
        if out.reset {
            dialog.filter = dialog.opened;
        }
        if out.ok {
            // The preview stays on the canvas until the result replaces it;
            // computed (or being computed) with these settings, it is the
            // result.
            let key = hash_of((layer_id, layer_rev, fingerprint));
            let shown = self.preview_shown.as_ref().filter(|p| {
                matches!(p.preview, LayerPreview::Filter(k) if k == key) && p.mask.is_none()
            });
            let ready = if self.smart_filters_apply() {
                None
            } else if let Some(p) = shown {
                Some(Ready::Image(p.image.clone()))
            } else {
                dialog
                    .canvas
                    .take()
                    .filter(|job| job.key == key && !job.stopped())
                    .map(Ready::Job)
            };
            self.start_filter_with(dialog.filter, ready);
        } else if out.cancel {
            self.end_layer_preview();
            self.shrink_after_preview();
        } else {
            self.filters.dialog = Some(dialog);
        }
    }

    /// Starts computing what the preview square must show, and shows what
    /// was computed.
    fn update_square(
        &mut self,
        ctx: &egui::Context,
        dialog: &mut FilterDialog,
        target: &Target,
        key: (u64, u64, u64),
    ) {
        let ppp = ctx.pixels_per_point();
        let (w, h) = target.image.dimensions();
        dialog.center = pos2(
            dialog.center.x.clamp(0.0, w as f32),
            dialog.center.y.clamp(0.0, h as f32),
        );
        let extent = SQUARE * ppp / dialog.zoom;
        let (x0, y0) = (
            (dialog.center.x - extent / 2.0).floor().max(0.0) as u32,
            (dialog.center.y - extent / 2.0).floor().max(0.0) as u32,
        );
        let region = [
            x0.min(w),
            y0.min(h),
            ((x0 as f32 + extent).ceil() as u32 + 1).min(w),
            ((y0 as f32 + extent).ceil() as u32 + 1).min(h),
        ];
        let zoom = dialog.zoom;
        // Shown as the canvas shows the document (its profile, the monitor's).
        let (lut_key, lut) = self.color.shown();
        let wanted = hash_of((key, region, zoom.to_bits(), lut_key));
        let square = &mut dialog.square;
        let polled = square.job.as_ref().map(Job::poll);
        if let Some(Err(())) = polled {
            // Failed: shown as it was, a new computation for new settings.
            square.job = None;
        }
        if let Some(Ok(Some((filtered, original)))) = polled
            && let Some(job) = square.job.take()
        {
            let (key, region) = (job.key, job.region);
            let load =
                |name: &str, image: ColorImage| ctx.load_texture(name, image, IMAGE_TEX_OPTIONS);
            square.shown = Some((
                key,
                region,
                load("filter-square", filtered),
                load("filter-square-original", original),
            ));
        }
        let shown = square.shown.as_ref().map(|s| s.0);
        if shown != Some(wanted) && square.job.is_none() {
            let (target, filter) = (target.clone(), dialog.filter);
            let name = format!("Filter dialog preview: {:?}", filter.kind());
            square.job = Some(Job::spawn(ctx, wanted, region, Some(name), move || {
                let mut original = image::imageops::crop_imm(
                    &*target.image,
                    region[0],
                    region[1],
                    region[2] - region[0],
                    region[3] - region[1],
                )
                .to_image();
                let mut filtered =
                    target.process_region(region, filter.reach(), |image, origin| {
                        filter.apply(image, origin)
                    });
                for image in [&mut filtered, &mut original] {
                    color_ui::show_through(lut.as_deref(), image.as_mut(), false);
                }
                (square_image(&filtered, zoom), square_image(&original, zoom))
            }));
        }
    }

    /// Starts computing the canvas preview, and shows it when computed.
    fn update_canvas_preview(
        &mut self,
        ctx: &egui::Context,
        dialog: &mut FilterDialog,
        target: &Target,
        key: u64,
    ) {
        if self.smart_filters_apply() {
            return self.smart_filter_preview(ctx, dialog.filter, dialog.preview);
        }
        match dialog.canvas.as_ref().map(Job::poll) {
            Some(Ok(Some(image))) => {
                if let Some(job) = dialog.canvas.take()
                    && dialog.preview
                    && !job.stopped()
                {
                    self.show_preview(LayerPreview::Filter(job.key), image);
                }
            }
            Some(Err(())) => {
                if let Some(job) = dialog.canvas.take()
                    && !job.stopped()
                {
                    dialog.failed = Some(job.key);
                    let name = title(dialog.filter.kind());
                    self.status_failed(tm!("fx3-filter-failed", name = name.as_str()));
                }
            }
            _ => {}
        }
        let shown = matches!(
            self.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(k)) if *k == key
        );
        // A computation for other settings is stopped; the newest ones
        // start once it is over (one computation at a time).
        if let Some(job) = &dialog.canvas
            && (job.key != key || !dialog.preview)
        {
            job.stop();
        }
        if dialog.preview && !shown && dialog.canvas.is_none() && dialog.failed != Some(key) {
            let (target, filter) = (target.clone(), dialog.filter);
            let reach = filter.reach().unwrap_or(0);
            let name = format!("Filter canvas preview: {:?}", filter.kind());
            dialog.canvas = Some(Job::spawn(ctx, key, [0; 4], Some(name), move || {
                target.process(reach, |image, origin| filter.apply(image, origin))
            }));
        }
    }
}

/// Pixels for the preview square: reduced below 100 %.
fn square_image(image: &RgbaImage, zoom: f32) -> ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    if zoom >= 1.0 || image.width() == 0 || image.height() == 0 {
        return ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    }
    let (w, h) = (
        ((image.width() as f32 * zoom).round() as u32).max(1),
        ((image.height() as f32 * zoom).round() as u32).max(1),
    );
    // Premultiplied: the hidden colors of transparent pixels stay hidden.
    let mut premultiplied = image.clone();
    for p in premultiplied.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 127) / 255) as u8;
        }
    }
    let small = image::imageops::resize(&premultiplied, w, h, FilterType::Triangle);
    ColorImage::from_rgba_premultiplied([w as usize, h as usize], small.as_raw())
}

/// The preview square: the result over a checkerboard, dragged to move
/// around, the original shown while pressed; − and + zoom below it.
fn preview_square(ui: &mut egui::Ui, dialog: &mut FilterDialog) {
    let ppp = ui.ctx().pixels_per_point();
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(SQUARE), Sense::drag());
    if response.dragged() {
        dialog.center -= response.drag_delta() * ppp / dialog.zoom;
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, color::BG0);
    let computing = dialog.square.job.is_some();
    if let Some((_, region, filtered, original)) = &dialog.square.shown {
        let scale = dialog.zoom / ppp;
        let to_screen = |x: u32, y: u32| {
            rect.center() + (vec2(x as f32, y as f32) - dialog.center.to_vec2()) * scale
        };
        let shown = Rect::from_min_max(
            to_screen(region[0], region[1]),
            to_screen(region[2], region[3]),
        );
        draw_checker(&painter, shown);
        let texture = if response.is_pointer_button_down_on() {
            original
        } else {
            filtered
        };
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(texture.id(), shown, uv, Color32::WHITE);
    }
    painter.rect_stroke(
        rect,
        0.0,
        UiStroke::new(1.0, color::LINE),
        StrokeKind::Inside,
    );
    if computing {
        let spinner =
            Rect::from_min_size(rect.right_bottom() - vec2(22.0, 22.0), Vec2::splat(14.0));
        ui.put(spinner, egui::Spinner::new().size(14.0).color(color::MUTED));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(if response.dragged() {
            CursorIcon::Grabbing
        } else {
            CursorIcon::Grab
        });
    }
    response.on_hover_text(t!("filter-preview-tip"));

    // Zoom: −, percentage, +.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let label = i18n::percent(i18n::short_decimal(f64::from(dialog.zoom) * 100.0));
        let width = ui.available_width();
        ui.add_space((width - 2.0 * 28.0 - 64.0 - 8.0).max(0.0) / 2.0);
        let i = SQUARE_ZOOMS
            .iter()
            .position(|&z| z >= dialog.zoom)
            .unwrap_or(6);
        if ui_kit::icon_button(ui, icon::MINUS, &t!("menu-view-zoom-out"), i > 0).clicked() {
            dialog.zoom = SQUARE_ZOOMS[i - 1];
        }
        let text = egui::RichText::new(label)
            .font(ui_kit::mono(12.0))
            .color(color::MUTED);
        ui.add_sized([64.0, 28.0], egui::Label::new(text));
        let last = SQUARE_ZOOMS.len() - 1;
        if ui_kit::icon_button(ui, icon::PLUS, &t!("menu-view-zoom-in"), i < last).clicked() {
            dialog.zoom = SQUARE_ZOOMS[i + 1];
        }
    });
}

/// The settings of a filter, in Photoshop's units. `size`: the layer's, for
/// the range of Offset.
fn settings(ui: &mut egui::Ui, filter: &mut Filter, tones: &mut bool, size: (u32, u32)) {
    let initial = filter.kind().initial();
    let px = " px";
    let percent = i18n::percent("");
    let levels = format!(" {}", t!("filter-unit-levels"));
    let radius = t!("filter-radius");
    let amount = t!("filter-amount");
    let threshold = t!("filter-threshold");
    let angle = t!("filter-angle");
    match (filter, initial) {
        (Filter::GaussianBlur { radius: r }, Filter::GaussianBlur { radius: d })
        | (Filter::HighPass { radius: r }, Filter::HighPass { radius: d }) => {
            fine(ui, &radius, r, 0.1..=1000.0, px, d);
        }
        (Filter::BoxBlur { radius: r }, Filter::BoxBlur { radius: d }) => {
            wide(ui, &radius, r, 1.0..=2000.0, px, d);
        }
        (
            Filter::MotionBlur { angle: a, distance },
            Filter::MotionBlur {
                angle: da,
                distance: dd,
            },
        ) => {
            whole(ui, &angle, a, -90.0..=90.0, "°", da);
            gap_between_blocks(ui);
            wide(ui, &t!("filter-distance"), distance, 1.0..=2000.0, px, dd);
        }
        (
            Filter::SurfaceBlur {
                radius: r,
                threshold: t,
            },
            Filter::SurfaceBlur {
                radius: dr,
                threshold: dt,
            },
        ) => {
            whole(ui, &radius, r, 1.0..=100.0, px, dr);
            gap_between_blocks(ui);
            whole(ui, &threshold, t, 2.0..=255.0, &levels, dt);
        }
        (
            Filter::UnsharpMask {
                amount: a,
                radius: r,
                threshold: t,
            },
            Filter::UnsharpMask {
                amount: da,
                radius: dr,
                threshold: dt,
            },
        ) => {
            let gain = t!("filter-sharpen-amount");
            whole(ui, &gain, a, 1.0..=500.0, &percent, da);
            gap_between_blocks(ui);
            fine(ui, &radius, r, 0.1..=1000.0, px, dr);
            gap_between_blocks(ui);
            whole(ui, &threshold, t, 0.0..=255.0, &levels, dt);
        }
        (Filter::SmartSharpen(s), Filter::SmartSharpen(d)) => {
            smart_sharpen(ui, s, &d, tones);
        }
        (Filter::AddNoise(n), Filter::AddNoise(d)) => {
            let row = Steps {
                decimals: 1,
                exponent: 2.0,
            };
            slider(
                ui,
                &amount,
                &mut n.amount,
                0.1..=400.0,
                &percent,
                d.amount,
                row,
            );
            gap_between_blocks(ui);
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("filter-distribution"));
                let (uniform, gaussian) = (t!("filter-uniform"), t!("filter-gaussian"));
                full_segmented(ui, &mut n.gaussian, &[(false, &uniform), (true, &gaussian)]);
            });
            gap_between_blocks(ui);
            ui_kit::checkbox(ui, &mut n.monochromatic, &t!("filter-monochromatic"));
        }
        (Filter::ReduceNoise(n), Filter::ReduceNoise(d)) => {
            whole(
                ui,
                &t!("filter-strength"),
                &mut n.strength,
                0.0..=10.0,
                "",
                d.strength,
            );
            for (label, value, default) in [
                (
                    t!("filter-preserve-details"),
                    &mut n.preserve_details,
                    d.preserve_details,
                ),
                (t!("filter-color-noise"), &mut n.color_noise, d.color_noise),
                (
                    t!("filter-sharpen-details"),
                    &mut n.sharpen_details,
                    d.sharpen_details,
                ),
            ] {
                gap_between_blocks(ui);
                whole(ui, &label, value, 0.0..=100.0, &percent, default);
            }
        }
        (Filter::Median { radius: r }, Filter::Median { radius: d }) => {
            wide(ui, &radius, r, 1.0..=500.0, px, d);
        }
        (
            Filter::DustAndScratches {
                radius: r,
                threshold: t,
            },
            Filter::DustAndScratches {
                radius: dr,
                threshold: dt,
            },
        ) => {
            wide(ui, &radius, r, 1.0..=500.0, px, dr);
            gap_between_blocks(ui);
            whole(ui, &threshold, t, 0.0..=255.0, &levels, dt);
        }
        (Filter::Offset(o), _) => {
            let (w, h) = (size.0 as f32, size.1 as f32);
            let right = t!("filter-offset-horizontal");
            whole(ui, &right, &mut o.horizontal, -w..=w, px, 0.0);
            gap_between_blocks(ui);
            let down = t!("filter-offset-vertical");
            whole(ui, &down, &mut o.vertical, -h..=h, px, 0.0);
            gap_between_blocks(ui);
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("filter-undefined-areas"));
                let label = |u: Undefined| match u {
                    Undefined::Transparent => t!("filter-set-transparent"),
                    Undefined::RepeatEdges => t!("filter-repeat-edges"),
                    Undefined::WrapAround => t!("filter-wrap-around"),
                };
                let width = ui.available_width();
                let chosen = label(o.undefined);
                ui_kit::combo(ui, "filter_undefined", None, &chosen, width, |ui| {
                    for u in [
                        Undefined::Transparent,
                        Undefined::RepeatEdges,
                        Undefined::WrapAround,
                    ] {
                        ui.selectable_value(&mut o.undefined, u, label(u));
                    }
                });
            });
        }
        (Filter::Minimum(m), _) | (Filter::Maximum(m), _) => {
            let Morphology { radius: d, .. } = match initial {
                Filter::Minimum(m) | Filter::Maximum(m) => m,
                _ => unreachable!("same kind"),
            };
            wide(ui, &radius, &mut m.radius, 1.0..=500.0, px, d);
            gap_between_blocks(ui);
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("filter-preserve"));
                let (square, round) = (t!("filter-squareness"), t!("filter-roundness"));
                full_segmented(ui, &mut m.roundness, &[(false, &square), (true, &round)]);
            });
        }
        (
            Filter::Emboss {
                angle: a,
                height,
                amount: k,
            },
            Filter::Emboss {
                angle: da,
                height: dh,
                amount: dk,
            },
        ) => {
            whole(ui, &angle, a, -180.0..=180.0, "°", da);
            gap_between_blocks(ui);
            whole(ui, &t!("filter-height"), height, 1.0..=100.0, px, dh);
            gap_between_blocks(ui);
            whole(ui, &amount, k, 1.0..=500.0, &percent, dk);
        }
        (Filter::Mosaic { cell }, Filter::Mosaic { cell: d }) => {
            let square = format!(" {}", t!("filter-unit-square"));
            wide(ui, &t!("filter-cell-size"), cell, 2.0..=200.0, &square, d);
        }
        // Without settings, or the settings of another kind (never).
        _ => {}
    }
}

/// Smart Sharpen: amount, radius, noise, blur to remove; shadows and
/// highlights fading on demand.
fn smart_sharpen(ui: &mut egui::Ui, s: &mut SmartSharpen, d: &SmartSharpen, tones: &mut bool) {
    let percent = i18n::percent("");
    let gain = t!("filter-sharpen-amount");
    whole(ui, &gain, &mut s.amount, 1.0..=500.0, &percent, d.amount);
    gap_between_blocks(ui);
    let steps = Steps {
        decimals: 1,
        exponent: 2.0,
    };
    let radius = t!("filter-radius");
    slider(
        ui,
        &radius,
        &mut s.radius,
        0.1..=64.0,
        " px",
        d.radius,
        steps,
    );
    gap_between_blocks(ui);
    let noise = t!("filter-reduce-noise-amount");
    whole(
        ui,
        &noise,
        &mut s.reduce_noise,
        0.0..=100.0,
        &percent,
        d.reduce_noise,
    );
    gap_between_blocks(ui);
    block(ui, |ui| {
        ui_kit::field_label(ui, &t!("filter-remove"));
        let label = |r: Remove| match r {
            Remove::Gaussian => t!("filter-gaussian-blur"),
            Remove::Lens => t!("filter-lens-blur"),
            Remove::Motion => t!("filter-motion-blur"),
        };
        let width = ui.available_width();
        let chosen = label(s.remove);
        ui_kit::combo(ui, "filter_remove", None, &chosen, width, |ui| {
            for r in [Remove::Gaussian, Remove::Lens, Remove::Motion] {
                ui.selectable_value(&mut s.remove, r, label(r));
            }
        });
    });
    if s.remove == Remove::Motion {
        gap_between_blocks(ui);
        whole(
            ui,
            &t!("filter-angle"),
            &mut s.angle,
            -180.0..=180.0,
            "°",
            d.angle,
        );
    }
    gap_between_blocks(ui);
    ui_kit::checkbox(ui, tones, &t!("filter-shadows-highlights"));
    if *tones {
        for (title, fade, default) in [
            (t!("filter-shadows"), &mut s.shadows, d.shadows),
            (t!("filter-highlights"), &mut s.highlights, d.highlights),
        ] {
            gap_between_blocks(ui);
            ui_kit::section(ui, &title);
            tone_fade(ui, fade, &default);
        }
    }
}

fn tone_fade(ui: &mut egui::Ui, f: &mut ToneFade, d: &ToneFade) {
    let percent = i18n::percent("");
    let fade = t!("filter-fade-amount");
    whole(ui, &fade, &mut f.fade, 0.0..=100.0, &percent, d.fade);
    gap_between_blocks(ui);
    let width = t!("filter-tonal-width");
    whole(
        ui,
        &width,
        &mut f.tonal_width,
        0.0..=100.0,
        &percent,
        d.tonal_width,
    );
    gap_between_blocks(ui);
    let radius = t!("filter-radius");
    whole(ui, &radius, &mut f.radius, 1.0..=100.0, " px", d.radius);
}

/// Slider of whole values.
fn whole(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    default: f32,
) {
    slider(ui, label, value, range, unit, default, Steps::WHOLE);
}

/// Slider of whole values over a wide range (the small ones get more room).
fn wide(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    default: f32,
) {
    let steps = Steps {
        decimals: 0,
        exponent: 2.5,
    };
    slider(ui, label, value, range, unit, default, steps);
}

/// Slider of tenths over a wide range (radii from 0.1 to 1000 px).
fn fine(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    default: f32,
) {
    let steps = Steps {
        decimals: 1,
        exponent: 3.0,
    };
    slider(ui, label, value, range, unit, default, steps);
}

fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    unit: &str,
    default: f32,
    steps: Steps,
) {
    slider_row_steps(
        ui,
        label,
        None,
        value,
        range,
        unit,
        Track::Accent,
        default,
        steps,
    );
}

/// While a filter is being applied: blocks the window; after a moment,
/// says which filter, with a spinner.
pub(super) fn busy_indicator(ctx: &egui::Context, name: &str, started: f64) {
    let elapsed = ctx.input(|i| i.time) - started;
    let visible = elapsed >= BUSY_DELAY;
    if !visible {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(BUSY_DELAY - elapsed));
    }
    let frame = if visible {
        egui::Frame::new()
            .fill(color::BG2)
            .stroke(UiStroke::new(1.0, color::WINDOW_LINE))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(18, 14))
    } else {
        egui::Frame::new()
    };
    egui::Modal::new(egui::Id::new("filter_busy"))
        .backdrop_color(Color32::TRANSPARENT)
        .frame(frame)
        .show(ctx, |ui| {
            if visible {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.add(egui::Spinner::new().size(16.0).color(color::MUTED));
                    ui.label(egui::RichText::new(format!("{name}…")).color(color::TEXT));
                });
            }
        });
}

/// Indicator of a long computation that can be stopped, shown after a
/// moment: its progress (a spinner without one) and Cancel. True when
/// cancelled, with the button or Esc.
pub(super) fn cancellable_indicator(
    ctx: &egui::Context,
    id: &str,
    name: &str,
    started: f64,
    progress: Option<f32>,
) -> bool {
    let elapsed = ctx.input(|i| i.time) - started;
    let visible = elapsed >= BUSY_DELAY;
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let frame = if visible {
        egui::Frame::new()
            .fill(color::BG2)
            .stroke(UiStroke::new(1.0, color::WINDOW_LINE))
            .corner_radius(12)
            .inner_margin(egui::Margin::symmetric(18, 14))
    } else {
        egui::Frame::new()
    };
    let mut cancel = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    egui::Modal::new(egui::Id::new(id))
        .backdrop_color(Color32::TRANSPARENT)
        .frame(frame)
        .show(ctx, |ui| {
            if !visible {
                return;
            }
            ui.set_width(280.0);
            ui.label(egui::RichText::new(format!("{name}…")).color(color::TEXT));
            ui.add_space(8.0);
            match progress {
                Some(fraction) => {
                    ui.add(egui::ProgressBar::new(fraction).desired_height(6.0));
                }
                None => {
                    ui.add(egui::Spinner::new().size(16.0).color(color::MUTED));
                }
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel |=
                    ui_kit::button(ui, &t!("dialog-cancel"), Kind::Normal, None, true).clicked();
            });
        });
    cancel
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    /// A 64 × 64 document: left half black, right half white.
    fn app() -> Harness<'static, AtelierApp> {
        let mut harness = Harness::builder()
            .with_size([1200.0, 900.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                let image = image::RgbaImage::from_fn(64, 64, |x, _| {
                    if x < 32 {
                        image::Rgba([0, 0, 0, 255])
                    } else {
                        image::Rgba([255, 255, 255, 255])
                    }
                });
                app.doc = Some(Document::new(image, None));
                app
            });
        harness.run_steps(2);
        harness
    }

    fn pixels(h: &Harness<'static, AtelierApp>) -> RgbaImage {
        h.state()
            .doc
            .as_ref()
            .unwrap()
            .active_layer()
            .image()
            .clone()
    }

    /// Runs frames until the background work is done.
    fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
        for _ in 0..500 {
            h.run_steps(1);
            if done(h.state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("background work not done");
    }

    fn applied(app: &AtelierApp) -> bool {
        app.filters.applying.is_none()
    }

    #[test]
    fn filter_menu_sits_between_select_and_view() {
        let h = app();
        let menus = h.state().menus();
        let titles: Vec<&str> = menus.iter().map(|(t, _)| t.as_str()).collect();
        let at = |key: String| titles.iter().position(|t| *t == key).expect("menu");
        let filter = at(t!("filter-menu"));
        assert_eq!(filter, at(t!("menu-select")) + 1);
        assert_eq!(filter + 1, at(t!("menu-view")));
        // Last Filter: nothing to repeat yet.
        let (_, entries) = &menus[filter];
        assert!(matches!(
            entries[0],
            MenuEntry::Item(_, false, Some(_), Action::LastFilter)
        ));
        // Every filter, once.
        let mut kinds: Vec<FilterKind> =
            GROUPS.iter().flat_map(|(_, k)| k.iter().copied()).collect();
        let n = kinds.len();
        kinds.dedup();
        assert_eq!(kinds.len(), n);
        assert_eq!(n, 18);
    }

    #[test]
    fn dialog_previews_then_applies_in_one_step_and_last_filter_repeats() {
        let mut h = app();
        let original = pixels(&h);
        h.state_mut().run(Action::Filter(FilterKind::GaussianBlur));
        assert!(h.state().modal_open(), "the dialog keeps the keyboard");
        // The canvas preview, then the square, are computed in the background.
        settle(&mut h, |app| {
            matches!(
                app.preview_shown.as_ref().map(|p| &p.preview),
                Some(LayerPreview::Filter(_))
            ) && app
                .filters
                .dialog
                .as_ref()
                .is_some_and(|d| d.square.shown.is_some())
        });
        assert_eq!(pixels(&h), original, "only previewed");
        let steps = h.state().doc.as_ref().unwrap().history_step();
        h.key_press(egui::Key::Enter);
        settle(&mut h, applied);
        let blurred = pixels(&h);
        assert!(
            blurred.get_pixel(31, 10).0[0] > 40,
            "blurred across the edge"
        );
        assert!(h.state().preview_shown.is_none());
        assert!(h.state().filters.dialog.is_none());
        let doc = h.state().doc.as_ref().unwrap();
        assert_eq!(doc.history_step(), steps + 1, "one undo step");
        // Alt+Ctrl+F: again, without a dialog.
        h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::ALT), egui::Key::F);
        h.run_steps(1);
        settle(&mut h, applied);
        assert!(h.state().filters.dialog.is_none());
        let twice = pixels(&h);
        assert_ne!(twice, blurred);
        h.state_mut().undo();
        assert_eq!(pixels(&h), blurred);
        h.state_mut().undo();
        assert_eq!(pixels(&h), original);
    }

    #[test]
    fn cancel_leaves_the_layer_and_the_history_alone() {
        let mut h = app();
        let original = pixels(&h);
        let steps = h.state().doc.as_ref().unwrap().history_step();
        h.state_mut().run(Action::Filter(FilterKind::Emboss));
        settle(&mut h, |app| app.preview_shown.is_some());
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(h.state().filters.dialog.is_none());
        assert!(h.state().preview_shown.is_none());
        assert_eq!(pixels(&h), original);
        assert_eq!(h.state().doc.as_ref().unwrap().history_step(), steps);
        assert!(h.state().filters.last.is_none());
    }

    #[test]
    fn filters_stay_inside_the_selection() {
        let mut h = app();
        let original = pixels(&h);
        let app = h.state_mut();
        app.selection = Some(
            PixelRect {
                x: 0,
                y: 0,
                w: 64,
                h: 32,
            }
            .into(),
        );
        app.run(Action::Filter(FilterKind::FindEdges));
        settle(&mut h, applied);
        let out = pixels(&h);
        assert!(
            out.get_pixel(32, 10).0[0] < 50,
            "edge found in the selection"
        );
        for y in 32..64 {
            for x in 0..64 {
                assert_eq!(out.get_pixel(x, y), original.get_pixel(x, y));
            }
        }
    }

    #[test]
    fn a_targeted_mask_is_filtered_instead_of_the_pixels() {
        let mut h = app();
        let original = pixels(&h);
        let app = h.state_mut();
        let doc = app.doc.as_mut().unwrap();
        assert!(doc.add_mask(crate::document::NewMask::RevealAll, None));
        assert!(doc.mask_targeted());
        app.run(Action::Filter(FilterKind::Clouds));
        // Clouds in the mask (the filter may not have started yet when the
        // first frames run, under a loaded test machine).
        let clouds = |app: &AtelierApp| {
            let doc = app.doc.as_ref().unwrap();
            let mask = doc.active_layer().mask.as_ref().expect("mask kept");
            let values: std::collections::HashSet<u8> =
                mask.image().pixels().map(|p| p.0[0]).collect();
            values.len() > 10
        };
        settle(&mut h, |app| applied(app) && clouds(app));
        assert_eq!(pixels(&h), original);
    }

    #[test]
    fn hidden_layers_are_not_filtered() {
        let mut h = app();
        let app = h.state_mut();
        app.doc.as_mut().unwrap().active_layer_mut().visible = false;
        app.run(Action::Filter(FilterKind::GaussianBlur));
        assert!(app.filters.dialog.is_none());
        app.run(Action::Filter(FilterKind::FindEdges));
        assert!(app.filters.applying.is_none());
        let name = app.doc.as_ref().unwrap().active_layer().name.clone();
        assert_eq!(
            app.status,
            t!("fx3-filter-layer-hidden", name = name.as_str())
        );
    }

    #[test]
    fn a_filter_being_applied_stops_with_esc() {
        let mut h = app();
        let original = pixels(&h);
        // Seconds of work on any image.
        let slow = Filter::Minimum(Morphology {
            radius: 500.0,
            roundness: true,
        });
        h.state_mut().start_filter(slow);
        assert!(h.state().filters.applying.is_some());
        h.run_steps(1);
        h.key_press(egui::Key::Escape);
        h.run_steps(1);
        assert!(h.state().filters.applying.is_none());
        assert_eq!(pixels(&h), original);
        assert!(!h.state().doc.as_ref().unwrap().can_undo(), "no step left");
    }

    #[test]
    fn a_small_layer_enlarged_for_the_preview_shrinks_back_on_cancel() {
        let mut h = app();
        let doc = h.state_mut().doc.as_mut().unwrap();
        let layer = doc.active_layer_mut();
        layer.replace_pixels(RgbaImage::from_pixel(10, 8, image::Rgba([200, 0, 0, 255])));
        layer.offset = (5, 6);
        h.state_mut().run(Action::Filter(FilterKind::GaussianBlur));
        let size = |h: &Harness<'static, AtelierApp>| {
            let l = h.state().doc.as_ref().unwrap().active_layer();
            (l.image().dimensions(), l.offset)
        };
        assert_eq!(size(&h).0, (64, 64), "previewed over the canvas");
        settle(&mut h, |app| app.preview_shown.is_some());
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert_eq!(size(&h), ((10, 8), (5, 6)));
    }

    #[test]
    fn a_layer_enlarged_for_the_preview_shrinks_back_when_the_result_is_cancelled() {
        let mut h = app();
        let doc = h.state_mut().doc.as_mut().unwrap();
        let layer = doc.active_layer_mut();
        layer.replace_pixels(RgbaImage::from_pixel(10, 8, image::Rgba([200, 0, 0, 255])));
        layer.offset = (5, 6);
        let app = h.state_mut();
        app.run(Action::Filter(FilterKind::Minimum));
        let filter = app.filters.dialog.take().expect("dialog").filter;
        // OK, then cancelled while it computes.
        app.start_filter(filter);
        assert!(app.filters.applying.is_some());
        app.cancel_filter();
        let doc = app.doc.as_ref().unwrap();
        let l = doc.active_layer();
        assert_eq!((l.image().dimensions(), l.offset), ((10, 8), (5, 6)));
        assert!(!doc.can_undo());
    }

    #[test]
    fn hidden_layers_are_neither_filled_nor_adjusted() {
        let mut h = app();
        let app = h.state_mut();
        app.doc.as_mut().unwrap().active_layer_mut().visible = false;
        let before = pixels(&h);
        let app = h.state_mut();
        for action in [
            Action::FillPrimary,
            Action::FillDialog,
            Action::MakeTransparent,
            Action::Adjust(AdjustKind::Invert),
            Action::Adjust(AdjustKind::Levels),
        ] {
            app.status.clear();
            app.run(action);
            assert_eq!(
                Some(&app.status),
                app.hidden_target(false).as_ref(),
                "{action:?}"
            );
        }
        assert!(app.fill_dialog.is_none() && app.adjust.is_none() && app.transparency.is_none());
        let doc = app.doc.as_ref().unwrap();
        assert!(!doc.can_undo());
        assert_eq!(pixels(&h), before);
        // Shown again: filled.
        let app = h.state_mut();
        app.doc.as_mut().unwrap().active_layer_mut().visible = true;
        app.run(Action::FillPrimary);
        assert!(app.doc.as_ref().unwrap().can_undo());
    }

    #[test]
    fn locked_layers_are_not_filtered() {
        let mut h = app();
        let app = h.state_mut();
        app.doc.as_mut().unwrap().active_layer_mut().locked = true;
        app.run(Action::Filter(FilterKind::GaussianBlur));
        assert!(app.filters.dialog.is_none());
        app.run(Action::Filter(FilterKind::Clouds));
        assert!(app.filters.applying.is_none());
    }

    #[test]
    fn preview_square_zooms_and_pans() {
        let mut h = app();
        h.state_mut().run(Action::Filter(FilterKind::Mosaic));
        settle(&mut h, |app| {
            app.filters
                .dialog
                .as_ref()
                .is_some_and(|d| d.square.shown.is_some())
        });
        use egui_kittest::kittest::Queryable as _;
        let click = |h: &mut Harness<'static, AtelierApp>, pos: Pos2| {
            h.hover_at(pos);
            h.run_steps(1);
            h.drag_at(pos);
            h.run_steps(1);
            h.event(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            h.run_steps(2);
        };
        // + is right of the percentage.
        let area = h.ctx.memory(|m| m.area_rect(egui::Id::new("filter")));
        let percent = h
            .get_all_by_label(&i18n::percent("100"))
            .map(|n| n.rect())
            .find(|r| area.is_some_and(|a| a.contains_rect(*r)))
            .expect("zoom of the square");
        click(&mut h, percent.right_center() + vec2(18.0, 0.0));
        let dialog = h.state().filters.dialog.as_ref().unwrap();
        assert_eq!(dialog.zoom, 2.0);
        let center = dialog.center;
        // Dragging the square moves the view the other way.
        let from = pos2(percent.center().x, percent.top() - 100.0);
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for i in 1..=4 {
            h.hover_at(from + vec2(5.0 * i as f32, 0.0));
            h.run_steps(1);
        }
        h.event(egui::Event::PointerButton {
            pos: from + vec2(20.0, 0.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(1);
        let moved = h.state().filters.dialog.as_ref().unwrap().center;
        assert!(moved.x < center.x, "{center:?} → {moved:?}");
    }
}
