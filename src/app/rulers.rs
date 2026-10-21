//! Rulers, guides and grid (View menu), like Photoshop's: rulers in pixels
//! along the canvas (Ctrl+R); guides dragged from them, moved with the Move
//! tool and deleted by dragging them out of the canvas; a grid; snapping
//! to guides, grid, layers and document bounds.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::dialogs::{DialogSpec, floating_dialog, gap_between_blocks};
use super::*;
use crate::guides::{self, Guide, Orientation};
use crate::i18n::t;
use ui_kit::color;

/// Thickness of the rulers.
const RULER: f32 = 18.0;
/// Distance (screen px) at which the Move tool grabs a guide.
const GUIDE_GRAB: f32 = 3.0;

pub(super) const SHORTCUT_RULERS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::R);
pub(super) const SHORTCUT_GUIDES: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Semicolon);
pub(super) const SHORTCUT_GRID: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Quote);
pub(super) const SHORTCUT_SNAP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Semicolon);
pub(super) const SHORTCUT_LOCK_GUIDES: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::Semicolon);

/// The View menu's shortcuts. Shift+; is typed ":" on many keyboards.
pub(super) const SHORTCUTS: [(KeyboardShortcut, Command); 6] = [
    (SHORTCUT_RULERS, Command::Rulers),
    (SHORTCUT_GUIDES, Command::Guides),
    (SHORTCUT_GRID, Command::Grid),
    (SHORTCUT_SNAP, Command::Snap),
    (
        KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Colon),
        Command::Snap,
    ),
    (SHORTCUT_LOCK_GUIDES, Command::LockGuides),
];

/// What the View menu shows and snaps to, and the grid's spacing; kept in
/// the settings folder (`view.json`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ViewSettings {
    pub rulers: bool,
    pub guides: bool,
    pub grid: bool,
    pub lock_guides: bool,
    pub snap: bool,
    pub snap_guides: bool,
    pub snap_grid: bool,
    pub snap_layers: bool,
    pub snap_bounds: bool,
    /// A main grid line every `grid_spacing` px, divided into `subdivisions`.
    pub grid_spacing: f32,
    pub subdivisions: u32,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            rulers: false,
            guides: true,
            grid: false,
            lock_guides: false,
            snap: true,
            snap_guides: true,
            snap_grid: true,
            snap_layers: true,
            snap_bounds: true,
            grid_spacing: 100.0,
            subdivisions: 4,
        }
    }
}

impl ViewSettings {
    /// Saved settings; tests start from the default ones.
    fn load() -> Self {
        if cfg!(test) {
            return Self::default();
        }
        std::fs::read(settings_file())
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if cfg!(test) {
            return;
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::create_dir_all(crate::config::dir());
            let _ = std::fs::write(settings_file(), json);
        }
    }

    /// Adds the visible guides and grid lines the settings snap to.
    pub(super) fn add_targets(&self, targets: &mut SnapTargets, doc: &Document) {
        if self.snap && self.guides && self.snap_guides {
            guides::add_targets(targets, &doc.guides);
        }
        if self.snap && self.grid && self.snap_grid {
            let size = (doc.width(), doc.height());
            guides::add_grid_targets(targets, size, self.grid_spacing, self.subdivisions);
        }
    }
}

fn settings_file() -> PathBuf {
    crate::config::dir().join("view.json")
}

/// View menu commands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Command {
    Rulers,
    Guides,
    Grid,
    Snap,
    SnapTo(SnapTo),
    LockGuides,
    ClearGuides,
    NewGuide,
    /// Preferences › Guides & Grid…
    Preferences,
}

/// View › Snap To.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum SnapTo {
    Guides,
    Grid,
    Layers,
    Bounds,
}

/// A guide being dragged: a new one from a ruler, or an existing one.
#[derive(Clone, Copy, Debug)]
struct GuideDrag {
    index: Option<usize>,
    orientation: Orientation,
    position: f32,
}

/// New Guide… dialog.
#[derive(Clone, Copy)]
struct NewGuide {
    orientation: Orientation,
    position: f32,
}

/// Preferences › Guides & Grid… dialog.
#[derive(Clone, Copy)]
struct GridPreferences {
    spacing: f32,
    subdivisions: u32,
}

pub(super) struct Rulers {
    pub settings: ViewSettings,
    drag: Option<GuideDrag>,
    /// Canvas area (without the rulers) and the whole area, last frame.
    area: Option<(Rect, Rect)>,
    new_guide: Option<NewGuide>,
    preferences: Option<GridPreferences>,
}

