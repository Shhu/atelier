//! Edit › Fade: the last pixel step, blended back over what was there, as a
//! history step of its own.

use std::sync::Arc;

use image::RgbaImage;

use super::*;
use crate::pixel_ops::{Fade, FadeArea};
use crate::selection::Selection;

/// What the step Fade would act on covered.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Covered {
    /// The selection, else the whole layer: a filter, an adjustment, a fill.
    Selection,
    /// The pixels it changed: a stroke, a brush, a transform.
    Changed,
    /// Nothing: the command changed no pixel, and an older step can't be
    /// faded in its place.
    Nothing,
}

/// The result Fade can act on: the history step, layer and target revision
/// right after it. Anything done since (a new step, an undo, another layer
/// or target) leaves it out of date.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct FadeMark {
    step: u64,
    layer: u64,
    target: u64,
    covered: Covered,
}

impl Document {
    /// The step just recorded, which worked on the selection, can be faded.
    pub fn mark_fade(&mut self) {
        self.fade_mark = Some(self.current_fade_mark(Covered::Selection));
    }

    /// The step just recorded can be faded, over the pixels it changed.
    pub fn mark_fade_changed(&mut self) {
        self.fade_mark = Some(self.current_fade_mark(Covered::Changed));
    }

    /// The command just run changed nothing: there is nothing to fade, and
    /// a Fade an action plays next has nothing to do.
    pub fn mark_fade_nothing(&mut self) {
        self.fade_mark = Some(self.current_fade_mark(Covered::Nothing));
    }

    /// The transform just recorded can be faded, if what was there still
    /// lies inside the layer's frame: the pixels it left outside couldn't
    /// come back.
    pub fn mark_fade_transformed(&mut self) {
        self.mark_fade_changed();
        let fits = self.before_step().is_some_and(|(before, from)| {
            let after = self.target_layer();
            after.is_some_and(|after| {
                framed_at(before.dimensions(), from, after.image().dimensions(), after.offset)
                    .is_some()
            })
        });
        if !fits {
            self.fade_mark = None;
        }
    }

    fn current_fade_mark(&self, covered: Covered) -> FadeMark {
        FadeMark {
            step: self.history_step(),
            layer: self.active_layer().id,
            target: self.target_rev(),
            covered,
        }
    }

    /// The mark, if still valid.
    fn fade_covered(&self) -> Option<Covered> {
        let mark = self.fade_mark?;
        (mark == self.current_fade_mark(mark.covered)).then_some(mark.covered)
    }

    /// The name of what can be faded now, if something can.
    pub fn fade_name(&self) -> Option<&str> {
        self.fade_covered()
            .filter(|c| *c != Covered::Nothing)
            .and_then(|_| self.undo.back().map(|s| s.name.as_str()))
    }

    /// The last command changed nothing, so a Fade after it has nothing to do.
    pub fn fade_of_nothing(&self) -> bool {
        self.fade_covered() == Some(Covered::Nothing)
    }

    /// The target's pixels before the last step, and their canvas position.
    fn before_step(&mut self) -> Option<(Arc<RgbaImage>, (i64, i64))> {
        let covered = self.fade_covered()?;
        let previous = self.undo.back()?.state.clone();
        let layer = self.active_layer().id;
        let current = self.restore(previous);
        let before = (self.active_layer().id == layer)
            .then(|| self.target_layer().map(|l| (l.shared_image(), l.offset)))
            .flatten();
        self.restore(current);
        // The quick mask's revision moved with the states.
        self.fade_mark = Some(self.current_fade_mark(covered));
        before
    }

