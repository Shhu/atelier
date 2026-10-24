use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use image::{GrayImage, RgbaImage, imageops::FilterType};

use crate::compose;
pub use crate::compose::BlendMode;
use crate::compose::gradient::GradientFill;
use crate::compose::styles::{Effects, Styles, render_styles};
use crate::elements::group_pixels;
use crate::guides::{self, Guide};
use crate::i18n::t;
use crate::mask_ops;
use crate::pixel_ops::{Adjustment, Clip, content_box, mix, over, scale};
use crate::rotate;
pub use crate::selection::{Channel, Selection};
use crate::text::{Rendered, TextData};
use crate::vector::{Path as VectorPath, Shape};

mod artboards;
mod channels;
mod clipboard;
mod color;
mod file_io;
mod layers;
mod masks;
mod native;
mod psd;
mod quick_mask;
mod shared_composite;
mod smart;
mod styles_jobs;
mod transform;
mod vectors;
pub use artboards::{
    ARTBOARD_GAP, Artboard, ArtboardBackground, ArtboardExport, ArtboardFiles, Side,
    artboard_file_stem, artboard_path, existing_artboard_files, windows_device,
};
pub use color::{ProofSetup, to_cmyk};
pub use file_io::{
    ExportFormat, FileError, FileMetadata, MAX_DIMENSION, Opened, PsdResource, check_size,
    file_size, file_thumbnail,
};
pub use file_io::{encode_cmyk, has_cmyk};
pub use file_io::{ImageMetadata, encode_image, write_atomic};
pub use masks::{Mask, NewMask};
pub use native::is_native;
pub use native::{adjustment_from_json, adjustment_to_json};
pub use psd::is_psd;
pub use shared_composite::note_painted;
pub use smart::{
    Contents, EmbeddedIn, Link, LinkFresh, LinkRead, LinkReport, SmartFilter, SmartFilterKind,
    SmartObject, SmartRender,
};
pub use styles_jobs::styles_pending;
pub use transform::{Target as TransformTarget, Transformed, transformed_selection};
pub use vectors::{PathRef, SavedPath};

const MAX_UNDO: usize = 50;

/// Memory the undo history of a document may keep beyond its current state:
/// 1 GiB still holds about ten steps on a 24 MP layer. The last step is
/// always kept. The documents set aside in other tabs share
/// [`TOTAL_HISTORY_BUDGET`] with it.
pub const HISTORY_BUDGET: usize = 1 << 30;

/// Memory the histories of all open documents may keep together.
pub const TOTAL_HISTORY_BUDGET: usize = 2 << 30;

/// Largest layer (in pixels, 1 GiB of RGBA) that merging or painting may
/// create beyond the size of the canvas.
const MAX_LAYER_PIXELS: u64 = 1 << 28;

/// Pixel rectangle in the canvas: x, y, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelRect {
    pub x: i64,
    pub y: i64,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    pub fn right(&self) -> i64 {
        self.x + self.w as i64
    }

    pub fn bottom(&self) -> i64 {
        self.y + self.h as i64
    }

    pub fn union(self, o: PixelRect) -> PixelRect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        let (r, b) = (self.right().max(o.right()), self.bottom().max(o.bottom()));
        PixelRect {
            x,
            y,
            w: (r - x) as u32,
            h: (b - y) as u32,
        }
    }

    pub fn translate(self, dx: i64, dy: i64) -> PixelRect {
        PixelRect {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
    }

    pub fn contains(&self, x: i64, y: i64) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }

    fn contains_rect(&self, o: PixelRect) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }
}

/// What a layer is.
// The settings of curves weigh a few hundred bytes: nothing next to a
// layer's pixels.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum LayerKind {
    /// Pixels (an editable text too: see [`Layer::text`]).
    Pixels,
    /// Adjusts what is below it (or its clipping base).
    Adjustment(Adjustment),
    /// Color or gradient over the whole canvas (through its mask).
    Fill(Fill),
    /// Folder of the layers just below it in the stack (those of greater
    /// [`Layer::depth`]), shown open or closed in the Layers panel.
    Group { expanded: bool },
}

/// Content of a fill layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid([u8; 3]),
    Gradient(GradientFill),
}

/// Styles drawn for a revision of the pixels and of the mask (its
/// fingerprint), and given settings.
#[derive(Clone)]
struct StylesCache {
    rev: u64,
    mask: u64,
    effects: Effects,
    styles: Arc<Styles>,
    /// The layer's place when they were last shown for its pixels (they
    /// stay there for other pixels, see `Layer::placed_styles`).
    offset: std::cell::Cell<(i64, i64)>,
    /// Part of a move of its shape since then not yet in `offset`, under
    /// a pixel (see `Layer::carry_styles`).
    moved: std::cell::Cell<(f32, f32)>,
}

/// An element of the composition. Its pixels may extend beyond the canvas:
/// they are never cut, only hidden on export.
#[derive(Clone)]
pub struct Layer {
    /// Stable identifier (used to associate a display texture).
    pub id: u64,
    pub name: String,
    pub kind: LayerKind,
    /// Pixels, shared with the history as long as they don't change.
    /// Modifiable only through [`Layer::pixels_mut`] / [`Layer::replace_pixels`].
    /// Empty for the layers without pixels (groups, adjustments, fills).
    image: Arc<RgbaImage>,
    /// Pixel revision: changes on every modification (cache key).
    rev: u64,
    /// Box of the visible pixels (layer coordinates) for a revision.
    bounds: Cell<Option<(u64, Option<[u32; 4]>)>>,
    styles_cache: RefCell<Option<StylesCache>>,
    /// Styles being drawn for the display (see [`Layer::styles_later`]).
    styles_job: styles_jobs::JobSlot,
    /// Position of the top-left corner in the canvas.
    pub offset: (i64, i64),
    pub visible: bool,
    pub opacity: f32,
    /// Fill opacity: like the opacity, but the styles keep theirs.
    pub fill: f32,
    /// Locked: neither editable nor movable, and ignored by clicks on the image.
    pub locked: bool,
    pub blend: BlendMode,
    /// Layer styles (shadow, stroke, glows…), non-destructive.
    pub effects: Effects,
    pub mask: Option<Mask>,
    /// Clipped to the layer below (clipping mask): shown only where that one
    /// (the first one below that isn't clipped) is.
    pub clipped: bool,
    /// Number of groups it is in.
    pub depth: u32,
    /// Layers linked together move together (Photoshop's chain).
    pub link: Option<u64>,
    /// Still editable text layer: its image is the rendering of this text.
    /// Goes back to `None` (ordinary pixels) as soon as its pixels are modified.
    pub text: Option<TextData>,
    /// Shape layer: its image is the rendering of this shape (layer
    /// coordinates: it moves with the layer). Like a text, it becomes pixels
    /// as soon as they are modified.
    pub shape: Option<Box<Shape>>,
    /// Artboard: a top-level group shown as a board of its own (see
    /// `artboards.rs`).
    pub artboard: Option<Artboard>,
    /// Smart object: its image is the rendering of its contents through
    /// its transform and smart filters (see `smart.rs`).
    pub smart: Option<Box<SmartObject>>,
}

impl Layer {
    pub fn rect(&self) -> PixelRect {
        PixelRect {
            x: self.offset.0,
            y: self.offset.1,
            w: self.image.width(),
            h: self.image.height(),
        }
    }

    pub fn image(&self) -> &RgbaImage {
        &self.image
    }

    pub fn is_group(&self) -> bool {
        matches!(self.kind, LayerKind::Group { .. })
    }

    /// A layer of pixels (or text), as opposed to groups, adjustments and
    /// fills.
    pub fn has_pixels(&self) -> bool {
        self.kind == LayerKind::Pixels
    }

    /// The mask, if it is on.
    pub fn active_mask(&self) -> Option<&Mask> {
        self.mask.as_ref().filter(|m| m.enabled)
    }

    /// Styles drawn from the layer's shape (its pixels, through its mask),
    /// computed once per revision and settings. `held`: during a brush
    /// stroke, the last ones drawn with the same settings are kept even if
    /// the pixels changed (they catch up on release).
    pub fn styles(&self, held: bool) -> Option<Arc<Styles>> {
        if !self.has_pixels() || !self.effects.rendered() {
            return None;
        }
        let mask = self.mask_key();
        if let Some(c) = self.styles_cache.borrow().as_ref()
            && c.effects == self.effects
            && (held || (c.rev, c.mask) == (self.rev, mask))
        {
            return Some(self.placed_styles(c));
        }
        let shape = match self.active_mask() {
            Some(m) => Cow::Owned(m.apply_to(&self.image, self.offset)),
            None => Cow::Borrowed(&*self.image),
        };
        let styles = Arc::new(Styles {
            version: next_rev(),
            ..render_styles(&shape, &self.effects)
        });
        *self.styles_cache.borrow_mut() = Some(StylesCache {
            rev: self.rev,
            mask,
            effects: self.effects.clone(),
            styles: Arc::clone(&styles),
            offset: self.offset.into(),
            moved: Default::default(),
        });
        Some(styles)
    }

    /// The styles as drawn with `later` (the display, see
    /// [`Self::styles_later`]), or at once.
    pub fn styles_shown(&self, held: bool, later: bool) -> Option<Arc<Styles>> {
        if later {
            self.styles_later(held)
        } else {
            self.styles(held)
        }
    }

    /// Fingerprint of what the mask does to the layer's shape.
    fn mask_key(&self) -> u64 {
        self.active_mask().map_or(0, |m| m.key(self.offset))
    }

    /// The shared pixels (without copy), to keep as they are while the layer
    /// is being modified (brush stroke).
    pub fn shared_image(&self) -> Arc<RgbaImage> {
        Arc::clone(&self.image)
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Mutable pixels. They are copied only if shared with the history; the
    /// revision changes, and a text becomes pixels.
    pub fn pixels_mut(&mut self) -> &mut RgbaImage {
        self.text = None;
        self.shape = None;
        self.smart = None;
        self.rev = next_rev();
        Arc::make_mut(&mut self.image)
    }

    /// Runs `f` on the mutable pixels (see [`Self::pixels_mut`]); when it
    /// changed nothing (`None`: a brush that laid no dab), the revision
    /// stays, so nothing is composited again.
    pub fn paint<R>(&mut self, f: impl FnOnce(&mut RgbaImage) -> Option<R>) -> Option<R> {
        let rev = self.rev;
        let out = f(self.pixels_mut());
        if out.is_none() {
            self.rev = rev;
        }
        out
    }

    /// Replaces the pixels (a text becomes pixels).
    pub fn replace_pixels(&mut self, image: RgbaImage) {
        self.text = None;
        self.shape = None;
        self.smart = None;
        self.rev = next_rev();
        self.image = Arc::new(image);
    }

    /// Puts back pixels the layer had (`rev` their revision) at `offset`:
    /// a layer enlarged for a dialog's preview, the dialog cancelled.
    pub fn put_back_pixels(&mut self, image: Arc<RgbaImage>, offset: (i64, i64), rev: u64) {
        self.image = image;
        self.offset = offset;
        self.rev = rev;
    }

    /// Box of the visible pixels (alpha > 0), in canvas coordinates.
    /// Computed once per pixel revision.
    pub fn content_bounds(&self) -> Option<PixelRect> {
        let local = match self.bounds.get() {
            Some((rev, b)) if rev == self.rev => b,
            _ => {
                let b = content_box(&self.image);
                self.bounds.set(Some((self.rev, b)));
                b
            }
        };
        local.map(|[x, y, w, h]| PixelRect {
            x: self.offset.0 + x as i64,
            y: self.offset.1 + y as i64,
            w,
            h,
        })
    }

    /// Pixel (x, y) of the canvas, `None` outside the layer.
    pub fn pixel_at(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        let (lx, ly) = (x - self.offset.0, y - self.offset.1);
        (lx >= 0 && ly >= 0 && lx < self.image.width() as i64 && ly < self.image.height() as i64)
            .then(|| self.image.get_pixel(lx as u32, ly as u32).0)
    }

    /// Alpha of the canvas pixel (x, y), 0 outside the layer.
    pub fn alpha_at(&self, x: i64, y: i64) -> u8 {
        let (lx, ly) = (x - self.offset.0, y - self.offset.1);
        if lx < 0 || ly < 0 || lx >= self.image.width() as i64 || ly >= self.image.height() as i64 {
            return 0;
        }
        self.image.get_pixel(lx as u32, ly as u32).0[3]
    }

    /// Moves the layer, and its mask even if unlinked (canvas operations).
    pub fn translate(&mut self, dx: i64, dy: i64) {
        self.offset = (self.offset.0 + dx, self.offset.1 + dy);
        if let Some(m) = &mut self.mask
            && !m.linked()
        {
            m.shift(dx, dy);
        }
    }

    /// Runs `f` on the mask as if it were unlinked (its origin in canvas
    /// coordinates), the layer having moved from `old` to its offset: a
    /// linked mask stays linked.
    fn edit_mask(&mut self, old: (i64, i64), f: impl FnOnce(&mut Mask)) {
        let offset = self.offset;
        if let Some(m) = &mut self.mask {
            let linked = m.linked();
            m.set_linked(false, old);
            f(m);
            m.set_linked(linked, offset);
        }
    }

    /// Box of what the layer shows on its own, styles included; `None` if
    /// it reaches everywhere (adjustment, fill without a hiding mask).
    pub fn extent(&self, held: bool) -> Option<PixelRect> {
        self.extent_with(self.styles(held).as_deref())
    }

    /// The same, with these styles drawn (see [`Self::styles_later`]).
    pub fn extent_with(&self, styles: Option<&Styles>) -> Option<PixelRect> {
        let own = match &self.kind {
            LayerKind::Pixels => {
                let r = self.rect();
                Some(match styles.and_then(|s| s.extent()) {
                    Some([x0, y0, x1, y1]) => r.union(PixelRect {
                        x: self.offset.0 + x0,
                        y: self.offset.1 + y0,
                        w: (x1 - x0) as u32,
                        h: (y1 - y0) as u32,
                    }),
                    None => r,
                })
            }
            _ => None,
        };
        // A mask that hides by default limits the layer to its box.
        match (self.active_mask().filter(|m| m.default == 0), own) {
            (Some(m), Some(r)) => {
                Some(intersect(r, m.rect(self.offset)).unwrap_or(PixelRect { w: 0, h: 0, ..r }))
            }
            (Some(m), None) => Some(m.rect(self.offset)),
            (None, own) => own,
        }
    }
}

/// Stacks the visible `layers` (a stack on their own) onto `out`, which
/// represents `region` of the canvas (gradient fills span `region`);
/// `held`: styles held during a brush stroke.
pub fn composite_layers<'a>(
    out: &mut RgbaImage,
    region: PixelRect,
    layers: impl IntoIterator<Item = &'a Layer>,
    held: bool,
) {
    let layers: Vec<Layer> = layers.into_iter().cloned().collect();
    let options = compose::Options {
        held,
        ..Default::default()
    };
    let image = compose::plan(&layers, region, &mut compose::FullSize, &options).render(region);
    for (d, s) in out.pixels_mut().zip(image.pixels()) {
        d.0 = over(d.0, s.0);
    }
}

static NEXT_REV: AtomicU64 = AtomicU64::new(1);

/// New revision, unique across the whole program.
/// Whether two selections (or their absence) are the same.
fn same_selection(a: Option<&Selection>, b: Option<&Selection>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.same(b),
        _ => false,
    }
}

impl Layer {
    /// Address and size of each big buffer the layer holds (pixels, masks,
    /// its styles' coverages, a smart object's contents and renderings),
    /// for the history's memory budget.
    fn buffers(&self, out: &mut Vec<(usize, usize)>) {
        let mut raw = |r: &[u8]| out.push((r.as_ptr() as usize, r.len()));
        raw(self.image.as_raw());
        if let Some(m) = &self.mask {
            raw(m.image().as_raw());
        }
        if let Some(c) = self.styles_cache.borrow().as_ref() {
            for f in c.styles.exterior.iter().chain(&c.styles.interior) {
                raw(f.coverage.as_raw());
            }
        }
        if let Some(smart) = &self.smart {
            smart.buffers(out);
        }
    }
}

fn next_rev() -> u64 {
    NEXT_REV.fetch_add(1, Ordering::Relaxed)
}

/// Intersection of two pixel rectangles.
pub fn intersect(a: PixelRect, b: PixelRect) -> Option<PixelRect> {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    (x1 > x0 && y1 > y0).then(|| PixelRect {
        x: x0,
        y: y0,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    })
}

/// The pixels of `layer` over `r` (canvas coordinates), inside `selection`
/// if any, transparent elsewhere.
fn pixels_within(layer: &Layer, r: PixelRect, selection: Option<&Selection>) -> RgbaImage {
    let mut out = RgbaImage::new(r.w, r.h);
    image::imageops::replace(
        &mut out,
        layer.image(),
        layer.offset.0 - r.x,
        layer.offset.1 - r.y,
    );
    if let Some(s) = selection.filter(|s| s.inverted || s.mask.is_some()) {
        for (x, y, p) in out.enumerate_pixels_mut() {
            p.0[3] = scale(p.0[3], s.coverage(r.x + x as i64, r.y + y as i64));
        }
    }
    out
}

