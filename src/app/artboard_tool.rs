//! Artboards in the app, like Photoshop. The Artboard tool (with the Move
//! tool, V / Shift+V) draws an artboard on the pasteboard, moves one (drag
//! it, or its name) with its layers, resizes it by its handles and adds one
//! next to it with its "+" buttons (Alt: a copy with its layers). The
//! names above the boards; the view and the selection following the canvas
//! when the artboards change it; layers moved by the Move tool going into
//! the artboard they are dropped on; zoom to an artboard; the Layer menu's
//! artboard commands and Artboards to Files.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, gap_between_blocks, help, slider_row,
};
use super::*;
use crate::document::{
    ARTBOARD_GAP, ArtboardBackground, ArtboardExport, ArtboardFiles, ExportFormat, Side,
    artboard_file_stem, unused_artboard_path,
};
use crate::export::ExportOptions;

/// Distance (screen px) from an edge of the active artboard to the center
/// of its "+" button, and the button's radius.
const PLUS_DISTANCE: f32 = 22.0;
const PLUS_RADIUS: f32 = 9.0;
/// Space (screen px) between an artboard and its name above it.
const LABEL_GAP: f32 = 4.0;

/// Sizes offered for new documents and artboards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Preset {
    Phone,
    PhoneLarge,
    Tablet,
    TabletLarge,
    WebSmall,
    WebMedium,
    WebLarge,
    SocialSquare,
    SocialPortrait,
    SocialStory,
    SocialLandscape,
}

impl Preset {
    pub(super) const ALL: [Preset; 11] = [
        Preset::Phone,
        Preset::PhoneLarge,
        Preset::Tablet,
        Preset::TabletLarge,
        Preset::WebSmall,
        Preset::WebMedium,
        Preset::WebLarge,
        Preset::SocialSquare,
        Preset::SocialPortrait,
        Preset::SocialStory,
        Preset::SocialLandscape,
    ];

    pub(super) fn size(self) -> (u32, u32) {
        match self {
            Preset::Phone => (1080, 1920),
            Preset::PhoneLarge => (1290, 2796),
            Preset::Tablet => (1536, 2048),
            Preset::TabletLarge => (2048, 2732),
            Preset::WebSmall => (1280, 800),
            Preset::WebMedium => (1440, 900),
            Preset::WebLarge => (1920, 1080),
            Preset::SocialSquare => (1080, 1080),
            Preset::SocialPortrait => (1080, 1350),
            Preset::SocialStory => (1080, 1920),
            Preset::SocialLandscape => (1200, 628),
        }
    }

    fn name(self) -> String {
        match self {
            Preset::Phone => t!("artboard-preset-phone"),
            Preset::PhoneLarge => t!("artboard-preset-phone-large"),
            Preset::Tablet => t!("artboard-preset-tablet"),
            Preset::TabletLarge => t!("artboard-preset-tablet-large"),
            Preset::WebSmall => t!("artboard-preset-web-small"),
            Preset::WebMedium => t!("artboard-preset-web-medium"),
            Preset::WebLarge => t!("artboard-preset-web-large"),
            Preset::SocialSquare => t!("artboard-preset-social-square"),
            Preset::SocialPortrait => t!("artboard-preset-social-portrait"),
            Preset::SocialStory => t!("artboard-preset-social-story"),
            Preset::SocialLandscape => t!("artboard-preset-social-landscape"),
        }
    }

    /// Name and size, for the lists.
    fn label(self) -> String {
        let (w, h) = self.size();
        format!("{} · {w} × {h}", self.name())
    }

    /// The preset of this size, either way round: `picked` (the last one
    /// picked, Phone and Story being the same size), else the first one
    /// of this orientation, else turned.
    fn of((w, h): (u32, u32), picked: Option<Preset>) -> Option<Preset> {
        let fits = |p: &Preset| p.size() == (w, h) || p.size() == (h, w);
        picked
            .filter(fits)
            .or_else(|| Preset::ALL.into_iter().find(|p| p.size() == (w, h)))
            .or_else(|| Preset::ALL.into_iter().find(fits))
    }
}

