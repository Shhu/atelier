//! Layer masks: grayscale images that hide a layer where they are black and
//! show it where they are white, with their own position and a value for
//! everything outside them. The painting tools and the pixel operations
//! edit a targeted mask through an RGBA stand-in of it (see
//! [`Document::with_target`]).

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use image::{GrayImage, Luma, RgbaImage};

use super::smart::{target_mask, target_mask_mut};
use super::{Document, Layer, LayerKind, PixelRect, intersect, next_rev};
use crate::i18n::t;
use crate::mask_ops::par_rows;
use crate::pixel_ops::blur;
use crate::selection::Selection;

/// Layer mask.
#[derive(Clone)]
pub struct Mask {
    image: Arc<GrayImage>,
    rev: u64,
    /// Top-left corner: relative to the layer when linked (it moves with
    /// it), in the canvas otherwise.
    offset: (i64, i64),
    linked: bool,
    /// Value everywhere outside the image (0: hidden, 255: shown).
    pub default: u8,
    /// Off (Shift + click on its thumbnail): the layer shows whole.
    pub enabled: bool,
    /// 0..1: how much the black areas hide (Properties panel).
    pub density: f32,
    /// Blur radius of the edges, in pixels (Properties panel).
    pub feather: f32,
    feathered: RefCell<Option<Feathered>>,
    /// The feathered image being made on a worker thread for the display
    /// (see [`Mask::shaped_later`]), shared by the mask's copies.
    feather_job: Arc<Mutex<Option<Arc<FeatherJob>>>>,
}

/// A feathered image being made: for which revision and radius (its
/// bits), and it once done.
struct FeatherJob {
    key: (u64, u32),
    done: Mutex<Option<Feathered>>,
    /// When it starts.
    asked: std::time::Instant,
}

/// A new feathered image is made at most this often while the radius
/// keeps changing (its slider): each one shown recomposites the layer.
const FEATHER_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// Feathered images being made.
static FEATHERING: AtomicUsize = AtomicUsize::new(0);

/// Whether feathered masks are being made for the display (it then looks
/// again soon).
pub(super) fn feathers_pending() -> bool {
    FEATHERING.load(Ordering::Relaxed) > 0
}

/// Below this many mask pixels, the display feathers at once.
const FEATHER_AT_ONCE: u64 = 1 << 20;

/// The feathered image for a revision and feather radius (its bits), and
/// the margin it adds on each side.
#[derive(Clone)]
struct Feathered {
    rev: u64,
    feather: u32,
    image: Arc<GrayImage>,
    margin: i64,
}

/// How a new mask starts (Layer › Layer Mask).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewMask {
    RevealAll,
    HideAll,
    RevealSelection,
    HideSelection,
}

impl Mask {
    /// Mask whose image's top-left corner is at `origin` in the canvas,
    /// linked to a layer placed at `layer_offset`.
    pub fn new(
        image: GrayImage,
        origin: (i64, i64),
        default: u8,
        layer_offset: (i64, i64),
    ) -> Self {
        Self {
            image: Arc::new(image),
            rev: next_rev(),
            offset: (origin.0 - layer_offset.0, origin.1 - layer_offset.1),
            linked: true,
            default,
            enabled: true,
            density: 1.0,
            feather: 0.0,
            feathered: RefCell::new(None),
            feather_job: Default::default(),
        }
    }

