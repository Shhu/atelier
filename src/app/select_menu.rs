//! Select menu beyond All, Deselect and Inverse, like Photoshop: Modify
//! (Border, Smooth, Expand, Contract, Feather), Grow, Similar, Transform
//! Selection, Edit in Quick Mask Mode, Save and Load Selection; the
//! selections computed in the background, and the overlay that shows a
//! mask over the canvas. Color Range and Select and Mask have their own
//! modules.

use std::sync::mpsc;

use image::{GrayImage, RgbaImage};

use super::canvas::{draw_frame, pointer_angle, rotate_around, rotate_handle_pos};
use super::color_range::ColorRange;
use super::dialogs::{DialogSpec, block, floating_dialog, gap_between_blocks};
use super::render::GpuImage;
use super::select_mask::{OutputTo, SelectMask};
use super::select_tools::{FreeLasso, ShapeOptions, combined};
use super::*;
use crate::i18n::t;
use crate::mask_ops::{self, Affine, Modify, par_rows};
use crate::selection::Channel;

pub(super) const SHORTCUT_FEATHER: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::SHIFT, Key::F6);
pub(super) const SHORTCUT_SELECT_AND_MASK: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::R);
/// Shown in the menu only: a plain key, see `handle_shortcuts`.
const SHORTCUT_QUICK_MASK: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Q);

/// Select menu commands with a shortcut (Edit in Quick Mask Mode, Q, is a
/// plain key: see `handle_shortcuts`).
pub(super) const SHORTCUTS: [(KeyboardShortcut, Command); 2] = [
    (SHORTCUT_FEATHER, Command::Modify(ModifyKind::Feather)),
    (SHORTCUT_SELECT_AND_MASK, Command::SelectAndMask),
];

/// Selections larger than this (pixels) are computed in the background.
const SYNC_PIXELS: u64 = 1 << 21;

/// A Select menu command.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Command {
    ColorRange,
    SelectAndMask,
    Modify(ModifyKind),
    Grow,
    Similar,
    Transform,
    QuickMask,
    Load,
    Save,
}

/// Select › Modify.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ModifyKind {
    Border,
    Smooth,
    Expand,
    Contract,
    Feather,
}

impl ModifyKind {
    pub(super) const ALL: [ModifyKind; 5] = [
        ModifyKind::Border,
        ModifyKind::Smooth,
        ModifyKind::Expand,
        ModifyKind::Contract,
        ModifyKind::Feather,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|&k| k == self).expect("listed")
    }

    pub(super) fn menu_label(self) -> String {
        match self {
            ModifyKind::Border => t!("sel-menu-border"),
            ModifyKind::Smooth => t!("sel-menu-smooth"),
            ModifyKind::Expand => t!("sel-menu-expand"),
            ModifyKind::Contract => t!("sel-menu-contract"),
            ModifyKind::Feather => t!("sel-menu-feather"),
        }
    }

    fn title(self) -> String {
        match self {
            ModifyKind::Border => t!("sel-border-title"),
            ModifyKind::Smooth => t!("sel-smooth-title"),
            ModifyKind::Expand => t!("sel-expand-title"),
            ModifyKind::Contract => t!("sel-contract-title"),
            ModifyKind::Feather => t!("sel-feather-title"),
        }
    }

    fn field(self) -> String {
        match self {
            ModifyKind::Border => t!("sel-border-width"),
            ModifyKind::Smooth => t!("sel-smooth-radius"),
            ModifyKind::Expand => t!("sel-expand-by"),
            ModifyKind::Contract => t!("sel-contract-by"),
            ModifyKind::Feather => t!("sel-feather-radius"),
        }
    }

    /// Largest amount (px), Photoshop's.
    fn max(self) -> u32 {
        match self {
            ModifyKind::Border => 200,
            ModifyKind::Smooth | ModifyKind::Expand | ModifyKind::Contract => 500,
            ModifyKind::Feather => 1000,
        }
    }

    fn op(self, amount: f32) -> Modify {
        match self {
            ModifyKind::Border => Modify::Border(amount),
            ModifyKind::Smooth => Modify::Smooth(amount),
            ModifyKind::Expand => Modify::Expand(amount),
            ModifyKind::Contract => Modify::Contract(amount),
            ModifyKind::Feather => Modify::Feather(amount),
        }
    }
}

/// Settings of a Select › Modify window.
#[derive(Clone, Copy)]
pub(super) struct ModifyDialog {
    kind: ModifyKind,
    pub(super) amount: f32,
    /// "Apply effect at canvas bounds".
    at_bounds: bool,
}

/// What Save Selection does with a channel.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChannelOp {
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl ChannelOp {
    const ALL: [ChannelOp; 4] = [
        ChannelOp::Replace,
        ChannelOp::Add,
        ChannelOp::Subtract,
        ChannelOp::Intersect,
    ];

    fn label(self) -> String {
        match self {
            ChannelOp::Replace => t!("sel-op-replace-channel"),
            ChannelOp::Add => t!("sel-op-add-channel"),
            ChannelOp::Subtract => t!("sel-op-subtract-channel"),
            ChannelOp::Intersect => t!("sel-op-intersect-channel"),
        }
    }
}

/// Settings of Save Selection: a new channel (`None`, named `name`) or an
/// existing one, and what to do with it.
pub(super) struct SaveDialog {
    channel: Option<usize>,
    name: String,
    op: ChannelOp,
}

/// Settings of Load Selection.
#[derive(Clone, Copy)]
pub(super) struct LoadDialog {
    channel: usize,
    invert: bool,
    mode: SelectMode,
}