/// Drop-down list of the presets, showing the one of `size` (else
/// "Custom"); returns the size of the one picked.
pub(super) fn preset_combo(
    ui: &mut egui::Ui,
    id: &str,
    size: (u32, u32),
    width: f32,
) -> Option<(u32, u32)> {
    let memory = egui::Id::new((id, "picked"));
    let current = Preset::of(size, ui.data(|d| d.get_temp::<Preset>(memory)));
    let shown = current.map_or_else(|| t!("artboard-preset-custom"), Preset::name);
    let mut picked = None;
    ui_kit::combo(ui, id, None, &shown, width, |ui| {
        for p in Preset::ALL {
            if ui.selectable_label(current == Some(p), p.label()).clicked() {
                picked = Some(p);
            }
        }
    });
    let picked = picked?;
    ui.data_mut(|d| d.insert_temp(memory, picked));
    Some(picked.size())
}

/// Portrait / landscape switch of a size; returns whether it turned.
pub(super) fn orientation_field(ui: &mut egui::Ui, (w, h): &mut (u32, u32)) -> bool {
    let mut portrait = *h >= *w;
    let (p, l) = (t!("artboard-portrait"), t!("artboard-landscape"));
    let options = [
        (true, icon::DEVICE_MOBILE, p.as_str()),
        (false, icon::RECTANGLE, l.as_str()),
    ];
    if ui_kit::segmented(ui, &mut portrait, &options) && (*h >= *w) != portrait {
        std::mem::swap(w, h);
        return true;
    }
    false
}

/// Artboard commands of the Layer and File menus.
#[derive(Clone, Copy, Debug)]
pub(super) enum Command {
    /// Layer › New › Artboard…
    New,
    /// Layer › New › Artboard from Group…
    FromGroup,
    /// Layer › New › Artboard from Layers…
    FromLayers,
    /// Layer › Ungroup Artboards.
    Ungroup,
    /// File › Export › Artboards to Files…
    ToFiles,
    /// Zoom to fit artboard `i` (double-click on its thumbnail).
    Fit(usize),
}

/// A gesture of the Artboard tool.
enum BoardDrag {
    /// A new artboard, from a corner to the pointer (canvas coordinates).
    Draw { from: Pos2, to: Pos2 },
    /// An artboard and its layers: their offsets at the start.
    Move {
        start: Pos2,
        layers: Vec<(usize, (i64, i64))>,
        moved: bool,
    },
    /// An artboard's edge or corner.
    Resize {
        board: usize,
        drag: RectDrag,
        moved: bool,
    },
}

/// What a new artboard is made of.
#[derive(Clone)]
enum Source {
    Empty,
    Group(usize),
    Layers(Vec<usize>),
}

/// Layer › New › Artboard (from a group, from layers): name and size.
#[derive(Clone)]
struct BoardDialog {
    source: Source,
    name: String,
    /// Top-left corner (canvas).
    at: (i64, i64),
    size: (u32, u32),
}

/// Settings of Artboards to Files, kept for the session.
#[derive(Clone, PartialEq)]
struct FilesSettings {
    folder: Option<PathBuf>,
    prefix: String,
    format: ExportFormat,
    quality: u8,
    background: bool,
    overlapping: bool,
}

impl Default for FilesSettings {
    fn default() -> Self {
        Self {
            folder: None,
            prefix: String::new(),
            format: ExportFormat::Png,
            quality: 90,
            background: true,
            overlapping: false,
        }
    }
}

/// Formats Artboards to Files writes (one image per artboard).
const FILE_FORMATS: [ExportFormat; 3] = [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::WebP];

#[derive(Default)]
pub(super) struct Artboards {
    drag: Option<BoardDrag>,
    /// Names drawn above the artboards in the last frame: layer id, box
    /// (screen).
    labels: Vec<(u64, Rect)>,
    /// Canvas area the next Fit on Screen shows (an artboard).
    pub fit_to: Option<PixelRect>,
    dialog: Option<BoardDialog>,
    /// Artboards to Files open, and its settings.
    files: Option<FilesSettings>,
    files_settings: FilesSettings,
    /// Files being written.
    job: Option<Receiver<Result<Vec<PathBuf>, String>>>,
    /// Artboards left out by Export As (ids).
    pub export_skip: Vec<u64>,
}

