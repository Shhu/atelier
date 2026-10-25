//! Filter › Liquify (Shift+Ctrl+X), like Photoshop's workspace: the tools
//! on the left, the layer being distorted in the middle (zoom, pan, the rest
//! of the image as a backdrop), the options on the right. The mesh is edited
//! over a reduced copy of the layer, and only what a dab changed is drawn
//! again; OK computes the result at full resolution on another thread, as
//! one undo step. The last mesh applied stays for Load Last Mesh and Last
//! Filter.

use std::sync::mpsc;

use image::RgbaImage;

use super::actions_panel::Stopped;
use super::dialogs::{
    DialogSpec, Steps, Track, dialog_body_height, floating_dialog, gap_between_blocks, slider_row,
    slider_row_steps,
};
use super::filters_ui::active_target;
use super::paint_tools::frame_samples;
use super::*;
use crate::compose::levels::reduce_rgba;
use crate::document::SmartFilterKind;
use crate::i18n::t;
use crate::liquify::{self, Brush, Effect, Frame, Liquify, Mesh, NodeRect, Snapshot};
use crate::pixel_ops::Clip;
use crate::retouch::Progress;
use ui_kit::color;

/// Filter › Liquify: Shift+Ctrl+X, like Photoshop.
pub(super) const SHORTCUT_LIQUIFY: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::X);

/// Longest side of the preview: larger layers are previewed halved (or
/// more), the mesh still at full resolution.
const PREVIEW_SIDE: u32 = 3072;
/// Width of the options column.
const OPTIONS_WIDTH: f32 = 272.0;
/// Steps kept by the workspace's own undo.
const MAX_UNDO: usize = 64;
/// Memory the workspace's undo may hold (bytes): fewer steps of a large
/// mesh.
const UNDO_BUDGET: usize = 256 << 20;
/// The result being computed shows its progress after this delay (s).
const PROGRESS_DELAY: f64 = 0.3;
/// Largest brush, in pixels.
const MAX_SIZE: f32 = 15000.0;
/// Mesh files: Save Mesh, Load Mesh.
const MESH_EXTENSION: &str = "msh";

/// The workspace's tools.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LiquifyTool {
    ForwardWarp,
    Reconstruct,
    Smooth,
    Twirl,
    Pucker,
    Bloat,
    PushLeft,
    Freeze,
    Thaw,
    Hand,
    Zoom,
}

impl LiquifyTool {
    const ALL: [LiquifyTool; 11] = [
        LiquifyTool::ForwardWarp,
        LiquifyTool::Reconstruct,
        LiquifyTool::Smooth,
        LiquifyTool::Twirl,
        LiquifyTool::Pucker,
        LiquifyTool::Bloat,
        LiquifyTool::PushLeft,
        LiquifyTool::Freeze,
        LiquifyTool::Thaw,
        LiquifyTool::Hand,
        LiquifyTool::Zoom,
    ];

    fn glyph(self) -> &'static str {
        match self {
            LiquifyTool::ForwardWarp => icon::HAND_POINTING,
            LiquifyTool::Reconstruct => icon::ARROW_COUNTER_CLOCKWISE,
            LiquifyTool::Smooth => icon::WAVE_SINE,
            LiquifyTool::Twirl => icon::SPIRAL,
            LiquifyTool::Pucker => icon::ARROWS_IN,
            LiquifyTool::Bloat => icon::ARROWS_OUT,
            LiquifyTool::PushLeft => icon::ARROW_FAT_LINE_LEFT,
            LiquifyTool::Freeze => icon::SNOWFLAKE,
            LiquifyTool::Thaw => icon::DROP,
            LiquifyTool::Hand => icon::HAND,
            LiquifyTool::Zoom => icon::MAGNIFYING_GLASS,
        }
    }

    fn name(self) -> String {
        match self {
            LiquifyTool::ForwardWarp => t!("liquify-tool-forward-warp"),
            LiquifyTool::Reconstruct => t!("liquify-tool-reconstruct"),
            LiquifyTool::Smooth => t!("liquify-tool-smooth"),
            LiquifyTool::Twirl => t!("liquify-tool-twirl"),
            LiquifyTool::Pucker => t!("liquify-tool-pucker"),
            LiquifyTool::Bloat => t!("liquify-tool-bloat"),
            LiquifyTool::PushLeft => t!("liquify-tool-push-left"),
            LiquifyTool::Freeze => t!("liquify-tool-freeze"),
            LiquifyTool::Thaw => t!("liquify-tool-thaw"),
            LiquifyTool::Hand => t!("tool-hand"),
            LiquifyTool::Zoom => t!("tool-zoom"),
        }
    }

    fn key(self) -> Key {
        match self {
            LiquifyTool::ForwardWarp => Key::W,
            LiquifyTool::Reconstruct => Key::R,
            LiquifyTool::Smooth => Key::E,
            LiquifyTool::Twirl => Key::C,
            LiquifyTool::Pucker => Key::S,
            LiquifyTool::Bloat => Key::B,
            LiquifyTool::PushLeft => Key::O,
            LiquifyTool::Freeze => Key::F,
            LiquifyTool::Thaw => Key::D,
            LiquifyTool::Hand => Key::H,
            LiquifyTool::Zoom => Key::Z,
        }
    }

    /// What the brush does, Alt reversing it: counterclockwise, pushing
    /// right.
    fn effect(self, alt: bool) -> Option<Effect> {
        Some(match self {
            LiquifyTool::ForwardWarp => Effect::Warp,
            LiquifyTool::Reconstruct => Effect::Reconstruct,
            LiquifyTool::Smooth => Effect::Smooth,
            LiquifyTool::Twirl => Effect::Twirl(!alt),
            LiquifyTool::Pucker => Effect::Pucker,
            LiquifyTool::Bloat => Effect::Bloat,
            LiquifyTool::PushLeft => Effect::Push(!alt),
            LiquifyTool::Freeze => Effect::Freeze,
            LiquifyTool::Thaw => Effect::Thaw,
            LiquifyTool::Hand | LiquifyTool::Zoom => return None,
        })
    }

    /// Works while held still, at the Rate.
    fn uses_rate(self) -> bool {
        matches!(
            self,
            LiquifyTool::Reconstruct
                | LiquifyTool::Smooth
                | LiquifyTool::Twirl
                | LiquifyTool::Pucker
                | LiquifyTool::Bloat
        )
    }
}

/// Mesh Size: lines along the longest side.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MeshSize {
    Small,
    Medium,
    Large,
}

impl MeshSize {
    const ALL: [MeshSize; 3] = [MeshSize::Small, MeshSize::Medium, MeshSize::Large];

    fn cells(self) -> f32 {
        match self {
            MeshSize::Small => 64.0,
            MeshSize::Medium => 32.0,
            MeshSize::Large => 16.0,
        }
    }

    fn label(self) -> String {
        match self {
            MeshSize::Small => t!("liquify-mesh-small"),
            MeshSize::Medium => t!("liquify-mesh-medium"),
            MeshSize::Large => t!("liquify-mesh-large"),
        }
    }
}

/// Colors of the mesh and of the mask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GuideColor {
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Magenta,
    Gray,
}

impl GuideColor {
    const ALL: [GuideColor; 8] = [
        GuideColor::Red,
        GuideColor::Orange,
        GuideColor::Yellow,
        GuideColor::Green,
        GuideColor::Cyan,
        GuideColor::Blue,
        GuideColor::Magenta,
        GuideColor::Gray,
    ];

    fn color(self) -> Color32 {
        match self {
            GuideColor::Red => Color32::from_rgb(0xff, 0x00, 0x00),
            GuideColor::Orange => Color32::from_rgb(0xff, 0x80, 0x00),
            GuideColor::Yellow => Color32::from_rgb(0xff, 0xff, 0x00),
            GuideColor::Green => Color32::from_rgb(0x00, 0xff, 0x00),
            GuideColor::Cyan => Color32::from_rgb(0x00, 0xff, 0xff),
            GuideColor::Blue => Color32::from_rgb(0x00, 0x00, 0xff),
            GuideColor::Magenta => Color32::from_rgb(0xff, 0x00, 0xff),
            GuideColor::Gray => Color32::from_rgb(0x80, 0x80, 0x80),
        }
    }

