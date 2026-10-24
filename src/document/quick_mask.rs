//! Quick Mask Mode, like Photoshop's: the selection becomes a gray mask
//! the size of the canvas (white selected), edited by every tool and
//! operation in place of the active layer's pixels, then turned back into
//! the selection. Entering and leaving it are history steps, and the mask
//! is in the history like the layers: undoing past its start leaves the
//! mode, and it follows the canvas (crop, rotation, size).

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::{Document, Layer, LayerKind, PixelRect, intersect, next_rev};
use crate::mask_ops::par_rows;
use crate::selection::Selection;

impl Document {
    /// The quick mask, in Quick Mask Mode.
    pub fn quick_mask(&self) -> Option<&Arc<GrayImage>> {
        self.quick_mask.as_ref()
    }

    /// Changes whenever the quick mask does (display key).
    pub fn quick_mask_key(&self) -> u64 {
        self.quick_mask_rev
    }

    /// Enters Quick Mask Mode, as the step `name`: the selection becomes
    /// the mask (without one, everything is selected).
    pub fn enter_quick_mask(&mut self, name: String) {
        if self.quick_mask.is_some() {
            return;
        }
        let canvas = self.full_rect();
        let mask = match &self.selection {
            Some(sel) => sel.to_mask(canvas),
            None => GrayImage::from_pixel(canvas.w, canvas.h, image::Luma([255])),
        };
        self.checkpoint(name);
        self.quick_mask = Some(Arc::new(mask));
        self.quick_mask_rev = next_rev();
        self.selection = None;
    }

    /// Leaves Quick Mask Mode, as the step `name`: the mask becomes the
    /// selection (none if nothing is left).
    pub fn leave_quick_mask(&mut self, name: String) {
        if self.quick_mask.is_none() {
            return;
        }
        self.end_paint();
        let canvas = self.full_rect();
        let mask = self.fitted_quick_mask().expect("checked");
        let selection =
            Selection::from_mask(&mask, (canvas.x, canvas.y)).and_then(|s| s.within(canvas));
        self.checkpoint(name);
        self.quick_mask = None;
        self.quick_mask_rev = next_rev();
        self.selection = selection;
    }

    /// The quick mask at the size of the canvas (it always is; a mask of
    /// another size is put on the canvas as it is, cut or completed).
    fn fitted_quick_mask(&self) -> Option<Arc<GrayImage>> {
        let mask = self.quick_mask.as_ref()?;
        let (w, h) = self.canvas;
        if mask.dimensions() == (w, h) {
            return Some(Arc::clone(mask));
        }
        Some(Arc::new(crate::mask_ops::recanvas(mask, 0, 0, w, h)))
    }

    /// After a change of the canvas: the saved channels and the quick mask
    /// through `f`.
    pub(super) fn map_canvas_masks(&mut self, f: impl Fn(&GrayImage) -> GrayImage) {
        super::Channel::map_all(&mut self.channels, &f);
        if let Some(mask) = &mut self.quick_mask {
            *mask = Arc::new(f(mask));
            self.quick_mask_rev = next_rev();
        }
    }

    /// RGBA stand-in of the quick mask for the active layer (same id and
    /// settings, see `masks.rs`): its gray in the colors, opaque, over the
    /// canvas. `None` outside Quick Mask Mode.
    pub(super) fn quick_mask_stand_in(&self) -> Option<Layer> {
        let mask = self.fitted_quick_mask()?;
        let canvas = self.full_rect();
        let mut raw = vec![255u8; mask.as_raw().len() * 4];
        let (w, src) = (canvas.w as usize, mask.as_raw());
        par_rows(&mut raw, w.max(1) * 4, |first, band| {
            for (p, &v) in band.chunks_exact_mut(4).zip(&src[first * w..]) {
                p[..3].fill(v);
            }
        });
        let image = RgbaImage::from_raw(canvas.w, canvas.h, raw).expect("size");
        let real = self.real_active();
        Some(Layer {
            kind: LayerKind::Pixels,
            image: Arc::new(image),
            rev: next_rev(),
            bounds: std::cell::Cell::new(None),
            styles_cache: std::cell::RefCell::new(None),
            offset: (canvas.x, canvas.y),
            effects: Default::default(),
            mask: None,
            text: None,
            shape: None,
            artboard: None,
            smart: None,
            locked: false,
            visible: true,
            ..real.clone()
        })
    }

