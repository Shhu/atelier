//! Retouching tools: Clone Stamp (S); Spot Healing Brush, Healing Brush,
//! Patch tool and Red Eye tool (J); Content-Aware Fill (Edit menu, and
//! Fill… with Content-Aware contents), computed off the UI thread.

use std::sync::mpsc;

use image::RgbaImage;

use super::dialogs::{DialogSpec, block, floating_dialog, gap_between_blocks, help};
use super::keys;
use super::models_window::{self, AfterDownload};
use super::*;
use crate::ai::models::Model;
use crate::compose;
use crate::paint::StrokeMode;
use crate::retouch::{self, CloneSource, Progress, patchmatch};
use ui_kit::color;

/// Spot Healing Brush: holes of up to this many pixels are filled at once,
/// larger ones on another thread.
const SYNC_HOLE: usize = 600;
/// Healing Brush: a window of more pixels than this is not healed while
/// painting (the cloned pixels show), only once the stroke ends…
const HEAL_LIVE: u64 = 1 << 20;
/// …and a stroke whose box has more is healed on another thread.
const HEAL_ASYNC: u64 = 2 << 20;
/// Holes at least this wide are painted in by the AI model, when it is
/// there (Spot Healing Brush).
const AI_HOLE: u32 = 64;
/// A fill under way shows its progress after this delay (s).
const PROGRESS_DELAY: f64 = 0.3;
/// Segments of the outline drawn while a patch is dragged, at most (beyond,
/// its box).
const MAX_PATCH_SEGMENTS: usize = 20_000;

/// What the Clone Stamp and the Healing Brush copy (Sample).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Sample {
    Current,
    CurrentAndBelow,
    All,
}

impl Sample {
    pub(super) const ALL: [Sample; 3] = [Sample::Current, Sample::CurrentAndBelow, Sample::All];

    pub(super) fn label(self) -> String {
        match self {
            Sample::Current => t!("retouch-sample-current"),
            Sample::CurrentAndBelow => t!("retouch-sample-below"),
            Sample::All => t!("retouch-sample-all"),
        }
    }
}

/// How the Spot Healing Brush fills (Type).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SpotType {
    ContentAware,
    ProximityMatch,
}

/// Patch tool: the selection is patched from where it is dragged
/// (Source), or patches where it is dragged (Destination).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum PatchMode {
    Source,
    Destination,
}

/// Where Content-Aware Fill takes its patches.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SamplingArea {
    /// Around the selection.
    Auto,
    /// The whole layer.
    Layer,
}

/// Where Content-Aware Fill puts its result (Output To).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Output {
    CurrentLayer,
    NewLayer,
}

/// Settings of Content-Aware Fill.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct FillSettings {
    pub area: SamplingArea,
    /// Color Adaptation: the fill takes on the colors around the hole.
    pub adapt: bool,
    pub output: Output,
    /// Painted in by the AI model.
    pub ai: bool,
}

/// A Content-Aware fill asked for: Edit › Content-Aware Fill…, or Fill…
/// (with its opacity and Preserve Transparency).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct FillRequest {
    pub settings: FillSettings,
    pub opacity: f32,
    pub keep_alpha: bool,
    /// From the Fill… window (its name in the history).
    pub from_fill: bool,
}

/// Settings of the retouching tools, and their gestures.
pub(super) struct RetouchTools {
    pub clone_opacity: f32,
    pub clone_flow: f32,
    pub clone_aligned: bool,
    pub clone_sample: Sample,
    pub heal_aligned: bool,
    pub heal_sample: Sample,
    pub spot_type: SpotType,
    pub spot_sample_all: bool,
    pub patch_mode: PatchMode,
    /// Red Eye tool: Pupil Size, Darken Amount (0..1).
    pub pupil: f32,
    pub darken: f32,
    /// Source point (Alt + click), shared by the Clone Stamp and the
    /// Healing Brush, in any open document.
    source: Option<Source>,
    /// The stroke in progress: its box, and the part not healed yet (layer
    /// coordinates); the offset to its source (canvas).
    bounds: Option<DirtyRect>,
    pending: Option<DirtyRect>,
    stroke_offset: Option<(i64, i64)>,
    /// Healing Brush sampling more than the current layer: what was
    /// visible over the layer at the stroke's start (layer coordinates),
    /// the colors the healing meets.
    seen: Option<Arc<RgbaImage>>,
    /// Source shown in the brush (Clone Stamp, Healing Brush): what it
    /// shows, and its texture.
    overlay: Option<(u64, egui::TextureHandle)>,
    /// Patch tool: the selection being dragged, from where to where
    /// (canvas), and its outline.
    patch: Option<PatchDrag>,
    /// Content-Aware Fill window open, and its settings for the session.
    pub(super) dialog: Option<FillSettings>,
    pub(super) settings: FillSettings,
    /// What waits for the AI model to be downloaded, and what it was asked
    /// on (see [`AtelierApp::fill_context`]).
    waiting: Option<(FillRequest, FillContext)>,
    job: Option<FillJob>,
}

impl Default for RetouchTools {
    fn default() -> Self {
        Self {
            clone_opacity: 1.0,
            clone_flow: 1.0,
            clone_aligned: true,
            clone_sample: Sample::Current,
            heal_aligned: false,
            heal_sample: Sample::Current,
            spot_type: SpotType::ContentAware,
            spot_sample_all: false,
            patch_mode: PatchMode::Source,
            pupil: 0.5,
            darken: 0.5,
            source: None,
            bounds: None,
            pending: None,
            stroke_offset: None,
            seen: None,
            overlay: None,
            patch: None,
            dialog: None,
            settings: FillSettings {
                area: SamplingArea::Auto,
                adapt: true,
                output: Output::CurrentLayer,
                ai: true,
            },
            waiting: None,
            job: None,
        }
    }
}

impl RetouchTools {
    /// A fill under way: it keeps the image and the keyboard.
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// The AI model is in use.
    pub(super) fn ai_busy(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.ai)
    }
}

/// The source point, in a document (Current Layer samples its active
/// layer when the stroke starts).
#[derive(Clone, Copy)]
struct Source {
    clone: CloneSource,
    doc: u64,
}

struct PatchDrag {
    from: Pos2,
    to: Pos2,
    outline: Vec<Edge>,
}

/// How a hole is filled.
#[derive(Clone, Copy, PartialEq)]
enum Method {
    Patches,
    Proximity,
    Ai,
}