    fn label(self) -> String {
        match self {
            GuideColor::Red => t!("liquify-color-red"),
            GuideColor::Orange => t!("liquify-color-orange"),
            GuideColor::Yellow => t!("liquify-color-yellow"),
            GuideColor::Green => t!("liquify-color-green"),
            GuideColor::Cyan => t!("liquify-color-cyan"),
            GuideColor::Blue => t!("liquify-color-blue"),
            GuideColor::Magenta => t!("liquify-color-magenta"),
            GuideColor::Gray => t!("liquify-color-gray"),
        }
    }
}

/// How the backdrop (the other layers) shows with the layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BackdropMode {
    InFront,
    Behind,
    Blend,
}

impl BackdropMode {
    const ALL: [BackdropMode; 3] = [
        BackdropMode::InFront,
        BackdropMode::Behind,
        BackdropMode::Blend,
    ];

    fn label(self) -> String {
        match self {
            BackdropMode::InFront => t!("liquify-backdrop-in-front"),
            BackdropMode::Behind => t!("liquify-backdrop-behind"),
            BackdropMode::Blend => t!("liquify-backdrop-blend"),
        }
    }
}

/// Liquify's settings, kept for the session (Photoshop's defaults).
#[derive(Clone, Copy, Debug)]
struct Settings {
    tool: LiquifyTool,
    brush: Brush,
    /// Stylus Pressure: a pen's pressure multiplies the brush's.
    stylus: bool,
    show_mesh: bool,
    mesh_size: MeshSize,
    mesh_color: GuideColor,
    show_mask: bool,
    mask_color: GuideColor,
    show_backdrop: bool,
    backdrop_mode: BackdropMode,
    /// 0..100.
    backdrop_opacity: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            tool: LiquifyTool::ForwardWarp,
            brush: Brush::default(),
            stylus: false,
            show_mesh: false,
            mesh_size: MeshSize::Medium,
            mesh_color: GuideColor::Gray,
            show_mask: true,
            mask_color: GuideColor::Red,
            show_backdrop: false,
            backdrop_mode: BackdropMode::InFront,
            backdrop_opacity: 50.0,
        }
    }
}

/// Liquify: its settings, its workspace, the result being computed.
#[derive(Default)]
pub(super) struct LiquifyState {
    settings: Settings,
    workspace: Option<Box<Workspace>>,
    applying: Option<Applying>,
    /// Mesh of the last Liquify applied (Load Last Mesh, Last Filter).
    pub(super) last: Option<Arc<Mesh>>,
}

impl LiquifyState {
    /// The workspace, or the result being computed, keeps the keyboard and
    /// the image.
    pub(super) fn busy(&self) -> bool {
        self.workspace.is_some() || self.applying.is_some()
    }
}

/// The workspace open.
struct Workspace {
    editor: Liquify,
    /// Document and layer it works on, the pixels it distorts and their
    /// place on the canvas.
    doc: u64,
    layer: u64,
    source: Arc<RgbaImage>,
    offset: (i64, i64),
    /// The selection: frozen outside it at the start (Reset too).
    clip: Option<Clip>,
    /// The source reduced for the preview: a pixel per `factor` × `factor`.
    reduced: Arc<RgbaImage>,
    factor: u32,
    /// The canvas's color table: the previews look as the canvas does.
    lut: Option<Arc<crate::cms::Lut>>,
    /// The result, the original (Preview off), the other layers, the
    /// freeze mask (a texel per node).
    preview: Option<TextureHandle>,
    original: Option<TextureHandle>,
    backdrop: Option<TextureHandle>,
    mask: Option<TextureHandle>,
    /// Texels of the preview, and nodes of the mask, to draw again.
    dirty: Option<[u32; 4]>,
    mask_dirty: Option<NodeRect>,
    /// Zoom (physical pixels per image pixel), pan; fit at the next frame.
    zoom: f32,
    pan: Vec2,
    fit: bool,
    /// The canvas's place at the last frame.
    area: Rect,
    /// The brush held down: where it was (pixel space), and when.
    stroke: Option<([f32; 2], f64)>,
    /// Where the last stroke ended: Shift+click strokes in a straight line
    /// from there.
    line_from: Option<[f32; 2]>,
    /// Pen pressure last reported during the stroke.
    pressure: Option<f32>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    preview_on: bool,
    /// Reconstruct… open.
    revert: Option<Revert>,
}

/// Revert Reconstruction: the distortion when it opened, how much of it is
/// kept (0..100).
struct Revert {
    base: Mesh,
    before: Snapshot,
    amount: f32,
}

/// What the buttons of the options asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Request {
    Reconstruct,
    RestoreAll,
    MaskNone,
    MaskAll,
    InvertMask,
    LoadMesh,
    LoadLastMesh,
    SaveMesh,
}

/// The result being computed.
struct Applying {
    doc: u64,
    layer: u64,
    mesh: Arc<Mesh>,
    started: f64,
    progress: Arc<Progress>,
    /// `None`: cancelled.
    rx: mpsc::Receiver<Option<RgbaImage>>,
}

impl Workspace {
    fn frame(&self) -> Frame {
        let f = self.factor as f32;
        Frame {
            origin: [(f - 1.0) / 2.0; 2],
            step: f,
        }
    }

    /// The preview to draw again where the nodes `r` changed (all of it
    /// without), and the mask with `mask`.
    fn changed(&mut self, r: Option<NodeRect>, mask: bool) {
        let (tw, th) = self.reduced.dimensions();
        let (cols, rows) = self.editor.mesh.nodes();
        let [i0, j0, i1, j1] = r.unwrap_or([0, 0, cols, rows]);
        let (s, f) = (self.editor.mesh.spacing(), self.factor);
        // The pixels between the nodes around the changed ones.
        let lo = |n: usize| n.saturating_sub(1) as u32 * s / f;
        let hi = |n: usize, max: u32| ((n as u32 * s + 1).div_ceil(f) + 1).min(max);
        let b = [lo(i0), lo(j0), hi(i1, tw), hi(j1, th)];
        self.dirty = Some(self.dirty.map_or(b, |a| {
            [
                a[0].min(b[0]),
                a[1].min(b[1]),
                a[2].max(b[2]),
                a[3].max(b[3]),
            ]
        }));
        if mask {
            let nodes = [i0, j0, i1, j1];
            self.mask_dirty = Some(self.mask_dirty.map_or(nodes, |a| {
                [a[0].min(i0), a[1].min(j0), a[2].max(i1), a[3].max(j1)]
            }));
        }
    }

    /// Everything to draw again.
    fn changed_all(&mut self) {
        self.changed(None, true);
    }

    /// An undo step, before a change.
    fn push_undo(&mut self, snapshot: Snapshot) {
        self.undo.push(snapshot);
        self.redo.clear();
        let mut bytes: usize = self.undo.iter().map(Snapshot::bytes).sum();
        while self.undo.len() > 1 && (self.undo.len() > MAX_UNDO || bytes > UNDO_BUDGET) {
            bytes -= self.undo.remove(0).bytes();
        }
    }