impl Artboards {
    pub(super) fn modal(&self) -> bool {
        self.dialog.is_some() || self.files.is_some()
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

/// The side whose "+" button of artboard `r` (screen) is at `p`.
fn plus_at(r: Rect, p: Pos2) -> Option<Side> {
    plus_buttons(r)
        .into_iter()
        .find(|(_, c)| c.distance(p) <= PLUS_RADIUS + 1.0)
        .map(|(s, _)| s)
}

fn plus_buttons(r: Rect) -> [(Side, Pos2); 4] {
    let c = r.center();
    [
        (Side::Left, pos2(r.left() - PLUS_DISTANCE, c.y)),
        (Side::Right, pos2(r.right() + PLUS_DISTANCE, c.y)),
        (Side::Top, pos2(c.x, r.top() - PLUS_DISTANCE)),
        (Side::Bottom, pos2(c.x, r.bottom() + PLUS_DISTANCE)),
    ]
}

/// The artboard drawn from `from` to `to` (Shift: a square), in pixels.
fn drawn_rect(from: Pos2, to: Pos2, square: bool) -> PixelRect {
    let mut to = to;
    if square {
        let d = to - from;
        let side = d.x.abs().max(d.y.abs());
        to = from + vec2(side.copysign(d.x), side.copysign(d.y));
    }
    to_pixel_rect(Rect::from_two_pos(from, to))
}

impl AtelierApp {
    pub(super) fn artboard_command(&mut self, command: Command) {
        let indices = self.selected_indices();
        let Some(doc) = &self.doc else { return };
        let active = doc.active_layer();
        match command {
            Command::New => {
                // To the right of the others, of the active one's size.
                let size = doc
                    .artboard_of(doc.active)
                    .and_then(|a| doc.layers[a].artboard_rect())
                    .map_or(Preset::WebLarge.size(), |r| (r.w, r.h));
                let at = doc
                    .artboards_bounds()
                    .map_or((0, 0), |b| (b.right() + ARTBOARD_GAP, b.y));
                self.artboards.dialog = Some(BoardDialog {
                    source: Source::Empty,
                    name: doc.new_artboard_name(),
                    at,
                    size,
                });
            }
            Command::FromGroup
                if active.is_group() && active.artboard.is_none() && active.depth == 0 =>
            {
                let r = doc.element_bounds(doc.active).unwrap_or(doc.full_rect());
                self.artboards.dialog = Some(BoardDialog {
                    source: Source::Group(doc.active),
                    name: active.name.clone(),
                    at: (r.x, r.y),
                    size: (r.w, r.h),
                });
            }
            Command::FromGroup => self.status = t!("artboard-status-not-group"),
            Command::FromLayers => {
                let r = indices
                    .iter()
                    .filter_map(|&i| doc.element_bounds(i))
                    .reduce(PixelRect::union)
                    .unwrap_or(doc.full_rect());
                self.artboards.dialog = Some(BoardDialog {
                    source: Source::Layers(indices),
                    name: doc.new_artboard_name(),
                    at: (r.x, r.y),
                    size: (r.w, r.h),
                });
            }
            Command::Ungroup => {
                if let Some(doc) = &mut self.doc
                    && doc.ungroup_artboards(&indices)
                {
                    self.reset_pending();
                }
            }
            Command::ToFiles if doc.has_artboards() => {
                let mut settings = self.artboards.files_settings.clone();
                if settings.prefix.is_empty() {
                    settings.prefix = format!("{}_", self.file_stem());
                }
                settings.folder = settings.folder.or_else(|| self.current_dir());
                self.artboards.files = Some(settings);
            }
            Command::ToFiles => self.status = t!("artboard-status-none"),
            Command::Fit(i) => {
                if let Some(r) = doc.layers.get(i).and_then(|l| l.artboard_rect()) {
                    self.artboards.fit_to = Some(r);
                    self.fit_pending = Some(MAX_ZOOM);
                }
            }
        }
    }

    /// The canvas becomes the box of the artboards again; the view, the
    /// selection and the one kept for Reselect follow, the content staying
    /// put on screen.
    pub(super) fn refit_artboards(&mut self) {
        let Some(doc) = &mut self.doc else { return };
        let old = to_rect(doc.full_rect());
        let Some(r) = doc.fit_artboards() else { return };
        let canvas = doc.full_rect();
        self.pan += (to_rect(r).center() - old.center()) * self.screen_zoom();
        let shift = |s: Selection| s.translate(-r.x, -r.y).within(canvas);
        self.selection = self.selection.take().and_then(shift);
        self.last_selection = self.last_selection.take().and_then(shift);
    }

    /// After layers (`ids`) were moved: those dropped on another artboard
    /// go into it, those dropped on the pasteboard leave theirs; the canvas
    /// follows the artboards.
    pub(super) fn artboards_after_move(&mut self, ids: &[u64]) {
        let Some(doc) = &mut self.doc else { return };
        if !doc.has_artboards() {
            return;
        }
        doc.adopt(ids);
        self.refit_artboards();
    }

    /// With artboards, a move of the Move tool is applied on release (the
    /// layer goes into the artboard it is dropped on), like Photoshop.
    pub(super) fn commit_artboard_move(&mut self, released: bool) {
        if !released
            || self.drag.is_some()
            || !self.doc.as_ref().is_some_and(Document::has_artboards)
        {
            return;
        }
        let (Some(r), Some(src)) = (self.transform_rect, self.transform_source()) else {
            return;
        };
        if r.size() == src.size() && self.transform_angle == 0.0 && !self.transform_distorted() {
            self.apply_transform();
        }
    }

    /// The artboard whose name is at `p` (screen), as drawn last.
    fn label_at(&self, p: Pos2) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let (id, _) = self.artboards.labels.iter().find(|(_, r)| r.contains(p))?;
        doc.layers.iter().position(|l| l.id == *id)
    }

    /// Selects artboard `b` and starts moving it with its layers.
    fn start_board_move(&mut self, b: usize, ip: Pos2) {
        self.select_layer(b);
        let Some(doc) = &self.doc else { return };
        if doc.layers[b].locked {
            return;
        }
        let layers = doc.block(b).map(|i| (i, doc.layers[i].offset)).collect();
        self.artboards.drag = Some(BoardDrag::Move {
            start: ip,
            layers,
            moved: false,
        });
    }

    /// Move tool: an artboard's name above it is dragged to move it.
    pub(super) fn artboard_label_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) -> Option<CursorIcon> {
        if self.artboards.drag.is_some() {
            return Some(self.board_drag(view, pointer, down, false, false));
        }
        let p = pointer?;
        let b = self.label_at(p)?;
        if start {
            self.start_board_move(b, view.to_image(p));
        }
        Some(CursorIcon::Move)
    }

