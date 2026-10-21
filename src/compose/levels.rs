//! Images of the layers at the scale they are composited: whole for the
//! export, reduced 2, 4, 8… times (box filter) for a zoomed-out view, so
//! the display only ever composites about as many pixels as the screen has.

use std::collections::HashMap;
use std::sync::Arc;

use image::{GrayImage, ImageBuffer, Luma, Pixel, Rgba, RgbaImage};

/// An image placed at `at`, in pixels of its level.
pub struct Src<P: Pixel> {
    pub image: Arc<ImageBuffer<P, Vec<u8>>>,
    pub at: (i64, i64),
}

impl<P: Pixel> Clone for Src<P> {
    fn clone(&self) -> Self {
        Self {
            image: Arc::clone(&self.image),
            at: self.at,
        }
    }
}

/// Which image of a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceKey {
    pub layer: u64,
    /// 0: pixels, 1: mask, 2…: styles.
    pub slot: u16,
}

/// Gives the images of the layers at a level.
pub trait Sources {
    /// Each pixel of the level covers 2^level × 2^level canvas pixels.
    fn level(&self) -> u32;
    /// `image`, whose top-left corner is at `at` in the canvas, at the
    /// level; `version` changes whenever its pixels do.
    fn rgba(
        &mut self,
        key: SourceKey,
        version: u64,
        image: &Arc<RgbaImage>,
        at: (i64, i64),
    ) -> Src<Rgba<u8>>;
    fn gray(
        &mut self,
        key: SourceKey,
        version: u64,
        image: &Arc<GrayImage>,
        at: (i64, i64),
    ) -> Src<Luma<u8>>;
}

/// The images as they are (level 0): export, flatten, merges.
pub struct FullSize;

impl Sources for FullSize {
    fn level(&self) -> u32 {
        0
    }

    fn rgba(
        &mut self,
        _: SourceKey,
        _: u64,
        image: &Arc<RgbaImage>,
        at: (i64, i64),
    ) -> Src<Rgba<u8>> {
        Src {
            image: Arc::clone(image),
            at,
        }
    }

    fn gray(
        &mut self,
        _: SourceKey,
        _: u64,
        image: &Arc<GrayImage>,
        at: (i64, i64),
    ) -> Src<Luma<u8>> {
        Src {
            image: Arc::clone(image),
            at,
        }
    }
}

/// Canvas area where the pixels and mask of a layer changed since the last
/// frame (brush stroke), to reduce only that part again.
#[derive(Clone, Copy, Debug)]
pub struct Hint {
    pub layer: u64,
    pub rect: crate::document::PixelRect,
}

enum Reduced {
    Rgba(Arc<RgbaImage>),
    Gray(Arc<GrayImage>),
}

/// A reduced image: version, canvas origin and size of its source.
struct Entry {
    version: u64,
    at: (i64, i64),
    size: (u32, u32),
    image: Reduced,
}

/// Reduced images of one level, kept from frame to frame.
pub struct LevelCache {
    level: u32,
    entries: HashMap<SourceKey, Entry>,
    hints: Vec<Hint>,
    /// Keys asked for since the last [`Self::retain_used`].
    used: std::collections::HashSet<SourceKey>,
}

impl LevelCache {
    pub fn new(level: u32) -> Self {
        Self {
            level,
            entries: HashMap::new(),
            hints: Vec::new(),
            used: Default::default(),
        }
    }

    /// Some pixels of a layer changed, only there (see [`Hint`]).
    pub fn hint(&mut self, hint: Hint) {
        self.hints.push(hint);
    }

    /// Forgets the images not asked for since the last call, and the hints.
    pub fn retain_used(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.entries.retain(|k, _| used.contains(k));
        self.hints.clear();
    }

    fn block(&self) -> u32 {
        1 << self.level
    }

    fn place(&self, at: (i64, i64)) -> (i64, i64) {
        let b = self.block() as i64;
        (at.0.div_euclid(b), at.1.div_euclid(b))
    }

    /// The blocks of a reduced image to redo, when a hint covers the change
    /// of a source that kept its place and size (pixels and masks only: the
    /// styles spread).
    fn partial(&self, key: SourceKey, at: (i64, i64), size: (u32, u32)) -> Option<[u32; 4]> {
        let e = self.entries.get(&key)?;
        if key.slot > 1 || e.at != at || e.size != size {
            return None;
        }
        let r = self
            .hints
            .iter()
            .filter(|h| h.layer == key.layer)
            .map(|h| h.rect)
            .reduce(crate::document::PixelRect::union)?;
        let own = crate::document::PixelRect {
            x: at.0,
            y: at.1,
            w: size.0,
            h: size.1,
        };
        let b = self.block();
        let (lw, lh) = (size.0.div_ceil(b), size.1.div_ceil(b));
        let Some(r) = crate::document::intersect(r, own) else {
            return Some([0, 0, 0, 0]);
        };
        let (x, y) = ((r.x - at.0) as u32, (r.y - at.1) as u32);
        let (x0, y0) = ((x / b).min(lw), (y / b).min(lh));
        let (x1, y1) = ((x + r.w).div_ceil(b).min(lw), (y + r.h).div_ceil(b).min(lh));
        Some([x0, y0, x1 - x0, y1 - y0])
    }
}