    /// Ctrl+Z (`true`) or Shift+Ctrl+Z inside the workspace.
    fn step(&mut self, back: bool) {
        let (from, to) = if back {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(snapshot) = from.pop() else { return };
        let extent = self.editor.extent(&snapshot);
        to.push(self.editor.restore(snapshot));
        self.changed(extent, true);
    }

    /// The textures, computed again where needed.
    fn refresh(&mut self, ctx: &egui::Context) {
        let (tw, th) = self.reduced.dimensions();
        let frame = self.frame();
        let render = |r: [u32; 4]| {
            let mesh = Some(&self.editor.mesh);
            let mut rgba = liquify::render_preview(&self.reduced, self.factor, mesh, frame, r);
            color_ui::show_through(self.lut.as_deref(), &mut rgba, true);
            let size = [(r[2] - r[0]) as usize, (r[3] - r[1]) as usize];
            ColorImage::from_rgba_premultiplied(size, &rgba)
        };
        match (&mut self.preview, self.dirty.take()) {
            (None, _) => {
                let image = render([0, 0, tw, th]);
                self.preview = Some(ctx.load_texture("liquify", image, PREVIEW_OPTIONS));
            }
            (Some(texture), Some(r)) if r[0] < r[2] && r[1] < r[3] => {
                let pos = [r[0] as usize, r[1] as usize];
                texture.set_partial(pos, render(r), PREVIEW_OPTIONS);
            }
            _ => {}
        }
        let (cols, rows) = self.editor.mesh.nodes();
        let freeze = self.editor.freeze();
        let mask = |[i0, j0, i1, j1]: NodeRect| {
            let pixels = (j0..j1)
                .flat_map(|j| (i0..i1).map(move |i| (i, j)))
                .map(|(i, j)| {
                    Color32::from_white_alpha((freeze[j * cols + i] * 255.0).round() as u8)
                })
                .collect();
            ColorImage::new([i1 - i0, j1 - j0], pixels)
        };
        match (&mut self.mask, self.mask_dirty.take()) {
            (None, _) => {
                let image = mask([0, 0, cols, rows]);
                self.mask = Some(ctx.load_texture("liquify-mask", image, PREVIEW_OPTIONS));
            }
            (Some(texture), Some(r)) if r[0] < r[2] && r[1] < r[3] => {
                texture.set_partial([r[0], r[1]], mask(r), PREVIEW_OPTIONS);
            }
            _ => {}
        }
        if !self.preview_on && self.original.is_none() {
            let size = [tw as usize, th as usize];
            let mut original = self.reduced.as_raw().clone();
            color_ui::show_through(self.lut.as_deref(), &mut original, false);
            let image = ColorImage::from_rgba_unmultiplied(size, &original);
            self.original = Some(ctx.load_texture("liquify-original", image, PREVIEW_OPTIONS));
        }
    }

    /// Zoom levels, like the canvas's, around `anchor`.
    fn zoom_step(&mut self, zoom_in: bool, anchor: Pos2, center: Pos2) {
        let zoom = if zoom_in {
            ZOOM_LEVELS.into_iter().find(|&z| z > self.zoom * 1.001)
        } else {
            ZOOM_LEVELS
                .into_iter()
                .rev()
                .find(|&z| z < self.zoom / 1.001)
        };
        if let Some(zoom) = zoom {
            self.zoom_around(zoom, anchor, center);
        }
    }

    fn zoom_around(&mut self, zoom: f32, anchor: Pos2, center: Pos2) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let rel = anchor - center - self.pan;
        self.pan += rel - rel * (zoom / self.zoom);
        self.zoom = zoom;
    }
}

/// Smooth when enlarged or reduced: a preview, not the pixels.
const PREVIEW_OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: None,
};

/// Brush size one step up or down ([ and ]): the brushes' steps, then
/// 100 px at a time up to [`MAX_SIZE`].
fn size_step(size: f32, up: bool) -> f32 {
    if size >= 500.0 && (up || size > 500.0) {
        let next = if up {
            (size / 100.0).floor() * 100.0 + 100.0
        } else {
            (size / 100.0).ceil() * 100.0 - 100.0
        };
        return next.clamp(1.0, MAX_SIZE);
    }
    brush_size_step(size, up)
}

impl AtelierApp {
    /// Filter › Liquify…: the workspace, over the active layer (or its
    /// targeted mask); outside the selection, frozen.
    pub(super) fn open_liquify(&mut self) {
        self.open_liquify_with(None);
    }

    /// [`Self::open_liquify`], the distortion starting from `mesh` (a
    /// smart filter edited again).
    pub(super) fn open_liquify_with(&mut self, mesh: Option<&Mesh>) {
        if self.refuse_hidden_target() {
            return;
        }
        let selection = self.selection.clone();
        // On a smart object: a smart filter, over what the filters below
        // it give.
        let smart = self.smart_filter_input();
        let Some(doc) = &mut self.doc else { return };
        if !doc.can_paint() && smart.is_none() {
            self.status = t!("filter-no-pixels");
            return;
        }
        // As for the other filters, the layer is enlarged to the canvas
        // (its pixels don't change; a text or a shape stays editable until
        // the result applies).
        self.enlarge_for_dialog();
        let Some(doc) = &mut self.doc else { return };
        let Some(target) = smart.or_else(|| active_target(doc, selection.as_ref())) else {
            // The layer enlarged above is put back as it was.
            self.shrink_after_preview();
            self.status = t!("status-selection-empty");
            return;
        };
        let (w, h) = target.image.dimensions();
        let mut factor = 1;
        while w.max(h).div_ceil(factor) > PREVIEW_SIDE {
            factor *= 2;
        }
        let reduced = if factor == 1 {
            Arc::clone(&target.image)
        } else {
            Arc::new(reduce_rgba(&target.image, factor))
        };
        let mut editor = Liquify::new(mesh.map_or_else(|| Mesh::new(w, h), |m| m.fitted(w, h)));
        freeze_outside(&mut editor, target.clip.as_ref());
        self.liquify.workspace = Some(Box::new(Workspace {
            editor,
            doc: doc.uid(),
            layer: doc.active_layer().id,
            source: target.image,
            offset: target.offset,
            clip: target.clip,
            reduced,
            factor,
            lut: self.color.shown().1,
            preview: None,
            original: None,
            backdrop: None,
            mask: None,
            dirty: None,
            mask_dirty: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            area: Rect::NOTHING,
            stroke: None,
            line_from: None,
            pressure: None,
            undo: Vec::new(),
            redo: Vec::new(),
            preview_on: true,
            revert: None,
        }));
    }

    /// Last Filter after a Liquify: its mesh again, without the workspace.
    pub(super) fn liquify_again(&mut self) {
        if self.refuse_hidden_target() {
            return;
        }
        if let Some(mesh) = self.liquify.last.clone() {
            self.start_liquify(mesh, None);
        }
    }

