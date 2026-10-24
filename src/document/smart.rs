//! Smart objects: layers showing a document of their own (layers
//! converted, or a placed file, embedded or linked) through a transform,
//! then through their smart filters. Their pixels are that rendering, made
//! again from the contents whenever one of them changes: scaled down then
//! up, a smart object loses nothing. The painting tools and the pixel
//! operations leave them alone (Rasterize first), like Photoshop's.

use std::ops::Range;
use std::time::SystemTime;

use super::*;
use crate::compose::blend::{atop, dissolve};
use crate::filters::{Filter, par_rows};
use crate::liquify::{self, Mesh};
use crate::mask_ops::Affine;
use crate::retouch::Progress;
use crate::xform::{self, Bounds, Grid, Homography, Map, Pt, Source, Transform, Warp};

/// What a smart object shows: a document without history.
#[derive(Clone)]
pub struct Contents {
    /// Shared by the instances of a smart object (Duplicate Layer): Edit
    /// Contents updates them all.
    pub id: u64,
    /// Its file's name (placed), or the layer's it was made from.
    pub name: String,
    layers: Vec<Layer>,
    active: usize,
    canvas: (u32, u32),
    guides: Vec<Guide>,
    channels: Vec<Channel>,
    paths: Vec<SavedPath>,
    work_path: Option<VectorPath>,
    icc_profile: Option<Arc<Vec<u8>>>,
    /// Its composite: what the smart object transforms.
    flat: Arc<RgbaImage>,
}

impl Contents {
    /// Canvas size.
    pub fn size(&self) -> (u32, u32) {
        self.canvas
    }

    fn bounds(&self) -> Bounds {
        [0.0, 0.0, self.canvas.0 as f64, self.canvas.1 as f64]
    }

    /// The same contents (instances share them; a copy shows the same).
    pub fn same_as(&self, other: &Contents) -> bool {
        Arc::ptr_eq(&self.flat, &other.flat)
            || (self.canvas == other.canvas
                && self.layers.len() == other.layers.len()
                && *self.flat == *other.flat)
    }
}

/// Place Linked: the file a smart object shows. Its contents are the last
/// copy read, shown while the file is missing.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub path: PathBuf,
    /// Modification time of the file when read: a newer one is read again.
    pub modified: Option<SystemTime>,
    /// Not found when last looked for.
    pub missing: bool,
}

impl Link {
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            modified: modified_time(path),
            missing: false,
        }
    }

    /// Its file is missing (found so, or not there now).
    pub fn gone(&self) -> bool {
        self.missing || !self.path.exists()
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What a smart filter does.
#[derive(Clone, Debug, PartialEq)]
pub enum SmartFilterKind {
    /// A filter of the Filter menu, with its settings.
    Filter(Filter),
    /// Liquify: its distortion, stretched to the rendering.
    Liquify(Arc<Mesh>),
    /// The Camera Raw Filter, with its settings.
    CameraRaw(Arc<crate::camera_raw::Settings>),
}

impl From<Filter> for SmartFilterKind {
    fn from(f: Filter) -> Self {
        Self::Filter(f)
    }
}

impl SmartFilterKind {
    /// Has settings to edit again (a dialog, Liquify's workspace).
    pub fn editable(&self) -> bool {
        match self {
            Self::Filter(f) => f.kind().has_settings(),
            Self::Liquify(_) | Self::CameraRaw(_) => true,
        }
    }

    /// Applies it to `image`, whose top-left pixel is at `origin`.
    fn apply(&self, image: &mut RgbaImage, origin: (i64, i64)) {
        match self {
            Self::Filter(f) => f.apply(image, origin),
            Self::Liquify(mesh) => {
                if let Some(out) = liquify::render(image, mesh, &Progress::default()) {
                    *image = out;
                }
            }
            Self::CameraRaw(settings) => *image = crate::camera_raw::apply(image, settings),
        }
    }
}

/// A smart filter, with its blending options.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartFilter {
    pub filter: SmartFilterKind,
    pub visible: bool,
    pub blend: BlendMode,
    pub opacity: f32,
}

impl SmartFilter {
    pub fn new(filter: impl Into<SmartFilterKind>) -> Self {
        Self {
            filter: filter.into(),
            visible: true,
            blend: BlendMode::Normal,
            opacity: 1.0,
        }
    }
}

/// A smart object.
#[derive(Clone)]
pub struct SmartObject {
    pub contents: Contents,
    pub link: Option<Link>,
    /// Where the contents go: from their box (0, 0)–(w, h) into the layer
    /// (its coordinates: it moves with the layer).
    pub map: Map,
    /// Resampling of the rendering (the Interpolation of the transform).
    pub interpolation: xform::Filter,
    /// Smart filters, applied from the first one (bottom of the list) up.
    pub filters: Vec<SmartFilter>,
    /// The eye of the Smart Filters row: off, none applies.
    pub filters_enabled: bool,
    /// Where the filters apply (white), linked to the layer.
    pub filter_mask: Option<Mask>,
    /// The rendering before the filters and after them (before their mask),
    /// aligned with the layer's image: to paint the mask.
    cache: Option<(Arc<RgbaImage>, Arc<RgbaImage>)>,
}

impl SmartObject {
    /// Its big buffers (see `Layer::buffers`): the contents' layers and
    /// composite, the renderings kept, the filter mask.
    pub(super) fn buffers(&self, out: &mut Vec<(usize, usize)>) {
        let c = &self.contents;
        for l in &c.layers {
            l.buffers(out);
        }
        let grays = c.channels.iter().map(|ch| ch.mask.as_raw());
        let grays = grays.chain(self.filter_mask.iter().map(|m| m.image().as_raw()));
        let rgba = std::iter::once(&c.flat).chain(self.cache.iter().flat_map(|(a, b)| [a, b]));
        for raw in grays.chain(rgba.map(|i| i.as_raw())) {
            out.push((raw.as_ptr() as usize, raw.len()));
        }
    }

    pub fn new(contents: Contents, map: Map) -> Self {
        Self {
            contents,
            link: None,
            map,
            interpolation: xform::Filter::Bicubic,
            filters: Vec::new(),
            filters_enabled: true,
            filter_mask: None,
            cache: None,
        }
    }

    /// Its contents' colors changed by `f`, layer by layer (Convert to
    /// Profile, Grayscale), in the profile `icc` if it changes: their
    /// composite is made again; the caller renders the object again.
    pub(super) fn recolor(&mut self, f: &mut dyn FnMut(&mut Layer), icc: Option<&Arc<Vec<u8>>>) {
        let c = &mut self.contents;
        c.layers.iter_mut().for_each(f);
        if let Some(icc) = icc {
            c.icc_profile = Some(Arc::clone(icc));
        }
        c.flat = Arc::new(Document::from_contents(c).flatten());
        self.cache = None;
    }

    /// Its transform, the layer being at `offset`: contents → canvas.
    pub fn transform(&self, offset: (i64, i64)) -> Transform {
        Transform {
            src: self.contents.bounds(),
            map: shift_map(self.map, [offset.0 as f64, offset.1 as f64]),
        }
    }

    /// The filters that apply, in order.
    fn applied(&self) -> Vec<SmartFilter> {
        match self.filters_enabled {
            true => self.filters.iter().filter(|f| f.visible).cloned().collect(),
            false => Vec::new(),
        }
    }

    fn job(&self, t: Transform, offset: (i64, i64), limit: PixelRect) -> RenderJob {
        let filters = self.applied();
        let mask = self
            .filter_mask
            .as_ref()
            .filter(|m| m.enabled && !filters.is_empty())
            .map(|m| MaskSample::of(m, offset));
        RenderJob {
            flat: Arc::clone(&self.contents.flat),
            t,
            interpolation: self.interpolation,
            filters,
            mask,
            moved_mask: None,
            limit,
        }
    }
}

/// `map` followed by a move of `d`.
fn shift_map(map: Map, d: Pt) -> Map {
    match map {
        Map::Projective(h) => {
            Map::Projective(h.then(&Homography::affine([1.0, 0.0, d[0], 0.0, 1.0, d[1]])))
        }
        Map::Warp(w) => Map::Warp(w.mapped(|p| [p[0] + d[0], p[1] + d[1]])),
    }
}

/// `inner` (contents → canvas) followed by `outer` (canvas → canvas): a
/// projective map if both are, else the warp through the same points at
/// the thirds of the contents (exact when the result is a warp).
pub(super) fn compose(inner: &Transform, outer: &Transform) -> Option<Map> {
    if let (Map::Projective(a), Map::Projective(b)) = (inner.map, outer.map) {
        return Some(Map::Projective(a.then(&b)));
    }
    let [x0, y0, x1, y1] = inner.src;
    let at =
        |u: f64, v: f64| outer.forward(inner.forward([x0 + (x1 - x0) * u, y0 + (y1 - y0) * v])?);
    fit_warp(at).map(Map::Warp)
}