/// What to do with a selection computed in the background.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Then {
    /// Combine it with the selection (a tool's new shape).
    Select(SelectMode),
    /// It replaces the selection it was made from (Select menu): say so if
    /// nothing, or nothing visible, is left.
    Modified,
    /// Select and Mask's output.
    Output(OutputTo),
}

/// A selection being computed in the background, for a document, from
/// the selection `base`, and its step's name.
struct SelectJob {
    result: mpsc::Receiver<Option<Selection>>,
    doc: u64,
    then: Then,
    base: Option<Selection>,
    /// The history generation it started in (see
    /// `Document::history_generation`): steps recorded meanwhile don't
    /// matter, an undo or a redo does.
    generation: u64,
    name: Option<String>,
}

/// Transform Selection in progress: the frame of the selection, moved,
/// scaled and turned like the Move tool's; applied by Enter.
pub(super) struct SelTransform {
    /// The selection transformed, and its box.
    source: Selection,
    from: Rect,
    /// The frame (canvas coordinates) and its rotation (degrees,
    /// clockwise) around its center.
    rect: Rect,
    angle: f32,
    drag: Option<RectDrag>,
    /// Rotation with the mouse: pointer angle and frame angle at its start.
    rotate: Option<(f32, f32)>,
    /// Outline of the selection (canvas coordinates), when short enough to
    /// be drawn moving.
    outline: Option<Vec<Edge>>,
}

impl SelTransform {
    /// From the selection's box to the frame.
    fn map(&self) -> Affine {
        let (c0, c1) = (self.from.center(), self.rect.center());
        let sx = self.rect.width() / self.from.width().max(1e-3);
        let sy = self.rect.height() / self.from.height().max(1e-3);
        let (s, c) = self.angle.to_radians().sin_cos();
        let (a, b, d, e) = (c * sx, -s * sy, s * sx, c * sy);
        Affine([
            a,
            b,
            c1.x - a * c0.x - b * c0.y,
            d,
            e,
            c1.y - d * c0.x - e * c0.y,
        ])
    }
}

/// Selection features beyond the tools' basic gestures: the options of the
/// marquees and lassos, the Lasso in progress, the Select menu windows,
/// Transform Selection, Quick Mask and the work in the background.
pub(super) struct SelState {
    pub(super) shape: ShapeOptions,
    pub(super) free_lasso: Option<FreeLasso>,
    job: Option<SelectJob>,
    /// Selections asked for while `job` runs, each started from the one
    /// before (a quick second Magic Wand click), applied in order.
    chained: std::collections::VecDeque<SelectJob>,
    pub(super) modify: Option<ModifyDialog>,
    /// Last amounts of the Modify windows (same order as `ModifyKind::ALL`)
    /// and "Apply effect at canvas bounds".
    amounts: [f32; 5],
    at_bounds: bool,
    pub(super) save: Option<SaveDialog>,
    pub(super) load: Option<LoadDialog>,
    pub(super) color_range: Option<ColorRange>,
    pub(super) select_mask: Option<SelectMask>,
    pub(super) transform: Option<SelTransform>,
    /// Name of the history step of the next selection change (what makes
    /// it, when the tool doesn't say), see `commit_selection`.
    pub(super) step_name: Option<String>,
    /// Mask shown over the canvas: what it was drawn for, image and place.
    overlay: Option<(u64, GpuImage, Rect)>,
}

impl Default for SelState {
    fn default() -> Self {
        Self {
            shape: ShapeOptions::default(),
            free_lasso: None,
            job: None,
            chained: std::collections::VecDeque::new(),
            modify: None,
            amounts: [10.0, 5.0, 5.0, 5.0, 5.0],
            at_bounds: false,
            save: None,
            load: None,
            color_range: None,
            select_mask: None,
            transform: None,
            step_name: None,
            overlay: None,
        }
    }
}

/// How a mask is shown over the image.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum OverlayStyle {
    /// A color over what isn't selected (Quick Mask, mattes), at an opacity.
    Tint([u8; 3], f32),
    /// The transparency checkerboard over what isn't selected (Onion Skin).
    Checker(f32),
    /// The selection in white over black (Grayscale, Black & White).
    Gray,
}

/// Coverage shown over the canvas.
pub(super) enum OverlayMask {
    Gray(Arc<GrayImage>),
}

/// A mask shown over the canvas: its coverage, `scale` of its pixels per
/// canvas pixel, how it is shown, and what identifies its content.
pub(super) struct OverlaySource {
    pub(super) mask: OverlayMask,
    pub(super) scale: f32,
    pub(super) style: OverlayStyle,
    pub(super) key: u64,
}

/// A menu item's label as the name of its history step (without "…").
pub(super) fn step_label(label: String) -> String {
    label.trim_end_matches(['…', '.']).trim_end().to_string()
}

/// Pixels the work on `sel` covers.
fn selection_pixels(sel: &Selection, canvas: PixelRect) -> u64 {
    let r = if sel.inverted { canvas } else { sel.rect };
    r.w as u64 * r.h as u64
}