/// Modifies the pixels of `clip` one by one (layer coordinates).
fn edit_pixels(layer: &mut Layer, clip: &Clip, mut f: impl FnMut(&mut [u8; 4], u8)) {
    let [x0, y0, x1, y1] = clip.rect;
    let image = layer.pixels_mut();
    for y in y0..y1 {
        for x in x0..x1 {
            let coverage = clip.coverage(x, y);
            if coverage > 0 {
                f(&mut image.get_pixel_mut(x, y).0, coverage);
            }
        }
    }
}

/// Erases a pixel according to the selection coverage.
fn clear(p: &mut [u8; 4], coverage: u8) {
    p[3] = scale(p[3], 255 - coverage);
    if p[3] == 0 {
        *p = [0, 0, 0, 0];
    }
}

/// Area `region` (canvas coordinates) converted to layer coordinates and
/// clamped to its image: (x0, y0, x1, y1 exclusive).
pub fn layer_clip(layer: &Layer, region: PixelRect) -> [u32; 4] {
    let (w, h) = (layer.image.width() as i64, layer.image.height() as i64);
    let clamp = |v: i64, max: i64| v.clamp(0, max) as u32;
    [
        clamp(region.x - layer.offset.0, w),
        clamp(region.y - layer.offset.1, h),
        clamp(region.right() - layer.offset.0, w),
        clamp(region.bottom() - layer.offset.1, h),
    ]
}

/// Why layers could not be merged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeError {
    /// Not enough visible layers.
    Nothing,
    /// One of the layers is locked.
    Locked,
    /// The result would be too large (contents far apart).
    TooLarge,
}

/// Alignment or distribution of layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignOp {
    Left,
    CenterH,
    Right,
    Top,
    CenterV,
    Bottom,
    /// Equal horizontal spacing between the elements (at least 3).
    DistributeH,
    /// Equal vertical spacing between the elements (at least 3).
    DistributeV,
}

/// Complete state, for the history.
#[derive(Clone)]
struct State {
    layers: Vec<Layer>,
    active: usize,
    canvas: (u32, u32),
    guides: Vec<Guide>,
    channels: Vec<Channel>,
    paths: Vec<SavedPath>,
    work_path: Option<VectorPath>,
    icc_profile: Option<Arc<Vec<u8>>>,
    selection: Option<Selection>,
    quick_mask: Option<Arc<GrayImage>>,
    mask_target: Option<u64>,
    filter_mask_target: bool,
    /// Identifies the state (whether it is the one saved).
    id: u64,
}

/// A step of the history: the state on one side of it (before it in the
/// undo list, after it in the redo list), its name (History panel), and
/// its number (in the order recorded, see [`Document::history_mark`]).
#[derive(Clone)]
struct Step {
    state: State,
    name: String,
    seq: u64,
}

/// Position in the history to go back to with [`Document::rollback_to`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryMark {
    /// Steps numbered up to this one were recorded before it.
    seq: u64,
    /// The state at the mark.
    id: u64,
}

/// A composition being edited: a canvas and layers (from the bottom to the
/// top), with undo history.
pub struct Document {
    pub layers: Vec<Layer>,
    pub active: usize,
    canvas: (u32, u32),
    /// File it was opened from or last saved to (as an Atelier document).
    pub path: Option<PathBuf>,
    /// Not saved as an Atelier document since the last change.
    pub modified: bool,
    /// Color profile of the pixels (Edit › Assign Profile, Convert to
    /// Profile), given to the files written; `None`: the working RGB. In
    /// the history like the layers.
    pub icc_profile: Option<Arc<Vec<u8>>>,
    /// View › Proof Setup, Proof Colors and Gamut Warning.
    pub proof: ProofSetup,
    /// Ruler guides (View menu), in the history like the layers.
    pub guides: Vec<Guide>,
    /// Selections saved with Select › Save Selection (Photoshop's alpha
    /// channels), in order: canvas-sized coverages, in the history like the
    /// layers. They follow the canvas (crop, rotation, size).
    pub channels: Vec<Channel>,
    /// Paths saved in the Paths panel, in order, and the Work Path (canvas
    /// coordinates), in the history like the layers.
    pub paths: Vec<SavedPath>,
    pub work_path: Option<VectorPath>,
    next_id: u64,
    /// Number of the last "Layer N" created.
    layer_number: u32,
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    /// Changes on every history change (see [`Self::history_step`]).
    step: u64,
    /// Number of the last step recorded.
    seq: u64,
    /// Changes when the history goes back or sideways (see
    /// [`Self::history_generation`]).
    generation: u64,
    /// The oldest step a full history let go to record the step numbered
    /// here, and whether steps were forgotten before: given back if that
    /// step is discarded (it changed nothing).
    evicted: Option<(u64, Step, bool)>,
    /// Steps were forgotten (a full history): the oldest state kept is no
    /// longer the one opened.
    forgotten: bool,
    /// The redo list a step's recording dropped, with that step's number:
    /// given back if the step is discarded (it changed nothing).
    dropped_redo: Option<(u64, Vec<Step>)>,
    /// Identifies the current state (see `State::id`), and the one saved.
    state_id: u64,
    saved_id: Option<u64>,
    /// The selection, in the history like the layers (the app holds the
    /// live one and records its changes here).
    selection: Option<Selection>,
    /// Quick Mask Mode: the mask edited instead of the selection, the size
    /// of the canvas (white selected). In the history like the layers.
    quick_mask: Option<Arc<GrayImage>>,
    /// Changes whenever the quick mask does (display key).
    quick_mask_rev: u64,
    /// Brush stroke in progress: the effects are not recomputed.
    effects_held: bool,
    /// Unique across the program (layer ids are only unique within a document).
    uid: u64,
    /// Layer whose mask (rather than pixels) the tools edit.
    mask_target: Option<u64>,
    /// The active layer, set aside while an RGBA stand-in of its mask takes
    /// its place (see `masks.rs`).
    swapped: Option<Box<Layer>>,
    /// The last RGBA stand-in of a targeted mask made (see
    /// `Document::target_layer`), for what it was made from.
    stand_in: std::cell::RefCell<Option<masks::StandIn>>,
    /// The targeted mask is the active smart object's smart filter mask.
    filter_mask_target: bool,
    /// The contents of a smart object opened by Edit Contents: saving
    /// gives them back to it.
    pub embedded_in: Option<EmbeddedIn>,
    /// What opening its file (a Photoshop document) had to simplify or
    /// leave out: saving over that file would lose it.
    pub file_notes: Vec<String>,
    /// What its file holds beside the layers (metadata, resolution),
    /// written back when it is saved in that format.
    pub file_metadata: FileMetadata,
}

impl Document {
    pub fn new(image: RgbaImage, path: Option<PathBuf>) -> Self {
        let canvas = image.dimensions();
        let mut doc = Self {
            layers: Vec::new(),
            active: 0,
            canvas,
            path,
            modified: false,
            icc_profile: None,
            proof: ProofSetup::default(),
            guides: Vec::new(),
            channels: Vec::new(),
            paths: Vec::new(),
            work_path: None,
            next_id: 0,
            layer_number: 0,
            undo: VecDeque::new(),
            redo: Vec::new(),
            step: 0,
            seq: 0,
            generation: 0,
            evicted: None,
            forgotten: false,
            dropped_redo: None,
            state_id: 0,
            saved_id: Some(0),
            selection: None,
            quick_mask: None,
            quick_mask_rev: next_rev(),
            effects_held: false,
            uid: next_rev(),
            mask_target: None,
            swapped: None,
            stand_in: Default::default(),
            filter_mask_target: false,
            embedded_in: None,
            file_notes: Vec::new(),
            file_metadata: FileMetadata::default(),
        };
        let layer = doc.make_layer(t!("doc-background-layer-name"), image, (0, 0));
        doc.layers.push(layer);
        doc
    }

    /// Opens an Atelier or Photoshop document, or an image file (its
    /// format read from its content) as a one-layer document.
    pub fn open(path: &Path) -> Result<Opened, FileError> {
        if is_native(path) {
            return Ok(Opened {
                doc: native::read(path)?,
                animated: false,
                notes: Vec::new(),
            });
        }
        if is_psd(path) {
            let psd::Imported { mut doc, notes } = psd::read(path)?;
            doc.file_notes = notes.clone();
            return Ok(Opened {
                doc,
                animated: false,
                notes,
            });
        }
        if crate::camera_raw::raw::is_raw(path) {
            let (image, preview_only) = crate::camera_raw::raw::develop(path)?;
            return Ok(Opened {
                doc: Self::new(image, Some(path.to_path_buf())),
                animated: false,
                notes: preview_only
                    .then(|| t!("raw-note-preview-only"))
                    .into_iter()
                    .collect(),
            });
        }
        let decoded = file_io::read_image(path)?;
        let mut doc = Self::new(decoded.image, Some(path.to_path_buf()));
        doc.icc_profile = decoded.icc_profile.map(Arc::new);
        doc.file_metadata.exif = decoded.exif.map(Arc::new);
        doc.file_metadata.image = Some(Arc::new(decoded.metadata));
        // Not said on opening (the first frame is): kept for saving over
        // the file in its format.
        doc.file_notes = decoded.losses;
        Ok(Opened {
            doc,
            animated: decoded.animated,
            notes: Vec::new(),
        })
    }

    /// What saving over its Photoshop file would lose: what opening it
    /// left out, and what such a file can't hold (translated notes).
    pub fn psd_losses(&self) -> Vec<String> {
        let mut notes = self.file_notes.clone();
        notes.extend(psd::losses(self));
        notes
    }

    /// Saves as an Atelier document (every layer kept), or as a Photoshop
    /// document if `path` names one, which becomes the document's file.
    /// The file at `path`, if any, is only replaced once the new one is
    /// completely written. Returns what a Photoshop document could not keep.
    pub fn save(&mut self, path: &Path) -> Result<Vec<String>, FileError> {
        let mut notes = Vec::new();
        if is_psd(path) {
            file_io::write_atomic(path, |out| {
                notes = psd::write(self, out)?;
                Ok(())
            })?;
        } else {
            let folder = path.parent();
            file_io::write_atomic(path, |out| native::write(self, out, folder))?;
        }
        self.path = Some(path.to_path_buf());
        if notes.is_empty() {
            self.mark_saved();
            self.file_notes.clear();
        } else {
            // The file holds less than the document: still to be saved,
            // whatever undo and redo come back to.
            self.saved_id = None;
            self.modified = true;
        }
        Ok(notes)
    }

    /// Export As: the flattened image, or a Photoshop document with its
    /// layers. The document stays as it is: still to be saved if it was.
    /// Returns what a Photoshop document could not keep.
    pub fn export(
        &self,
        path: &Path,
        format: ExportFormat,
        quality: u8,
    ) -> Result<Vec<String>, FileError> {
        if format == ExportFormat::Psd {
            let mut notes = Vec::new();
            file_io::write_atomic(path, |out| {
                notes = psd::write(self, out)?;
                Ok(())
            })?;
            return Ok(notes);
        }
        let profile = self.icc_profile.as_deref().map(Vec::as_slice);
        file_io::export(&self.flatten(), path, format, quality, profile)?;
        Ok(Vec::new())
    }

    /// Final image: visible layers stacked, cropped to the canvas.
    pub fn flatten(&self) -> RgbaImage {
        self.composite(self.full_rect(), None)
    }