    pub fn image(&self) -> &GrayImage {
        &self.image
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn linked(&self) -> bool {
        self.linked
    }

    /// Links (or unlinks) the mask to its layer at `layer_offset`, in place.
    pub fn set_linked(&mut self, linked: bool, layer_offset: (i64, i64)) {
        let origin = self.origin(layer_offset);
        self.linked = linked;
        self.set_origin(origin, layer_offset);
    }

    /// Canvas position of the image's top-left corner.
    pub fn origin(&self, layer_offset: (i64, i64)) -> (i64, i64) {
        if self.linked {
            (
                self.offset.0 + layer_offset.0,
                self.offset.1 + layer_offset.1,
            )
        } else {
            self.offset
        }
    }

    pub fn set_origin(&mut self, origin: (i64, i64), layer_offset: (i64, i64)) {
        self.offset = if self.linked {
            (origin.0 - layer_offset.0, origin.1 - layer_offset.1)
        } else {
            origin
        };
    }

    /// Box of the image in the canvas, its feathered edge included.
    pub fn rect(&self, layer_offset: (i64, i64)) -> PixelRect {
        let (x, y) = self.origin(layer_offset);
        let m = self.margin();
        PixelRect {
            x: x - m,
            y: y - m,
            w: self.image.width() + 2 * m as u32,
            h: self.image.height() + 2 * m as u32,
        }
    }

    fn margin(&self) -> i64 {
        if self.feather > 0.0 {
            (self.feather * 2.0).ceil() as i64 + 1
        } else {
            0
        }
    }

    /// Mutable pixels (copied if shared with the history).
    pub fn pixels_mut(&mut self) -> &mut GrayImage {
        self.rev = next_rev();
        Arc::make_mut(&mut self.image)
    }

    /// New pixels, with their top-left corner at `origin` in the canvas.
    pub fn replace(&mut self, image: GrayImage, origin: (i64, i64), layer_offset: (i64, i64)) {
        self.rev = next_rev();
        self.image = Arc::new(image);
        self.set_origin(origin, layer_offset);
    }

    /// `values` (over `area`, canvas) replace the image's there; the image
    /// grows to cover `area` (the default around what it held), and keeps
    /// what lies outside it.
    pub fn put_over(&mut self, values: &GrayImage, area: PixelRect, layer_offset: (i64, i64)) {
        let (ox, oy) = self.origin(layer_offset);
        let (iw, ih) = (self.image.width() as i64, self.image.height() as i64);
        let (x0, y0) = (ox.min(area.x), oy.min(area.y));
        let (x1, y1) = ((ox + iw).max(area.right()), (oy + ih).max(area.bottom()));
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        let mut out = if (x0, y0, w, h) == (ox, oy, iw as u32, ih as u32) {
            (*self.image).clone()
        } else {
            let mut out = GrayImage::from_pixel(w, h, image::Luma([self.default]));
            image::imageops::replace(&mut out, &*self.image, ox - x0, oy - y0);
            out
        };
        image::imageops::replace(&mut out, values, area.x - x0, area.y - y0);
        self.replace(out, (x0, y0), layer_offset);
    }

    /// The image as the compositing reads it (feathered), and the canvas
    /// position of its top-left corner. Density is applied when reading.
    pub fn shaped(&self, layer_offset: (i64, i64)) -> (Arc<GrayImage>, (i64, i64)) {
        let (x, y) = self.origin(layer_offset);
        if self.feather <= 0.0 || self.image.width() == 0 || self.image.height() == 0 {
            return (Arc::clone(&self.image), (x, y));
        }
        let key = (self.rev, self.feather.to_bits());
        if let Some(f) = self.feathered.borrow().as_ref()
            && (f.rev, f.feather) == key
        {
            return (Arc::clone(&f.image), (x - f.margin, y - f.margin));
        }
        let f = feathered(&self.image, self.default, self.feather, self.rev);
        let (image, m) = (Arc::clone(&f.image), f.margin);
        *self.feathered.borrow_mut() = Some(f);
        (image, (x - m, y - m))
    }

    /// [`Self::shaped`] for the display: when only the feather radius
    /// changed (its slider) on a large mask, the new image is made on a
    /// worker thread and the former one shows meanwhile. Also gives what
    /// the image shown was made from (revision, radius bits).
    pub fn shaped_later(
        &self,
        layer_offset: (i64, i64),
    ) -> (Arc<GrayImage>, (i64, i64), (u64, u32)) {
        let key = (self.rev, self.feather.to_bits());
        let (w, h) = self.image.dimensions();
        let at_once = || {
            let (image, origin) = self.shaped(layer_offset);
            (image, origin, key)
        };
        if self.feather <= 0.0 || u64::from(w) * u64::from(h) <= FEATHER_AT_ONCE {
            return at_once();
        }
        let (x, y) = self.origin(layer_offset);
        let shown = |f: &Feathered| {
            let image = Arc::clone(&f.image);
            (image, (x - f.margin, y - f.margin), (f.rev, f.feather))
        };
        if let Some(f) = self.feathered.borrow().as_ref()
            && (f.rev, f.feather) == key
        {
            return shown(f);
        }
        let mut slot = self.feather_job.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = slot.as_ref() {
            let done = job.done.lock().unwrap_or_else(|e| e.into_inner()).clone();
            match done {
                Some(f) if f.rev == self.rev => {
                    let out = shown(&f);
                    *self.feathered.borrow_mut() = Some(f);
                    if job.key == key {
                        return out;
                    }
                }
                Some(_) => {}
                None => {
                    // Still feathering (for this radius or another one).
                    if let Some(f) = self
                        .feathered
                        .borrow()
                        .as_ref()
                        .filter(|f| f.rev == self.rev)
                    {
                        return shown(f);
                    }
                }
            }
        }
        // The former image of these pixels shows meanwhile; without one
        // (other pixels), feathered at once.
        let Some(former) = self
            .feathered
            .borrow()
            .as_ref()
            .filter(|f| f.rev == self.rev)
            .map(shown)
        else {
            drop(slot);
            return at_once();
        };
        // Soon after the former one: it waits a little (the radius shown
        // changes about ten times a second at most).
        let wait = slot.as_ref().map_or(std::time::Duration::ZERO, |j| {
            FEATHER_EVERY.saturating_sub(j.asked.elapsed())
        });
        let job = Arc::new(FeatherJob {
            key,
            done: Mutex::new(None),
            asked: std::time::Instant::now() + wait,
        });
        let (image, default, feather, rev) = (
            Arc::clone(&self.image),
            self.default,
            self.feather,
            self.rev,
        );
        let work = Arc::clone(&job);
        FEATHERING.fetch_add(1, Ordering::Relaxed);
        let spawned = std::thread::Builder::new()
            .name("feather".into())
            .spawn(move || {
                /// The count goes down even if feathering fails.
                struct Running;
                impl Drop for Running {
                    fn drop(&mut self) {
                        FEATHERING.fetch_sub(1, Ordering::Relaxed);
                    }
                }
                let _running = Running;
                std::thread::sleep(wait);
                let f = feathered(&image, default, feather, rev);
                *work.done.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
            });
        if spawned.is_err() {
            FEATHERING.fetch_sub(1, Ordering::Relaxed);
            drop(slot);
            return at_once();
        }
        *slot = Some(job);
        former
    }

    /// The pixels of `area` (image coordinates) just changed, from revision
    /// `before`: a feathered image of `before` is redone around them only
    /// (painting a feathered mask). Exact: the blur reaches no further.
    fn refeather(&self, before: u64, area: PixelRect) {
        let mut cache = self.feathered.borrow_mut();
        let m = self.margin();
        let (iw, ih) = (self.image.width() as i64, self.image.height() as i64);
        let Some(f) = cache.as_mut().filter(|f| {
            (f.rev, f.feather, f.margin) == (before, self.feather.to_bits(), m)
                && f.image.dimensions() == ((iw + 2 * m) as u32, (ih + 2 * m) as u32)
        }) else {
            return;
        };
        // Reach of the blur: three box passes (see `blur`).
        let reach = 3 * (self.feather / 1.7).round() as i64;
        let (fw, fh) = (iw + 2 * m, ih + 2 * m);
        // In the feathered image: what changes, and what that reads.
        let (x0, y0) = ((area.x + m - reach).max(0), (area.y + m - reach).max(0));
        let (x1, y1) = (
            (area.right() + m + reach).min(fw),
            (area.bottom() + m + reach).min(fh),
        );
        if x0 >= x1 || y0 >= y1 {
            f.rev = self.rev;
            return;
        }
        let (rx, ry) = (x0 - reach, y0 - reach);
        let (rw, rh) = (
            (x1 - x0 + 2 * reach) as usize,
            (y1 - y0 + 2 * reach) as usize,
        );
        let d = self.default as f32;
        let raw = self.image.as_raw();
        let mut field = vec![0.0f32; rw * rh];
        for (j, line) in field.chunks_exact_mut(rw).enumerate() {
            let py = ry + j as i64 - m;
            if !(0..ih).contains(&py) {
                continue;
            }
            for (i, v) in line.iter_mut().enumerate() {
                let px = rx + i as i64 - m;
                if (0..iw).contains(&px) {
                    *v = raw[(py * iw + px) as usize] as f32 - d;
                }
            }
        }
        blur(&mut field, rw, rh, self.feather);
        let image = Arc::make_mut(&mut f.image);
        let out = image.as_mut();
        for y in y0..y1 {
            let from = ((y - ry) as usize * rw) + (x0 - rx) as usize;
            let part = to_gray(&field[from..from + (x1 - x0) as usize], d);
            let at = (y * fw + x0) as usize;
            out[at..at + part.len()].copy_from_slice(&part);
        }
        f.rev = self.rev;
    }

    /// Fingerprint of what the mask does (cache keys).
    pub fn key(&self, layer_offset: (i64, i64)) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (
            self.rev,
            self.origin(layer_offset),
            self.default,
            self.enabled,
            self.density.to_bits(),
            self.feather.to_bits(),
        )
            .hash(&mut h);
        h.finish()
    }

    /// Effect (0..1, density applied) at canvas pixel (x, y), from the
    /// feathered image.
    pub fn value_at(&self, x: i64, y: i64, layer_offset: (i64, i64)) -> f32 {
        let (image, (ox, oy)) = self.shaped(layer_offset);
        let (lx, ly) = (x - ox, y - oy);
        let v = if lx >= 0 && ly >= 0 && lx < image.width() as i64 && ly < image.height() as i64 {
            image.get_pixel(lx as u32, ly as u32).0[0]
        } else {
            self.default
        };
        self.effects()[v as usize]
    }

    /// Effect (0..1) of each mask value, density applied.
    fn effects(&self) -> [f32; 256] {
        let density = self.density.clamp(0.0, 1.0);
        std::array::from_fn(|v| 1.0 - density * (1.0 - v as f32 / 255.0))
    }

    /// The feathered values over `area` (canvas), row by row: the default
    /// outside the image.
    fn values_over(&self, area: PixelRect, layer_offset: (i64, i64)) -> Vec<u8> {
        let (image, origin) = self.shaped(layer_offset);
        self.placed_over(&image, origin, area)
    }

