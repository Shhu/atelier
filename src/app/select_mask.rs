//! Select › Select and Mask, like Photoshop's workspace reduced to a
//! window: view modes (Onion Skin, On Black, On White, Overlay, Black &
//! White) previewing the refined selection over the canvas, the global
//! refinements (Smooth, Feather, Contrast, Shift Edge) and the output (the
//! selection, a layer mask, a new layer, a new layer with a layer mask).

use image::GrayImage;

use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, gap_between_blocks, slider_row, tool_dialog_pos,
};
use super::select_menu::{OverlayMask, OverlaySource, OverlayStyle, Then};
use super::*;
use crate::document::Mask;
use crate::i18n::t;
use crate::mask_ops::{self, Refine};

/// Longest side (px) of the coverage the preview is refined on.
const PREVIEW_SIDE: u32 = 1600;

/// How the refined selection is shown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    OnionSkin,
    OnBlack,
    OnWhite,
    Overlay,
    BlackWhite,
}

impl ViewMode {
    const ALL: [ViewMode; 5] = [
        ViewMode::OnionSkin,
        ViewMode::OnBlack,
        ViewMode::OnWhite,
        ViewMode::Overlay,
        ViewMode::BlackWhite,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|&m| m == self).expect("listed")
    }

    fn label(self) -> String {
        match self {
            ViewMode::OnionSkin => t!("sel-mask-view-onion"),
            ViewMode::OnBlack => t!("sel-mask-view-black"),
            ViewMode::OnWhite => t!("sel-mask-view-white"),
            ViewMode::Overlay => t!("sel-mask-view-overlay"),
            ViewMode::BlackWhite => t!("sel-mask-view-bw"),
        }
    }

    fn style(self, opacity: f32) -> OverlayStyle {
        match self {
            ViewMode::OnionSkin => OverlayStyle::Checker(opacity),
            ViewMode::OnBlack => OverlayStyle::Tint([0, 0, 0], opacity),
            ViewMode::OnWhite => OverlayStyle::Tint([255, 255, 255], opacity),
            ViewMode::Overlay => OverlayStyle::Tint([255, 0, 0], opacity),
            ViewMode::BlackWhite => OverlayStyle::Gray,
        }
    }
}

/// Where Select and Mask puts its result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Output {
    Selection,
    LayerMask,
    NewLayer,
    NewLayerWithMask,
}

impl Output {
    const ALL: [Output; 4] = [
        Output::Selection,
        Output::LayerMask,
        Output::NewLayer,
        Output::NewLayerWithMask,
    ];

    fn label(self) -> String {
        match self {
            Output::Selection => t!("sel-mask-output-selection"),
            Output::LayerMask => t!("sel-mask-output-layer-mask"),
            Output::NewLayer => t!("sel-mask-output-new-layer"),
            Output::NewLayerWithMask => t!("sel-mask-output-new-layer-mask"),
        }
    }
}

/// Where Select and Mask's output goes: Output To, the layer it was
/// opened on (its id), and whether it refines that layer's own mask.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct OutputTo {
    output: Output,
    layer: u64,
    own_mask: bool,
}

/// Select and Mask open.
pub(super) struct SelectMask {
    /// The selection refined (`None`: nothing, e.g. a Hide All mask), and
    /// its coverage reduced for the preview (`scale`: its pixels per canvas
    /// pixel).
    source: Option<Selection>,
    reduced: Arc<GrayImage>,
    scale: f32,
    pub(super) refine: Refine,
    view: ViewMode,
    /// Opacity (0..100) of each view mode, Black & White's unused.
    opacity: [f32; 5],
    pub(super) output: Output,
    /// Preview for the settings it was refined with.
    shown: Option<(Refine, Arc<GrayImage>)>,
    /// What it was opened on: the document, its history position, the
    /// active layer (its id), and whether its mask is the source.
    doc: u64,
    step: u64,
    layer: u64,
    own_mask: bool,
}

impl SelectMask {
    /// `doc` is still as it was opened on: same history position, active
    /// layer and target.
    fn opened_on(&self, doc: &Document) -> bool {
        doc.uid() == self.doc
            && doc.history_step() == self.step
            && doc.active_layer().id == self.layer
            && (!self.own_mask || refines_layer_mask(doc))
    }