impl AtelierApp {
    /// Runs a Select menu command; those that need a selection (or a saved
    /// one) do nothing without it.
    pub(super) fn select_command(&mut self, command: Command) {
        let Some(doc) = &self.doc else { return };
        let has_sel = self.selection.is_some();
        match command {
            Command::ColorRange => self.open_color_range(),
            Command::SelectAndMask => self.open_select_mask(),
            Command::Modify(kind) if has_sel => {
                self.sel.modify = Some(ModifyDialog {
                    kind,
                    amount: self.sel.amounts[kind.index()],
                    at_bounds: self.sel.at_bounds,
                });
            }
            Command::Grow | Command::Similar if has_sel => {
                self.grow_selection(command == Command::Grow);
            }
            Command::Transform if has_sel => self.start_selection_transform(),
            Command::QuickMask => self.toggle_quick_mask(),
            Command::Save if has_sel => {
                // The first number no channel has.
                let name = (1..)
                    .map(|n: usize| t!("sel-channel-default-name", n = n))
                    .find(|n| doc.channels.iter().all(|c| c.name != *n))
                    .expect("a free name");
                self.sel.save = Some(SaveDialog {
                    channel: None,
                    name,
                    op: ChannelOp::Replace,
                });
            }
            Command::Load if !doc.channels.is_empty() => {
                self.sel.load = Some(LoadDialog {
                    channel: doc.channels.len() - 1,
                    invert: false,
                    mode: SelectMode::Replace,
                });
            }
            _ => {}
        }
    }