    /// The values of the image as stored (neither feathered nor faded by
    /// the density) over `area` (canvas): the default outside the image.
    pub fn raw_over(&self, area: PixelRect, layer_offset: (i64, i64)) -> GrayImage {
        let values = self.placed_over(&self.image, self.origin(layer_offset), area);
        GrayImage::from_raw(area.w, area.h, values).expect("size")
    }

    /// `image` placed at `(ox, oy)` over `area`, row by row.
    fn placed_over(&self, image: &GrayImage, (ox, oy): (i64, i64), area: PixelRect) -> Vec<u8> {
        let (w, h) = (area.w as usize, area.h as usize);
        let mut out = vec![self.default; w * h];
        let (iw, ih) = (image.width() as i64, image.height() as i64);
        // Columns of `area` over the image.
        let (x0, x1) = ((ox - area.x).max(0), (ox + iw - area.x).min(area.w as i64));
        if w == 0 || x0 >= x1 {
            return out;
        }
        let raw = image.as_raw();
        par_rows(&mut out, w, |first, band| {
            for (j, row) in band.chunks_exact_mut(w).enumerate() {
                let ly = area.y + (first + j) as i64 - oy;
                if (0..ih).contains(&ly) {
                    let from = (ly * iw + area.x + x0 - ox) as usize;
                    let len = (x1 - x0) as usize;
                    row[x0 as usize..x1 as usize].copy_from_slice(&raw[from..from + len]);
                }
            }
        });
        out
    }

    /// `image` (a layer at `layer_offset`) with its opacity multiplied by
    /// the mask.
    pub fn apply_to(&self, image: &RgbaImage, layer_offset: (i64, i64)) -> RgbaImage {
        let mut out = image.clone();
        let area = PixelRect {
            x: layer_offset.0,
            y: layer_offset.1,
            w: image.width(),
            h: image.height(),
        };
        let values = self.values_over(area, layer_offset);
        let effects = self.effects();
        let w = image.width() as usize;
        par_rows(out.as_mut(), w.max(1) * 4, |first, band| {
            for (p, &v) in band.chunks_exact_mut(4).zip(&values[first * w..]) {
                if p[3] > 0 {
                    p[3] = (p[3] as f32 * effects[v as usize]).round() as u8;
                }
            }
        });
        out
    }

    /// The image as opaque gray RGBA, to go through the pixel transforms.
    fn to_rgba(&self) -> RgbaImage {
        RgbaImage::from_fn(self.image.width(), self.image.height(), |x, y| {
            let v = self.image.get_pixel(x, y).0[0];
            image::Rgba([v, v, v, 255])
        })
    }

    /// Back from [`Self::to_rgba`] after a transform: where it became
    /// transparent (the corners of a rotation), the default value.
    fn set_from_rgba(&mut self, image: &RgbaImage, origin: (i64, i64), layer_offset: (i64, i64)) {
        let d = self.default as u32;
        let gray = GrayImage::from_fn(image.width(), image.height(), |x, y| {
            let [v, _, _, a] = image.get_pixel(x, y).0;
            let a = a as u32;
            Luma([((v as u32 * a + d * (255 - a) + 127) / 255) as u8])
        });
        self.replace(gray, origin, layer_offset);
    }

    /// Moves the image (unlinked masks: with the canvas operations).
    pub(super) fn shift(&mut self, dx: i64, dy: i64) {
        self.offset = (self.offset.0 + dx, self.offset.1 + dy);
    }

    /// Mirrored within the canvas `w` × `h` (Flip Canvas).
    pub(super) fn flip(&mut self, horizontal: bool, (w, h): (i64, i64), layer_offset: (i64, i64)) {
        let (x, y) = self.origin(layer_offset);
        let (iw, ih) = (self.image.width() as i64, self.image.height() as i64);
        let (image, origin) = if horizontal {
            (
                image::imageops::flip_horizontal(&*self.image),
                (w - x - iw, y),
            )
        } else {
            (
                image::imageops::flip_vertical(&*self.image),
                (x, h - y - ih),
            )
        };
        self.replace(image, origin, layer_offset);
    }

    /// Scaled by (`sx`, `sy`) from the canvas origin (Image Size).
    pub(super) fn scale(
        &mut self,
        (sx, sy): (f64, f64),
        filter: image::imageops::FilterType,
        layer_offset: (i64, i64),
    ) {
        let (x, y) = self.origin(layer_offset);
        let (w, h) = self.image.dimensions();
        let x0 = (x as f64 * sx).round() as i64;
        let y0 = (y as f64 * sy).round() as i64;
        let x1 = ((x + w as i64) as f64 * sx).round() as i64;
        let y1 = ((y + h as i64) as f64 * sy).round() as i64;
        self.feather *= (sx * sy).sqrt() as f32;
        if w == 0 || h == 0 {
            return self.set_origin((x0, y0), layer_offset);
        }
        let (nw, nh) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
        let image = image::imageops::resize(&*self.image, nw, nh, filter);
        self.replace(image, (x0, y0), layer_offset);
    }

    /// Turned by `degrees` (clockwise) around `center` (canvas), placed
    /// by `turn` (where a point relative to `center` goes, relative to
    /// `new_center`): Image Rotation, and rotating a layer.
    pub(super) fn rotate(
        &mut self,
        degrees: f32,
        (center, new_center): ((f32, f32), (f32, f32)),
        turn: impl Fn(f32, f32) -> (f32, f32),
        nearest: bool,
        layer_offset: (i64, i64),
    ) {
        let (x, y) = self.origin(layer_offset);
        let (w, h) = (self.image.width() as f32, self.image.height() as f32);
        if w == 0.0 || h == 0.0 {
            return;
        }
        let own = (x as f32 + w / 2.0, y as f32 + h / 2.0);
        let (dx, dy) = turn(own.0 - center.0, own.1 - center.1);
        let image = crate::rotate::rotate(&self.to_rgba(), degrees, nearest);
        let (iw, ih) = (image.width() as f32, image.height() as f32);
        let origin = (
            (new_center.0 + dx - iw / 2.0).round() as i64,
            (new_center.1 + dy - ih / 2.0).round() as i64,
        );
        self.set_from_rgba(&image, origin, layer_offset);
    }

    /// Resized from `from` to `to` (canvas boxes), the way a transform
    /// moves a layer's pixels.
    pub(super) fn map(
        &mut self,
        from: PixelRect,
        to: (i64, i64, u32, u32),
        filter: image::imageops::FilterType,
        layer_offset: (i64, i64),
    ) {
        let (x, y) = self.origin(layer_offset);
        let (w, h) = self.image.dimensions();
        let (sx, sy) = (to.2 as f64 / from.w as f64, to.3 as f64 / from.h as f64);
        let map_x = |v: i64| to.0 + ((v - from.x) as f64 * sx).round() as i64;
        let map_y = |v: i64| to.1 + ((v - from.y) as f64 * sy).round() as i64;
        let (x0, y0) = (map_x(x), map_y(y));
        if w == 0 || h == 0 {
            return self.set_origin((x0, y0), layer_offset);
        }
        let (x1, y1) = (map_x(x + w as i64), map_y(y + h as i64));
        let (nw, nh) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
        let image = if (nw, nh) == (w, h) {
            (*self.image).clone()
        } else {
            image::imageops::resize(&*self.image, nw, nh, filter)
        };
        self.replace(image, (x0, y0), layer_offset);
    }

