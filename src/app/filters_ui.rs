//! Filter menu: the filter dialogs (settings, preview square, live preview
//! on the canvas), Last Filter, and the filters computed off the UI thread.

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use image::RgbaImage;

use super::actions_panel::Stopped;
use super::dialogs::{
    DialogSpec, Steps, Track, block, dialog_body_room, dialog_bounds, floating_dialog,
    full_segmented, gap_between_blocks, slider_row_steps, tool_dialog_pos,
};
use super::*;
use crate::filters::{
    Filter, FilterKind, Morphology, PolarConversion, Remove, RippleSize, Shear, SmartSharpen,
    SpherizeMode, Target, ToneFade, Undefined, WaveType, ZigZagStyle,
};
use crate::i18n::{Message, t};
use ui_kit::color;

/// Last Filter: Alt+Ctrl+F, like Photoshop.
pub(super) const SHORTCUT_LAST_FILTER: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::F);

/// Submenus of the Filter menu, in Photoshop's order.
const GROUPS: [(Group, &[FilterKind]); 8] = [
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
        Group::Distort,
        &[
            FilterKind::Pinch,
            FilterKind::PolarCoordinates,
            FilterKind::Ripple,
            FilterKind::Shear,
            FilterKind::Spherize,
            FilterKind::Twirl,
            FilterKind::Wave,
            FilterKind::ZigZag,
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
    Distort,
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
            Group::Distort => t!("distort8-group"),
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
        FilterKind::Pinch => t!("distort8-pinch"),
        FilterKind::PolarCoordinates => t!("distort8-polar"),
        FilterKind::Ripple => t!("distort8-ripple"),
        FilterKind::Shear => t!("distort8-shear"),
        FilterKind::Spherize => t!("distort8-spherize"),
        FilterKind::Twirl => t!("distort8-twirl"),
        FilterKind::Wave => t!("distort8-wave"),
        FilterKind::ZigZag => t!("distort8-zigzag"),
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

/// What the command palette also finds a filter by.
pub(super) fn keywords(kind: FilterKind) -> String {
    match kind {
        FilterKind::Pinch => t!("distort8-keywords-pinch"),
        FilterKind::PolarCoordinates => t!("distort8-keywords-polar"),
        FilterKind::Ripple => t!("distort8-keywords-ripple"),
        FilterKind::Shear => t!("distort8-keywords-shear"),
        FilterKind::Spherize => t!("distort8-keywords-spherize"),
        FilterKind::Twirl => t!("distort8-keywords-twirl"),
        FilterKind::Wave => t!("distort8-keywords-wave"),
        FilterKind::ZigZag => t!("distort8-keywords-zigzag"),
        _ => String::new(),
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
    pub(super) last: Option<Filter>,
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
    /// An adjustment's OK being computed.
    adjusting: Option<AdjustApplying>,
    /// The Fade window.
    pub(super) fade: Option<fade_stroke::FadeDialog>,
    /// The Stroke window is open, and what it opens with.
    pub(super) stroke_open: bool,
    pub(super) stroke_options: fade_stroke::StrokeOptions,
    /// Fade, Stroke or an Auto command being computed.
    pub(super) processing: Option<fade_stroke::Processing>,
}

/// An adjustment applied (its dialog's OK), computed on another thread:
/// for the document, layer and target revision then.
struct AdjustApplying {
    adjustment: crate::pixel_ops::Adjustment,
    doc: u64,
    layer: u64,
    rev: u64,
    started: f64,
    job: Job<RgbaImage>,
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
        self.dialog.is_some()
            || self.applying.is_some()
            || self.adjusting.is_some()
            || self.fade.is_some()
            || self.stroke_open
            || self.processing.is_some()
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
    /// The canvas computation is a quick one (see [`crate::filters::drafted`]).
    canvas_draft: bool,
    /// Key of the canvas preview shown if it is a quick one: the result
    /// itself is computed next.
    drafted: Option<u64>,
    /// Key of a canvas preview whose computation failed (not tried again).
    failed: Option<u64>,
    /// Smart Sharpen: Shadows and Highlights settings shown.
    tones: bool,
    /// What the filter will find once the target is enlarged, without
    /// changing it (see [`Document::target_as_painted`]), for a revision.
    painted: Option<(u64, Option<crate::document::Layer>)>,
    /// Height of the settings, once known.
    settings_height: Option<f32>,
}

/// Width and height the preview square and the settings take in a dialog.
struct DialogLayout {
    /// Side by side rather than one above the other.
    columns: bool,
    /// Side of the preview square.
    side: f32,
    /// Height the settings get beside it.
    room: f32,
}

impl DialogLayout {
    /// Below each other when all fits; else, settings no taller than the
    /// square, a smaller square above them; else side by side when the
    /// window is wide enough, the settings scrolling on their own beside the
    /// square.
    fn of(ctx: &egui::Context, settings: Option<f32>) -> Self {
        let room = dialog_body_room(ctx);
        let settings = settings.unwrap_or(0.0);
        // The zoom row, the gap and the spacing between them.
        let between = ZOOM_ROW + 18.0;
        let stacked = SQUARE + between + settings;
        let side = (room - ZOOM_ROW).clamp(MIN_SQUARE, SQUARE);
        let wide = dialog_bounds(ctx).width() >= side + SQUARE + SCROLL_ROOM + 8.0 + 34.0;
        if stacked <= room || !wide {
            return Self {
                columns: false,
                side: SQUARE,
                room,
            };
        }
        if settings <= side {
            return Self {
                columns: false,
                side: (room - between - settings).clamp(MIN_SQUARE_ABOVE, SQUARE),
                room,
            };
        }
        Self {
            columns: true,
            side,
            room: room.max(side + ZOOM_ROW),
        }
    }

    /// Width of the dialog.
    fn width(&self) -> f32 {
        if self.columns {
            self.side + SQUARE + SCROLL_ROOM + 8.0 + 32.0
        } else {
            SQUARE + 32.0
        }
    }
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

/// Size and canvas position of an image.
type ImageBox = ((u32, u32), (i64, i64));

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

/// Side of the preview square (the dialog's body width), and of the settings'
/// width.
const SQUARE: f32 = 288.0;
/// Room for the scroll bar of settings beside the square.
const SCROLL_ROOM: f32 = 12.0;
/// Smallest side of the preview square, beside the settings on a short window.
const MIN_SQUARE: f32 = 128.0;
/// Smallest side of the preview square above short settings.
const MIN_SQUARE_ABOVE: f32 = 96.0;
/// Height of the zoom row below the square, with the gap.
const ZOOM_ROW: f32 = 36.0;
/// Zoom levels of the preview square.
const SQUARE_ZOOMS: [f32; 14] = [
    0.0625, 0.125, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0,
];
/// Delay (s) before the busy indicator of a filter being applied shows.
pub(super) const BUSY_DELAY: f64 = 0.3;

/// What a filter works on: the active layer's pixels, or its mask when
/// targeted (see [`Document::target_layer`]), and the area of the
/// selection. `None` without either, or if the selection does not touch it.
pub(super) fn active_target(doc: &Document, selection: Option<&Selection>) -> Option<Target> {
    target_on(doc, doc.target_layer()?.as_ref(), selection)
}

/// [`active_target`] with `layer` as the pixels.
fn target_on(
    doc: &Document,
    layer: &crate::document::Layer,
    selection: Option<&Selection>,
) -> Option<Target> {
    let clip = match selection {
        Some(selection) => {
            let clip = selection.clip(layer, doc.full_rect())?;
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
        alpha_locked: doc.alpha_locked(),
        whole_source: false,
    })
}

/// A filter that fits its geometry to its area, without a selection: the
/// canvas's box, not however far the layer's hidden pixels go, which stay as
/// they are.
fn fit_to_canvas(target: &mut Target, doc: &Document, filter: &Filter) {
    if target.clip.is_some() || !filter.fits_the_area() {
        return;
    }
    let (w, h) = target.image.dimensions();
    let c = doc.full_rect();
    let local = |at: i64, from: i64, most: u32| (at - from).clamp(0, i64::from(most)) as u32;
    let area = [
        local(c.x, target.offset.0, w),
        local(c.y, target.offset.1, h),
        local(c.right(), target.offset.0, w),
        local(c.bottom(), target.offset.1, h),
    ];
    if area != [0, 0, w, h] && area[0] < area[2] && area[1] < area[3] {
        target.clip = Some(area.into());
    }
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
            self.status_cannot(t!("filter-no-pixels"));
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
        let painted = doc.target_as_painted();
        let Some(offset) = painted
            .as_ref()
            .map(|l| l.offset)
            .or_else(|| doc.target_layer().map(|l| l.offset))
        else {
            return;
        };
        let center = pos2(
            (around.x - offset.0) as f32 + around.w as f32 / 2.0,
            (around.y - offset.1) as f32 + around.h as f32 / 2.0,
        );
        let painted = Some((doc.target_rev(), painted));
        self.filters.dialog = Some(FilterDialog {
            filter,
            opened: filter,
            preview: true,
            zoom: 1.0,
            center,
            square: Square::default(),
            canvas: None,
            canvas_draft: false,
            drafted: None,
            failed: None,
            tones: false,
            painted,
            settings_height: None,
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
    /// `ready` comes with the size and the position of the image it fits.
    fn start_filter_with(
        &mut self,
        filter: Filter,
        ready: Option<(Ready, ImageBox)>,
    ) {
        if self.refuse_hidden_target() {
            return;
        }
        if self.smart_filters_apply() {
            // Last Filter and the action being recorded get it once
            // rendered (not cancelled).
            self.add_smart_filter(filter, move |app| {
                app.filters.last = Some(filter);
                app.filters.settings.insert(filter.kind(), filter);
                app.record(crate::actions::Recorded::Filter { filter });
            });
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let rasterized = doc.active_layer().text.is_some() && !doc.mask_targeted();
        doc.checkpoint(title(filter.kind()));
        // The layer grows to the canvas, or its targeted mask stands in for
        // it, until the result is in (`poll_filter`).
        if !doc.begin_paint() {
            doc.discard_step();
            self.status_cannot(t!("filter-no-pixels"));
            return;
        }
        let Some(mut target) = active_target(doc, self.selection.as_ref()) else {
            doc.end_paint();
            doc.discard_step();
            self.status_cannot(t!("status-selection-empty"));
            return;
        };
        fit_to_canvas(&mut target, doc, &filter);
        // A preview of other dimensions (a text, enlarged only now) would
        // not fit the layer any more.
        if self
            .preview_shown
            .as_ref()
            .is_some_and(|p| p.image.dimensions() != target.image.dimensions())
        {
            self.preview_shown = None;
        }
        // The preview is the result only if it was computed for this box.
        let placed = (target.image.dimensions(), target.offset);
        let ready = ready.filter(|(_, fits)| *fits == placed).map(|(r, _)| r);
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
                    doc.mark_fade();
                    self.status = t!("filter-applied", name = name.as_str());
                    if a.rasterized {
                        let layer = doc.active_layer().name.as_str();
                        let note = t!("canvas-status-text-rasterized", name = layer);
                        self.status = format!("{} — {note}", self.status);
                    }
                    self.filters.last = Some(a.filter);
                    self.filters.settings.insert(a.filter.kind(), a.filter);
                    // Recorded once done (not when cancelled).
                    self.record(crate::actions::Recorded::Filter { filter: a.filter });
                }
                // The computation failed: nothing changes.
                None => {
                    doc.end_paint();
                    doc.discard_step();
                    let kind = a.filter.kind();
                    let text = tm!("fx3-filter-failed", name = Message::of(|| title(kind)));
                    self.actions
                        .step_stopped(Stopped::Failed(text.shown.clone()));
                    self.status_failed(text);
                    self.shrink_after_preview();
                }
            }
        }
        self.end_layer_preview();
        self.reset_pending();
        if let Some(doc) = &self.doc {
            doc.release_stand_in();
        }
    }

    /// The preview of an adjustment dialog (or Make Transparent) on the
    /// canvas, computed on another thread: the last one stays shown until
    /// the next is ready, one computation at a time.
    pub(super) fn adjust_preview(&mut self, preview: LayerPreview) {
        let Some(doc) = &self.doc else { return };
        let (id, rev) = (doc.active_layer().id, doc.target_rev());
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
                                let name = Message::of(|| a.title());
                                tm!("fx4-preview-failed", name = name)
                            }
                            LayerPreview::Transparency(_) => {
                                let name = Message::of(|| t!("dialog-transparency-title"));
                                tm!("fx4-preview-failed", name = name)
                            }
                            LayerPreview::Fade(_) | LayerPreview::Filter(_) => {
                                tm!("fx4-preview-failed-any")
                            }
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
        // Like the result: only inside the selection (Fade: what it faded
        // was).
        let selection = self.selection.as_ref();
        let selection = selection.filter(|_| !matches!(preview, LayerPreview::Fade(_)));
        let Some(target) = active_target(doc, selection) else {
            return self.end_layer_preview();
        };
        let wanted = preview.clone();
        let name = match &preview {
            LayerPreview::Adjust(a) => format!("Adjustment preview: {:?}", a.kind()),
            LayerPreview::Transparency(_) => "Make Transparent preview".to_owned(),
            LayerPreview::Fade(_) => "Fade preview".to_owned(),
            LayerPreview::Filter(_) => "Layer preview".to_owned(),
        };
        let job = Job::spawn(&self.ctx, 0, [0; 4], Some(name), move || {
            target.process(0, |image, _| wanted.apply(image))
        });
        self.filters.adjust = Some((preview, id, rev, job));
    }

    /// OK of the adjustment dialog: `adjustment` applied to the target
    /// (inside the selection). The preview shown is put in when it is the
    /// result; else the one being computed is waited for, or the result is
    /// computed on another thread (see [`Self::adjusting_window`]).
    pub(super) fn apply_adjustment(&mut self, adjustment: crate::pixel_ops::Adjustment) {
        let Some(doc) = &mut self.doc else { return };
        if adjustment.is_neutral() {
            // Nothing changed: a Fade after it has nothing to fade.
            doc.mark_fade_nothing();
            return self.record(crate::actions::Recorded::Adjust { adjustment });
        }
        let (doc_id, id, rev) = (doc.uid(), doc.active_layer().id, doc.target_rev());
        let wanted = LayerPreview::Adjust(adjustment.clone());
        if let Some(p) = self
            .preview_shown
            .take_if(|p| (&p.preview, p.layer, p.rev) == (&wanted, id, rev))
        {
            return self.put_adjustment(adjustment, Arc::unwrap_or_clone(p.image));
        }
        self.preview_shown = None;
        let Some(doc) = &self.doc else { return };
        let job = match self.filters.adjust.take() {
            Some((preview, layer, rev_then, job))
                if (&preview, layer, rev_then) == (&wanted, id, rev) && !job.stopped() =>
            {
                job
            }
            _ => {
                let Some(target) = active_target(doc, self.selection.as_ref()) else {
                    self.status_cannot(t!("status-selection-empty"));
                    return;
                };
                let name = format!("Adjustment: {:?}", adjustment.kind());
                let a = adjustment.clone();
                Job::spawn(&self.ctx, 0, [0; 4], Some(name), move || {
                    target.process(0, |image, _| a.apply(image))
                })
            }
        };
        self.filters.adjusting = Some(AdjustApplying {
            adjustment,
            doc: doc_id,
            layer: id,
            rev,
            started: self.ctx.input(|i| i.time),
            job,
        });
    }

    /// The adjustment's result `image` replaces the target's pixels (one
    /// step; computed again here if it no longer fits them), and is
    /// recorded.
    fn put_adjustment(&mut self, adjustment: crate::pixel_ops::Adjustment, image: RgbaImage) {
        let Some(doc) = &mut self.doc else { return };
        if doc.put_processed_active(adjustment.title(), image)
            || doc.adjust_active(&adjustment, self.selection.as_ref())
        {
            doc.mark_fade();
            self.status = t!("dialog-adjust-applied", name = adjustment.title());
        } else {
            doc.mark_fade_nothing();
        }
        self.record(crate::actions::Recorded::Adjust { adjustment });
    }

    /// The indicator of an adjustment being applied, then its result put
    /// in. Returns whether one is under way.
    pub(super) fn adjusting_window(&mut self, ctx: &egui::Context) -> bool {
        let Some(a) = &self.filters.adjusting else {
            return false;
        };
        let result = match a.job.poll() {
            Ok(None) => {
                let name = a.adjustment.title();
                if cancellable_indicator(ctx, "adjust_busy", &name, a.started, None) {
                    self.filters.adjusting = None;
                    self.status = t!("retouch-status-cancelled", name = name.as_str());
                    self.actions.step_stopped(Stopped::Cancelled);
                    self.reset_pending();
                    return false;
                }
                return true;
            }
            Ok(Some(image)) => Some(image),
            Err(()) => None,
        };
        let a = self.filters.adjusting.take().expect("checked above");
        let current = self.doc.as_ref().is_some_and(|d| {
            (d.uid(), d.active_layer().id, d.target_rev()) == (a.doc, a.layer, a.rev)
        });
        match result {
            Some(image) if current => self.put_adjustment(a.adjustment, image),
            Some(_) => {}
            None => {
                let adjustment = a.adjustment;
                let text = tm!(
                    "fx3-filter-failed",
                    name = Message::of(|| adjustment.title())
                );
                self.actions
                    .step_stopped(Stopped::Failed(text.shown.clone()));
                self.status_failed(text);
            }
        }
        self.reset_pending();
        false
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
        match self.hidden_message(filter) {
            Some(message) => {
                self.status_failed(message);
                true
            }
            None => false,
        }
    }

    /// Why the active layer can't be changed: it is hidden (or in a hidden
    /// group). `filter`: in a filter's words. In Quick Mask Mode, the mask
    /// is changed instead, always shown.
    pub(super) fn hidden_target(&self, filter: bool) -> Option<String> {
        self.hidden_message(filter).map(|m| m.shown)
    }

    /// [`Self::hidden_target`], for the report too: the hidden group named
    /// when the layer's own eye is on.
    fn hidden_message(&self, filter: bool) -> Option<crate::i18n::Message> {
        let doc = self.doc.as_ref()?;
        if doc.shown(doc.active) || doc.quick_mask().is_some() {
            return None;
        }
        let name = doc.active_layer().name.as_str();
        Some(match (self.hidden_group(), filter) {
            (Some(group), true) => tm!("tools6-status-group-hidden-filter", name = name, group = group),
            (Some(group), false) => tm!("tools6-status-group-hidden", name = name, group = group),
            (None, true) => tm!("fx3-filter-layer-hidden", name = name),
            (None, false) => tm!("fx4-layer-hidden", name = name),
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
    pub(super) fn cancel_filter(&mut self) {
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
        self.actions.step_stopped(Stopped::Cancelled);
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
        let target_rev = doc.target_rev();
        if dialog.painted.as_ref().is_none_or(|(rev, _)| *rev != target_rev) {
            dialog.painted = Some((target_rev, doc.target_as_painted()));
        }
        let painted = dialog.painted.as_ref().and_then(|(_, l)| l.as_ref());
        let selection = self.selection.as_ref();
        let target = smart.or_else(|| {
            let mut target = match painted {
                Some(layer) => target_on(doc, layer, selection),
                None => active_target(doc, selection),
            }?;
            fit_to_canvas(&mut target, doc, &dialog.filter);
            Some(target)
        });
        let layer = doc.active_layer();
        let (layer_id, layer_rev) = (layer.id, layer.rev());
        let size = target
            .as_ref()
            .map_or(layer.image().dimensions(), |t| t.image.dimensions());
        let fingerprint = hash_of(format!("{:?}", dialog.filter));
        let layout = DialogLayout::of(ctx, dialog.settings_height);
        if let Some(target) = &target {
            let key = (layer_id, layer_rev, fingerprint);
            self.update_square(ctx, &mut dialog, target, key, layout.side);
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
                width: layout.width(),
                ..DialogSpec::new("filter", title(kind), tool_dialog_pos(ctx))
            },
            |ui| {
                let height = if layout.columns {
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(layout.side);
                            preview_square(ui, &mut dialog, layout.side);
                        });
                        ui.vertical(|ui| {
                            ui.set_width(SQUARE + SCROLL_ROOM);
                            // A body that scrolls shows it.
                            ui.spacing_mut().scroll.dormant_handle_opacity = 0.6;
                            egui::ScrollArea::vertical()
                                .id_salt("filter_settings")
                                .max_height(layout.room)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    ui.scope(|ui| {
                                        ui.set_max_width(SQUARE);
                                        settings(ui, &mut dialog.filter, &mut dialog.tones, size);
                                    })
                                    .response
                                    .rect
                                    .height()
                                })
                                .inner
                        })
                        .inner
                    })
                    .inner
                } else {
                    preview_square(ui, &mut dialog, layout.side);
                    gap_between_blocks(ui);
                    ui.scope(|ui| {
                        settings(ui, &mut dialog.filter, &mut dialog.tones, size);
                    })
                    .response
                    .rect
                    .height()
                };
                dialog.settings_height = Some(height);
            },
        );
        // The first frame only measures: shown laid out for what it measured.
        let measured = DialogLayout::of(ctx, dialog.settings_height);
        if measured.columns != layout.columns || measured.side != layout.side {
            ctx.request_discard("the filter dialog's layout");
        }
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
            // A quick preview is not the result.
            let shown = shown.filter(|_| dialog.drafted != Some(key));
            let ready = if self.smart_filters_apply() {
                None
            } else if let Some(p) = shown {
                Some(Ready::Image(p.image.clone()))
            } else {
                dialog
                    .canvas
                    .take()
                    .filter(|job| job.key == key && !job.stopped() && !dialog.canvas_draft)
                    .map(Ready::Job)
            };
            let placed = target.as_ref().map(|t| (t.image.dimensions(), t.offset));
            let ready = ready.zip(placed);
            self.start_filter_with(dialog.filter, ready);
        } else if out.cancel {
            self.end_layer_preview();
            self.shrink_after_preview();
            if let Some(doc) = &self.doc {
                doc.release_stand_in();
            }
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
        side: f32,
    ) {
        let ppp = ctx.pixels_per_point();
        let (w, h) = target.image.dimensions();
        dialog.center = pos2(
            dialog.center.x.clamp(0.0, w as f32),
            dialog.center.y.clamp(0.0, h as f32),
        );
        let extent = side * ppp / dialog.zoom;
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
            && !job.stopped()
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
        // A computation for another view is stopped; the newest one starts
        // once it is over.
        if let Some(job) = &square.job
            && job.key != wanted
        {
            job.stop();
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
                // Reduced below 100 %: one sample per pixel is enough.
                let mut filtered =
                    crate::filters::drafted(zoom < 1.0, || target.filter_region(region, &filter));
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
                    && image.dimensions() == target.image.dimensions()
                {
                    let preview = LayerPreview::Filter(job.key);
                    dialog.drafted = dialog.canvas_draft.then_some(job.key);
                    self.show_preview_at(preview, image, target.offset);
                }
            }
            Some(Err(())) => {
                if let Some(job) = dialog.canvas.take()
                    && !job.stopped()
                {
                    dialog.failed = Some(job.key);
                    let kind = dialog.filter.kind();
                    let name = Message::of(|| title(kind));
                    self.status_failed(tm!("fx3-filter-failed", name = name));
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
        // Settings just changed: a quick preview first, then the result
        // itself once it is shown and the settings stay.
        let draft = !shown && dialog.filter.has_draft();
        let refine = shown && dialog.drafted == Some(key);
        if dialog.preview
            && (!shown || refine)
            && dialog.canvas.is_none()
            && dialog.failed != Some(key)
        {
            let (target, filter) = (target.clone(), dialog.filter);
            let reach = filter.reach().unwrap_or(0);
            let name = format!("Filter canvas preview: {:?}", filter.kind());
            dialog.canvas_draft = draft;
            dialog.canvas = Some(Job::spawn(ctx, key, [0; 4], Some(name), move || {
                crate::filters::drafted(draft, || {
                    target.process(reach, |image, origin| filter.apply(image, origin))
                })
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
fn preview_square(ui: &mut egui::Ui, dialog: &mut FilterDialog, side: f32) {
    let ppp = ui.ctx().pixels_per_point();
    // Narrower than the settings below it: centered above them.
    let (rect, response) = ui
        .vertical_centered(|ui| ui.allocate_exact_size(Vec2::splat(side), Sense::drag()))
        .inner;
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
        // Painted, not added: a widget here only while computing would
        // shift the automatic ids of the fields below and drop their focus.
        egui::Spinner::new()
            .size(14.0)
            .color(color::MUTED)
            .paint_at(ui, spinner);
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
        (Filter::Ripple { amount: a, size }, Filter::Ripple { amount: d, .. }) => {
            whole(ui, &amount, a, -999.0..=999.0, &percent, d);
            gap_between_blocks(ui);
            block(ui, |ui| {
                ui_kit::field_label(ui, &t!("distort8-size"));
                let (s, m, l) = (
                    t!("distort8-size-small"),
                    t!("distort8-size-medium"),
                    t!("distort8-size-large"),
                );
                full_segmented(
                    ui,
                    size,
                    &[
                        (RippleSize::Small, &s),
                        (RippleSize::Medium, &m),
                        (RippleSize::Large, &l),
                    ],
                );
            });
        }
        (Filter::Wave(w), Filter::Wave(d)) => wave_settings(ui, w, &d),
        (Filter::Twirl { angle: a }, Filter::Twirl { angle: d }) => {
            whole(ui, &angle, a, -999.0..=999.0, "°", d);
        }
        (Filter::Spherize { amount: a, mode }, Filter::Spherize { amount: d, .. }) => {
            whole(ui, &amount, a, -100.0..=100.0, &percent, d);
            gap_between_blocks(ui);
            choice(
                ui,
                "distort8_mode",
                &t!("distort8-mode"),
                mode,
                &[
                    (SpherizeMode::Normal, t!("distort8-mode-normal")),
                    (SpherizeMode::HorizontalOnly, t!("distort8-mode-horizontal")),
                    (SpherizeMode::VerticalOnly, t!("distort8-mode-vertical")),
                ],
            );
        }
        (Filter::PolarCoordinates { conversion }, _) => {
            choice(
                ui,
                "distort8_conversion",
                &t!("distort8-conversion"),
                conversion,
                &[
                    (
                        PolarConversion::RectangularToPolar,
                        t!("distort8-rectangular-to-polar"),
                    ),
                    (
                        PolarConversion::PolarToRectangular,
                        t!("distort8-polar-to-rectangular"),
                    ),
                ],
            );
        }
        (Filter::Pinch { amount: a }, Filter::Pinch { amount: d }) => {
            whole(ui, &amount, a, -100.0..=100.0, &percent, d);
        }
        (
            Filter::ZigZag {
                amount: a,
                ridges,
                style,
            },
            Filter::ZigZag {
                amount: da,
                ridges: dr,
                ..
            },
        ) => {
            whole(ui, &amount, a, -100.0..=100.0, &percent, da);
            gap_between_blocks(ui);
            whole(ui, &t!("distort8-ridges"), ridges, 1.0..=20.0, "", dr);
            gap_between_blocks(ui);
            choice(
                ui,
                "distort8_style",
                &t!("distort8-style"),
                style,
                &[
                    (ZigZagStyle::AroundCenter, t!("distort8-around-center")),
                    (ZigZagStyle::OutFromCenter, t!("distort8-out-from-center")),
                    (ZigZagStyle::PondRipples, t!("distort8-pond-ripples")),
                ],
            );
        }
        (Filter::Shear(s), _) => {
            shear_curve(ui, s);
            gap_between_blocks(ui);
            if ui_kit::button(
                ui,
                &t!("distort8-shear-default"),
                ui_kit::Kind::Normal,
                None,
                true,
            )
            .clicked()
            {
                *s = Shear::straight(s.undefined);
            }
            gap_between_blocks(ui);
            undefined_choice(ui, &mut s.undefined);
        }
        // Without settings, or the settings of another kind (never).
        _ => {}
    }
}

/// A drop-down under its label.
fn choice<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    label: &str,
    value: &mut T,
    options: &[(T, String)],
) {
    block(ui, |ui| {
        ui_kit::field_label(ui, label);
        let width = ui.available_width();
        let chosen = options
            .iter()
            .find(|(v, _)| v == value)
            .map_or("", |(_, name)| name.as_str());
        ui_kit::combo(ui, id, None, chosen, width, |ui| {
            for (v, name) in options {
                ui.selectable_value(value, *v, name);
            }
        });
    });
}

/// Undefined areas of Wave and Shear.
fn undefined_choice(ui: &mut egui::Ui, undefined: &mut Undefined) {
    choice(
        ui,
        "distort8_undefined",
        &t!("filter-undefined-areas"),
        undefined,
        &[
            (Undefined::WrapAround, t!("filter-wrap-around")),
            (Undefined::RepeatEdges, t!("filter-repeat-edges")),
        ],
    );
}

/// Wave: generators, lengths (a minimum stays below its maximum), scales,
/// type, undefined areas, Randomize.
fn wave_settings(ui: &mut egui::Ui, w: &mut crate::filters::Wave, d: &crate::filters::Wave) {
    let percent = i18n::percent("");
    let row = |ui: &mut egui::Ui, label: String, value: &mut f32, unit: &str, default: f32| {
        wide(ui, &label, value, 1.0..=999.0, unit, default);
        gap_between_blocks(ui);
    };
    row(
        ui,
        t!("distort8-generators"),
        &mut w.generators,
        "",
        d.generators,
    );
    let min = w.wavelength_min;
    row(
        ui,
        t!("distort8-wavelength-min"),
        &mut w.wavelength_min,
        " px",
        d.wavelength_min,
    );
    row(
        ui,
        t!("distort8-wavelength-max"),
        &mut w.wavelength_max,
        " px",
        d.wavelength_max,
    );
    keep_ordered(&mut w.wavelength_min, &mut w.wavelength_max, min);
    let min = w.amplitude_min;
    row(
        ui,
        t!("distort8-amplitude-min"),
        &mut w.amplitude_min,
        " px",
        d.amplitude_min,
    );
    row(
        ui,
        t!("distort8-amplitude-max"),
        &mut w.amplitude_max,
        " px",
        d.amplitude_max,
    );
    keep_ordered(&mut w.amplitude_min, &mut w.amplitude_max, min);
    for (label, value, default) in [
        (
            t!("distort8-scale-horizontal"),
            &mut w.scale_horizontal,
            d.scale_horizontal,
        ),
        (
            t!("distort8-scale-vertical"),
            &mut w.scale_vertical,
            d.scale_vertical,
        ),
    ] {
        whole(ui, &label, value, 1.0..=100.0, &percent, default);
        gap_between_blocks(ui);
    }
    block(ui, |ui| {
        ui_kit::field_label(ui, &t!("distort8-type"));
        let (sine, triangle, square) = (
            t!("distort8-type-sine"),
            t!("distort8-type-triangle"),
            t!("distort8-type-square"),
        );
        full_segmented(
            ui,
            &mut w.wave_type,
            &[
                (WaveType::Sine, &sine),
                (WaveType::Triangle, &triangle),
                (WaveType::Square, &square),
            ],
        );
    });
    gap_between_blocks(ui);
    undefined_choice(ui, &mut w.undefined);
    gap_between_blocks(ui);
    if ui_kit::button(
        ui,
        &t!("distort8-randomize"),
        ui_kit::Kind::Normal,
        None,
        true,
    )
    .clicked()
    {
        w.seed = fresh_seed();
    }
}

/// A minimum moved above its maximum pushes it up, and the other way round.
fn keep_ordered(min: &mut f32, max: &mut f32, min_before: f32) {
    if *min > *max {
        if *min != min_before {
            *max = *min;
        } else {
            *min = *max;
        }
    }
}

/// The curve of Shear: a click adds a point, a drag moves one or adds one
/// where it starts; a point dragged out of the grid is removed.
fn shear_curve(ui: &mut egui::Ui, s: &mut Shear) {
    let width = ui.available_width().min(SQUARE);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 160.0), Sense::click_and_drag());
    let name = t!("distort8-shear-curve");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &name));
    let to_screen = |p: [f32; 2]| {
        pos2(
            rect.center().x + p[0] * rect.width() / 2.0,
            rect.top() + p[1] * rect.height(),
        )
    };
    let from_screen = |p: Pos2| {
        (
            (p.x - rect.center().x) / (rect.width() / 2.0),
            ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
        )
    };
    let id = response.id.with("point");
    let mut held: Option<usize> = ui.data(|d| d.get_temp(id)).flatten();
    let on_a_point = |s: &Shear, at: Pos2| {
        s.used().iter().any(|&p| to_screen(p).distance(at) <= 10.0)
    };
    if response.clicked()
        && let Some(at) = response.interact_pointer_pos()
        && !on_a_point(s, at)
    {
        let (shift, height) = from_screen(at);
        s.insert(shift, height);
    }
    // From where it was pressed: a drag only starts once the pointer moved.
    if response.drag_started()
        && let Some(at) = ui
            .input(|i| i.pointer.press_origin())
            .or(response.interact_pointer_pos())
    {
        let near = s
            .used()
            .iter()
            .enumerate()
            .map(|(i, &p)| (i, to_screen(p).distance(at)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|&(_, d)| d <= 10.0)
            .map(|(i, _)| i);
        let (shift, height) = from_screen(at);
        held = near.or_else(|| s.insert(shift, height));
    }
    if let Some(i) = held
        && let Some(at) = response.interact_pointer_pos()
    {
        let (shift, height) = from_screen(at);
        if response.dragged() {
            s.place(i, shift, height);
        }
        if response.drag_stopped() {
            if !rect.expand(8.0).contains(at) {
                s.remove(i);
            }
            held = None;
        }
    }
    if !response.dragged() && !response.drag_stopped() {
        held = None;
    }
    ui.data_mut(|d| d.insert_temp(id, held));

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, color::BG0);
    let grid = UiStroke::new(1.0, color::LINE);
    for i in 1..4 {
        let f = i as f32 / 4.0;
        let x = rect.left() + rect.width() * f;
        let y = rect.top() + rect.height() * f;
        painter.vline(x, rect.y_range(), grid);
        painter.hline(rect.x_range(), y, grid);
    }
    let line: Vec<Pos2> = (0..=64)
        .map(|i| {
            let height = i as f32 / 64.0;
            to_screen([s.shift_at(height), height])
        })
        .collect();
    painter.add(egui::Shape::line(line, UiStroke::new(1.5, color::ACCENT)));
    for &p in s.used() {
        painter.circle_filled(to_screen(p), 3.5, color::ACCENT);
    }
    painter.rect_stroke(rect, 0.0, grid, StrokeKind::Inside);
    response.on_hover_text(t!("distort8-shear-tip"));
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

    /// Runs a dialog of `kind` to its result: only in one step, then Last
    /// Filter and the recorded settings have it.
    fn distort_applies(h: &mut Harness<'static, AtelierApp>, kind: FilterKind) {
        let original = pixels(h);
        let steps = h.state().doc.as_ref().unwrap().history_step();
        h.state_mut().run(Action::Filter(kind));
        assert!(h.state().modal_open(), "{kind:?}: a dialog");
        settle(h, |app| {
            matches!(
                app.preview_shown.as_ref().map(|p| &p.preview),
                Some(LayerPreview::Filter(_))
            ) && app
                .filters
                .dialog
                .as_ref()
                .is_some_and(|d| d.square.shown.is_some())
        });
        assert_eq!(pixels(h), original, "{kind:?}: only previewed");
        h.key_press(egui::Key::Enter);
        settle(h, applied);
        assert!(h.state().filters.dialog.is_none());
        assert_eq!(
            h.state().doc.as_ref().unwrap().history_step(),
            steps + 1,
            "{kind:?}: one step"
        );
        assert_eq!(h.state().filters.last, Some(kind.initial()));
    }

    #[test]
    fn distort_filters_preview_apply_and_repeat() {
        let mut h = app();
        for kind in [
            FilterKind::Pinch,
            FilterKind::PolarCoordinates,
            FilterKind::Ripple,
            FilterKind::Shear,
            FilterKind::Spherize,
            FilterKind::Twirl,
            FilterKind::Wave,
            FilterKind::ZigZag,
        ] {
            distort_applies(&mut h, kind);
        }
        // The result is what the filter gives, and Last Filter does it again.
        let before = pixels(&h);
        h.state_mut().run(Action::LastFilter);
        settle(&mut h, applied);
        let mut expected = before;
        FilterKind::ZigZag.initial().apply(&mut expected, (0, 0));
        assert_eq!(pixels(&h), expected);
    }

    #[test]
    fn distort_group_comes_after_blur_with_its_dialogs() {
        let h = app();
        let menus = h.state().menus();
        let (_, entries) = menus
            .iter()
            .find(|(t, _)| *t == t!("filter-menu"))
            .expect("menu");
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Submenu(name, _) => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let group = t!("distort8-group");
        let at = names.iter().position(|n| *n == group).expect("group");
        assert_eq!(names[at - 1], t!("filter-group-blur"));
        assert_eq!(names[at + 1], t!("filter-group-noise"));
    }

    #[test]
    fn distort_dialog_settings_change_the_filter_and_cancel_restores() {
        let mut h = app();
        let original = pixels(&h);
        h.state_mut().run(Action::Filter(FilterKind::Twirl));
        for _ in 0..3 {
            h.run_steps(1);
        }
        let dialog = h.state_mut().filters.dialog.as_mut().unwrap();
        dialog.filter = Filter::Twirl { angle: 300.0 };
        settle(&mut h, |app| {
            let key = hash_of((
                app.doc.as_ref().unwrap().active_layer().id,
                app.doc.as_ref().unwrap().active_layer().rev(),
                hash_of(format!("{:?}", Filter::Twirl { angle: 300.0 })),
            ));
            matches!(
                app.preview_shown.as_ref().map(|p| &p.preview),
                Some(LayerPreview::Filter(k)) if *k == key
            )
        });
        let step = h.state().doc.as_ref().unwrap().history_step();
        h.key_press(egui::Key::Escape);
        for _ in 0..3 {
            h.run_steps(1);
        }
        assert!(h.state().filters.dialog.is_none());
        assert!(h.state().preview_shown.is_none());
        assert_eq!(pixels(&h), original);
        assert_eq!(h.state().doc.as_ref().unwrap().history_step(), step);
    }

    #[test]
    fn distort_applies_inside_the_selection() {
        let mut h = app();
        let original = pixels(&h);
        h.state_mut().selection = Some(Selection {
            rect: PixelRect {
                x: 12,
                y: 12,
                w: 40,
                h: 40,
            },
            mask: None,
            inverted: false,
        });
        h.state_mut().start_filter(Filter::Twirl { angle: 200.0 });
        settle(&mut h, applied);
        let out = pixels(&h);
        for (x, y, p) in out.enumerate_pixels() {
            if !(12..52).contains(&x) || !(12..52).contains(&y) {
                assert_eq!(p, original.get_pixel(x, y), "outside the selection");
            }
        }
        assert_ne!(out, original);
    }

    #[test]
    fn distort_dialog_opens_on_shear_and_wave_with_every_row_in_french_too() {
        for lang in [Lang::En, Lang::Fr] {
            let mut h = app();
            i18n::set_lang(lang);
            for kind in [FilterKind::Wave, FilterKind::Shear, FilterKind::ZigZag] {
                h.state_mut().run(Action::Filter(kind));
                h.run_steps(3);
                assert!(h.state().filters.dialog.is_some());
                h.key_press(egui::Key::Escape);
                h.run_steps(3);
            }
        }
        i18n::set_lang(Lang::En);
    }

    use egui_kittest::kittest::Queryable as _;

    const DISTORTS: [FilterKind; 8] = [
        FilterKind::Ripple,
        FilterKind::Wave,
        FilterKind::Twirl,
        FilterKind::Spherize,
        FilterKind::PolarCoordinates,
        FilterKind::Pinch,
        FilterKind::ZigZag,
        FilterKind::Shear,
    ];

    /// At the smallest window: all of each dialog can be reached, OK and
    /// Cancel on screen and working.
    #[test]
    fn distort_dialogs_fit_a_small_window_with_ok_and_cancel_reachable() {
        for lang in [Lang::En, Lang::Fr] {
            i18n::set_lang(lang);
            for kind in DISTORTS {
                let mut h = app();
                h.set_size(egui::vec2(660.0, 500.0));
                h.run_steps(3);
                h.state_mut().run(Action::Filter(kind));
                h.run_steps(4);
                let what = format!("{lang:?} {kind:?}");
                let window = h.ctx.content_rect();
                for name in [t!("dialog-ok"), t!("dialog-cancel")] {
                    let r = h.get_by_label(&name).rect();
                    assert!(window.contains_rect(r), "{what} {name}: {r:?}");
                }
                // The rows below the preview come up by scrolling, Wave's last
                // one too, above the buttons.
                if kind == FilterKind::Wave {
                    let ok = h.get_by_label(&t!("dialog-ok")).rect();
                    let below = h.get_by_label(&t!("distort8-randomize")).rect();
                    assert!(below.top() >= ok.top(), "{what}: {below:?}");
                    h.hover_at(pos2(ok.left() - 150.0, ok.top() - 150.0));
                    h.run_steps(1);
                    h.event(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -3000.0),
                        modifiers: egui::Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    });
                    h.run_steps(30);
                    let below = h.get_by_label(&t!("distort8-randomize")).rect();
                    assert!(below.bottom() <= ok.top(), "{what}: {below:?} scrolled");
                }
                // Cancel, then OK, both by the pointer.
                h.get_by_label(&t!("dialog-cancel")).click();
                h.run_steps(3);
                assert!(h.state().filters.dialog.is_none(), "{what}: cancelled");
                h.state_mut().run(Action::Filter(kind));
                h.run_steps(3);
                let steps = h.state().doc.as_ref().unwrap().history_step();
                h.get_by_label(&t!("dialog-ok")).click();
                settle(&mut h, |a| a.filters.dialog.is_none() && applied(a));
                assert_eq!(h.state().doc.as_ref().unwrap().history_step(), steps + 1, "{what}");
            }
        }
        i18n::set_lang(Lang::En);
    }

    /// A distort dialog at the smallest window, to look at:
    /// `ATELIER_CAPTURE=out.png ATELIER_LANG=fr ATELIER_FILTER=Wave`.
    #[test]
    #[ignore = "needs a graphics renderer"]
    fn capture_distort_dialog() {
        let name = std::env::var("ATELIER_FILTER").unwrap_or_else(|_| "Wave".into());
        let kind = DISTORTS
            .into_iter()
            .find(|k| format!("{k:?}") == name)
            .expect("a distort filter");
        if std::env::var("ATELIER_LANG").as_deref() == Ok("fr") {
            i18n::set_lang(Lang::Fr);
        }
        let mut h = app();
        h.set_size(egui::vec2(660.0, 500.0));
        h.run_steps(3);
        h.state_mut().run(Action::Filter(kind));
        settle(&mut h, |a| a.filters.dialog.as_ref().is_some_and(|d| d.square.shown.is_some()));
        h.run_steps(3);
        let out = std::env::var("ATELIER_CAPTURE").unwrap_or_else(|_| "capture.png".into());
        h.render().expect("a renderer").save(out).expect("written");
    }

    /// A drag on the Shear curve: a new point under the pointer, then that
    /// point moved; off the grid, it goes.
    #[test]
    fn a_drag_on_the_shear_curve_changes_it() {
        let mut h = app();
        h.state_mut().run(Action::Filter(FilterKind::Shear));
        h.run_steps(3);
        let curve = |h: &Harness<'static, AtelierApp>| -> Shear {
            match h.state().filters.dialog.as_ref().expect("the dialog").filter {
                Filter::Shear(s) => s,
                ref other => panic!("{other:?}"),
            }
        };
        let straight = curve(&h);
        assert_eq!(straight.used().len(), 2);
        let r = h.get_by_label(&t!("distort8-shear-curve")).rect();
        let drag = |h: &mut Harness<'static, AtelierApp>, from: Pos2, to: Pos2| {
            h.hover_at(from);
            h.run_steps(1);
            h.drag_at(from);
            h.run_steps(1);
            for i in 1..=4 {
                h.hover_at(from.lerp(to, i as f32 / 4.0));
                h.run_steps(1);
            }
            h.drop_at(to);
            h.run_steps(2);
        };
        let at = |x: f32, y: f32| pos2(r.center().x + x * r.width() / 2.0, r.top() + y * r.height());
        // On the line, in the middle, pulled to the right.
        drag(&mut h, at(0.0, 0.5), at(0.5, 0.5));
        let s = curve(&h);
        assert_eq!(s.used().len(), 3, "a point added");
        assert!((s.shift_at(0.5) - 0.5).abs() < 0.03, "{}", s.shift_at(0.5));
        assert!(s.shift_at(0.0).abs() < 1e-3 && s.shift_at(1.0).abs() < 1e-3, "the ends stay");
        // That point again, lower and to the left.
        drag(&mut h, at(0.5, 0.5), at(-0.4, 0.75));
        let s = curve(&h);
        assert_eq!(s.used().len(), 3, "the same one moved");
        assert!((s.shift_at(0.75) + 0.4).abs() < 0.03, "{}", s.shift_at(0.75));
        // Out of the grid: gone, and the line is straight.
        drag(&mut h, at(-0.4, 0.75), pos2(r.right() + 60.0, r.center().y));
        assert_eq!(curve(&h).used().len(), 2, "removed");
        assert!(curve(&h).shift_at(0.5).abs() < 1e-3);
        // Applied, the image follows the curve in the middle rows only.
        drag(&mut h, at(0.0, 0.5), at(0.5, 0.5));
        let before = pixels(&h);
        h.key_press(egui::Key::Enter);
        settle(&mut h, |a| a.filters.dialog.is_none() && applied(a));
        let after = pixels(&h);
        assert_ne!(after, before);
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
        assert_eq!(n, 26);
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

    /// A document of `w` × `h` with varied colors.
    fn sized(w: u32, h: u32) -> Harness<'static, AtelierApp> {
        let mut harness = Harness::builder()
            .with_size([1200.0, 900.0])
            .build_eframe(move |cc| {
                let mut app = AtelierApp::new(cc, None);
                let image = image::RgbaImage::from_fn(w, h, |x, y| {
                    image::Rgba([(x % 251) as u8, (y % 241) as u8, 90, 255])
                });
                app.doc = Some(Document::new(image, None));
                app
            });
        harness.run_steps(2);
        harness
    }

    /// Levels and Curves show the histogram of the targeted mask, and of
    /// the quick mask, as they are now: not the one of before an edit.
    #[test]
    fn histogram_follows_mask_and_quick_mask_edits() {
        use crate::pixel_ops::Adjustment;
        let mut h = sized(300, 200);
        let app = h.state_mut();
        let doc = app.doc.as_mut().unwrap();
        assert!(doc.add_mask(crate::document::NewMask::RevealAll, None));
        doc.target_mask(true);
        let white = app.active_histogram().unwrap();
        assert_eq!(white[0][0], 0);
        let doc = app.doc.as_mut().unwrap();
        assert!(doc.adjust_active(&Adjustment::Invert, None));
        let cached = app.active_histogram().unwrap();
        app.histogram = None;
        assert_eq!(cached, app.active_histogram().unwrap(), "layer mask");
        assert_eq!(cached[0][255], 0, "the mask is black now");

        let doc = app.doc.as_mut().unwrap();
        doc.target_mask(false);
        app.selection = Some(
            crate::document::PixelRect {
                x: 0,
                y: 0,
                w: 100,
                h: 200,
            }
            .into(),
        );
        app.run(Action::Select(
            super::super::select_menu::Command::QuickMask,
        ));
        h.run_steps(2);
        let app = h.state_mut();
        assert!(app.doc.as_ref().unwrap().quick_mask().is_some());
        app.selection = None;
        let before = app.active_histogram().unwrap();
        let doc = app.doc.as_mut().unwrap();
        assert!(doc.adjust_active(&Adjustment::Invert, None));
        let cached = app.active_histogram().unwrap();
        app.histogram = None;
        assert_eq!(cached, app.active_histogram().unwrap(), "quick mask");
        assert_ne!(cached, before);
    }

    /// Liquify and the Camera Raw Filter opened with a selection off the
    /// canvas say so and leave the layer as it was (not enlarged).
    #[test]
    fn liquify_and_camera_raw_with_an_empty_selection_leave_the_layer() {
        for which in ["liquify", "camera raw"] {
            let mut h = sized(300, 200);
            let app = h.state_mut();
            let doc = app.doc.as_mut().unwrap();
            let small = image::RgbaImage::from_pixel(50, 50, image::Rgba([200, 0, 0, 255]));
            doc.add_layer("small".into(), small, (20, 20));
            let depth = doc.history().1;
            app.selection = Some(
                crate::document::PixelRect {
                    x: 1000,
                    y: 1000,
                    w: 30,
                    h: 30,
                }
                .into(),
            );
            match which {
                "liquify" => app.open_liquify_with(None),
                _ => app.camera_raw_filter(None),
            }
            h.run_steps(3);
            let app = h.state();
            let doc = app.doc.as_ref().unwrap();
            let layer = doc.active_layer();
            assert_eq!(layer.image().dimensions(), (50, 50), "{which}");
            assert_eq!(layer.offset, (20, 20), "{which}");
            assert_eq!(doc.history().1, depth, "{which}");
            assert_eq!(app.status, t!("status-selection-empty"), "{which}");
            assert!(!app.liquify.busy() && !app.camera_raw.busy(), "{which}");
        }
    }

    /// OK of an adjustment puts in the preview's result when it is shown
    /// (nothing computed again on the UI thread); without one, the result
    /// is computed on another thread, then put in and recorded.
    #[test]
    fn adjustment_ok_reuses_its_preview_or_computes_off_the_ui_thread() {
        use crate::pixel_ops::{Adjustment, HueSaturation};
        let hue = Adjustment::HueSaturation(HueSaturation {
            hue: 40.0,
            ..Default::default()
        });
        let mut h = sized(200, 150);
        let expected = {
            let doc = h.state().doc.as_ref().unwrap();
            doc.processed_active(None, |i| hue.apply(i)).unwrap()
        };
        h.state_mut().run(Action::Adjust(AdjustKind::HueSaturation));
        h.state_mut().adjust.as_mut().unwrap().adjustment = hue.clone();
        let wanted = LayerPreview::Adjust(hue.clone());
        settle(&mut h, |app| {
            app.preview_shown
                .as_ref()
                .is_some_and(|p| p.preview == wanted)
        });
        let app = h.state_mut();
        let dialog = app.adjust.take().unwrap();
        app.apply_adjustment(dialog.adjustment);
        assert!(app.filters.adjusting.is_none(), "put in at once");
        assert_eq!(pixels(&h), expected);
        assert_eq!(h.state().doc.as_ref().unwrap().history().1, 1);

        // No preview: on another thread, one step, recorded once in.
        let mut h = sized(200, 150);
        let expected = {
            let doc = h.state().doc.as_ref().unwrap();
            doc.processed_active(None, |i| hue.apply(i)).unwrap()
        };
        let app = h.state_mut();
        app.actions.library.sets = vec![crate::actions::Set {
            name: "S".into(),
            actions: vec![crate::actions::Sequence {
                name: "A".into(),
                key: None,
                steps: Vec::new(),
                expanded: true,
            }],
            expanded: true,
        }];
        app.actions.start_recording((0, 0));
        app.apply_adjustment(hue.clone());
        assert!(
            app.filters.adjusting.is_some(),
            "computed on another thread"
        );
        assert!(app.modal_open(), "the image waits for it");
        settle(&mut h, |app| app.filters.adjusting.is_none());
        assert_eq!(pixels(&h), expected);
        let app = h.state();
        assert_eq!(app.doc.as_ref().unwrap().history().1, 1);
        assert_eq!(app.actions.library.sets[0].actions[0].steps.len(), 1);
    }
}

#[cfg(test)]
#[path = "distort9_tests.rs"]
mod distort9_tests;
