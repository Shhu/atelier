//! Select › Color Range, like Photoshop: colors sampled on the image with
//! the eyedroppers (pick, add, subtract), Fuzziness, Invert, a thumbnail
//! of the selection (or of the image) and a preview over the canvas. Within
//! the current selection, if there is one.

use std::sync::mpsc;

use image::{GrayImage, RgbaImage};

use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, gap_between_blocks, help, slider_row,
    tool_dialog_pos,
};
use super::keys;
use super::select_menu::{OverlayMask, OverlaySource, OverlayStyle, Then};
use super::*;
use crate::i18n::t;
use crate::mask_ops::{self, RangeSample};

/// Longest side (px) of the image the previews are computed on.
const PREVIEW_SIDE: u32 = 1024;
/// Width of the thumbnail in the window (points).
const THUMB_WIDTH: f32 = 288.0;

/// What a click on the image does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Picker {
    /// Samples this color only.
    Sample,
    Add,
    Subtract,
}

/// Preview of the selection over the canvas.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Preview {
    None,
    Grayscale,
    BlackMatte,
    WhiteMatte,
    QuickMask,
}

impl Preview {
    const ALL: [Preview; 5] = [
        Preview::None,
        Preview::Grayscale,
        Preview::BlackMatte,
        Preview::WhiteMatte,
        Preview::QuickMask,
    ];

    fn label(self) -> String {
        match self {
            Preview::None => t!("sel-range-preview-none"),
            Preview::Grayscale => t!("sel-range-preview-grayscale"),
            Preview::BlackMatte => t!("sel-range-preview-black"),
            Preview::WhiteMatte => t!("sel-range-preview-white"),
            Preview::QuickMask => t!("sel-range-preview-quick-mask"),
        }
    }

    fn style(self) -> Option<OverlayStyle> {
        match self {
            Preview::None => None,
            Preview::Grayscale => Some(OverlayStyle::Gray),
            Preview::BlackMatte => Some(OverlayStyle::Tint([0, 0, 0], 1.0)),
            Preview::WhiteMatte => Some(OverlayStyle::Tint([255, 255, 255], 1.0)),
            Preview::QuickMask => Some(OverlayStyle::Tint([255, 0, 0], 0.5)),
        }
    }
}

/// The image as displayed, reduced for the previews: being made, or ready
/// with its scale (its pixels per canvas pixel).
enum Reduced {
    Pending(mpsc::Receiver<(RgbaImage, f32)>),
    Ready(Arc<RgbaImage>, f32),
}

/// Color Range window open.
pub(super) struct ColorRange {
    samples: Vec<RangeSample>,
    /// 0..200, Photoshop's.
    fuzziness: f32,
    pub(super) invert: bool,
    picker: Picker,
    preview: Preview,
    /// The thumbnail shows the image rather than the selection.
    thumb_image: bool,
    reduced: Reduced,
    /// Coverage of the reduced image for the settings (identified by the
    /// key), and the thumbnail drawn from it.
    shown: Option<(u64, Arc<GrayImage>)>,
    thumb: Option<(u64, egui::TextureHandle)>,
}

impl ColorRange {
    fn key(&self) -> u64 {
        let samples: Vec<_> = self
            .samples
            .iter()
            .map(|s| (s.lab.map(f32::to_bits), s.add))
            .collect();
        hash_of((samples, self.fuzziness.to_bits(), self.invert))
    }

    /// Coverage of the reduced image, computed again when the settings
    /// changed. `None` until the image is ready.
    fn coverage(&mut self) -> Option<(Arc<GrayImage>, f32)> {
        if let Reduced::Pending(rx) = &self.reduced
            && let Ok((image, scale)) = rx.try_recv()
        {
            self.reduced = Reduced::Ready(Arc::new(image), scale);
        }
        let Reduced::Ready(image, scale) = &self.reduced else {
            return None;
        };
        let key = self.key();
        if self.shown.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut mask = mask_ops::color_range(image, &self.samples, self.fuzziness);
            if self.invert {
                mask.iter_mut().for_each(|v| *v = 255 - *v);
            }
            self.shown = Some((key, Arc::new(mask)));
        }
        self.shown.as_ref().map(|(_, m)| (Arc::clone(m), *scale))
    }
}