    /// The image enlarged (with its default value) to cover `area` (canvas).
    fn grown_to(&mut self, area: PixelRect, layer_offset: (i64, i64)) {
        let (x, y) = self.origin(layer_offset);
        let own = PixelRect {
            x,
            y,
            w: self.image.width(),
            h: self.image.height(),
        };
        let u = if own.w == 0 || own.h == 0 {
            area
        } else {
            own.union(area)
        };
        if u == own {
            return;
        }
        let mut image = GrayImage::from_pixel(u.w, u.h, Luma([self.default]));
        image::imageops::replace(&mut image, &*self.image, own.x - u.x, own.y - u.y);
        // Its feathered image grows the same way: around the old image,
        // the default value only adds more of the default.
        let key = (self.rev, self.feather.to_bits());
        let feathered = self
            .feathered
            .borrow()
            .clone()
            .filter(|f| (f.rev, f.feather) == key);
        let empty = own.w == 0 || own.h == 0;
        self.replace(image, (u.x, u.y), layer_offset);
        let m = self.margin();
        let old = match feathered {
            _ if self.feather <= 0.0 => return,
            _ if empty => None,
            Some(f) if f.margin == m => Some(f),
            _ => return,
        };
        let size = (u.w + 2 * m as u32, u.h + 2 * m as u32);
        let mut grown = GrayImage::from_pixel(size.0, size.1, Luma([self.default]));
        if let Some(f) = old {
            image::imageops::replace(&mut grown, &*f.image, own.x - u.x, own.y - u.y);
        }
        *self.feathered.borrow_mut() = Some(Feathered {
            rev: self.rev,
            feather: key.1,
            image: Arc::new(grown),
            margin: m,
        });
    }
}

impl Document {
    /// Whether the painting tools and pixel operations edit the active
    /// layer's mask: chosen by clicking its thumbnail; always for a layer
    /// without pixels.
    pub fn mask_targeted(&self) -> bool {
        // In Quick Mask Mode, everything edits the quick mask.
        if self.quick_mask.is_some() {
            return true;
        }
        let layer = self.real_active();
        if self.filter_mask_target {
            return self.mask_target == Some(layer.id) && target_mask(layer, true).is_some();
        }
        layer.mask.is_some() && (!layer.has_pixels() || self.mask_target == Some(layer.id))
    }

    /// Targets the active layer's mask (`true`) or its pixels.
    pub fn target_mask(&mut self, on: bool) {
        self.mask_target = on.then(|| self.real_active().id);
        self.filter_mask_target = false;
    }

    /// Whether the painting tools have something to edit: pixels (not a
    /// smart object's) or a mask.
    pub fn can_paint(&self) -> bool {
        self.real_active().paintable() || self.mask_targeted()
    }

    /// The active layer, even while its mask stands in for it.
    pub(super) fn real_active(&self) -> &Layer {
        self.swapped.as_deref().unwrap_or(&self.layers[self.active])
    }

    /// Adds a mask to the active layer (Layer › Layer Mask, or the button
    /// of the Layers panel), which then becomes the target. Refused if it
    /// has one.
    pub fn add_mask(&mut self, kind: NewMask, selection: Option<&Selection>) -> bool {
        if self.layers[self.active].mask.is_some() {
            return false;
        }
        self.checkpoint(t!("xchg-hist-add-mask"));
        self.put_mask(kind, selection);
        true
    }

    /// Gives the active layer a new mask of `kind`, targeted, within the
    /// current step.
    pub(super) fn put_mask(&mut self, kind: NewMask, selection: Option<&Selection>) {
        let canvas = self.full_rect();
        let mask = match (kind, selection) {
            (NewMask::RevealAll, _) | (NewMask::HideSelection, None) => {
                Mask::new(GrayImage::new(0, 0), (0, 0), 255, (0, 0))
            }
            (NewMask::HideAll, _) | (NewMask::RevealSelection, None) => {
                Mask::new(GrayImage::new(0, 0), (0, 0), 0, (0, 0))
            }
            (NewMask::RevealSelection | NewMask::HideSelection, Some(sel)) => {
                let hide = kind == NewMask::HideSelection;
                let mut image = sel.to_mask(canvas);
                if hide {
                    image.pixels_mut().for_each(|p| p.0[0] = 255 - p.0[0]);
                }
                // Outside the canvas: like the selection there.
                let outside = if sel.inverted != hide { 255 } else { 0 };
                Mask::new(image, (canvas.x, canvas.y), outside, (0, 0))
            }
        };
        let layer = &mut self.layers[self.active];
        let origin = mask.origin((0, 0));
        let mut mask = mask;
        mask.set_origin(origin, layer.offset);
        layer.mask = Some(mask);
        self.mask_target = Some(layer.id);
    }

    /// Removes the active layer's mask.
    pub fn delete_mask(&mut self) -> bool {
        if self.layers[self.active].mask.is_none() {
            return false;
        }
        self.checkpoint(t!("xchg-hist-delete-mask"));
        self.layers[self.active].mask = None;
        true
    }

    /// Applies the mask to the pixels (they lose the opacity it hid), and
    /// removes it. Only for pixel layers.
    pub fn apply_mask(&mut self) -> bool {
        let layer = &self.layers[self.active];
        let Some(mask) = layer.mask.as_ref().filter(|_| layer.has_pixels()) else {
            return false;
        };
        let image = match mask.enabled {
            true => mask.apply_to(layer.image(), layer.offset),
            false => layer.image().clone(),
        };
        self.checkpoint(t!("xchg-hist-apply-mask"));
        let layer = &mut self.layers[self.active];
        let text = layer.text.take();
        layer.replace_pixels(image);
        // Only the opacity changed where the mask showed everything.
        layer.text = text.filter(|_| mask_was_white(layer.mask.as_ref()));
        layer.mask = None;
        true
    }

    /// Turns the active layer's mask on or off (Shift + click on it).
    pub fn set_mask_enabled(&mut self, on: bool) -> bool {
        match &self.layers[self.active].mask {
            Some(m) if m.enabled != on => {
                self.checkpoint(if on {
                    t!("core-mask-enable")
                } else {
                    t!("core-mask-disable")
                });
                if let Some(m) = &mut self.layers[self.active].mask {
                    m.enabled = on;
                }
                true
            }
            _ => false,
        }
    }

    /// Links or unlinks the active layer and its mask (the chain between
    /// their thumbnails).
    pub fn set_mask_linked(&mut self, linked: bool) -> bool {
        let layer = &self.layers[self.active];
        if layer.mask.as_ref().is_none_or(|m| m.linked == linked) {
            return false;
        }
        self.checkpoint(if linked {
            t!("xchg-hist-link-mask")
        } else {
            t!("xchg-hist-unlink-mask")
        });
        let layer = &mut self.layers[self.active];
        let offset = layer.offset;
        if let Some(m) = &mut layer.mask {
            m.set_linked(linked, offset);
        }
        true
    }