    /// The Select menu after Subject, like Photoshop's.
    pub(super) fn select_menu_entries(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Check, Item, Separator, Submenu};
        let has_doc = self.doc.is_some();
        let has_sel = has_doc && self.selection.is_some();
        let channels = self.doc.as_ref().is_some_and(|d| !d.channels.is_empty());
        let modify = ModifyKind::ALL
            .into_iter()
            .map(|k| {
                let shortcut = (k == ModifyKind::Feather).then_some(&SHORTCUT_FEATHER);
                Item(
                    k.menu_label(),
                    has_sel,
                    shortcut,
                    Action::Select(Command::Modify(k)),
                )
            })
            .collect();
        let item = |label: String, enabled: bool, command: Command| {
            Item(label, enabled, None, Action::Select(command))
        };
        vec![
            Separator,
            Item(
                t!("sel-menu-select-and-mask"),
                self.can_select_mask(),
                Some(&SHORTCUT_SELECT_AND_MASK),
                Action::Select(Command::SelectAndMask),
            ),
            Submenu(t!("sel-menu-modify"), modify),
            Separator,
            item(t!("sel-menu-grow"), has_sel, Command::Grow),
            item(t!("sel-menu-similar"), has_sel, Command::Similar),
            Separator,
            item(t!("sel-menu-transform"), has_sel, Command::Transform),
            Separator,
            Check(
                t!("sel-menu-quick-mask"),
                has_doc,
                self.doc.as_ref().is_some_and(|d| d.quick_mask().is_some()),
                Some(&SHORTCUT_QUICK_MASK),
                Action::Select(Command::QuickMask),
            ),
            Separator,
            item(t!("sel-menu-load"), channels, Command::Load),
            item(t!("sel-menu-save"), has_sel, Command::Save),
        ]
    }

    /// The selection's outline is drawn: not while Transform Selection draws
    /// its own, nor while Select and Mask shows the refined one.
    pub(super) fn selection_outline_shown(&self) -> bool {
        self.sel.transform.is_none() && self.sel.select_mask.is_none()
    }

    /// Select windows that block the rest of the window.
    pub(super) fn select_modal(&self) -> bool {
        self.sel.modify.is_some() || self.sel.save.is_some() || self.sel.load.is_some()
    }

    pub(super) fn select_windows(&mut self, ctx: &egui::Context) {
        self.modify_window(ctx);
        self.save_selection_window(ctx);
        self.load_selection_window(ctx);
        self.color_range_window(ctx);
        self.select_mask_window(ctx);
    }

    /// The document leaves (other tab, closed): Quick Mask turns back into
    /// the selection, the Select windows close, gestures stop.
    pub(super) fn leave_select_modes(&mut self) {
        let s = &mut self.sel;
        s.free_lasso = None;
        s.modify = None;
        s.save = None;
        s.load = None;
        s.color_range = None;
        s.select_mask = None;
        s.transform = None;
        s.overlay = None;
    }

    /// Capture test: a circle selected in the middle, and `what` open.
    #[cfg(test)]
    pub(super) fn capture_select(&mut self, what: &str) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let side = canvas.w.min(canvas.h) / 2;
        let around = PixelRect {
            x: (canvas.w - side) as i64 / 2,
            y: (canvas.h - side) as i64 / 2,
            w: side,
            h: side,
        };
        self.selection = Selection::ellipse(around, canvas, true);
        let command = match what {
            "color-range" => Command::ColorRange,
            "select-mask" => Command::SelectAndMask,
            "feather" => Command::Modify(ModifyKind::Feather),
            "save-selection" => Command::Save,
            "transform-selection" => Command::Transform,
            _ => Command::QuickMask,
        };
        self.run(Action::Select(command));
        if let Some(sm) = &mut self.sel.select_mask {
            sm.refine.feather = 12.0;
        }
        if let Some(t) = &mut self.sel.transform {
            t.rect = t.rect.expand(20.0);
            t.angle = 15.0;
        }
    }

    // ------------------------------------------------------------ jobs

    /// Computes a selection with `work` — in the background when it covers
    /// many `pixels` — and does `then` with it.
    pub(super) fn select_later(
        &mut self,
        then: Then,
        pixels: u64,
        work: impl FnOnce() -> Option<Selection> + Send + 'static,
    ) {
        // One at a time, in order: the next one starts from its result,
        // on a worker when it only combines with it (the interface isn't
        // held up meanwhile).
        let work = match self.chain_select_job(then, work) {
            Some(work) => work,
            None => return,
        };
        self.wait_select_job();
        let Some(doc) = &self.doc else { return };
        if pixels <= SYNC_PIXELS {
            let result = work();
            self.finish_select(result, then);
            return;
        }
        // A new shape combines with the selection there too.
        let (work, then): (Box<dyn FnOnce() -> _ + Send>, _) = match then {
            Then::Select(mode) if mode != SelectMode::Replace => {
                let (current, canvas) = (self.selection.clone(), doc.full_rect());
                let work = move || combined(current.as_ref(), work(), mode, canvas);
                (Box::new(work), Then::Select(SelectMode::Replace))
            }
            _ => (Box::new(work), then),
        };
        let (send, result) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Selection");
            let _ = send.send(work());
            ctx.request_repaint();
        });
        self.sel.job = Some(SelectJob {
            result,
            doc: doc.uid(),
            then,
            base: self.selection.clone(),
            generation: doc.history_generation(),
            name: self.sel.step_name.take(),
        });
        self.status = t!("sel-status-working");
    }

    /// Applies the selection computed in the background, once ready (never
    /// to another document).
    pub(super) fn poll_select_job(&mut self) {
        let Some(job) = &self.sel.job else { return };
        match job.result.try_recv() {
            Ok(result) => self.select_job_done(result),
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.sel.job = None;
                self.sel.chained.clear();
            }
        }
    }

    /// Waits for the selection being computed and applies it: what comes
    /// next (a command, a click on the canvas) works with it, as if it
    /// had been computed at once.
    pub(super) fn wait_select_job(&mut self) {
        // The ones chained on it too.
        while let Some(job) = &self.sel.job {
            match job.result.recv() {
                Ok(result) => self.select_job_done(result),
                Err(_) => {
                    self.sel.job = None;
                    self.sel.chained.clear();
                }
            }
        }
    }

    /// With a selection being computed, a new one that only combines with
    /// it (Then::Select) is chained on a worker after it, rather than
    /// waited for on the interface thread: gives `work` back otherwise.
    fn chain_select_job<W>(&mut self, then: Then, work: W) -> Option<W>
    where
        W: FnOnce() -> Option<Selection> + Send + 'static,
    {
        let Then::Select(mode) = then else {
            return Some(work);
        };
        let Some(doc) = &self.doc else {
            return Some(work);
        };
        let (uid, canvas, generation) = (doc.uid(), doc.full_rect(), doc.history_generation());
        let last = match self.sel.chained.back_mut() {
            Some(last) => last,
            None => match &mut self.sel.job {
                Some(job) => job,
                None => return Some(work),
            },
        };
        // After a Select and Mask output, there is no selection to start
        // from: waited for.
        if last.doc != uid || matches!(last.then, Then::Output(_)) {
            return Some(work);
        }
        // Its result goes on to the interface through this thread, which
        // then combines the new shape with it.
        let (forward, earlier) = mpsc::channel();
        let before = std::mem::replace(&mut last.result, earlier);
        let (send, result) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let Ok(previous) = before.recv() else { return };
            let _ = forward.send(previous.clone());
            ctx.request_repaint();
            let _job = crate::diag::job("Selection");
            let _ = send.send(combined(previous.as_ref(), work(), mode, canvas));
            ctx.request_repaint();
        });
        self.sel.chained.push_back(SelectJob {
            result,
            doc: uid,
            then: Then::Select(SelectMode::Replace),
            // Set once the one before applies (see `select_job_done`).
            base: None,
            generation,
            name: self.sel.step_name.take(),
        });
        self.status = t!("sel-status-working");
        None
    }

    /// The selection computed in the background is ready: it applies to
    /// its document if the selection it started from is still there.
    fn select_job_done(&mut self, result: Option<Selection>) {
        let Some(job) = self.sel.job.take() else {
            return;
        };
        // The ones chained on it start from its result: dropped with it
        // when it is (as this returns early), next otherwise.
        let chained = std::mem::take(&mut self.sel.chained);
        self.status.clear();
        let Some(doc) = self.doc.as_ref().filter(|d| d.uid() == job.doc) else {
            return;
        };
        // Undone or redone meanwhile: it was computed from another state,
        // and recording it would wipe the redo list. Steps recorded forward
        // (a Layers panel change) don't matter.
        if doc.history_generation() != job.generation {
            self.status = t!("hist5-selection-undone");
            return;
        }
        let unchanged = match (&job.base, &self.selection) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same(b),
            _ => false,
        };
        if !unchanged {
            self.status = t!("hist3-selection-changed");
            return;
        }
        // The name already given to what comes next stays its own.
        let next = std::mem::replace(&mut self.sel.step_name, job.name);
        self.finish_select(result, job.then);
        self.sel.step_name = next;
        self.resume_chained(chained);
    }

    /// After a selection computed in the background applied, the next one
    /// chained on it waits in its place, from what it left.
    fn resume_chained(&mut self, mut chained: std::collections::VecDeque<SelectJob>) {
        let Some(mut next) = chained.pop_front() else {
            return;
        };
        next.base = self.selection.clone();
        self.sel.job = Some(next);
        self.sel.chained = chained;
        self.status = t!("sel-status-working");
    }

    /// A selection computed in the background is still to come.
    pub(super) fn select_busy(&self) -> bool {
        self.sel.job.is_some()
    }

    fn finish_select(&mut self, result: Option<Selection>, then: Then) {
        let faint = result.as_ref().is_some_and(|s| {
            !s.inverted
                && s.mask
                    .as_ref()
                    .is_some_and(|m| m.pixels().all(|p| p.0[0] < 128))
        });
        match then {
            Then::Select(mode) => self.apply_selection(result, mode),
            Then::Modified => {
                if result.is_none() {
                    self.status = t!("sel-status-nothing-selected");
                }
                self.apply_selection(result, SelectMode::Replace);
            }
            Then::Output(output) => {
                self.output_refined(result, output);
                self.sel.step_name = None;
                return;
            }
        }
        if faint {
            self.status = t!("sel-status-faint");
        }
    }

    // ------------------------------------------------------------ Modify

    fn modify_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.sel.modify else { return };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("sel_modify", d.kind.title(), center)
            },
            |ui| {
                let label = d.kind.field();
                let width = ui.available_width();
                if d.kind == ModifyKind::Feather {
                    let max = d.kind.max() as f32;
                    ui_kit::number_field(ui, &label, &mut d.amount, 0.1..=max, " px", width);
                } else {
                    let mut amount = d.amount.round().max(1.0) as u32;
                    let range = 1..=d.kind.max();
                    if ui_kit::number_field(ui, &label, &mut amount, range, " px", width).changed()
                    {
                        d.amount = amount as f32;
                    }
                }
                gap_between_blocks(ui);
                ui_kit::checkbox(ui, &mut d.at_bounds, &t!("sel-modify-at-bounds"))
                    .on_hover_text(t!("sel-modify-at-bounds-tip"));
            },
        );
        self.sel.modify = (!out.ok && !out.cancel).then_some(d);
        if out.ok {
            if d.kind != ModifyKind::Feather {
                d.amount = d.amount.round().max(1.0);
            }
            self.sel.amounts[d.kind.index()] = d.amount;
            self.sel.at_bounds = d.at_bounds;
            self.record(crate::actions::Recorded::ModifySelection {
                modify: actions_panel::modify(d.kind),
                amount: d.amount,
                at_bounds: d.at_bounds,
            });
            self.sel.step_name = Some(step_label(d.kind.menu_label()));
            self.modify_selection(d.kind.op(d.amount), d.at_bounds);
        }
    }

    /// Select › Modify on the selection.
    fn modify_selection(&mut self, op: Modify, at_bounds: bool) {
        self.wait_select_job();
        let (Some(sel), Some(doc)) = (self.selection.clone(), &self.doc) else {
            return;
        };
        let canvas = doc.full_rect();
        let pixels = selection_pixels(&sel, canvas);
        self.select_later(Then::Modified, pixels, move || {
            sel.modified(op, canvas, at_bounds)
        });
    }

    /// Select › Modify with these settings (an action's step): at once, or
    /// its window filled with them.
    pub(super) fn modify_selection_step(
        &mut self,
        kind: ModifyKind,
        amount: f32,
        at_bounds: bool,
        dialog: bool,
    ) {
        let least = if kind == ModifyKind::Feather {
            0.1
        } else {
            1.0
        };
        let amount = amount.clamp(least, kind.max() as f32);
        if dialog {
            self.sel.modify = Some(ModifyDialog {
                kind,
                amount,
                at_bounds,
            });
        } else {
            self.sel.step_name = Some(step_label(kind.menu_label()));
            self.modify_selection(kind.op(amount), at_bounds);
        }
    }

    // ------------------------------------------------------------ Grow, Similar

    /// Pixels the Magic Wand's options sample (active layer, or all layers
    /// as displayed), placed on the canvas: made later, on another thread.
    fn wand_pixels_later(&self) -> Option<Box<dyn FnOnce() -> RgbaImage + Send>> {
        let doc = self.doc.as_ref()?;
        if self.wand_all_layers {
            return Some(Box::new(doc.flatten_later()));
        }
        let canvas = doc.full_rect();
        let layer = doc.active_layer();
        let (pixels, offset) = (layer.shared_image(), layer.offset);
        Some(Box::new(move || {
            let mut out = RgbaImage::new(canvas.w, canvas.h);
            image::imageops::replace(&mut out, &*pixels, offset.0, offset.1);
            out
        }))
    }

    /// Grow (`contiguous`) and Similar, with the Magic Wand's tolerance.
    fn grow_selection(&mut self, contiguous: bool) {
        self.wait_select_job();
        let (Some(sel), Some(doc), Some(pixels)) =
            (self.selection.clone(), &self.doc, self.wand_pixels_later())
        else {
            return;
        };
        let canvas = doc.full_rect();
        let tolerance = (self.wand_tolerance * 255.0).round() as u8;
        let area = canvas.w as u64 * canvas.h as u64;
        self.sel.step_name = Some(step_label(if contiguous {
            t!("sel-menu-grow")
        } else {
            t!("sel-menu-similar")
        }));
        self.select_later(Then::Modified, area, move || {
            let image = pixels();
            let grown = mask_ops::grow(&image, &sel.to_mask(canvas), tolerance, contiguous);
            Selection::from_mask(&grown, (canvas.x, canvas.y))
        });
    }

    // ------------------------------------------------------------ Save, Load

    fn save_selection_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.sel.save.take() else {
            return;
        };
        let Some(names) = self.doc.as_ref().map(|doc| {
            doc.channels
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let center = ctx.content_rect().center();
        let new = t!("sel-channel-new");
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok_enabled: d.channel.is_some() || !d.name.trim().is_empty(),
                ..DialogSpec::new("sel_save", t!("sel-save-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-channel"));
                    let width = ui.available_width();
                    let shown = d.channel.map_or(new.as_str(), |i| names[i].as_str());
                    ui_kit::combo(ui, "sel_save_channel", None, shown, width, |ui| {
                        ui.selectable_value(&mut d.channel, None, new.as_str());
                        for (i, name) in names.iter().enumerate() {
                            ui.selectable_value(&mut d.channel, Some(i), name.as_str());
                        }
                    });
                    if d.channel.is_none() {
                        let id = egui::Id::new("sel_save_name");
                        ui_kit::text_field(ui, &t!("sel-channel-name"), &mut d.name, id);
                    }
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-operation"));
                    if d.channel.is_none() {
                        ui.label(t!("sel-op-new-channel"));
                    } else {
                        for op in ChannelOp::ALL {
                            ui.radio_value(&mut d.op, op, op.label());
                        }
                    }
                });
            },
        );
        if out.ok {
            self.save_selection(&d);
        } else if !out.cancel {
            self.sel.save = Some(d);
        }
    }

    /// Save Selection: into a new channel of the document, or combined with
    /// an existing one. One undo step.
    fn save_selection(&mut self, d: &SaveDialog) {
        let (Some(sel), Some(doc)) = (&self.selection, &mut self.doc) else {
            return;
        };
        let canvas = doc.full_rect();
        let mask = sel.to_mask(canvas);
        doc.checkpoint(t!("xchg-hist-save-selection"));
        let name = match d.channel.and_then(|i| doc.channels.get_mut(i)) {
            None => {
                let name = d.name.trim().to_owned();
                doc.channels.push(Channel {
                    name: name.clone(),
                    mask: Arc::new(mask),
                });
                name
            }
            Some(c) => {
                let mut old = mask_ops::recanvas(&c.mask, 0, 0, canvas.w, canvas.h);
                let row = canvas.w as usize;
                let new = mask.as_raw();
                par_rows(old.as_mut(), row, |y0, band| {
                    for (i, v) in band.iter_mut().enumerate() {
                        let n = new[y0 * row + i];
                        *v = match d.op {
                            ChannelOp::Replace => n,
                            ChannelOp::Add => (*v).max(n),
                            ChannelOp::Subtract => v.saturating_sub(n),
                            ChannelOp::Intersect => (*v).min(n),
                        };
                    }
                });
                c.mask = Arc::new(old);
                c.name.clone()
            }
        };
        self.status = t!("sel-status-saved", name = name);
    }

    fn load_selection_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.sel.load else { return };
        let Some(names) = self
            .doc
            .as_ref()
            .map(|doc| {
                doc.channels
                    .iter()
                    .map(|c| c.name.clone())
                    .collect::<Vec<_>>()
            })
            .filter(|n| d.channel < n.len())
        else {
            self.sel.load = None;
            return;
        };
        let has_sel = self.selection.is_some();
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("sel_load", t!("sel-load-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-channel"));
                    let width = ui.available_width();
                    ui_kit::combo(
                        ui,
                        "sel_load_channel",
                        None,
                        &names[d.channel],
                        width,
                        |ui| {
                            for (i, name) in names.iter().enumerate() {
                                ui.selectable_value(&mut d.channel, i, name.as_str());
                            }
                        },
                    );
                    ui_kit::checkbox(ui, &mut d.invert, &t!("sel-load-invert"));
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-operation"));
                    let modes = [
                        (SelectMode::Replace, t!("sel-op-new-selection")),
                        (SelectMode::Add, t!("options-select-add-hint")),
                        (SelectMode::Subtract, t!("options-select-subtract-hint")),
                        (SelectMode::Intersect, t!("options-select-intersect-hint")),
                    ];
                    for (mode, label) in modes {
                        if mode == SelectMode::Replace || has_sel {
                            ui.radio_value(&mut d.mode, mode, label);
                        }
                    }
                });
            },
        );
        self.sel.load = (!out.ok && !out.cancel).then_some(d);
        if out.ok {
            self.load_selection(d);
        }
    }

    /// Load Selection: a saved channel (inverted or not) combined with the
    /// selection.
    fn load_selection(&mut self, d: LoadDialog) {
        let Some(doc) = &self.doc else { return };
        let Some(channel) = doc.channels.get(d.channel) else {
            return;
        };
        let (canvas, mask) = (doc.full_rect(), Arc::clone(&channel.mask));
        let pixels = mask.width() as u64 * mask.height() as u64;
        let mode = if self.selection.is_some() {
            d.mode
        } else {
            SelectMode::Replace
        };
        self.sel.step_name = Some(step_label(t!("sel-menu-load")));
        self.select_later(Then::Select(mode), pixels, move || {
            let loaded = Selection::from_mask(&mask, (0, 0)).and_then(|s| s.within(canvas));
            if !d.invert {
                return loaded;
            }
            match loaded {
                Some(s) => Selection {
                    inverted: !s.inverted,
                    ..s
                }
                .within(canvas),
                None => Some(canvas.into()),
            }
        });
    }

    // ------------------------------------------------------------ Transform Selection

    fn start_selection_transform(&mut self) {
        let (Some(sel), Some(doc)) = (self.selection.clone(), &self.doc) else {
            return;
        };
        let canvas = doc.full_rect();
        let Some(bounds) = sel.bounds(canvas) else {
            return;
        };
        let area = if sel.inverted { canvas } else { sel.rect };
        let outline = (area.w as u64 * area.h as u64 <= SYNC_PIXELS)
            .then(|| sel.outline(area))
            .filter(|e| e.len() <= render::MAX_OUTLINE_SHAPES);
        let from = to_rect(bounds);
        self.sel.transform = Some(SelTransform {
            source: sel,
            from,
            rect: from,
            angle: 0.0,
            drag: None,
            rotate: None,
            outline,
        });
    }

    /// Applies Transform Selection (Enter, double-click in the frame, or
    /// anything else that commits what is pending).
    pub(super) fn apply_selection_transform(&mut self) {
        self.wait_select_job();
        let (Some(t), Some(canvas)) = (
            self.sel.transform.take(),
            self.doc.as_ref().map(Document::full_rect),
        ) else {
            return;
        };
        if t.rect == t.from && t.angle == 0.0 {
            return;
        }
        let map = t.map();
        let r = t.rect;
        let pixels = (r.width() * r.height()).max(0.0) as u64 + selection_pixels(&t.source, canvas);
        self.sel.step_name = Some(step_label(t!("sel-menu-transform")));
        self.select_later(Then::Modified, pixels, move || {
            t.source.transformed(map, canvas)
        });
    }

    /// Transform Selection takes the pointer: handles scale, inside moves,
    /// outside (or the round handle) turns; Shift: free proportions at the
    /// corners, 15° steps, one axis; Alt: from the center.
    pub(super) fn selection_transform_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        double_click: bool,
    ) -> CursorIcon {
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        // The selection changed meanwhile (a menu command): given up.
        if self
            .sel
            .transform
            .as_ref()
            .is_some_and(|t| !self.selection.as_ref().is_some_and(|s| s.same(&t.source)))
        {
            self.sel.transform = None;
        }
        let Some(t) = &mut self.sel.transform else {
            return CursorIcon::Default;
        };
        let rect_screen = view.rect_to_screen(t.rect);
        let center = rect_screen.center();
        let local = |p: Pos2| rotate_around(p, center, -t.angle);
        let hover = pointer.and_then(|p| handles::hit_test(rect_screen, local(p)));
        let knob = pointer.is_some_and(|p| {
            local(p).distance(rotate_handle_pos(rect_screen)) <= handles::GRAB_RADIUS
        });
        let turning = knob || hover.is_none();
        if double_click && hover == Some(Handle::Move) {
            self.apply_selection_transform();
            return CursorIcon::Default;
        }
        if start && let Some(p) = pointer {
            if turning {
                t.rotate = Some((pointer_angle(p, center), t.angle));
            } else {
                let ip = rotate_around(view.to_image(p), t.rect.center(), -t.angle);
                t.drag = hover.map(|h| RectDrag::new(h, t.rect, ip));
            }
        }
        if !down {
            t.drag = None;
            t.rotate = None;
        } else if let (Some((from, angle)), Some(p)) = (t.rotate, pointer) {
            let mut a = angle + pointer_angle(p, center) - from;
            if shift {
                a = (a / ROTATE_SNAP).round() * ROTATE_SNAP;
            }
            t.angle = normalize_angle(a);
        } else if let (Some(drag), Some(p)) = (t.drag, pointer) {
            let mut ip = rotate_around(view.to_image(p), drag.start_center(), -t.angle);
            if drag.handle == Handle::Move && shift {
                ip = drag.constrain_axis(ip);
            }
            let corner = matches!(
                drag.handle,
                Handle::NW | Handle::NE | Handle::SW | Handle::SE
            );
            let keep_ratio = corner && !shift;
            t.rect = if alt {
                drag.update_from_center(ip, keep_ratio)
            } else {
                drag.update(ip, keep_ratio)
            };
        }
        if t.drag.is_some() || t.rotate.is_some() {
            CursorIcon::Grabbing
        } else if turning {
            CursorIcon::Alias
        } else {
            hover.map_or(CursorIcon::Default, |h| h.cursor(t.angle))
        }
    }

    /// Frame of Transform Selection, and the selection's outline in it.
    pub(super) fn draw_selection_transform(&self, painter: &Painter, view: View) {
        let Some(t) = &self.sel.transform else { return };
        if let Some(edges) = &t.outline {
            let map = t.map();
            let at = |(x, y): (i64, i64)| {
                let (x, y) = map.apply(x as f32, y as f32);
                view.to_screen(pos2(x, y))
            };
            let mut black = egui::epaint::Mesh::default();
            let mut white = egui::epaint::Mesh::default();
            for &[a, b] in edges {
                let (a, b) = (at(a), at(b));
                segment(&mut black, a, b, 1.5, Color32::BLACK);
                // Dashes of 4 points.
                let length = a.distance(b);
                let mut s = 0.0;
                while s < length {
                    let e = (s + 4.0).min(length);
                    segment(
                        &mut white,
                        a.lerp(b, s / length),
                        a.lerp(b, e / length),
                        1.0,
                        Color32::WHITE,
                    );
                    s += 8.0;
                }
            }
            painter.add(black);
            painter.add(white);
        }
        let size = (
            t.rect.width().round().max(1.0) as u32,
            t.rect.height().round().max(1.0) as u32,
        );
        draw_frame(painter, view.rect_to_screen(t.rect), t.angle, true, size);
    }

    // ------------------------------------------------------------ canvas

    /// Clicks on the canvas taken by a Select window: Color Range picks
    /// colors there; Select and Mask lets nothing through. Returns `true`
    /// if the click is taken.
    pub(super) fn select_window_click(&mut self, view: View, pointer: Option<Pos2>) -> bool {
        if self.sel.select_mask.is_some() {
            return true;
        }
        let (Some(_), Some(p)) = (&self.sel.color_range, pointer) else {
            return false;
        };
        let ip = view.to_image(p);
        self.color_range_pick(ip);
        true
    }

    /// The mask over the canvas, if a mode shows one: Quick Mask, Select
    /// and Mask's view, Color Range's preview. Drawn at screen resolution
    /// over the visible part, again only when the view or the mask change.
    pub(super) fn draw_select_overlay(&mut self, painter: &Painter, view: View) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let mut source = self.quick_mask_overlay();
        if source.is_none() {
            source = self.select_mask_overlay();
        }
        if source.is_none() {
            source = self.color_range_overlay();
        }
        if source.is_none() {
            source = self.channel_overlay();
        }
        let Some(source) = source else {
            self.sel.overlay = None;
            return;
        };
        let ppp = self.ctx.pixels_per_point();
        let shown = view
            .rect_to_screen(to_rect(canvas))
            .intersect(painter.clip_rect());
        if !shown.is_positive() {
            return;
        }
        let key = hash_of((
            [view.origin.x, view.origin.y, view.zoom, ppp].map(f32::to_bits),
            [shown.min.x, shown.min.y, shown.max.x, shown.max.y].map(f32::to_bits),
            source.key,
            format!("{:?}", source.style),
        ));
        if self.sel.overlay.as_ref().is_none_or(|(k, ..)| *k != key) {
            let (x0, y0) = ((shown.min.x * ppp).floor(), (shown.min.y * ppp).floor());
            let (x1, y1) = ((shown.max.x * ppp).ceil(), (shown.max.y * ppp).ceil());
            let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
            let image = overlay_image(&source, view, ppp, (x0, y0), (w, h));
            let rect = Rect::from_min_max(pos2(x0, y0) / ppp, pos2(x1, y1) / ppp);
            let gpu = match self.sel.overlay.take() {
                Some((_, mut gpu, _)) => {
                    gpu.set(&self.ctx, &image, 0);
                    gpu
                }
                None => GpuImage::new(&self.ctx, "selection overlay".into(), &image, 0),
            };
            self.sel.overlay = Some((key, gpu, rect));
        }
        if let Some((_, gpu, rect)) = &self.sel.overlay {
            gpu.paint_all(painter, *rect, Color32::WHITE);
        }
    }
}