/// The Bezier patch going through `f`'s points at the thirds of (u, v).
fn fit_warp(f: impl Fn(f64, f64) -> Option<Pt>) -> Option<Warp> {
    // Inverse of the cubic Bernstein matrix at 0, 1/3, 2/3 and 1.
    const INV: [[f64; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [-5.0 / 6.0, 3.0, -1.5, 1.0 / 3.0],
        [1.0 / 3.0, -1.5, 3.0, -5.0 / 6.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut s = [[[0.0; 2]; 4]; 4];
    for (j, row) in s.iter_mut().enumerate() {
        for (i, p) in row.iter_mut().enumerate() {
            *p = f(i as f64 / 3.0, j as f64 / 3.0)?;
        }
    }
    let mut out = [[[0.0; 2]; 4]; 4];
    for (j, row) in out.iter_mut().enumerate() {
        for (i, p) in row.iter_mut().enumerate() {
            for r in 0..4 {
                for c in 0..4 {
                    let w = INV[j][r] * INV[i][c];
                    p[0] += w * s[r][c][0];
                    p[1] += w * s[r][c][1];
                }
            }
        }
    }
    Some(Warp(out))
}

/// The whole-pixel move `map` is, if it is one.
fn whole_move(map: &Map) -> Option<(i64, i64)> {
    let Map::Projective(h) = map else { return None };
    let [a, b, c, d, e, f, g, hh, i] = h.0.map(|v| v / h.0[8]);
    let near = |v: f64, w: f64| (v - w).abs() < 1e-9;
    let whole = |v: f64| near(v, v.round());
    (near(a, 1.0) && near(b, 0.0) && near(d, 0.0) && near(e, 1.0))
        .then_some(())
        .filter(|_| near(g, 0.0) && near(hh, 0.0) && near(i, 1.0) && whole(c) && whole(f))
        .map(|_| (c.round() as i64, f.round() as i64))
}

/// A mask as the rendering reads it: feathered, in the canvas.
struct MaskSample {
    image: Arc<GrayImage>,
    origin: (i64, i64),
    default: u8,
    density: f32,
}

impl MaskSample {
    fn of(m: &Mask, offset: (i64, i64)) -> Self {
        let (image, origin) = m.shaped(offset);
        Self {
            image,
            origin,
            default: m.default,
            density: m.density,
        }
    }

    /// Coverage (0..255, density applied) at canvas pixel (x, y).
    fn at(&self, x: i64, y: i64) -> u8 {
        let (lx, ly) = (x - self.origin.0, y - self.origin.1);
        let (w, h) = self.image.dimensions();
        let v = if lx >= 0 && ly >= 0 && lx < w as i64 && ly < h as i64 {
            self.image.get_pixel(lx as u32, ly as u32).0[0]
        } else {
            self.default
        };
        let hidden = self.density.clamp(0.0, 1.0) * (255 - v) as f32;
        (255.0 - hidden).round() as u8
    }
}

/// The work of rendering a smart object, to run on any thread.
pub struct RenderJob {
    flat: Arc<RgbaImage>,
    /// Contents → canvas.
    t: Transform,
    interpolation: xform::Filter,
    filters: Vec<SmartFilter>,
    mask: Option<MaskSample>,
    /// The filter mask's new image and origin when it moves too.
    moved_mask: Option<(GrayImage, (i64, i64))>,
    limit: PixelRect,
}

/// A smart object's rendering (see [`Layer::put_smart`]).
pub struct SmartRender {
    image: RgbaImage,
    /// Canvas position of the image.
    origin: (i64, i64),
    /// Contents → canvas.
    map: Map,
    base: Arc<RgbaImage>,
    filtered: Arc<RgbaImage>,
    moved_mask: Option<(GrayImage, (i64, i64))>,
}

impl SmartRender {
    /// The pixels taken out (a preview's, shown), the rest kept to be put
    /// in later with [`Self::with_image`] (the preview applied).
    pub fn take_image(mut self) -> (RgbaImage, Self) {
        (std::mem::take(&mut self.image), self)
    }

    pub fn with_image(self, image: RgbaImage) -> Self {
        Self { image, ..self }
    }
}

impl RenderJob {
    /// Number of pixels it computes, about, filters counted (to run small
    /// ones at once).
    pub fn size(&self) -> u64 {
        let (w, h) = self.flat.dimensions();
        w as u64 * h as u64 * (1 + self.filters.len() as u64)
    }

    /// It runs smart filters (whose cost its size can't tell).
    pub fn has_filters(&self) -> bool {
        !self.filters.is_empty()
    }

    /// The filter mask moved as `image` at `origin` (canvas), with the
    /// settings of `m`.
    fn with_moved_mask(mut self, m: &Mask, image: GrayImage, origin: (i64, i64)) -> Self {
        if self.mask.is_some() {
            let mut moved = m.clone();
            moved.set_linked(false, (0, 0));
            moved.replace(image.clone(), origin, (0, 0));
            self.mask = Some(MaskSample::of(&moved, (0, 0)));
        }
        self.moved_mask = Some((image, origin));
        self
    }

    pub fn run(self) -> SmartRender {
        let (base, origin) = self.base();
        let base = Arc::new(base);
        let filtered = match self.filters.is_empty() {
            true => Arc::clone(&base),
            false => {
                let mut image = (*base).clone();
                for f in &self.filters {
                    apply_filter(&mut image, f, origin);
                }
                Arc::new(image)
            }
        };
        let image = match &self.mask {
            Some(m) => {
                let mut image = (*filtered).clone();
                remix(&mut image, &base, &filtered, origin, m, None);
                image
            }
            None => (*filtered).clone(),
        };
        SmartRender {
            image,
            origin,
            map: self.t.map,
            base,
            filtered,
            moved_mask: self.moved_mask,
        }
    }

    /// The contents through the transform, over the pixels they cover
    /// (within the limit), and its canvas position.
    fn base(&self) -> (RgbaImage, (i64, i64)) {
        let (w, h) = self.flat.dimensions();
        let covered = match self.t.bounds([0.0, 0.0, w as f64, h as f64]) {
            Some(b) => grid_rect(Grid::covering(b, 1.0)),
            None => self.limit,
        };
        let mut r = covered;
        if r.w as u64 * r.h as u64 > MAX_LAYER_PIXELS {
            r = intersect(r, self.limit).unwrap_or(PixelRect { w: 0, h: 0, ..r });
        }
        if r.w == 0 || r.h == 0 || w == 0 || h == 0 {
            return (RgbaImage::new(0, 0), (r.x, r.y));
        }
        // A whole-pixel move: the contents' pixels as they are.
        if let Some((dx, dy)) = whole_move(&self.t.map) {
            let mut out = RgbaImage::new(r.w, r.h);
            image::imageops::replace(&mut out, &*self.flat, dx - r.x, dy - r.y);
            return (out, (r.x, r.y));
        }
        let grid = Grid {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            px: 1.0,
        };
        let src = Source {
            image: &*self.flat,
            origin: [0.0, 0.0],
            px: 1.0,
        };
        let image = xform::transform_rgba(src, &self.t, grid, self.interpolation);
        (image, (r.x, r.y))
    }
}

fn grid_rect(g: Grid) -> PixelRect {
    PixelRect {
        x: g.x,
        y: g.y,
        w: g.w,
        h: g.h,
    }
}

/// `f` applied to `image` (at `origin` in the canvas), blended with it by
/// its mode and opacity.
fn apply_filter(image: &mut RgbaImage, f: &SmartFilter, origin: (i64, i64)) {
    let mut out = image.clone();
    f.filter.apply(&mut out, origin);
    let opacity = f.opacity.clamp(0.0, 1.0);
    if opacity >= 1.0 && matches!(f.blend, BlendMode::Normal | BlendMode::PassThrough) {
        *image = out;
        return;
    }
    let w = image.width() as usize;
    let src = out.as_raw();
    let mode = f.blend;
    par_rows(image.as_mut(), w * 4, |first, band| {
        for (k, px) in band.chunks_exact_mut(4).enumerate() {
            let i = first * w * 4 + k * 4;
            let s = [src[i], src[i + 1], src[i + 2], src[i + 3]];
            let d = [px[0], px[1], px[2], px[3]];
            let blended = match mode {
                BlendMode::Normal | BlendMode::PassThrough => {
                    mix(d, s, (opacity * 255.0).round() as u8)
                }
                BlendMode::Dissolve => {
                    let (x, y) = (origin.0 + (k % w) as i64, origin.1 + (first + k / w) as i64);
                    if dissolve(opacity, x, y) > 0.0 { s } else { d }
                }
                _ => {
                    let a = d[3] as f32 / 255.0;
                    let p = [0, 1, 2].map(|c| d[c] as f32 / 255.0 * a);
                    let mut p = [p[0], p[1], p[2], a];
                    let rgb = [0, 1, 2].map(|c| s[c] as f32 / 255.0);
                    atop(&mut p, rgb, opacity * s[3] as f32 / 255.0, mode);
                    p[3] = a;
                    compose::unpremultiply(p)
                }
            };
            px.copy_from_slice(&blended);
        }
    });
}

/// The filtered pixels shown through the filter mask over the others, in
/// `area` (canvas; everywhere with `None`) of `image` (at `origin`).
fn remix(
    image: &mut RgbaImage,
    base: &RgbaImage,
    filtered: &RgbaImage,
    origin: (i64, i64),
    mask: &MaskSample,
    area: Option<PixelRect>,
) {
    let (w, h) = image.dimensions();
    if base.dimensions() != (w, h) || filtered.dimensions() != (w, h) {
        return;
    }
    let own = PixelRect {
        x: origin.0,
        y: origin.1,
        w,
        h,
    };
    let Some(r) = intersect(area.unwrap_or(own), own) else {
        return;
    };
    let row = w as usize * 4;
    let (b, f) = (base.as_raw(), filtered.as_raw());
    par_rows(image.as_mut(), row, |first, band| {
        for (k, line) in band.chunks_exact_mut(row).enumerate() {
            let y = first + k;
            let cy = origin.1 + y as i64;
            if cy < r.y || cy >= r.bottom() {
                continue;
            }
            for cx in r.x..r.right() {
                let x = (cx - origin.0) as usize;
                let i = y * row + x * 4;
                let from = [b[i], b[i + 1], b[i + 2], b[i + 3]];
                let to = [f[i], f[i + 1], f[i + 2], f[i + 3]];
                line[x * 4..x * 4 + 4].copy_from_slice(&mix(from, to, mask.at(cx, cy)));
            }
        }
    });
}

impl Layer {
    pub fn is_smart(&self) -> bool {
        self.smart.is_some()
    }

    /// Pixels the painting tools and pixel operations may change: not a
    /// smart object's (rasterized first).
    pub fn paintable(&self) -> bool {
        self.has_pixels() && self.smart.is_none()
    }

    /// The rendering of its smart object as it is, within `limit` (canvas)
    /// when too large.
    pub fn smart_job(&self, limit: PixelRect) -> Option<RenderJob> {
        let s = self.smart.as_deref()?;
        Some(s.job(s.transform(self.offset), self.offset, limit))
    }

    /// Shows `r`: its pixels, its position (a linked mask moves with it:
    /// see [`Self::keep_masks`]), the transform it was made with.
    pub fn put_smart(&mut self, r: SmartRender) {
        let Some(s) = self.smart.as_deref_mut() else {
            return;
        };
        s.map = shift_map(r.map, [-r.origin.0 as f64, -r.origin.1 as f64]);
        s.cache = Some((r.base, r.filtered));
        if let (Some((image, origin)), Some(m)) = (r.moved_mask, &mut s.filter_mask) {
            m.replace(image, origin, r.origin);
        }
        self.offset = r.origin;
        self.image = Arc::new(r.image);
        self.rev = next_rev();
    }

    /// After a move of the layer from `old` that its masks must not follow.
    fn keep_masks(&mut self, old: (i64, i64)) {
        let new = self.offset;
        let masks = self
            .mask
            .iter_mut()
            .chain(self.smart.iter_mut().filter_map(|s| s.filter_mask.as_mut()));
        for m in masks {
            let origin = m.origin(old);
            m.set_origin(origin, new);
        }
    }

    /// Renders its smart object again here, its masks staying in place.
    pub fn render_smart(&mut self, limit: PixelRect) {
        let Some(job) = self.smart_job(limit) else {
            return;
        };
        let old = self.offset;
        self.put_smart(job.run());
        self.keep_masks(old);
    }

    /// Its smart object mapped by `a` (canvas coordinates, the layer having
    /// been at `old` before), rendered again, its filter mask following.
    /// `false` if it has none.
    pub fn map_smart(&mut self, old: (i64, i64), a: &Affine, limit: PixelRect) -> bool {
        let Some(s) = self.smart.as_deref() else {
            return false;
        };
        let [p, q, r, u, v, w] = a.0.map(f64::from);
        let outer = Transform::projective([0.0; 4], Homography::affine([p, q, r, u, v, w]));
        let inner = s.transform(old);
        let Some(map) = compose(&inner, &outer) else {
            return false;
        };
        let t = Transform {
            src: inner.src,
            map,
        };
        let mut job = s.job(t, old, limit);
        if let Some(m) = &s.filter_mask {
            let (image, origin) = moved_mask(m, old, &outer, limit);
            job = job.with_moved_mask(m, image, origin);
        }
        self.put_smart(job.run());
        true
    }

    /// The filter mask over `area` (canvas) shown again after it was
    /// painted: the filtered pixels through it.
    pub(super) fn remix_filter_mask(&mut self, area: Option<PixelRect>) {
        let (offset, rect) = (self.offset, self.rect());
        let Some(s) = self.smart.as_deref_mut() else {
            return;
        };
        if s.cache.is_none() {
            let job = s.job(s.transform(offset), offset, rect);
            let r = job.run();
            s.cache = Some((r.base, r.filtered));
        }
        let (Some(m), Some((base, filtered))) = (&s.filter_mask, &s.cache) else {
            return;
        };
        if !m.enabled || s.applied().is_empty() {
            return;
        }
        let sample = MaskSample::of(m, offset);
        let (base, filtered) = (Arc::clone(base), Arc::clone(filtered));
        remix(
            Arc::make_mut(&mut self.image),
            &base,
            &filtered,
            offset,
            &sample,
            area,
        );
        self.rev = next_rev();
    }
}

/// Mask `m` of a layer at `offset` as `t` moves it, within `limit`.
fn moved_mask(
    m: &Mask,
    offset: (i64, i64),
    t: &Transform,
    limit: PixelRect,
) -> (GrayImage, (i64, i64)) {
    let image = m.image();
    let origin = m.origin(offset);
    let area = PixelRect {
        x: origin.0,
        y: origin.1,
        w: image.width(),
        h: image.height(),
    };
    if area.w == 0 || area.h == 0 {
        return (image.clone(), origin);
    }
    let src = [
        area.x as f64,
        area.y as f64,
        area.right() as f64,
        area.bottom() as f64,
    ];
    let covered = t
        .bounds(src)
        .map(|b| grid_rect(Grid::covering(b, 1.0)))
        .and_then(|r| intersect(r, limit));
    let Some(r) = covered else {
        return (GrayImage::new(0, 0), origin);
    };
    let grid = Grid {
        x: r.x,
        y: r.y,
        w: r.w,
        h: r.h,
        px: 1.0,
    };
    let source = Source {
        image,
        origin: [area.x as f64, area.y as f64],
        px: 1.0,
    };
    let moved = xform::transform_gray(source, t, grid, xform::Filter::Bilinear, m.default);
    (moved, (r.x, r.y))
}

/// The smart object's job of a transform (see `transform.rs`): its
/// rendering through `t` (canvas), its filter mask following.
pub(super) fn transformed_job(layer: &Layer, t: &Transform, limit: PixelRect) -> Option<RenderJob> {
    let s = layer.smart.as_deref()?;
    let inner = s.transform(layer.offset);
    let map = compose(&inner, t)?;
    let mut job = s.job(
        Transform {
            src: inner.src,
            map,
        },
        layer.offset,
        limit,
    );
    if let Some(m) = &s.filter_mask {
        let (image, origin) = moved_mask(m, layer.offset, t, limit);
        job = job.with_moved_mask(m, image, origin);
    }
    Some(job)
}

/// The tab of a smart object's contents opened by Edit Contents.
#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddedIn {
    /// The document of the smart object ([`Document::uid`]).
    pub parent: u64,
    /// The contents' id.
    pub contents: u64,
    /// The tab's name.
    pub name: String,
}

/// A linked file to look at again, and the instances of its smart object
/// rendered with it (see [`Document::stale_links`]).
pub struct LinkRead {
    path: PathBuf,
    /// The modification time of the copy shown, and whether the file was
    /// missing: read again if it changed or came back.
    known: Option<SystemTime>,
    missing: bool,
    /// The contents' id, kept.
    id: u64,
    instances: Vec<LinkInstance>,
    limit: PixelRect,
}

/// A smart object showing a linked file, as it was when looked at.
struct LinkInstance {
    layer: u64,
    /// The layer's revision: rendered for it as it was.
    rev: u64,
    name: String,
    smart: SmartObject,
    offset: (i64, i64),
}

/// What became of a linked file (see [`LinkRead::run`]).
enum LinkState {
    Unchanged,
    Missing,
    /// It exists but could not be read (being written, damaged).
    Unreadable,
    /// Read: the contents shared by the instances, each one's rendering.
    Read(Box<(Contents, Vec<SmartRender>)>),
}

/// A linked file looked at again.
pub struct LinkFresh {
    path: PathBuf,
    /// The file's modification time before it was read.
    stamp: Option<SystemTime>,
    /// The instances' layers, revisions and names.
    instances: Vec<(u64, u64, String)>,
    state: LinkState,
}

impl LinkRead {
    /// Looks at the file, reads it once if it changed and renders each
    /// instance with it (slow, also on a network path: any thread).
    pub fn run(self) -> LinkFresh {
        let instances = self
            .instances
            .iter()
            .map(|i| (i.layer, i.rev, i.name.clone()))
            .collect();
        let fresh = |stamp, state| LinkFresh {
            path: self.path.clone(),
            stamp,
            instances,
            state,
        };
        let Ok(meta) = std::fs::metadata(&self.path) else {
            return fresh(None, LinkState::Missing);
        };
        let stamp = meta.modified().ok();
        if stamp == self.known && !self.missing {
            return fresh(stamp, LinkState::Unchanged);
        }
        let Ok(opened) = Document::open(&self.path) else {
            return fresh(stamp, LinkState::Unreadable);
        };
        let contents = opened.doc.to_contents(self.id, file_name(&self.path));
        let renders = self
            .instances
            .into_iter()
            .map(|i| {
                let mut smart = i.smart;
                smart.contents = contents.clone();
                smart
                    .job(smart.transform(i.offset), i.offset, self.limit)
                    .run()
            })
            .collect();
        fresh(stamp, LinkState::Read(Box::new((contents, renders))))
    }
}

/// What became of the linked files (see [`Document::put_links`]).
#[derive(Default, Debug, PartialEq)]
pub struct LinkReport {
    /// Names of the layers whose file changed, shown again.
    pub updated: Vec<String>,
    /// Names of the layers whose file went missing.
    pub missing: Vec<String>,
    /// Names of the layers whose file was already missing (said once).
    pub still_missing: Vec<String>,
    /// Names of the layers whose file exists but could not be read.
    pub unreadable: Vec<String>,
}

impl Document {
    /// These contents (its layers, canvas, guides, channels, paths), the
    /// composite made once.
    pub fn to_contents(&self, id: u64, name: String) -> Contents {
        Contents {
            id,
            name,
            layers: self.render_layers().into_owned(),
            active: self.active,
            canvas: self.canvas,
            guides: self.guides.clone(),
            channels: self.channels.clone(),
            paths: self.paths.clone(),
            work_path: self.work_path.clone(),
            icc_profile: self.icc_profile.clone(),
            flat: Arc::new(self.flatten()),
        }
    }

    /// A document showing `c`, without history.
    pub fn from_contents(c: &Contents) -> Self {
        let mut doc = Self::new(RgbaImage::new(0, 0), None);
        doc.layers = c.layers.clone();
        doc.active = c.active.min(c.layers.len().saturating_sub(1));
        doc.canvas = c.canvas;
        doc.guides = c.guides.clone();
        doc.channels = c.channels.clone();
        doc.paths = c.paths.clone();
        doc.work_path = c.work_path.clone();
        doc.icc_profile = c.icc_profile.clone();
        doc.next_id = c.layers.iter().map(|l| l.id).max().unwrap_or(0);
        doc
    }

    /// The contents of smart object `i`, as a document of its own (Edit
    /// Contents, Export Contents).
    pub fn smart_contents(&self, i: usize) -> Option<Document> {
        let s = self.layers.get(i)?.smart.as_deref()?;
        let mut doc = Self::from_contents(&s.contents);
        doc.embedded_in = Some(EmbeddedIn {
            parent: self.uid,
            contents: s.contents.id,
            name: s.contents.name.clone(),
        });
        Some(doc)
    }

    /// The selected blocks (a group with its layers), merged, bottom first.
    fn top_blocks(&self, indices: &[usize]) -> Vec<Range<usize>> {
        let mut blocks: Vec<Range<usize>> = indices
            .iter()
            .filter(|&&i| i < self.layers.len())
            .map(|&i| self.block(i))
            .collect();
        blocks.sort_by_key(|r| r.start);
        let mut tops: Vec<Range<usize>> = Vec::new();
        for r in blocks {
            match tops.last_mut() {
                Some(t) if r.start < t.end => t.end = t.end.max(r.end),
                _ => tops.push(r),
            }
        }
        tops
    }

    /// Convert to Smart Object: the layers `indices` (and those of the
    /// groups among them) become the contents of a smart object where the
    /// topmost of them was, their stack, masks and styles kept inside. A
    /// single layer keeps its visibility, opacity, blend mode and clipping
    /// on the smart object. Refused if one of them is locked.
    pub fn convert_to_smart(&mut self, indices: &[usize]) -> bool {
        let tops = self.top_blocks(indices);
        let Some(top) = tops.last().cloned() else {
            return false;
        };
        if tops
            .iter()
            .flat_map(|r| r.clone())
            .any(|i| self.layers[i].locked)
        {
            return false;
        }
        let canvas = self.full_rect();
        let bounds = tops
            .iter()
            .flat_map(|r| r.clone())
            .map(|i| self.layers[i].extent(false).unwrap_or(canvas))
            .filter(|r| r.w > 0 && r.h > 0)
            .reduce(PixelRect::union)
            .unwrap_or(canvas);
        self.checkpoint(t!("smart-menu-convert"));
        let top_layer = self.layers[top.end - 1].clone();
        let mut moved: Vec<Layer> = Vec::new();
        for r in tops.iter().rev() {
            let depth = self.layers[r.end - 1].depth;
            let mut part: Vec<Layer> = self.layers.drain(r.clone()).collect();
            for l in &mut part {
                l.depth -= depth;
                l.translate(-bounds.x, -bounds.y);
            }
            moved.splice(0..0, part);
        }
        let below: usize = tops[..tops.len() - 1].iter().map(|r| r.len()).sum();
        let at = top.start - below;
        let single = moved.len() == 1;
        let mut smart = self.make_layer(
            top_layer.name.clone(),
            RgbaImage::new(0, 0),
            (bounds.x, bounds.y),
        );
        smart.depth = top_layer.depth;
        if single {
            let l = &mut moved[0];
            (smart.visible, smart.opacity, smart.blend) = (l.visible, l.opacity, l.blend);
            (smart.clipped, smart.link) = (l.clipped, l.link);
            (l.visible, l.opacity, l.blend, l.clipped, l.link) =
                (true, 1.0, BlendMode::Normal, false, None);
        }
        let mut inner = Self::new(RgbaImage::new(0, 0), None);
        inner.layers = moved;
        inner.canvas = (bounds.w, bounds.h);
        inner.active = inner.layers.len() - 1;
        inner.fix_depths();
        let contents = inner.to_contents(next_rev(), top_layer.name.clone());
        smart.smart = Some(Box::new(SmartObject::new(
            contents,
            Map::Projective(Homography::IDENTITY),
        )));
        smart.render_smart(self.shape_limit());
        self.layers.insert(at, smart);
        self.active = at;
        self.fix_depths();
        true
    }

    /// New Smart Object via Copy: a copy of smart object `active` whose
    /// contents are its own (Duplicate Layer makes another instance).
    pub fn new_smart_via_copy(&mut self) -> bool {
        if !self.active_layer().is_smart() {
            return false;
        }
        self.checkpoint(t!("smart-menu-new-via-copy"));
        self.next_id += 1;
        let mut copy = Layer {
            id: self.next_id,
            ..self.active_layer().clone()
        };
        copy.name = self.copy_name(&copy.name);
        if let Some(s) = copy.smart.as_deref_mut() {
            s.contents.id = next_rev();
        }
        self.layers.insert(self.active + 1, copy);
        self.active += 1;
        true
    }

    /// Place Embedded / Place Linked: the file at `path` (an image, an
    /// Atelier or Photoshop document) as a new smart object above the
    /// active layer, centered, reduced to fit the canvas if larger.
    /// Returns whether only the first frame of an animation was taken.
    pub fn place_file(&mut self, path: &Path, linked: bool) -> Result<bool, FileError> {
        let opened = Self::open(path)?;
        let contents = opened.doc.to_contents(next_rev(), file_name(path));
        let (w, h) = contents.size();
        // Fitted and centered in the active layer's artboard, or the canvas.
        let (x0, y0, cw, ch) = match self.artboard_of(self.active) {
            Some(b) => self.layers[b]
                .artboard_rect()
                .map_or((0, 0, 0, 0), |r| (r.x, r.y, r.w, r.h)),
            None => (0, 0, self.canvas.0, self.canvas.1),
        };
        let (cw, ch) = (f64::from(cw), f64::from(ch));
        let k = (cw / w.max(1) as f64).min(ch / h.max(1) as f64).min(1.0);
        let (tx, ty) = (
            x0 as f64 + (cw - w as f64 * k) / 2.0,
            y0 as f64 + (ch - h as f64 * k) / 2.0,
        );
        // Whole pixels when not reduced: the file shows as it is.
        let (tx, ty) = if k == 1.0 {
            (tx.floor(), ty.floor())
        } else {
            (tx, ty)
        };
        let map = Map::Projective(Homography::affine([k, 0.0, tx, 0.0, k, ty]));
        let name = path.file_stem().map_or_else(
            || t!("doc-image-layer-default-name"),
            |n| n.to_string_lossy().into_owned(),
        );
        self.checkpoint(if linked {
            t!("smart-hist-place-linked")
        } else {
            t!("smart-hist-place-embedded")
        });
        let mut layer = self.make_layer(name, RgbaImage::new(0, 0), (0, 0));
        let mut smart = SmartObject::new(contents, map);
        smart.link = linked.then(|| Link::new(path));
        layer.smart = Some(Box::new(smart));
        layer.render_smart(self.shape_limit());
        self.insert_layer(layer);
        Ok(opened.animated)
    }

    /// Replace Contents: smart object `active` and the other instances of
    /// its contents show the file at `path` instead, each centered where
    /// its contents were, with the same transform, as one step.
    pub fn replace_contents(&mut self, path: &Path) -> Result<(), FileError> {
        let Some(old) = self.active_layer().smart.as_deref() else {
            return Ok(());
        };
        let id = old.contents.id;
        let opened = Self::open(path)?;
        // New contents, a new id (shared by the instances): an Edit
        // Contents tab of the old ones no longer saves over them.
        let contents = opened.doc.to_contents(next_rev(), file_name(path));
        let (ow, oh) = old.contents.size();
        let (nw, nh) = contents.size();
        let d = [(ow as f64 - nw as f64) / 2.0, (oh as f64 - nh as f64) / 2.0];
        let shift = Homography::affine([1.0, 0.0, d[0], 0.0, 1.0, d[1]]);
        self.checkpoint(t!("smart-hist-replace"));
        let limit = self.shape_limit();
        let instance = |l: &Layer| l.smart.as_ref().is_some_and(|s| s.contents.id == id);
        for layer in self.layers.iter_mut().filter(|l| instance(l)) {
            if let Some(s) = layer.smart.as_deref_mut() {
                s.contents = contents.clone();
                if let Map::Projective(h) = s.map {
                    s.map = Map::Projective(shift.then(&h));
                }
                if s.link.is_some() {
                    s.link = Some(Link::new(path));
                }
            }
            layer.render_smart(limit);
        }
        Ok(())
    }

    /// The contents `id` edited (Edit Contents, saved): every instance of
    /// it shows them, embedded if its link was missing, as one undo step.
    /// An instance whose linked file is there follows its file: left as it
    /// is, under an id of its own. Returns whether one was updated.
    pub fn update_contents(&mut self, id: u64, mut contents: Contents) -> bool {
        let found = |l: &Layer| l.smart.as_ref().is_some_and(|s| s.contents.id == id);
        let follows_file = |l: &Layer| {
            l.smart
                .as_ref()
                .and_then(|s| s.link.as_ref())
                .is_some_and(|k| !k.gone())
        };
        if !self.layers.iter().any(|l| found(l) && !follows_file(l)) {
            return false;
        }
        self.checkpoint(t!("smart-hist-update"));
        contents.id = id;
        let limit = self.shape_limit();
        let apart = next_rev();
        for layer in self.layers.iter_mut().filter(|l| found(l)) {
            if follows_file(layer) {
                if let Some(s) = layer.smart.as_deref_mut() {
                    s.contents.id = apart;
                }
                continue;
            }
            if let Some(s) = layer.smart.as_deref_mut() {
                contents.name = s.contents.name.clone();
                s.contents = contents.clone();
                // Its file (missing) would replace these edits: embedded.
                s.link = None;
            }
            layer.render_smart(limit);
        }
        true
    }

    /// Embed Linked: smart object `i` and the other instances of its
    /// contents linked to the same file hold them from now on (the
    /// instances share one contents id, so they are embedded together), as
    /// one undo step. Returns whether it was linked.
    pub fn embed_linked(&mut self, i: usize, name: String) -> bool {
        let Some((id, path)) = self
            .layers
            .get(i)
            .and_then(|l| l.smart.as_deref())
            .and_then(|s| Some((s.contents.id, s.link.as_ref()?.path.clone())))
        else {
            return false;
        };
        let same = |l: &Layer| {
            l.smart.as_deref().is_some_and(|s| {
                s.contents.id == id && s.link.as_ref().is_some_and(|k| k.path == path)
            })
        };
        // An instance linked elsewhere (an older document) keeps its link,
        // under an id of its own.
        let apart = next_rev();
        self.checkpoint(name);
        for layer in &mut self.layers {
            let embed = same(layer);
            if let Some(s) = layer.smart.as_deref_mut()
                && s.contents.id == id
            {
                if embed {
                    s.link = None;
                } else if s.link.is_some() {
                    s.contents.id = apart;
                }
            }
        }
        true
    }

    /// Linked smart objects whose file changed show it again, as one undo
    /// step; their files are read here (the app reads them on another
    /// thread: see [`Self::stale_links`]).
    #[cfg(test)]
    pub fn update_links(&mut self) -> LinkReport {
        let reads = self.stale_links(true);
        self.put_links(reads.into_iter().map(LinkRead::run).collect())
    }

    /// The linked files to look at again ([`LinkRead::run`], on any thread,
    /// then [`Self::put_links`]): one per file and contents, whatever the
    /// number of instances. Without `network`, files on another machine are
    /// left alone (looking would connect to it).
    pub fn stale_links(&self, network: bool) -> Vec<LinkRead> {
        let limit = self.shape_limit();
        let mut reads: Vec<LinkRead> = Vec::new();
        for l in &self.layers {
            let Some(s) = l.smart.as_deref() else {
                continue;
            };
            let Some(link) = &s.link else {
                continue;
            };
            if !network && super::native::remote(&link.path) {
                continue;
            }
            let instance = LinkInstance {
                layer: l.id,
                rev: l.rev,
                name: l.name.clone(),
                smart: s.clone(),
                offset: l.offset,
            };
            let id = s.contents.id;
            match reads.iter_mut().find(|r| r.id == id && r.path == link.path) {
                Some(r) => {
                    r.missing |= link.missing;
                    r.instances.push(instance);
                }
                None => reads.push(LinkRead {
                    path: link.path.clone(),
                    known: link.modified,
                    missing: link.missing,
                    id,
                    instances: vec![instance],
                    limit,
                }),
            }
        }
        reads
    }

    /// The linked files looked at again: those read shown, as one undo
    /// step (rendered again here when a smart object changed meanwhile);
    /// the missing ones marked so, their last copy kept (the document
    /// doesn't change).
    pub fn put_links(&mut self, fresh: Vec<LinkFresh>) -> LinkReport {
        let mut report = LinkReport::default();
        let limit = self.shape_limit();
        let mut stepped = false;
        for f in fresh {
            let renders = match f.state {
                LinkState::Unchanged => continue,
                LinkState::Missing => {
                    for (layer, _, name) in f.instances {
                        let link = self
                            .layer_index(layer)
                            .and_then(|i| self.layers[i].smart.as_deref_mut())
                            .and_then(|s| s.link.as_mut())
                            .filter(|l| l.path == f.path);
                        if let Some(link) = link {
                            if link.missing {
                                report.still_missing.push(name);
                            } else {
                                report.missing.push(name);
                            }
                            link.missing = true;
                        }
                    }
                    continue;
                }
                LinkState::Unreadable => {
                    report
                        .unreadable
                        .extend(f.instances.into_iter().map(|(_, _, name)| name));
                    continue;
                }
                LinkState::Read(read) => *read,
            };
            let (contents, renders) = renders;
            for ((layer, rev, name), render) in f.instances.into_iter().zip(renders) {
                let Some(i) = self.layer_index(layer) else {
                    continue;
                };
                let same_link = |s: &SmartObject| s.link.as_ref().is_some_and(|l| l.path == f.path);
                if !self.layers[i].smart.as_deref().is_some_and(same_link) {
                    continue;
                }
                if !stepped {
                    self.checkpoint(t!("smart-hist-update-links"));
                    stepped = true;
                }
                let layer = &mut self.layers[i];
                let unchanged = layer.rev == rev;
                if let Some(s) = layer.smart.as_deref_mut() {
                    // Shared by the instances (their layers too), read once.
                    s.contents = contents.clone();
                    if let Some(link) = &mut s.link {
                        link.missing = false;
                        link.modified = f.stamp;
                    }
                }
                if unchanged {
                    let old = layer.offset;
                    layer.put_smart(render);
                    layer.keep_masks(old);
                } else {
                    layer.render_smart(limit);
                }
                self.modified = true;
                report.updated.push(name);
            }
        }
        report
    }

    /// Records the undo step `name`, then changes smart object `i` with
    /// `f` and renders it again here (see [`Self::edit_smart_later`]).
    pub fn edit_smart(&mut self, i: usize, name: String, f: impl FnOnce(&mut SmartObject)) -> bool {
        if !self.edit_smart_later(i, name, f) {
            return false;
        }
        let limit = self.shape_limit();
        self.layers[i].render_smart(limit);
        true
    }

    /// [`Self::edit_smart`] without the rendering, left to the job
    /// [`Self::smart_job`] gives, put in place by [`Self::put_smart`].
    pub fn edit_smart_later(
        &mut self,
        i: usize,
        name: String,
        f: impl FnOnce(&mut SmartObject),
    ) -> bool {
        if !self.layers.get(i).is_some_and(Layer::is_smart) {
            return false;
        }
        self.checkpoint(name);
        if let Some(s) = self.layers[i].smart.as_deref_mut() {
            f(s);
        }
        true
    }

    /// The rendering of smart object `i` as it is now.
    pub fn smart_job(&self, i: usize) -> Option<RenderJob> {
        self.layers.get(i)?.smart_job(self.shape_limit())
    }

    /// The rendering of smart object `i` changed by `f`, the document left
    /// as it is (the previews of the dialogs).
    pub fn smart_preview_job(
        &self,
        i: usize,
        f: impl FnOnce(&mut SmartObject),
    ) -> Option<RenderJob> {
        let l = self.layers.get(i)?;
        let mut s = l.smart.as_deref()?.clone();
        f(&mut s);
        Some(s.job(s.transform(l.offset), l.offset, self.shape_limit()))
    }

    /// What smart filter `k` of smart object `i` filters (`None`: a new
    /// one, on top): the rendering through the filters below it, before
    /// their mask.
    pub fn smart_filter_input(&self, i: usize, k: Option<usize>) -> Option<Arc<RgbaImage>> {
        let s = self.layers.get(i)?.smart.as_deref()?;
        // On top, or under all of them: as rendered last.
        if let Some((base, filtered)) = &s.cache {
            match k {
                None if s.filters_enabled => return Some(Arc::clone(filtered)),
                Some(0) => return Some(Arc::clone(base)),
                _ => {}
            }
        }
        let job = self.smart_preview_job(i, |s| {
            s.filters.truncate(k.unwrap_or(s.filters.len()));
            s.filter_mask = None;
        })?;
        Some(Arc::new(job.run().image))
    }

    /// Shows `r` on the smart object `id`, if still there (no new step).
    pub fn put_smart(&mut self, id: u64, r: SmartRender) -> bool {
        let Some(i) = self.layer_index(id) else {
            return false;
        };
        let layer = &mut self.layers[i];
        let old = layer.offset;
        layer.put_smart(r);
        layer.keep_masks(old);
        self.modified = true;
        true
    }

    /// Adds a smart filter mask to smart object `i`: from the selection if
    /// any (the filters apply inside it), else showing everything.
    pub fn add_filter_mask(&mut self, i: usize, selection: Option<&Selection>) -> bool {
        let canvas = self.full_rect();
        let Some(layer) = self.layers.get(i) else {
            return false;
        };
        let offset = layer.offset;
        let mask = match selection {
            Some(sel) => {
                let outside = if sel.inverted { 255 } else { 0 };
                Mask::new(sel.to_mask(canvas), (canvas.x, canvas.y), outside, offset)
            }
            None => Mask::new(GrayImage::new(0, 0), (0, 0), 255, offset),
        };
        self.edit_smart(i, t!("smart-hist-add-filter-mask"), |s| {
            s.filter_mask = Some(mask)
        })
    }

    /// Whether the targeted mask is the active smart object's filter mask.
    pub fn filter_mask_targeted(&self) -> bool {
        self.filter_mask_target && self.quick_mask.is_none() && self.mask_targeted()
    }

    /// Targets the active smart object's filter mask (painting tools,
    /// filters and adjustments edit it), or its pixels.
    pub fn target_filter_mask(&mut self, on: bool) {
        let has = self
            .real_active()
            .smart
            .as_ref()
            .is_some_and(|s| s.filter_mask.is_some());
        self.target_mask(on && has);
        self.filter_mask_target = on && has;
    }
}

/// Name of the file at `path`.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// The mask the tools edit on `layer`: its smart filter mask (`filter`),
/// or its layer mask.
pub(super) fn target_mask(layer: &Layer, filter: bool) -> Option<&Mask> {
    match filter {
        true => layer.smart.as_ref()?.filter_mask.as_ref(),
        false => layer.mask.as_ref(),
    }
}

pub(super) fn target_mask_mut(layer: &mut Layer, filter: bool) -> Option<&mut Mask> {
    match filter {
        true => layer.smart.as_mut()?.filter_mask.as_mut(),
        false => layer.mask.as_mut(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A 64 × 48 pattern of sharp details: every pixel differs.
    fn pattern() -> RgbaImage {
        RgbaImage::from_fn(64, 48, |x, y| {
            Rgba([
                (x * 4) as u8,
                (y * 5) as u8,
                ((x * 7 + y * 13) % 256) as u8,
                255,
            ])
        })
    }

    /// 200 × 150 white canvas, the pattern at (20, 10) as a smart object.
    fn doc() -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(200, 150, Rgba([255; 4])), None);
        doc.add_layer("pattern".into(), pattern(), (20, 10));
        assert!(doc.convert_to_smart(&[1]));
        doc
    }

    fn scale(doc: &mut Document, k: f64) {
        let src = doc.element_bounds(doc.active).unwrap();
        let b = [
            src.x as f64,
            src.y as f64,
            src.right() as f64,
            src.bottom() as f64,
        ];
        let t = Transform::projective(b, Homography::scale([b[0], b[1]], k, k));
        let layers = [doc.active];
        doc.transform_target(
            TransformTarget::Layer,
            &layers,
            &t,
            FilterType::Lanczos3,
            "Scale".into(),
        );
    }

    #[test]
    fn converting_keeps_the_look_and_the_layers_inside() {
        let mut doc = Document::new(RgbaImage::from_pixel(200, 150, Rgba([255; 4])), None);
        doc.add_layer("pattern".into(), pattern(), (20, 10));
        doc.active_layer_mut().opacity = 0.5;
        let before = doc.flatten();
        assert!(doc.convert_to_smart(&[1]));
        assert_eq!(doc.layers.len(), 2);
        let l = doc.active_layer();
        assert!(l.is_smart() && !l.paintable());
        assert_eq!(l.name, "pattern");
        assert_eq!((l.opacity, l.offset), (0.5, (20, 10)));
        assert_eq!(doc.flatten(), before);
        let inner = doc.smart_contents(1).unwrap();
        assert_eq!((inner.width(), inner.height()), (64, 48));
        assert_eq!(inner.layers.len(), 1);
        assert_eq!(inner.layers[0].opacity, 1.0);
        assert_eq!(*inner.layers[0].image(), pattern());
        // One undo step.
        assert!(doc.undo());
        assert!(!doc.active_layer().is_smart());
        assert_eq!(doc.flatten(), before);
    }

    #[test]
    fn scaling_down_then_up_loses_nothing() {
        let mut doc = doc();
        scale(&mut doc, 0.125);
        assert_eq!(doc.active_layer().rect().w, 8);
        scale(&mut doc, 8.0);
        let l = doc.active_layer();
        assert!(l.is_smart());
        assert_eq!(
            l.rect(),
            PixelRect {
                x: 20,
                y: 10,
                w: 64,
                h: 48
            }
        );
        assert_eq!(*l.image(), pattern(), "rendered again from the contents");

        // A pixel layer does lose its details.
        let mut plain = Document::new(RgbaImage::from_pixel(200, 150, Rgba([255; 4])), None);
        plain.add_layer("pattern".into(), pattern(), (20, 10));
        scale(&mut plain, 0.125);
        scale(&mut plain, 8.0);
        assert_ne!(*plain.active_layer().image(), pattern());
    }

    #[test]
    fn canvas_operations_keep_it_smart() {
        let mut doc = doc();
        assert!(doc.resize_image(100, 75, FilterType::Lanczos3));
        assert!(doc.active_layer().is_smart());
        assert_eq!(
            doc.active_layer().rect(),
            PixelRect {
                x: 10,
                y: 5,
                w: 32,
                h: 24
            }
        );
        assert!(doc.resize_image(200, 150, FilterType::Lanczos3));
        assert_eq!(*doc.active_layer().image(), pattern());
        doc.flip_canvas(true);
        assert!(doc.active_layer().is_smart());
        let l = doc.active_layer();
        assert_eq!(l.rect().x, 200 - 20 - 64);
        assert_eq!(
            l.pixel_at(l.rect().right() - 1, 10),
            Some(pattern().get_pixel(0, 0).0)
        );
    }

    #[test]
    fn a_warp_after_a_rotation_keeps_both() {
        let mut doc = doc();
        let src = doc.element_bounds(doc.active).unwrap();
        let b = [
            src.x as f64,
            src.y as f64,
            src.right() as f64,
            src.bottom() as f64,
        ];
        let turn = Transform::projective(b, Homography::rotate([b[0], b[1]], 90.0));
        doc.transform_target(
            TransformTarget::Layer,
            &[1],
            &turn,
            FilterType::Triangle,
            "Rotate".into(),
        );
        let src = doc.element_bounds(doc.active).unwrap();
        let b = [
            src.x as f64,
            src.y as f64,
            src.right() as f64,
            src.bottom() as f64,
        ];
        let flat = Warp::from_map(
            &Homography::rect_to_quad(
                [0.0, 0.0, 1.0, 1.0],
                [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]],
            )
            .unwrap(),
        )
        .unwrap();
        let t = Transform {
            src: b,
            map: Map::Warp(flat),
        };
        doc.transform_target(
            TransformTarget::Layer,
            &[1],
            &t,
            FilterType::Triangle,
            "Warp".into(),
        );
        // An unbent warp of the turned contents: still the quarter turn.
        let s = doc.active_layer().smart.as_deref().unwrap();
        let tr = s.transform(doc.active_layer().offset);
        let p = tr.forward([0.0, 0.0]).unwrap();
        let q = turn.forward([20.0, 10.0]).unwrap();
        assert!(
            (p[0] - q[0]).abs() < 1e-6 && (p[1] - q[1]).abs() < 1e-6,
            "{p:?} {q:?}"
        );
        let r = tr.forward([64.0, 0.0]).unwrap();
        let q = turn.forward([84.0, 10.0]).unwrap();
        assert!((r[0] - q[0]).abs() < 1e-6 && (r[1] - q[1]).abs() < 1e-6);
    }

    fn blur(radius: f32) -> SmartFilter {
        SmartFilter::new(Filter::GaussianBlur { radius })
    }

    #[test]
    fn smart_filters_apply_in_order_with_visibility_and_mask() {
        let mut doc = doc();
        let original = doc.active_layer().image().clone();
        let filtered = |filters: &[Filter]| {
            let mut image = original.clone();
            for f in filters {
                f.apply(&mut image, (20, 10));
            }
            image
        };
        let mosaic = Filter::Mosaic { cell: 8.0 };
        let edges = Filter::FindEdges;
        let set = [blur(2.0), SmartFilter::new(mosaic)];
        assert!(doc.edit_smart(1, "Filters".into(), |s| s.filters = set.to_vec()));
        let blur2 = Filter::GaussianBlur { radius: 2.0 };
        assert_eq!(*doc.active_layer().image(), filtered(&[blur2, mosaic]));
        // In the other order: another result.
        doc.edit_smart(1, "Order".into(), |s| s.filters.reverse());
        assert_eq!(*doc.active_layer().image(), filtered(&[mosaic, blur2]));
        // Hidden, a filter doesn't apply; all off, none does.
        doc.edit_smart(1, "Hide".into(), |s| s.filters[1].visible = false);
        assert_eq!(*doc.active_layer().image(), filtered(&[mosaic]));
        doc.edit_smart(1, "Off".into(), |s| s.filters_enabled = false);
        assert_eq!(*doc.active_layer().image(), original);
        // Half opacity: half way.
        doc.edit_smart(1, "Opacity".into(), |s| {
            s.filters_enabled = true;
            s.filters = vec![SmartFilter {
                opacity: 0.5,
                ..SmartFilter::new(edges)
            }];
        });
        let full = filtered(&[edges]);
        let (p, o, f) = (
            doc.active_layer().image().get_pixel(5, 5).0,
            original.get_pixel(5, 5).0,
            full.get_pixel(5, 5).0,
        );
        assert_eq!(p, mix(o, f, 128));
        // The mask: filtered on the left half only.
        doc.edit_smart(1, "Full".into(), |s| s.filters[0].opacity = 1.0);
        let sel: Selection = PixelRect {
            x: 20,
            y: 10,
            w: 32,
            h: 48,
        }
        .into();
        assert!(doc.add_filter_mask(1, Some(&sel)));
        let l = doc.active_layer();
        assert_eq!(l.pixel_at(25, 15), Some(full.get_pixel(5, 5).0));
        assert_eq!(l.pixel_at(60, 15), Some(original.get_pixel(40, 5).0));
        // Undone one by one.
        assert!(doc.undo());
        assert_eq!(
            doc.active_layer().pixel_at(60, 15),
            Some(full.get_pixel(40, 5).0)
        );
    }

    #[test]
    fn painting_the_filter_mask_shows_through() {
        let mut doc = doc();
        let original = doc.active_layer().image().clone();
        doc.edit_smart(1, "Mosaic".into(), |s| {
            s.filters = vec![SmartFilter::new(Filter::Mosaic { cell: 8.0 })]
        });
        let mut full = original.clone();
        Filter::Mosaic { cell: 8.0 }.apply(&mut full, (20, 10));
        assert!(doc.add_filter_mask(1, None));
        doc.target_filter_mask(true);
        assert!(doc.filter_mask_targeted() && doc.can_paint());
        // Black over the left column: the filter doesn't apply there.
        assert!(doc.begin_paint());
        {
            let stand_in = doc.active_layer_mut();
            let at = stand_in.offset;
            let image = stand_in.pixels_mut();
            for y in 0..image.height() {
                for x in 0..image.width() {
                    if (at.0 + x as i64) == 20 {
                        image.put_pixel(x, y, Rgba([0, 0, 0, 255]));
                    }
                }
            }
        }
        doc.sync_paint(None);
        let shown = doc.render_layers()[1].pixel_at(20, 12);
        assert_eq!(
            shown,
            Some(original.get_pixel(0, 2).0),
            "shown while painting"
        );
        doc.end_paint();
        let l = doc.active_layer();
        assert!(l.is_smart());
        assert_eq!(l.pixel_at(20, 12), Some(original.get_pixel(0, 2).0));
        assert_eq!(l.pixel_at(21, 12), Some(full.get_pixel(1, 2).0));
        assert!(l.mask.is_none(), "the layer mask is another one");
    }

    #[test]
    fn edit_contents_updates_every_instance() {
        let mut doc = doc();
        doc.duplicate_active();
        doc.new_smart_via_copy();
        assert_eq!(doc.layers.len(), 4);
        let mut inner = doc.smart_contents(1).unwrap();
        let link = inner.embedded_in.clone().unwrap();
        assert_eq!(link.parent, doc.uid());
        inner.add_layer(
            "red".into(),
            RgbaImage::from_pixel(64, 48, Rgba([255, 0, 0, 255])),
            (0, 0),
        );
        let contents = inner.to_contents(0, String::new());
        assert!(doc.update_contents(link.contents, contents));
        let red = Some([255, 0, 0, 255]);
        assert_eq!(doc.layers[1].pixel_at(30, 20), red);
        assert_eq!(
            doc.layers[2].pixel_at(30, 20),
            red,
            "Duplicate Layer: an instance"
        );
        assert_ne!(
            doc.layers[3].pixel_at(30, 20),
            red,
            "via Copy: contents of its own"
        );
        assert_eq!(
            doc.layers[1].smart.as_ref().unwrap().contents.name,
            "pattern"
        );
        assert!(doc.undo());
        assert_ne!(doc.layers[1].pixel_at(30, 20), red);
    }

    #[test]
    fn painting_and_pixel_operations_leave_it_alone() {
        let mut doc = doc();
        assert!(!doc.can_paint());
        assert!(!doc.begin_paint());
        assert!(!doc.adjust_active(&Adjustment::Invert, None));
        assert!(doc.active_layer().is_smart());
        assert!(doc.rasterize_active());
        assert!(!doc.active_layer().is_smart());
        assert!(doc.can_paint());
    }

    #[test]
    fn placed_files_linked_or_not() {
        let dir = std::env::temp_dir().join("atelier-smart-place");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("big.png");
        RgbaImage::from_pixel(400, 300, Rgba([0, 0, 255, 255]))
            .save(&path)
            .unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(200, 150, Rgba([255; 4])), None);
        assert!(!doc.place_file(&path, false).unwrap());
        let l = doc.active_layer();
        assert_eq!(l.name, "big");
        assert_eq!(
            l.rect(),
            PixelRect {
                x: 0,
                y: 0,
                w: 200,
                h: 150
            },
            "fitted"
        );
        assert_eq!(l.smart.as_ref().unwrap().contents.size(), (400, 300));
        // Linked: follows the file, keeps its copy when it is gone.
        assert!(!doc.place_file(&path, true).unwrap());
        assert_eq!(doc.update_links(), LinkReport::default());
        std::thread::sleep(std::time::Duration::from_millis(20));
        RgbaImage::from_pixel(400, 300, Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let s = doc.active_layer().smart.as_deref().unwrap();
        let stamp = s.link.as_ref().unwrap().modified;
        if modified_time(&path) != stamp {
            let report = doc.update_links();
            assert_eq!(report.updated, vec!["big".to_string()]);
            assert_eq!(doc.active_layer().pixel_at(100, 75), Some([255, 0, 0, 255]));
            assert_eq!(
                doc.layers[1].pixel_at(100, 75),
                Some([0, 0, 255, 255]),
                "embedded"
            );
        }
        std::fs::remove_file(&path).unwrap();
        let report = doc.update_links();
        assert_eq!(report.missing, vec!["big".to_string()]);
        let s = doc.active_layer().smart.as_deref().unwrap();
        assert!(s.link.as_ref().unwrap().missing);
        assert!(
            doc.active_layer().pixel_at(100, 75).is_some(),
            "its last copy"
        );
    }

    /// Placed with a layer of an artboard active: fitted and centered in
    /// that artboard.
    #[test]
    fn placed_files_fit_the_active_artboard() {
        let dir = std::env::temp_dir().join("atelier-smart-place-artboard");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photo.png");
        RgbaImage::from_pixel(1920, 1080, Rgba([200, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let white = crate::document::ArtboardBackground::White;
        let mut doc = Document::with_artboard(1000, 1000, white);
        let name = doc.new_artboard_name();
        let board = PixelRect {
            x: 1100,
            y: 0,
            w: 1000,
            h: 1000,
        };
        doc.add_artboard("new".into(), name, board, white);
        doc.active = doc.artboards()[1];
        doc.place_file(&path, false).unwrap();
        assert_eq!(doc.artboard_of(doc.active), Some(doc.artboards()[1]));
        let r = doc.active_layer().rect();
        assert_eq!((r.x, r.w), (1100, 1000), "{r:?}");
        assert!(r.y > 0 && r.bottom() < 1000, "{r:?}");
    }

    /// Linked files read on another thread: a smart object changed
    /// meanwhile is rendered again as it is now; a missing file changes
    /// nothing in the document.
    #[test]
    fn links_read_later_and_missing_files() {
        let dir = std::env::temp_dir().join("atelier-smart-links-later");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.png");
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
            .save(&path)
            .unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.place_file(&path, true).unwrap();
        doc.modified = false;
        // Changed: read, while the smart object moves.
        if let Some(l) = doc
            .active_layer_mut()
            .smart
            .as_deref_mut()
            .and_then(|s| s.link.as_mut())
        {
            l.modified = None;
        }
        RgbaImage::from_pixel(20, 20, Rgba([0, 255, 0, 255]))
            .save(&path)
            .unwrap();
        let reads = doc.stale_links(false);
        assert_eq!(reads.len(), 1);
        let fresh: Vec<LinkFresh> = reads.into_iter().map(LinkRead::run).collect();
        let i = doc.active;
        let before = doc.active_layer().rect();
        doc.edit_smart(i, "move".into(), |s| s.map = shift_map(s.map, [10.0, 0.0]));
        let moved = doc.active_layer().rect();
        assert_eq!(moved.x, before.x + 10);
        let report = doc.put_links(fresh);
        assert_eq!(report.updated.len(), 1);
        assert_eq!(doc.active_layer().rect(), moved, "the move kept");
        let (x, y) = (moved.x + 5, moved.y + 5);
        assert_eq!(doc.active_layer().pixel_at(x, y), Some([0, 255, 0, 255]));
        let report = doc.put_links(
            doc.stale_links(false)
                .into_iter()
                .map(LinkRead::run)
                .collect(),
        );
        assert_eq!(report, LinkReport::default(), "up to date");
        // Missing: reported, nothing to save.
        doc.modified = false;
        std::fs::remove_file(&path).unwrap();
        assert_eq!(doc.update_links().missing.len(), 1);
        assert!(!doc.modified);
        // Being written (not an image yet): could not be read, not missing.
        std::fs::write(&path, b"half").unwrap();
        let report = doc.update_links();
        assert_eq!((report.missing.len(), report.unreadable.len()), (0, 1));
    }

    /// Instances of a linked file: read once, their contents still
    /// shared (one copy in memory), each rendered with its own transform.
    #[test]
    fn instances_of_a_link_read_once() {
        let dir = std::env::temp_dir().join("atelier-smart-links-instances");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.png");
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
            .save(&path)
            .unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.place_file(&path, true).unwrap();
        doc.duplicate_active();
        let i = doc.active;
        doc.edit_smart(i, "move".into(), |s| s.map = shift_map(s.map, [30.0, 0.0]));
        std::thread::sleep(std::time::Duration::from_millis(20));
        RgbaImage::from_pixel(20, 20, Rgba([0, 255, 0, 255]))
            .save(&path)
            .unwrap();
        for l in &mut doc.layers {
            if let Some(link) = l.smart.as_deref_mut().and_then(|s| s.link.as_mut()) {
                link.modified = None;
            }
        }
        let reads = doc.stale_links(false);
        assert_eq!(reads.len(), 1, "one read");
        assert_eq!(reads[0].instances.len(), 2);
        let report = doc.put_links(reads.into_iter().map(LinkRead::run).collect());
        assert_eq!(report.updated.len(), 2);
        let flats: Vec<&Arc<RgbaImage>> = doc
            .layers
            .iter()
            .filter_map(|l| l.smart.as_deref().map(|s| &s.contents.flat))
            .collect();
        assert!(Arc::ptr_eq(flats[0], flats[1]), "shared");
        for l in doc.layers.iter().filter(|l| l.is_smart()) {
            let r = l.rect();
            assert_eq!(l.pixel_at(r.x + 5, r.y + 5), Some([0, 255, 0, 255]));
        }
    }

    /// A link to another machine isn't looked at by itself (that would
    /// connect to it, maybe for long), only when asked.
    #[test]
    fn network_links_looked_at_only_when_asked() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        let dir = std::env::temp_dir().join("atelier-smart-links-network");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("logo.png");
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
            .save(&path)
            .unwrap();
        doc.place_file(&path, true).unwrap();
        if let Some(link) = doc
            .active_layer_mut()
            .smart
            .as_deref_mut()
            .and_then(|s| s.link.as_mut())
        {
            link.path = PathBuf::from(r"\\host\share\logo.png");
        }
        assert!(doc.stale_links(false).is_empty());
        assert_eq!(doc.stale_links(true).len(), 1);
    }

    #[test]
    fn convert_to_profile_converts_the_contents() {
        use crate::cms::{self, Conversion, Intent, WorkingRgb};
        let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba([255; 4])), None);
        doc.add_layer(
            "red".into(),
            RgbaImage::from_pixel(20, 10, Rgba([255, 0, 0, 255])),
            (5, 5),
        );
        assert!(doc.convert_to_smart(&[1]));
        let adobe = WorkingRgb::AdobeRgb.profile();
        doc.convert_to_profile(&Conversion {
            from: cms::srgb(),
            to: adobe,
            intent: Intent::RelativeColorimetric,
            bpc: true,
        });
        let shown = doc.active_layer().pixel_at(10, 10).unwrap();
        assert!(shown[0] < 230, "{shown:?}");
        // Rendered again from its contents: the same colors.
        let canvas = doc.full_rect();
        doc.active_layer_mut().render_smart(canvas);
        assert_eq!(doc.active_layer().pixel_at(10, 10), Some(shown));
        let inner = doc.smart_contents(1).unwrap();
        assert_eq!(inner.layers[0].pixel_at(5, 5), Some(shown));
        assert_eq!(inner.icc_profile.as_deref(), Some(adobe.icc().as_ref()));
        assert!(doc.undo());
        assert_eq!(doc.active_layer().pixel_at(10, 10), Some([255, 0, 0, 255]));
    }

    #[test]
    fn fitted_warps_go_through_their_points() {
        let w = Warp::from_map(&Homography::affine([30.0, 5.0, 2.0, -4.0, 20.0, 7.0])).unwrap();
        let mut bent = w;
        bent.drag((0.4, 0.6), [3.0, -2.0]);
        let fitted = fit_warp(|u, v| Some(bent.eval(u, v))).unwrap();
        for (a, b) in fitted.0.iter().flatten().zip(bent.0.iter().flatten()) {
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("atelier-smart5-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A linked logo.png (blue) placed, and an instance of it moved aside.
    fn two_instances(dir: &Path) -> (Document, PathBuf) {
        let path = dir.join("logo.png");
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255])).save(&path).unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.place_file(&path, true).unwrap();
        doc.duplicate_active();
        let i = doc.active;
        doc.edit_smart(i, "move".into(), |s| s.map = shift_map(s.map, [40.0, 0.0]));
        (doc, path)
    }

    fn pixels(doc: &Document) -> Vec<([u8; 4], [u8; 4], bool)> {
        doc.layers
            .iter()
            .filter_map(|l| {
                let s = l.smart.as_deref()?;
                let r = l.rect();
                Some((l.pixel_at(r.x + 5, r.y + 5)?, s.contents.flat.get_pixel(5, 5).0, s.link.is_some()))
            })
            .collect()
    }

    /// Embed Linked on one instance embeds the instances of its contents
    /// together: the file changing afterwards changes none of them.
    #[test]
    fn embed_linked_embeds_the_instances_together() {
        let dir = temp("embed-together");
        let (mut doc, path) = two_instances(&dir);
        let i = doc.active;
        let depth = doc.history().1;
        assert!(doc.embed_linked(i, "embed".into()));
        assert_eq!(doc.history().1, depth + 1, "one step");
        std::thread::sleep(std::time::Duration::from_millis(20));
        RgbaImage::from_pixel(20, 20, Rgba([0, 255, 0, 255])).save(&path).unwrap();
        assert_eq!(doc.update_links(), LinkReport::default());
        let blue = [0, 0, 255, 255];
        assert_eq!(pixels(&doc), vec![(blue, blue, false); 2]);
        assert!(!doc.embed_linked(i, "again".into()), "no longer linked");
    }

    /// Smart objects that differ under one contents id (an older version's
    /// Embed Linked of one instance, its link then updated): each saved
    /// with its own contents.
    #[test]
    fn diverged_instances_each_saved_with_their_contents() {
        let dir = temp("diverged");
        let (mut doc, path) = two_instances(&dir);
        let i = doc.active;
        doc.edit_smart(i, "embed".into(), |s| s.link = None);
        std::thread::sleep(std::time::Duration::from_millis(20));
        RgbaImage::from_pixel(20, 20, Rgba([0, 255, 0, 255])).save(&path).unwrap();
        for l in &mut doc.layers {
            if let Some(link) = l.smart.as_deref_mut().and_then(|s| s.link.as_mut()) {
                link.modified = None;
            }
        }
        assert_eq!(doc.update_links().updated.len(), 1);
        let before = pixels(&doc);
        let (blue, green) = ([0, 0, 255, 255], [0, 255, 0, 255]);
        assert_eq!(before, vec![(green, green, true), (blue, blue, false)]);
        let saved = dir.join("doc.atelier");
        doc.save(&saved).unwrap();
        std::fs::remove_file(&path).unwrap();
        let back = Document::open(&saved).unwrap().doc;
        assert_eq!(pixels(&back), before);
        let ids: Vec<u64> = back.layers.iter().filter_map(|l| Some(l.smart.as_deref()?.contents.id)).collect();
        assert_ne!(ids[0], ids[1], "apart from now on");
    }

    /// Edit Contents saved for contents whose instances are linked to a
    /// file that is there and embedded: the embedded one updated, the
    /// linked one left following its file (still linked, its contents and
    /// rendering unchanged, under an id of its own).
    #[test]
    fn update_contents_leaves_a_present_link_alone() {
        let dir = temp("update-present");
        let (mut doc, _path) = two_instances(&dir);
        let i = doc.active;
        doc.edit_smart(i, "embed".into(), |s| s.link = None);
        let id = doc.active_layer().smart.as_deref().unwrap().contents.id;
        let red = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255, 0, 0, 255])), None);
        assert!(doc.update_contents(id, red.to_contents(id, "x".into())));
        let (blue, red) = ([0, 0, 255, 255], [255, 0, 0, 255]);
        assert_eq!(pixels(&doc), vec![(blue, blue, true), (red, red, false)]);
        let ids: Vec<u64> = doc.layers.iter().filter_map(|l| Some(l.smart.as_deref()?.contents.id)).collect();
        assert_ne!(ids[0], id);
        assert_eq!(ids[1], id);
        // Its file gone: updated, and embedded.
        let (mut doc, path) = two_instances(&temp("update-missing"));
        std::fs::remove_file(&path).unwrap();
        let id = doc.active_layer().smart.as_deref().unwrap().contents.id;
        let red_doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba(red)), None);
        assert!(doc.update_contents(id, red_doc.to_contents(id, "x".into())));
        assert_eq!(pixels(&doc), vec![(red, red, false); 2]);
    }

    /// Replace Contents gives the instances new contents under a new id
    /// (shared): an Edit Contents tab of the old ones finds nothing to
    /// save over.
    #[test]
    fn replace_contents_takes_a_new_id() {
        let dir = temp("replace-id");
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
        doc.add_layer("a".into(), RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255])), (0, 0));
        assert!(doc.convert_to_smart(&[doc.active]));
        doc.duplicate_active();
        let old = doc.active_layer().smart.as_deref().unwrap().contents.id;
        let other = dir.join("other.png");
        RgbaImage::from_pixel(20, 20, Rgba([0, 255, 0, 255])).save(&other).unwrap();
        doc.replace_contents(&other).unwrap();
        let ids: Vec<u64> = doc.layers.iter().filter_map(|l| Some(l.smart.as_deref()?.contents.id)).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0] == ids[1] && ids[0] != old);
        let stale = Document::new(RgbaImage::from_pixel(20, 20, Rgba([255, 0, 0, 255])), None);
        assert!(!doc.update_contents(old, stale.to_contents(old, "x".into())));
    }

    /// A missing link is reported once; looked at again, it is still
    /// missing, not reported as newly missing.
    #[test]
    fn a_missing_link_is_reported_once() {
        let (mut doc, path) = two_instances(&temp("missing-once"));
        std::fs::remove_file(&path).unwrap();
        let first = doc.update_links();
        assert_eq!((first.missing.len(), first.still_missing.len()), (2, 0));
        let again = doc.update_links();
        assert_eq!((again.missing.len(), again.still_missing.len()), (0, 2));
    }
}
