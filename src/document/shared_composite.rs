//! The composite the Sample All Layers tools start each stroke from, kept
//! between strokes: only what changed since is composited again.

use std::sync::{Arc, Mutex};

use image::RgbaImage;

use super::{Document, PixelRect, intersect};
use crate::compose;

/// The last composite: document (uid), area, the layers' fingerprints
/// then, the canvas areas painted since (layer id), and the image.
struct Shared {
    doc: u64,
    region: PixelRect,
    sigs: Vec<compose::Sig>,
    painted: Vec<(u64, PixelRect)>,
    image: Arc<RgbaImage>,
}

static LAST: Mutex<Option<Shared>> = Mutex::new(None);

/// Brush strokes on document `doc` since the last frame (see
/// [`Document::composite_shared`]): layer id and canvas area.
pub fn note_painted(doc: u64, strokes: &[(u64, PixelRect)]) {
    if strokes.is_empty() {
        return;
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = last.as_mut().filter(|s| s.doc == doc) {
        s.painted.extend_from_slice(strokes);
    }
}

impl Document {
    /// [`Self::composite`] of `region`, kept for the next call: then only
    /// what changed is composited again.
    pub fn composite_shared(&self, region: PixelRect) -> Arc<RgbaImage> {
        let options = compose::Options {
            held: self.effects_held,
            ..Default::default()
        };
        let layers = self.render_layers();
        let sigs = compose::signatures(&layers, &options);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let kept = last
            .take()
            .filter(|s| (s.doc, s.region) == (self.uid, region));
        let plan = || compose::plan(&layers, self.full_rect(), &mut compose::FullSize, &options);
        let image = match kept {
            Some(s) => match compose::dirty_area(&s.sigs, &sigs, &s.painted) {
                compose::Dirty::Nothing => s.image,
                compose::Dirty::Area(a) => {
                    let mut image = s.image;
                    if let Some(a) = intersect(a, region) {
                        let part = plan().render(a);
                        let at = (a.x - region.x, a.y - region.y);
                        image::imageops::replace(Arc::make_mut(&mut image), &part, at.0, at.1);
                    }
                    image
                }
                compose::Dirty::All => Arc::new(plan().render(region)),
            },
            None => Arc::new(plan().render(region)),
        };
        *last = Some(Shared {
            doc: self.uid,
            region,
            sigs,
            painted: Vec::new(),
            image: Arc::clone(&image),
        });
        image
    }
}