    /// Inverts the active layer's mask (Properties panel, or Ctrl+I with
    /// the mask targeted).
    pub fn invert_mask(&mut self) -> bool {
        if self.layers[self.active].mask.is_none() {
            return false;
        }
        self.checkpoint(t!("xchg-hist-invert-mask"));
        if let Some(m) = &mut self.layers[self.active].mask {
            m.default = 255 - m.default;
            m.pixels_mut()
                .pixels_mut()
                .for_each(|p| p.0[0] = 255 - p.0[0]);
        }
        true
    }

    /// Selection from layer `i`'s mask (Ctrl + click on its thumbnail):
    /// what it shows, within the canvas.
    pub fn mask_selection(&self, i: usize) -> Option<Selection> {
        let layer = self.layers.get(i)?;
        let mask = layer.mask.as_ref()?;
        let canvas = self.full_rect();
        let effects = mask.effects();
        let values = mask.values_over(canvas, layer.offset);
        let raw = values
            .iter()
            .map(|&v| (effects[v as usize] * 255.0).round() as u8)
            .collect();
        let gray = GrayImage::from_raw(canvas.w, canvas.h, raw).expect("size");
        Selection::from_mask(&gray, (canvas.x, canvas.y))
    }

    // ------------------------------------------------------- mask editing

    /// Before painting (brush, eraser…): the target is made ready. A mask
    /// is enlarged to the canvas and stands in for the layer as RGBA pixels
    /// (gray, opaque) until [`Self::end_paint`]. Returns `false` if there
    /// is nothing to paint on (a group or an adjustment without a mask).
    pub fn begin_paint(&mut self) -> bool {
        if self.mask_targeted() {
            self.swap_in_mask();
            return true;
        }
        if !self.layers[self.active].paintable() {
            return false;
        }
        self.expand_active_to_canvas();
        true
    }

    /// The painting is over: the mask gets back its place.
    pub fn end_paint(&mut self) {
        self.swap_out_mask();
    }

    /// Copies area `area` (canvas) of the painted stand-in into the mask
    /// (all of it with `None`), so the display follows the brush.
    pub fn sync_paint(&mut self, area: Option<PixelRect>) {
        if self.quick_mask.is_some() {
            return self.sync_quick_mask(area);
        }
        let Some(real) = self.swapped.as_deref_mut() else {
            return;
        };
        write_back(
            &self.layers[self.active],
            real,
            area,
            self.filter_mask_target,
        );
    }

    /// The layers as they are composited: a mask being painted is shown
    /// as a mask, not as its stand-in.
    pub fn render_layers(&self) -> std::borrow::Cow<'_, [Layer]> {
        match &self.swapped {
            None => std::borrow::Cow::Borrowed(&self.layers),
            Some(real) => {
                let mut layers = self.layers.clone();
                layers[self.active] = (**real).clone();
                std::borrow::Cow::Owned(layers)
            }
        }
    }

    /// Layer `i` as composited (see [`Self::render_layers`]), without
    /// copying the others.
    pub fn render_layer(&self, i: usize) -> &Layer {
        match &self.swapped {
            Some(real) if i == self.active => real,
            _ => &self.layers[i],
        }
    }

    /// Runs `f`, an operation on the active layer's pixels, on its mask
    /// instead when the mask is targeted. Without pixels or a targeted mask,
    /// `None`.
    pub(super) fn with_target<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        if self.swapped.is_some() {
            return Some(f(self));
        }
        if self.mask_targeted() {
            self.swap_in_mask();
            let out = f(self);
            self.swap_out_mask();
            return Some(out);
        }
        self.layers[self.active].paintable().then(|| f(self))
    }

    /// The active layer as the pixel operations see it: its pixels, or its
    /// mask's stand-in (see [`Self::with_target`]); `None` without either.
    pub fn target_layer(&self) -> Option<std::borrow::Cow<'_, Layer>> {
        if self.swapped.is_some() {
            return Some(std::borrow::Cow::Borrowed(&self.layers[self.active]));
        }
        if let Some(proxy) = self.quick_mask_stand_in() {
            return Some(std::borrow::Cow::Owned(proxy));
        }
        if self.mask_targeted() {
            return Some(std::borrow::Cow::Owned(self.cached_stand_in()));
        }
        let layer = &self.layers[self.active];
        layer
            .has_pixels()
            .then_some(std::borrow::Cow::Borrowed(layer))
    }

    /// [`stand_in`] of the targeted mask, made again only when the mask
    /// (or the canvas) changed: a dialog asks for it at every frame.
    fn cached_stand_in(&self) -> Layer {
        let layer = &self.layers[self.active];
        let (area, filter) = (self.full_rect(), self.filter_mask_target);
        let mask = target_mask(layer, filter).expect("a mask");
        let key = (
            layer.id,
            mask.rev(),
            mask.origin(layer.offset),
            mask.image.dimensions(),
            mask.default,
            filter,
            (area.x, area.y, area.w, area.h),
        );
        let mut cache = self.stand_in.borrow_mut();
        if let Some(c) = cache.as_ref().filter(|c| c.key == key) {
            return Layer {
                image: Arc::clone(&c.image),
                rev: c.rev,
                offset: c.offset,
                ..stand_in_shell(layer)
            };
        }
        let made = stand_in(layer, area, filter);
        *cache = Some(StandIn {
            key,
            image: Arc::clone(&made.image),
            rev: made.rev,
            offset: made.offset,
        });
        made
    }

    fn swap_in_mask(&mut self) {
        if self.swapped.is_some() {
            return;
        }
        if let Some(proxy) = self.quick_mask_stand_in() {
            let layer = &mut self.layers[self.active];
            self.swapped = Some(Box::new(std::mem::replace(layer, proxy)));
            return;
        }
        let canvas = self.full_rect();
        let layer = &mut self.layers[self.active];
        let offset = layer.offset;
        let filter = self.filter_mask_target;
        if let Some(m) = target_mask_mut(layer, filter) {
            m.grown_to(canvas, offset);
        }
        let proxy = stand_in(layer, canvas, filter);
        self.swapped = Some(Box::new(std::mem::replace(layer, proxy)));
    }

    fn swap_out_mask(&mut self) {
        if self.quick_mask.is_some() && self.swapped.is_some() {
            self.sync_quick_mask(None);
            if let Some(real) = self.swapped.take() {
                self.layers[self.active] = *real;
            }
            return;
        }
        let Some(mut real) = self.swapped.take() else {
            return;
        };
        let slot = &mut self.layers[self.active];
        // The operation may have put back an earlier state (nothing changed).
        if slot.mask.is_some() || slot.id != real.id {
            return;
        }
        write_back(slot, &mut real, None, self.filter_mask_target);
        *slot = *real;
    }
}

/// Whether a mask (before applying it) showed the whole layer.
fn mask_was_white(mask: Option<&Mask>) -> bool {
    mask.is_some_and(|m| m.default == 255 && m.image.pixels().all(|p| p.0[0] == 255))
}

/// The last stand-in made (see `Document::cached_stand_in`): what it was
/// made from, its pixels and their revision.
pub(super) struct StandIn {
    key: StandInKey,
    image: Arc<RgbaImage>,
    rev: u64,
    offset: (i64, i64),
}

/// Layer id, mask revision, origin, size and default, smart filter mask,
/// canvas.
type StandInKey = (
    u64,
    u64,
    (i64, i64),
    (u32, u32),
    u8,
    bool,
    (i64, i64, u32, u32),
);

