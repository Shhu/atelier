//! Patterns in the interface: Edit › Define Pattern…, the library's picker
//! (Fill…, Pattern Fill layers, Pattern Overlay), its Rename and Delete.

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};

use image::RgbaImage;

use super::dialogs::{DialogSpec, floating_dialog, gap_between_blocks};
use super::*;
use crate::compose::pattern::{Pattern, PatternFill, SCALE_RANGE};
use crate::patterns::{Added, Change, Library, Loaded, Saver};

/// Edit › Define Pattern…: its tile, computed on another thread, and name.
pub(super) struct DefineDialog {
    name: String,
    /// The name it proposed, while the library was still being read: a
    /// better one once it is, unless typed over.
    proposed: Option<String>,
    /// Played by an action, without showing the dialog.
    quiet: bool,
    job: Option<Receiver<Option<Arc<Pattern>>>>,
    tile: Option<Arc<Pattern>>,
}

#[cfg(test)]
impl DefineDialog {
    fn image_ready(&self) -> bool {
        self.tile.is_some()
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Rename of a pattern of the library.
pub(super) struct RenameDialog {
    id: u64,
    name: String,
}

#[cfg(test)]
impl RenameDialog {
    fn name(&self) -> &str {
        &self.name
    }
}

/// Delete Pattern asks first: nothing brings a pattern back.
pub(super) struct DeleteDialog {
    id: u64,
    name: String,
}

/// What a picker asks of the library, handled at the next frame.
#[derive(Clone, Copy)]
enum Request {
    Rename(u64),
    Delete(u64),
}

/// The library and its windows.
#[derive(Default)]
pub(super) struct PatternState {
    pub(super) library: Library,
    /// The user's patterns being read at startup.
    loading: Option<Receiver<Loaded>>,
    /// The library's changes on their way to the disk.
    saver: Saver,
    /// The last one chosen: the first offered next time.
    pub(super) last: u64,
    pub(super) define: Option<DefineDialog>,
    pub(super) rename: Option<RenameDialog>,
    pub(super) delete: Option<DeleteDialog>,
}

fn list_id() -> egui::Id {
    egui::Id::new("pattern_library")
}

fn request_id() -> egui::Id {
    egui::Id::new("pattern_request")
}

/// Whether the user's patterns are still being read.
fn loading_id() -> egui::Id {
    egui::Id::new("pattern_loading")
}

/// Thumbnails, by pattern and size, with when each was last drawn.
#[derive(Default)]
struct ThumbCache {
    map: std::collections::HashMap<(u64, u32), (egui::TextureHandle, std::time::Instant)>,
    /// When those not drawn for [`THUMB_UNUSED`] were last let go.
    pruned: Option<std::time::Instant>,
}

type Thumbs = Arc<Mutex<ThumbCache>>;

/// A pattern thumbnail not drawn for this long leaves (a pattern deleted,
/// replaced, or a size no longer shown).
const THUMB_UNUSED: std::time::Duration = std::time::Duration::from_secs(60);

/// Texture of a pattern's tile for drawing it `side` physical pixels wide
/// at most: the smallest of its reduced versions at least that size (drawn
/// smaller by less than half, smooth), a small tile as it is (enlarged
/// sharp).
pub(super) fn thumbnail(ctx: &egui::Context, p: &Pattern, side: f32) -> egui::TextureId {
    let cache: Thumbs = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Thumbs>(egui::Id::new("pattern_thumbs"))
            .clone()
    });
    let tile = p.image.width().max(p.image.height());
    let ratio = (tile as f32 / side.max(1.0)).max(1.0);
    let k = ratio.log2().floor() as usize;
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    if cache
        .pruned
        .is_none_or(|t| now.duration_since(t) >= THUMB_UNUSED)
    {
        cache.pruned = Some(now);
        cache
            .map
            .retain(|_, (_, used)| now.duration_since(*used) < THUMB_UNUSED);
    }
    let entry = cache.map.entry((p.id, k as u32)).or_insert_with(|| {
        let image = p.mip(k);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        );
        let tex = ctx.load_texture(
            format!("pattern-{:x}-{k}", p.id),
            color,
            egui::TextureOptions {
                magnification: egui::TextureFilter::Nearest,
                minification: egui::TextureFilter::Linear,
                ..egui::TextureOptions::NEAREST
            },
        );
        (tex, now)
    });
    entry.1 = now;
    entry.0.id()
}