/// `image` reduced to at most [`PREVIEW_SIDE`], and its scale.
fn reduce(image: &RgbaImage) -> (RgbaImage, f32) {
    let (w, h) = image.dimensions();
    let scale = (PREVIEW_SIDE as f32 / w.max(h).max(1) as f32).min(1.0);
    if scale >= 1.0 {
        return (image.clone(), 1.0);
    }
    let (rw, rh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    let reduced = image::imageops::resize(image, rw, rh, image::imageops::FilterType::Triangle);
    (reduced, rw as f32 / w as f32)
}

impl AtelierApp {
    pub(super) fn open_color_range(&mut self) {
        let Some(doc) = &self.doc else { return };
        let flatten = doc.flatten_later();
        let reduced = if doc.width() as u64 * doc.height() as u64 <= 1 << 20 {
            let (image, scale) = reduce(&flatten());
            Reduced::Ready(Arc::new(image), scale)
        } else {
            let (send, rx) = mpsc::channel();
            let ctx = self.ctx.clone();
            std::thread::spawn(move || {
                let _ = send.send(reduce(&flatten()));
                ctx.request_repaint();
            });
            Reduced::Pending(rx)
        };
        let [r, g, b, _] = self.primary.to_srgba_unmultiplied();
        self.sel.color_range = Some(ColorRange {
            samples: vec![RangeSample::new([r, g, b], true)],
            fuzziness: 40.0,
            invert: false,
            picker: Picker::Sample,
            preview: Preview::None,
            thumb_image: false,
            reduced,
            shown: None,
            thumb: None,
        });
    }

    /// A click on the image with Color Range open: the color there is
    /// sampled, added (Shift) or subtracted (Alt), as the eyedropper chosen.
    pub(super) fn color_range_pick(&mut self, at: Pos2) {
        let (Some(cr), Some(doc)) = (&mut self.sel.color_range, &self.doc) else {
            return;
        };
        let Some([r, g, b, _]) = doc.color_at(at.x.floor() as i64, at.y.floor() as i64) else {
            return;
        };
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let picker = match (shift, alt) {
            (true, _) => Picker::Add,
            (_, true) => Picker::Subtract,
            _ => cr.picker,
        };
        if picker == Picker::Sample {
            cr.samples.clear();
        }
        cr.samples
            .push(RangeSample::new([r, g, b], picker != Picker::Subtract));
    }

    /// Color Range's preview over the canvas, if one is chosen.
    pub(super) fn color_range_overlay(&mut self) -> Option<OverlaySource> {
        let cr = self.sel.color_range.as_mut()?;
        let style = cr.preview.style()?;
        let (mask, scale) = cr.coverage()?;
        Some(OverlaySource {
            key: cr.key(),
            mask: OverlayMask::Gray(mask),
            scale,
            style,
        })
    }

    pub(super) fn color_range_window(&mut self, ctx: &egui::Context) {
        let Some(mut cr) = self.sel.color_range.take() else {
            return;
        };
        let coverage = cr.coverage();
        // Thumbnail: the selection, or the image.
        let thumb_key = hash_of((cr.thumb_image, cr.shown.as_ref().map(|(k, _)| *k)));
        if coverage.is_some() && cr.thumb.as_ref().is_none_or(|(k, _)| *k != thumb_key) {
            let image = match (&cr.reduced, &coverage) {
                (Reduced::Ready(image, _), _) if cr.thumb_image => {
                    let size = [image.width() as usize, image.height() as usize];
                    ColorImage::from_rgba_unmultiplied(size, image.as_raw())
                }
                (_, Some((mask, _))) => {
                    let size = [mask.width() as usize, mask.height() as usize];
                    ColorImage::from_gray(size, mask.as_raw())
                }
                _ => ColorImage::default(),
            };
            let texture = ctx.load_texture("color-range", image, egui::TextureOptions::LINEAR);
            cr.thumb = Some((thumb_key, texture));
        }
        let mut fuzziness = cr.fuzziness;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                ok_enabled: coverage.is_some(),
                ..DialogSpec::new("color_range", t!("sel-range-title"), tool_dialog_pos(ctx))
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-range-sampled"));
                    ui.horizontal(|ui| {
                        let (pick, add, sub) = (
                            t!("sel-range-eyedropper"),
                            t!("sel-range-add"),
                            t!("sel-range-subtract"),
                        );
                        ui_kit::segmented(
                            ui,
                            &mut cr.picker,
                            &[
                                (Picker::Sample, icon::EYEDROPPER, &pick),
                                (Picker::Add, icon::PLUS, &add),
                                (Picker::Subtract, icon::MINUS, &sub),
                            ],
                        );
                    });
                });
                gap_between_blocks(ui);
                let label = t!("sel-range-fuzziness");
                slider_row(
                    ui,
                    &label,
                    None,
                    &mut fuzziness,
                    0.0..=200.0,
                    "",
                    Track::Accent,
                    40.0,
                );
                gap_between_blocks(ui);
                block(ui, |ui| {
                    let width = ui.available_width().min(THUMB_WIDTH);
                    match (&cr.thumb, &coverage) {
                        (Some((_, texture)), Some(_)) => {
                            let size = texture.size_vec2();
                            let size =
                                size * (width / size.x.max(1.0)).min(180.0 / size.y.max(1.0));
                            ui.vertical_centered(|ui| ui.image((texture.id(), size)));
                        }
                        _ => {
                            ui_kit::hint(ui, t!("sel-range-preparing"));
                        }
                    }
                    let (selection, image) =
                        (t!("sel-range-show-selection"), t!("sel-range-show-image"));
                    ui_kit::segmented(
                        ui,
                        &mut cr.thumb_image,
                        &[(false, &selection, ""), (true, &image, "")],
                    );
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("sel-range-preview"));
                    let width = ui.available_width();
                    ui_kit::combo(
                        ui,
                        "color_range_preview",
                        None,
                        &cr.preview.label(),
                        width,
                        |ui| {
                            for p in Preview::ALL {
                                ui.selectable_value(&mut cr.preview, p, p.label());
                            }
                        },
                    );
                });
                gap_between_blocks(ui);
                ui_kit::checkbox(ui, &mut cr.invert, &t!("sel-range-invert"));
                gap_between_blocks(ui);
                let (add, subtract) = (
                    keys::modifiers(ui.ctx(), Modifiers::SHIFT),
                    keys::modifiers(ui.ctx(), Modifiers::ALT),
                );
                help(ui, &t!("sel-range-hint", add = add, subtract = subtract));
            },
        );
        cr.fuzziness = fuzziness;
        self.sel.color_range = Some(cr);
        if out.ok || out.cancel {
            self.close_color_range(out.ok);
        }
    }

    /// Closes Color Range: OK (`apply`) makes the selection over the full
    /// image, within the current selection if any.
    pub(super) fn close_color_range(&mut self, apply: bool) {
        let Some(cr) = self.sel.color_range.take() else {
            return;
        };
        if apply && cr.shown.is_some() {
            self.apply_color_range(&cr);
        }
    }

    fn apply_color_range(&mut self, cr: &ColorRange) {
        let Some(doc) = &self.doc else { return };
        let (canvas, flatten) = (doc.full_rect(), doc.flatten_later());
        let (samples, fuzziness, invert) = (cr.samples.clone(), cr.fuzziness, cr.invert);
        let within = self.selection.clone();
        let pixels = canvas.w as u64 * canvas.h as u64;
        self.select_later(Then::Modified, pixels, move || {
            let mut mask = mask_ops::color_range(&flatten(), &samples, fuzziness);
            if invert {
                mask.iter_mut().for_each(|v| *v = 255 - *v);
            }
            let found = Selection::from_mask(&mask, (canvas.x, canvas.y))?;
            match within {
                Some(w) => Selection::combine(Some(&w), found, SelectMode::Intersect, canvas),
                None => Some(found),
            }
        });
    }
}
