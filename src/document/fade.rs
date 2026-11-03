//! Edit › Fade: the last pixel step, blended back over what was there, as a
//! history step of its own.

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::*;
use crate::pixel_ops::{Clip, ClipShape, Fade, FadeArea};
use crate::selection::Selection;

/// What the step Fade would act on covered: Fade blends there, even the
/// pixels the step left as they were.
#[derive(Clone)]
enum Covered {
    /// The selection, else the whole layer: a filter, an adjustment, a fill.
    Selection,
    /// What it painted, with its coverage, at its canvas position: a brush,
    /// the Paint Bucket, a stroke.
    Painted(Arc<GrayImage>, (i64, i64)),
    /// The pixels it changed: the brushes that work dab after dab (Blur,
    /// Smudge…).
    Changed,
    /// The pixels it changed, and the canvas box it put its result in: a
    /// transform.
    Moved(PixelRect),
    /// Nothing: the command changed no pixel, and an older step can't be
    /// faded in its place.
    Nothing,
}

/// The result Fade can act on: the history step, layer and target revision
/// right after it. Anything done since (a new step, an undo, another layer
/// or target) leaves it out of date.
#[derive(Clone)]
pub(super) struct FadeMark {
    step: u64,
    layer: u64,
    target: u64,
    covered: Covered,
}

impl Document {
    /// The step just recorded, which worked on the selection, can be faded.
    pub fn mark_fade(&mut self) {
        self.set_fade_mark(Covered::Selection);
    }

    /// The step just recorded painted `coverage` (its corner at the canvas
    /// position `at`): it can be faded there.
    pub fn mark_fade_painted(&mut self, coverage: GrayImage, at: (i64, i64)) {
        self.set_fade_mark(Covered::Painted(Arc::new(coverage), at));
    }

    /// The step just recorded can be faded, over the pixels it changed.
    pub fn mark_fade_changed(&mut self) {
        self.set_fade_mark(Covered::Changed);
    }

    /// The command just run changed nothing: there is nothing to fade, and
    /// a Fade an action plays next has nothing to do.
    pub fn mark_fade_nothing(&mut self) {
        self.set_fade_mark(Covered::Nothing);
    }

    /// The transform just recorded, which put its result in the canvas box
    /// `to`, can be faded: over the pixels it changed and that box. The
    /// layer's frame grows to hold what was there if it must.
    pub fn mark_fade_transformed(&mut self, to: Option<PixelRect>) {
        let to = to.unwrap_or(PixelRect { x: 0, y: 0, w: 0, h: 0 });
        self.set_fade_mark(Covered::Moved(to));
    }

    fn set_fade_mark(&mut self, covered: Covered) {
        self.fade_mark = Some(FadeMark {
            step: self.history_step(),
            layer: self.active_layer().id,
            target: self.target_rev(),
            covered,
        });
    }

    /// What the mark covered, if it is still valid.
    fn fade_covered(&self) -> Option<&Covered> {
        let mark = self.fade_mark.as_ref()?;
        let now = (self.history_step(), self.active_layer().id, self.target_rev());
        (now == (mark.step, mark.layer, mark.target)).then_some(&mark.covered)
    }

    /// The name of what can be faded now, if something can.
    pub fn fade_name(&self) -> Option<&str> {
        self.fade_covered()
            .filter(|c| !matches!(c, Covered::Nothing))
            .and_then(|_| self.undo.back().map(|s| s.name.as_str()))
    }

    /// The last command changed nothing, so a Fade after it has nothing to do.
    pub fn fade_of_nothing(&self) -> bool {
        matches!(self.fade_covered(), Some(Covered::Nothing))
    }

    /// The target's pixels before the last step, and their canvas position.
    fn before_step(&mut self) -> Option<(Arc<RgbaImage>, (i64, i64))> {
        let covered = self.fade_covered()?.clone();
        let previous = self.undo.back()?.state.clone();
        let layer = self.active_layer().id;
        let current = self.restore(previous);
        let before = (self.active_layer().id == layer)
            .then(|| self.target_layer().map(|l| (l.shared_image(), l.offset)))
            .flatten();
        self.restore(current);
        // The quick mask's revision moved with the states.
        self.set_fade_mark(covered);
        before
    }