    /// [`Self::flatten`], to run on another thread: the layers are taken
    /// along (their pixels are shared, not copied).
    pub fn flatten_later(&self) -> impl FnOnce() -> RgbaImage + Send + 'static {
        let mut copy = Self::new(RgbaImage::new(0, 0), None);
        copy.canvas = self.canvas;
        copy.layers = self.render_layers().into_owned();
        move || copy.flatten()
    }

    /// Identity of the document, unique across the program.
    pub fn uid(&self) -> u64 {
        self.uid
    }

    /// A copy to work on while this one stays untouched: same file and
    /// layers, no history, an identity of its own.
    pub fn duplicate(&self) -> Self {
        Self {
            layers: self.layers.clone(),
            active: self.active,
            canvas: self.canvas,
            path: self.path.clone(),
            modified: self.modified,
            icc_profile: self.icc_profile.clone(),
            proof: self.proof.clone(),
            guides: self.guides.clone(),
            channels: self.channels.clone(),
            paths: self.paths.clone(),
            work_path: self.work_path.clone(),
            next_id: self.next_id,
            layer_number: self.layer_number,
            undo: VecDeque::new(),
            redo: Vec::new(),
            step: 0,
            seq: 0,
            generation: 0,
            evicted: None,
            forgotten: false,
            dropped_redo: None,
            state_id: 0,
            saved_id: (!self.modified).then_some(0),
            selection: self.selection.clone(),
            quick_mask: self.quick_mask.clone(),
            quick_mask_rev: next_rev(),
            effects_held: false,
            uid: next_rev(),
            mask_target: self.mask_target,
            swapped: self.swapped.clone(),
            stand_in: Default::default(),
            filter_mask_target: self.filter_mask_target,
            // Never gives contents back to a smart object.
            embedded_in: None,
            file_notes: self.file_notes.clone(),
            file_metadata: self.file_metadata.clone(),
        }
    }

    /// Stacks the visible layers (according to their opacity and blend mode)
    /// over the region `region` of the canvas, which may extend beyond it.
    /// `skip`: layer to leave out (preview of its transformation).
    pub fn composite(&self, region: PixelRect, skip: Option<usize>) -> RgbaImage {
        let options = compose::Options {
            held: self.effects_held,
            hidden: skip,
            ..Default::default()
        };
        let layers = self.render_layers();
        compose::plan(&layers, self.full_rect(), &mut compose::FullSize, &options).render(region)
    }

    /// During a brush stroke (`hold`), the effects keep their image from
    /// before the stroke instead of being recomputed on every frame.
    pub fn hold_effects(&mut self, hold: bool) {
        self.effects_held = hold;
    }

    /// Effects held during a brush stroke (see [`Self::hold_effects`]).
    pub fn effects_held(&self) -> bool {
        self.effects_held
    }

    /// File name, or "Untitled".
    pub fn display_name(&self) -> String {
        if let Some(e) = &self.embedded_in {
            return e.name.clone();
        }
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map_or_else(|| t!("doc-untitled"), |n| n.to_string_lossy().into_owned())
    }

    pub fn width(&self) -> u32 {
        self.canvas.0
    }

    pub fn height(&self) -> u32 {
        self.canvas.1
    }

    pub fn full_rect(&self) -> PixelRect {
        PixelRect {
            x: 0,
            y: 0,
            w: self.canvas.0,
            h: self.canvas.1,
        }
    }

    pub fn active_layer(&self) -> &Layer {
        &self.layers[self.active]
    }

    pub fn active_layer_mut(&mut self) -> &mut Layer {
        &mut self.layers[self.active]
    }

    /// Box of the visible pixels of all shown layers.
    pub fn content_bounds(&self) -> Option<PixelRect> {
        (0..self.layers.len())
            .filter(|&i| self.shown(i))
            .filter_map(|i| self.layers[i].content_bounds())
            .reduce(PixelRect::union)
    }

    /// Box of the visible pixels of layer `i`, a group: of its layers.
    pub fn element_bounds(&self, i: usize) -> Option<PixelRect> {
        self.block(i)
            .filter_map(|j| self.layers[j].content_bounds())
            .reduce(PixelRect::union)
    }

    /// Topmost visible and unlocked layer with a non-transparent pixel at
    /// (x, y).
    pub fn pick_layer(&self, x: i64, y: i64) -> Option<usize> {
        self.layers_at(x, y).find(|&i| !self.layers[i].locked)
    }

    /// Visible layers with a non-transparent pixel at (x, y), topmost
    /// first (locked ones too).
    pub fn layers_at(&self, x: i64, y: i64) -> impl Iterator<Item = usize> + '_ {
        (0..self.layers.len()).rev().filter(move |&i| {
            let l = &self.layers[i];
            let masked = l.active_mask().map_or(1.0, |m| m.value_at(x, y, l.offset));
            l.alpha_at(x, y) > 0 && masked > 0.0 && self.shown(i) && self.within_artboard(i, x, y)
        })
    }

    // ------------------------------------------------------------ history

    fn state(&self) -> State {
        let mut layers = self.layers.clone();
        // A mask being edited is recorded as a mask.
        if let Some(real) = &self.swapped {
            layers[self.active] = (**real).clone();
        }
        State {
            layers,
            active: self.active,
            canvas: self.canvas,
            guides: self.guides.clone(),
            channels: self.channels.clone(),
            paths: self.paths.clone(),
            work_path: self.work_path.clone(),
            icc_profile: self.icc_profile.clone(),
            selection: self.selection.clone(),
            quick_mask: self.quick_mask.clone(),
            mask_target: self.mask_target,
            filter_mask_target: self.filter_mask_target,
            id: self.state_id,
        }
    }

    /// Replaces the current state with `s` and returns the old one (without
    /// copy). A mask being edited is given its place back first: what it
    /// was painting is replaced too.
    fn restore(&mut self, s: State) -> State {
        if let Some(real) = self.swapped.take() {
            self.layers[self.active] = *real;
        }
        // Folding a group is not a step: the groups stay as they are shown.
        let folds: Vec<(u64, bool)> = self
            .layers
            .iter()
            .filter_map(|l| match l.kind {
                LayerKind::Group { expanded } => Some((l.id, expanded)),
                _ => None,
            })
            .collect();
        let previous = State {
            layers: std::mem::replace(&mut self.layers, s.layers),
            active: self.active,
            canvas: self.canvas,
            guides: std::mem::replace(&mut self.guides, s.guides),
            channels: std::mem::replace(&mut self.channels, s.channels),
            paths: std::mem::replace(&mut self.paths, s.paths),
            work_path: std::mem::replace(&mut self.work_path, s.work_path),
            icc_profile: std::mem::replace(&mut self.icc_profile, s.icc_profile),
            selection: std::mem::replace(&mut self.selection, s.selection),
            quick_mask: std::mem::replace(&mut self.quick_mask, s.quick_mask),
            mask_target: std::mem::replace(&mut self.mask_target, s.mask_target),
            filter_mask_target: std::mem::replace(
                &mut self.filter_mask_target,
                s.filter_mask_target,
            ),
            id: std::mem::replace(&mut self.state_id, s.id),
        };
        for l in &mut self.layers {
            if let LayerKind::Group { expanded } = &mut l.kind
                && let Some(&(_, fold)) = folds.iter().find(|(id, _)| *id == l.id)
            {
                *expanded = fold;
            }
        }
        self.active = s.active.min(self.layers.len() - 1);
        self.canvas = s.canvas;
        self.quick_mask_rev = next_rev();
        self.modified = self.saved_id != Some(self.state_id);
        previous
    }

    /// Records the current state before a modification named `name` (the
    /// step's name in the History panel).
    pub fn checkpoint(&mut self, name: String) {
        self.push_state(self.state(), name);
    }

    /// Records an already captured state (to add it only if something changed).
    fn push_state(&mut self, state: State, name: String) {
        let _measured = crate::diag::timer("History: record step");
        self.seq += 1;
        self.undo.push_back(Step {
            state,
            name,
            seq: self.seq,
        });
        // The oldest step goes, but stays at hand until another step comes:
        // this one may change nothing (a cancelled dialog).
        self.evicted = None;
        if self.undo.len() > MAX_UNDO
            && let Some(oldest) = self.undo.pop_front()
        {
            self.evicted = Some((self.seq, oldest, self.forgotten));
            self.forgotten = true;
        }
        // Kept until another step comes: this one may change nothing.
        let redo = std::mem::take(&mut self.redo);
        match &self.dropped_redo {
            // A step after the one that dropped them: they come back only
            // if it is discarded too.
            Some(_) if redo.is_empty() => {}
            _ => self.dropped_redo = Some((self.seq, redo)),
        }
        // The step that dropped them was forgotten: nothing can give them back.
        if let Some((seq, _)) = &self.dropped_redo
            && self.undo.front().is_none_or(|s| s.seq > *seq)
        {
            self.dropped_redo = None;
        }
        self.trim_history(HISTORY_BUDGET);
        self.step += 1;
        self.state_id = next_rev();
        self.modified = true;
    }

    /// Memory the history keeps beyond the current state, in bytes: each
    /// buffer (pixels, masks, smart objects' contents and renderings,
    /// channels, quick mask) counts once, however many states share it.
    /// The redo list a step dropped (kept in case it is discarded) counts too.
    pub fn history_bytes(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        self.state_buffers(&mut seen, None);
        let mut total = 0;
        let dropped = self.dropped_redo.iter().flat_map(|(_, steps)| steps);
        let evicted = self.evicted.iter().map(|(_, step, _)| step);
        for state in self.redo.iter().chain(&self.undo).chain(dropped).chain(evicted) {
            total += self.state_buffers(&mut seen, Some(&state.state));
        }
        total
    }

    /// Bytes of the buffers of `state` (the current one with `None`) not
    /// yet `seen`.
    fn state_buffers(
        &self,
        seen: &mut std::collections::HashSet<usize>,
        state: Option<&State>,
    ) -> usize {
        let mut buffers = Vec::new();
        let (layers, channels, quick_mask, selection) = match state {
            Some(s) => (&s.layers, &s.channels, &s.quick_mask, &s.selection),
            None => (
                &self.layers,
                &self.channels,
                &self.quick_mask,
                &self.selection,
            ),
        };
        for l in layers {
            l.buffers(&mut buffers);
        }
        if state.is_none()
            && let Some(real) = &self.swapped
        {
            real.buffers(&mut buffers);
        }
        let grays = channels.iter().map(|c| &c.mask).chain(quick_mask);
        let grays = grays.chain(selection.iter().filter_map(|s| s.mask.as_ref()));
        buffers.extend(grays.map(|g| (g.as_raw().as_ptr() as usize, g.as_raw().len())));
        buffers
            .into_iter()
            .filter(|&(ptr, _)| seen.insert(ptr))
            .map(|(_, len)| len)
            .sum()
    }

    /// Forgets the oldest steps beyond `budget` bytes of history (see
    /// [`Self::history_bytes`]). The last step is always kept. A dropped
    /// redo list goes first: it only comes back if a step changes nothing.
    pub fn trim_history(&mut self, budget: usize) {
        if (self.dropped_redo.is_some() || self.evicted.is_some())
            && self.history_bytes() > budget
        {
            self.dropped_redo = None;
            self.evicted = None;
        }
        let mut seen = std::collections::HashSet::new();
        self.state_buffers(&mut seen, None);
        let mut total = 0;
        for step in &self.redo {
            total += self.state_buffers(&mut seen, Some(&step.state));
        }
        // From the most recent step back: the first one is always kept.
        let mut keep = 0;
        for step in self.undo.iter().rev() {
            total += self.state_buffers(&mut seen, Some(&step.state));
            if keep > 0 && total > budget {
                break;
            }
            keep += 1;
        }
        let drop = self.undo.len() - keep;
        self.undo.drain(..drop);
        self.forgotten |= drop > 0;
    }

    /// Whether the oldest state of the history is no longer the one the
    /// document had when opened (steps forgotten to keep within limits).
    pub fn history_forgotten(&self) -> bool {
        self.forgotten
    }

    /// Identifies the history position: it changes whenever a step is
    /// recorded, undone or redone (tells whether nothing was recorded since).
    pub fn history_step(&self) -> u64 {
        self.step
    }

    /// Identifies the current state: a new one with each change, the same
    /// one back when a step is undone or discarded.
    pub fn state_id(&self) -> u64 {
        self.state_id
    }

    /// Changes when the history goes back or sideways: an undo, a redo (a
    /// click in the History panel), a step discarded or rolled back. Steps
    /// recorded forward leave it as it is: a result computed in the
    /// background still applies after them, but never over a state it was
    /// not computed from (nor wiping the redo list it did not know).
    pub fn history_generation(&self) -> u64 {
        self.generation
    }

    /// The document was just saved: its current state is the saved one.
    pub fn mark_saved(&mut self) {
        self.saved_id = Some(self.state_id);
        self.modified = false;
    }

    /// The current state changed without a new step (a series of edits
    /// joining the last one): it is no longer the one saved.
    pub fn mark_modified(&mut self) {
        self.state_id = next_rev();
        self.modified = true;
    }

    /// Cancels the last recorded step, whose modification turned out to
    /// change nothing (a brush click outside the selection, a cancelled
    /// dialog): the steps it made impossible to redo can be redone again,
    /// and the document is as modified as before it.
    pub fn discard_step(&mut self) {
        if let Some(prev) = self.undo.pop_back() {
            self.restore(prev.state);
            self.step += 1;
            self.generation += 1;
            // The oldest step it made the history let go comes back.
            if self.evicted.as_ref().is_some_and(|(seq, ..)| *seq == prev.seq) {
                let (_, oldest, forgotten) = self.evicted.take().expect("checked");
                self.undo.push_front(oldest);
                self.forgotten = forgotten;
            }
            if let Some((seq, _)) = &self.dropped_redo
                && *seq >= prev.seq
            {
                let (seq, redo) = self.dropped_redo.take().expect("checked");
                if seq == prev.seq {
                    self.redo = redo;
                }
            }
        }
    }

    /// The current position in the history, to give up what is recorded
    /// after it with [`Self::rollback_to`] (a text edit cancelled).
    pub fn history_mark(&self) -> HistoryMark {
        HistoryMark {
            seq: self.seq,
            id: self.state_id,
        }
    }

    /// How many steps were recorded since `mark` (and are still there).
    pub fn steps_since(&self, mark: HistoryMark) -> usize {
        self.undo
            .iter()
            .rev()
            .take_while(|s| s.seq > mark.seq)
            .count()
    }

    /// Discards the steps recorded since `mark` (as [`Self::discard_step`]),
    /// even if the oldest ones were forgotten meanwhile (a full history).
    /// Returns whether the state at the mark is back.
    pub fn rollback_to(&mut self, mark: HistoryMark) -> bool {
        while self.undo.back().is_some_and(|s| s.seq > mark.seq) {
            self.discard_step();
        }
        self.state_id == mark.id
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        let state = self.restore(prev.state);
        self.redo.push(Step { state, ..prev });
        self.dropped_redo = None;
        self.evicted = None;
        self.step += 1;
        self.generation += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let state = self.restore(next.state);
        self.undo.push_back(Step { state, ..next });
        self.dropped_redo = None;
        self.evicted = None;
        self.step += 1;
        self.generation += 1;
        true
    }

    /// Names of the steps of the history, oldest first, and how many of
    /// them are done (the others are undone: redo gets them back).
    pub fn history(&self) -> (Vec<&str>, usize) {
        let done = self.undo.iter().map(|s| s.name.as_str());
        let undone = self.redo.iter().rev().map(|s| s.name.as_str());
        (done.chain(undone).collect(), self.undo.len())
    }

    /// Numbers of the steps of [`Self::history`], in the same order: they
    /// identify a step even when steps before it are added or forgotten.
    pub fn history_numbers(&self) -> Vec<u64> {
        let done = self.undo.iter().map(|s| s.seq);
        done.chain(self.redo.iter().rev().map(|s| s.seq)).collect()
    }

    /// Undoes or redoes steps until `done` of them are done (a click in the
    /// History panel). Returns whether the state changed.
    pub fn go_to_step(&mut self, done: usize) -> bool {
        let mut moved = false;
        while self.undo.len() > done && self.undo() {
            moved = true;
        }
        while self.undo.len() < done && self.redo() {
            moved = true;
        }
        moved
    }

    // ------------------------------------------------------------ selection

    /// The selection as the history knows it.
    pub fn selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }

    /// Records the selection `new` as a step named `name`, if it differs
    /// from the one known. Returns whether a step was recorded.
    pub fn record_selection(&mut self, new: Option<Selection>, name: String) -> bool {
        let equal = match (self.selection.as_ref(), new.as_ref()) {
            (None, None) => true,
            (Some(a), Some(b)) => a.equal(b),
            _ => false,
        };
        if equal {
            self.selection = new;
            return false;
        }
        self.checkpoint(name);
        self.selection = new;
        true
    }

    /// Updates the selection known to the history without a step of its
    /// own (it follows a change already recorded: a crop, a move).
    pub fn sync_selection(&mut self, selection: Option<Selection>) {
        if !same_selection(self.selection.as_ref(), selection.as_ref()) {
            self.selection = selection;
        }
    }

    // ------------------------------------------------------------ layers

    fn make_layer(
        &mut self,
        name: String,
        image: impl Into<Arc<RgbaImage>>,
        offset: (i64, i64),
    ) -> Layer {
        self.next_id += 1;
        Layer {
            id: self.next_id,
            name,
            kind: LayerKind::Pixels,
            image: image.into(),
            rev: next_rev(),
            bounds: Cell::new(None),
            styles_cache: RefCell::new(None),
            styles_job: Default::default(),
            offset,
            visible: true,
            opacity: 1.0,
            fill: 1.0,
            locked: false,
            blend: BlendMode::Normal,
            effects: Effects::default(),
            mask: None,
            clipped: false,
            depth: 0,
            link: None,
            text: None,
            shape: None,
            artboard: None,
            smart: None,
        }
    }

    /// Adds a layer above the active layer and makes it active (New Layer).
    pub fn add_layer(
        &mut self,
        name: String,
        image: impl Into<Arc<RgbaImage>>,
        offset: (i64, i64),
    ) {
        self.add_layer_as(t!("menu-layer-new"), name, image, offset);
    }

    /// [`Self::add_layer`] as the history step `step`.
    fn add_layer_as(
        &mut self,
        step: String,
        name: String,
        image: impl Into<Arc<RgbaImage>>,
        offset: (i64, i64),
    ) {
        self.checkpoint(step);
        let layer = self.make_layer(name, image, offset);
        self.insert_layer(layer);
    }

    /// Adds an image as a layer, centered in the canvas, as the history
    /// step `step` (Paste, Place).
    pub fn add_centered_layer(&mut self, step: String, name: String, image: RgbaImage) {
        let x = (self.canvas.0 as i64 - image.width() as i64) / 2;
        let y = (self.canvas.1 as i64 - image.height() as i64) / 2;
        self.add_layer_as(step, name, image, (x, y));
    }

    /// Pixels of the active layer inside the selection, over its box (canvas
    /// coordinates), transparent elsewhere. Without a selection: its visible
    /// pixels.
    pub fn active_pixels(&self, selection: Option<&Selection>) -> Option<RgbaImage> {
        let layer = self.active_layer();
        let r = match selection {
            Some(s) => s.bounds(self.full_rect())?,
            None => layer.content_bounds()?,
        };
        Some(pixels_within(layer, r, selection))
    }

    /// What Copy takes: [`Self::active_pixels`], or the targeted mask in
    /// gray (inside the selection, else over the whole canvas), like
    /// Photoshop.
    pub fn copied_pixels(&self, selection: Option<&Selection>) -> Option<RgbaImage> {
        if !self.mask_targeted() {
            return self.active_pixels(selection);
        }
        let mask = self.target_layer()?;
        let r = match selection {
            Some(s) => s.bounds(self.full_rect())?,
            None => self.full_rect(),
        };
        Some(pixels_within(&mask, r, selection))
    }

    /// Adds a text layer above the active layer, its origin at `origin`
    /// (canvas coordinates; `rendered` was rendered with its fractional
    /// part as anchor).
    pub fn add_text_layer(&mut self, mut data: TextData, rendered: Rendered, origin: (f32, f32)) {
        let offset = (
            (origin.0 - rendered.anchor.0).round() as i64,
            (origin.1 - rendered.anchor.1).round() as i64,
        );
        data.anchor = rendered.anchor;
        self.add_layer_as(
            t!("xchg-hist-type-layer"),
            data.auto_name(),
            rendered.image,
            offset,
        );
        self.active_layer_mut().text = Some(data);
    }

    /// Replaces the text of the active layer and its rendering, without
    /// touching the history (the caller records the state before a series of
    /// keystrokes). The name follows the text as long as it hasn't been
    /// changed by hand. The text's origin stays where `data.anchor` puts it
    /// (relative to the layer's position).
    pub fn set_active_text(&mut self, mut data: TextData, rendered: Rendered) {
        let layer = self.active_layer_mut();
        if layer
            .text
            .as_ref()
            .is_none_or(|old| layer.name == old.auto_name())
        {
            layer.name = data.auto_name();
        }
        layer.offset.0 += (data.anchor.0 - rendered.anchor.0).round() as i64;
        layer.offset.1 += (data.anchor.1 - rendered.anchor.1).round() as i64;
        data.anchor = rendered.anchor;
        // New rendering of the same text: it stays editable.
        layer.image = Arc::new(rendered.image);
        layer.rev = next_rev();
        layer.text = Some(data);
        self.modified = true;
    }

    /// New empty layer, numbered like Photoshop's ("Layer 1", "Layer 2"…:
    /// a counter per document, skipping names already in use).
    pub fn add_empty_layer(&mut self) {
        let name = loop {
            self.layer_number += 1;
            let name = t!("doc-layer-default-name", number = self.layer_number);
            if self.layers.iter().all(|l| l.name != name) {
                break name;
            }
        };
        self.add_layer(name, RgbaImage::new(self.canvas.0, self.canvas.1), (0, 0));
    }

    /// Splits the active layer into one layer per group of visible pixels
    /// (see [`group_pixels`]). Each new layer is cropped to its content and
    /// keeps its place. Returns the number of elements found; the layer is
    /// not modified if there is only one.
    pub fn split_active(&mut self, gap: u32) -> usize {
        let src = self.active_layer().clone();
        let groups = group_pixels(&src.image, gap);
        if groups.boxes.len() <= 1 {
            return groups.boxes.len();
        }
        self.checkpoint(t!("xchg-hist-split"));

        // Reading order: top to bottom, then left to right.
        let mut order: Vec<usize> = (0..groups.boxes.len()).collect();
        order.sort_by_key(|&g| (groups.boxes[g][1], groups.boxes[g][0]));

        let width = src.image.width() as usize;
        let mut new_layers = Vec::with_capacity(order.len());
        for (n, &g) in order.iter().enumerate() {
            let [bx, by, bw, bh] = groups.boxes[g];
            let label = g as u32 + 1;
            let image = RgbaImage::from_fn(bw, bh, |x, y| {
                let (sx, sy) = (bx + x, by + y);
                if groups.labels[sy as usize * width + sx as usize] == label {
                    *src.image.get_pixel(sx, sy)
                } else {
                    image::Rgba([0, 0, 0, 0])
                }
            });
            let offset = (src.offset.0 + bx as i64, src.offset.1 + by as i64);
            let mut layer = self.make_layer(format!("{} {}", src.name, n + 1), image, offset);
            layer.visible = src.visible;
            layer.opacity = src.opacity;
            layer.locked = src.locked;
            layer.blend = src.blend;
            layer.effects = src.effects.clone();
            new_layers.push(layer);
        }

        // The first element at the top of the stack (so first in the displayed list).
        let count = new_layers.len();
        new_layers.reverse();
        self.layers.splice(self.active..=self.active, new_layers);
        self.active += count - 1;
        count
    }

    /// Whether a layer covering `r` stays within the memory limit.
    fn fits(&self, r: PixelRect) -> bool {
        let canvas = self.canvas.0 as u64 * self.canvas.1 as u64;
        r.w as u64 * r.h as u64 <= MAX_LAYER_PIXELS.max(canvas)
    }

    /// Whether some layer is hidden (Flatten Image discards it).
    pub fn has_hidden_layers(&self) -> bool {
        self.layers.iter().any(|l| !l.visible)
    }

    /// Flattens the image into a single layer the size of the canvas
    /// (Photoshop's Flatten Image): what is outside the canvas is cut and
    /// hidden layers are discarded. Transparent areas stay transparent.
    pub fn flatten_image(&mut self) -> bool {
        if self.layers.is_empty() {
            return false;
        }
        let canvas = self.full_rect();
        let image = self.flatten();
        self.checkpoint(t!("menu-layer-flatten"));
        let name = t!("doc-background-layer-name");
        let layer = self.make_layer(name, image, (canvas.x, canvas.y));
        self.layers = vec![layer];
        self.active = 0;
        true
    }

    /// Paint bucket on the active layer, at the canvas point (x, y).
    /// Returns `false` if nothing changed.
    #[cfg(test)]
    pub fn fill_at(
        &mut self,
        x: i64,
        y: i64,
        color: [u8; 4],
        tolerance: u8,
        within: Option<&Selection>,
        contiguous: bool,
    ) -> bool {
        let style = crate::pixel_ops::BucketStyle {
            sample: None,
            anti_alias: false,
            opacity: 1.0,
            blend: &|d, s, _, _| over(d, s),
        };
        self.fill_at_with(x, y, color, tolerance, within, contiguous, &style, false)
    }

    /// [`Self::fill_at`] painted as `style` says; with `all_layers`, the
    /// colors compared are those seen (not on a mask).
    #[allow(clippy::too_many_arguments)]
    pub fn fill_at_with(
        &mut self,
        x: i64,
        y: i64,
        color: [u8; 4],
        tolerance: u8,
        within: Option<&Selection>,
        contiguous: bool,
        style: &crate::pixel_ops::BucketStyle,
        all_layers: bool,
    ) -> bool {
        let all_layers = all_layers && !self.mask_targeted();
        self.with_target(|doc| {
            doc.fill_target_at(
                x, y, color, tolerance, within, contiguous, style, all_layers,
            )
        })
        .unwrap_or(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn fill_target_at(
        &mut self,
        x: i64,
        y: i64,
        color: [u8; 4],
        tolerance: u8,
        within: Option<&Selection>,
        contiguous: bool,
        style: &crate::pixel_ops::BucketStyle,
        all_layers: bool,
    ) -> bool {
        let canvas = self.full_rect();
        if x < canvas.x || y < canvas.y || x >= canvas.right() || y >= canvas.bottom() {
            return false;
        }
        let before = self.state();
        self.expand_active_to_canvas();
        let visible = all_layers.then(|| self.composite(self.layers[self.active].rect(), None));
        let layer = &mut self.layers[self.active];
        let (lx, ly) = ((x - layer.offset.0) as u32, (y - layer.offset.1) as u32);
        let clip = match within {
            Some(sel) => match sel.clip(layer, canvas) {
                Some(clip) => Some(clip),
                None => {
                    self.restore(before);
                    return false;
                }
            },
            None => None,
        };
        let image = layer.pixels_mut();
        let style = crate::pixel_ops::BucketStyle {
            sample: visible.as_ref(),
            ..*style
        };
        let filled = crate::pixel_ops::flood_fill_with(
            image, lx, ly, color, tolerance, clip, contiguous, &style,
        );
        if filled.is_none() {
            self.restore(before);
            return false;
        }
        self.push_state(before, t!("tool-bucket"));
        true
    }

    /// The pixels of the active layer (of its mask's stand-in when the mask
    /// is targeted, see [`Self::target_layer`]) with `op` applied inside
    /// `within` (blended by its coverage, like Photoshop), or everywhere
    /// without a selection. `None` if the selection does not touch the
    /// layer, or if it has no pixels.
    pub fn processed_active(
        &self,
        within: Option<&Selection>,
        op: impl FnOnce(&mut RgbaImage),
    ) -> Option<RgbaImage> {
        let layer = self.target_layer()?;
        let layer = &*layer;
        let mut out = layer.image().clone();
        let Some(selection) = within else {
            op(&mut out);
            return Some(out);
        };
        let clip = selection.clip(layer, self.full_rect())?;
        let [x0, y0, x1, y1] = clip.rect;
        if x0 >= x1 || y0 >= y1 {
            return None;
        }
        let mut part =
            image::imageops::crop_imm(layer.image(), x0, y0, x1 - x0, y1 - y0).to_image();
        op(&mut part);
        let (row, part_row) = (out.width() as usize * 4, (x1 - x0) as usize * 4);
        let rows = &mut out.as_mut()[y0 as usize * row..y1 as usize * row];
        let part = part.as_raw();
        crate::filters::par_rows(rows, row, |first, band| {
            for (i, line) in band.chunks_exact_mut(row).enumerate() {
                let y = y0 + (first + i) as u32;
                let src = &part[(first + i) * part_row..];
                for x in x0..x1 {
                    let (d, s) = (x as usize * 4, (x - x0) as usize * 4);
                    let from = [line[d], line[d + 1], line[d + 2], line[d + 3]];
                    let to = [src[s], src[s + 1], src[s + 2], src[s + 3]];
                    line[d..d + 4].copy_from_slice(&mix(from, to, clip.coverage(x, y)));
                }
            }
        });
        Some(out)
    }

    /// Makes a color of the active layer transparent (see
    /// [`crate::pixel_ops::color_to_alpha`]), inside `within`. A text
    /// becomes pixels.
    pub fn color_to_alpha_active(
        &mut self,
        key: [u8; 3],
        tolerance: f32,
        softness: f32,
        within: Option<&Selection>,
    ) -> bool {
        let op = |image: &mut RgbaImage| {
            crate::pixel_ops::color_to_alpha(image, key, tolerance, softness);
        };
        self.replace_active(t!("xchg-hist-make-transparent"), within, op)
    }

    /// Applies an image adjustment (levels, curves…) to the active layer,
    /// inside `within`. A text becomes pixels.
    pub fn adjust_active(
        &mut self,
        adjustment: &crate::pixel_ops::Adjustment,
        within: Option<&Selection>,
    ) -> bool {
        !adjustment.is_neutral()
            && self.replace_active(adjustment.title(), within, |image| adjustment.apply(image))
    }

    /// Records the step `name` and replaces the active layer's pixels with
    /// [`Self::processed_active`].
    fn replace_active(
        &mut self,
        name: String,
        within: Option<&Selection>,
        op: impl FnOnce(&mut RgbaImage),
    ) -> bool {
        self.with_target(|doc| {
            let Some(image) = doc.processed_active(within, op) else {
                return false;
            };
            doc.checkpoint(name);
            doc.active_layer_mut().replace_pixels(image);
            true
        })
        .unwrap_or(false)
    }

    /// Color shown at the canvas point (x, y) (all visible layers combined),
    /// if it is not fully transparent.
    pub fn color_at(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        // As shown: styles still being drawn are not waited for.
        let options = compose::Options {
            held: self.effects_held,
            later_styles: true,
            ..Default::default()
        };
        let layers = self.render_layers();
        let p = compose::plan(&layers, self.full_rect(), &mut compose::FullSize, &options)
            .render(PixelRect { x, y, w: 1, h: 1 })
            .get_pixel(0, 0)
            .0;
        (p[3] > 0).then_some(p)
    }

    /// Aligns the layers `indices` (their visible pixels): to the canvas if
    /// there is only one, to each other otherwise. Locked layers don't move.
    /// Returns `false` if nothing moved.
    pub fn align(&mut self, indices: &[usize], op: AlignOp) -> bool {
        let items: Vec<(usize, PixelRect)> = indices
            .iter()
            .filter(|&&i| i < self.layers.len() && !self.layers[i].locked)
            .filter_map(|&i| self.element_bounds(i).map(|b| (i, b)))
            .collect();
        if items.is_empty() {
            return false;
        }
        let reference = if indices.len() == 1 {
            self.full_rect()
        } else {
            items
                .iter()
                .map(|(_, b)| *b)
                .reduce(PixelRect::union)
                .expect("at least one element")
        };
        let center = |a: i64, len: u32| a + len as i64 / 2;
        let mut moves: Vec<(usize, i64, i64)> = Vec::new();
        match op {
            AlignOp::DistributeH | AlignOp::DistributeV => {
                if items.len() < 3 {
                    return false;
                }
                let horizontal = op == AlignOp::DistributeH;
                let pos = |b: &PixelRect| {
                    if horizontal {
                        (b.x, b.w as i64)
                    } else {
                        (b.y, b.h as i64)
                    }
                };
                let mut sorted = items.clone();
                sorted.sort_by_key(|(_, b)| pos(b).0);
                let (first, last) = (pos(&sorted[0].1), pos(&sorted[sorted.len() - 1].1));
                let span = last.0 + last.1 - first.0;
                let total: i64 = sorted.iter().map(|(_, b)| pos(b).1).sum();
                let gap = (span - total) as f64 / (sorted.len() - 1) as f64;
                let mut cursor = first.0 as f64;
                for (i, b) in &sorted {
                    let target = cursor.round() as i64;
                    let delta = target - pos(b).0;
                    moves.push(if horizontal {
                        (*i, delta, 0)
                    } else {
                        (*i, 0, delta)
                    });
                    cursor += pos(b).1 as f64 + gap;
                }
            }
            _ => {
                let r = reference;
                for (i, b) in &items {
                    let (dx, dy) = match op {
                        AlignOp::Left => (r.x - b.x, 0),
                        AlignOp::CenterH => (center(r.x, r.w) - center(b.x, b.w), 0),
                        AlignOp::Right => (r.right() - b.right(), 0),
                        AlignOp::Top => (0, r.y - b.y),
                        AlignOp::CenterV => (0, center(r.y, r.h) - center(b.y, b.h)),
                        AlignOp::Bottom => (0, r.bottom() - b.bottom()),
                        _ => unreachable!(),
                    };
                    moves.push((*i, dx, dy));
                }
            }
        }
        if moves.iter().all(|&(_, dx, dy)| dx == 0 && dy == 0) {
            return false;
        }
        self.checkpoint(match op {
            AlignOp::DistributeH | AlignOp::DistributeV => t!("xchg-hist-distribute"),
            _ => t!("xchg-hist-align"),
        });
        // A group with its layers.
        for (i, dx, dy) in moves {
            for j in self.block(i) {
                self.layers[j].offset.0 += dx;
                self.layers[j].offset.1 += dy;
            }
        }
        true
    }

    /// Puts the visible content of the active layer at the center of the
    /// canvas (plain move: a text stays editable).
    pub fn center_active(&mut self) -> bool {
        let Some(b) = self.element_bounds(self.active) else {
            return false;
        };
        let dx = (self.canvas.0 as i64 - b.w as i64).div_euclid(2) - b.x;
        let dy = (self.canvas.1 as i64 - b.h as i64).div_euclid(2) - b.y;
        if (dx, dy) == (0, 0) {
            return false;
        }
        self.checkpoint(t!("xchg-hist-move"));
        for j in self.block(self.active) {
            self.layers[j].offset.0 += dx;
            self.layers[j].offset.1 += dy;
        }
        true
    }

    /// Fills the active layer with `color` over the canvas, or only inside
    /// `within`, with opacity `opacity` (0..1). With `preserve_transparency`,
    /// each pixel keeps its opacity (Photoshop: "Preserve Transparency").
    /// What extends beyond the canvas is not touched.
    pub fn fill_active(
        &mut self,
        color: [u8; 4],
        within: Option<&Selection>,
        opacity: f32,
        preserve_transparency: bool,
    ) -> bool {
        self.with_target(|doc| doc.fill_target(color, within, opacity, preserve_transparency))
            .unwrap_or(false)
    }

    fn fill_target(
        &mut self,
        color: [u8; 4],
        within: Option<&Selection>,
        opacity: f32,
        preserve_transparency: bool,
    ) -> bool {
        let canvas = self.full_rect();
        let selection = within.cloned().unwrap_or(canvas.into());
        if selection.bounds(canvas).is_none() {
            return false;
        }
        let before = self.state();
        self.expand_active_to_canvas();
        let layer = &mut self.layers[self.active];
        let Some(clip) = selection.clip(layer, canvas) else {
            self.restore(before);
            return false;
        };
        self.push_state(before, t!("xchg-hist-fill"));
        let layer = &mut self.layers[self.active];
        let alpha = (color[3] as f32 * opacity.clamp(0.0, 1.0)).round() as u8;
        let src = [color[0], color[1], color[2], alpha];
        edit_pixels(layer, &clip, |p, coverage| {
            let src = [src[0], src[1], src[2], scale(src[3], coverage)];
            if preserve_transparency {
                let [r, g, b, _] = over([p[0], p[1], p[2], 255], src);
                *p = [r, g, b, p[3]];
            } else {
                *p = over(*p, src);
            }
        });
        true
    }

    /// Makes the selection transparent in the active layer (hidden, in a
    /// targeted mask).
    pub fn clear_region(&mut self, selection: &Selection) -> bool {
        self.with_target(|doc| doc.clear_target(selection))
            .unwrap_or(false)
    }

    fn clear_target(&mut self, selection: &Selection) -> bool {
        let layer = self.active_layer();
        let Some(clip) = selection.clip(layer, self.full_rect()) else {
            return false;
        };
        if clip.rect[0] >= clip.rect[2] || clip.rect[1] >= clip.rect[3] {
            return false;
        }
        self.checkpoint(t!("menu-edit-clear"));
        edit_pixels(self.active_layer_mut(), &clip, clear);
        true
    }

    /// "Remove Background": `mask` (same size as the active layer's image)
    /// becomes its layer mask, within the mask it already has. The layer is
    /// untouched: a text, shape or smart object stays one.
    pub fn mask_active(&mut self, mask: &GrayImage) -> bool {
        let layer = &self.layers[self.active];
        if !layer.has_pixels() || mask.dimensions() != layer.image().dimensions() {
            return false;
        }
        let offset = layer.offset;
        let mut image = mask.clone();
        // A disabled mask hides nothing: a new one replaces it.
        let old = layer.mask.as_ref().filter(|m| m.enabled);
        if let Some(old) = old {
            let (ox, oy) = old.origin(offset);
            let before = old.image();
            for (x, y, p) in image.enumerate_pixels_mut() {
                let (bx, by) = (offset.0 + x as i64 - ox, offset.1 + y as i64 - oy);
                let inside =
                    bx >= 0 && by >= 0 && bx < before.width() as i64 && by < before.height() as i64;
                let v = if inside {
                    before.get_pixel(bx as u32, by as u32).0[0]
                } else {
                    old.default
                };
                p.0[0] = scale(p.0[0], v);
            }
        }
        self.checkpoint(t!("menu-layer-remove-background"));
        let layer = &mut self.layers[self.active];
        match &mut layer.mask {
            Some(m) if m.enabled => {
                m.replace(image, offset, offset);
                m.default = 0;
            }
            _ => layer.mask = Some(Mask::new(image, offset, 0, offset)),
        }
        self.mask_target = Some(layer.id);
        self.filter_mask_target = false;
        true
    }

    /// Ctrl+drag a selection (like Photoshop): the pixels of `region` of the
    /// active layer go into a floating layer just above, which becomes active
    /// and is moved; with `copy`, the original stays in place.
    /// Records the single undo step of the move. Returns the floating
    /// layer's id, `None` if the area contains nothing visible.
    pub fn lift_region(&mut self, selection: &Selection, copy: bool) -> Option<u64> {
        let canvas = self.full_rect();
        let (Some(pixels), Some(region)) = (
            self.active_pixels(Some(selection)),
            selection.bounds(canvas),
        ) else {
            return None;
        };
        if pixels.pixels().all(|p| p.0[3] == 0) {
            return None;
        }
        self.checkpoint(t!("xchg-hist-move"));
        if !copy && let Some(clip) = selection.clip(self.active_layer(), canvas) {
            edit_pixels(self.active_layer_mut(), &clip, clear);
        }
        let source = self.active_layer();
        let (name, opacity, blend) = (source.name.clone(), source.opacity, source.blend);
        let (depth, clipped) = (source.depth, source.clipped);
        let mut layer = self.make_layer(name, pixels, (region.x, region.y));
        (layer.opacity, layer.blend) = (opacity, blend);
        // In the source's group, and clipped like it.
        (layer.depth, layer.clipped) = (depth, clipped);
        let id = layer.id;
        self.active += 1;
        self.layers.insert(self.active, layer);
        Some(id)
    }

    /// Drops the floating layer `id` of [`Self::lift_region`] onto the layer
    /// below, enlarged if needed, without a new undo step. Returns `false`
    /// (and changes nothing) if that layer is no longer there.
    pub fn drop_floating(&mut self, id: u64) -> bool {
        let index = self.layers.iter().position(|l| l.id == id);
        let Some(index) = index.filter(|&i| i > 0) else {
            return false;
        };
        let floating = self.layers.remove(index);
        self.active = index - 1;
        let target = &mut self.layers[self.active];
        let region = target.rect().union(floating.rect());
        if region != target.rect() {
            let mut grown = RgbaImage::new(region.w, region.h);
            image::imageops::replace(
                &mut grown,
                target.image(),
                target.offset.0 - region.x,
                target.offset.1 - region.y,
            );
            target.replace_pixels(grown);
            target.offset = (region.x, region.y);
        }
        let (dx, dy) = (
            (floating.offset.0 - target.offset.0) as u32,
            (floating.offset.1 - target.offset.1) as u32,
        );
        let image = target.pixels_mut();
        for (x, y, p) in floating.image().enumerate_pixels() {
            if p.0[3] > 0 {
                let dst = image.get_pixel_mut(x + dx, y + dy);
                dst.0 = over(dst.0, p.0);
            }
        }
        true
    }

    /// New layer with the pixels of `region` of the active layer, at the same place.
    pub fn layer_from_region(&mut self, selection: &Selection) -> bool {
        let (Some(pixels), Some(region)) = (
            self.active_pixels(Some(selection)),
            selection.bounds(self.full_rect()),
        ) else {
            return false;
        };
        if pixels.pixels().all(|p| p.0[3] == 0) {
            return false;
        }
        let name = t!(
            "doc-layer-selection-name",
            name = self.active_layer().name.as_str()
        );
        let step = t!("menu-layer-via-copy");
        self.add_layer_as(step, name, pixels, (region.x, region.y));
        true
    }

    /// Before painting: enlarges the active layer to cover the whole canvas
    /// (its empty margins are dropped). A text layer becomes a pixel layer.
    /// Returns `true` if the layer changed size; a layer whose content lies
    /// too far from the canvas keeps its size.
    pub fn expand_active_to_canvas(&mut self) -> bool {
        let canvas = self.full_rect();
        let layer = self.active_layer();
        if !layer.has_pixels() {
            return false;
        }
        let (rect, content) = (layer.rect(), layer.content_bounds());
        let u = content.map_or(canvas, |c| c.union(canvas));
        let grow = !rect.contains_rect(canvas) && self.fits(u);
        let layer = &mut self.layers[self.active];
        layer.text = None;
        if !grow {
            return false;
        }
        let mut image = RgbaImage::new(u.w, u.h);
        image::imageops::replace(&mut image, layer.image(), rect.x - u.x, rect.y - u.y);
        layer.replace_pixels(image);
        layer.offset = (u.x, u.y);
        true
    }

    // ------------------------------------------------------------ geometry

    /// Canvas Size: the given rectangle (in current canvas coordinates)
    /// becomes the new canvas. Layers are not modified, only repositioned:
    /// what falls outside the frame stays intact.
    pub fn resize_canvas(&mut self, x: i64, y: i64, width: u32, height: u32) {
        self.recanvas(
            t!("xchg-hist-canvas-size"),
            PixelRect {
                x,
                y,
                w: width,
                h: height,
            },
        );
    }

    /// Crop: [`Self::resize_canvas`] to `r`, as its own history step.
    pub fn crop(&mut self, r: PixelRect) {
        self.recanvas(t!("menu-image-crop"), r);
    }

    fn recanvas(&mut self, name: String, r: PixelRect) {
        let PixelRect {
            x,
            y,
            w: width,
            h: height,
        } = r;
        if width == 0 || height == 0 || ((x, y) == (0, 0) && (width, height) == self.canvas) {
            return;
        }
        self.checkpoint(name);
        for layer in &mut self.layers {
            layer.translate(-x, -y);
        }
        self.map_paths(&mask_ops::Affine([
            1.0, 0.0, -x as f32, 0.0, 1.0, -y as f32,
        ]));
        guides::shift(&mut self.guides, -x as f32, -y as f32);
        self.map_canvas_masks(|m| mask_ops::recanvas(m, x, y, width, height));
        self.canvas = (width, height);
    }

    /// Image Rotation (Image menu): the whole image turns by `degrees`
    /// (clockwise) around the canvas center, and the canvas becomes the box
    /// of the rotated canvas. Quarter turns are exact. Texts become pixels.
    pub fn rotate_canvas(&mut self, degrees: f32) -> bool {
        let degrees = degrees.rem_euclid(360.0);
        if degrees == 0.0 {
            return false;
        }
        self.checkpoint(t!("xchg-hist-rotate-canvas"));
        let (w, h) = (self.canvas.0 as f32, self.canvas.1 as f32);
        let (rw, rh) = rotate::rotated_size(w, h, degrees);
        let canvas = (rw.round().max(1.0) as u32, rh.round().max(1.0) as u32);
        let (s, c) = degrees.to_radians().sin_cos();
        // Clockwise on screen (y goes down).
        let turn = |x: f32, y: f32| (c * x - s * y, s * x + c * y);
        let (old_center, new_center) = (
            (w / 2.0, h / 2.0),
            (canvas.0 as f32 / 2.0, canvas.1 as f32 / 2.0),
        );
        // Shapes and paths turn as vectors (quarter turns exactly).
        let (es, ec) = vectors::sin_cos(degrees);
        let (cx, cy) = old_center;
        let (ox, oy) = (ec * cx - es * cy, es * cx + ec * cy);
        let map = mask_ops::Affine([ec, -es, new_center.0 - ox, es, ec, new_center.1 - oy]);
        let limit = self.shape_limit();
        self.map_paths(&map);
        for layer in &mut self.layers {
            let old = layer.offset;
            let (lw, lh) = match layer.artboard {
                Some(a) => (a.size.0 as f32, a.size.1 as f32),
                None => (layer.image.width() as f32, layer.image.height() as f32),
            };
            let center = (
                layer.offset.0 as f32 + lw / 2.0,
                layer.offset.1 as f32 + lh / 2.0,
            );
            let (dx, dy) = turn(center.0 - old_center.0, center.1 - old_center.1);
            let image = rotate::rotate(&layer.image, degrees, false);
            let (mut iw, mut ih) = (image.width() as f32, image.height() as f32);
            if let Some(a) = &mut layer.artboard {
                // An artboard becomes the box of its turned rectangle.
                let (rw, rh) = rotate::rotated_size(lw, lh, degrees);
                a.size = (rw.round().max(1.0) as u32, rh.round().max(1.0) as u32);
                (iw, ih) = (a.size.0 as f32, a.size.1 as f32);
            }
            layer.offset = (
                (new_center.0 + dx - iw / 2.0).round() as i64,
                (new_center.1 + dy - ih / 2.0).round() as i64,
            );
            if !layer.map_shape(old, &map, limit)
                && !layer.map_smart(old, &map, limit)
                && layer.has_pixels()
            {
                layer.replace_pixels(image);
            }
            layer.effects.turn(turn);
            layer.edit_mask(old, |m| {
                m.rotate(degrees, (old_center, new_center), turn, false, (0, 0));
            });
        }
        guides::rotate(&mut self.guides, degrees, self.canvas, canvas);
        self.map_canvas_masks(|m| mask_ops::rotate_canvas(m, degrees, canvas));
        self.canvas = canvas;
        true
    }

    /// Flip Canvas Horizontal / Vertical: the whole image is mirrored.
    /// Texts become pixels.
    pub fn flip_canvas(&mut self, horizontal: bool) {
        self.checkpoint(if horizontal {
            t!("menu-image-flip-horizontal")
        } else {
            t!("menu-image-flip-vertical")
        });
        guides::flip(&mut self.guides, horizontal, self.canvas);
        self.map_canvas_masks(|m| mask_ops::flip(m, horizontal));
        let (w, h) = (self.canvas.0 as i64, self.canvas.1 as i64);
        let map = mask_ops::Affine(if horizontal {
            [-1.0, 0.0, w as f32, 0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0, 0.0, -1.0, h as f32]
        });
        let limit = self.shape_limit();
        self.map_paths(&map);
        for layer in &mut self.layers {
            let (old, r) = (layer.offset, layer.artboard_rect().unwrap_or(layer.rect()));
            let image = if horizontal {
                layer.offset.0 = w - (r.x + r.w as i64);
                image::imageops::flip_horizontal(&*layer.image)
            } else {
                layer.offset.1 = h - (r.y + r.h as i64);
                image::imageops::flip_vertical(&*layer.image)
            };
            if !layer.map_shape(old, &map, limit)
                && !layer.map_smart(old, &map, limit)
                && layer.has_pixels()
            {
                layer.replace_pixels(image);
            }
            layer
                .effects
                .turn(|x, y| if horizontal { (-x, y) } else { (x, -y) });
            layer.edit_mask(old, |m| m.flip(horizontal, (w, h), (0, 0)));
        }
    }

    /// After a canvas enlargement: the area added around `old` (the former
    /// canvas, in new canvas coordinates) gets `color` on the bottom layer,
    /// like Photoshop's canvas extension color. Part of the same undo step.
    /// Only on a plain pixel layer, as Photoshop only extends a Background:
    /// a text or a layer with effects keeps a transparent extension.
    pub fn fill_canvas_extension(&mut self, old: PixelRect, color: [u8; 4]) {
        let canvas = self.full_rect();
        let plain = |l: &&mut Layer| {
            l.has_pixels() && l.text.is_none() && !l.effects.any() && l.mask.is_none()
        };
        let Some(bottom) = self.layers.first_mut().filter(plain) else {
            return;
        };
        let region = canvas.union(bottom.rect());
        let mut out = RgbaImage::new(region.w, region.h);
        for (x, y, p) in out.enumerate_pixels_mut() {
            let (cx, cy) = (region.x + x as i64, region.y + y as i64);
            let in_canvas = canvas.contains(cx, cy);
            if in_canvas && !old.contains(cx, cy) {
                *p = image::Rgba(color);
            }
        }
        let (dx, dy) = (bottom.offset.0 - region.x, bottom.offset.1 - region.y);
        for (x, y, p) in bottom.image.enumerate_pixels() {
            let dst = out.get_pixel_mut((x as i64 + dx) as u32, (y as i64 + dy) as u32);
            dst.0 = over(dst.0, p.0);
        }
        bottom.replace_pixels(out);
        bottom.offset = (region.x, region.y);
    }

    /// Image Size (Ctrl+Alt+I): the canvas becomes `width` × `height` and
    /// all layers follow (pixels, position, effects). Texts keep their
    /// content, with a scaled font size; the caller can render them again
    /// for a sharp result.
    pub fn resize_image(&mut self, width: u32, height: u32, filter: FilterType) -> bool {
        if width == 0 || height == 0 || (width, height) == self.canvas {
            return false;
        }
        self.checkpoint(t!("xchg-hist-image-size"));
        let (sx, sy) = (
            width as f64 / self.canvas.0 as f64,
            height as f64 / self.canvas.1 as f64,
        );
        let s = (sx * sy).sqrt() as f32;
        let map = mask_ops::Affine([sx as f32, 0.0, 0.0, 0.0, sy as f32, 0.0]);
        let limit = self.shape_limit();
        self.map_paths(&map);
        for layer in &mut self.layers {
            let old = layer.offset;
            let (w, h) = layer.image.dimensions();
            let x0 = (layer.offset.0 as f64 * sx).round() as i64;
            let y0 = (layer.offset.1 as f64 * sy).round() as i64;
            let x1 = ((layer.offset.0 + w as i64) as f64 * sx).round() as i64;
            let y1 = ((layer.offset.1 + h as i64) as f64 * sy).round() as i64;
            let (nw, nh) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
            // Its origin follows the scaled canvas.
            let text = layer.text.take().map(|t| TextData {
                anchor: (
                    ((old.0 as f64 + f64::from(t.anchor.0)) * sx - x0 as f64) as f32,
                    ((old.1 as f64 + f64::from(t.anchor.1)) * sy - y0 as f64) as f32,
                ),
                ..t.scaled(s)
            });
            layer.offset = (x0, y0);
            if let Some(a) = &mut layer.artboard {
                let x1 = ((old.0 + a.size.0 as i64) as f64 * sx).round() as i64;
                let y1 = ((old.1 + a.size.1 as i64) as f64 * sy).round() as i64;
                a.size = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
            }
            if !layer.map_shape(old, &map, limit)
                && !layer.map_smart(old, &map, limit)
                && layer.has_pixels()
            {
                layer.replace_pixels(crate::pixel_ops::resize(layer.image(), nw, nh, filter));
            }
            layer.text = text;
            layer.edit_mask(old, |m| m.scale((sx, sy), filter, (0, 0)));
            layer.effects.scale(s, sx as f32, sy as f32);
        }
        guides::scale(&mut self.guides, sx as f32, sy as f32);
        self.map_canvas_masks(|m| mask_ops::resize(m, width, height));
        self.canvas = (width, height);
        true
    }

    /// Resizes, moves and/or rotates the region `src` of the active layer
    /// according to `place` (a group: all its layers, as one element; a
    /// linked mask goes with its layer). With `fit_canvas`, the canvas
    /// becomes exactly the box of the transformed element.
    pub fn transform_active(
        &mut self,
        src: PixelRect,
        place: Placement,
        filter: FilterType,
        fit_canvas: bool,
    ) {
        let Placement { at, size, angle } = place;
        if size.0 == 0 || size.1 == 0 || src.w == 0 || src.h == 0 {
            return;
        }
        let turns = angle.rem_euclid(360.0);
        let rotating = turns > 1e-3 && turns < 360.0 - 1e-3;
        let layer_unchanged = !rotating && size == (src.w, src.h) && at == (src.x, src.y);
        let canvas_unchanged = !fit_canvas || (!rotating && at == (0, 0) && size == self.canvas);
        if layer_unchanged && canvas_unchanged {
            return;
        }
        self.checkpoint(if !rotating && size == (src.w, src.h) {
            t!("xchg-hist-move")
        } else {
            t!("menu-edit-free-transform")
        });
        let block = self.block(self.active);
        if !layer_unchanged {
            if self.layers[self.active].is_group() {
                // Each of its layers as a part of the whole: scaled from the
                // whole's corner, turned around its center.
                let (sx, sy) = (size.0 as f64 / src.w as f64, size.1 as f64 / src.h as f64);
                let center = (
                    at.0 as f64 + size.0 as f64 / 2.0,
                    at.1 as f64 + size.1 as f64 / 2.0,
                );
                let (sin, cos) = (angle as f64).to_radians().sin_cos();
                for i in block.clone() {
                    let l = &self.layers[i];
                    let Some(part) = l.content_bounds().filter(|_| l.has_pixels()) else {
                        // Without pixels (adjustment, fill, the group
                        // itself): its mask follows the whole. An artboard
                        // only follows a move.
                        if !l.has_pixels() && (l.artboard.is_none() || size == (src.w, src.h)) {
                            self.transform_layer(i, src, place, filter);
                        }
                        continue;
                    };
                    let x0 = at.0 as f64 + (part.x - src.x) as f64 * sx;
                    let y0 = at.1 as f64 + (part.y - src.y) as f64 * sy;
                    let (w, h) = (part.w as f64 * sx, part.h as f64 * sy);
                    let (cx, cy) = (x0 + w / 2.0 - center.0, y0 + h / 2.0 - center.1);
                    let (rx, ry) = (cos * cx - sin * cy, sin * cx + cos * cy);
                    let size = ((w.round() as u32).max(1), (h.round() as u32).max(1));
                    let at = (
                        (center.0 + rx - size.0 as f64 / 2.0).round() as i64,
                        (center.1 + ry - size.1 as f64 / 2.0).round() as i64,
                    );
                    self.transform_layer(i, part, Placement { at, size, angle }, filter);
                }
            } else {
                self.transform_layer(self.active, src, place, filter);
            }
        }
        if fit_canvas {
            // What shows (a turned box has transparent corners).
            let r = block
                .filter(|&i| self.layers[i].has_pixels())
                .map(|i| {
                    let l = &self.layers[i];
                    l.content_bounds().unwrap_or(l.rect())
                })
                .reduce(PixelRect::union);
            if let Some(r) = r {
                self.fit_canvas_to(r);
            }
        }
    }

    /// [`Self::transform_active`] for layer `i` alone.
    fn transform_layer(&mut self, i: usize, src: PixelRect, place: Placement, filter: FilterType) {
        let Placement { at, size, angle } = place;
        let turns = angle.rem_euclid(360.0);
        let rotating = turns > 1e-3 && turns < 360.0 - 1e-3;
        let (map, limit) = (vectors::placement_map(src, place), self.shape_limit());
        let layer = &mut self.layers[i];
        let old = layer.offset;
        let only_moved = !rotating && size == (src.w, src.h);
        if (layer.text.is_some() || layer.smart.is_some()) && only_moved {
            // A text or a smart object that is only moved stays as it is.
            layer.offset.0 += at.0 - src.x;
            layer.offset.1 += at.1 - src.y;
            return;
        }
        if src.w > 0
            && src.h > 0
            && (layer.map_shape(old, &map, limit) || layer.map_smart(old, &map, limit))
        {
            // A shape stays a vector, a smart object smart.
        } else if src.w > 0 && src.h > 0 && layer.has_pixels() {
            let lx = (src.x - layer.offset.0).max(0) as u32;
            let ly = (src.y - layer.offset.1).max(0) as u32;
            let region = image::imageops::crop_imm(layer.image(), lx, ly, src.w, src.h).to_image();
            let scaled = if size == region.dimensions() {
                region
            } else {
                crate::pixel_ops::resize(&region, size.0, size.1, filter)
            };
            if rotating {
                // The center of the element stays in place. A half pixel is
                // dropped toward the frame's corner, the same way in both
                // directions: two quarter turns make an exact half turn.
                let rotated = rotate::rotate(&scaled, angle, filter == FilterType::Nearest);
                let dx = (i64::from(size.0) - i64::from(rotated.width())) / 2;
                let dy = (i64::from(size.1) - i64::from(rotated.height())) / 2;
                layer.offset = (at.0 + dx, at.1 + dy);
                layer.replace_pixels(rotated);
            } else {
                layer.replace_pixels(scaled);
                layer.offset = at;
            }
        } else {
            layer.offset.0 += at.0 - src.x;
            layer.offset.1 += at.1 - src.y;
        }
        // Its linked mask, the same way.
        if layer.mask.as_ref().is_some_and(Mask::linked) && (src.w, src.h) != (0, 0) {
            let center = (
                at.0 as f32 + size.0 as f32 / 2.0,
                at.1 as f32 + size.1 as f32 / 2.0,
            );
            let (s, c) = angle.to_radians().sin_cos();
            let turn = move |x: f32, y: f32| (c * x - s * y, s * x + c * y);
            let nearest = filter == FilterType::Nearest;
            layer.edit_mask(old, |m| {
                m.map(src, (at.0, at.1, size.0, size.1), filter, (0, 0));
                if rotating {
                    m.rotate(angle, (center, center), turn, nearest, (0, 0));
                }
            });
        }
    }

    /// The canvas becomes exactly `r` (canvas coordinates), without a new
    /// undo step.
    pub fn fit_canvas_to(&mut self, r: PixelRect) {
        for layer in &mut self.layers {
            layer.translate(-r.x, -r.y);
        }
        self.map_paths(&mask_ops::Affine([
            1.0,
            0.0,
            -r.x as f32,
            0.0,
            1.0,
            -r.y as f32,
        ]));
        guides::shift(&mut self.guides, -r.x as f32, -r.y as f32);
        self.map_canvas_masks(|m| mask_ops::recanvas(m, r.x, r.y, r.w, r.h));
        self.canvas = (r.w, r.h);
    }
}

/// Where and how to place a transformed element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Top-left corner of the rectangle, before rotation.
    pub at: (i64, i64),
    pub size: (u32, u32),
    /// Rotation in degrees (clockwise) around the center of the rectangle.
    pub angle: f32,
}