    /// Applies `mesh` to the active layer (or its targeted mask), inside
    /// the selection, on another thread: one undo step once done. `frame`:
    /// the offset and size of the pixels it was made on (they change when
    /// a text becomes pixels over the canvas); without, it is stretched to
    /// the layer.
    pub(super) fn start_liquify(
        &mut self,
        mesh: Arc<Mesh>,
        frame: Option<((i64, i64), (u32, u32))>,
    ) {
        if self.refuse_hidden_target() {
            return;
        }
        if self.smart_filters_apply() {
            // Last Filter and the action being recorded get it once
            // rendered (not cancelled).
            let filter = SmartFilterKind::Liquify(Arc::clone(&mesh));
            self.add_smart_filter(filter, move |app| app.liquify_applied(mesh));
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        doc.checkpoint(t!("liquify-title"));
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
        let (w, h) = target.image.dimensions();
        let fitted = match frame {
            Some((offset, size)) if (offset, size) != (target.offset, (w, h)) => {
                let corner = (target.offset.0 - offset.0, target.offset.1 - offset.1);
                Arc::new(mesh.reframed(corner, w, h))
            }
            Some(_) => Arc::clone(&mesh),
            None => Arc::new(mesh.fitted(w, h)),
        };
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let (ctx, shared) = (self.ctx.clone(), Arc::clone(&progress));
        std::thread::spawn(move || {
            let _job = crate::diag::job("Liquify");
            let mut done = true;
            let image = target.process(u32::MAX, |image, _| {
                match liquify::render(image, &fitted, &shared) {
                    Some(out) => *image = out,
                    None => done = false,
                }
            });
            let _ = tx.send(done.then_some(image));
            ctx.request_repaint();
        });
        self.liquify.applying = Some(Applying {
            doc: doc.uid(),
            layer: doc.active_layer().id,
            mesh,
            started: self.ctx.input(|i| i.time),
            progress,
            rx,
        });
    }

    /// Liquify applied with `mesh` (its result in, not cancelled): Last
    /// Filter repeats it, an action being recorded gets it.
    fn liquify_applied(&mut self, mesh: Arc<Mesh>) {
        self.record(crate::actions::Recorded::Liquify {
            mesh: mesh.to_bytes(),
        });
        self.liquify.last = Some(mesh);
        self.filters.forget_last();
    }

    /// The result is in: its pixels replace the layer's.
    fn poll_liquify(&mut self) {
        let Some(applying) = &self.liquify.applying else {
            return;
        };
        // Disconnected: the computation failed (panicked).
        let result = match applying.rx.try_recv() {
            Ok(image) => Ok(image),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err(()),
        };
        let Some(a) = self.liquify.applying.take() else {
            return;
        };
        let name = t!("liquify-title");
        if let Some(doc) = &mut self.doc
            && (doc.uid(), doc.active_layer().id) == (a.doc, a.layer)
        {
            match result {
                Ok(Some(image)) => {
                    doc.active_layer_mut().replace_pixels(image);
                    doc.end_paint();
                    self.status = t!("filter-applied", name = name.as_str());
                    self.liquify_applied(a.mesh);
                }
                Ok(None) => {
                    doc.end_paint();
                    doc.discard_step();
                    self.status = t!("retouch-status-cancelled", name = name.as_str());
                    self.actions.step_stopped(Stopped::Cancelled);
                    self.shrink_after_preview();
                }
                Err(()) => {
                    doc.end_paint();
                    doc.discard_step();
                    // The name in English too, for the log and the report.
                    let name = crate::i18n::Message::of(|| t!("liquify-title"));
                    let text = tm!("fx3-filter-failed", name = name);
                    self.actions
                        .step_stopped(Stopped::Failed(text.shown.clone()));
                    self.status_failed(text);
                    self.shrink_after_preview();
                }
            }
        }
        self.reset_pending();
    }

    /// The workspace, or the progress of the result being computed.
    pub(super) fn liquify_window(&mut self, ctx: &egui::Context) {
        self.poll_liquify();
        if let Some(a) = &self.liquify.applying {
            if liquify_progress(ctx, a) {
                a.progress.cancel();
            }
            return;
        }
        let Some(mut ws) = self.liquify.workspace.take() else {
            return;
        };
        // Its document was closed, or another layer is active (undone).
        let Some(doc) = self
            .doc
            .as_ref()
            .filter(|d| (d.uid(), d.active_layer().id) == (ws.doc, ws.layer))
        else {
            return;
        };
        let mut settings = self.liquify.settings;
        if settings.show_backdrop && ws.backdrop.is_none() {
            let (w, h) = ws.source.dimensions();
            let (x, y) = ws.offset;
            let others = doc.composite(PixelRect { x, y, w, h }, Some(doc.active));
            let mut others = reduce_rgba(&others, ws.factor);
            color_ui::show_through(ws.lut.as_deref(), others.as_mut(), false);
            let size = [others.width() as usize, others.height() as usize];
            let image = ColorImage::from_rgba_unmultiplied(size, others.as_raw());
            ws.backdrop = Some(ctx.load_texture("liquify-backdrop", image, PREVIEW_OPTIONS));
        }
        if ws.revert.is_none() {
            workspace_keys(ctx, &mut ws, &mut settings);
        }
        let screen = ctx.content_rect();
        let mut preview = ws.preview_on;
        let mut request = None;
        let has_last = self.liquify.last.is_some();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: (screen.width() - 48.0).max(560.0),
                reset: Some(!ws.undo.is_empty()),
                preview: Some(&mut preview),
                ..DialogSpec::new("liquify", t!("liquify-title"), screen.center())
            },
            |ui| {
                let height = dialog_body_height(ctx);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = vec2(12.0, 4.0);
                    tool_column(ui, &mut settings);
                    let width = (ui.available_width() - OPTIONS_WIDTH - 12.0).max(120.0);
                    canvas(ui, &mut ws, &settings, vec2(width, height));
                    egui::ScrollArea::vertical()
                        .id_salt("liquify_options")
                        .max_height(height)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.set_width(OPTIONS_WIDTH);
                                request = options(ui, &mut settings, has_last);
                            });
                        });
                });
            },
        );
        self.liquify.settings = settings;
        if preview != ws.preview_on {
            ws.preview_on = preview;
        }
        if out.reset {
            let snapshot = ws.editor.snapshot();
            ws.push_undo(snapshot);
            ws.editor.restore_all();
            freeze_outside(&mut ws.editor, ws.clip.as_ref());
            ws.changed_all();
        }
        if let Some(request) = request {
            self.liquify_request(&mut ws, request);
        }
        if ws.revert.is_some() {
            revert_window(ctx, &mut ws);
        }
        if out.ok && !ws.editor.mesh.is_identity() {
            let frame = (ws.offset, ws.source.dimensions());
            self.start_liquify(Arc::new(ws.editor.mesh.clone()), Some(frame));
        } else if out.ok || out.cancel {
            self.shrink_after_preview();
        } else {
            self.liquify.workspace = Some(ws);
        }
    }

    /// A button of the options: an undo step of the workspace, except for
    /// the windows they open.
    fn liquify_request(&mut self, ws: &mut Workspace, request: Request) {
        let (w, h) = ws.source.dimensions();
        // A mesh made for another image is stretched to this one.
        let loaded = match request {
            Request::Reconstruct => {
                ws.revert = Some(Revert {
                    base: ws.editor.mesh.clone(),
                    before: ws.editor.snapshot(),
                    amount: 100.0,
                });
                return;
            }
            Request::SaveMesh => {
                self.save_mesh(&ws.editor.mesh);
                return;
            }
            Request::LoadMesh => match self.load_mesh() {
                Some(mesh) => Some(mesh.fitted(w, h)),
                None => return,
            },
            Request::LoadLastMesh => match &self.liquify.last {
                Some(mesh) => Some(mesh.fitted(w, h)),
                None => return,
            },
            Request::RestoreAll | Request::MaskNone | Request::MaskAll | Request::InvertMask => {
                None
            }
        };
        let snapshot = ws.editor.snapshot();
        ws.push_undo(snapshot);
        match (request, loaded) {
            (_, Some(mesh)) => ws.editor.mesh = mesh,
            (Request::RestoreAll, _) => ws.editor.restore_all(),
            (Request::MaskNone, _) => ws.editor.mask_all(false),
            (Request::MaskAll, _) => ws.editor.mask_all(true),
            (Request::InvertMask, _) => ws.editor.invert_mask(),
            _ => {}
        }
        ws.changed_all();
    }

    /// Save Mesh…: the distortion in a file, to apply it again later.
    fn save_mesh(&mut self, mesh: &Mesh) {
        let filter = t!("liquify-mesh-files");
        let mut dialog = rfd::FileDialog::new()
            .set_title(t!("liquify-save-mesh-title"))
            .set_file_name(format!("{}.{MESH_EXTENSION}", t!("liquify-mesh-file-name")))
            .add_filter(filter, &[MESH_EXTENSION]);
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(mut path) = dialog.save_file() else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension(MESH_EXTENSION);
        }
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let file = file.unwrap_or_default();
        self.status = match std::fs::write(&path, mesh.to_bytes()) {
            Ok(()) => t!("liquify-status-mesh-saved", file = file.as_str()),
            Err(e) => t!("liquify-status-mesh-error", error = e.to_string()),
        };
    }

    /// Load Mesh…: a mesh saved earlier (stretched to the layer).
    fn load_mesh(&mut self) -> Option<Mesh> {
        let mut dialog = rfd::FileDialog::new()
            .set_title(t!("liquify-load-mesh-title"))
            .add_filter(t!("liquify-mesh-files"), &[MESH_EXTENSION])
            .add_filter(t!("app-file-filter-all"), &["*"]);
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        let path = dialog.pick_file()?;
        match std::fs::read(&path) {
            Ok(bytes) => {
                let mesh = Mesh::from_bytes(&bytes);
                if mesh.is_none() {
                    self.status = t!("liquify-status-not-a-mesh");
                }
                mesh
            }
            Err(e) => {
                self.status = t!("liquify-status-mesh-error", error = e.to_string());
                None
            }
        }
    }
}

