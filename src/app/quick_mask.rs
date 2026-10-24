//! Edit in Quick Mask Mode (Q), like Photoshop: the selection becomes a
//! mask, shown in red over what isn't selected. Every tool and operation
//! then edits it instead of the layer (black masks, white selects, grays
//! in between: see `document/quick_mask.rs`). Q again turns it back into
//! the selection. Entering and leaving it, and each change of the mask,
//! are history steps.

use super::keys;
use super::select_menu::{OverlayMask, OverlaySource, OverlayStyle};
use super::*;
use crate::i18n::t;

impl AtelierApp {
    pub(super) fn toggle_quick_mask(&mut self) {
        self.finish_stroke();
        self.settle_selection();
        let Some(doc) = &mut self.doc else { return };
        let name = t!("hist3-quick-mask");
        if doc.quick_mask().is_some() {
            doc.leave_quick_mask(name);
            self.status = t!("sel-status-quick-mask-off");
        } else {
            doc.enter_quick_mask(name);
            self.status = t!("sel-status-quick-mask-on", key = keys::key(Key::Q));
        }
        self.prop_session = None;
        self.pull_selection();
    }

    /// The mask in red over the canvas, in Quick Mask Mode; a dialog's
    /// preview (a filter, an adjustment) shows there.
    pub(super) fn quick_mask_overlay(&self) -> Option<OverlaySource> {
        let doc = self.doc.as_ref()?;
        let mask = doc.quick_mask()?;
        let previewed = self.preview_shown.as_ref().and_then(|p| match &p.mask {
            Some((gray, _)) if p.quick_mask && gray.dimensions() == mask.dimensions() => {
                Some((gray, p.fingerprint))
            }
            _ => None,
        });
        let (mask, key) = match previewed {
            Some((gray, version)) => (gray, hash_of((doc.uid(), "preview", version))),
            None => (mask, hash_of((doc.uid(), doc.quick_mask_key()))),
        };
        Some(OverlaySource {
            mask: OverlayMask::Gray(Arc::clone(mask)),
            scale: 1.0,
            style: OverlayStyle::Tint([255, 0, 0], 0.5),
            key,
        })
    }

    /// In Quick Mask Mode, moving and transforming are refused: they would
    /// act on the layer under the mask. Says so and returns `true`.
    pub(super) fn refuse_in_quick_mask(&mut self) -> bool {
        let on = self.doc.as_ref().is_some_and(|d| d.quick_mask().is_some());
        if on {
            self.status = t!("sel4-quick-mask-no-move", key = keys::key(Key::Q));
        }
        on
    }

    /// The Quick Mask button under the colors, like Photoshop's: on while
    /// in Quick Mask Mode.
    pub(super) fn quick_mask_button(&mut self, ui: &mut egui::Ui) {
        let on = self.doc.as_ref().is_some_and(|d| d.quick_mask().is_some());
        let (rect, response) = ui.allocate_exact_size(vec2(28.0, 22.0), Sense::click());
        let enabled = self.doc.is_some();
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let fill = match (on, hovered) {
            (true, _) => ui_kit::color::ACCENT_SOFT,
            (false, true) => ui_kit::color::BUTTON_HOVER,
            (false, false) => Color32::TRANSPARENT,
        };
        painter.rect_filled(rect, 5.0, fill);
        // A frame with a circle in it: filled when the outside is masked.
        let ink = match (on, enabled) {
            (true, _) => ui_kit::color::ACCENT_TEXT,
            (false, true) if hovered => ui_kit::color::TEXT,
            (false, true) => ui_kit::color::MUTED,
            (false, false) => ui_kit::color::DISABLED,
        };
        let frame = Rect::from_center_size(rect.center(), vec2(16.0, 12.0));
        painter.rect_stroke(frame, 2.0, UiStroke::new(1.0, ink), StrokeKind::Inside);
        if on {
            painter.circle_filled(frame.center(), 3.5, ink);
        } else {
            painter.circle_stroke(frame.center(), 3.5, UiStroke::new(1.0, ink));
        }
        let tip = t!("sel-menu-quick-mask");
        if enabled && ui_kit::tip_with_key(response, &tip, &keys::key(Key::Q)).clicked() {
            self.run(Action::Select(super::select_menu::Command::QuickMask));
        }
    }
}