    /// Copies area `area` (canvas; all of it with `None`) of the quick
    /// mask's stand-in, being painted, into the quick mask.
    pub(super) fn sync_quick_mask(&mut self, area: Option<PixelRect>) {
        if self.swapped.is_none() {
            return;
        }
        let canvas = self.full_rect();
        let proxy = &self.layers[self.active];
        let p = proxy.rect();
        let Some(mask) = &mut self.quick_mask else {
            return;
        };
        let src = proxy.image.as_raw();
        if p != canvas || mask.dimensions() != (canvas.w, canvas.h) {
            // Resized or moved stand-in (whole-image operations): replaced.
            let mut image = GrayImage::new(canvas.w, canvas.h);
            if let Some(r) = intersect(p, canvas) {
                for y in r.y..r.bottom() {
                    for x in r.x..r.right() {
                        let i = (((y - p.y) * p.w as i64 + x - p.x) * 4) as usize;
                        let v = gray(src[i..i + 4].try_into().expect("4"));
                        image.put_pixel((x - canvas.x) as u32, (y - canvas.y) as u32, [v].into());
                    }
                }
            }
            *mask = Arc::new(image);
        } else {
            let Some(r) = intersect(area.unwrap_or(canvas), canvas) else {
                return;
            };
            let w = canvas.w as usize;
            let (x0, x1) = ((r.x - canvas.x) as usize, (r.right() - canvas.x) as usize);
            let (y0, y1) = ((r.y - canvas.y) as usize, (r.bottom() - canvas.y) as usize);
            // Compared on all the cores (24 MP after each stroke).
            let raw = mask.as_raw();
            let changed = crate::mask_ops::par_collect(y1 - y0, |rows| {
                let changed = rows.map(|j| y0 + j).any(|y| {
                    (x0..x1).any(|x| {
                        let i = y * w + x;
                        gray(src[i * 4..i * 4 + 4].try_into().expect("4")) != raw[i]
                    })
                });
                vec![changed]
            })
            .contains(&true);
            if !changed {
                return;
            }
            let out = &mut Arc::make_mut(mask).as_mut()[y0 * w..y1 * w];
            par_rows(out, w, |first, band| {
                for (j, row) in band.chunks_exact_mut(w).enumerate() {
                    let y = y0 + first + j;
                    let from = &src[(y * w + x0) * 4..(y * w + x1) * 4];
                    for (v, p) in row[x0..x1].iter_mut().zip(from.chunks_exact(4)) {
                        *v = gray(p.try_into().expect("4"));
                    }
                }
            });
        }
        self.quick_mask_rev = next_rev();
    }
}

/// Mask value of a stand-in pixel: its gray, what is erased hiding.
fn gray([r, g, b, a]: [u8; 4]) -> u8 {
    let luma = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
    ((luma * a as u32 + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn doc() -> Document {
        Document::new(RgbaImage::from_pixel(20, 10, Rgba([10, 20, 30, 255])), None)
    }

    fn left_half() -> Selection {
        PixelRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        }
        .into()
    }

    #[test]
    fn quick_mask_turns_the_selection_into_a_mask_and_back() {
        let mut doc = doc();
        doc.record_selection(Some(left_half()), "select".into());
        doc.enter_quick_mask("qm".into());
        assert!(doc.selection().is_none() && doc.mask_targeted());
        let mask = doc.quick_mask().unwrap();
        assert_eq!(mask.get_pixel(2, 2).0[0], 255);
        assert_eq!(mask.get_pixel(15, 2).0[0], 0);
        // Filling (here: the whole canvas in white) edits the mask, not the layer.
        assert!(doc.fill_active([255; 4], None, 1.0, false));
        assert_eq!(doc.layers[0].image().get_pixel(15, 2).0, [10, 20, 30, 255]);
        assert_eq!(doc.quick_mask().unwrap().get_pixel(15, 2).0[0], 255);
        doc.leave_quick_mask("qm".into());
        assert!(doc.quick_mask().is_none() && !doc.mask_targeted());
        let sel = doc.selection().unwrap();
        assert_eq!(sel.coverage(15, 2), 255);
        // Undo: back in Quick Mask Mode, then the fill undone, then out of it.
        assert!(doc.undo() && doc.quick_mask().is_some());
        assert!(doc.undo());
        assert_eq!(doc.quick_mask().unwrap().get_pixel(15, 2).0[0], 0);
        assert!(doc.undo() && doc.quick_mask().is_none());
        assert!(doc.selection().unwrap().same(&left_half()));
    }

    #[test]
    fn the_quick_mask_follows_the_canvas() {
        let mut doc = doc();
        doc.record_selection(Some(left_half()), "select".into());
        doc.enter_quick_mask("qm".into());
        doc.rotate_canvas(90.0);
        assert_eq!(doc.quick_mask().unwrap().dimensions(), (10, 20));
        // Rotated clockwise: the left half is now the top half.
        assert_eq!(doc.quick_mask().unwrap().get_pixel(5, 3).0[0], 255);
        assert_eq!(doc.quick_mask().unwrap().get_pixel(5, 15).0[0], 0);
        // Undoing the rotation brings back the mask that went with it.
        assert!(doc.undo());
        assert_eq!(doc.quick_mask().unwrap().dimensions(), (20, 10));
        doc.leave_quick_mask("qm".into());
        assert!(doc.selection().unwrap().same(&left_half()));
    }
}
