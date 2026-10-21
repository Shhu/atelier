//! Layer masks: grayscale images that hide a layer where they are black and
//! show it where they are white, with their own position and a value for
//! everything outside them. The painting tools and the pixel operations
//! edit a targeted mask through an RGBA stand-in of it (see
//! [`Document::with_target`]).

use std::cell::RefCell;
use std::sync::Arc;

use image::{GrayImage, Luma, RgbaImage};

use super::smart::{target_mask, target_mask_mut};
use super::{Document, Layer, LayerKind, PixelRect, intersect, next_rev};
use crate::i18n::t;
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
}

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
        let m = self.margin();
        let (w, h) = (
            (self.image.width() as i64 + 2 * m) as usize,
            (self.image.height() as i64 + 2 * m) as usize,
        );
        let mut field = vec![self.default as f32; w * h];
        for (px, py, p) in self.image.enumerate_pixels() {
            field[(py as usize + m as usize) * w + px as usize + m as usize] = p.0[0] as f32;
        }
        blur(&mut field, w, h, self.feather);
        let image = GrayImage::from_fn(w as u32, h as u32, |px, py| {
            Luma([field[py as usize * w + px as usize]
                .round()
                .clamp(0.0, 255.0) as u8])
        });
        let image = Arc::new(image);
        *self.feathered.borrow_mut() = Some(Feathered {
            rev: key.0,
            feather: key.1,
            image: Arc::clone(&image),
            margin: m,
        });
        (image, (x - m, y - m))
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
        1.0 - self.density.clamp(0.0, 1.0) * (1.0 - v as f32 / 255.0)
    }

    /// `image` (a layer at `layer_offset`) with its opacity multiplied by
    /// the mask.
    pub fn apply_to(&self, image: &RgbaImage, layer_offset: (i64, i64)) -> RgbaImage {
        let mut out = image.clone();
        for (x, y, p) in out.enumerate_pixels_mut() {
            if p.0[3] > 0 {
                let v = self.value_at(
                    layer_offset.0 + x as i64,
                    layer_offset.1 + y as i64,
                    layer_offset,
                );
                p.0[3] = (p.0[3] as f32 * v).round() as u8;
            }
        }
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
        self.replace(image, (u.x, u.y), layer_offset);
    }
}

impl Document {
    /// Whether the painting tools and pixel operations edit the active
    /// layer's mask: chosen by clicking its thumbnail; always for a layer
    /// without pixels.
    pub fn mask_targeted(&self) -> bool {
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
        self.checkpoint(t!("xchg-hist-add-mask"));
        let layer = &mut self.layers[self.active];
        let origin = mask.origin((0, 0));
        let mut mask = mask;
        mask.set_origin(origin, layer.offset);
        layer.mask = Some(mask);
        self.mask_target = Some(layer.id);
        true
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
        let gray = GrayImage::from_fn(canvas.w, canvas.h, |x, y| {
            let v = mask.value_at(x as i64, y as i64, layer.offset);
            Luma([(v * 255.0).round() as u8])
        });
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
        if self.mask_targeted() {
            return Some(std::borrow::Cow::Owned(stand_in(
                &self.layers[self.active],
                self.full_rect(),
                self.filter_mask_target,
            )));
        }
        let layer = &self.layers[self.active];
        layer
            .has_pixels()
            .then_some(std::borrow::Cow::Borrowed(layer))
    }

    fn swap_in_mask(&mut self) {
        if self.swapped.is_some() {
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
    let image = RgbaImage::from_fn(r.w, r.h, |px, py| {
        let (cx, cy) = (r.x + px as i64, r.y + py as i64);
        let v = if own.contains(cx, cy) {
            mask.image
                .get_pixel((cx - own.x) as u32, (cy - own.y) as u32)
                .0[0]
        } else {
            mask.default
        };
        image::Rgba([v, v, v, 255])
    });
    Layer {
        kind: LayerKind::Pixels,
        image: Arc::new(image),
        rev: next_rev(),
        bounds: std::cell::Cell::new(None),
        styles_cache: RefCell::new(None),
        offset: (r.x, r.y),
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
        let image = mask.pixels_mut();
        for cy in r.y..r.bottom() {
            for cx in r.x..r.right() {
                let (lx, ly) = ((cx - p.x) as u32, (cy - p.y) as u32);
                image.put_pixel(lx, ly, Luma([gray(proxy.image.get_pixel(lx, ly).0)]));
            }
        }
    } else {
        return;
    }
    if filter {
        real.remix_filter_mask(area);
    }
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

    fn doc() -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba([255, 0, 0, 255])), None);
        doc.add_layer(
            "top".into(),
            RgbaImage::from_pixel(10, 10, Rgba([0, 0, 255, 255])),
            (0, 0),
        );
        doc
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
}