/// Frozen outside `clip` (the selection), thawed inside.
fn freeze_outside(editor: &mut Liquify, clip: Option<&Clip>) {
    match clip {
        Some(clip) => editor.freeze_from(|x, y| {
            let at = |v: f32| v.round().max(0.0) as u32;
            1.0 - clip.coverage(at(x), at(y)) as f32 / 255.0
        }),
        None => editor.mask_all(false),
    }
}

/// Keys of the workspace: the tools' letters, [ and ], zoom, its own undo.
fn workspace_keys(ctx: &egui::Context, ws: &mut Workspace, settings: &mut Settings) {
    if ctx.egui_wants_keyboard_input() || egui::Popup::is_any_open(ctx) {
        return;
    }
    let center = ws.area.center();
    ctx.input_mut(|i| {
        if let Some(tool) = LiquifyTool::ALL
            .into_iter()
            .find(|t| key_typed(i, Modifiers::NONE, t.key()))
        {
            settings.tool = tool;
        }
        if let Some((up, m)) = bracket_press(i)
            && m.is_none()
        {
            settings.brush.size = size_step(settings.brush.size, up);
        }
        match take_shortcut(i, ZOOM_SHORTCUTS) {
            Some(Action::ZoomIn) => ws.zoom_step(true, center, center),
            Some(Action::ZoomOut) => ws.zoom_step(false, center, center),
            Some(Action::Fit) => ws.fit = true,
            Some(Action::ActualSize) => {
                ws.pan = Vec2::ZERO;
                ws.zoom = 1.0;
            }
            _ => {}
        }
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let back = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
        if ws.stroke.is_none() {
            if consume_exact(i, undo) || consume_exact(i, back) {
                ws.step(true);
            } else if consume_exact(i, redo) {
                ws.step(false);
            }
        }
    });
}

/// The tools, in a column.
fn tool_column(ui: &mut egui::Ui, settings: &mut Settings) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        for tool in LiquifyTool::ALL {
            let key = keys::key(tool.key());
            let name = tool.name();
            if ui_kit::tool_button(ui, tool.glyph(), &name, &key, settings.tool == tool).clicked() {
                settings.tool = tool;
            }
            if tool == LiquifyTool::PushLeft || tool == LiquifyTool::Thaw {
                ui.add_space(6.0);
            }
        }
    });
}

/// The layer being distorted: drawn with the backdrop, the mask and the
/// mesh; the brushes, zoom and pan.
fn canvas(ui: &mut egui::Ui, ws: &mut Workspace, settings: &Settings, size: Vec2) {
    let (area, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    ws.area = area;
    let ppp = ctx.pixels_per_point();
    let (w, h) = ws.source.dimensions();
    if ws.fit && area.width() > 0.0 {
        let fit = ((area.width() - 32.0) / w as f32).min((area.height() - 32.0) / h as f32);
        ws.zoom = (fit * ppp).clamp(MIN_ZOOM, 1.0);
        ws.pan = Vec2::ZERO;
        ws.fit = false;
    }
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let (alt, space) = ctx.input(|i| (i.modifiers.alt, i.key_down(Key::Space)));
    let space = space && !ctx.egui_wants_keyboard_input();

    // Wheel: scrolling, zoom with Alt (or Ctrl, or a pinch), as on the canvas.
    if response.hovered()
        && let Some(pos) = pointer
    {
        let (scroll, pinch) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta()));
        let wheel_zoom = if alt { scroll.y + scroll.x } else { 0.0 };
        let factor = pinch * (wheel_zoom * WHEEL_ZOOM_RATE).exp();
        if factor != 1.0 {
            ws.zoom_around(ws.zoom * factor, pos, area.center());
        }
        if !alt {
            ws.pan += scroll;
        }
    }
    let hand = space || settings.tool == LiquifyTool::Hand;
    if response.dragged_by(PointerButton::Middle)
        || (hand && ws.stroke.is_none() && response.dragged_by(PointerButton::Primary))
    {
        ws.pan += response.drag_delta();
    }
    let shown = vec2(w as f32, h as f32) * ws.zoom / ppp;
    let corner = area.center() + ws.pan - shown / 2.0;
    let view = View {
        origin: (corner.to_vec2() * ppp).round().to_pos2() / ppp,
        zoom: ws.zoom / ppp,
    };
    if settings.tool == LiquifyTool::Zoom
        && !space
        && response.clicked()
        && let Some(pos) = pointer
    {
        ws.zoom_step(!alt, pos, area.center());
    }
    brush_input(&ctx, ws, settings, &response, view, hand, alt);
    ws.refresh(&ctx);

    // Drawing.
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 8.0, color::BG0);
    let image_rect = Rect::from_min_size(view.origin, vec2(w as f32, h as f32) * view.zoom);
    let painter = painter.with_clip_rect(area.intersect(image_rect));
    draw_checker(&painter, image_rect);
    let (tw, th) = ws.reduced.dimensions();
    let f = ws.factor as f32;
    let texels = Rect::from_min_size(view.origin, vec2(tw as f32, th as f32) * f * view.zoom);
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let opacity = settings.backdrop_opacity / 100.0;
    let backdrop = ws.backdrop.as_ref().filter(|_| settings.show_backdrop);
    let draw = |texture: &TextureHandle, alpha: f32| {
        painter.image(
            texture.id(),
            texels,
            uv,
            Color32::WHITE.gamma_multiply(alpha),
        );
    };
    let layer_alpha = match (backdrop, settings.backdrop_mode) {
        (Some(b), BackdropMode::Behind) => {
            draw(b, opacity);
            1.0
        }
        (Some(b), BackdropMode::Blend) => {
            draw(b, 1.0);
            1.0 - opacity
        }
        _ => 1.0,
    };
    let layer = if ws.preview_on {
        ws.preview.as_ref()
    } else {
        ws.original.as_ref()
    };
    if let Some(layer) = layer {
        draw(layer, layer_alpha);
    }
    if let Some(b) = backdrop
        && settings.backdrop_mode == BackdropMode::InFront
    {
        draw(b, opacity);
    }
    if settings.show_mask
        && let Some(mask) = &ws.mask
    {
        // A texel per node, centered on it.
        let s = ws.editor.mesh.spacing() as f32;
        let (cols, rows) = ws.editor.mesh.nodes();
        let min = pos2(0.5 - s / 2.0, 0.5 - s / 2.0);
        let max = min + vec2(cols as f32, rows as f32) * s;
        let rect = Rect::from_min_max(view.to_screen(min), view.to_screen(max));
        let tint = settings.mask_color.color().gamma_multiply(0.5);
        painter.image(mask.id(), rect, uv, tint);
    }
    if settings.show_mesh && ws.preview_on {
        draw_mesh(&painter, view, &ws.editor.mesh, settings);
    }

    // The brush, or the tool's cursor.
    let hovered = response.hovered() || ws.stroke.is_some();
    if hovered {
        let pointer = pointer.filter(|p| area.contains(*p));
        let cursor = if hand {
            if response.dragged() {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            }
        } else if settings.tool == LiquifyTool::Zoom {
            if alt {
                CursorIcon::ZoomOut
            } else {
                CursorIcon::ZoomIn
            }
        } else if let Some(p) = pointer {
            let radius = settings.brush.size / 2.0 * view.zoom;
            let outer = ui.painter_at(area);
            outer.circle_stroke(p, radius, UiStroke::new(1.5, Color32::BLACK));
            outer.circle_stroke(
                p,
                (radius - 1.0).max(0.5),
                UiStroke::new(1.0, Color32::WHITE),
            );
            if radius < 3.0 {
                CursorIcon::Crosshair
            } else {
                CursorIcon::None
            }
        } else {
            CursorIcon::Default
        };
        ctx.set_cursor_icon(cursor);
    }
    let zoom = i18n::percent(i18n::short_decimal(f64::from(ws.zoom) * 100.0));
    ui_kit::pill(
        &ui.painter_at(area),
        area.left_bottom() + vec2(10.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        &zoom,
    );
}