#[cfg(test)]
impl Placement {
    pub fn new(at: (i64, i64), size: (u32, u32)) -> Self {
        Self {
            at,
            size,
            angle: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);
    const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

    /// Transparent 10 × 10 canvas with a red 2 × 3 square at (4, 5).
    fn logo() -> Document {
        let mut img = RgbaImage::new(10, 10);
        for (x, y) in [(4, 5), (5, 5), (4, 6), (5, 6), (4, 7), (5, 7)] {
            img.put_pixel(x, y, RED);
        }
        Document::new(img, None)
    }

    #[test]
    fn shrinking_canvas_keeps_pixels_intact() {
        let mut doc = logo();
        doc.resize_canvas(0, 0, 3, 3);
        assert_eq!(doc.flatten().dimensions(), (3, 3));
        assert!(doc.flatten().pixels().all(|p| *p == CLEAR));
        // The content still exists: bring it back into the frame.
        let src = doc.active_layer().content_bounds().unwrap();
        assert_eq!(
            src,
            PixelRect {
                x: 4,
                y: 5,
                w: 2,
                h: 3
            }
        );
        doc.transform_active(
            src,
            Placement::new((0, 0), (2, 3)),
            FilterType::Nearest,
            false,
        );
        assert_eq!(*doc.flatten().get_pixel(1, 2), RED);
    }

    #[test]
    fn growing_canvas_adds_transparency() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, RED), None);
        doc.resize_canvas(-2, -1, 8, 6);
        let flat = doc.flatten();
        assert_eq!(flat.dimensions(), (8, 6));
        assert_eq!(*flat.get_pixel(0, 0), CLEAR);
        assert_eq!(*flat.get_pixel(2, 1), RED);
        assert!(doc.undo());
        assert_eq!(doc.flatten().dimensions(), (4, 4));
    }

    #[test]
    fn content_bounds_ignores_transparent_pixels() {
        assert_eq!(
            logo().content_bounds(),
            Some(PixelRect {
                x: 4,
                y: 5,
                w: 2,
                h: 3
            })
        );
        assert_eq!(
            Document::new(RgbaImage::new(3, 3), None).content_bounds(),
            None
        );
    }

    #[test]
    fn transform_with_fit_canvas_becomes_whole_image() {
        let mut doc = logo();
        let src = doc.content_bounds().unwrap();
        doc.transform_active(
            src,
            Placement::new((4, 5), (4, 6)),
            FilterType::Nearest,
            true,
        );
        let flat = doc.flatten();
        assert_eq!(flat.dimensions(), (4, 6));
        assert!(flat.pixels().all(|p| *p == RED));
    }

    #[test]
    fn rotation_keeps_element_centered() {
        // Horizontal red 4 × 2 bar at (3, 4): center (5, 5).
        let mut img = RgbaImage::new(10, 10);
        for x in 3..7 {
            for y in 4..6 {
                img.put_pixel(x, y, RED);
            }
        }
        let mut doc = Document::new(img, None);
        let src = doc.content_bounds().unwrap();
        let place = Placement {
            at: (3, 4),
            size: (4, 2),
            angle: 90.0,
        };
        doc.transform_active(src, place, FilterType::Nearest, false);
        // Now vertical 2 × 4, still centered at (5, 5).
        assert_eq!(
            doc.content_bounds(),
            Some(PixelRect {
                x: 4,
                y: 3,
                w: 2,
                h: 4
            })
        );
        assert!(doc.undo());

        // Arbitrary angle with fitted canvas: the canvas encloses the rotated element.
        let place = Placement {
            at: (3, 4),
            size: (4, 2),
            angle: 30.0,
        };
        doc.transform_active(src, place, FilterType::Lanczos3, true);
        let (w, h) = (doc.width(), doc.height());
        assert!((5..=6).contains(&w) && (3..=5).contains(&h), "{w} × {h}");
        assert_eq!(doc.active_layer().rect(), doc.full_rect());
    }

    #[test]
    fn layers_stack_and_pick_topmost() {
        let mut doc = logo();
        doc.add_layer("blue".into(), RgbaImage::from_pixel(2, 2, BLUE), (5, 6));
        assert_eq!(doc.active, 1);
        let flat = doc.flatten();
        assert_eq!(*flat.get_pixel(4, 5), RED);
        assert_eq!(*flat.get_pixel(5, 6), BLUE);
        assert_eq!(doc.pick_layer(5, 6), Some(1));
        assert_eq!(doc.pick_layer(4, 5), Some(0));
        assert_eq!(doc.pick_layer(0, 0), None);

        assert!(doc.move_active(false));
        assert_eq!(*doc.flatten().get_pixel(5, 6), RED);
        doc.layers[1].visible = false;
        assert_eq!(doc.pick_layer(5, 6), Some(0));
    }

    #[test]
    fn split_creates_one_cropped_layer_per_element() {
        let mut img = RgbaImage::new(20, 10);
        img.put_pixel(2, 2, RED);
        img.put_pixel(3, 2, RED);
        img.put_pixel(15, 7, BLUE);
        let mut doc = Document::new(img, None);
        doc.resize_canvas(1, 1, 18, 8);

        assert_eq!(doc.split_active(0), 2);
        assert_eq!(doc.layers.len(), 2);
        // The first element (top left) is at the top of the stack and active.
        assert_eq!(doc.active, 1);
        assert_eq!(doc.layers[1].name, "Background 1");
        assert_eq!(
            doc.layers[1].rect(),
            PixelRect {
                x: 1,
                y: 1,
                w: 2,
                h: 1
            }
        );
        assert_eq!(
            doc.layers[0].rect(),
            PixelRect {
                x: 14,
                y: 6,
                w: 1,
                h: 1
            }
        );
        // The final image is unchanged.
        let flat = doc.flatten();
        assert_eq!(*flat.get_pixel(1, 1), RED);
        assert_eq!(*flat.get_pixel(14, 6), BLUE);

        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 1);
    }

    #[test]
    fn split_with_large_gap_keeps_single_layer() {
        let mut img = RgbaImage::new(10, 1);
        img.put_pixel(0, 0, RED);
        img.put_pixel(9, 0, RED);
        let mut doc = Document::new(img, None);
        assert_eq!(doc.split_active(20), 1);
        assert!(!doc.can_undo());
    }

    #[test]
    fn dropped_layer_reorders_and_selects() {
        let mut doc = logo();
        doc.add_empty_layer();
        doc.add_empty_layer();
        let ids: Vec<u64> = doc.layers.iter().map(|l| l.id).collect();
        // Dropped under the bottom layer.
        assert!(doc.drop_layer(2, Some(0)));
        assert_eq!(
            doc.layers.iter().map(|l| l.id).collect::<Vec<_>>(),
            vec![ids[2], ids[0], ids[1]]
        );
        assert_eq!(doc.active, 0);
        assert!(!doc.drop_layer(1, Some(1)));
        assert!(doc.undo());
        assert_eq!(doc.layers.iter().map(|l| l.id).collect::<Vec<_>>(), ids);
    }

    #[test]
    fn merge_down_and_visible() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, RED), None);
        doc.add_layer("blue".into(), RgbaImage::from_pixel(2, 2, BLUE), (3, 3));
        // No layer below the background.
        doc.active = 0;
        assert!(doc.merge_down().is_err());
        doc.active = 1;
        assert!(doc.merge_down().is_ok());
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.active_layer().name, "Background");
        // The overflow of the blue layer is kept.
        assert_eq!(
            doc.active_layer().rect(),
            PixelRect {
                x: 0,
                y: 0,
                w: 5,
                h: 5
            }
        );
        assert_eq!(doc.active_layer().pixel_at(4, 4), Some(BLUE.0));
        assert_eq!(doc.active_layer().pixel_at(0, 0), Some(RED.0));
        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 2);

        // Merge visible: the hidden layer stays apart.
        doc.add_empty_layer();
        doc.active_layer_mut().visible = false;
        assert!(doc.merge_visible().is_ok());
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.active, 0);
        assert!(!doc.layers[1].visible);
        assert_eq!(doc.layers[0].pixel_at(3, 3), Some(BLUE.0));
    }

    #[test]
    fn flatten_cuts_to_canvas_and_drops_hidden_layers() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, RED), None);
        doc.add_layer("blue".into(), RgbaImage::from_pixel(2, 2, BLUE), (3, 3));
        doc.add_layer("hidden".into(), RgbaImage::from_pixel(4, 4, BLUE), (0, 0));
        doc.active_layer_mut().visible = false;
        assert!(doc.flatten_image());
        assert_eq!(doc.layers.len(), 1);
        let layer = doc.active_layer();
        assert_eq!(layer.rect(), doc.full_rect());
        assert_eq!(layer.pixel_at(0, 0), Some(RED.0));
        assert_eq!(layer.pixel_at(3, 3), Some(BLUE.0));
        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 3);
    }

    #[test]
    fn rotate_and_flip_canvas() {
        // 4 × 2 canvas, a blue pixel layer at (3, 0).
        let mut doc = Document::new(RgbaImage::from_pixel(4, 2, RED), None);
        doc.add_layer("blue".into(), RgbaImage::from_pixel(1, 1, BLUE), (3, 0));
        assert!(doc.rotate_canvas(90.0));
        assert_eq!(doc.canvas, (2, 4));
        // Top right corner → bottom right after a clockwise quarter turn.
        assert_eq!(doc.layers[1].offset, (1, 3));
        assert_eq!(doc.layers[0].rect(), doc.full_rect());
        doc.flip_canvas(true);
        assert_eq!(doc.layers[1].offset, (0, 3));
        doc.flip_canvas(false);
        assert_eq!(doc.layers[1].offset, (0, 0));
        assert!(doc.rotate_canvas(180.0));
        assert_eq!(doc.layers[1].offset, (1, 3));
        // Any angle: the canvas grows to hold the rotated canvas.
        assert!(doc.rotate_canvas(45.0));
        assert_eq!(doc.canvas, (4, 4));
        assert!(doc.undo());
        assert_eq!(doc.canvas, (2, 4));
    }

    #[test]
    fn canvas_extension_color_fills_only_the_added_area() {
        let mut doc = Document::new(RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 0, 0])), None);
        doc.resize_canvas(-1, -1, 4, 4);
        let old = PixelRect {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        };
        doc.fill_canvas_extension(old, BLUE.0);
        let layer = &doc.layers[0];
        assert_eq!(layer.rect(), doc.full_rect());
        assert_eq!(layer.pixel_at(0, 0), Some(BLUE.0));
        // The former canvas keeps its (transparent) pixels.
        assert_eq!(layer.pixel_at(1, 1), Some([0, 0, 0, 0]));
    }

    #[test]
    fn fill_covers_canvas_only() {
        let mut doc = Document::new(RgbaImage::from_pixel(2, 2, RED), None);
        // Overflowing layer: 2 × 2 at (-1, -1) in a 4 × 4 canvas.
        doc.resize_canvas(1, 1, 4, 4);
        doc.fill_active([0, 0, 255, 255], None, 1.0, false);
        let flat = doc.flatten();
        assert!(flat.pixels().all(|p| *p == BLUE));
        // The out-of-frame pixel (-1, -1) stayed red.
        assert_eq!(doc.active_layer().alpha_at(-1, -1), 255);
        assert_eq!(doc.active_layer().image().get_pixel(0, 0), &RED);
    }

    #[test]
    fn text_layer_stays_editable_until_pixels_change() {
        let mut doc = logo();
        let data = TextData::plain("Hi", Default::default());
        let rendered = |w, anchor| Rendered {
            image: RgbaImage::from_pixel(w, 2, BLUE),
            anchor,
        };
        doc.add_text_layer(data.clone(), rendered(4, (0.5, 0.0)), (1.5, 1.0));
        assert_eq!(doc.active_layer().name, "Hi");
        assert_eq!(doc.active_layer().offset, (1, 1));

        // The name follows the text…
        let edited = TextData::plain("Hello", Default::default());
        doc.set_active_text(
            TextData {
                anchor: (0.5, 0.0),
                ..edited
            },
            rendered(5, (0.5, 0.0)),
        );
        assert_eq!(doc.active_layer().name, "Hello");
        // … unless it was renamed. The text's origin stays in place.
        doc.active_layer_mut().name = "Title".into();
        doc.set_active_text(
            TextData {
                anchor: (0.5, 0.0),
                ..data
            },
            rendered(4, (2.5, 3.0)),
        );
        assert_eq!(doc.active_layer().name, "Title");
        assert_eq!(doc.active_layer().offset, (-1, -2));
        assert_eq!(doc.active_layer().text.as_ref().unwrap().anchor, (2.5, 3.0));
        doc.set_active_text(
            doc.active_layer().text.clone().unwrap(),
            rendered(4, (0.5, 0.0)),
        );

        // A plain move keeps it editable, with an exact offset.
        let src = doc.active_layer().content_bounds().unwrap();
        doc.transform_active(
            src,
            Placement::new((3, 4), (src.w, src.h)),
            FilterType::Nearest,
            false,
        );
        assert!(doc.active_layer().text.is_some());
        assert_eq!(doc.active_layer().offset, (3, 4));
        // Resizing it turns it into pixels.
        let src = doc.active_layer().content_bounds().unwrap();
        doc.transform_active(
            src,
            Placement::new((3, 4), (8, 4)),
            FilterType::Nearest,
            false,
        );
        assert!(doc.active_layer().text.is_none());
        assert!(doc.undo());
        assert!(doc.active_layer().text.is_some());
        // Painting too.
        doc.expand_active_to_canvas();
        assert!(doc.active_layer().text.is_none());
    }

    #[test]
    fn center_active_centers_visible_pixels() {
        let mut doc = logo(); // 2 × 3 square at (4, 5) in 10 × 10
        assert!(doc.center_active());
        assert_eq!(
            doc.content_bounds(),
            Some(PixelRect {
                x: 4,
                y: 3,
                w: 2,
                h: 3
            })
        );
        assert!(!doc.center_active());
    }

    #[test]
    fn locked_layers_are_not_picked() {
        let mut doc = logo();
        doc.add_layer("blue".into(), RgbaImage::from_pixel(2, 2, BLUE), (4, 5));
        assert_eq!(doc.pick_layer(4, 5), Some(1));
        doc.layers[1].locked = true;
        // The click goes through the locked layer.
        assert_eq!(doc.pick_layer(4, 5), Some(0));
        doc.layers[0].locked = true;
        assert_eq!(doc.pick_layer(4, 5), None);
    }

    #[test]
    fn blend_modes() {
        let gray = Rgba([128, 128, 128, 255]);
        let flat = |top: Rgba<u8>, mode: BlendMode, opacity: f32| {
            let mut doc = Document::new(RgbaImage::from_pixel(1, 1, gray), None);
            doc.add_layer("top".into(), RgbaImage::from_pixel(1, 1, top), (0, 0));
            doc.active_layer_mut().blend = mode;
            doc.active_layer_mut().opacity = opacity;
            doc.flatten().get_pixel(0, 0).0
        };
        // Multiply: white changes nothing, black gives black.
        assert_eq!(flat(Rgba([255; 4]), BlendMode::Multiply, 1.0), gray.0);
        assert_eq!(
            flat(Rgba([0, 0, 0, 255]), BlendMode::Multiply, 1.0),
            [0, 0, 0, 255]
        );
        // Screen: black changes nothing, white gives white.
        assert_eq!(flat(Rgba([0, 0, 0, 255]), BlendMode::Screen, 1.0), gray.0);
        assert_eq!(flat(Rgba([255; 4]), BlendMode::Screen, 1.0), [255; 4]);
        // Over transparency, all modes put the color as is.
        for mode in BlendMode::all().filter(|m| *m != BlendMode::Dissolve) {
            let mut doc = Document::new(RgbaImage::new(1, 1), None);
            doc.add_layer(
                "t".into(),
                RgbaImage::from_pixel(1, 1, Rgba([10, 20, 30, 200])),
                (0, 0),
            );
            doc.active_layer_mut().blend = mode;
            assert_eq!(
                doc.flatten().get_pixel(0, 0).0,
                [10, 20, 30, 200],
                "{mode:?}"
            );
        }
        // Normal at half opacity.
        let mut doc = Document::new(RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255])), None);
        doc.add_layer(
            "w".into(),
            RgbaImage::from_pixel(1, 1, Rgba([255; 4])),
            (0, 0),
        );
        doc.active_layer_mut().opacity = 0.5;
        assert_eq!(doc.flatten().get_pixel(0, 0).0, [128, 128, 128, 255]);
    }

    #[test]
    fn composite_uses_blend_mode_and_region() {
        let mut doc = Document::new(
            RgbaImage::from_pixel(4, 4, image::Rgba([200, 100, 50, 255])),
            None,
        );
        doc.add_layer(
            "half".into(),
            RgbaImage::from_pixel(2, 2, image::Rgba([128, 128, 128, 255])),
            (1, 1),
        );
        doc.active_layer_mut().blend = BlendMode::Multiply;
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(0, 0).0, [200, 100, 50, 255]);
        assert_eq!(flat.get_pixel(1, 1).0, [100, 50, 25, 255]);
        // A region extending beyond the canvas: transparent outside.
        let r = doc.composite(
            PixelRect {
                x: 3,
                y: 3,
                w: 2,
                h: 2,
            },
            None,
        );
        assert_eq!(r.get_pixel(0, 0).0[3], 255);
        assert_eq!(r.get_pixel(1, 1).0[3], 0);
        // Without the top layer.
        assert_eq!(
            doc.composite(doc.full_rect(), Some(1)).get_pixel(1, 1).0,
            [200, 100, 50, 255]
        );
    }

    #[test]
    fn fill_at_fills_region_and_records_history() {
        let mut doc = logo(); // transparent background, red square at (4, 5)
        assert!(doc.fill_at(0, 0, [0, 0, 255, 255], 0, None, true));
        assert_eq!(*doc.flatten().get_pixel(0, 0), BLUE);
        assert_eq!(*doc.flatten().get_pixel(4, 5), RED);
        assert_eq!(doc.color_at(4, 5), Some(RED.0));
        // Same color: nothing changes, nothing is recorded.
        let step = doc.history_step();
        assert!(!doc.fill_at(0, 0, [0, 0, 255, 255], 0, None, true));
        assert_eq!(doc.history_step(), step);
        assert!(!doc.fill_at(50, 50, [0, 0, 0, 255], 0, None, true));
        assert!(doc.undo());
        assert_eq!(doc.color_at(0, 0), None);
    }

    #[test]
    fn image_size_scales_everything() {
        let mut doc = logo();
        doc.add_layer("B".into(), RgbaImage::from_pixel(2, 2, RED), (6, 4));
        doc.active_layer_mut().effects.stroke.width = 3.0;
        assert!(!doc.resize_image(10, 10, FilterType::Nearest));
        assert!(doc.resize_image(20, 30, FilterType::Nearest));
        assert_eq!(doc.canvas, (20, 30));
        let b = &doc.layers[1];
        assert_eq!(
            b.rect(),
            PixelRect {
                x: 12,
                y: 12,
                w: 4,
                h: 6
            }
        );
        assert!((b.effects.stroke.width - 3.0 * 6f32.sqrt()).abs() < 1e-4);
        assert_eq!(doc.layers[0].pixel_at(9, 16), Some(RED.0));
        assert!(doc.undo());
        assert_eq!(doc.canvas, (10, 10));
    }

    #[test]
    fn region_operations() {
        let mut doc = Document::new(RgbaImage::from_pixel(10, 10, RED), None);
        let sel = PixelRect {
            x: 2,
            y: 2,
            w: 3,
            h: 3,
        };
        // New layer from the selection, at the same place.
        assert!(doc.layer_from_region(&sel.into()));
        assert_eq!(doc.active_layer().rect(), sel);
        assert_eq!(doc.active_layer().name, "Background (selection)");
        // Clear the area of the background.
        doc.active = 0;
        assert!(doc.clear_region(&sel.into()));
        assert_eq!(doc.layers[0].alpha_at(3, 3), 0);
        assert_eq!(doc.layers[0].alpha_at(5, 5), 255);
        // Fill only the selection.
        doc.fill_active([0, 0, 255, 255], Some(&sel.into()), 1.0, false);
        assert_eq!(doc.layers[0].pixel_at(3, 3), Some(BLUE.0));
        assert_eq!(doc.layers[0].pixel_at(6, 6), Some(RED.0));
        // Paint bucket limited to the selection.
        let mut doc = Document::new(RgbaImage::from_pixel(10, 10, RED), None);
        assert!(doc.fill_at(3, 3, [0, 0, 255, 255], 0, Some(&sel.into()), true));
        assert_eq!(doc.layers[0].pixel_at(4, 4), Some(BLUE.0));
        assert_eq!(doc.layers[0].pixel_at(5, 5), Some(RED.0));
        // Inverted selection: everything but the rectangle.
        let inverted = Selection {
            rect: sel,
            mask: None,
            inverted: true,
        };
        assert!(inverted.contains(0.0, 0.0) && !inverted.contains(3.0, 3.0));
        assert!(doc.clear_region(&inverted));
        assert_eq!(doc.layers[0].alpha_at(0, 0), 0);
        assert_eq!(doc.layers[0].alpha_at(9, 9), 0);
        assert_eq!(doc.layers[0].alpha_at(3, 3), 255);
        let copied = doc.active_pixels(Some(&inverted)).unwrap();
        assert_eq!(copied.dimensions(), (10, 10));
        assert_eq!(copied.get_pixel(3, 3).0[3], 0);
        // Fill at 50%, then preserving transparency.
        let mut doc = Document::new(RgbaImage::new(2, 1), None);
        doc.active_layer_mut().pixels_mut().put_pixel(0, 0, RED);
        assert!(doc.fill_active([0, 0, 255, 255], None, 0.5, true));
        assert_eq!(doc.layers[0].pixel_at(0, 0), Some([127, 0, 128, 255]));
        assert_eq!(doc.layers[0].alpha_at(1, 0), 0);
        assert!(doc.fill_active([0, 0, 255, 255], None, 0.5, false));
        assert_eq!(doc.layers[0].alpha_at(1, 0), 128);
        // Non-contiguous bucket: all areas of the same color.
        let mut img = RgbaImage::from_pixel(5, 1, RED);
        img.put_pixel(2, 0, BLUE);
        let mut doc = Document::new(img, None);
        assert!(doc.fill_at(0, 0, [0, 255, 0, 255], 0, None, false));
        assert_eq!(doc.layers[0].pixel_at(4, 0), Some([0, 255, 0, 255]));
        assert_eq!(doc.layers[0].pixel_at(2, 0), Some(BLUE.0));
    }

    #[test]
    fn align_and_distribute() {
        // Three 2 × 2 squares in a 20 × 10 canvas.
        let mut doc = Document::new(RgbaImage::new(20, 10), None);
        for (x, y) in [(1, 1), (5, 6), (14, 3)] {
            doc.add_layer("c".into(), RgbaImage::from_pixel(2, 2, RED), (x, y));
        }
        let all = [1, 2, 3];
        assert!(doc.align(&all, AlignOp::Top));
        assert!(all.iter().all(|&i| doc.layers[i].offset.1 == 1));
        assert!(doc.align(&all, AlignOp::Right));
        assert!(all.iter().all(|&i| doc.layers[i].offset.0 == 14));
        assert!(!doc.align(&all, AlignOp::Right));

        // Distribution: equal spacing between 1, 5 and 14 → 1, 7.5 (8), 14.
        let mut doc = Document::new(RgbaImage::new(20, 10), None);
        for (x, y) in [(1, 1), (5, 6), (14, 3)] {
            doc.add_layer("c".into(), RgbaImage::from_pixel(2, 2, RED), (x, y));
        }
        assert!(doc.align(&all, AlignOp::DistributeH));
        assert_eq!(doc.layers[2].offset.0, 8);
        assert!(!doc.align(&[1, 2], AlignOp::DistributeH));

        // A single layer: aligned to the canvas; locked: does not move.
        assert!(doc.align(&[3], AlignOp::CenterV));
        assert_eq!(doc.layers[3].offset.1, 4);
        doc.layers[1].locked = true;
        assert!(!doc.align(&[1], AlignOp::Bottom));
    }

    #[test]
    fn effects_appear_in_flatten_under_content() {
        let mut doc = Document::new(RgbaImage::new(10, 10), None);
        doc.add_layer("c".into(), RgbaImage::from_pixel(2, 2, RED), (4, 4));
        doc.active_layer_mut().effects.stroke = crate::compose::styles::StrokeFx {
            enabled: true,
            color: [0, 0, 255, 255],
            width: 1.0,
            ..Effects::default().stroke
        };
        let flat = doc.flatten();
        assert_eq!(*flat.get_pixel(4, 4), RED); // content on top
        assert_eq!(*flat.get_pixel(3, 4), BLUE); // stroke around
        assert_eq!(flat.get_pixel(1, 1).0[3], 0);
    }

    #[test]
    fn pixels_are_shared_until_modified() {
        let mut doc = logo();
        doc.duplicate_active();
        // Copy and original share their pixels; so does the history.
        assert!(std::ptr::eq(doc.layers[0].image(), doc.layers[1].image()));
        let rev = doc.layers[1].rev();
        // A move doesn't touch the pixels.
        doc.layers[1].offset.0 += 3;
        assert_eq!(doc.layers[1].rev(), rev);
        assert_eq!(
            doc.layers[1].content_bounds(),
            Some(PixelRect {
                x: 7,
                y: 5,
                w: 2,
                h: 3
            })
        );
        // First modification: copy, new revision, box recomputed.
        doc.layers[1].pixels_mut().put_pixel(0, 0, RED);
        assert!(!std::ptr::eq(doc.layers[0].image(), doc.layers[1].image()));
        assert_ne!(doc.layers[1].rev(), rev);
        assert_eq!(
            doc.layers[1].content_bounds(),
            Some(PixelRect {
                x: 3,
                y: 0,
                w: 6,
                h: 8
            })
        );
        assert_eq!(doc.layers[0].alpha_at(0, 0), 0);
    }

    #[test]
    fn modifying_pixels_turns_text_into_pixels() {
        let mut doc = logo();
        let data = TextData::plain("a", Default::default());
        let image = RgbaImage::new(2, 2);
        doc.add_text_layer(
            data,
            Rendered {
                image,
                anchor: (0.0, 0.0),
            },
            (0.0, 0.0),
        );
        doc.active_layer_mut().pixels_mut();
        assert!(doc.active_layer().text.is_none());
    }

    #[test]
    fn delete_keeps_at_least_one_layer() {
        let mut doc = logo();
        assert!(!doc.delete_active());
        doc.add_empty_layer();
        assert!(doc.delete_active());
        assert_eq!(doc.layers.len(), 1);
        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 2);
    }

    #[test]
    fn lifted_selection_moves_and_drops_in_one_undo_step() {
        let mut doc = logo();
        let sel = PixelRect {
            x: 4,
            y: 5,
            w: 2,
            h: 3,
        };
        let empty = PixelRect {
            x: 0,
            y: 0,
            w: 2,
            h: 2,
        };
        assert_eq!(doc.lift_region(&empty.into(), false), None);
        let id = doc.lift_region(&sel.into(), false).expect("lifted");
        assert_eq!((doc.layers.len(), doc.active), (2, 1));
        assert_eq!(doc.layers[0].alpha_at(4, 5), 0);
        // Moved partly outside the original layer: it grows.
        doc.active_layer_mut().offset = (9, 5);
        assert!(doc.drop_floating(id));
        // Already dropped: nothing else is merged.
        assert!(!doc.drop_floating(id));
        assert_eq!((doc.layers.len(), doc.active), (1, 0));
        let layer = doc.active_layer();
        assert_eq!(
            layer.rect(),
            PixelRect {
                x: 0,
                y: 0,
                w: 11,
                h: 10
            }
        );
        assert_eq!(layer.pixel_at(10, 7), Some(RED.0));
        assert_eq!(layer.alpha_at(4, 5), 0);
        assert!(doc.undo());
        assert_eq!(doc.active_layer().pixel_at(4, 5), Some(RED.0));
        assert!(!doc.can_undo());
        // Copy: the original stays.
        let id = doc.lift_region(&sel.into(), true).expect("lifted");
        assert!(doc.drop_floating(id));
        assert_eq!(doc.active_layer().pixel_at(4, 5), Some(RED.0));
    }

    #[test]
    fn expand_for_painting_covers_canvas() {
        let mut doc = Document::new(RgbaImage::from_pixel(2, 2, RED), None);
        doc.resize_canvas(-3, -3, 8, 8);
        assert!(doc.expand_active_to_canvas());
        assert_eq!(
            doc.active_layer().rect(),
            PixelRect {
                x: 0,
                y: 0,
                w: 8,
                h: 8
            }
        );
        assert_eq!(*doc.flatten().get_pixel(3, 3), RED);
        assert!(!doc.expand_active_to_canvas());
    }

    const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);

    fn red_stroke() -> crate::compose::styles::StrokeFx {
        crate::compose::styles::StrokeFx {
            enabled: true,
            color: [255, 0, 0, 255],
            width: 2.0,
            ..Effects::default().stroke
        }
    }

    #[test]
    fn remove_background_replaces_a_disabled_mask() {
        let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba([9, 9, 9, 255])), None);
        doc.add_mask(NewMask::HideAll, None);
        let m = doc.active_layer_mut().mask.as_mut().unwrap();
        m.enabled = false;
        m.density = 0.5;
        let cut = GrayImage::from_fn(8, 8, |x, _| image::Luma([if x < 4 { 255 } else { 0 }]));
        assert!(doc.mask_active(&cut));
        let m = doc.active_layer().mask.as_ref().unwrap();
        // The cut-out shows, whole (the hidden-all mask was off).
        assert!(m.enabled && m.density == 1.0);
        assert_eq!(m.image().get_pixel(1, 1).0, [255]);
        assert_eq!(m.image().get_pixel(6, 1).0, [0]);
    }

    #[test]
    fn history_step_changes_even_when_the_history_is_full() {
        let mut doc = logo();
        for _ in 0..MAX_UNDO + 2 {
            doc.checkpoint("step".into());
        }
        let step = doc.history_step();
        doc.checkpoint("step".into());
        assert_ne!(doc.history_step(), step);
        // Undo then a new step: not the same position either.
        let step = doc.history_step();
        doc.undo();
        doc.checkpoint("step".into());
        assert_ne!(doc.history_step(), step);
    }

    #[test]
    fn a_discarded_step_gives_back_the_oldest_one_of_a_full_history() {
        let mut doc = Document::new(RgbaImage::new(4, 4), None);
        for i in 0..MAX_UNDO {
            doc.checkpoint(format!("s{i}"));
        }
        assert!(!doc.history_forgotten());
        let numbers = doc.history_numbers();
        // A cancelled dialog: recorded, then discarded.
        doc.checkpoint("cancelled".into());
        assert!(doc.history_forgotten());
        doc.discard_step();
        assert_eq!(doc.history_numbers(), numbers);
        assert!(!doc.history_forgotten(), "the opened state is still the oldest");
        // Kept, a new step makes the oldest go for good.
        doc.checkpoint("kept".into());
        doc.checkpoint("next".into());
        doc.discard_step();
        assert_eq!(doc.undo.len(), MAX_UNDO);
        assert_eq!(doc.history_numbers()[0], numbers[1]);
        assert!(doc.history_forgotten());
    }

    #[test]
    fn only_going_back_or_sideways_changes_the_generation() {
        let mut doc = Document::new(RgbaImage::new(4, 4), None);
        let start = doc.history_generation();
        doc.checkpoint("a".into());
        doc.checkpoint("b".into());
        assert_eq!(doc.history_generation(), start, "steps forward");
        doc.undo();
        let undone = doc.history_generation();
        assert_ne!(undone, start);
        doc.redo();
        assert_ne!(doc.history_generation(), undone);
        let redone = doc.history_generation();
        doc.checkpoint("c".into());
        doc.discard_step();
        assert_ne!(doc.history_generation(), redone, "a step discarded");
    }

    #[test]
    fn a_new_adjustment_layer_is_one_step_with_a_full_history() {
        let mut doc = Document::new(RgbaImage::new(16, 16), None);
        for i in 0..MAX_UNDO + 10 {
            doc.checkpoint(format!("s{i}"));
        }
        let oldest = doc.history_numbers()[0];
        doc.add_adjustment_layer(crate::pixel_ops::Adjustment::Invert, None);
        // One step: only one more of the oldest is forgotten.
        assert_eq!(doc.history_numbers()[0], oldest + 1);
        assert!(doc.layers.last().unwrap().mask.is_some());
        doc.undo();
        assert_eq!(doc.layers.len(), 1);
        let (names, done) = doc.history();
        assert!(names[done - 1].starts_with('s'));
    }

    #[test]
    fn a_dropped_redo_list_counts_in_the_budget() {
        let mut doc = Document::new(RgbaImage::new(256, 256), None);
        for _ in 0..4 {
            doc.checkpoint("paint".into());
            doc.active_layer_mut()
                .replace_pixels(RgbaImage::new(256, 256));
        }
        doc.undo();
        doc.undo();
        // A new step: the undone ones stay (in case it changes nothing),
        // counted.
        doc.checkpoint("paint".into());
        doc.active_layer_mut()
            .replace_pixels(RgbaImage::new(256, 256));
        doc.checkpoint("paint".into());
        assert!(doc.dropped_redo.is_some());
        let with = doc.history_bytes();
        // Over the budget, they are forgotten first.
        let kept = doc.undo.len();
        doc.trim_history(with - 1);
        assert!(doc.dropped_redo.is_none());
        assert!(doc.history_bytes() < with);
        assert_eq!(doc.undo.len(), kept);
        // The step that dropped them forgotten: they go too.
        let mut doc = Document::new(RgbaImage::new(8, 8), None);
        doc.checkpoint("a".into());
        doc.undo();
        doc.checkpoint("b".into());
        assert!(doc.dropped_redo.is_some());
        for _ in 0..MAX_UNDO {
            doc.checkpoint("c".into());
        }
        assert!(doc.dropped_redo.is_none());
    }

    #[test]
    fn a_recomputed_identical_selection_is_no_step() {
        let mut doc = Document::new(RgbaImage::new(16, 16), None);
        let rect = PixelRect {
            x: 2,
            y: 2,
            w: 4,
            h: 4,
        };
        let mask = || Arc::new(GrayImage::from_pixel(4, 4, image::Luma([200])));
        let sel = |mask| Selection {
            rect,
            mask: Some(mask),
            inverted: false,
        };
        assert!(doc.record_selection(Some(sel(mask())), "Grow".into()));
        assert!(!doc.record_selection(Some(sel(mask())), "Grow".into()));
        assert_eq!(doc.history().0.len(), 1);
    }

    #[test]
    fn undo_keeps_groups_folded_as_shown() {
        let mut doc = Document::new(RgbaImage::new(8, 8), None);
        doc.add_group();
        let group = doc.active;
        doc.checkpoint("other".into());
        doc.mark_saved();
        let fold =
            |doc: &Document| matches!(doc.layers[group].kind, LayerKind::Group { expanded: false });
        if let LayerKind::Group { expanded } = &mut doc.layers[group].kind {
            *expanded = false;
        }
        doc.mark_modified();
        doc.undo();
        assert!(fold(&doc), "undo leaves the fold");
        doc.redo();
        assert!(fold(&doc));
        assert!(doc.modified, "folded since the save");
    }

    #[test]
    fn history_keeps_to_its_memory_budget() {
        // 64 MiB per step: the budget holds 16 of them, the count limit 30.
        let mut doc = Document::new(RgbaImage::new(4096, 4096), None);
        for _ in 0..MAX_UNDO {
            doc.checkpoint("step".into());
            doc.active_layer_mut()
                .replace_pixels(RgbaImage::new(4096, 4096));
        }
        // The newest step shares its pixels with the current state.
        assert_eq!(doc.undo.len(), HISTORY_BUDGET / (4096 * 4096 * 4) + 1);
        // Steps that share their pixels cost nothing: the count limit applies.
        let mut doc = Document::new(RgbaImage::new(4096, 4096), None);
        for _ in 0..MAX_UNDO + 1 {
            doc.checkpoint("step".into());
            doc.active_layer_mut().opacity -= 0.01;
        }
        assert_eq!(doc.undo.len(), MAX_UNDO);
        // The last step is always kept, however big.
        let mut doc = Document::new(RgbaImage::new(1, 1), None);
        doc.checkpoint("step".into());
        doc.layers[0].replace_pixels(RgbaImage::new(1, 1));
        doc.trim_history(HISTORY_BUDGET);
        assert!(doc.can_undo());
    }

    #[test]
    fn a_discarded_step_gives_back_redo_and_the_saved_state() {
        let mut doc = Document::new(RgbaImage::new(4, 4), None);
        doc.checkpoint("a".into());
        doc.active_layer_mut().opacity = 0.5;
        doc.mark_saved();
        assert!(doc.undo() && doc.modified);
        // A step that turns out to change nothing (a cancelled dialog).
        doc.checkpoint("b".into());
        assert!(!doc.can_redo());
        doc.discard_step();
        assert!(doc.can_redo() && doc.modified);
        assert!(doc.redo() && !doc.modified);
        // Two steps given up together (a text cancelled) as well.
        doc.undo();
        let mark = doc.history_mark();
        doc.checkpoint("c".into());
        doc.checkpoint("d".into());
        assert!(doc.rollback_to(mark));
        assert!(doc.can_redo() && doc.redo() && !doc.modified);
    }

    #[test]
    fn rollback_reaches_its_mark_through_a_full_history() {
        let mut doc = Document::new(RgbaImage::new(4, 4), None);
        for _ in 0..MAX_UNDO {
            doc.checkpoint("step".into());
        }
        let mark = doc.history_mark();
        doc.add_empty_layer();
        doc.checkpoint("typing".into());
        assert!(doc.rollback_to(mark));
        assert_eq!(doc.layers.len(), 1);
        // Further back than the history reaches (the oldest step the last
        // one let go comes back, not those before): not at the mark.
        for _ in 0..MAX_UNDO + 2 {
            doc.checkpoint("step".into());
        }
        assert!(!doc.rollback_to(mark));
    }

    #[test]
    fn selection_and_mask_target_are_in_the_history() {
        let mut doc = Document::new(RgbaImage::new(8, 8), None);
        let sel: Selection = PixelRect {
            x: 1,
            y: 1,
            w: 2,
            h: 2,
        }
        .into();
        assert!(doc.record_selection(Some(sel.clone()), "select".into()));
        assert!(!doc.record_selection(Some(sel.clone()), "again".into()));
        doc.add_mask(NewMask::RevealAll, None);
        assert!(doc.mask_targeted());
        doc.target_mask(false);
        doc.checkpoint("step".into());
        doc.target_mask(true);
        doc.undo();
        assert!(!doc.mask_targeted());
        doc.redo();
        assert!(doc.mask_targeted());
        while doc.undo() {}
        assert!(doc.selection().is_none());
        doc.redo();
        assert!(doc.selection().is_some_and(|s| s.same(&sel)));
    }

    #[test]
    fn history_memory_counts_channels_and_smart_objects() {
        let mut doc = Document::new(RgbaImage::new(100, 100), None);
        doc.checkpoint("step".into());
        let before = doc.history_bytes();
        // A channel replaced: the old one is only in the history.
        doc.channels.push(Channel {
            name: "a".into(),
            mask: Arc::new(GrayImage::new(100, 100)),
        });
        doc.checkpoint("step".into());
        doc.channels[0].mask = Arc::new(GrayImage::new(100, 100));
        assert_eq!(doc.history_bytes(), before + 100 * 100);
        // A smart object's contents count too.
        let mut smart = Document::new(RgbaImage::new(50, 50), None);
        assert!(smart.convert_to_smart(&[0]));
        let mut buffers = Vec::new();
        smart.layers[0].buffers(&mut buffers);
        let total: usize = buffers.iter().map(|b| b.1).sum();
        assert!(total >= 2 * 50 * 50 * 4, "contents and image: {total}");
    }

    #[test]
    fn layer_opacity_applies_to_content_and_effects_together() {
        let mut doc = Document::new(RgbaImage::from_pixel(12, 12, WHITE), None);
        doc.add_layer("sq".into(), RgbaImage::from_pixel(4, 4, WHITE), (4, 4));
        let layer = doc.active_layer_mut();
        layer.effects.stroke = red_stroke();
        layer.effects.shadow.enabled = true;
        layer.opacity = 0.5;
        let flat = doc.flatten();
        // Neither the stroke nor the shadow shows through the content.
        assert_eq!(flat.get_pixel(5, 5).0, WHITE.0);
        assert_eq!(flat.get_pixel(6, 6).0, WHITE.0);
        // The stroke is there, at half strength (alone: the shadow, in
        // Multiply on what is below, darkens through it).
        doc.active_layer_mut().effects.shadow.enabled = false;
        assert_eq!(doc.flatten().get_pixel(2, 5).0, [255, 128, 128, 255]);
    }

    #[test]
    fn outside_stroke_stays_out_of_semi_transparent_content() {
        let mut doc = Document::new(RgbaImage::from_pixel(12, 12, WHITE), None);
        let image = RgbaImage::from_pixel(6, 6, Rgba([255, 255, 255, 160]));
        doc.add_layer("soft".into(), image, (3, 3));
        doc.active_layer_mut().effects.stroke = red_stroke();
        assert_eq!(doc.flatten().get_pixel(5, 5).0, WHITE.0);
    }

    #[test]
    fn image_size_keeps_transparent_edges_clean() {
        let mut img = RgbaImage::new(4, 4);
        for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            img.put_pixel(x, y, WHITE);
        }
        let mut doc = Document::new(img, None);
        for filter in [
            FilterType::Triangle,
            FilterType::CatmullRom,
            FilterType::Lanczos3,
        ] {
            doc.resize_image(8, 8, filter);
            for p in doc.active_layer().image().pixels() {
                assert!(
                    p.0[3] == 0 || p.0[..3].iter().all(|&c| c >= 250),
                    "{filter:?}: {p:?}"
                );
            }
            doc.undo();
        }
    }

    #[test]
    fn canvas_extension_leaves_effects_and_texts_alone() {
        let mut doc = Document::new(RgbaImage::new(10, 10), None);
        doc.active_layer_mut().pixels_mut().put_pixel(5, 5, RED);
        doc.active_layer_mut().effects.stroke = red_stroke();
        doc.resize_canvas(-2, -2, 14, 14);
        let before = doc.flatten();
        let old = PixelRect {
            x: 2,
            y: 2,
            w: 10,
            h: 10,
        };
        doc.fill_canvas_extension(old, [255, 255, 255, 255]);
        // Nothing baked, no stroke around a new opaque band.
        assert!(doc.layers[0].effects.stroke.enabled);
        assert!(doc.flatten() == before);
    }

    #[test]
    fn split_and_duplicate_keep_layer_properties() {
        let mut img = RgbaImage::new(20, 10);
        img.put_pixel(2, 2, RED);
        img.put_pixel(15, 7, RED);
        let mut doc = Document::new(RgbaImage::from_pixel(20, 10, Rgba([0, 255, 0, 255])), None);
        doc.add_layer("L".into(), img, (0, 0));
        doc.active_layer_mut().blend = BlendMode::Screen;
        doc.active_layer_mut().effects.stroke.enabled = true;
        let before = doc.flatten();
        assert_eq!(doc.split_active(0), 2);
        assert!(doc.flatten() == before, "split changed the final image");

        let mut doc = Document::new(RgbaImage::new(10, 10), None);
        let data = TextData::plain("Hi", Default::default());
        let image = RgbaImage::from_pixel(4, 2, RED);
        doc.add_text_layer(
            data,
            Rendered {
                image,
                anchor: (0.0, 0.0),
            },
            (1.0, 1.0),
        );
        let layer = doc.active_layer_mut();
        (layer.visible, layer.locked) = (false, true);
        doc.duplicate_active();
        let copy = doc.active_layer();
        assert!(copy.text.is_some() && !copy.visible && copy.locked);
    }

    #[test]
    fn merges_refuse_locked_layers_alike() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, RED), None);
        doc.add_layer("b".into(), RgbaImage::from_pixel(2, 2, RED), (0, 0));
        doc.active_layer_mut().locked = true;
        assert_eq!(doc.merge_down(), Err(MergeError::Locked));
        assert_eq!(doc.merge_visible(), Err(MergeError::Locked));
    }

    #[test]
    fn merging_far_apart_layers_is_refused() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, RED), None);
        doc.add_layer(
            "far".into(),
            RgbaImage::from_pixel(1, 1, RED),
            (60000, 60000),
        );
        assert_eq!(doc.merge_down(), Err(MergeError::TooLarge));
        assert_eq!(doc.merge_visible(), Err(MergeError::TooLarge));
        // Painting it keeps its size instead of allocating 14 GB.
        assert!(!doc.expand_active_to_canvas());
        assert_eq!(doc.active_layer().rect().w, 1);
    }

    #[test]
    fn new_layer_names_never_repeat() {
        let mut doc = logo();
        doc.add_empty_layer();
        doc.add_empty_layer();
        assert_eq!(doc.active_layer().name, "Layer 2");
        doc.delete_active();
        doc.add_empty_layer();
        assert_eq!(doc.active_layer().name, "Layer 3");
        let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Background", "Layer 1", "Layer 3"]);
    }

    #[test]
    fn adjustments_stay_inside_the_selection() {
        use crate::pixel_ops::{Adjustment, HueSaturation};
        let mut doc = Document::new(RgbaImage::from_pixel(4, 1, RED), None);
        let mask = GrayImage::from_raw(4, 1, vec![255, 128, 0, 0]).unwrap();
        let selection = Selection::from_mask(&mask, (0, 0)).unwrap();
        let invert = Adjustment::HueSaturation(HueSaturation {
            lightness: -1.0,
            ..Default::default()
        });
        assert!(doc.adjust_active(&invert, Some(&selection)));
        let px: Vec<u8> = doc
            .active_layer()
            .image()
            .pixels()
            .map(|p| p.0[0])
            .collect();
        assert_eq!(px[0], 0);
        assert!((126..=130).contains(&px[1]), "{px:?}");
        assert_eq!(px[2..], [255, 255]);
        // A selection beside the layer: nothing to do, no step.
        doc.add_layer("small".into(), RgbaImage::from_pixel(1, 1, RED), (3, 0));
        let steps = doc.history_step();
        assert!(!doc.adjust_active(&invert, Some(&selection)));
        assert!(!doc.color_to_alpha_active([255, 0, 0], 0.1, 0.0, Some(&selection)));
        assert_eq!(doc.history_step(), steps);
    }

    #[test]
    fn copy_takes_the_targeted_mask() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 2, RED), None);
        assert_eq!(*doc.copied_pixels(None).unwrap().get_pixel(0, 0), RED);
        doc.add_mask(NewMask::HideAll, None);
        assert!(doc.mask_targeted());
        // The whole mask over the canvas, in gray (black: it hides all).
        let copied = doc.copied_pixels(None).unwrap();
        assert_eq!(copied.dimensions(), (4, 2));
        assert_eq!(copied.get_pixel(3, 1).0, [0, 0, 0, 255]);
        let sel: Selection = PixelRect {
            x: 1,
            y: 0,
            w: 2,
            h: 1,
        }
        .into();
        assert_eq!(doc.copied_pixels(Some(&sel)).unwrap().dimensions(), (2, 1));
        doc.target_mask(false);
        assert_eq!(*doc.copied_pixels(None).unwrap().get_pixel(0, 0), RED);
    }

    #[test]
    fn discarded_step_restores_the_state() {
        let mut doc = Document::new(RgbaImage::from_pixel(2, 2, RED), None);
        doc.resize_canvas(-1, -1, 4, 4);
        doc.checkpoint("step".into());
        let before = doc.undo.len();
        doc.expand_active_to_canvas();
        doc.discard_step();
        assert_eq!(doc.undo.len(), before - 1);
        assert_eq!(doc.active_layer().rect().w, 2);
        assert!(!doc.can_redo());
    }
}