/// Paints the tile fitted in `rect`, over a neutral background.
pub(super) fn paint_tile(painter: &egui::Painter, rect: Rect, p: &Pattern, corner: f32) {
    painter.rect_filled(rect, corner, ui_kit::color::BG3);
    let (w, h) = (p.image.width() as f32, p.image.height() as f32);
    let k = (rect.width() / w).min(rect.height() / h);
    let fitted = Rect::from_center_size(rect.center(), vec2(w * k, h * k));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let tint = Color32::WHITE;
    let side = fitted.width().max(fitted.height()) * painter.ctx().pixels_per_point();
    painter.image(thumbnail(painter.ctx(), p, side), fitted, uv, tint);
}

/// Tile and name of `current`, the library in a drop-down: `current` becomes the one
/// clicked (a tile from a file, not in the library, stays offered first).
pub(super) fn pattern_picker(ui: &mut egui::Ui, current: &mut Arc<Pattern>) -> bool {
    let list: Arc<Vec<Arc<Pattern>>> = ui.data(|d| d.get_temp(list_id())).unwrap_or_default();
    let builtin = crate::patterns::BUILTIN_COUNT;
    let mut chosen = None;
    let label = t!("pattern8-pattern");
    // A built-in pattern is named in the current language, whichever it was
    // given in.
    let position = list.iter().position(|p| p.id == current.id);
    let shown = match position {
        Some(i) if i < builtin => list[i].name.clone(),
        _ => current.name.clone(),
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
        paint_tile(ui.painter(), rect, current, 4.0);
        let width = (ui.available_width() - 8.0).max(40.0);
        // A long name is cut, not the dialog widened: whole on hover.
        let font = egui::FontId::proportional(ui_kit::TEXT_SIZE);
        let text_w = |s: &str| {
            ui.painter().layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x
        };
        let spacing = ui.spacing();
        let room = width
            - text_w(&label)
            - 8.0
            - 2.0 * spacing.button_padding.x
            - spacing.icon_width
            - spacing.icon_spacing
            - 4.0;
        let cut = elided(&shown, room, text_w);
        // Stays open on a right click, for the menu of a tile.
        let combo = ui_kit::combo_closing(
            ui,
            ("pattern_picker", ui.id()),
            Some(&label),
            &cut,
            width,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(width.max(220.0));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(4.0);
                    let known = list.iter().any(|p| p.id == current.id);
                    let extra = (!known).then(|| current.clone());
                    for (i, p) in extra.iter().chain(list.iter()).enumerate() {
                        let user = known && i >= builtin || !known && i > builtin;
                        let response = tile(ui, p, p.id == current.id, user);
                        if response.clicked() {
                            chosen = Some(p.clone());
                            ui.close();
                        }
                        if user {
                            tile_menu(ui, &response, p.id);
                        }
                    }
                });
            },
        );
        if cut != shown {
            ui_kit::name_control(ui, combo.response.id, &label, &shown);
            combo.response.on_hover_text(&shown);
        }
    });
    match chosen {
        Some(p) if p.id != current.id || p.name != current.name => {
            *current = p;
            true
        }
        _ => false,
    }
}

/// `text` ending with "…" so as to be at most `max` wide (`width` measures).
fn elided(text: &str, max: f32, width: impl Fn(&str) -> f32) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let cut = |n: usize| format!("{}…", chars[..n].iter().collect::<String>().trim_end());
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width(&cut(mid)) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    cut(lo)
}