    /// What Fade needs (the opacity and mode at 100% Normal): the target's
    /// pixels before the step, in the frame of the current ones, and the
    /// area it covered. `selection` is the one of the step: changing it is
    /// a step, which ends the Fade.
    pub fn fade_source(&mut self, selection: Option<&Selection>) -> Option<Fade> {
        self.fade_name()?;
        let covered = self.fade_covered()?.clone();
        let (before, from) = self.before_step()?;
        self.frame_for_fade(before.dimensions(), from)?;
        let canvas = self.full_rect();
        let after = self.target_layer()?;
        let origin = after.offset;
        let (w, h) = after.image().dimensions();
        let area = match (covered, selection) {
            (Covered::Selection, None) => FadeArea::All,
            (Covered::Selection, Some(s)) => {
                s.clip(&after, canvas).map_or(FadeArea::Changed, FadeArea::Within)
            }
            (Covered::Painted(mask, at), _) => FadeArea::Within(Clip {
                rect: [0, 0, w, h],
                shape: Some(ClipShape::Mask(mask, (at.0 - origin.0, at.1 - origin.1))),
                inverted: false,
            }),
            (Covered::Changed, _) => FadeArea::Changed,
            (Covered::Moved(to), _) => {
                let x = |v: i64| (v - origin.0).clamp(0, i64::from(w)) as u32;
                let y = |v: i64| (v - origin.1).clamp(0, i64::from(h)) as u32;
                let r = [x(to.x), y(to.y), x(to.x + i64::from(to.w)), y(to.y + i64::from(to.h))];
                FadeArea::ChangedOr(r.into())
            }
            (Covered::Nothing, _) => return None,
        };
        let before = if (before.dimensions(), from) == ((w, h), origin) {
            before
        } else {
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

    /// The target's frame holds the pixels before the step (`size` at
    /// `from`): a step that moved them out of it (a transform turning or
    /// shrinking the layer) has the layer's frame grown to hold both, with
    /// transparent margins, so that the faded result keeps them. `None` if
    /// it can't (a mask's frame, a box too large).
    fn frame_for_fade(&mut self, size: (u32, u32), from: (i64, i64)) -> Option<()> {
        let after = self.target_layer()?;
        let (frame, origin) = (after.image().dimensions(), after.offset);
        if framed_at(size, from, frame, origin).is_some() {
            return Some(());
        }
        let rect = after.rect();
        drop(after);
        if self.mask_targeted() {
            return None;
        }
        let old = PixelRect {
            x: from.0,
            y: from.1,
            w: size.0,
            h: size.1,
        };
        let u = old.union(rect);
        if !self.fits(u) {
            return None;
        }
        let covered = self.fade_covered()?.clone();
        let layer = &mut self.layers[self.active];
        let mut image = RgbaImage::new(u.w, u.h);
        image::imageops::replace(&mut image, layer.image(), rect.x - u.x, rect.y - u.y);
        layer.replace_pixels(image);
        layer.reframe((u.x, u.y));
        // Not a change of its pixels: Fade still follows the step.
        self.set_fade_mark(covered);
        Some(())
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
    fn a_transform_that_shrinks_the_frame_grows_it_back_for_fade() {
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
        d.mark_fade_transformed(None);
        assert!(d.fade_source(None).is_some(), "what was there is inside");
        assert_eq!(d.active_layer().rect(), PixelRect { x: 1, y: 1, w: 4, h: 4 });
        d.checkpoint("Smaller".into());
        let layer = d.active_layer_mut();
        layer.replace_pixels(RgbaImage::new(1, 1));
        layer.offset = (3, 3);
        d.mark_fade_transformed(Some(PixelRect { x: 3, y: 3, w: 1, h: 1 }));
        let fade = d.fade_source(None).expect("its corners come back");
        assert_eq!(d.active_layer().rect(), PixelRect { x: 1, y: 1, w: 4, h: 4 });
        assert_eq!(fade.before.dimensions(), (4, 4));
        assert!(d.fade_name().is_some(), "still the step to fade");
        assert!(matches!(fade.area, FadeArea::ChangedOr(_)));
    }
}