/// A fill to compute: a region of the layer.
struct FillTask {
    /// What the hole is filled from (the layer, or what is visible).
    sample: RgbaImage,
    /// The layer's pixels.
    base: RgbaImage,
    /// Of each pixel, 0..1: the hole.
    coverage: Vec<f32>,
    /// Where patches may come from (`None`: anywhere but the hole).
    allowed: Option<Vec<bool>>,
    adapt: bool,
    opacity: f32,
    keep_alpha: bool,
    method: Method,
    output: Output,
    /// `sample` is what is visible (all layers): the fill meets its colors.
    visible: bool,
}

/// A fill computed on another thread.
struct FillJob {
    doc: u64,
    layer: u64,
    /// Its name (the history step's, the progress window's title).
    name: String,
    /// The region's corner, in the layer.
    at: (u32, u32),
    output: Output,
    ai: bool,
    started: f64,
    progress: Arc<Progress>,
    rx: mpsc::Receiver<Filled>,
}

/// A fill's pixels (`None`: cancelled, nothing to copy, or nothing
/// changed), why the AI model was not used, if it failed, and whether it
/// left the layer as it was.
type Filled = (Option<RgbaImage>, Option<String>, bool);

impl FillTask {
    fn compute(self, progress: &Progress) -> Filled {
        let hole: Vec<bool> = self.coverage.iter().map(|&c| c > 0.0).collect();
        let patches =
            || patchmatch::inpaint(&self.sample, &hole, self.allowed.as_deref(), progress);
        let mut failure = None;
        let filled = match self.method {
            Method::Patches => patches(),
            Method::Proximity => retouch::proximity_fill(&self.sample, &hole).or_else(patches),
            Method::Ai => {
                match crate::ai::catch_panic(|| crate::ai::inpaint::inpaint(&self.sample, &hole)) {
                    Ok(filled) => {
                        // A large hole the model saw reduced: its answer
                        // gives the shapes, patches the finer texture.
                        let size = self.sample.dimensions();
                        let reduction = crate::ai::inpaint::reduction(size, &hole);
                        let levels = reduction.log2().floor() as usize;
                        if levels == 0 {
                            Some(filled)
                        } else {
                            patchmatch::refine(&self.sample, &hole, progress, &filled, levels)
                        }
                    }
                    Err(e) => {
                        failure = Some(e);
                        patches()
                    }
                }
            }
        };
        let Some(filled) = filled.filter(|_| !progress.cancelled()) else {
            return (None, failure, false);
        };
        let out = match self.output {
            Output::CurrentLayer => retouch::blend_fill(
                &self.base,
                &filled,
                &self.coverage,
                self.adapt,
                self.opacity,
                self.keep_alpha,
                self.visible.then_some(&self.sample),
            ),
            // The fill alone, faded by the coverage.
            Output::NewLayer => {
                let whole: Vec<f32> = hole.iter().map(|&h| f32::from(u8::from(h))).collect();
                let mut out =
                    retouch::blend_fill(&self.base, &filled, &whole, self.adapt, 1.0, false, None);
                for (p, &c) in out.pixels_mut().zip(&self.coverage) {
                    p.0[3] = (p.0[3] as f32 * c.min(1.0) * self.opacity).round() as u8;
                    if p.0[3] == 0 {
                        p.0 = [0; 4];
                    }
                }
                out
            }
        };
        // Nothing changes (an empty layer filled from itself…): no step.
        if self.output == Output::CurrentLayer && out == self.base {
            return (None, failure, true);
        }
        (Some(out), failure, false)
    }
}

impl Tool {
    /// Alt + click sets their source.
    pub(super) fn clones(self) -> bool {
        matches!(self, Tool::CloneStamp | Tool::HealingBrush)
    }

    /// Tools with a brush size ([ and ], Alt + right drag).
    pub(super) fn sized_brush(self) -> bool {
        matches!(
            self,
            Tool::Brush
                | Tool::Eraser
                | Tool::Dodge
                | Tool::Burn
                | Tool::Sponge
                | Tool::Blur
                | Tool::Sharpen
                | Tool::Smudge
        ) || self.retouch_brush()
    }

    /// Retouching tools painted with the brush engine.
    pub(super) fn retouch_brush(self) -> bool {
        matches!(
            self,
            Tool::CloneStamp | Tool::HealingBrush | Tool::SpotHealing
        )
    }
}

/// `r` grown by `margin`, within `w` × `h`.
fn grow(r: DirtyRect, margin: u32, (w, h): (u32, u32)) -> DirtyRect {
    let (x0, y0) = (r.x.saturating_sub(margin), r.y.saturating_sub(margin));
    let (x1, y1) = ((r.x + r.w + margin).min(w), (r.y + r.h + margin).min(h));
    DirtyRect {
        x: x0,
        y: y0,
        w: x1.saturating_sub(x0),
        h: y1.saturating_sub(y0),
    }
}

/// Side of the tiles the Healing Brush heals a stroke by, at the end.
const HEAL_TILE: u32 = 256;

/// `r` cut in tiles of `side` (overlapping by 2 pixels, so that each tile's
/// kept edge is healed by its neighbor).
fn tiles(r: DirtyRect, side: u32) -> Vec<DirtyRect> {
    let starts = |from: u32, len: u32| {
        let end = from + len;
        let mut at = vec![from];
        while at.last().is_some_and(|&a| a + side < end) {
            at.push(at.last().expect("not empty") + side - 2);
        }
        at.into_iter().map(move |a| (a, side.min(end - a)))
    };
    starts(r.y, r.h)
        .flat_map(|(y, h)| starts(r.x, r.w).map(move |(x, w)| DirtyRect { x, y, w, h }))
        .collect()
}

/// Whether `a` and `b` (same size) have the same pixels in `r`.
fn same_pixels(a: &RgbaImage, b: &RgbaImage, r: DirtyRect) -> bool {
    a.dimensions() == b.dimensions()
        && (r.y..r.y + r.h).all(|y| {
            let from = ((y * a.width() + r.x) * 4) as usize;
            let to = from + r.w as usize * 4;
            a.as_raw()[from..to] == b.as_raw()[from..to]
        })
}

fn crop(image: &RgbaImage, r: DirtyRect) -> RgbaImage {
    image::imageops::crop_imm(image, r.x, r.y, r.w, r.h).to_image()
}

/// What is visible over `region` (canvas) from the active layer down.
fn composite_below(doc: &Document, region: PixelRect) -> RgbaImage {
    let layers = doc.render_layers();
    let mut below: Vec<Layer> = layers[..=doc.active].to_vec();
    // The groups it is in, holding only what is below it.
    let mut i = doc.active;
    while let Some(group) = doc.parent(i) {
        below.push(layers[group].clone());
        i = group;
    }
    let options = compose::Options {
        held: doc.effects_held(),
        ..Default::default()
    };
    compose::plan(&below, doc.full_rect(), &mut compose::FullSize, &options).render(region)
}