/// The menu of a tile, on a right click. Not a context menu: egui opens one
/// popup at a time, and that would close the picker it is in.
fn tile_menu(ui: &mut egui::Ui, response: &egui::Response, id: u64) {
    let key = egui::Id::new("pattern_tile_menu");
    if response.secondary_clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        ui.data_mut(|d| d.insert_temp(key, (id, pos)));
    }
    let open = ui.data(|d| d.get_temp::<(u64, Pos2)>(key));
    let Some((_, pos)) = open.filter(|&(which, _)| which == id) else {
        return;
    };
    let mut still = true;
    egui::Popup::new(key, ui.ctx().clone(), pos, ui.layer_id())
        .open_bool(&mut still)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
        .show(|ui| {
            let ask = |ui: &egui::Ui, r: Request| {
                ui.data_mut(|d| d.insert_temp(request_id(), Some(r)));
            };
            if ui.button(t!("pattern8-rename")).clicked() {
                ask(ui, Request::Rename(id));
            }
            if ui.button(t!("pattern8-delete")).clicked() {
                ask(ui, Request::Delete(id));
            }
        });
    if !still {
        ui.data_mut(|d| d.remove_temp::<(u64, Pos2)>(key));
    }
}

/// A pattern in the grid.
fn tile(ui: &mut egui::Ui, p: &Pattern, selected: bool, user: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::click());
    paint_tile(ui.painter(), rect, p, 4.0);
    let line = if selected {
        UiStroke::new(2.0, ui_kit::color::ACCENT)
    } else if response.hovered() {
        UiStroke::new(1.0, ui_kit::color::LINE_HOVER)
    } else {
        UiStroke::new(1.0, ui_kit::color::LINE)
    };
    ui.painter()
        .rect_stroke(rect, 4.0, line, StrokeKind::Inside);
    ui_kit::name_control(ui, response.id, &p.name, "");
    if user {
        response.on_hover_text(t!("pattern9-tile-tip", name = p.name.as_str()))
    } else {
        response.on_hover_text(&p.name)
    }
}

/// The picker for a choice kept as an id (Fill…): the first one if unknown,
/// once the library is read (until then, the choice may still come).
pub(super) fn pattern_choice(ui: &mut egui::Ui, id: &mut u64) {
    let list: Arc<Vec<Arc<Pattern>>> = ui.data(|d| d.get_temp(list_id())).unwrap_or_default();
    let loading = ui.data(|d| d.get_temp::<bool>(loading_id())).unwrap_or(false);
    let found = list.iter().find(|p| p.id == *id);
    let Some(mut current) = found.or(list.first()).cloned() else {
        return;
    };
    if pattern_picker(ui, &mut current) || found.is_some() || !loading {
        *id = current.id;
    }
}

/// Pattern, angle, scale, Link with Layer, Snap to Origin of a Pattern Fill
/// layer or Pattern Overlay. `corner`: where the layer's visible pixels
/// start from its frame (see `Document::tile_corner`); a linked tile's
/// offset is shown, and snapped, from there.
pub(super) fn pattern_fill_fields(ui: &mut egui::Ui, f: &mut PatternFill, corner: (f32, f32)) {
    pattern_picker(ui, &mut f.pattern);
    let half = (ui.available_width() - 8.0) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui_kit::glyph_number_field(
            ui,
            icon::ANGLE,
            &t!("core-gradient-angle-tip"),
            &mut f.angle,
            -180.0..=180.0,
            "°",
            half,
        );
        let mut scale = (f.scale * 100.0).round();
        let unit = i18n::percent("");
        // Image Size may take it beyond what is typed: shown as it is.
        let range = (SCALE_RANGE.start() * 100.0).min(scale)..=(SCALE_RANGE.end() * 100.0).max(scale);
        let label = t!("core-gradient-scale");
        if ui_kit::number_field(ui, &label, &mut scale, range, &unit, half).changed() {
            f.scale = scale / 100.0;
        }
    });
    // Where the tile starts, from the layer's visible corner or the
    // document's origin.
    let base = if f.linked { corner } else { (0.0, 0.0) };
    let mut shown = (f.phase.0 - base.0, f.phase.1 - base.1);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let range =
            -(crate::document::MAX_DIMENSION as f32)..=crate::document::MAX_DIMENSION as f32;
        let tip = t!("pattern9-offset-tip");
        let x = ui_kit::number_field(
            ui,
            &t!("pattern9-offset-x"),
            &mut shown.0,
            range.clone(),
            " px",
            half,
        )
        .on_hover_text(&tip);
        let y = ui_kit::number_field(
            ui,
            &t!("pattern9-offset-y"),
            &mut shown.1,
            range,
            " px",
            half,
        )
        .on_hover_text(&tip);
        if x.changed() || y.changed() {
            f.phase = (shown.0 + base.0, shown.1 + base.1);
        }
    });
    ui_kit::checkbox(ui, &mut f.linked, &t!("pattern8-link"))
        .on_hover_text(t!("pattern8-link-tip"));
    let at_origin = shown == (0.0, 0.0);
    let snap = ui_kit::button(
        ui,
        &t!("pattern8-snap"),
        ui_kit::Kind::Normal,
        None,
        !at_origin,
    );
    if snap.on_hover_text(t!("pattern8-snap-tip")).clicked() {
        f.phase = base;
    }
}

