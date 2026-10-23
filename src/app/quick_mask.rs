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

    /// The mask in red over the canvas, in Quick Mask Mode.
    pub(super) fn quick_mask_overlay(&self) -> Option<OverlaySource> {
        let doc = self.doc.as_ref()?;
        let mask = doc.quick_mask()?;
        Some(OverlaySource {
            mask: OverlayMask::Gray(Arc::clone(mask)),
            scale: 1.0,
            style: OverlayStyle::Tint([255, 0, 0], 0.5),
            key: hash_of((doc.uid(), doc.quick_mask_key())),
        })
    }
}