/// A line from `a` to `b`, `width` thick, as two triangles.
fn segment(mesh: &mut egui::epaint::Mesh, a: Pos2, b: Pos2, width: f32, color: Color32) {
    let d = b - a;
    let n = if d.length() > 0.0 {
        vec2(-d.y, d.x).normalized() * (width / 2.0)
    } else {
        vec2(width / 2.0, 0.0)
    };
    let i = mesh.vertices.len() as u32;
    for p in [a + n, a - n, b - n, b + n] {
        mesh.colored_vertex(p, color);
    }
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
}

/// Image of the overlay over `size` physical pixels from `origin` (physical
/// pixels of the screen): each one shows the mask at its center, bilinear.
fn overlay_image(
    source: &OverlaySource,
    view: View,
    ppp: f32,
    origin: (f32, f32),
    (w, h): (u32, u32),
) -> RgbaImage {
    let mut image = RgbaImage::new(w, h);
    let (mw, mh, raw) = match &source.mask {
        OverlayMask::Gray(m) => (m.width() as i64, m.height() as i64, m.as_raw()),
    };
    if mw == 0 || mh == 0 {
        return image;
    }
    let at = |x: i64, y: i64| raw[(y.clamp(0, mh - 1) * mw + x.clamp(0, mw - 1)) as usize] as f32;
    let row = w as usize;
    let checker_a = ui_kit::color::CHECKER_A.to_array();
    let checker_b = ui_kit::color::CHECKER_B.to_array();
    par_rows(image.as_mut(), row * 4, |y0, band| {
        for (i, px) in band.chunks_exact_mut(4).enumerate() {
            let (sx, sy) = (
                origin.0 + (i % row) as f32 + 0.5,
                origin.1 + (y0 + i / row) as f32 + 0.5,
            );
            let screen = pos2(sx / ppp, sy / ppp);
            let c = view.to_image(screen);
            let (mx, my) = (c.x * source.scale - 0.5, c.y * source.scale - 0.5);
            let (fx, fy) = (mx - mx.floor(), my - my.floor());
            let (ix, iy) = (mx.floor() as i64, my.floor() as i64);
            let top = at(ix, iy) * (1.0 - fx) + at(ix + 1, iy) * fx;
            let bottom = at(ix, iy + 1) * (1.0 - fx) + at(ix + 1, iy + 1) * fx;
            let cov = (top * (1.0 - fy) + bottom * fy) / 255.0;
            let out = match source.style {
                OverlayStyle::Tint([r, g, b], opacity) => {
                    [r, g, b, ((1.0 - cov) * opacity * 255.0).round() as u8]
                }
                OverlayStyle::Checker(opacity) => {
                    let p = screen - view.origin;
                    let tile = ((p.x / 8.0).floor() + (p.y / 8.0).floor()) as i64;
                    let [r, g, b, _] = if tile % 2 == 0 { checker_a } else { checker_b };
                    [r, g, b, ((1.0 - cov) * opacity * 255.0).round() as u8]
                }
                OverlayStyle::Gray => {
                    let v = (cov * 255.0).round() as u8;
                    [v, v, v, 255]
                }
            };
            px.copy_from_slice(&out);
        }
    });
    image
}