    /// What Fade needs (the opacity and mode at 100% Normal): the target's
    /// pixels before the step, in the frame of the current ones, and the
    /// area it covered. `selection` is the one of the step: changing it is
    /// a step, which ends the Fade.
    pub fn fade_source(&mut self, selection: Option<&Selection>) -> Option<Fade> {
        self.fade_name()?;
        let covered = self.fade_covered()?;
        let (before, from) = self.before_step()?;
        let canvas = self.full_rect();
        let after = self.target_layer()?;
        let origin = after.offset;
        let (w, h) = after.image().dimensions();
        let area = match (covered, selection) {
            (Covered::Selection, None) => FadeArea::All,
            (Covered::Selection, Some(s)) => {
                s.clip(&after, canvas).map_or(FadeArea::Changed, FadeArea::Within)
            }
            _ => FadeArea::Changed,
        };
        let before = if (before.dimensions(), from) == ((w, h), origin) {
            before
        } else {
            // The step enlarged the layer: nothing else moves its pixels.
            let (dx, dy) = framed_at(before.dimensions(), from, (w, h), origin)?;
            let mut framed = RgbaImage::new(w, h);
            image::imageops::replace(&mut framed, &*before, dx, dy);
            Arc::new(framed)
        };
        Some(Fade {
            before,
            origin,
            area,
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

/// Where an image of `size` at `from` lies in a frame of `frame` at `origin`,
/// if it lies inside.
fn framed_at(
    size: (u32, u32),
    from: (i64, i64),
    frame: (u32, u32),
    origin: (i64, i64),
) -> Option<(i64, i64)> {
    let (dx, dy) = (from.0 - origin.0, from.1 - origin.1);
    let fits = dx >= 0
        && dy >= 0
        && dx + i64::from(size.0) <= i64::from(frame.0)
        && dy + i64::from(size.1) <= i64::from(frame.1);
    fits.then_some((dx, dy))
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
        let mut fade = d.fade_source(None).unwrap();
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
        let fade = d.fade_source(None).unwrap();
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
        let fade = d.fade_source(None).expect("the mask is the target");
        let mut image = d.target_layer().unwrap().image().clone();
        fade.apply(&mut image);
        let name = d.fade_name().unwrap().to_owned();
        assert!(d.fade_put(name, image));
    }

    #[test]
    fn a_step_that_changed_nothing_ends_the_one_before() {
        let mut d = doc();
        d.adjust_active(&Adjustment::Invert, None);
        d.mark_fade_nothing();
        assert!(d.fade_name().is_none());
        assert!(d.fade_source(None).is_none());
        assert!(d.fade_of_nothing());
        d.adjust_active(&Adjustment::Invert, None);
        assert!(d.fade_name().is_some());
        assert!(!d.fade_of_nothing());
    }

    #[test]
    fn a_selection_bounds_what_is_faded() {
        let mut d = doc();
        let selection: Selection = PixelRect { x: 0, y: 0, w: 2, h: 2 }.into();
        assert!(d.adjust_active(&Adjustment::Invert, Some(&selection)));
        let mut fade = d.fade_source(Some(&selection)).unwrap();
        assert!(matches!(fade.area, FadeArea::Within(_)));
        fade.mode = BlendMode::Multiply;
        let mut image = d.active_layer().image().clone();
        fade.apply(&mut image);
        assert_eq!(image.get_pixel(3, 3).0, [200, 100, 50, 255]);
        assert_ne!(image.get_pixel(0, 0), d.active_layer().image().get_pixel(0, 0));
    }

    #[test]
    fn a_transform_that_shrinks_the_frame_cant_be_faded() {
        let mut small = RgbaImage::new(2, 2);
        small.fill(255);
        let mut d = Document::new(RgbaImage::new(6, 6), None);
        d.add_empty_layer();
        let layer = d.active_layer_mut();
        layer.replace_pixels(small);
        layer.offset = (2, 2);
        d.checkpoint("Bigger".into());
        let layer = d.active_layer_mut();
        layer.replace_pixels(RgbaImage::new(4, 4));
        layer.offset = (1, 1);
        d.mark_fade_transformed();
        assert!(d.fade_name().is_some(), "what was there is inside");
        d.checkpoint("Smaller".into());
        let layer = d.active_layer_mut();
        layer.replace_pixels(RgbaImage::new(1, 1));
        d.mark_fade_transformed();
        assert!(d.fade_name().is_none(), "its corners would be lost");
    }
}