/// The image a source is sampled in, and its corner on the canvas: in
/// `from`, the document of the source, for a stroke on `doc`.
fn sample_image(doc: &Document, from: &Document, sample: Sample) -> (Arc<RgbaImage>, (i64, i64)) {
    let layer = from.active_layer();
    // Over the stroke's layer, or the whole canvas of another document.
    let rect = if from.uid() == doc.uid() {
        layer.rect()
    } else {
        from.full_rect()
    };
    match sample {
        _ if doc.mask_targeted() && from.uid() == doc.uid() => (layer.shared_image(), layer.offset),
        Sample::Current => (layer.shared_image(), layer.offset),
        Sample::CurrentAndBelow => (Arc::new(composite_below(from, rect)), (rect.x, rect.y)),
        Sample::All => (from.composite_shared(rect), (rect.x, rect.y)),
    }
}

impl RetouchTools {
    /// The stroke of a retouching tool starting at `at` (canvas), ready:
    /// the Clone Stamp and the Healing Brush copy from their source.
    pub(super) fn prepare(
        &mut self,
        tool: Tool,
        doc: &Document,
        parked: &[tabs::ParkedDoc],
        stroke: Stroke,
        at: Pos2,
    ) -> Stroke {
        self.bounds = None;
        self.pending = None;
        self.stroke_offset = None;
        self.seen = None;
        let (aligned, sample) = match tool {
            Tool::CloneStamp => (self.clone_aligned, self.clone_sample),
            Tool::HealingBrush => (self.heal_aligned, self.heal_sample),
            _ => return stroke,
        };
        let Some(source) = self.source.as_mut() else {
            return stroke;
        };
        let Some(from) = source_document(doc, parked, source.doc) else {
            return stroke;
        };
        let offset = source.clone.stroke_offset((at.x, at.y), aligned);
        self.stroke_offset = Some(offset);
        let (image, origin) = sample_image(doc, from, sample);
        if tool == Tool::HealingBrush
            && sample != Sample::Current
            && !doc.mask_targeted()
            && doc.quick_mask().is_none()
        {
            let rect = doc.active_layer().rect();
            let same = from.uid() == doc.uid() && origin == (rect.x, rect.y);
            self.seen = Some(match sample {
                _ if same => Arc::clone(&image),
                Sample::All => doc.composite_shared(rect),
                _ => Arc::new(composite_below(doc, rect)),
            });
        }
        let layer = doc.active_layer().offset;
        stroke.with_clone(
            image,
            (offset.0 + layer.0 - origin.0, offset.1 + layer.1 - origin.1),
        )
    }

    /// After dabs that changed `r` (layer coordinates).
    pub(super) fn dabbed(&mut self, r: DirtyRect) {
        self.bounds = Some(self.bounds.map_or(r, |b| b.union(r)));
        self.pending = Some(self.pending.map_or(r, |b| b.union(r)));
    }
}

/// The open document `uid`: the active one or a tab set aside.
fn source_document<'a>(
    doc: &'a Document,
    parked: &'a [tabs::ParkedDoc],
    uid: u64,
) -> Option<&'a Document> {
    if doc.uid() == uid {
        return Some(doc);
    }
    parked.iter().map(|p| &p.doc).find(|d| d.uid() == uid)
}

/// Healing Brush: area `r` of `image` (layer coordinates) healed from the
/// stroke's source.
fn heal_stroke(stroke: &Stroke, image: &mut RgbaImage, r: DirtyRect, seen: Option<&RgbaImage>) {
    let Some(sample) = stroke.clone_source() else {
        return;
    };
    let (coverage, full, w) = (stroke.coverage(), stroke.full_coverage(), image.width());
    let cover = |x: u32, y: u32| (coverage[(y * w + x) as usize] as f32 / full).min(1.0);
    let seen = seen.filter(|s| s.dimensions() == image.dimensions());
    retouch::heal_area(image, stroke.base(), cover, sample, r, seen);
}

/// The Healing Brush's final pass over `window` of `image`, tile by tile,
/// only where the stroke went: each tile meets the pixels healed around it
/// (the box of a long diagonal stroke is most of the image).
fn heal_tiles(stroke: &Stroke, image: &mut RgbaImage, window: DirtyRect, seen: Option<&RgbaImage>) {
    let width = image.width();
    let cover = stroke.coverage();
    for tile in tiles(window, HEAL_TILE) {
        let covered = (tile.y..tile.y + tile.h).any(|y| {
            let row = (y * width) as usize;
            cover[row + tile.x as usize..row + (tile.x + tile.w) as usize]
                .iter()
                .any(|&c| c > 0)
        });
        if covered {
            heal_stroke(stroke, image, tile, seen);
        }
    }
}

/// Color of the Spot Healing Brush's stroke while it is painted, at half
/// opacity: where the fill will go.
const SPOT_COLOR: [u8; 4] = [0, 0, 0, 255];

impl AtelierApp {
    /// What a stroke of a retouching brush paints: the Clone Stamp and the
    /// Healing Brush copy their source (the healing comes after), the Spot
    /// Healing Brush shows where it goes.
    pub(super) fn retouch_stroke_mode(&self, s: &mut BrushSettings) -> StrokeMode {
        match self.tool {
            Tool::CloneStamp => {
                (s.opacity, s.flow) = (self.retouch.clone_opacity, self.retouch.clone_flow);
                s.pressure_opacity = self.paint.pressure_opacity;
                StrokeMode::Clone
            }
            Tool::HealingBrush => StrokeMode::Clone,
            _ => {
                (s.color, s.opacity) = (SPOT_COLOR, 0.5);
                StrokeMode::Paint
            }
        }
    }

    /// The source point, if set in a document still open, and that document.
    fn clone_source(&self) -> Option<(&Source, &Document)> {
        let source = self.retouch.source.as_ref()?;
        let doc = source_document(self.doc.as_ref()?, &self.parked, source.doc)?;
        Some((source, doc))
    }

    /// Alt + click with the Clone Stamp or the Healing Brush: the source.
    pub(super) fn set_clone_source(&mut self, at: Pos2) {
        let Some(doc) = &self.doc else { return };
        self.retouch.source = Some(Source {
            clone: CloneSource::new((at.x, at.y)),
            doc: doc.uid(),
        });
        self.retouch.overlay = None;
        self.status = t!("retouch-status-source-set");
    }