impl Sources for LevelCache {
    fn level(&self) -> u32 {
        self.level
    }

    fn rgba(
        &mut self,
        key: SourceKey,
        version: u64,
        image: &Arc<RgbaImage>,
        at: (i64, i64),
    ) -> Src<Rgba<u8>> {
        if self.level == 0 {
            return FullSize.rgba(key, version, image, at);
        }
        self.used.insert(key);
        let b = self.block();
        let placed = self.place(at);
        if let Some(Entry {
            version: v,
            image: Reduced::Rgba(cached),
            ..
        }) = self.entries.get(&key)
            && *v == version
        {
            return Src {
                image: Arc::clone(cached),
                at: placed,
            };
        }
        let size = image.dimensions();
        let partial = self.partial(key, at, size);
        let reduced = match (partial, self.entries.remove(&key)) {
            (
                Some(area),
                Some(Entry {
                    image: Reduced::Rgba(mut cached),
                    ..
                }),
            ) => {
                reduce_rgba_into(Arc::make_mut(&mut cached), image, b, area);
                cached
            }
            _ => Arc::new(reduce_rgba(image, b)),
        };
        let image = Reduced::Rgba(Arc::clone(&reduced));
        self.entries.insert(
            key,
            Entry {
                version,
                at,
                size,
                image,
            },
        );
        Src {
            image: reduced,
            at: placed,
        }
    }

    fn gray(
        &mut self,
        key: SourceKey,
        version: u64,
        image: &Arc<GrayImage>,
        at: (i64, i64),
    ) -> Src<Luma<u8>> {
        if self.level == 0 {
            return FullSize.gray(key, version, image, at);
        }
        self.used.insert(key);
        let b = self.block();
        let placed = self.place(at);
        if let Some(Entry {
            version: v,
            image: Reduced::Gray(cached),
            ..
        }) = self.entries.get(&key)
            && *v == version
        {
            return Src {
                image: Arc::clone(cached),
                at: placed,
            };
        }
        let size = image.dimensions();
        let partial = self.partial(key, at, size);
        let reduced = match (partial, self.entries.remove(&key)) {
            (
                Some(area),
                Some(Entry {
                    image: Reduced::Gray(mut cached),
                    ..
                }),
            ) => {
                reduce_gray_into(Arc::make_mut(&mut cached), image, b, area);
                cached
            }
            _ => Arc::new(reduce_gray(image, b)),
        };
        let image = Reduced::Gray(Arc::clone(&reduced));
        self.entries.insert(
            key,
            Entry {
                version,
                at,
                size,
                image,
            },
        );
        Src {
            image: reduced,
            at: placed,
        }
    }
}

/// `image` reduced `b` times: each pixel is the average of a block (in
/// premultiplied colors, so hidden colors don't bleed).
pub fn reduce_rgba(image: &RgbaImage, b: u32) -> RgbaImage {
    let (w, h) = (image.width().div_ceil(b), image.height().div_ceil(b));
    let mut out = RgbaImage::new(w, h);
    reduce_rgba_into(&mut out, image, b, [0, 0, w, h]);
    out
}

/// Recomputes area `[x, y, w, h]` (in pixels of `out`) of a reduction.
fn reduce_rgba_into(out: &mut RgbaImage, image: &RgbaImage, b: u32, [x, y, w, h]: [u32; 4]) {
    let (iw, ih) = image.dimensions();
    let (ow, raw) = (out.width() as usize, image.as_raw());
    let stride = iw as usize * 4;
    let rows = out
        .as_mut()
        .chunks_exact_mut(ow * 4)
        .skip(y as usize)
        .take(h as usize);
    let rows: Vec<(usize, &mut [u8])> =
        rows.enumerate().map(|(i, r)| (y as usize + i, r)).collect();
    parallel_rows(rows, |ly, row| {
        let ys = ly as u32 * b..((ly as u32 + 1) * b).min(ih);
        for lx in x..x + w {
            let xs = lx * b..((lx + 1) * b).min(iw);
            let mut sum = [0u32; 4];
            for sy in ys.clone() {
                let line = &raw[sy as usize * stride..];
                for sx in xs.clone() {
                    let p = &line[sx as usize * 4..][..4];
                    let a = p[3] as u32;
                    sum[0] += p[0] as u32 * a;
                    sum[1] += p[1] as u32 * a;
                    sum[2] += p[2] as u32 * a;
                    sum[3] += a;
                }
            }
            let n = (ys.len() * xs.len()).max(1) as u32;
            let px = &mut row[lx as usize * 4..][..4];
            for c in 0..3 {
                px[c] = (sum[c] + sum[3] / 2).checked_div(sum[3]).unwrap_or(0) as u8;
            }
            px[3] = ((sum[3] + n / 2) / n) as u8;
        }
    });
}