impl Rulers {
    pub(super) fn load() -> Self {
        Self {
            settings: ViewSettings::load(),
            drag: None,
            area: None,
            new_guide: None,
            preferences: None,
        }
    }

    /// The canvas area left by the rulers in `area` (all of it without
    /// rulers), remembered for this frame.
    pub(super) fn layout(&mut self, area: Rect) -> Rect {
        let canvas = if self.settings.rulers {
            Rect::from_min_max(area.min + Vec2::splat(RULER), area.max)
        } else {
            area
        };
        self.area = Some((canvas, area));
        canvas
    }

    /// Canvas area of this frame, if a document is shown.
    pub(super) fn canvas_area(&self) -> Option<Rect> {
        self.area.map(|(canvas, _)| canvas)
    }

    /// No document: no canvas area.
    pub(super) fn forget_area(&mut self) {
        self.area = None;
    }

    /// A dialog of its own is open.
    pub(super) fn modal_open(&self) -> bool {
        self.new_guide.is_some() || self.preferences.is_some()
    }
}

impl AtelierApp {
    pub(super) fn view_command(&mut self, command: Command) {
        let s = &mut self.rulers.settings;
        match command {
            Command::Rulers => s.rulers = !s.rulers,
            Command::Guides => s.guides = !s.guides,
            Command::Grid => s.grid = !s.grid,
            Command::Snap => s.snap = !s.snap,
            Command::SnapTo(SnapTo::Guides) => s.snap_guides = !s.snap_guides,
            Command::SnapTo(SnapTo::Grid) => s.snap_grid = !s.snap_grid,
            Command::SnapTo(SnapTo::Layers) => s.snap_layers = !s.snap_layers,
            Command::SnapTo(SnapTo::Bounds) => s.snap_bounds = !s.snap_bounds,
            Command::LockGuides => s.lock_guides = !s.lock_guides,
            Command::ClearGuides => {
                if let Some(doc) = self.doc.as_mut().filter(|d| !d.guides.is_empty()) {
                    doc.checkpoint(t!("xchg-hist-clear-guides"));
                    doc.guides.clear();
                }
            }
            Command::NewGuide => {
                if self.doc.is_some() {
                    self.rulers.new_guide = Some(NewGuide {
                        orientation: Orientation::Horizontal,
                        position: 0.0,
                    });
                }
            }
            Command::Preferences => {
                self.rulers.preferences = Some(GridPreferences {
                    spacing: s.grid_spacing,
                    subdivisions: s.subdivisions,
                });
            }
        }
        self.rulers.settings.save();
    }