    /// The Artboard tool. Returns the cursor to show.
    pub(super) fn artboard_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
        alt: bool,
    ) -> CursorIcon {
        if self.artboards.drag.is_some() {
            return self.board_drag(view, pointer, down, shift, alt);
        }
        let (Some(p), Some(doc)) = (pointer, &self.doc) else {
            return CursorIcon::Default;
        };
        let ip = view.to_image(p);
        // The active artboard's "+" buttons and handles.
        let active = doc
            .artboard_of(doc.active)
            .filter(|&a| !doc.layers[a].locked && doc.layers[a].visible);
        if let Some(a) = active
            && let Some(r) = doc.layers[a].artboard_rect()
        {
            let screen = view.rect_to_screen(to_rect(r));
            if let Some(side) = plus_at(screen, p) {
                if start && let Some(doc) = &mut self.doc {
                    doc.add_adjacent_artboard(a, side, alt);
                    self.refit_artboards();
                }
                return CursorIcon::PointingHand;
            }
            if let Some(h) = hit_test(screen, p).filter(|&h| h != Handle::Move) {
                if start {
                    self.artboards.drag = Some(BoardDrag::Resize {
                        board: a,
                        drag: RectDrag::new(h, to_rect(r), ip),
                        moved: false,
                    });
                }
                return h.cursor(0.0);
            }
        }
        // An artboard, or its name: selected and moved.
        let under = self
            .label_at(p)
            .or_else(|| doc.artboard_at(ip.x.floor() as i64, ip.y.floor() as i64));
        if let Some(b) = under {
            if start {
                self.start_board_move(b, ip);
            }
            return CursorIcon::Move;
        }
        if start {
            self.artboards.drag = Some(BoardDrag::Draw { from: ip, to: ip });
        }
        CursorIcon::Crosshair
    }

    /// The gesture in progress, until the button is released.
    fn board_drag(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        down: bool,
        shift: bool,
        alt: bool,
    ) -> CursorIcon {
        let Some(mut drag) = self.artboards.drag.take() else {
            return CursorIcon::Default;
        };
        let ip = pointer.map(|p| view.to_image(p));
        let Some(doc) = &mut self.doc else {
            return CursorIcon::Default;
        };
        let (cursor, changed) = match &mut drag {
            BoardDrag::Draw { from, to } => {
                if let Some(ip) = ip {
                    *to = ip;
                }
                // A click alone draws nothing.
                let far = (*to - *from).length() * view.zoom > 3.0;
                if !down && far {
                    let r = drawn_rect(*from, *to, shift);
                    if r.w > 0 && r.h > 0 {
                        let name = doc.new_artboard_name();
                        let white = ArtboardBackground::White;
                        doc.add_artboard(t!("artboard-hist-new"), name, r, white);
                    }
                }
                (CursorIcon::Crosshair, !down && far)
            }
            BoardDrag::Move {
                start,
                layers,
                moved,
            } => {
                if let Some(ip) = ip {
                    let mut d = ip - *start;
                    if shift {
                        d = handles::constrain_axis(d);
                    }
                    let (dx, dy) = ((d.x + 0.5).floor() as i64, (d.y + 0.5).floor() as i64);
                    if (dx, dy) != (0, 0) && !*moved {
                        doc.checkpoint(t!("artboard-hist-move"));
                        *moved = true;
                    }
                    if *moved {
                        for &(i, (ox, oy)) in layers.iter() {
                            doc.layers[i].offset = (ox + dx, oy + dy);
                        }
                    }
                }
                (CursorIcon::Grabbing, !down && *moved)
            }
            BoardDrag::Resize { board, drag, moved } => {
                if let Some(ip) = ip {
                    let r = if alt {
                        drag.update_from_center(ip, shift)
                    } else {
                        drag.update(ip, shift)
                    };
                    let r = to_pixel_rect(r);
                    let current = doc.layers[*board].artboard_rect();
                    if r.w > 0 && r.h > 0 && current != Some(r) {
                        if !*moved {
                            doc.checkpoint(t!("artboard-hist-resize"));
                            *moved = true;
                        }
                        doc.set_artboard_rect(*board, r);
                    }
                }
                (drag.handle.cursor(0.0), !down && *moved)
            }
        };
        if down {
            self.artboards.drag = Some(drag);
        } else if changed {
            self.refit_artboards();
        }
        cursor
    }

    /// The artboards' boxes (canvas) and whether each is transparent, for
    /// the view to draw them on the pasteboard: none without artboards.
    pub(super) fn board_backdrops(&self) -> Vec<(Rect, bool)> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        doc.artboards()
            .into_iter()
            .map(|i| &doc.layers[i])
            .filter(|l| l.visible)
            .filter_map(|l| {
                let r = to_rect(l.artboard_rect()?);
                Some((r, l.artboard?.background.color().is_none()))
            })
            .collect()
    }

    /// The box (screen) out of which layer `i`, being moved, shows as a
    /// ghost: its artboard's.
    pub(super) fn ghost_hole(&self, i: usize, view: View) -> Option<Rect> {
        let doc = self.doc.as_ref()?;
        let a = doc.parent(i).and_then(|p| doc.artboard_of(p))?;
        Some(view.rect_to_screen(to_rect(doc.layers[a].artboard_rect()?)))
    }

    /// Names above the artboards; with the Artboard tool, the active one's
    /// frame, handles and "+" buttons, and the artboard being drawn.
    pub(super) fn draw_artboard_overlay(&mut self, painter: &Painter, view: View) {
        self.artboards.labels.clear();
        let Some(doc) = &self.doc else { return };
        let active = doc.artboard_of(doc.active);
        for i in doc.artboards() {
            let l = &doc.layers[i];
            let Some(r) = l.artboard_rect().filter(|_| l.visible) else {
                continue;
            };
            let screen = view.rect_to_screen(to_rect(r));
            let color = if active == Some(i) {
                ui_kit::color::TEXT
            } else {
                ui_kit::color::LABEL
            };
            let galley =
                painter.layout_no_wrap(l.name.clone(), egui::FontId::proportional(12.0), color);
            let at = screen.left_top() - vec2(0.0, galley.size().y + LABEL_GAP);
            self.artboards
                .labels
                .push((l.id, Rect::from_min_size(at, galley.size())));
            painter.galley(at, galley, color);
        }
        if self.tool != Tool::Artboard {
            return;
        }
        if let Some(BoardDrag::Draw { from, to }) = &self.artboards.drag {
            let r = drawn_rect(*from, *to, self.ctx.input(|i| i.modifiers.shift));
            let screen = view.rect_to_screen(to_rect(r));
            let stroke = UiStroke::new(1.0, ui_kit::color::ACCENT);
            ui_kit::dashed_rect(painter, screen, stroke, 4.0, 3.0);
            ui_kit::pill(
                painter,
                pos2(screen.center().x, screen.bottom() + 12.0),
                egui::Align2::CENTER_TOP,
                &format!("{} × {}", r.w, r.h),
            );
            return;
        }
        let Some(a) = active.filter(|&a| doc.layers[a].visible) else {
            return;
        };
        let Some(r) = doc.layers[a].artboard_rect() else {
            return;
        };
        let screen = view.rect_to_screen(to_rect(r));
        let stroke = UiStroke::new(1.5, ui_kit::color::ACCENT);
        painter.rect_stroke(screen, 0.0, stroke, egui::StrokeKind::Outside);
        if doc.layers[a].locked {
            return;
        }
        for (_, p) in handle_positions(screen) {
            let h = Rect::from_center_size(p, Vec2::splat(9.0));
            painter.rect(h, 2.0, Color32::WHITE, stroke, egui::StrokeKind::Inside);
        }
        if self.artboards.drag.is_some() {
            return;
        }
        for (_, c) in plus_buttons(screen) {
            painter.circle(c, PLUS_RADIUS, ui_kit::color::BG2, stroke);
            let s = UiStroke::new(1.5, ui_kit::color::TEXT);
            painter.line_segment([c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)], s);
            painter.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], s);
        }
    }

    /// Layer › New › Artboard (from a group, from layers), Artboards to
    /// Files, and the files being written.
    pub(super) fn artboard_windows(&mut self, ctx: &egui::Context) {
        self.board_dialog(ctx);
        self.files_dialog(ctx);
        let Some(job) = &self.artboards.job else {
            return;
        };
        match job.try_recv() {
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Ok(Ok(files)) => {
                let folder = files
                    .first()
                    .and_then(|f| f.parent())
                    .map(|f| f.display().to_string())
                    .unwrap_or_default();
                self.status = t!(
                    "artboard-status-exported",
                    count = files.len(),
                    folder = folder
                );
            }
            Ok(Err(error)) => {
                self.status_failed(t!("artboard-status-export-failed", error = error))
            }
            Err(TryRecvError::Disconnected) => {}
        }
        self.artboards.job = None;
    }

    fn board_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.artboards.dialog.clone() else {
            return;
        };
        let title = match d.source {
            Source::Empty => t!("artboard-dialog-new"),
            Source::Group(_) => t!("artboard-dialog-from-group"),
            Source::Layers(_) => t!("artboard-dialog-from-layers"),
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("artboard", title, center)
            },
            |ui| {
                block(ui, |ui| {
                    let id = egui::Id::new("artboard_name");
                    ui_kit::text_field(ui, &t!("artboard-name-label"), &mut d.name, id);
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("artboard-preset-label"));
                    let width = ui.available_width();
                    if let Some(size) = preset_combo(ui, "artboard_dialog_preset", d.size, width) {
                        d.size = size;
                    }
                    size_fields(ui, &mut d.size);
                });
            },
        );
        self.artboards.dialog = Some(d.clone());
        if out.ok || out.cancel {
            self.artboards.dialog = None;
        }
        if !out.ok {
            return;
        }
        let rect = PixelRect {
            x: d.at.0,
            y: d.at.1,
            w: d.size.0,
            h: d.size.1,
        };
        let Some(doc) = &mut self.doc else { return };
        let name = if d.name.trim().is_empty() {
            doc.new_artboard_name()
        } else {
            d.name
        };
        let done = match d.source {
            Source::Empty => {
                let white = ArtboardBackground::White;
                doc.add_artboard(t!("artboard-hist-new"), name, rect, white);
                true
            }
            Source::Group(g) => {
                doc.layers[g].name = name;
                doc.artboard_from_group(g, rect)
            }
            Source::Layers(indices) => doc.artboard_from_layers(&indices, name, rect),
        };
        if done {
            self.reset_pending();
            self.fit_pending = Some(MAX_ZOOM);
        } else {
            self.status = t!("artboard-status-refused");
        }
    }

    fn files_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.artboards.files.clone() else {
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok: t!("file-export-ok"),
                ok_enabled: s.folder.is_some(),
                width: 360.0,
                ..DialogSpec::new("artboards_to_files", t!("artboard-files-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("artboard-files-destination"));
                    let shown = s.folder.as_ref().map_or_else(
                        || t!("artboard-files-no-folder"),
                        |f| f.display().to_string(),
                    );
                    ui.label(egui::RichText::new(shown).color(ui_kit::color::MUTED));
                    let browse = t!("artboard-files-browse");
                    if ui_kit::button(ui, &browse, Kind::Normal, None, true).clicked() {
                        let mut dialog = rfd::FileDialog::new().set_title(browse);
                        if let Some(dir) = &s.folder {
                            dialog = dialog.set_directory(dir);
                        }
                        if let Some(folder) = dialog.pick_folder() {
                            s.folder = Some(folder);
                        }
                    }
                    let id = egui::Id::new("artboard_files_prefix");
                    ui_kit::text_field(ui, &t!("artboard-files-prefix"), &mut s.prefix, id);
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("file-export-format"));
                    let width = ui.available_width();
                    ui_kit::combo(
                        ui,
                        "artboard_files_format",
                        None,
                        s.format.name(),
                        width,
                        |ui| {
                            for f in FILE_FORMATS {
                                ui.selectable_value(&mut s.format, f, f.name());
                            }
                        },
                    );
                    if s.format == ExportFormat::Jpeg {
                        let mut quality = s.quality as f32;
                        let label = t!("file-export-quality");
                        let range = 1.0..=100.0;
                        slider_row(
                            ui,
                            &label,
                            None,
                            &mut quality,
                            range,
                            "",
                            Track::Accent,
                            90.0,
                        );
                        s.quality = quality.round().clamp(1.0, 100.0) as u8;
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::checkbox(ui, &mut s.background, &t!("artboard-files-background"));
                    let (only, over) = (
                        t!("artboard-files-content-only"),
                        t!("artboard-files-overlapping"),
                    );
                    let options = [(false, only.as_str(), ""), (true, over.as_str(), "")];
                    ui_kit::segmented(ui, &mut s.overlapping, &options);
                    help(ui, &t!("artboard-files-hint"));
                });
            },
        );
        self.artboards.files = Some(s.clone());
        self.artboards.files_settings = s.clone();
        if out.ok || out.cancel {
            self.artboards.files = None;
        }
        let (true, Some(folder)) = (out.ok, s.folder.clone()) else {
            return;
        };
        self.commit_pending();
        let Some(doc) = &self.doc else { return };
        let which = doc.artboards();
        let files = ArtboardFiles {
            folder,
            prefix: s.prefix,
            format: s.format,
            quality: s.quality,
            options: ArtboardExport {
                background: s.background,
                overlapping: s.overlapping,
            },
        };
        let job = doc.export_artboards_later(&which, files);
        self.run_artboards_job(which.len(), move || job().map_err(|e| e.to_string()));
    }

    /// Writes artboard files on another thread (see `artboard_windows`).
    fn run_artboards_job(
        &mut self,
        count: usize,
        job: impl FnOnce() -> Result<Vec<PathBuf>, String> + Send + 'static,
    ) {
        // One at a time: a second one would write the same files.
        if self.artboards.job.is_some() {
            self.status = t!("files3-status-artboards-busy");
            return;
        }
        let (send, receive) = channel();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Artboards to Files");
            let _ = send.send(job());
        });
        self.artboards.job = Some(receive);
        self.status = t!("artboard-status-exporting", count = count);
    }

    /// Export As: the artboards to export, each to its own file (all
    /// unchecked: the whole document, as without artboards).
    pub(super) fn export_as_artboards(
        ui: &mut egui::Ui,
        doc: Option<&Document>,
        skip: &mut Vec<u64>,
    ) {
        let Some(doc) = doc.filter(|d| d.has_artboards()) else {
            return;
        };
        gap_between_blocks(ui);
        block(ui, |ui| {
            ui_kit::field_label(ui, &t!("artboard-export-as-list"));
            for i in doc.artboards().into_iter().rev() {
                let l = &doc.layers[i];
                let mut on = !skip.contains(&l.id);
                if ui_kit::checkbox(ui, &mut on, &l.name).changed() {
                    skip.retain(|&id| id != l.id);
                    if !on {
                        skip.push(l.id);
                    }
                }
            }
        });
    }

    /// Export As with artboards checked: one file per artboard, named after
    /// it, in a folder asked for, with Export As's settings. Returns
    /// whether it took the export.
    pub(super) fn export_as_artboards_files(&mut self, options: &ExportOptions) -> bool {
        let Some(doc) = &self.doc else { return false };
        let skip = &self.artboards.export_skip;
        let which: Vec<usize> = doc
            .artboards()
            .into_iter()
            .filter(|&i| !skip.contains(&doc.layers[i].id))
            .collect();
        if which.is_empty() || options.format == ExportFormat::Psd {
            return false;
        }
        let mut dialog = rfd::FileDialog::new().set_title(t!("file-export-title"));
        if let Some(dir) = self.current_dir() {
            dialog = dialog.set_directory(dir);
        }
        let Some(folder) = dialog.pick_folder() else {
            return true;
        };
        self.commit_pending();
        let Some(doc) = &self.doc else { return true };
        let content = ArtboardExport {
            background: true,
            overlapping: false,
        };
        let render = doc.artboard_images_later(&which, content);
        let icc = doc.icc_profile.clone();
        let options = options.clone();
        // Converted as a single export is (sRGB, Use Proof Setup).
        let (proof, srgb) = (self.proof_converter(), self.srgb_source(&options));
        self.run_artboards_job(which.len(), move || {
            let mut used = Vec::new();
            let mut written = Vec::new();
            for (name, image) in render() {
                let stem = artboard_file_stem(&name, &mut used);
                let ext = options.format.extensions()[0];
                let path = unused_artboard_path(&folder, &stem, ext, &mut used);
                let icc = icc.as_deref().map(Vec::as_slice);
                let bytes = super::export_as::export_bytes(
                    image,
                    &options,
                    icc,
                    proof.as_ref(),
                    srgb.as_ref(),
                );
                bytes
                    .and_then(|b| crate::export::write(&path, &b))
                    .map_err(|e| e.to_string())?;
                written.push(path);
            }
            Ok(written)
        });
        true
    }
}