/// The brush on the canvas: a stroke from the press to the release, its
/// positions as the system reports them, the tools over time going on while
/// it is held still.
fn brush_input(
    ctx: &egui::Context,
    ws: &mut Workspace,
    settings: &Settings,
    response: &egui::Response,
    view: View,
    hand: bool,
    alt: bool,
) {
    let effect = settings.tool.effect(alt);
    let (pressed, down, now) = ctx.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.time,
        )
    });
    // Pixel space: pixel (x, y) is the point (x, y).
    let to_mesh = |p: Pos2| {
        let p = view.to_image(p);
        [p.x - 0.5, p.y - 0.5]
    };
    let pointer = ctx.input(|i| i.pointer.interact_pos());
    let Some((mut last, since)) = ws.stroke else {
        if let Some(effect) = effect
            && pressed
            && !hand
            && response.hovered()
            && let Some(p) = pointer
        {
            let at = to_mesh(p);
            let from = match ws.line_from {
                Some(from) if ctx.input(|i| i.modifiers.shift) => from,
                _ => at,
            };
            ws.editor.begin_stroke();
            ws.pressure = None;
            ws.stroke = Some((at, now));
            let changed = ws
                .editor
                .stroke(effect, &settings.brush, from, at, 0.0, 1.0);
            ws.changed(changed, is_mask(effect));
        }
        return;
    };
    // Released, or another tool chosen meanwhile (H, Z).
    let Some(effect) = effect.filter(|_| down) else {
        if let Some(snapshot) = ws.editor.end_stroke() {
            ws.push_undo(snapshot);
        }
        ws.stroke = None;
        ws.line_from = Some(last);
        return;
    };
    let samples = frame_samples(ctx, pointer);
    let mut points: Vec<([f32; 2], Option<f32>)> = samples
        .into_iter()
        .map(|(p, force)| (to_mesh(p), force))
        .collect();
    if points.is_empty() {
        points.push((last, None));
    }
    // A fast pointer gives many events a frame: they make one path, dabbed
    // at the brush's spacing (not a whole dab per event).
    points = spaced(points, last, settings.brush.spacing());
    // A frame that took long counts for no more than a tenth of a second.
    let dt = ((now - since) as f32).min(0.1) / points.len() as f32;
    for (at, force) in points {
        if force.is_some() {
            ws.pressure = force;
        }
        let strength = if settings.stylus {
            ws.pressure.unwrap_or(1.0)
        } else {
            1.0
        };
        let changed = ws
            .editor
            .stroke(effect, &settings.brush, last, at, dt, strength);
        ws.changed(changed, is_mask(effect));
        last = at;
    }
    ws.stroke = Some((last, now));
    // The tools over time go on while held still.
    ctx.request_repaint();
}

/// The points of `path` (after `from`) at least `spacing` apart, the last
/// one always kept, with the latest pressure of those dropped.
fn spaced(
    path: Vec<([f32; 2], Option<f32>)>,
    from: [f32; 2],
    spacing: f32,
) -> Vec<([f32; 2], Option<f32>)> {
    let n = path.len();
    let mut kept: Vec<([f32; 2], Option<f32>)> = Vec::with_capacity(n);
    let (mut previous, mut force) = (from, None);
    for (k, (at, f)) in path.into_iter().enumerate() {
        force = f.or(force);
        let far = (at[0] - previous[0]).hypot(at[1] - previous[1]) >= spacing;
        if far || k + 1 == n {
            kept.push((at, force));
            previous = at;
            force = None;
        }
    }
    kept
}

/// Paints the freeze mask (else the mesh).
fn is_mask(effect: Effect) -> bool {
    matches!(effect, Effect::Freeze | Effect::Thaw)
}

/// Show Mesh: its lines where the pixels went.
fn draw_mesh(painter: &Painter, view: View, mesh: &Mesh, settings: &Settings) {
    let (w, h) = mesh.size();
    let cell = (w.max(h) as f32 / settings.mesh_size.cells()).max(4.0);
    let stroke = UiStroke::new(1.0, settings.mesh_color.color());
    let to_screen = |p: [f32; 2]| {
        let p = mesh.displaced(p);
        view.to_screen(pos2(p[0] + 0.5, p[1] + 0.5))
    };
    let (w, h) = ((w - 1) as f32, (h - 1) as f32);
    let line = |from: [f32; 2], to: [f32; 2]| {
        let n = ((to[0] - from[0] + to[1] - from[1]) / cell * 4.0)
            .ceil()
            .max(1.0) as usize;
        let points = (0..=n)
            .map(|k| {
                let t = k as f32 / n as f32;
                to_screen([
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ])
            })
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    };
    let mut x = 0.0;
    while x <= w {
        line([x, 0.0], [x, h]);
        x += cell;
    }
    let mut y = 0.0;
    while y <= h {
        line([0.0, y], [w, y]);
        y += cell;
    }
}

/// The options: brush, reconstruction, mask, meshes, view.
fn options(ui: &mut egui::Ui, s: &mut Settings, has_last: bool) -> Option<Request> {
    let mut request = None;
    let percent = i18n::percent("");
    let percent_row =
        |ui: &mut egui::Ui, label: String, value: &mut f32, min: f32, default: f32| {
            let mut shown = *value * 100.0;
            slider_row(
                ui,
                &label,
                None,
                &mut shown,
                min..=100.0,
                &percent,
                Track::Accent,
                default,
            );
            *value = shown / 100.0;
        };
    ui.spacing_mut().item_spacing.y = 8.0;
    ui_kit::section(ui, &t!("liquify-brush-options"));
    let steps = Steps {
        decimals: 0,
        exponent: 3.0,
    };
    let b = &mut s.brush;
    let label = t!("liquify-size");
    slider_row_steps(
        ui,
        &label,
        None,
        &mut b.size,
        1.0..=MAX_SIZE,
        " px",
        Track::Accent,
        100.0,
        steps,
    );
    percent_row(ui, t!("liquify-density"), &mut b.density, 0.0, 50.0);
    percent_row(ui, t!("liquify-pressure"), &mut b.pressure, 1.0, 100.0);
    ui.add_enabled_ui(s.tool.uses_rate(), |ui| {
        percent_row(ui, t!("liquify-rate"), &mut b.rate, 0.0, 80.0);
    });
    ui_kit::checkbox(ui, &mut s.stylus, &t!("liquify-stylus-pressure"));
    ui_kit::checkbox(ui, &mut b.pin_edges, &t!("liquify-pin-edges"));

    let mut buttons = |ui: &mut egui::Ui, items: &[(Request, String, bool)]| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (r, label, enabled) in items {
                if ui_kit::button(ui, label, Kind::Normal, None, *enabled).clicked() {
                    request = Some(*r);
                }
            }
        });
    };
    gap_between_blocks(ui);
    ui_kit::section(ui, &t!("liquify-reconstruct-options"));
    buttons(
        ui,
        &[
            (Request::Reconstruct, t!("liquify-reconstruct"), true),
            (Request::RestoreAll, t!("liquify-restore-all"), true),
        ],
    );
    gap_between_blocks(ui);
    ui_kit::section(ui, &t!("liquify-mask-options"));
    buttons(
        ui,
        &[
            (Request::MaskNone, t!("liquify-mask-none"), true),
            (Request::MaskAll, t!("liquify-mask-all"), true),
            (Request::InvertMask, t!("liquify-invert-all"), true),
        ],
    );
    gap_between_blocks(ui);
    ui_kit::section(ui, &t!("liquify-load-mesh-options"));
    buttons(
        ui,
        &[
            (Request::LoadMesh, t!("liquify-load-mesh"), true),
            (
                Request::LoadLastMesh,
                t!("liquify-load-last-mesh"),
                has_last,
            ),
            (Request::SaveMesh, t!("liquify-save-mesh"), true),
        ],
    );

    gap_between_blocks(ui);
    ui_kit::section(ui, &t!("liquify-view-options"));
    let width = ui.available_width();
    let half = (width - 8.0) / 2.0;
    ui_kit::checkbox(ui, &mut s.show_mesh, &t!("liquify-show-mesh"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (label, size) = (t!("liquify-mesh-size"), s.mesh_size.label());
        ui_kit::combo(
            ui,
            "liquify_mesh_size",
            Some(&label),
            &size,
            half - 8.0,
            |ui| {
                for m in MeshSize::ALL {
                    ui.selectable_value(&mut s.mesh_size, m, m.label());
                }
            },
        );
        color_combo(ui, "liquify_mesh_color", &mut s.mesh_color, half - 8.0);
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui_kit::checkbox(ui, &mut s.show_mask, &t!("liquify-show-mask"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            color_combo(ui, "liquify_mask_color", &mut s.mask_color, half - 8.0);
        });
    });
    ui_kit::checkbox(ui, &mut s.show_backdrop, &t!("liquify-show-backdrop"));
    ui.add_enabled_ui(s.show_backdrop, |ui| {
        let mode = s.backdrop_mode.label();
        let label = t!("liquify-backdrop-mode");
        ui_kit::combo(
            ui,
            "liquify_backdrop_mode",
            Some(&label),
            &mode,
            width - 8.0,
            |ui| {
                for m in BackdropMode::ALL {
                    ui.selectable_value(&mut s.backdrop_mode, m, m.label());
                }
            },
        );
        let label = t!("liquify-backdrop-opacity");
        let unit = i18n::percent("");
        slider_row(
            ui,
            &label,
            None,
            &mut s.backdrop_opacity,
            0.0..=100.0,
            &unit,
            Track::Accent,
            50.0,
        );
    });
    request
}