/// The tile of `area` (the canvas: `None`) of a composite; nothing in it: `None`.
fn tile_of(composite: &RgbaImage, area: Option<PixelRect>) -> Option<RgbaImage> {
    let image = match area {
        Some(r) => {
            image::imageops::crop_imm(composite, r.x as u32, r.y as u32, r.w, r.h).to_image()
        }
        None => composite.clone(),
    };
    let empty = image.width() == 0 || image.height() == 0;
    (!empty && image.pixels().any(|p| p.0[3] > 0)).then_some(image)
}

impl AtelierApp {
    /// Reads the user's patterns on another thread.
    pub(super) fn load_patterns(&mut self) {
        let (send, recv) = channel();
        std::thread::spawn(move || {
            let _ = send.send(Library::read(&crate::patterns::folder()));
        });
        self.patterns.loading = Some(recv);
    }

    /// What pickers read and asked for; once per frame, before the panels.
    pub(super) fn patterns_frame(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.patterns.loading {
            match job.try_recv() {
                Ok(read) => {
                    if let Some(backup) = &read.set_aside {
                        self.status.set(t!(
                            "pattern9-status-index-damaged",
                            backup = backup.as_str()
                        ));
                        crate::diag::warn(tm!(
                            "words14-log-pattern-index-damaged",
                            backup = backup.as_str()
                        ));
                    } else if read.left_out > 0 {
                        self.status.set(t!("pattern9-status-left-out", count = read.left_out));
                        crate::diag::warn(tm!(
                            "words14-log-patterns-left-out",
                            count = read.left_out
                        ));
                    }
                    self.patterns.library.merge(read);
                    self.patterns.loading = None;
                    let name = self.default_pattern_name();
                    if let Some(d) = &mut self.patterns.define
                        && let Some(proposed) = d.proposed.take()
                        && d.name == proposed
                    {
                        d.name = name;
                    }
                }
                Err(TryRecvError::Disconnected) => self.patterns.loading = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(error) = self.patterns.saver.error() {
            self.status.set(t!("pattern9-status-not-saved", error = error));
        }
        self.patterns.library.follow_language();
        let asked = ctx.data_mut(|d| d.remove_temp::<Option<Request>>(request_id()));
        match asked.flatten() {
            Some(Request::Rename(id)) => {
                if let Some(p) = self.patterns.library.get(id) {
                    self.patterns.rename = Some(RenameDialog {
                        id,
                        name: p.name.clone(),
                    });
                }
            }
            Some(Request::Delete(id)) => {
                if let Some(p) = self.patterns.library.get(id) {
                    self.patterns.delete = Some(DeleteDialog {
                        id,
                        name: p.name.clone(),
                    });
                }
            }
            None => {}
        }
        let list = Arc::new(self.patterns.library.items.clone());
        let loading = self.patterns.loading.is_some();
        ctx.data_mut(|d| {
            d.insert_temp(list_id(), list);
            d.insert_temp(loading_id(), loading);
        });
    }

    /// "Pattern N", after the user's patterns and unlike their names.
    fn default_pattern_name(&self) -> String {
        let user = self.patterns.library.user();
        let names: Vec<&str> = user.iter().map(|p| p.name.as_str()).collect();
        crate::patterns::default_name(&names, user.len())
    }

    /// The pattern offered first: the last one chosen.
    pub(super) fn last_pattern(&self) -> Arc<Pattern> {
        let library = &self.patterns.library;
        match library.get(self.patterns.last).or(library.items.first()) {
            Some(p) => p.clone(),
            None => crate::patterns::default_pattern(),
        }
    }

    /// Why Define Pattern can't run now, if so: it takes the visible pixels
    /// of the canvas or of a rectangular selection without feather.
    pub(super) fn define_pattern_refusal(&self) -> Option<String> {
        if self.doc.is_none() {
            return Some(t!("ui3-why-no-image"));
        }
        if self.in_quick_mask() {
            return Some(t!("pattern8-why-quick-mask"));
        }
        match (&self.selection, &self.doc) {
            (Some(s), _) if s.mask.is_some() || s.inverted => Some(t!("pattern8-why-rectangle")),
            // Nothing selected on the canvas isn't the same as no selection.
            (Some(s), Some(doc)) if s.bounds(doc.full_rect()).is_none() => {
                Some(t!("pattern8-status-empty"))
            }
            _ => None,
        }
    }

    /// Edit › Define Pattern…
    pub(super) fn open_define_pattern(&mut self) {
        if let Some(why) = self.define_pattern_refusal() {
            self.status_cannot(why);
            return;
        }
        let Some(doc) = &self.doc else { return };
        // (Not on the canvas: refused above.)
        let area = self
            .selection
            .as_ref()
            .and_then(|s| s.bounds(doc.full_rect()));
        let flatten = doc.flatten_later();
        let (send, recv) = channel();
        std::thread::spawn(move || {
            let tile = tile_of(&flatten(), area).map(|image| Pattern::new("", image));
            let _ = send.send(tile);
        });
        let name = self.default_pattern_name();
        let loading = self.patterns.loading.is_some();
        self.patterns.define = Some(DefineDialog {
            proposed: loading.then(|| name.clone()),
            name,
            quiet: false,
            job: Some(recv),
            tile: None,
        });
    }

    /// The dialog just opened by an action's step: under the recorded name,
    /// `quiet` without showing it.
    pub(super) fn define_as(&mut self, name: &str, quiet: bool) {
        if let Some(d) = &mut self.patterns.define {
            d.name = name.to_owned();
            d.quiet = quiet;
        }
    }

    pub(super) fn pattern_windows(&mut self, ctx: &egui::Context) {
        self.define_pattern_window(ctx);
        self.rename_pattern_window(ctx);
        self.delete_pattern_window(ctx);
    }

    fn define_pattern_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.patterns.define.take() else {
            return;
        };
        if let Some(job) = &d.job {
            match job.try_recv() {
                Ok(Some(tile)) => {
                    d.tile = Some(tile);
                    d.job = None;
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {
                    let text = t!("pattern8-status-empty");
                    if d.quiet {
                        self.actions
                            .step_stopped(actions_panel::Stopped::Failed(text.clone()));
                    }
                    self.status.set(text);
                    return;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint(),
            }
        }
        let ready = d.tile.is_some() && !d.name.trim().is_empty();
        if d.quiet {
            match d.tile.take() {
                Some(tile) => self.define_tile(&tile, d.name.trim()),
                None => self.patterns.define = Some(d),
            }
            return;
        }
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok_enabled: ready,
                ..DialogSpec::new(
                    "define_pattern",
                    t!("pattern8-define-title"),
                    ctx.content_rect().center(),
                )
            },
            |ui| {
                if let Some(tile) = &d.tile {
                    let (rect, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
                    paint_tile(ui.painter(), rect, tile, 4.0);
                    let (w, h) = tile.image.dimensions();
                    ui_kit::key_value(ui, &t!("pattern8-size"), &format!("{w} × {h} px"));
                }
                gap_between_blocks(ui);
                let id = ui.id().with("pattern_name");
                ui_kit::text_field(ui, &t!("pattern8-name"), &mut d.name, id);
            },
        );
        if out.ok
            && let Some(tile) = d.tile.take()
        {
            let name = d.name.trim();
            self.define_tile(&tile, name);
            self.record(crate::actions::Recorded::DefinePattern {
                name: name.to_owned(),
            });
            return;
        }
        if !(out.cancel || out.ok) {
            self.patterns.define = Some(d);
        }
    }

    /// The tile goes in the library under `name`, unless it is already there.
    fn define_tile(&mut self, tile: &Pattern, name: &str) {
        let p = tile.renamed(name);
        self.patterns.last = p.id;
        match self.patterns.library.add(p.clone()) {
            Added::New => {
                self.status.set(t!("pattern8-status-defined", name = p.name.as_str()));
                let file = self.patterns.library.file_for(p.id);
                self.patterns.saver.send(Change::Add { file, pattern: p });
            }
            Added::Already(name) => {
                self.status.set(t!("pattern9-status-already", name = name));
            }
        }
    }

    fn rename_pattern_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.patterns.rename.take() else {
            return;
        };
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok_enabled: !d.name.trim().is_empty(),
                ..DialogSpec::new(
                    "rename_pattern",
                    t!("pattern8-rename-title"),
                    ctx.content_rect().center(),
                )
            },
            |ui| {
                let id = ui.id().with("pattern_rename");
                ui_kit::text_field(ui, &t!("pattern8-name"), &mut d.name, id);
            },
        );
        if out.ok {
            let name = d.name.trim();
            if self.patterns.library.rename(d.id, name) {
                let file = self.patterns.library.file_for(d.id);
                self.patterns.saver.send(Change::Rename {
                    file,
                    name: name.to_owned(),
                });
            }
        } else if !out.cancel {
            self.patterns.rename = Some(d);
        }
    }

    fn delete_pattern_window(&mut self, ctx: &egui::Context) {
        let Some(d) = self.patterns.delete.take() else {
            return;
        };
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok: t!("pattern9-delete-ok"),
                ..DialogSpec::new(
                    "delete_pattern",
                    t!("pattern9-delete-title"),
                    ctx.content_rect().center(),
                )
            },
            |ui| {
                ui.label(t!("pattern9-delete-question", name = d.name.as_str()));
            },
        );
        if out.ok {
            let file = self.patterns.library.file_for(d.id);
            if self.patterns.library.remove(d.id) {
                self.patterns.saver.send(Change::Remove { file });
            }
        } else if !out.cancel {
            self.patterns.delete = Some(d);
        }
    }

    /// Fills with library pattern `id` (see [`Document::fill_active_pattern`]).
    pub(super) fn fill_with_pattern(&mut self, id: u64, opacity: f32, preserve: bool) -> bool {
        let Some(pattern) = self.patterns.library.get(id).cloned() else {
            self.status_cannot(t!("pattern8-why-missing"));
            return false;
        };
        self.patterns.last = id;
        let Some(doc) = &mut self.doc else {
            return false;
        };
        let selection = self.selection.as_ref();
        let done = doc.fill_active_pattern(&pattern, selection, opacity, preserve);
        if done {
            doc.mark_fade();
            self.status.set(t!(
                "status-filled",
                name = doc.active_layer().name.as_str(),
                content = pattern.name.as_str()
            ));
            self.reset_pending();
        } else {
            doc.mark_fade_nothing();
        }
        done
    }
}

#[cfg(test)]
#[path = "patterns_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "patterns11_tests.rs"]
mod tests11;
