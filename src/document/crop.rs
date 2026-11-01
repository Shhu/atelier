//! The Crop tool's cut: the canvas, and what falls outside it if asked.

use image::RgbaImage;

use super::{Document, Layer, PixelRect, intersect};
use crate::i18n::t;

impl Document {
    /// Crop to `r`; with `delete`, the layers' pixels and masks beyond the
    /// new canvas go as well. One step.
    pub fn crop_area(&mut self, r: PixelRect, delete: bool) {
        let mark = self.history_mark();
        self.crop(r);
        if delete && self.delete_outside_canvas() {
            self.join_since(mark);
        }
    }

    /// Cuts the plain pixel layers and every mask to the canvas, as its own
    /// step. Texts, shapes, smart objects and artboards are not pixels to
    /// cut, nor is what a board holds (the board stays as it was): they stay
    /// whole. Returns whether anything went.
    fn delete_outside_canvas(&mut self) -> bool {
        let canvas = self.full_rect();
        let overhangs = |r: PixelRect| r.w > 0 && r.h > 0 && intersect(r, canvas) != Some(r);
        let cuttable: Vec<usize> = (0..self.layers.len())
            .filter(|&i| {
                let l = &self.layers[i];
                if self.artboard_of(i).is_some() {
                    return false;
                }
                plain_pixels(l) && overhangs(l.rect())
                    || l.mask.as_ref().is_some_and(|m| overhangs(m.rect(l.offset)))
            })
            .collect();
        if cuttable.is_empty() {
            return false;
        }
        self.checkpoint(t!("menu-image-crop"));
        for i in cuttable {
            let layer = &mut self.layers[i];
            let old = layer.offset;
            if plain_pixels(layer) && overhangs(layer.rect()) {
                let r = layer.rect();
                let (image, offset) = match intersect(r, canvas) {
                    Some(k) => (
                        image::imageops::crop_imm(
                            layer.image(),
                            (k.x - r.x) as u32,
                            (k.y - r.y) as u32,
                            k.w,
                            k.h,
                        )
                        .to_image(),
                        (k.x, k.y),
                    ),
                    None => (RgbaImage::new(0, 0), (0, 0)),
                };
                layer.replace_pixels(image);
                layer.offset = offset;
            }
            layer.edit_mask(old, |m| m.cut_to(canvas));
        }
        true
    }
}

/// Pixels that can be cut: not what renders them.
fn plain_pixels(l: &Layer) -> bool {
    l.has_pixels() && l.text.is_none() && l.shape.is_none() && l.smart.is_none() && l.artboard.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);

    fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    #[test]
    fn delete_cropped_pixels_cuts_the_layers_in_one_step() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 80, RED), None);
        doc.crop_area(rect(10, 20, 30, 40), true);
        assert_eq!((doc.width(), doc.height()), (30, 40));
        assert_eq!(doc.layers[0].rect(), rect(0, 0, 30, 40));
        assert!(doc.undo());
        assert_eq!((doc.width(), doc.height()), (100, 80));
        assert_eq!(doc.layers[0].rect(), rect(0, 0, 100, 80));
    }

    #[test]
    fn without_delete_the_pixels_stay() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 80, RED), None);
        doc.crop_area(rect(10, 20, 30, 40), false);
        assert_eq!(doc.layers[0].rect(), rect(-10, -20, 100, 80));
    }

    #[test]
    fn a_layer_entirely_outside_is_emptied_and_a_mask_is_cut() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 80, RED), None);
        doc.add_layer("far".into(), RgbaImage::from_pixel(5, 5, RED), (90, 70));
        doc.active = 0;
        let sel = crate::selection::Selection::from(rect(0, 0, 100, 80));
        assert!(doc.add_mask(crate::document::NewMask::RevealSelection, Some(&sel)));
        doc.crop_area(rect(0, 0, 20, 20), true);
        assert_eq!(doc.layers[1].image().width(), 0);
        let (layer, mask) = (&doc.layers[0], doc.layers[0].mask.as_ref().expect("a mask"));
        assert_eq!(mask.rect(layer.offset), rect(0, 0, 20, 20));
    }
}