fn color_combo(ui: &mut egui::Ui, id: &str, value: &mut GuideColor, width: f32) {
    let (label, shown) = (t!("liquify-color"), value.label());
    ui_kit::combo(ui, id, Some(&label), &shown, width, |ui| {
        for c in GuideColor::ALL {
            ui.selectable_value(value, c, c.label());
        }
    });
}

/// Reconstruct…: how much of the distortion stays, previewed.
fn revert_window(ctx: &egui::Context, ws: &mut Workspace) {
    let Some(mut revert) = ws.revert.take() else {
        return;
    };
    let before = revert.amount;
    let out = floating_dialog(
        ctx,
        DialogSpec {
            pivot: egui::Align2::CENTER_CENTER,
            ..DialogSpec::new(
                "liquify_revert",
                t!("liquify-revert-title"),
                ctx.content_rect().center(),
            )
        },
        |ui| {
            let label = t!("liquify-amount");
            let unit = i18n::percent("");
            slider_row(
                ui,
                &label,
                None,
                &mut revert.amount,
                0.0..=100.0,
                &unit,
                Track::Accent,
                100.0,
            );
        },
    );
    if revert.amount != before {
        ws.editor.revert(&revert.base, revert.amount / 100.0);
        ws.changed_all();
    }
    if out.ok {
        ws.push_undo(revert.before);
    } else if out.cancel {
        ws.editor.restore(revert.before);
        ws.changed_all();
    } else {
        ws.revert = Some(revert);
    }
}