    /// The refined preview, computed again when the settings changed.
    fn preview(&mut self) -> Arc<GrayImage> {
        if self.shown.as_ref().is_none_or(|(r, _)| *r != self.refine) {
            let mut mask = (*self.reduced).clone();
            mask_ops::refine(&mut mask, &self.refine, 1.0 / self.scale);
            self.shown = Some((self.refine, Arc::new(mask)));
        }
        Arc::clone(&self.shown.as_ref().expect("just made").1)
    }
}

/// A selection's coverage over `canvas`, reduced to at most
/// [`PREVIEW_SIDE`], and its scale.
fn reduced_coverage(sel: Option<&Selection>, canvas: PixelRect) -> (GrayImage, f32) {
    let full = match sel {
        Some(sel) => sel.to_mask(canvas),
        None => GrayImage::new(canvas.w, canvas.h),
    };
    let scale = (PREVIEW_SIDE as f32 / canvas.w.max(canvas.h).max(1) as f32).min(1.0);
    if scale >= 1.0 {
        return (full, 1.0);
    }
    let (w, h) = (
        ((canvas.w as f32 * scale).round() as u32).max(1),
        ((canvas.h as f32 * scale).round() as u32).max(1),
    );
    // Averaged by blocks first, on all the cores (the full size is large).
    let block = ((1.0 / scale).floor() as u32).max(1);
    let blocks = crate::compose::levels::reduce_gray(&full, block);
    let reduced = image::imageops::resize(&blocks, w, h, image::imageops::FilterType::Triangle);
    (reduced, w as f32 / canvas.w as f32)
}

/// A layer mask from `sel`, for a layer at `layer_offset`: white where it is
/// selected.
fn mask_of(sel: &Selection, canvas: PixelRect, layer_offset: (i64, i64)) -> Mask {
    let outside = if sel.inverted { 255 } else { 0 };
    Mask::new(
        sel.to_mask(canvas),
        (canvas.x, canvas.y),
        outside,
        layer_offset,
    )
}

impl AtelierApp {
    /// Select and Mask has something to refine: the selection, or the
    /// targeted layer mask.
    pub(super) fn can_select_mask(&self) -> bool {
        self.selection.is_some() || self.doc.as_ref().is_some_and(refines_layer_mask)
    }

    pub(super) fn open_select_mask(&mut self) {
        self.wait_select_job();
        let Some(doc) = &self.doc else { return };
        // A targeted layer mask is refined as it is stored (its density,
        // feather and what lies outside the canvas stay), and gets the
        // result. An empty one (Hide All) is a start too.
        let own_mask = refines_layer_mask(doc);
        let source = match own_mask {
            true => {
                let layer = doc.active_layer();
                let canvas = doc.full_rect();
                let mask = layer.mask.as_ref().expect("checked");
                let raw = mask.raw_over(canvas, layer.offset);
                Selection::from_mask(&raw, (canvas.x, canvas.y))
            }
            false => match self.selection.clone() {
                Some(sel) => Some(sel),
                None => {
                    self.status = t!("sel4-mask-needs-selection");
                    return;
                }
            },
        };
        let (reduced, scale) = reduced_coverage(source.as_ref(), doc.full_rect());
        let output = match own_mask {
            true => Output::LayerMask,
            false => Output::Selection,
        };
        self.sel.select_mask = Some(SelectMask {
            source,
            reduced: Arc::new(reduced),
            scale,
            refine: Refine::default(),
            view: ViewMode::OnionSkin,
            opacity: [50.0, 100.0, 100.0, 50.0, 100.0],
            output,
            shown: None,
            doc: doc.uid(),
            step: doc.history_step(),
            layer: doc.active_layer().id,
            own_mask,
        });
    }

    /// The refined selection over the canvas, as the view mode shows it.
    pub(super) fn select_mask_overlay(&mut self) -> Option<OverlaySource> {
        let sm = self.sel.select_mask.as_mut()?;
        let mask = sm.preview();
        let opacity = sm.opacity[sm.view.index()] / 100.0;
        let r = sm.refine;
        Some(OverlaySource {
            key: hash_of((
                Arc::as_ptr(&mask) as usize,
                [r.smooth, r.feather, r.contrast, r.shift].map(f32::to_bits),
            )),
            mask: OverlayMask::Gray(mask),
            scale: sm.scale,
            style: sm.view.style(opacity),
        })
    }