    /// Before a stroke of a retouching tool: refused (with a message) when
    /// the Clone Stamp or the Healing Brush has no source yet.
    pub(super) fn retouch_refuses(&mut self) -> bool {
        if self.tool.clones() && self.clone_source().is_none() {
            let alt_click = keys::combo(&self.ctx, Modifiers::ALT, &t!("hint-key-click"));
            self.status = t!("retouch-status-no-source", keys = alt_click);
            return true;
        }
        false
    }

    /// The Healing Brush heals what the stroke painted since the last time,
    /// and a margin around (once per frame).
    pub(super) fn heal_pending(&mut self) {
        let Some(r) = self.retouch.pending.take() else {
            return;
        };
        if self.tool != Tool::HealingBrush {
            return;
        }
        let (Some(stroke), Some(doc)) = (&self.stroke, &mut self.doc) else {
            return;
        };
        let layer = doc.active_layer_mut();
        let margin = (self.brush_size * 0.25).ceil().max(4.0) as u32;
        let window = grow(r, margin, layer.image().dimensions());
        // A large brush: healed at the end only (seconds per frame
        // otherwise).
        if u64::from(window.w) * u64::from(window.h) > HEAL_LIVE {
            return;
        }
        heal_stroke(
            stroke,
            layer.pixels_mut(),
            window,
            self.retouch.seen.as_deref(),
        );
        self.stroke_dirty = Some(self.stroke_dirty.map_or(window, |d| d.union(window)));
    }

    /// End of a retouching stroke: the Healing Brush heals it whole, the
    /// Spot Healing Brush fills what it covered. `None` once the stroke is
    /// dealt with.
    pub(super) fn finish_retouch(&mut self, stroke: Stroke) -> Option<Stroke> {
        let bounds = self.retouch.bounds.take();
        self.retouch.pending = None;
        self.retouch.stroke_offset = None;
        let seen = self.retouch.seen.take();
        match self.tool {
            Tool::HealingBrush => {
                let (Some(b), Some(doc)) = (bounds, &mut self.doc) else {
                    return Some(stroke);
                };
                let layer = doc.active_layer_mut();
                let size = layer.image().dimensions();
                let window = grow(b, 2, size);
                if u64::from(window.w) * u64::from(window.h) > HEAL_ASYNC && stroke.painted() {
                    self.heal_later(stroke, window, seen);
                    return None;
                }
                heal_tiles(&stroke, layer.pixels_mut(), window, seen.as_deref());
                self.stroke_dirty = Some(self.stroke_dirty.map_or(window, |d| d.union(window)));
                Some(stroke)
            }
            Tool::SpotHealing => {
                self.spot_heal(&stroke, bounds);
                None
            }
            _ => Some(stroke),
        }
    }

    /// Spot Healing Brush: what the stroke covered is filled from around it
    /// and blended in; small spots at once, larger ones on another thread.
    fn spot_heal(&mut self, stroke: &Stroke, bounds: Option<DirtyRect>) {
        let name = self.tool.name();
        let (spot_type, sample_all) = (self.retouch.spot_type, self.retouch.spot_sample_all);
        let Some(doc) = &mut self.doc else { return };
        doc.hold_effects(false);
        let Some(b) = bounds.filter(|_| stroke.painted()) else {
            doc.end_paint();
            doc.discard_step();
            return;
        };
        let layer = doc.active_layer();
        let size = layer.image().dimensions();
        let margin = b.w.max(b.h).max(16);
        let region = grow(b, margin, size);
        let base = crop(stroke.base(), region);
        let visible = sample_all && !doc.mask_targeted();
        let sample = if visible {
            let r = PixelRect {
                x: layer.offset.0 + region.x as i64,
                y: layer.offset.1 + region.y as i64,
                w: region.w,
                h: region.h,
            };
            doc.composite(r, None)
        } else {
            base.clone()
        };
        let (full, cover) = (stroke.full_coverage(), stroke.coverage());
        let coverage: Vec<f32> = (region.y..region.y + region.h)
            .flat_map(|y| (region.x..region.x + region.w).map(move |x| (x, y)))
            .map(|(x, y)| (cover[(y * size.0 + x) as usize] as f32 / full).min(1.0))
            .collect();
        let holes = coverage.iter().filter(|&&c| c > 0.0).count();
        let ai_wanted = spot_type == SpotType::ContentAware
            && b.w.max(b.h) >= AI_HOLE
            && self.retouch.settings.ai
            && Model::Inpaint.ready();
        let method = match spot_type {
            SpotType::ProximityMatch => Method::Proximity,
            SpotType::ContentAware if ai_wanted && ai_usable(&sample) => Method::Ai,
            SpotType::ContentAware => Method::Patches,
        };
        if ai_wanted && method != Method::Ai {
            self.status = t!("fx3-ai-skipped-transparency");
        }
        let task = FillTask {
            sample,
            base,
            coverage,
            allowed: None,
            adapt: true,
            opacity: 1.0,
            keep_alpha: false,
            method,
            output: Output::CurrentLayer,
            visible,
        };
        if method == Method::Ai || (method == Method::Patches && holes > SYNC_HOLE) {
            self.spawn_fill(task, name, (region.x, region.y));
            return;
        }
        let (filled, _, unchanged) = task.compute(&Progress::default());
        let doc = self.doc.as_mut().expect("checked above");
        match filled {
            Some(pixels) => {
                image::imageops::replace(
                    doc.active_layer_mut().pixels_mut(),
                    &pixels,
                    region.x as i64,
                    region.y as i64,
                );
                self.stroke_dirty = Some(self.stroke_dirty.map_or(region, |d| d.union(region)));
                doc.end_paint();
            }
            None => {
                doc.end_paint();
                doc.discard_step();
                self.status = if unchanged {
                    t!("tools4-status-heal-unchanged")
                } else {
                    t!("retouch-status-nothing-to-sample")
                };
            }
        }
    }

