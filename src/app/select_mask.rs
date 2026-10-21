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

/// Select and Mask open.
pub(super) struct SelectMask {
    /// The selection refined, and its coverage reduced for the preview
    /// (`scale`: its pixels per canvas pixel).
    source: Selection,
    reduced: Arc<GrayImage>,
    scale: f32,
    pub(super) refine: Refine,
    view: ViewMode,
    /// Opacity (0..100) of each view mode, Black & White's unused.
    opacity: [f32; 5],
    pub(super) output: Output,
    /// Preview for the settings it was refined with.
    shown: Option<(Refine, Arc<GrayImage>)>,
}

impl SelectMask {
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
fn reduced_coverage(sel: &Selection, canvas: PixelRect) -> (GrayImage, f32) {
    let full = sel.to_mask(canvas);
    let scale = (PREVIEW_SIDE as f32 / canvas.w.max(canvas.h).max(1) as f32).min(1.0);
    if scale >= 1.0 {
        return (full, 1.0);
    }
    let (w, h) = (
        ((canvas.w as f32 * scale).round() as u32).max(1),
        ((canvas.h as f32 * scale).round() as u32).max(1),
    );
    let reduced = image::imageops::resize(&full, w, h, image::imageops::FilterType::Triangle);
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
    pub(super) fn open_select_mask(&mut self) {
        let (Some(sel), Some(doc)) = (self.selection.clone(), &self.doc) else {
            return;
        };
        let (reduced, scale) = reduced_coverage(&sel, doc.full_rect());
        self.sel.select_mask = Some(SelectMask {
            source: sel,
            reduced: Arc::new(reduced),
            scale,
            refine: Refine::default(),
            view: ViewMode::OnionSkin,
            opacity: [50.0, 100.0, 100.0, 50.0, 100.0],
            output: Output::Selection,
            shown: None,
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
        let has_mask = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active_layer().mask.is_some());
        let neutral = sm.refine == Refine::default();
        let out = floating_dialog(
            ctx,
            DialogSpec {
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
                    if sm.output == Output::LayerMask && has_mask {
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
        let (Some(sm), Some(canvas)) = (
            self.sel.select_mask.take(),
            self.doc.as_ref().map(Document::full_rect),
        ) else {
            return;
        };
        if !apply {
            return;
        }
        let (source, r) = (sm.source, sm.refine);
        let pixels = canvas.w as u64 * canvas.h as u64;
        self.select_later(Then::Output(sm.output), pixels, move || {
            if r == Refine::default() {
                Some(source)
            } else {
                source.refined(&r, canvas)
            }
        });
    }

    /// Select and Mask's result, where Output To says. Except as the
    /// selection, the selection is then dropped, like Photoshop; one undo
    /// step.
    pub(super) fn output_refined(&mut self, refined: Option<Selection>, output: Output) {
        if output == Output::Selection {
            self.apply_selection(refined, SelectMode::Replace);
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let canvas = doc.full_rect();
        // Nothing selected: everything hidden.
        let sel = refined.unwrap_or_else(|| Selection {
            inverted: true,
            ..canvas.into()
        });
        let source = doc.active;
        match output {
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
                layer.mask = Some(mask_of(&sel, canvas, layer.offset));
                doc.layers[source].visible = false;
            }
            Output::Selection => {}
        }
        self.deselect();
        self.reset_pending();
    }
}
