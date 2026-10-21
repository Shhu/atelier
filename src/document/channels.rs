//! Channels panel operations on the saved selections (Photoshop's alpha
//! channels), each one undo step, and the color channels of the image.

use std::sync::Arc;

use image::GrayImage;

use super::{Channel, Document};
use crate::i18n::t;
use crate::selection::Selection;

impl Document {
    /// New Channel: an empty (black) alpha channel. Returns its index.
    pub fn new_channel(&mut self) -> usize {
        let (w, h) = (self.width(), self.height());
        self.add_channel(GrayImage::new(w, h), t!("xchg-hist-new-channel"))
    }

    /// Save Selection as Channel (the panel's button): `mask`, the
    /// selection over the canvas, as a new alpha channel.
    pub fn save_selection_channel(&mut self, mask: GrayImage) -> usize {
        self.add_channel(mask, t!("xchg-hist-save-selection"))
    }

    /// A new alpha channel, numbered like Photoshop's ("Alpha 1",
    /// "Alpha 2"…), as the history step `step`.
    fn add_channel(&mut self, mask: GrayImage, step: String) -> usize {
        let make = |n: usize| t!("xchg-channel-alpha-name", number = n);
        let name = (1..=self.channels.len() + 1)
            .map(make)
            .find(|n| self.channels.iter().all(|c| c.name != *n))
            .unwrap_or_else(|| make(1));
        self.checkpoint(step);
        self.channels.push(Channel {
            name,
            mask: Arc::new(mask),
        });
        self.channels.len() - 1
    }

    /// Duplicate Channel: a copy of channel `i` just after it ("… copy").
    pub fn duplicate_channel(&mut self, i: usize) -> Option<usize> {
        let c = self.channels.get(i)?.clone();
        self.checkpoint(t!("xchg-hist-duplicate-channel"));
        let name = t!("doc-layer-copy-name", name = c.name.as_str());
        self.channels.insert(i + 1, Channel { name, ..c });
        Some(i + 1)
    }

    pub fn delete_channel(&mut self, i: usize) -> bool {
        if i >= self.channels.len() {
            return false;
        }
        self.checkpoint(t!("xchg-hist-delete-channel"));
        self.channels.remove(i);
        true
    }

    /// Renames channel `i` (an empty or unchanged name changes nothing).
    pub fn rename_channel(&mut self, i: usize, name: &str) -> bool {
        let name = name.trim();
        match self.channels.get(i) {
            Some(c) if !name.is_empty() && c.name != name => {
                self.checkpoint(t!("xchg-hist-channel-properties"));
                self.channels[i].name = name.to_owned();
                true
            }
            _ => false,
        }
    }

    /// Selection from channel `i` (Ctrl + click in the Channels panel).
    pub fn channel_selection(&self, i: usize) -> Option<Selection> {
        let c = self.channels.get(i)?;
        Selection::from_mask(&c.mask, (0, 0))
    }

    /// Red, green or blue (`c` 0..3) of the flattened image, transparency
    /// counting as white: a color channel as the Channels panel shows it.
    pub fn color_channel(&self, c: usize) -> GrayImage {
        let flat = self.flatten();
        let (w, h) = flat.dimensions();
        let raw = flat
            .pixels()
            .map(|p| {
                let a = p.0[3] as u32;
                ((p.0[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8
            })
            .collect();
        GrayImage::from_raw(w, h, raw).expect("matching size")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Luma, Rgba, RgbaImage};

    #[test]
    fn channel_operations_are_undo_steps() {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 2, Rgba([200, 100, 0, 255])), None);
        let a = doc.new_channel();
        let b = doc.new_channel();
        assert_eq!((a, b), (0, 1));
        Arc::make_mut(&mut doc.channels[0].mask).put_pixel(1, 1, Luma([255]));
        assert_eq!(doc.duplicate_channel(0), Some(1));
        assert_eq!(doc.channels.len(), 3);
        assert_eq!(doc.channels[1].mask.get_pixel(1, 1).0[0], 255);
        assert!(doc.rename_channel(1, " Sky "));
        assert!(!doc.rename_channel(1, "Sky"), "unchanged");
        assert!(!doc.rename_channel(1, "  "), "empty");
        assert_eq!(doc.channels[1].name, "Sky");
        assert!(doc.delete_channel(2));
        assert!(!doc.delete_channel(5));
        let sel = doc.channel_selection(1).unwrap();
        assert_eq!(sel.coverage(1, 1), 255);
        assert_eq!(sel.coverage(0, 0), 0);
        // Each was one step, named.
        let (names, done) = doc.history();
        assert_eq!(done, 5);
        assert_eq!(names[2], "Duplicate Channel");
        assert!(doc.go_to_step(2));
        assert_eq!(doc.channels.len(), 2);
        // Color channels of the composite.
        assert_eq!(doc.color_channel(0).get_pixel(0, 0).0[0], 200);
        assert_eq!(doc.color_channel(2).get_pixel(3, 1).0[0], 0);
    }
}