/// The result being computed: after a moment, its progress and Cancel.
/// Returns `true` when Cancel is clicked.
fn liquify_progress(ctx: &egui::Context, a: &Applying) -> bool {
    let elapsed = ctx.input(|i| i.time) - a.started;
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
    egui::Modal::new(egui::Id::new("liquify_progress"))
        .backdrop_color(Color32::TRANSPARENT)
        .frame(frame)
        .show(ctx, |ui| {
            if !visible {
                return;
            }
            ui.set_width(280.0);
            let name = t!("liquify-title");
            ui.label(egui::RichText::new(format!("{name}…")).color(color::TEXT));
            ui.add_space(8.0);
            ui.add(egui::ProgressBar::new(a.progress.fraction()).desired_height(6.0));
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
    use egui_kittest::Harness;

    /// A 200 × 150 document: black, a white 9 × 9 square around (60, 75).
    fn app() -> Harness<'static, AtelierApp> {
        let mut harness = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                let image = RgbaImage::from_fn(200, 150, |x, y| {
                    if x.abs_diff(60) <= 4 && y.abs_diff(75) <= 4 {
                        image::Rgba([255, 255, 255, 255])
                    } else {
                        image::Rgba([0, 0, 0, 255])
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

    fn steps(h: &Harness<'static, AtelierApp>) -> u64 {
        h.state().doc.as_ref().unwrap().history_step()
    }

    fn workspace<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Workspace {
        h.state()
            .liquify
            .workspace
            .as_deref()
            .expect("workspace open")
    }

    /// Center of the white pixels, along x.
    fn feature_x(image: &RgbaImage) -> f32 {
        let (mut sum, mut n) = (0.0, 0.0);
        for (x, _, p) in image.enumerate_pixels() {
            sum += x as f32 * p.0[0] as f32;
            n += p.0[0] as f32;
        }
        sum / n
    }

    /// Where pixel `p` of the layer shows in the workspace.
    fn on_screen(h: &Harness<'static, AtelierApp>, p: Pos2) -> Pos2 {
        let ws = workspace(h);
        let ppp = h.ctx.pixels_per_point();
        let (w, hh) = ws.source.dimensions();
        let zoom = ws.zoom / ppp;
        let corner = ws.area.center() + ws.pan - vec2(w as f32, hh as f32) * zoom / 2.0;
        let corner = (corner.to_vec2() * ppp).round().to_pos2() / ppp;
        corner + (p.to_vec2() + vec2(0.5, 0.5)) * zoom
    }

    /// A drag on the workspace's canvas, from `from` to `to` (layer pixels).
    fn drag(h: &mut Harness<'static, AtelierApp>, from: Pos2, to: Pos2) {
        let (a, b) = (on_screen(h, from), on_screen(h, to));
        h.hover_at(a);
        h.run_steps(1);
        h.drag_at(a);
        h.run_steps(1);
        for i in 1..=6 {
            h.hover_at(a + (b - a) * i as f32 / 6.0);
            h.run_steps(1);
        }
        h.event(egui::Event::PointerButton {
            pos: b,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(2);
    }

    /// Runs frames until the result is in.
    fn settle(h: &mut Harness<'static, AtelierApp>) {
        for _ in 0..500 {
            h.run_steps(1);
            if h.state().liquify.applying.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("Liquify not applied");
    }

    #[test]
    fn the_filter_menu_and_its_shortcut_open_the_workspace() {
        let mut h = app();
        let (_, entries) = h.state().filter_menu();
        assert!(entries.iter().any(|e| matches!(
            e,
            MenuEntry::Item(_, true, Some(s), Action::Liquify) if **s == SHORTCUT_LIQUIFY
        )));
        h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::X);
        h.run_steps(2);
        assert!(h.state().modal_open(), "the workspace keeps the keyboard");
        let ws = workspace(&h);
        assert!(ws.preview.is_some() && ws.mask.is_some());
        assert!(ws.area.width() > 600.0, "a large workspace: {:?}", ws.area);
        // Its tools' letters, and [ ].
        h.key_press(Key::S);
        h.key_press(Key::CloseBracket);
        h.run_steps(1);
        let settings = h.state().liquify.settings;
        assert_eq!(settings.tool, LiquifyTool::Pucker);
        assert_eq!(settings.brush.size, 125.0);
        h.key_press(Key::W);
        h.key_press(Key::OpenBracket);
        h.run_steps(1);
        // Zoom: Ctrl++ and Ctrl+0.
        let fitted = workspace(&h).zoom;
        h.key_press_modifiers(Modifiers::COMMAND, Key::Plus);
        h.run_steps(1);
        assert!(workspace(&h).zoom > fitted);
        h.key_press_modifiers(Modifiers::COMMAND, Key::Num0);
        h.run_steps(2);
        assert_eq!(workspace(&h).zoom, fitted);
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(h.state().liquify.workspace.is_none());
    }

    /// As egui-winit reports it: the modifiers, a cut instead of the press
    /// of `key`, then its release.
    fn cut_with(h: &mut Harness<'static, AtelierApp>, key: Key) {
        let m = Modifiers::COMMAND.plus(Modifiers::SHIFT);
        h.event(egui::Event::ModifiersChanged(m));
        h.event(egui::Event::Cut);
        h.event(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: false,
            repeat: false,
            modifiers: m,
        });
        h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(2);
    }

    #[test]
    fn shift_ctrl_x_opens_liquify_and_shift_ctrl_delete_fills() {
        let mut h = app();
        let original = pixels(&h);
        // Shift+Ctrl+Delete (a cut too, on Windows): the background color,
        // preserving transparency, as Shift+Ctrl+Backspace.
        cut_with(&mut h, Key::Delete);
        assert!(h.state().liquify.workspace.is_none());
        let filled = pixels(&h);
        assert_ne!(filled, original);
        let [r, g, b, _] = h.state().secondary.to_srgba_unmultiplied();
        assert_eq!(filled.get_pixel(5, 5).0, [r, g, b, 255]);
        // Shift+Ctrl+X.
        cut_with(&mut h, Key::X);
        assert!(h.state().liquify.workspace.is_some());
        assert_eq!(pixels(&h), filled);
    }

    #[test]
    fn a_stroke_applies_in_one_step_and_last_filter_repeats_it() {
        let mut h = app();
        let original = pixels(&h);
        let before = steps(&h);
        h.state_mut().run(Action::Liquify);
        h.run_steps(2);
        h.state_mut().liquify.settings.brush.size = 60.0;
        drag(&mut h, pos2(60.0, 75.0), pos2(90.0, 75.0));
        let ws = workspace(&h);
        assert!(ws.stroke.is_none(), "stroke over");
        assert_eq!(ws.undo.len(), 1);
        // Moved with the brush, on the mesh (nothing applied yet).
        let o = ws.editor.mesh.offset_at(90.0, 75.0);
        assert!(o[0] < -20.0, "{o:?}");
        assert_eq!(pixels(&h), original);
        // The workspace's own undo and redo.
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        h.run_steps(1);
        assert!(workspace(&h).editor.mesh.is_identity());
        h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
        h.run_steps(1);
        assert!(!workspace(&h).editor.mesh.is_identity());
        h.key_press(Key::Enter);
        settle(&mut h);
        assert!(h.state().liquify.workspace.is_none());
        let once = pixels(&h);
        let x = feature_x(&once);
        assert!(x > 80.0, "moved right: {x}");
        assert_eq!(steps(&h), before + 1, "one undo step");
        assert!(h.state().liquify.last.is_some());
        // Alt+Ctrl+F: the same mesh again, without the workspace.
        h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::ALT), Key::F);
        h.run_steps(1);
        settle(&mut h);
        assert!(h.state().liquify.workspace.is_none());
        assert_ne!(pixels(&h), once);
        h.state_mut().undo();
        assert_eq!(pixels(&h), once);
        h.state_mut().undo();
        assert_eq!(pixels(&h), original);
    }

    #[test]
    fn many_pointer_events_make_dabs_at_the_brush_spacing() {
        // Sixteen events a pixel apart, a spacing of 4: four points, the
        // last one kept, the pressure of those dropped carried along.
        let path: Vec<([f32; 2], Option<f32>)> = (1..=16)
            .map(|i| ([i as f32, 0.0], (i == 2).then_some(0.5)))
            .collect();
        let kept = spaced(path, [0.0, 0.0], 4.0);
        let xs: Vec<f32> = kept.iter().map(|(p, _)| p[0]).collect();
        assert_eq!(xs, [4.0, 8.0, 12.0, 16.0]);
        assert_eq!(kept[0].1, Some(0.5));
        // Slow: every event.
        let kept = spaced(vec![([10.0, 0.0], None)], [0.0, 0.0], 4.0);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn cancel_changes_nothing() {
        let mut h = app();
        let original = pixels(&h);
        let before = steps(&h);
        h.state_mut().run(Action::Liquify);
        h.run_steps(2);
        drag(&mut h, pos2(60.0, 75.0), pos2(80.0, 90.0));
        assert!(!workspace(&h).editor.mesh.is_identity());
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(!h.state().liquify.busy());
        assert_eq!(pixels(&h), original);
        assert_eq!(steps(&h), before);
        assert!(h.state().liquify.last.is_none());
    }

    #[test]
    fn freeze_mask_tool_and_selection_protect_pixels() {
        let mut h = app();
        let original = pixels(&h);
        // Selected: the right part; the dot (left) frozen from the start.
        h.state_mut().selection = Some(
            PixelRect {
                x: 100,
                y: 0,
                w: 100,
                h: 150,
            }
            .into(),
        );
        h.state_mut().run(Action::Liquify);
        h.run_steps(2);
        let ws = workspace(&h);
        let (cols, _) = ws.editor.mesh.nodes();
        assert_eq!(ws.editor.freeze()[75 * cols + 60], 1.0);
        assert_eq!(ws.editor.freeze()[75 * cols + 150], 0.0);
        drag(&mut h, pos2(60.0, 75.0), pos2(90.0, 75.0));
        assert_eq!(workspace(&h).editor.mesh.offset_at(60.0, 75.0), [0.0; 2]);
        // Thawed with the Thaw Mask tool: it moves.
        h.key_press(Key::D);
        h.state_mut().liquify.settings.brush.size = 200.0;
        drag(&mut h, pos2(60.0, 75.0), pos2(61.0, 75.0));
        h.key_press(Key::W);
        h.state_mut().liquify.settings.brush.size = 60.0;
        drag(&mut h, pos2(60.0, 75.0), pos2(90.0, 75.0));
        assert_ne!(workspace(&h).editor.mesh.offset_at(80.0, 75.0), [0.0; 2]);
        // Applied, the selection still has the last word.
        h.key_press(Key::Enter);
        settle(&mut h);
        let out = pixels(&h);
        for y in 0..150 {
            for x in 0..100 {
                assert_eq!(out.get_pixel(x, y), original.get_pixel(x, y), "({x}, {y})");
            }
        }
    }

    #[test]
    fn locked_layers_and_targeted_masks_like_the_other_filters() {
        let mut h = app();
        h.state_mut()
            .doc
            .as_mut()
            .unwrap()
            .active_layer_mut()
            .locked = true;
        h.state_mut().run(Action::Liquify);
        assert!(h.state().liquify.workspace.is_none());
        h.state_mut()
            .doc
            .as_mut()
            .unwrap()
            .active_layer_mut()
            .locked = false;
        // A gradient in the mask, targeted: it is what moves.
        let original = pixels(&h);
        let doc = h.state_mut().doc.as_mut().unwrap();
        assert!(doc.add_mask(crate::document::NewMask::RevealAll, None));
        let gray = image::GrayImage::from_fn(200, 150, |x, _| image::Luma([x as u8]));
        let layer = doc.active_layer_mut();
        let offset = layer.offset;
        layer.mask = Some(crate::document::Mask::new(
            gray.clone(),
            (0, 0),
            255,
            offset,
        ));
        assert!(doc.mask_targeted());
        h.state_mut().run(Action::Liquify);
        h.run_steps(2);
        drag(&mut h, pos2(100.0, 75.0), pos2(130.0, 75.0));
        h.key_press(Key::Enter);
        settle(&mut h);
        assert_eq!(pixels(&h), original);
        let doc = h.state().doc.as_ref().unwrap();
        let mask = doc.active_layer().mask.as_ref().expect("mask kept");
        assert_ne!(*mask.image(), gray);
    }
}