/// RGBA stand-in of `layer`'s mask (its smart filter mask with `filter`)
/// over `area` (canvas): the gray value in the colors, opaque. Same id and
/// settings as the layer, without mask or styles.
fn stand_in(layer: &Layer, area: PixelRect, filter: bool) -> Layer {
    let mask = target_mask(layer, filter).expect("a mask");
    let (x, y) = mask.origin(layer.offset);
    let own = PixelRect {
        x,
        y,
        w: mask.image.width(),
        h: mask.image.height(),
    };
    let r = if own.w == 0 || own.h == 0 { area } else { own };
    let (w, d) = (r.w as usize, mask.default);
    let mut raw = vec![255u8; w * r.h as usize * 4];
    let src = mask.image.as_raw();
    par_rows(&mut raw, w.max(1) * 4, |first, band| {
        for (j, row) in band.chunks_exact_mut(w * 4).enumerate() {
            let cy = r.y + (first + j) as i64;
            let inside = (own.y..own.bottom()).contains(&cy) && own.w > 0;
            for (i, p) in row.chunks_exact_mut(4).enumerate() {
                let cx = r.x + i as i64;
                let v = if inside && (own.x..own.right()).contains(&cx) {
                    src[((cy - own.y) * own.w as i64 + cx - own.x) as usize]
                } else {
                    d
                };
                p[..3].fill(v);
            }
        }
    });
    let image = RgbaImage::from_raw(r.w, r.h, raw).expect("size");
    Layer {
        image: Arc::new(image),
        rev: next_rev(),
        offset: (r.x, r.y),
        ..stand_in_shell(layer)
    }
}

/// A stand-in's settings: `layer`'s, as plain pixels without mask or
/// styles (its pixels to set).
fn stand_in_shell(layer: &Layer) -> Layer {
    Layer {
        kind: LayerKind::Pixels,
        bounds: std::cell::Cell::new(None),
        styles_cache: RefCell::new(None),
        styles_job: Default::default(),
        effects: Default::default(),
        mask: None,
        text: None,
        shape: None,
        artboard: None,
        smart: None,
        ..layer.clone()
    }
}

/// Copies area `area` (canvas; all with `None`) of the stand-in `proxy`
/// into `real`'s mask (its smart filter mask with `filter`, the filters
/// then shown through it): the gray of its colors; erased (transparent)
/// pixels hide.
fn write_back(proxy: &Layer, real: &mut Layer, area: Option<PixelRect>, filter: bool) {
    let offset = real.offset;
    let Some(mask) = target_mask_mut(real, filter) else {
        return;
    };
    let (x, y) = mask.origin(offset);
    let p = proxy.rect();
    let mut area = area;
    if (p.w, p.h) != mask.image.dimensions() || (p.x, p.y) != (x, y) {
        // Resized or moved stand-in (whole-image operations): replaced.
        let image = GrayImage::from_fn(p.w, p.h, |px, py| {
            Luma([gray(proxy.image.get_pixel(px, py).0)])
        });
        mask.replace(image, (p.x, p.y), offset);
        area = None;
    } else if let Some(r) = intersect(area.unwrap_or(p), p) {
        // Only where the stand-in differs: nothing changes (no new
        // revision) when nothing was painted.
        let (pw, mw) = (p.w as usize, mask.image.width() as usize);
        let (src, own) = (proxy.image.as_raw(), mask.image.as_raw());
        let (x0, x1) = ((r.x - p.x) as usize, (r.right() - p.x) as usize);
        // Per row: the columns that changed.
        let scan = |part: std::ops::Range<usize>| -> Vec<Option<(usize, usize)>> {
            part.map(|j| {
                let ly = (r.y - p.y) as usize + j;
                let new = |lx: usize| gray(src[(ly * pw + lx) * 4..][..4].try_into().expect("4"));
                let differs = |lx: &usize| new(*lx) != own[ly * mw + lx];
                let first = (x0..x1).find(differs)?;
                let last = (x0..x1).rev().find(differs)?;
                Some((first, last + 1))
            })
            .collect()
        };
        let rows = if r.w as usize * r.h as usize >= 1 << 16 {
            crate::mask_ops::par_collect(r.h as usize, scan)
        } else {
            scan(0..r.h as usize)
        };
        let changed = rows
            .iter()
            .enumerate()
            .filter_map(|(j, c)| c.map(|c| (j, c)));
        let Some(local) = changed
            .map(|(j, (a, b))| PixelRect {
                x: a as i64,
                y: (r.y - p.y) + j as i64,
                w: (b - a) as u32,
                h: 1,
            })
            .reduce(PixelRect::union)
        else {
            return;
        };
        let before = mask.rev();
        let image = mask.pixels_mut();
        let (lx0, lx1) = (local.x as usize, local.right() as usize);
        let out = &mut image.as_mut()[local.y as usize * mw..local.bottom() as usize * mw];
        par_rows(out, mw, |first, band| {
            for (j, row) in band.chunks_exact_mut(mw).enumerate() {
                let ly = local.y as usize + first + j;
                for lx in lx0..lx1 {
                    row[lx] = gray(src[(ly * pw + lx) * 4..][..4].try_into().expect("4"));
                }
            }
        });
        mask.refeather(before, local);
        area = Some(PixelRect {
            x: x + local.x,
            y: y + local.y,
            ..local
        });
    } else {
        return;
    }
    if filter {
        real.remix_filter_mask(area);
    }
}

/// Values blurred as a difference from the default `d`, as mask values.
/// `image` (whose surroundings are `default`) blurred by `feather`, with
/// the margin it adds on each side. Only the box of its pixels that differ
/// from the default (and its margin) is blurred: elsewhere the blur adds
/// nothing to the default.
fn feathered(image: &GrayImage, default: u8, feather: f32, rev: u64) -> Feathered {
    let m = if feather > 0.0 {
        (feather * 2.0).ceil() as i64 + 1
    } else {
        0
    };
    let (iw, ih, mu) = (image.width() as usize, image.height() as usize, m as usize);
    let (w, h) = (iw + 2 * mu, ih + 2 * mu);
    let raw = image.as_raw();
    let mut out = vec![default; w * h];
    // Box (image coordinates) of the values that differ from the default.
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (y, row) in raw.chunks_exact(iw.max(1)).enumerate() {
        let Some(left) = row.iter().position(|&v| v != default) else {
            continue;
        };
        let right = row.iter().rposition(|&v| v != default).unwrap_or(left);
        (x0, x1) = (x0.min(left), x1.max(right + 1));
        (y0, y1) = (y0.min(y), y + 1);
    }
    if x0 < x1 {
        // That box and its margin, in the feathered image.
        let (rw, rh) = (x1 - x0 + 2 * mu, y1 - y0 + 2 * mu);
        // Blurred as a difference from the default value: everything
        // around the image (its margin, and beyond) counts as the default.
        let d = default as f32;
        let mut field = vec![0.0f32; rw * rh];
        par_rows(&mut field, rw, |first, band| {
            for (i, line) in band.chunks_exact_mut(rw).enumerate() {
                let Some(py) = (first + i).checked_sub(mu).filter(|&j| j < y1 - y0) else {
                    continue;
                };
                let row = &raw[(y0 + py) * iw + x0..][..x1 - x0];
                for (f, &v) in line[mu..mu + (x1 - x0)].iter_mut().zip(row) {
                    *f = v as f32 - d;
                }
            }
        });
        blur(&mut field, rw, rh, feather);
        let gray = to_gray(&field, d);
        for (j, part) in gray.chunks_exact(rw).enumerate() {
            let at = (y0 + j) * w + x0;
            out[at..at + rw].copy_from_slice(part);
        }
    }
    Feathered {
        rev,
        feather: feather.to_bits(),
        image: Arc::new(GrayImage::from_raw(w as u32, h as u32, out).expect("size")),
        margin: m,
    }
}