/// W and H of a size, with its orientation.
pub(super) fn size_fields(ui: &mut egui::Ui, size: &mut (u32, u32)) {
    ui.horizontal(|ui| {
        let half = (ui.available_width() - 8.0 - 70.0) / 2.0;
        let (w, h) = (t!("optbar-width-short"), t!("optbar-height-short"));
        let range = 1..=crate::document::MAX_DIMENSION;
        ui_kit::number_field(ui, &w, &mut size.0, range.clone(), " px", half);
        ui_kit::number_field(ui, &h, &mut size.1, range, " px", half);
        orientation_field(ui, size);
    });
}

#[cfg(test)]
mod tests {
    use super::Preset;

    #[test]
    fn presets_shown_for_a_size() {
        // The default 1920 × 1080: Web Large, not Phone turned.
        assert_eq!(Preset::of((1920, 1080), None), Some(Preset::WebLarge));
        assert_eq!(Preset::of((1080, 1920), None), Some(Preset::Phone));
        // Story picked: still Story, either way round.
        let story = Some(Preset::SocialStory);
        assert_eq!(Preset::of((1080, 1920), story), Some(Preset::SocialStory));
        assert_eq!(Preset::of((1920, 1080), story), Some(Preset::SocialStory));
        assert_eq!(Preset::of((800, 1280), None), Some(Preset::WebSmall));
        assert_eq!(Preset::of((1000, 1000), story), None);
    }
}
