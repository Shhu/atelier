//! Edit › Fade: the last filter, adjustment or stroke, blended back over
//! what was there, as a history step of its own.

use std::sync::Arc;

use image::RgbaImage;

use super::*;
use crate::pixel_ops::Fade;

/// The result Fade can act on: the history step, layer and target revision
/// right after it. Anything done since (a new step, an undo, another layer
/// or target) leaves it out of date.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FadeMark {
    step: u64,
    layer: u64,
    target: u64,
}

impl Document {
    /// The step just recorded can be faded.
    pub fn mark_fade(&mut self) {
        self.fade_mark = Some(self.current_fade_mark());
    }

    fn current_fade_mark(&self) -> FadeMark {
        FadeMark {
            step: self.history_step(),
            layer: self.active_layer().id,
            target: self.target_rev(),
        }
    }

    /// The name of what can be faded now, if something can.
    pub fn fade_name(&self) -> Option<&str> {
        let current = self.fade_mark.is_some_and(|m| m == self.current_fade_mark());
        current
            .then(|| self.undo.back().map(|s| s.name.as_str()))
            .flatten()
    }

    /// What Fade needs (the opacity and mode at 100% Normal): the target's
    /// pixels before the step, in the frame of the current ones.
    pub fn fade_source(&mut self) -> Option<Fade> {
        self.fade_name()?;
        let previous = self.undo.back()?.state.clone();
        let layer = self.active_layer().id;
        let current = self.restore(previous);
        let before = (self.active_layer().id == layer)
            .then(|| self.target_layer().map(|l| (l.shared_image(), l.offset)))
            .flatten();
        self.restore(current);
        // The quick mask's revision moved with the states.
        self.mark_fade();
        let (before, from) = before?;
        let after = self.target_layer()?;
        let origin = after.offset;
        let (w, h) = after.image().dimensions();
        let before = if (before.dimensions(), from) == ((w, h), origin) {
            before
        } else {
            // The step enlarged the layer: nothing else moves its pixels.
            let (dx, dy) = (from.0 - origin.0, from.1 - origin.1);
            let fits = dx >= 0
                && dy >= 0
                && dx + i64::from(before.width()) <= i64::from(w)
                && dy + i64::from(before.height()) <= i64::from(h);
            if !fits {
                return None;
            }
            let mut framed = RgbaImage::new(w, h);
            image::imageops::replace(&mut framed, &*before, dx, dy);
            Arc::new(framed)
        };
        Some(Fade {
            before,
            origin,
            opacity: 1.0,
            mode: BlendMode::Normal,
        })
    }

    /// `image` (what [`Fade::apply`] gave) replaces the target's pixels, in a
    /// new step named `name`. It can't be faded again.
    pub fn fade_put(&mut self, name: String, image: RgbaImage) -> bool {
        if self.fade_name().is_none() {
            return false;
        }
        let fits = self
            .target_layer()
            .is_some_and(|l| l.image().dimensions() == image.dimensions());
        if !fits {
            return false;
        }
        self.checkpoint(name);
        self.with_target(|doc| doc.active_layer_mut().replace_pixels(image));
        self.step += 1;
        self.state_id = next_rev();
        self.modified = true;
        self.fade_mark = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_ops::{Adjustment, AdjustKind};
    use image::Rgba;

    fn doc() -> Document {
        Document::new(RgbaImage::from_pixel(4, 4, Rgba([200, 100, 50, 255])), None)
    }

    #[test]
    fn fade_needs_the_last_step_and_is_a_step_of_its_own() {
        let mut d = doc();
        assert!(d.fade_name().is_none(), "nothing yet");
        let steps = d.history().1;
        assert!(d.adjust_active(&Adjustment::Invert, None));
        assert_eq!(d.fade_name(), Some(AdjustKind::Invert.title().as_str()));
        let mut fade = d.fade_source().unwrap();
        assert_eq!(fade.before.get_pixel(0, 0).0, [200, 100, 50, 255]);
        fade.opacity = 0.5;
        let mut image = d.active_layer().image().clone();
        fade.apply(&mut image);
        assert!(d.fade_put("Fade".into(), image));
        assert_eq!(d.active_layer().image().get_pixel(0, 0).0[0], 128);
        assert_eq!(d.history().1, steps + 2, "a step of its own");
        assert_eq!(d.history().0.last(), Some(&"Fade"));
        assert!(d.fade_name().is_none(), "once");
        assert!(d.undo());
        assert_eq!(d.active_layer().image().get_pixel(0, 0).0[0], 55);
        assert!(d.fade_name().is_none());
        assert!(d.undo());
        assert_eq!(d.active_layer().image().get_pixel(0, 0).0[0], 200);
    }

    #[test]
    fn another_step_or_layer_ends_it() {
        let mut d = doc();
        d.adjust_active(&Adjustment::Invert, None);
        d.checkpoint("Something".into());
        assert!(d.fade_name().is_none());
        d.adjust_active(&Adjustment::Invert, None);
        assert!(d.fade_name().is_some());
        d.add_empty_layer();
        assert!(d.fade_name().is_none());
        // Undo and redo don't bring it back.
        let mut d = doc();
        d.adjust_active(&Adjustment::Invert, None);
        d.undo();
        d.redo();
        assert!(d.fade_name().is_none());
    }

    #[test]
    fn a_layer_enlarged_by_the_step_keeps_the_frame() {
        let mut small = RgbaImage::new(2, 2);
        small.fill(255);
        let mut d = Document::new(RgbaImage::new(6, 6), None);
        d.add_empty_layer();
        let layer = d.active_layer_mut();
        layer.replace_pixels(small);
        layer.offset = (2, 2);
        assert!(d.fill_active([0, 0, 0, 255], None, 1.0, false));
        d.mark_fade();
        let fade = d.fade_source().unwrap();
        let after = d.active_layer().image().dimensions();
        assert_eq!(fade.before.dimensions(), after);
        assert_eq!(after, (6, 6));
        assert_eq!(fade.before.get_pixel(2, 2).0, [255, 255, 255, 255]);
        assert_eq!(fade.before.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn fades_a_quick_mask() {
        let mut d = doc();
        d.enter_quick_mask("Quick Mask".into());
        assert!(d.adjust_active(&Adjustment::Invert, None));
        let fade = d.fade_source().expect("the mask is the target");
        let mut image = d.target_layer().unwrap().image().clone();
        fade.apply(&mut image);
        let name = d.fade_name().unwrap().to_owned();
        assert!(d.fade_put(name, image));
    }
}