    pub(super) fn select_mask_window(&mut self, ctx: &egui::Context) {
        let Some(mut sm) = self.sel.select_mask.take() else {
            return;
        };
        // Another layer, an undo, a change behind the window: what it
        // refines is no longer what it shows; it closes.
        if !self.doc.as_ref().is_some_and(|d| sm.opened_on(d)) {
            self.status = t!("sel4-mask-target-changed");
            return;
        }
        // Its own mask is refined, not replaced.
        let replaced = !sm.own_mask
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.active_layer().mask.is_some());
        let neutral = sm.refine == Refine::default();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                // The view can be zoomed and moved meanwhile.
                modal: false,
                reset: Some(!neutral),
                ..DialogSpec::new("select_mask", t!("sel-mask-title"), tool_dialog_pos(ctx))
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-mask-view"));
                    let width = ui.available_width();
                    ui_kit::combo(
                        ui,
                        "select_mask_view",
                        None,
                        &sm.view.label(),
                        width,
                        |ui| {
                            for m in ViewMode::ALL {
                                ui.selectable_value(&mut sm.view, m, m.label());
                            }
                        },
                    );
                });
                if sm.view != ViewMode::BlackWhite {
                    gap_between_blocks(ui);
                    let label = if sm.view == ViewMode::OnionSkin {
                        t!("sel-mask-transparency")
                    } else {
                        t!("sel-mask-opacity")
                    };
                    let opacity = &mut sm.opacity[sm.view.index()];
                    let unit = i18n::percent("");
                    slider_row(
                        ui,
                        &label,
                        None,
                        opacity,
                        0.0..=100.0,
                        &unit,
                        Track::Accent,
                        50.0,
                    );
                }
                gap_between_blocks(ui);
                ui_kit::section(ui, &t!("sel-mask-global"));
                let r = &mut sm.refine;
                let label = t!("sel-mask-smooth");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut r.smooth,
                    0.0..=100.0,
                    "",
                    Track::Accent,
                    0.0,
                );
                let label = t!("sel-mask-feather");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut r.feather,
                    0.0..=250.0,
                    " px",
                    Track::Accent,
                    0.0,
                );
                let label = t!("sel-mask-contrast");
                let unit = i18n::percent("");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut r.contrast,
                    0.0..=100.0,
                    &unit,
                    Track::Accent,
                    0.0,
                );
                let label = t!("sel-mask-shift-edge");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut r.shift,
                    -100.0..=100.0,
                    &unit,
                    Track::Accent,
                    0.0,
                );
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-mask-output"));
                    let width = ui.available_width();
                    ui_kit::combo(
                        ui,
                        "select_mask_output",
                        None,
                        &sm.output.label(),
                        width,
                        |ui| {
                            for o in Output::ALL {
                                // A layer that has a mask gets it replaced.
                                ui.selectable_value(&mut sm.output, o, o.label());
                            }
                        },
                    );
                    if sm.output == Output::LayerMask && replaced {
                        ui_kit::hint(ui, t!("sel-mask-replaces-mask"));
                    }
                });
            },
        );
        if out.reset {
            sm.refine = Refine::default();
        }
        self.sel.select_mask = Some(sm);
        if out.ok || out.cancel {
            self.close_select_mask(out.ok);
        }
    }

    /// Closes Select and Mask: OK (`apply`) refines the selection at full
    /// size and outputs it.
    pub(super) fn close_select_mask(&mut self, apply: bool) {
        // A locked layer's mask is neither changed nor added: the window
        // stays open for another output.
        if apply
            && self
                .sel
                .select_mask
                .as_ref()
                .is_some_and(|sm| sm.output == Output::LayerMask)
            && self.refuse_if_locked()
        {
            return;
        }
        let (Some(sm), Some(doc)) = (self.sel.select_mask.take(), self.doc.as_ref()) else {
            return;
        };
        if !apply {
            return;
        }
        let canvas = doc.full_rect();
        // What it was opened on is no longer there: its result would land
        // on something else.
        if !sm.opened_on(doc) {
            self.status = t!("sel4-mask-target-changed");
            return;
        }
        let (mut source, r) = (sm.source, sm.refine);
        // Its own mask, untouched: nothing to do.
        if sm.own_mask && sm.output == Output::LayerMask && r == Refine::default() {
            return;
        }
        // Into the mask itself (or its copy, for a new layer with mask),
        // its stored values are refined; otherwise what it shows, its
        // density and feather included, as when its thumbnail is
        // Ctrl+clicked.
        if sm.own_mask
            && matches!(sm.output, Output::Selection | Output::NewLayer)
            && doc
                .active_layer()
                .mask
                .as_ref()
                .is_some_and(|m| m.density < 1.0 || m.feather > 0.0)
        {
            source = doc.mask_selection(doc.active);
        }
        let to = OutputTo {
            output: sm.output,
            layer: sm.layer,
            own_mask: sm.own_mask,
        };
        let pixels = canvas.w as u64 * canvas.h as u64;
        let name = t!("sel-menu-select-and-mask");
        self.sel.step_name = Some(super::select_menu::step_label(name));
        self.select_later(Then::Output(to), pixels, move || {
            if r == Refine::default() {
                source
            } else {
                source?.refined(&r, canvas)
            }
        });
    }

    /// Select and Mask's result, where Output To says. Except as the
    /// selection, the selection is then dropped, like Photoshop; one undo
    /// step.
    pub(super) fn output_refined(&mut self, refined: Option<Selection>, to: OutputTo) {
        let output = to.output;
        if output == Output::Selection {
            self.apply_selection(refined, SelectMode::Replace);
            return;
        }
        if output == Output::LayerMask && self.refuse_if_locked() {
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        // Only on the layer it was opened on.
        let source = doc.active;
        if doc.active_layer().id != to.layer {
            self.status = t!("sel4-mask-target-changed");
            return;
        }
        let canvas = doc.full_rect();
        // Nothing selected: everything hidden.
        let sel = refined.unwrap_or_else(|| Selection {
            inverted: true,
            ..canvas.into()
        });
        match output {
            Output::LayerMask if to.own_mask && doc.active_layer().mask.is_some() => {
                // Refined in place: its settings, link and what lies outside
                // the canvas stay.
                doc.checkpoint(t!("xchg-hist-select-and-mask"));
                let layer = doc.active_layer_mut();
                let offset = layer.offset;
                let mask = layer.mask.as_mut().expect("checked");
                mask.put_over(&sel.to_mask(canvas), canvas, offset);
            }
            Output::LayerMask => {
                if doc.active_layer().mask.is_some() {
                    doc.checkpoint(t!("xchg-hist-select-and-mask"));
                    let layer = doc.active_layer_mut();
                    layer.mask = Some(mask_of(&sel, canvas, layer.offset));
                } else {
                    doc.add_mask(crate::document::NewMask::RevealSelection, Some(&sel));
                }
            }
            Output::NewLayer => {
                if !doc.layer_from_region(&sel) {
                    self.status = t!("status-selection-empty");
                    return;
                }
                doc.layers[source].visible = false;
            }
            Output::NewLayerWithMask => {
                doc.duplicate_active();
                let layer = doc.active_layer_mut();
                let offset = layer.offset;
                match layer.mask.as_mut() {
                    // The copy of its own mask is refined in place, its
                    // settings kept, as with Output To Layer Mask.
                    Some(mask) if to.own_mask => {
                        mask.put_over(&sel.to_mask(canvas), canvas, offset);
                    }
                    _ => layer.mask = Some(mask_of(&sel, canvas, offset)),
                }
                doc.layers[source].visible = false;
            }
            Output::Selection => {}
        }
        self.deselect();
        // Dropped as part of the output's step.
        self.sync_selection();
        self.reset_pending();
    }
}

/// Whether Select and Mask refines the active layer's mask: it is
/// targeted (not the Quick Mask, nor a smart filter mask).
fn refines_layer_mask(doc: &Document) -> bool {
    doc.quick_mask().is_none()
        && !doc.filter_mask_targeted()
        && doc.mask_targeted()
        && doc.active_layer().mask.is_some()
}