    /// Entries of the View menu for rulers, guides, grid and snapping.
    pub(super) fn view_menu_entries(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Check, Item, Separator, Submenu};
        let s = &self.rulers.settings;
        let has_doc = self.doc.is_some();
        let has_guides = self.doc.as_ref().is_some_and(|d| !d.guides.is_empty());
        let check =
            |label, on, shortcut, command| Check(label, true, on, shortcut, Action::View(command));
        vec![
            Separator,
            Submenu(
                t!("uip-menu-show"),
                vec![
                    check(
                        t!("uip-menu-grid"),
                        s.grid,
                        Some(&SHORTCUT_GRID),
                        Command::Grid,
                    ),
                    check(
                        t!("uip-menu-guides"),
                        s.guides,
                        Some(&SHORTCUT_GUIDES),
                        Command::Guides,
                    ),
                ],
            ),
            check(
                t!("uip-menu-rulers"),
                s.rulers,
                Some(&SHORTCUT_RULERS),
                Command::Rulers,
            ),
            Separator,
            check(
                t!("uip-menu-snap"),
                s.snap,
                Some(&SHORTCUT_SNAP),
                Command::Snap,
            ),
            Submenu(
                t!("uip-menu-snap-to"),
                [
                    (t!("uip-menu-guides"), s.snap_guides, SnapTo::Guides),
                    (t!("uip-menu-grid"), s.snap_grid, SnapTo::Grid),
                    (t!("uip-menu-snap-layers"), s.snap_layers, SnapTo::Layers),
                    (t!("uip-menu-snap-bounds"), s.snap_bounds, SnapTo::Bounds),
                ]
                .into_iter()
                .map(|(label, on, to)| {
                    Check(label, s.snap, on, None, Action::View(Command::SnapTo(to)))
                })
                .collect(),
            ),
            Separator,
            check(
                t!("uip-menu-lock-guides"),
                s.lock_guides,
                Some(&SHORTCUT_LOCK_GUIDES),
                Command::LockGuides,
            ),
            Item(
                t!("uip-menu-clear-guides"),
                has_guides,
                None,
                Action::View(Command::ClearGuides),
            ),
            Item(
                t!("uip-menu-new-guide"),
                has_doc,
                None,
                Action::View(Command::NewGuide),
            ),
        ]
    }

    /// Guides: dragged from the rulers (any tool), or moved with the Move
    /// tool. Takes the press (`start`) when it grabs a guide; returns the
    /// cursor to show.
    pub(super) fn guides_input(
        &mut self,
        ui: &mut egui::Ui,
        view: View,
        pointer: Option<Pos2>,
        hovered: bool,
        start: &mut bool,
    ) -> Option<CursorIcon> {
        let (canvas, whole) = self.rulers.area?;
        let settings = self.rulers.settings;
        let doc = self.doc.as_ref()?;
        let cursor = |o: Orientation| match o {
            Orientation::Horizontal => CursorIcon::ResizeRow,
            Orientation::Vertical => CursorIcon::ResizeColumn,
        };

        // A press on a ruler pulls out a new guide.
        if settings.rulers {
            for (rect, orientation, salt) in [
                (top_ruler(canvas, whole), Orientation::Horizontal, "top"),
                (left_ruler(canvas, whole), Orientation::Vertical, "left"),
            ] {
                let response = ui.interact(rect, ui.id().with(("ruler", salt)), Sense::drag());
                if response.drag_started() && self.rulers.drag.is_none() {
                    self.rulers.drag = Some(GuideDrag {
                        index: None,
                        orientation,
                        position: 0.0,
                    });
                    // The guides are shown again to see the new one.
                    self.rulers.settings.guides = true;
                }
            }
        }

        let Some(mut drag) = self.rulers.drag else {
            // The Move tool grabs an existing guide.
            if self.tool != Tool::Transform || !settings.guides || settings.lock_guides || !hovered
            {
                return None;
            }
            let ip = view.to_image(pointer?);
            let i = guides::hit(&doc.guides, (ip.x, ip.y), GUIDE_GRAB / view.zoom)?;
            let g = doc.guides[i];
            if *start {
                *start = false;
                self.rulers.drag = Some(GuideDrag {
                    index: Some(i),
                    orientation: g.orientation,
                    position: g.position,
                });
            }
            return Some(cursor(g.orientation));
        };

        // From a ruler, Alt switches the orientation, like Photoshop.
        let (alt, shift, down) =
            ui.input(|i| (i.modifiers.alt, i.modifiers.shift, i.pointer.primary_down()));
        let orientation = match (drag.index, alt) {
            (None, true) => match drag.orientation {
                Orientation::Horizontal => Orientation::Vertical,
                Orientation::Vertical => Orientation::Horizontal,
            },
            _ => drag.orientation,
        };
        if let Some(p) = pointer {
            let ip = view.to_image(p);
            let along = match orientation {
                Orientation::Horizontal => ip.y,
                Orientation::Vertical => ip.x,
            };
            drag.position = if shift {
                // Shift: on the ruler's ticks.
                let (step, divisions) = guides::ruler_step(view.zoom);
                let tick = step / divisions as f32;
                (along / tick).round() * tick
            } else {
                self.snap_guide(doc, orientation, along, view.zoom)
                    .unwrap_or_else(|| along.round())
            };
        }
        let over_canvas = pointer.is_some_and(|p| canvas.contains(p));
        if down {
            self.rulers.drag = Some(drag);
            // Over the rulers too.
            ui.ctx().set_cursor_icon(cursor(orientation));
            return Some(cursor(orientation));
        }

        // Released: on the canvas the guide is placed, elsewhere (back on a
        // ruler, off the window) it goes.
        self.rulers.drag = None;
        let doc = self.doc.as_mut()?;
        let guide = Guide::new(orientation, drag.position);
        match drag.index {
            Some(i) if !over_canvas => {
                doc.checkpoint(t!("xchg-hist-delete-guide"));
                doc.guides.remove(i);
            }
            Some(i) if doc.guides[i] != guide => {
                doc.checkpoint(t!("xchg-hist-move-guide"));
                doc.guides[i] = guide;
            }
            None if over_canvas => {
                doc.checkpoint(t!("xchg-hist-new-guide"));
                doc.guides.push(guide);
            }
            _ => {}
        }
        Some(cursor(orientation))
    }

    /// Where a dragged guide snaps: document bounds, layer edges and grid,
    /// as the View menu says.
    fn snap_guide(&self, doc: &Document, o: Orientation, along: f32, zoom: f32) -> Option<f32> {
        if self.snap_off {
            return None;
        }
        let settings = ViewSettings {
            snap_guides: false,
            ..self.rulers.settings
        };
        let targets = snap_targets(doc, &[], &settings);
        let targets = match o {
            Orientation::Horizontal => &targets.ys,
            Orientation::Vertical => &targets.xs,
        };
        guides::nearest(along, targets, SNAP_DISTANCE / zoom)
    }

    /// Grid (inside the canvas) and guides (across the view), the one being
    /// dragged in its new place.
    pub(super) fn draw_guides(&self, painter: &Painter, view: View, full: Rect) {
        let Some((area, _)) = self.rulers.area else {
            return;
        };
        let settings = &self.rulers.settings;
        let Some(doc) = &self.doc else { return };
        // On the pixel grid of the screen, for sharp lines.
        let ppp = self.ctx.pixels_per_point();
        let sharp = |v: f32| ((v * ppp).floor() + 0.5) / ppp;
        if settings.grid {
            let canvas = view.rect_to_screen(full).intersect(area);
            let painter = painter.with_clip_rect(canvas);
            let spacing = settings.grid_spacing;
            let minor_gap = spacing / settings.subdivisions.max(1) as f32 * view.zoom;
            for (length, vertical) in [(full.width(), true), (full.height(), false)] {
                for (at, main) in guides::grid_lines(length, spacing, settings.subdivisions) {
                    if (!main && minor_gap < 6.0) || (main && spacing * view.zoom < 6.0) {
                        continue;
                    }
                    let stroke =
                        UiStroke::new(1.0, if main { color::GRID } else { color::GRID_MINOR });
                    if vertical {
                        let x = sharp(view.to_screen(pos2(at, 0.0)).x);
                        painter.vline(x, canvas.y_range(), stroke);
                    } else {
                        let y = sharp(view.to_screen(pos2(0.0, at)).y);
                        painter.hline(canvas.x_range(), y, stroke);
                    }
                }
            }
        }
        let drag = self.rulers.drag;
        let mut shown: Vec<Guide> = if settings.guides {
            doc.guides
                .iter()
                .enumerate()
                .filter(|(i, _)| drag.is_none_or(|d| d.index != Some(*i)))
                .map(|(_, g)| *g)
                .collect()
        } else {
            Vec::new()
        };
        // The dragged guide shows over the canvas only.
        let pointer = self.ctx.pointer_latest_pos();
        if let Some(d) = drag
            && pointer.is_some_and(|p| area.contains(p))
        {
            let alt = self.ctx.input(|i| i.modifiers.alt) && d.index.is_none();
            let orientation = match (d.orientation, alt) {
                (Orientation::Horizontal, true) => Orientation::Vertical,
                (Orientation::Vertical, true) => Orientation::Horizontal,
                (o, false) => o,
            };
            shown.push(Guide::new(orientation, d.position));
        }
        let stroke = UiStroke::new(1.0, color::CANVAS_GUIDE);
        for g in shown {
            match g.orientation {
                Orientation::Horizontal => {
                    let y = sharp(view.to_screen(pos2(0.0, g.position)).y);
                    painter.hline(area.x_range(), y, stroke);
                }
                Orientation::Vertical => {
                    let x = sharp(view.to_screen(pos2(g.position, 0.0)).x);
                    painter.vline(x, area.y_range(), stroke);
                }
            }
        }
    }

    /// Rulers along the top and left of the canvas, in pixels from the
    /// canvas origin, with the pointer's position marked.
    pub(super) fn draw_rulers(&self, painter: &Painter, view: View) {
        let Some((canvas, whole)) = self.rulers.area.filter(|_| self.rulers.settings.rulers) else {
            return;
        };
        let (top, left) = (top_ruler(canvas, whole), left_ruler(canvas, whole));
        let corner = Rect::from_min_size(whole.min, Vec2::splat(RULER));
        for r in [top, left, corner] {
            painter.rect_filled(r, 0.0, color::BG1);
        }
        let line = UiStroke::new(1.0, color::LINE);
        painter.hline(whole.x_range(), top.bottom() - 0.5, line);
        painter.vline(left.right() - 0.5, whole.y_range(), line);

        let (step, divisions) = guides::ruler_step(view.zoom);
        let tick = step / divisions as f32;
        let font = ui_kit::mono(10.0);
        let tick_stroke = UiStroke::new(1.0, color::SWATCH_LINE);
        // Ticks of one ruler: positions along it, from `from` to `to` (screen).
        let ticks = |from: f32, to: f32, origin: f32| {
            let first = ((from - origin) / view.zoom / tick).floor() as i64;
            let last = ((to - origin) / view.zoom / tick).ceil() as i64;
            (first..=last).map(move |i| {
                let value = i as f32 * tick;
                let rank = i.rem_euclid(divisions as i64);
                let length = if rank == 0 {
                    RULER
                } else if divisions % 2 == 0 && rank == divisions as i64 / 2 {
                    RULER * 0.45
                } else {
                    RULER * 0.25
                };
                (origin + value * view.zoom, value, rank == 0, length)
            })
        };
        let top_painter = painter.with_clip_rect(top);
        for (x, value, numbered, length) in ticks(top.left(), top.right(), view.origin.x) {
            let x = x.round() + 0.5;
            top_painter.vline(x, top.bottom() - length..=top.bottom(), tick_stroke);
            if numbered {
                top_painter.text(
                    pos2(x + 3.0, top.top() + 1.0),
                    egui::Align2::LEFT_TOP,
                    signed(value),
                    font.clone(),
                    color::LABEL,
                );
            }
        }
        let left_painter = painter.with_clip_rect(left);
        for (y, value, numbered, length) in ticks(left.top(), left.bottom(), view.origin.y) {
            let y = y.round() + 0.5;
            left_painter.hline(left.right() - length..=left.right(), y, tick_stroke);
            if numbered {
                // Written upwards, like Photoshop's.
                let galley = left_painter.layout_no_wrap(signed(value), font.clone(), color::LABEL);
                let at = pos2(left.left() + 1.0, y + 3.0 + galley.size().x);
                left_painter.add(
                    egui::epaint::TextShape::new(at, galley, color::LABEL)
                        .with_angle(-std::f32::consts::FRAC_PI_2),
                );
            }
        }
        // Pointer position.
        if let Some(p) = self
            .ctx
            .pointer_latest_pos()
            .filter(|p| canvas.contains(*p))
        {
            let mark = UiStroke::new(1.0, color::TEXT);
            top_painter.vline(p.x.round() + 0.5, top.y_range(), mark);
            left_painter.hline(left.x_range(), p.y.round() + 0.5, mark);
        }
    }

    /// View › New Guide…: orientation and position.
    pub(super) fn new_guide_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.rulers.new_guide else {
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 300.0,
                ..DialogSpec::new("new_guide", t!("uip-new-guide-title"), center)
            },
            |ui| {
                ui_kit::field_label(ui, &t!("uip-new-guide-orientation"));
                let (h, v) = (t!("uip-new-guide-horizontal"), t!("uip-new-guide-vertical"));
                ui_kit::segmented(
                    ui,
                    &mut d.orientation,
                    &[
                        (Orientation::Horizontal, &h, ""),
                        (Orientation::Vertical, &v, ""),
                    ],
                );
                gap_between_blocks(ui);
                let max = MAX_DIMENSION as f32;
                let label = t!("uip-new-guide-position");
                let width = ui.available_width();
                ui_kit::number_field(ui, &label, &mut d.position, -max..=max, " px", width);
            },
        );
        self.rulers.new_guide = Some(d);
        if out.ok
            && let Some(doc) = &mut self.doc
        {
            doc.checkpoint(t!("xchg-hist-new-guide"));
            doc.guides.push(Guide::new(d.orientation, d.position));
            self.rulers.settings.guides = true;
        }
        if out.ok || out.cancel {
            self.rulers.new_guide = None;
        }
    }

    /// Edit › Preferences › Guides & Grid…: grid spacing and subdivisions.
    pub(super) fn grid_preferences_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.rulers.preferences else {
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 300.0,
                ..DialogSpec::new("guides_grid", t!("uip-preferences-title"), center)
            },
            |ui| {
                ui_kit::section(ui, &t!("uip-preferences-grid"));
                let width = ui.available_width();
                let every = t!("uip-preferences-gridline-every");
                let max = MAX_DIMENSION as f32;
                ui_kit::number_field(ui, &every, &mut d.spacing, 1.0..=max, " px", width);
                let label = t!("uip-preferences-subdivisions");
                ui_kit::number_field(ui, &label, &mut d.subdivisions, 1..=100, "", width);
            },
        );
        self.rulers.preferences = Some(d);
        if out.ok {
            let s = &mut self.rulers.settings;
            (s.grid_spacing, s.subdivisions) = (d.spacing, d.subdivisions);
            s.save();
        }
        if out.ok || out.cancel {
            self.rulers.preferences = None;
        }
    }
}

fn top_ruler(canvas: Rect, whole: Rect) -> Rect {
    Rect::from_min_max(
        pos2(canvas.left(), whole.top()),
        pos2(whole.right(), canvas.top()),
    )
}

fn left_ruler(canvas: Rect, whole: Rect) -> Rect {
    Rect::from_min_max(
        pos2(whole.left(), canvas.top()),
        pos2(canvas.left(), whole.bottom()),
    )
}
