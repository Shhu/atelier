//! Layer styles drawn on worker threads for the display: on a large layer
//! they take up to a second (distance fields, blurs), so the view keeps
//! showing the former ones until the new ones are ready.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::{Layer, StylesCache, next_rev};
use crate::compose::styles::{Effects, Styles, render_styles};

/// Below this many pixels, styles are drawn at once (quick enough).
const AT_ONCE: u64 = 1 << 20;

/// What styles are drawn from: pixel revision, mask fingerprint, settings.
#[derive(Clone, PartialEq)]
struct Key {
    rev: u64,
    mask: u64,
    effects: Effects,
}

/// Styles being drawn for a layer (`None` until done).
pub(super) struct Job {
    key: Key,
    done: Mutex<Option<Arc<Styles>>>,
}

/// The styles being drawn for a layer, shared by its copies (the display
/// composites copies of the layers while a mask is painted): one job at a
/// time, a newer request waits for it to end.
pub(super) type JobSlot = Arc<Mutex<Option<Arc<Job>>>>;

/// Jobs still running.
static RUNNING: AtomicUsize = AtomicUsize::new(0);

/// Whether styles are being drawn (the display then looks again soon).
pub fn styles_pending() -> bool {
    RUNNING.load(Ordering::Relaxed) > 0
}

impl Layer {
    /// The styles as the display shows them (see [`Self::styles`]): those
    /// of the current pixels, mask and settings when drawn; otherwise they
    /// are drawn on a worker thread, and meanwhile the former ones show
    /// (none for a layer that never had any).
    pub fn styles_later(&self, held: bool) -> Option<Arc<Styles>> {
        if !self.has_pixels() || !self.effects.rendered() {
            return None;
        }
        let (w, h) = self.image.dimensions();
        if u64::from(w) * u64::from(h) <= AT_ONCE {
            return self.styles(held);
        }
        let key = Key {
            rev: self.rev,
            mask: self.mask_key(),
            effects: self.effects.clone(),
        };
        if let Some(c) = self.styles_cache.borrow().as_ref()
            && c.effects == key.effects
            && (held || (c.rev, c.mask) == (key.rev, key.mask))
        {
            return Some(Arc::clone(&c.styles));
        }
        let mut slot = self.styles_job.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = slot.as_ref() {
            let done = job.done.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let Some(styles) = done else {
                // Still drawing (for these settings or older ones): the
                // former styles meanwhile.
                return self.styles_cached();
            };
            *self.styles_cache.borrow_mut() = Some(StylesCache {
                rev: job.key.rev,
                mask: job.key.mask,
                effects: job.key.effects.clone(),
                styles: Arc::clone(&styles),
            });
            if job.key == key {
                return Some(styles);
            }
        }
        let job = Arc::new(Job {
            key,
            done: Mutex::new(None),
        });
        let (image, mask, offset) = (
            self.shared_image(),
            self.active_mask().cloned(),
            self.offset,
        );
        let work = Arc::clone(&job);
        RUNNING.fetch_add(1, Ordering::Relaxed);
        let spawned = std::thread::Builder::new()
            .name("styles".into())
            .spawn(move || {
                /// The count goes down even if drawing fails.
                struct Running;
                impl Drop for Running {
                    fn drop(&mut self) {
                        RUNNING.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                let _running = Running;
                let shape = match mask {
                    Some(m) => Arc::new(m.apply_to(&image, offset)),
                    None => image,
                };
                let styles = Arc::new(Styles {
                    version: next_rev(),
                    ..render_styles(&shape, &work.key.effects)
                });
                *work.done.lock().unwrap_or_else(|e| e.into_inner()) = Some(styles);
            });
        if spawned.is_err() {
            RUNNING.fetch_sub(1, Ordering::Relaxed);
            drop(slot);
            return self.styles(held);
        }
        *slot = Some(job);
        drop(slot);
        self.styles_cached()
    }

    /// The last styles drawn for the layer, whatever its pixels and
    /// settings since.
    pub fn styles_cached(&self) -> Option<Arc<Styles>> {
        if !self.has_pixels() || !self.effects.rendered() {
            return None;
        }
        let cache = self.styles_cache.borrow();
        cache.as_ref().map(|c| Arc::clone(&c.styles))
    }
}

#[cfg(test)]
mod tests {
    use crate::document::Document;
    use image::{Rgba, RgbaImage};

    /// The styles of a large layer are drawn in the background: none at
    /// first, then the same as drawn at once; after a change, the former
    /// ones until the new ones are ready.
    #[test]
    fn large_styles_are_drawn_in_the_background() {
        let (w, h) = (1100, 1000);
        let mut doc = Document::new(RgbaImage::new(w, h), None);
        let square = RgbaImage::from_fn(w, h, |x, y| {
            let inside = (300..700).contains(&x) && (200..600).contains(&y);
            Rgba(if inside { [0, 0, 255, 255] } else { [0; 4] })
        });
        doc.add_layer("s".into(), square, (0, 0));
        let fx = &mut doc.active_layer_mut().effects;
        fx.stroke.enabled = true;
        fx.stroke.width = 3.0;
        let ready = |doc: &Document| {
            let start = std::time::Instant::now();
            loop {
                let layer = doc.active_layer();
                if let Some(s) = layer.styles_later(false)
                    && layer
                        .styles_cached()
                        .is_some_and(|c| c.version == s.version)
                {
                    let direct = layer.clone();
                    *direct.styles_cache.borrow_mut() = None;
                    if direct.styles(false).is_some_and(|d| {
                        d.exterior.len() == s.exterior.len()
                            && d.exterior
                                .iter()
                                .zip(&s.exterior)
                                .all(|(a, b)| *a.coverage == *b.coverage)
                    }) {
                        return s;
                    }
                }
                assert!(start.elapsed().as_secs() < 60, "styles never came");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        assert!(doc.active_layer().styles_later(false).is_none(), "none yet");
        let first = ready(&doc);
        doc.active_layer_mut()
            .pixels_mut()
            .put_pixel(100, 100, Rgba([255, 0, 0, 255]));
        let meanwhile = doc.active_layer().styles_later(false).expect("the former");
        assert_eq!(meanwhile.version, first.version);
        let next = ready(&doc);
        assert_ne!(next.version, first.version);
    }
}