pub fn reduce_gray(image: &GrayImage, b: u32) -> GrayImage {
    let (w, h) = (image.width().div_ceil(b), image.height().div_ceil(b));
    let mut out = GrayImage::new(w, h);
    reduce_gray_into(&mut out, image, b, [0, 0, w, h]);
    out
}

fn reduce_gray_into(out: &mut GrayImage, image: &GrayImage, b: u32, [x, y, w, h]: [u32; 4]) {
    let (iw, ih) = image.dimensions();
    let (ow, raw) = (out.width() as usize, image.as_raw());
    let rows = out
        .as_mut()
        .chunks_exact_mut(ow.max(1))
        .skip(y as usize)
        .take(h as usize);
    let rows: Vec<(usize, &mut [u8])> =
        rows.enumerate().map(|(i, r)| (y as usize + i, r)).collect();
    parallel_rows(rows, |ly, row| {
        let ys = ly as u32 * b..((ly as u32 + 1) * b).min(ih);
        for lx in x..x + w {
            let xs = lx * b..((lx + 1) * b).min(iw);
            let mut sum = 0u32;
            for sy in ys.clone() {
                let line = &raw[sy as usize * iw as usize..];
                sum += xs.clone().map(|sx| line[sx as usize] as u32).sum::<u32>();
            }
            let n = (ys.len() * xs.len()).max(1) as u32;
            row[lx as usize] = ((sum + n / 2) / n) as u8;
        }
    });
}

/// Runs `f` on each (index, row), spread over the cores when there are many.
fn parallel_rows(rows: Vec<(usize, &mut [u8])>, f: impl Fn(usize, &mut [u8]) + Sync) {
    let total: usize = rows.iter().map(|(_, r)| r.len()).sum();
    if total < 1 << 16 {
        for (i, row) in rows {
            f(i, row);
        }
        return;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let chunk = rows.len().div_ceil(threads).max(1);
    let f = &f;
    let mut rows = rows;
    std::thread::scope(|s| {
        while !rows.is_empty() {
            let rest = rows.split_off(chunk.min(rows.len()));
            let part = std::mem::replace(&mut rows, rest);
            s.spawn(move || {
                for (i, row) in part {
                    f(i, row);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reductions_average_blocks_and_follow_partial_changes() {
        let mut image = RgbaImage::from_pixel(5, 3, Rgba([255, 0, 0, 255]));
        image.put_pixel(0, 0, Rgba([0, 0, 255, 0]));
        let r = reduce_rgba(&image, 2);
        assert_eq!(r.dimensions(), (3, 2));
        // Transparent pixels don't bring their color.
        assert_eq!(r.get_pixel(0, 0).0, [255, 0, 0, 191]);
        assert_eq!(r.get_pixel(2, 1).0, [255, 0, 0, 255]);

        let mut cache = LevelCache::new(1);
        let key = SourceKey { layer: 1, slot: 0 };
        let shared = Arc::new(image.clone());
        let first = cache.rgba(key, 1, &shared, (3, -1));
        assert_eq!(first.at, (1, -1));
        // The same version: the same image.
        let again = cache.rgba(key, 1, &shared, (3, -1));
        assert!(Arc::ptr_eq(&first.image, &again.image));
        drop((first, again));
        // A stroke at (4, 2) of the image: only that block is redone.
        image.put_pixel(4, 2, Rgba([0, 255, 0, 255]));
        cache.hint(Hint {
            layer: 1,
            rect: crate::document::PixelRect {
                x: 7,
                y: 1,
                w: 1,
                h: 1,
            },
        });
        let next = cache.rgba(key, 2, &Arc::new(image.clone()), (3, -1));
        assert_eq!(next.image.get_pixel(2, 1).0, [0, 255, 0, 255]);
        assert_eq!(*next.image, reduce_rgba(&image, 2));
        let gray = reduce_gray(&GrayImage::from_raw(2, 1, vec![0, 255]).unwrap(), 2);
        assert_eq!(gray.get_pixel(0, 0).0, [128]);
    }
}
