//! What the clipboard commands take and give: Copy (the active layer or
//! its targeted mask), Copy Merged (everything visible), and the pasted
//! layers, in place or into the selection.

use image::{GrayImage, RgbaImage};

use super::{Document, Mask, PixelRect, intersect};
use crate::pixel_ops::scale;
use crate::selection::Selection;

impl Document {
    /// What Copy takes ([`Self::copied_pixels`]), with its box on the
    /// canvas (where Paste in Place puts it back).
    pub fn copied(&self, selection: Option<&Selection>) -> Option<(RgbaImage, PixelRect)> {
        let r = match selection {
            Some(s) => s.bounds(self.full_rect())?,
            None if self.mask_targeted() => self.full_rect(),
            None => self.active_layer().content_bounds()?,
        };
        Some((self.copied_pixels(selection)?, r))
    }

    /// Copy Merged: the visible layers together inside the selection (all
    /// of the canvas without one), with its box.
    pub fn merged_pixels(&self, selection: Option<&Selection>) -> Option<(RgbaImage, PixelRect)> {
        let canvas = self.full_rect();
        let r = match selection {
            Some(s) => intersect(s.bounds(canvas)?, canvas)?,
            None => canvas,
        };
        let mut image = self.composite(r, None);
        if let Some(s) = selection.filter(|s| s.inverted || s.mask.is_some()) {
            for (x, y, p) in image.enumerate_pixels_mut() {
                p.0[3] = scale(p.0[3], s.coverage(r.x + x as i64, r.y + y as i64));
            }
        }
        image.pixels().any(|p| p.0[3] > 0).then_some((image, r))
    }

    /// Paste in Place: `image` as a new layer at `offset` (canvas
    /// coordinates). One history step.
    pub fn paste_at(&mut self, step: String, name: String, image: RgbaImage, offset: (i64, i64)) {
        self.add_layer_as(step, name, image, offset);
    }

    /// Paste Into: `image` as a new layer centered on the selection, shown
    /// only inside it by a layer mask (unlinked, to move the pasted pixels
    /// in it). One history step.
    pub fn paste_into(
        &mut self,
        step: String,
        name: String,
        image: RgbaImage,
        selection: &Selection,
    ) {
        let canvas = self.full_rect();
        let Some(r) = selection.bounds(canvas) else {
            return self.add_centered_layer(step, name, image);
        };
        let offset = (
            r.x + (r.w as i64 - image.width() as i64) / 2,
            r.y + (r.h as i64 - image.height() as i64) / 2,
        );
        self.add_layer_as(step, name, image, offset);
        let coverage = GrayImage::from_fn(r.w, r.h, |x, y| {
            image::Luma([selection.coverage(r.x + x as i64, r.y + y as i64)])
        });
        let layer = self.active_layer_mut();
        let mut mask = Mask::new(coverage, (r.x, r.y), 0, layer.offset);
        mask.set_linked(false, layer.offset);
        layer.mask = Some(mask);
    }
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    #[test]
    fn copy_merged_takes_what_shows_inside_the_selection() {
        let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255])), None);
        doc.add_layer(
            "top".into(),
            RgbaImage::from_pixel(2, 2, Rgba([0, 0, 255, 255])),
            (4, 4),
        );
        let rect = PixelRect {
            x: 3,
            y: 3,
            w: 4,
            h: 4,
        };
        let (image, at) = doc.merged_pixels(Some(&rect.into())).unwrap();
        assert_eq!(at, rect);
        assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(1, 1).0, [0, 0, 255, 255]);
        // Copy takes the active layer only, from its own box.
        let (image, at) = doc.copied(None).unwrap();
        assert_eq!((image.dimensions(), (at.x, at.y)), ((2, 2), (4, 4)));
    }
}