    /// A long stroke of a large Healing Brush, healed on another thread:
    /// the layer as painted (the cloned pixels) is healed over `window`,
    /// then put in place like a fill (see `poll_fill`).
    fn heal_later(&mut self, stroke: Stroke, window: DirtyRect, seen: Option<Arc<RgbaImage>>) {
        let Some(doc) = &self.doc else { return };
        let mut image = doc.active_layer().image().clone();
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Healing Brush");
            heal_tiles(&stroke, &mut image, window, seen.as_deref());
            let unchanged = same_pixels(&image, stroke.base(), window);
            let healed = (!unchanged).then(|| crop(&image, window));
            let _ = tx.send((healed, None, unchanged));
            ctx.request_repaint();
        });
        let name = self.tool.name();
        self.watch_fill(
            name,
            (window.x, window.y),
            Output::CurrentLayer,
            false,
            progress,
            rx,
        );
    }

    /// Computes `task` on another thread, for the active layer (its pixels
    /// at `at`); the document stays ready to paint until it is done (see
    /// `poll_fill`).
    fn spawn_fill(&mut self, task: FillTask, name: String, at: (u32, u32)) {
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let shared = Arc::clone(&progress);
        let (output, ai) = (task.output, task.method == Method::Ai);
        std::thread::spawn(move || {
            let _job = crate::diag::job(if ai {
                "Content-Aware Fill (AI)"
            } else {
                "Content-Aware Fill"
            });
            let filled = task.compute(&shared);
            let _ = tx.send(filled);
            ctx.request_repaint();
        });
        self.watch_fill(name, at, output, ai, progress, rx);
    }

    /// The fill computed on another thread, until done.
    fn watch_fill(
        &mut self,
        name: String,
        at: (u32, u32),
        output: Output,
        ai: bool,
        progress: Arc<Progress>,
        rx: mpsc::Receiver<Filled>,
    ) {
        let Some(doc) = &self.doc else { return };
        self.retouch.job = Some(FillJob {
            doc: doc.uid(),
            layer: doc.active_layer().id,
            name,
            at,
            output,
            ai,
            started: self.ctx.input(|i| i.time),
            progress,
            rx,
        });
    }

    /// A fill computed on another thread is done: into the layer, or a new
    /// layer.
    fn poll_fill(&mut self) {
        let Some(job) = &self.retouch.job else { return };
        let (filled, failure, unchanged) = match job.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => (None, None, false),
        };
        let job = self.retouch.job.take().expect("checked above");
        let cancelled = job.progress.cancelled();
        let Some(doc) = self.doc.as_mut().filter(|d| d.uid() == job.doc) else {
            return;
        };
        if job.output == Output::NewLayer {
            if let Some(pixels) = filled {
                let layer = doc.active_layer();
                let at = (
                    layer.offset.0 + job.at.0 as i64,
                    layer.offset.1 + job.at.1 as i64,
                );
                doc.add_layer(job.name.clone(), pixels, at);
            }
        } else if let Some(pixels) = filled.filter(|_| doc.active_layer().id == job.layer) {
            image::imageops::replace(
                doc.active_layer_mut().pixels_mut(),
                &pixels,
                job.at.0 as i64,
                job.at.1 as i64,
            );
            let r = DirtyRect {
                x: job.at.0,
                y: job.at.1,
                w: pixels.width(),
                h: pixels.height(),
            };
            self.stroke_dirty = Some(self.stroke_dirty.map_or(r, |d| d.union(r)));
            doc.hold_effects(false);
            doc.end_paint();
        } else {
            doc.hold_effects(false);
            doc.end_paint();
            doc.discard_step();
        }
        if let Some(error) = failure {
            self.status_failed(tm!("retouch-status-ai-failed", error = error));
        } else if unchanged {
            self.status = t!("tools4-status-heal-unchanged");
        } else if cancelled {
            self.status = t!("retouch-status-cancelled", name = job.name.as_str());
        }
        self.reset_pending();
    }

    // ------------------------------------------------- Content-Aware Fill

    /// Edit › Content-Aware Fill…: its window, over a selection.
    pub(super) fn open_content_aware(&mut self) {
        if self.selection.is_none() {
            self.status = t!("retouch-status-select-first");
            return;
        }
        if self.refuse_if_locked() || self.refuse_hidden_layer() {
            return;
        }
        let mut settings = self.retouch.settings;
        // The model is used when there, unless declined in the window.
        settings.ai &= Model::Inpaint.ready();
        self.retouch.dialog = Some(settings);
    }

    /// Fills the selection of the active layer content-aware, on another
    /// thread: one undo step once done. Asks for the AI model first if it
    /// is wanted but missing.
    pub(super) fn start_content_aware(&mut self, request: FillRequest) {
        if request.settings.ai && !Model::Inpaint.ready() {
            self.retouch.waiting = Some((request, self.fill_context()));
            if self.request_model(AfterDownload::ContentAwareFill) {
                self.retouch.waiting = None;
            } else {
                return;
            }
        }
        if self.refuse_if_locked() || self.refuse_hidden_layer() {
            return;
        }
        // Filled on the layer itself: a text or shape is rasterized first
        // (asked), like with the painting tools.
        let on_layer = request.settings.output == Output::CurrentLayer
            || self.doc.as_ref().is_some_and(Document::mask_targeted);
        if on_layer && self.paint_refuses() {
            return;
        }
        let name = if request.from_fill {
            t!("dialog-fill-title")
        } else {
            t!("retouch-content-aware-fill")
        };
        let Some(selection) = self.selection.clone() else {
            self.status = t!("retouch-status-select-first");
            return;
        };
        let Some(doc) = &mut self.doc else { return };
        let mut output = request.settings.output;
        if doc.mask_targeted() {
            output = Output::CurrentLayer;
        }
        // On the layer itself: one step, the layer ready to receive it.
        if output == Output::CurrentLayer {
            doc.checkpoint(name.clone());
            if !doc.begin_paint() {
                doc.discard_step();
                self.status = self.nothing_to_paint();
                return;
            }
        }
        let Some(layer) = doc.target_layer() else {
            self.status = self.nothing_to_paint();
            return;
        };
        let image = layer.shared_image();
        let size = image.dimensions();
        let clip = selection.clip(&layer, doc.full_rect());
        let hole = clip.as_ref().and_then(|clip| {
            let [x0, y0, x1, y1] = clip.rect;
            let mut b: Option<DirtyRect> = None;
            for y in y0..y1 {
                for x in x0..x1 {
                    if clip.coverage(x, y) > 0 {
                        let r = DirtyRect { x, y, w: 1, h: 1 };
                        b = Some(b.map_or(r, |b| b.union(r)));
                    }
                }
            }
            b
        });
        let (Some(clip), Some(hole)) = (clip, hole) else {
            if output == Output::CurrentLayer {
                doc.end_paint();
                doc.discard_step();
            }
            self.status = t!("status-selection-empty");
            return;
        };
        let region = match request.settings.area {
            SamplingArea::Auto => grow(hole, hole.w.max(hole.h).max(64), size),
            SamplingArea::Layer => DirtyRect {
                x: 0,
                y: 0,
                w: size.0,
                h: size.1,
            },
        };
        let base = crop(&image, region);
        let coverage: Vec<f32> = (region.y..region.y + region.h)
            .flat_map(|y| (region.x..region.x + region.w).map(move |x| (x, y)))
            .map(|(x, y)| clip.coverage(x, y) as f32 / 255.0)
            .collect();
        let method = if request.settings.ai && ai_usable(&base) {
            Method::Ai
        } else {
            Method::Patches
        };
        if request.settings.ai && method != Method::Ai && Model::Inpaint.ready() {
            self.status = t!("fx3-ai-skipped-transparency");
        }
        let task = FillTask {
            sample: base.clone(),
            base,
            coverage,
            allowed: None,
            adapt: request.settings.adapt,
            opacity: request.opacity,
            keep_alpha: request.keep_alpha,
            method,
            output,
            visible: false,
        };
        self.spawn_fill(task, name, (region.x, region.y));
    }

    /// What a fill waiting for the AI model was asked on: the document,
    /// the layer and the selection.
    fn fill_context(&self) -> FillContext {
        let doc = self.doc.as_ref();
        (
            doc.map(|d| (d.uid(), d.active_layer().id)),
            self.selection.clone(),
        )
    }

    /// The AI model has been downloaded for a fill that waits for it: the
    /// fill runs if what it was asked on is still there (else the model is
    /// just ready, for the command again).
    pub(super) fn content_aware_downloaded(&mut self) {
        if let Some((request, context)) = self.retouch.waiting.take() {
            if context == self.fill_context() {
                self.start_content_aware(request);
            } else {
                let name = Model::Inpaint.name();
                self.status = t!("fx3-model-ready-run-again", name = name);
            }
        }
    }

    /// The fill waiting for the AI model goes without it.
    pub(super) fn content_aware_without_ai(&mut self) {
        if let Some((mut request, _)) = self.retouch.waiting.take() {
            request.settings.ai = false;
            self.start_content_aware(request);
        }
    }

    /// The document goes away: a fill waiting for the AI model is dropped.
    pub(super) fn forget_waiting_fill(&mut self) {
        self.retouch.waiting = None;
    }

    /// Content-Aware Fill window, then the progress of a fill under way.
    pub(super) fn retouch_windows(&mut self, ctx: &egui::Context) {
        self.poll_fill();
        if let Some(job) = &self.retouch.job {
            if fill_progress(ctx, job) {
                job.progress.cancel();
            }
            return;
        }
        let Some(mut settings) = self.retouch.dialog else {
            return;
        };
        let ready = Model::Inpaint.ready();
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok_enabled: self.selection.is_some(),
                ..DialogSpec::new(
                    "content_aware_fill",
                    t!("retouch-content-aware-fill"),
                    center,
                )
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("retouch-sampling-area"));
                    let width = ui.available_width();
                    let label = |a: SamplingArea| match a {
                        SamplingArea::Auto => t!("retouch-sampling-auto"),
                        SamplingArea::Layer => t!("retouch-sampling-layer"),
                    };
                    ui_kit::combo(ui, "caf_area", None, &label(settings.area), width, |ui| {
                        for a in [SamplingArea::Auto, SamplingArea::Layer] {
                            ui.selectable_value(&mut settings.area, a, label(a));
                        }
                    });
                });
                gap_between_blocks(ui);
                ui_kit::checkbox(ui, &mut settings.adapt, &t!("retouch-color-adaptation"))
                    .on_hover_text(t!("retouch-color-adaptation-tip"));
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("retouch-output-to"));
                    let width = ui.available_width();
                    let label = |o: Output| match o {
                        Output::CurrentLayer => t!("retouch-output-current"),
                        Output::NewLayer => t!("retouch-output-new"),
                    };
                    ui_kit::combo(
                        ui,
                        "caf_output",
                        None,
                        &label(settings.output),
                        width,
                        |ui| {
                            for o in [Output::CurrentLayer, Output::NewLayer] {
                                ui.selectable_value(&mut settings.output, o, label(o));
                            }
                        },
                    );
                });
                gap_between_blocks(ui);
                let ai = if ready {
                    t!("retouch-use-ai")
                } else {
                    t!(
                        "retouch-use-ai-download",
                        size = models_window::size_text(Model::Inpaint.size())
                    )
                };
                ui_kit::checkbox(ui, &mut settings.ai, &ai);
                help(ui, &t!("retouch-use-ai-hint"));
            },
        );
        self.retouch.dialog = Some(settings);
        if out.ok || out.cancel {
            self.retouch.dialog = None;
        }
        if out.ok {
            self.retouch.settings = settings;
            self.start_content_aware(FillRequest {
                settings,
                opacity: 1.0,
                keep_alpha: false,
                from_fill: false,
            });
        }
    }

    // ------------------------------------------------------------ patch

    /// Patch tool: outside the selection (or with Shift / Alt), a freehand
    /// selection like the Lasso's; dragged from inside it, the patch, on
    /// release.
    pub(super) fn patch_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        double_click: bool,
    ) -> CursorIcon {
        let ip = pointer.map(|p| view.to_image(p));
        if let Some(drag) = &mut self.retouch.patch {
            if let Some(ip) = ip {
                drag.to = ip;
            }
            if !down {
                let drag = self.retouch.patch.take().expect("checked above");
                self.apply_patch(drag.from, drag.to);
            }
            return CursorIcon::Grabbing;
        }
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let over = self.sel.free_lasso.is_none()
            && !shift
            && !alt
            && ip.is_some_and(|p| {
                self.selection
                    .as_ref()
                    .is_some_and(|s| s.contains(p.x, p.y))
            });
        if over
            && let (true, Some(ip), Some(doc), Some(sel)) = (start, ip, &self.doc, &self.selection)
        {
            let outline = sel.outline(doc.full_rect());
            self.retouch.patch = Some(PatchDrag {
                from: ip,
                to: ip,
                outline,
            });
            return CursorIcon::Grabbing;
        }
        if over {
            return CursorIcon::Move;
        }
        self.free_lasso_input(view, pointer, start, down, double_click)
    }

    /// The selection patched from `to` − `from` away (Source), or where it
    /// was dragged patched with it (Destination): one undo step.
    fn apply_patch(&mut self, from: Pos2, to: Pos2) {
        let (dx, dy) = (
            (to.x - from.x).round() as i64,
            (to.y - from.y).round() as i64,
        );
        if (dx, dy) == (0, 0) || self.paint_refuses() {
            return;
        }
        let Some(selection) = self.selection.clone() else {
            return;
        };
        let name = Tool::Patch.name();
        let mode = self.retouch.patch_mode;
        let Some(doc) = &mut self.doc else { return };
        doc.checkpoint(name);
        if !doc.begin_paint() {
            doc.discard_step();
            self.status = self.nothing_to_paint();
            return;
        }
        let canvas = doc.full_rect();
        let (area, offset) = match mode {
            PatchMode::Source => (Some(selection.clone()), (dx, dy)),
            PatchMode::Destination => (
                selection.clone().translate(dx, dy).within(canvas),
                (-dx, -dy),
            ),
        };
        let layer = doc.active_layer_mut();
        let clip = area.as_ref().and_then(|a| a.clip(layer, canvas));
        let Some(clip) = clip else {
            doc.end_paint();
            doc.discard_step();
            return;
        };
        let [x0, y0, x1, y1] = clip.rect;
        let window = grow(
            DirtyRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            },
            1,
            layer.image().dimensions(),
        );
        let base = layer.shared_image();
        let cover = |x: u32, y: u32| clip.coverage(x, y) as f32 / 255.0;
        retouch::heal_area(
            layer.pixels_mut(),
            &base,
            cover,
            (&base, offset),
            window,
            None,
        );
        let changed = !same_pixels(layer.image(), &base, window);
        doc.end_paint();
        if !changed {
            // An empty layer, or a patch from an identical area.
            doc.discard_step();
            return;
        }
        if mode == PatchMode::Destination {
            // Moved with the patch, in its step.
            self.selection = area;
            self.sync_selection();
        }
        self.reset_pending();
    }

    // ---------------------------------------------------------- red eye

    /// Red Eye tool: a click on a red pupil darkens it.
    pub(super) fn red_eye_click(&mut self, at: Pos2) {
        if self.paint_refuses() {
            return;
        }
        let (pupil, darken) = (self.retouch.pupil, self.retouch.darken);
        let name = Tool::RedEye.name();
        let Some(doc) = &mut self.doc else { return };
        doc.checkpoint(name);
        if !doc.begin_paint() {
            doc.discard_step();
            self.status = self.nothing_to_paint();
            return;
        }
        let canvas = doc.full_rect();
        let clip = self
            .selection
            .as_ref()
            .map(|s| s.clip(doc.active_layer(), canvas).unwrap_or([0; 4].into()));
        let layer = doc.active_layer_mut();
        let (x, y) = (
            at.x.floor() as i64 - layer.offset.0,
            at.y.floor() as i64 - layer.offset.1,
        );
        let (w, h) = layer.image().dimensions();
        let radius = (w.min(h) / 20).clamp(16, 400);
        let inside = x >= 0 && y >= 0 && x < w as i64 && y < h as i64;
        let changed = inside
            && retouch::redeye::remove_red_eye(
                layer.pixels_mut(),
                (x as u32, y as u32),
                radius,
                pupil,
                darken,
                clip.as_ref(),
            )
            .is_some();
        doc.end_paint();
        if !changed {
            doc.discard_step();
            self.status = t!("retouch-status-no-red-eye");
        }
    }

    // ---------------------------------------------------------- drawing

    /// Patch being dragged: the selection's outline where it goes. Clone
    /// Stamp and Healing Brush: a crosshair on the source while painting,
    /// the source inside the brush before.
    pub(super) fn draw_retouch_overlay(
        &mut self,
        painter: &Painter,
        view: View,
        hover: Option<Pos2>,
    ) {
        if let Some(drag) = &self.retouch.patch {
            let d = view.to_screen(drag.to) - view.to_screen(drag.from);
            let line = |a: (i64, i64)| view.to_screen(pos2(a.0 as f32, a.1 as f32)) + d;
            if drag.outline.len() <= MAX_PATCH_SEGMENTS {
                for [a, b] in &drag.outline {
                    let (a, b) = (line(*a), line(*b));
                    painter
                        .line_segment([a, b], UiStroke::new(2.0, Color32::from_black_alpha(160)));
                    painter.line_segment([a, b], UiStroke::new(1.0, Color32::WHITE));
                }
            } else if let Some(sel) = &self.selection {
                let r = view.rect_to_screen(to_rect(sel.rect)).translate(d);
                ui_kit::dashed_rect(painter, r, UiStroke::new(1.0, Color32::WHITE), 6.0, 4.0);
            }
            return;
        }
        if !self.tool.clones() {
            return;
        }
        let aligned = match self.tool {
            Tool::CloneStamp => self.retouch.clone_aligned,
            _ => self.retouch.heal_aligned,
        };
        let Some(source) = self.clone_source().map(|(s, _)| s.clone) else {
            return;
        };
        if self.stroke.is_some() {
            if let (Some(at), Some((dx, dy))) = (self.last_paint, self.retouch.stroke_offset) {
                let p = view.to_screen(at + vec2(dx as f32, dy as f32));
                for (w, c) in [(3.0, Color32::from_black_alpha(160)), (1.0, Color32::WHITE)] {
                    painter.line_segment(
                        [p - vec2(8.0, 0.0), p + vec2(8.0, 0.0)],
                        UiStroke::new(w, c),
                    );
                    painter.line_segment(
                        [p - vec2(0.0, 8.0), p + vec2(0.0, 8.0)],
                        UiStroke::new(w, c),
                    );
                }
            }
            return;
        }
        let alt = self.ctx.input(|i| i.modifiers.alt);
        let (Some(pos), false) = (hover, alt) else {
            return;
        };
        let at = view.to_image(pos);
        let (dx, dy) = source.preview_offset((at.x, at.y), aligned);
        let radius = self.brush_size * 0.5;
        if radius * view.zoom < 3.0 {
            return;
        }
        // The source under the brush, all it covers (at most 1024 texels
        // across: a large brush's sampled every few pixels).
        let covered = self.brush_size.ceil() as u32 + 2;
        let corner = (
            (at.x - covered as f32 / 2.0).floor() as i64,
            (at.y - covered as f32 / 2.0).floor() as i64,
        );
        let Some(texture) = self.overlay_texture((corner.0 + dx, corner.1 + dy), covered) else {
            return;
        };
        let min = view.to_screen(pos2(corner.0 as f32, corner.1 as f32));
        let span = covered as f32 * view.zoom;
        // A disc: a fan of triangles around the pointer.
        let mut mesh = egui::Mesh::with_texture(texture);
        let mut vertex = |pos: Pos2| {
            let uv = pos2((pos.x - min.x) / span, (pos.y - min.y) / span);
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv,
                color: Color32::WHITE,
            });
        };
        vertex(pos);
        const SEGMENTS: u32 = 48;
        for i in 0..SEGMENTS {
            let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
            vertex(pos + vec2(a.cos(), a.sin()) * radius * view.zoom);
        }
        for i in 0..SEGMENTS {
            mesh.add_triangle(0, i + 1, (i + 1) % SEGMENTS + 1);
        }
        painter.add(egui::Shape::mesh(mesh));
    }

    /// Texture of the `covered` × `covered` pixels at `corner` (canvas) of
    /// what the source samples (at most 1024 texels across), made again
    /// only when they change.
    fn overlay_texture(&mut self, corner: (i64, i64), covered: u32) -> Option<egui::TextureId> {
        let side = covered.min(1024);
        let step = covered as f32 / side as f32;
        let (_, doc) = self.clone_source()?;
        let sample = match self.tool {
            Tool::CloneStamp => self.retouch.clone_sample,
            _ => self.retouch.heal_sample,
        };
        let layer = doc.active_layer();
        let key = hash_of((
            doc.uid(),
            doc.history_step(),
            layer.id,
            layer.rev(),
            corner,
            covered,
            sample as u8,
        ));
        if self.retouch.overlay.as_ref().is_none_or(|(k, _)| *k != key) {
            let region = PixelRect {
                x: corner.0,
                y: corner.1,
                w: side,
                h: side,
            };
            let at = move |t: u32| (t as f32 * step) as i64;
            let points = (0..side)
                .flat_map(move |y| (0..side).map(move |x| (corner.0 + at(x), corner.1 + at(y))));
            let color = |[r, g, b, a]: [u8; 4]| Color32::from_rgba_unmultiplied(r, g, b, a);
            let pixels: Vec<Color32> = match sample {
                Sample::Current => points
                    .map(|(x, y)| color(layer.pixel_at(x, y).unwrap_or([0; 4])))
                    .collect(),
                _ if covered == side => doc
                    .composite(region, None)
                    .pixels()
                    .map(|p| color(p.0))
                    .collect(),
                // Larger: read in the composite kept for the strokes.
                _ => {
                    let full = doc.full_rect();
                    let seen = doc.composite_shared(full);
                    points
                        .map(|(x, y)| {
                            let (sx, sy) = (x - full.x, y - full.y);
                            let inside = sx >= 0
                                && sy >= 0
                                && sx < seen.width() as i64
                                && sy < seen.height() as i64;
                            match inside {
                                true => color(seen.get_pixel(sx as u32, sy as u32).0),
                                false => Color32::TRANSPARENT,
                            }
                        })
                        .collect()
                }
            };
            let image = ColorImage::new([side as usize, side as usize], pixels);
            let texture = self
                .ctx
                .load_texture("clone_overlay", image, IMAGE_TEX_OPTIONS);
            self.retouch.overlay = Some((key, texture));
        }
        self.retouch.overlay.as_ref().map(|(_, t)| t.id())
    }

    /// Keys of the retouching tools, for the status bar.
    pub(super) fn retouch_hints(&self, ctx: &egui::Context) -> Vec<(String, String)> {
        let click = t!("hint-key-click");
        let alt_click = keys::combo(ctx, Modifiers::ALT, &click);
        let shift_click = keys::combo(ctx, Modifiers::SHIFT, &click);
        let brush = |mut hints: Vec<(String, String)>| {
            hints.push(("[ ]".to_owned(), t!("hint-size")));
            hints.push((
                keys::combo(ctx, Modifiers::SHIFT, "[ ]"),
                t!("hint-hardness"),
            ));
            hints.push((shift_click.clone(), t!("hint-straight-line")));
            hints.push((keys::key(Key::Space), t!("hint-pan")));
            hints
        };
        match self.tool {
            Tool::CloneStamp => brush(vec![
                (alt_click.clone(), t!("retouch-hint-source")),
                ("1…0".to_owned(), t!("hint-opacity")),
                (
                    keys::combo(ctx, Modifiers::SHIFT, "1…0"),
                    t!("paint-hint-flow"),
                ),
            ]),
            Tool::HealingBrush => brush(vec![(alt_click.clone(), t!("retouch-hint-source"))]),
            Tool::SpotHealing => brush(vec![(t!("hint-key-drag"), t!("retouch-hint-spot"))]),
            Tool::Patch => vec![
                (t!("hint-key-drag"), t!("sel-hint-draw")),
                (t!("retouch-key-drag-inside"), t!("retouch-hint-patch")),
                (keys::modifiers(ctx, Modifiers::SHIFT), t!("hint-add")),
                (keys::modifiers(ctx, Modifiers::ALT), t!("hint-subtract")),
            ],
            _ => vec![(click, t!("retouch-hint-red-eye"))],
        }
    }
}