fn to_gray(field: &[f32], d: f32) -> Vec<u8> {
    const ROW: usize = 4096;
    let mut out = vec![0u8; field.len()];
    par_rows(&mut out, ROW, |first, band| {
        for (o, v) in band.iter_mut().zip(&field[first * ROW..]) {
            *o = (v + d).round().clamp(0.0, 255.0) as u8;
        }
    });
    out
}

/// Mask value of a stand-in pixel.
fn gray([r, g, b, a]: [u8; 4]) -> u8 {
    let luma = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
    ((luma * a as u32 + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// Feathering blurs only around the values that differ from the
    /// default, with the same result as blurring all of the image.
    #[test]
    fn feathering_blurs_only_around_what_differs() {
        let mut image = GrayImage::from_pixel(300, 200, Luma([255]));
        for (x, y) in [(40, 30), (41, 30), (200, 150), (299, 199)] {
            image.put_pixel(x, y, Luma([0]));
        }
        for x in 100..140 {
            image.put_pixel(x, 90, Luma([60]));
        }
        let feather = 6.5;
        let f = feathered(&image, 255, feather, 1);
        // All of the image, blurred.
        let m = f.margin as usize;
        let (w, h) = (300 + 2 * m, 200 + 2 * m);
        let mut field = vec![0.0f32; w * h];
        for (x, y, p) in image.enumerate_pixels() {
            field[(y as usize + m) * w + x as usize + m] = p.0[0] as f32 - 255.0;
        }
        blur(&mut field, w, h, feather);
        let whole = to_gray(&field, 255.0);
        assert_eq!(f.image.dimensions(), (w as u32, h as u32));
        let worst = f
            .image
            .as_raw()
            .iter()
            .zip(&whole)
            .map(|(a, b)| a.abs_diff(*b))
            .max();
        assert!(worst <= Some(1), "{worst:?}");
        // Nothing but the default: nothing to blur.
        let plain = feathered(&GrayImage::from_pixel(50, 40, Luma([0])), 0, feather, 1);
        assert!(plain.image.pixels().all(|p| p.0[0] == 0));
    }

    /// The display's feathered image of a large mask whose radius changed
    /// is made on a worker thread: the former one meanwhile, then the same
    /// as made at once.
    #[test]
    fn a_new_feather_radius_shows_once_ready() {
        let mut image = GrayImage::from_pixel(1100, 1000, Luma([0]));
        for y in 300..700 {
            for x in 200..900 {
                image.put_pixel(x, y, Luma([255]));
            }
        }
        let mut m = Mask::new(image, (0, 0), 0, (0, 0));
        m.feather = 4.0;
        let (_, _, first) = m.shaped_later((0, 0));
        assert_eq!(first, (m.rev(), 4.0f32.to_bits()), "made at once at first");
        m.feather = 9.0;
        let (_, at, meanwhile) = m.shaped_later((0, 0));
        assert_eq!(meanwhile, first, "the former one meanwhile");
        assert_eq!(at, (-9, -9), "in its place");
        let start = std::time::Instant::now();
        let (image, at) = loop {
            let (image, at, key) = m.shaped_later((0, 0));
            if key.1 == 9.0f32.to_bits() {
                break (image, at);
            }
            assert!(start.elapsed().as_secs() < 60, "never ready");
            std::thread::sleep(std::time::Duration::from_millis(2));
        };
        let fresh = m.clone();
        *fresh.feathered.borrow_mut() = None;
        let (direct, direct_at) = fresh.shaped((0, 0));
        assert_eq!((at, *image == *direct), (direct_at, true));
    }

    /// While the radius keeps changing (its slider), a new feathered image
    /// shows at most every `FEATHER_EVERY`: each one recomposites the layer.
    #[test]
    fn feather_radius_changes_show_at_most_ten_times_a_second() {
        let mut m = Mask::new(
            GrayImage::from_pixel(1100, 1000, Luma([255])),
            (0, 0),
            0,
            (0, 0),
        );
        m.feather = 2.0;
        m.shaped_later((0, 0));
        let start = std::time::Instant::now();
        let mut shown = Vec::new();
        for k in 0..400 {
            m.feather = 3.0 + (k % 7) as f32;
            let (_, _, key) = m.shaped_later((0, 0));
            if shown.last() != Some(&key.1) {
                shown.push(key.1);
            }
            if start.elapsed() > std::time::Duration::from_millis(350) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // The first one, then one new radius per 100 ms at most.
        assert!(shown.len() <= 5, "{} images in 350 ms", shown.len());
    }

    fn doc() -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255])), None);
        doc.add_layer(
            "top".into(),
            RgbaImage::from_pixel(10, 10, Rgba([0, 0, 255, 255])),
            (0, 0),
        );
        doc
    }

    /// The stand-in of a targeted mask is made once while the mask doesn't
    /// change (dialogs ask for it at every frame), again when it does.
    #[test]
    fn mask_stand_in_is_made_again_only_when_the_mask_changes() {
        let mut doc = doc();
        assert!(doc.add_mask(NewMask::HideAll, None));
        let a = doc.target_layer().unwrap().into_owned();
        let b = doc.target_layer().unwrap().into_owned();
        assert!(Arc::ptr_eq(&a.image, &b.image));
        assert_eq!(a.rev, b.rev);
        assert_eq!(a.image.get_pixel(5, 5).0, [0, 0, 0, 255]);
        assert!(doc.begin_paint());
        doc.active_layer_mut()
            .pixels_mut()
            .put_pixel(5, 5, Rgba([255, 255, 255, 255]));
        doc.end_paint();
        let c = doc.target_layer().unwrap().into_owned();
        assert!(!Arc::ptr_eq(&a.image, &c.image));
        assert_ne!(a.rev, c.rev);
        assert_eq!(c.image.get_pixel(5, 5).0, [255, 255, 255, 255]);
    }

    #[test]
    fn masks_hide_and_reveal() {
        let mut doc = doc();
        assert!(doc.add_mask(NewMask::HideAll, None));
        assert!(doc.mask_targeted());
        assert_eq!(doc.flatten().get_pixel(5, 5).0, [255, 0, 0, 255]);
        // Painting white in the mask shows the layer there.
        assert!(doc.begin_paint());
        doc.active_layer_mut()
            .pixels_mut()
            .put_pixel(5, 5, Rgba([255, 255, 255, 255]));
        doc.sync_paint(None);
        assert_eq!(
            doc.render_layers()[1]
                .mask
                .as_ref()
                .unwrap()
                .image()
                .get_pixel(5, 5)
                .0,
            [255]
        );
        doc.end_paint();
        assert!(doc.active_layer().mask.is_some());
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(5, 5).0, [0, 0, 255, 255]);
        assert_eq!(flat.get_pixel(4, 5).0, [255, 0, 0, 255]);
        // Disabled: the layer shows whole.
        assert!(doc.set_mask_enabled(false));
        assert_eq!(doc.flatten().get_pixel(4, 5).0, [0, 0, 255, 255]);
        assert!(doc.undo());
        // Applying it: the pixels lose what it hid.
        assert!(doc.apply_mask());
        assert!(doc.active_layer().mask.is_none());
        assert_eq!(doc.active_layer().alpha_at(4, 5), 0);
        assert_eq!(doc.active_layer().alpha_at(5, 5), 255);
    }

    #[test]
    fn mask_operations_and_selection() {
        let mut doc = doc();
        let sel: Selection = PixelRect {
            x: 2,
            y: 2,
            w: 3,
            h: 3,
        }
        .into();
        assert!(doc.add_mask(NewMask::RevealSelection, Some(&sel)));
        assert!(!doc.add_mask(NewMask::RevealAll, None));
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(3, 3).0, [0, 0, 255, 255]);
        assert_eq!(flat.get_pixel(8, 8).0, [255, 0, 0, 255]);
        // Filling the mask (it is targeted): gray of the color.
        assert!(doc.fill_active([255, 255, 255, 255], None, 1.0, false));
        assert_eq!(doc.flatten().get_pixel(8, 8).0, [0, 0, 255, 255]);
        // Deleting with a selection: hides.
        assert!(doc.clear_region(&sel));
        assert_eq!(doc.flatten().get_pixel(3, 3).0, [255, 0, 0, 255]);
        let back = doc.mask_selection(1).unwrap();
        assert_eq!(back.coverage(3, 3), 0);
        assert_eq!(back.coverage(8, 8), 255);
        // Targeting the pixels: the fill goes to the layer.
        doc.target_mask(false);
        assert!(doc.fill_active([0, 255, 0, 255], None, 1.0, false));
        assert_eq!(doc.active_layer().pixel_at(0, 0), Some([0, 255, 0, 255]));
        // Unlinked, the mask stays when the layer moves.
        doc.target_mask(true);
        assert!(doc.set_mask_linked(false));
        doc.active_layer_mut().offset = (1, 0);
        let layer = doc.active_layer();
        assert_eq!(layer.mask.as_ref().unwrap().origin(layer.offset), (0, 0));
        assert!(doc.delete_mask());
        assert!(!doc.mask_targeted());
    }

    #[test]
    fn density_and_feather_soften_the_mask() {
        let mut doc = doc();
        doc.add_mask(NewMask::HideAll, None);
        let layer = doc.active_layer_mut();
        layer.mask.as_mut().unwrap().density = 0.5;
        let half = doc.flatten().get_pixel(5, 5).0;
        assert!(
            half[0].abs_diff(128) <= 1 && half[2].abs_diff(128) <= 1,
            "{half:?}"
        );
        let sel: Selection = PixelRect {
            x: 0,
            y: 0,
            w: 5,
            h: 10,
        }
        .into();
        let mut doc = self::doc();
        doc.add_mask(NewMask::RevealSelection, Some(&sel));
        doc.active_layer_mut().mask.as_mut().unwrap().feather = 2.0;
        let flat = doc.flatten();
        let edge = flat.get_pixel(5, 5).0;
        assert!(edge[0] > 30 && edge[2] > 30, "{edge:?}");
        assert!(flat.get_pixel(2, 5).0[2] > flat.get_pixel(7, 5).0[2]);
    }

    /// A feathered mask from scratch (no cache), to compare with.
    fn fresh(m: &Mask) -> Arc<GrayImage> {
        let copy = m.clone();
        *copy.feathered.borrow_mut() = None;
        copy.shaped((0, 0)).0
    }

    fn noisy(w: u32, h: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| Luma([((x * 37 + y * 91) % 256) as u8]))
    }

    /// Painting a feathered mask redoes its edges around the dabs only,
    /// exactly as feathering it all again.
    #[test]
    fn feathered_masks_follow_the_brush_exactly() {
        for default in [0, 255] {
            let mut m = Mask::new(noisy(80, 60), (0, 0), default, (0, 0));
            m.feather = 4.0;
            let _ = m.shaped((0, 0));
            for area in [(10, 10, 12, 7), (0, 0, 5, 5), (70, 50, 10, 10)] {
                let before = m.rev();
                let (x, y, w, h) = area;
                for py in y..y + h {
                    for px in x..x + w {
                        m.pixels_mut().put_pixel(px, py, Luma([200]));
                    }
                }
                let r = PixelRect {
                    x: x as i64,
                    y: y as i64,
                    w,
                    h,
                };
                m.refeather(before, r);
                let (kept, _) = m.shaped((0, 0));
                assert!(*kept == *fresh(&m), "default {default}, area {area:?}");
            }
            // Grown to a larger area: the same as feathered again.
            let canvas = PixelRect {
                x: -20,
                y: -10,
                w: 140,
                h: 90,
            };
            m.grown_to(canvas, (0, 0));
            let (kept, origin) = m.shaped((0, 0));
            assert_eq!(origin, (-20 - m.margin(), -10 - m.margin()));
            assert!(*kept == *fresh(&m), "grown, default {default}");
        }
    }

    /// Painting a feathered mask through the stand-in: the display reads
    /// the feathered image kept up to date; a stroke that changes nothing
    /// leaves the mask's revision as it was.
    #[test]
    fn painting_a_feathered_mask() {
        let mut doc = doc();
        doc.add_mask(NewMask::RevealAll, None);
        doc.active_layer_mut().mask.as_mut().unwrap().feather = 2.0;
        let _ = doc.flatten();
        assert!(doc.begin_paint());
        let rev = doc.render_layers()[1].mask.as_ref().unwrap().rev();
        doc.sync_paint(None);
        assert_eq!(doc.render_layers()[1].mask.as_ref().unwrap().rev(), rev);
        doc.active_layer_mut()
            .pixels_mut()
            .put_pixel(5, 5, Rgba([0, 0, 0, 255]));
        doc.sync_paint(Some(PixelRect {
            x: 5,
            y: 5,
            w: 1,
            h: 1,
        }));
        let layers = doc.render_layers();
        let m = layers[1].mask.as_ref().unwrap();
        assert_ne!(m.rev(), rev);
        assert!(*m.shaped((0, 0)).0 == *fresh(m));
        drop(layers);
        doc.end_paint();
        let m = doc.active_layer().mask.as_ref().unwrap();
        assert!(*m.shaped((0, 0)).0 == *fresh(m));
        assert!(doc.flatten().get_pixel(5, 5).0[0] > 0, "the dab shows");
    }

    /// Applying a mask and making a selection of it read the feathered
    /// image as before (once, not per pixel).
    #[test]
    fn mask_values_over_areas() {
        let mut m = Mask::new(noisy(20, 10), (3, 2), 255, (0, 0));
        m.feather = 1.5;
        m.density = 0.6;
        let image = RgbaImage::from_pixel(30, 20, Rgba([10, 20, 30, 200]));
        let applied = m.apply_to(&image, (-2, -1));
        for (x, y, p) in applied.enumerate_pixels() {
            let v = m.value_at(x as i64 - 2, y as i64 - 1, (-2, -1));
            assert_eq!(p.0[3], (200.0 * v).round() as u8, "{x}, {y}");
        }
    }
}
