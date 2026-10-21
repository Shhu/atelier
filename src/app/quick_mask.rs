//! Edit in Quick Mask Mode (Q), like Photoshop: the selection becomes a
//! mask, shown in red over what isn't selected; the Brush paints it (black
//! masks, white selects, grays in between) and the Eraser selects again.
//! Q again turns it back into the selection. Its strokes are undone with
//! Ctrl+Z while the mode lasts.

use image::{GrayImage, RgbaImage};

use super::keys;
use super::select_menu::{OverlayMask, OverlaySource, OverlayStyle};
use super::*;
use crate::i18n::t;
use crate::mask_ops::par_rows;
use crate::paint::StrokeMode;

/// Most strokes Undo can take back in Quick Mask Mode.
const MAX_UNDO: usize = 30;

/// Quick Mask Mode on.
pub(super) struct QuickMask {
    /// The mask, the size of the canvas: gray (white selected), and what
    /// is erased counts as white.
    image: Arc<RgbaImage>,
    /// Changes with every stroke (preview key).
    rev: u64,
    stroke: Option<Stroke>,
    undo: Vec<Arc<RgbaImage>>,
    redo: Vec<Arc<RgbaImage>>,
}

impl AtelierApp {
    pub(super) fn toggle_quick_mask(&mut self) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let row = canvas.w as usize;
        if let Some(qm) = self.sel.quick_mask.take() {
            let mut mask = GrayImage::new(canvas.w, canvas.h);
            let raw = qm.image.as_raw();
            par_rows(mask.as_mut(), row, |y0, band| {
                for (i, v) in band.iter_mut().enumerate() {
                    let p = &raw[(y0 * row + i) * 4..][..4];
                    let (g, a) = (p[0] as u32, p[3] as u32);
                    *v = ((g * a + 255 * (255 - a) + 127) / 255) as u8;
                }
            });
            self.selection =
                Selection::from_mask(&mask, (canvas.x, canvas.y)).and_then(|s| s.within(canvas));
            self.status = t!("sel-status-quick-mask-off");
            return;
        }
        // Without a selection, everything is selected: nothing is red.
        let mut image = RgbaImage::from_pixel(canvas.w, canvas.h, image::Rgba([255; 4]));
        if let Some(sel) = self.selection.take() {
            let mask = sel.to_mask(canvas);
            let raw = mask.as_raw();
            par_rows(image.as_mut(), row * 4, |y0, band| {
                for (i, p) in band.chunks_exact_mut(4).enumerate() {
                    let v = raw[y0 * row + i];
                    p.copy_from_slice(&[v, v, v, 255]);
                }
            });
        }
        self.sel.quick_mask = Some(QuickMask {
            image: Arc::new(image),
            rev: 0,
            stroke: None,
            undo: Vec::new(),
            redo: Vec::new(),
        });
        self.status = t!("sel-status-quick-mask-on", key = keys::key(Key::Q));
    }

    /// Brush and Eraser in Quick Mask Mode: they paint the mask, the Brush
    /// in the gray of the foreground color.
    pub(super) fn quick_mask_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) {
        let erase = self.tool == Tool::Eraser;
        let [r, g, b, _] = self.primary.to_srgba_unmultiplied();
        let gray = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).round() as u8;
        let settings = BrushSettings::round(
            [gray, gray, gray, 255],
            self.brush_size,
            self.hardness,
            self.opacity,
        );
        let Some(qm) = &mut self.sel.quick_mask else {
            return;
        };
        if start {
            qm.undo.push(Arc::clone(&qm.image));
            if qm.undo.len() > MAX_UNDO {
                qm.undo.remove(0);
            }
            qm.redo.clear();
            let mode = if erase {
                StrokeMode::Erase
            } else {
                StrokeMode::Paint
            };
            qm.stroke = Some(Stroke::begin(Arc::clone(&qm.image), mode, settings));
        }
        if let (Some(stroke), Some(p)) = (&mut qm.stroke, pointer)
            && (down || start)
        {
            let ip = view.to_image(p);
            if stroke
                .add_point(Arc::make_mut(&mut qm.image), ip.x, ip.y, 1.0)
                .is_some()
            {
                qm.rev += 1;
            }
        }
        if !down {
            qm.stroke = None;
        }
    }

    /// Ctrl+Z (`redo`: Shift+Ctrl+Z) in Quick Mask Mode: the last stroke on
    /// the mask. Returns `false` if there is none (outside of the mode too):
    /// the document's history goes on.
    pub(super) fn quick_mask_undo(&mut self, redo: bool) -> bool {
        let Some(qm) = &mut self.sel.quick_mask else {
            return false;
        };
        qm.stroke = None;
        let (from, to) = if redo {
            (&mut qm.redo, &mut qm.undo)
        } else {
            (&mut qm.undo, &mut qm.redo)
        };
        let Some(image) = from.pop() else {
            return false;
        };
        to.push(std::mem::replace(&mut qm.image, image));
        qm.rev += 1;
        true
    }

    /// The mask in red over the canvas, in Quick Mask Mode.
    pub(super) fn quick_mask_overlay(&self) -> Option<OverlaySource> {
        let qm = self.sel.quick_mask.as_ref()?;
        Some(OverlaySource {
            mask: OverlayMask::Paint(Arc::clone(&qm.image)),
            scale: 1.0,
            style: OverlayStyle::Tint([255, 0, 0], 0.5),
            key: hash_of((Arc::as_ptr(&qm.image) as usize, qm.rev)),
        })
    }
}