/// Document and layer (uid, id), and selection a fill was asked on.
type FillContext = (Option<(u64, u64)>, Option<Selection>);

/// Whether the AI model can paint `image` in: downloaded, and the image
/// opaque (it knows no transparency).
fn ai_usable(image: &RgbaImage) -> bool {
    Model::Inpaint.ready() && image.pixels().all(|p| p.0[3] == 255)
}

/// While a fill is computed: blocks the window; after a moment, says which
/// with its progress, and offers to cancel. Returns whether Cancel was
/// clicked.
fn fill_progress(ctx: &egui::Context, job: &FillJob) -> bool {
    let elapsed = ctx.input(|i| i.time) - job.started;
    let visible = elapsed >= PROGRESS_DELAY;
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
    let mut cancel = false;
    egui::Modal::new(egui::Id::new("fill_progress"))
        .backdrop_color(Color32::TRANSPARENT)
        .frame(frame)
        .show(ctx, |ui| {
            if !visible {
                return;
            }
            ui.set_width(280.0);
            ui.label(egui::RichText::new(format!("{}…", job.name)).color(color::TEXT));
            ui.add_space(8.0);
            let fraction = job.progress.fraction();
            if job.ai {
                ui.add(egui::Spinner::new().size(16.0).color(color::MUTED));
            } else {
                ui.add(egui::ProgressBar::new(fraction).desired_height(6.0));
            }
            ui.add_space(8.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                cancel =
                    ui_kit::button(ui, &t!("dialog-cancel"), Kind::Normal, None, true).clicked();
            });
        });
    cancel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heal_tiles_cover_the_area_and_overlap() {
        let r = DirtyRect {
            x: 10,
            y: 20,
            w: 600,
            h: 300,
        };
        let tiles = tiles(r, 256);
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                assert!(
                    tiles
                        .iter()
                        .any(|t| x >= t.x && x < t.x + t.w && y >= t.y && y < t.y + t.h)
                );
            }
        }
        assert!(
            tiles
                .iter()
                .all(|t| t.w <= 256 && t.h <= 256 && t.w > 2 && t.h > 2)
        );
        // Neighbors share two columns.
        assert_eq!(tiles[1].x, tiles[0].x + 254);
        assert_eq!(tiles.len(), 3 * 2);
    }
}
