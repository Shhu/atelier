//! What the canvas shows on the GPU: the composited document as tiles,
//! images of any size (split into textures the GPU accepts), and the
//! selection outline.

use egui::epaint::Mesh;
use image::RgbaImage;

use super::*;
use crate::compose;
use crate::document::Layer;
use crate::xform;

/// An image of any size on the GPU: textures no larger than the GPU allows,
/// side by side, and copies of it reduced 2, 4, 8… times (box filter), drawn
/// when zoomed out: the GPU itself only blends 4 pixels, so reduced too much
/// an image shimmers.
pub(super) struct GpuImage {
    name: String,
    size: [u32; 2],
    /// Side of the textures.
    tile: u32,
    /// Row by row.
    tiles: Vec<TextureHandle>,
    /// The image reduced 2, 4… times.
    levels: Vec<GpuImage>,
}

impl GpuImage {
    pub(super) fn new(ctx: &egui::Context, name: String, image: &RgbaImage, levels: usize) -> Self {
        // At most 8192 px (256 MiB), more likely to fit in GPU memory than one
        // huge texture.
        let tile = ctx.input(|i| i.max_texture_side).clamp(1, 8192) as u32;
        let mut gpu = Self {
            name,
            size: [0, 0],
            tile,
            tiles: Vec::new(),
            levels: Vec::new(),
        };
        gpu.set(ctx, image, levels);
        gpu
    }

    /// Area of each texture in the image: x, y, width, height.
    fn tile_rects(&self) -> impl Iterator<Item = [u32; 4]> + use<> {
        let ([w, h], t) = (self.size, self.tile);
        (0..h.div_ceil(t)).flat_map(move |row| {
            (0..w.div_ceil(t)).map(move |col| {
                let (x, y) = (col * t, row * t);
                [x, y, t.min(w - x), t.min(h - y)]
            })
        })
    }

    /// Sends the whole image, whose size may have changed, with `levels`
    /// reduced copies.
    pub(super) fn set(&mut self, ctx: &egui::Context, image: &RgbaImage, levels: usize) {
        self.size = [image.width(), image.height()];
        let rects: Vec<_> = self.tile_rects().collect();
        self.tiles.truncate(rects.len());
        for (i, r) in rects.into_iter().enumerate() {
            let pixels = color_region(image, r);
            match self.tiles.get_mut(i) {
                Some(tex) => tex.set(pixels, IMAGE_TEX_OPTIONS),
                None => {
                    let name = format!("{}-{i}", self.name);
                    self.tiles
                        .push(ctx.load_texture(name, pixels, IMAGE_TEX_OPTIONS));
                }
            }
        }
        self.levels.clear();
        self.set_levels(ctx, image, levels);
    }

    /// Keeps `levels` reduced copies of `image`, the image shown.
    pub(super) fn set_levels(&mut self, ctx: &egui::Context, image: &RgbaImage, levels: usize) {
        self.levels.truncate(levels);
        for k in self.levels.len() + 1..=levels {
            let reduced = reduce(image, (0, 0), self.size, 1 << k, [0, 0, u32::MAX, u32::MAX]);
            let name = format!("{}-{k}", self.name);
            self.levels.push(GpuImage::new(ctx, name, &reduced, 0));
        }
    }

    /// Draws the part `uv` (0..1) of the image into `dest` (screen), rotated
    /// by `angle` degrees (clockwise) around `pivot`, from the reduced copy
    /// that suits its size.
    pub(super) fn paint(
        &self,
        painter: &Painter,
        dest: Rect,
        uv: Rect,
        tint: Color32,
        angle: f32,
        pivot: Pos2,
    ) {
        let size = vec2(self.size[0] as f32, self.size[1] as f32);
        let src = Rect::from_min_max(
            (uv.min.to_vec2() * size).to_pos2(),
            (uv.max.to_vec2() * size).to_pos2(),
        );
        if !src.is_positive() || !dest.is_positive() {
            return;
        }
        // Image pixels per pixel of the screen.
        let shrink = (src.size() / (dest.size() * painter.ctx().pixels_per_point())).max_elem();
        let k = (shrink.log2().floor().max(0.0) as usize).min(self.levels.len());
        if k > 0 {
            let level = &self.levels[k - 1];
            // Its pixels cover `b` pixels of the image, the last ones fewer.
            let b = (1u32 << k) as f32;
            let scale = size / (vec2(level.size[0] as f32, level.size[1] as f32) * b);
            let uv = Rect::from_min_max(
                (uv.min.to_vec2() * scale).to_pos2(),
                (uv.max.to_vec2() * scale).to_pos2(),
            );
            return level.paint(painter, dest, uv, tint, angle, pivot);
        }
        let scale = dest.size() / src.size();
        let to_dest = |p: Pos2| dest.min + (p - src.min) * scale;
        let rotation = egui::emath::Rot2::from_angle(angle.to_radians());
        for (tex, [x, y, w, h]) in self.tiles.iter().zip(self.tile_rects()) {
            let tile = Rect::from_min_size(pos2(x as f32, y as f32), vec2(w as f32, h as f32));
            let part = tile.intersect(src);
            if !part.is_positive() {
                continue;
            }
            let rect = Rect::from_min_max(to_dest(part.min), to_dest(part.max));
            if angle == 0.0 && !rect.intersects(painter.clip_rect()) {
                continue;
            }
            let tile_uv = Rect::from_min_max(
                ((part.min - tile.min) / tile.size()).to_pos2(),
                ((part.max - tile.min) / tile.size()).to_pos2(),
            );
            let mut mesh = Mesh::with_texture(tex.id());
            mesh.add_rect_with_uv(rect, tile_uv, tint);
            if angle != 0.0 {
                mesh.rotate(rotation, pivot);
            }
            painter.add(egui::Shape::mesh(mesh));
        }
    }

    /// Draws the whole image into `dest`.
    pub(super) fn paint_all(&self, painter: &Painter, dest: Rect, tint: Color32) {
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        self.paint(painter, dest, uv, tint, 0.0, dest.center());
    }
}

/// Number of reduced copies to keep for a zoom (physical pixels per image
/// pixel): enough to never reduce one more than twice. At exactly 50 %, the
/// copy reduced twice is shown pixel for pixel.
pub(super) fn levels_for(zoom: f32) -> usize {
    if zoom > 0.5 {
        0
    } else {
        ((1.0 / zoom).log2().floor() as usize).min(8)
    }
}

/// Rows per band to share `h` rows of `w` pixels among the cores (all in one
/// band when there are few pixels).
fn band_rows(w: usize, h: usize) -> usize {
    const MIN_BAND: usize = 1 << 16;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    h.div_ceil(threads).max(MIN_BAND.div_ceil(w.max(1))).max(1)
}

/// Runs `f(first row, rows)` on bands of the `h` rows of `w` pixels of
/// `out` (`per_row` values each), on all the cores when there are many.
fn in_bands<T: Send>(
    out: &mut [T],
    w: usize,
    h: usize,
    per_row: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) {
    let rows = band_rows(w, h);
    if rows >= h {
        return f(0, out);
    }
    std::thread::scope(|s| {
        for (k, band) in out.chunks_mut(rows * per_row).enumerate() {
            let f = &f;
            s.spawn(move || f(k * rows, band));
        }
    });
}

/// Area `[x, y, w, h]` of an image, for the GPU (premultiplied colors).
fn color_region(image: &RgbaImage, [x, y, w, h]: [u32; 4]) -> ColorImage {
    let (stride, x, w) = (image.width() as usize * 4, x as usize * 4, w as usize);
    let raw = image.as_raw();
    let mut pixels = vec![Color32::TRANSPARENT; w * h as usize];
    if w > 0 {
        in_bands(&mut pixels, w, h as usize, w, |first, band| {
            for (i, row) in band.chunks_exact_mut(w).enumerate() {
                let start = (y as usize + first + i) * stride + x;
                let src = raw[start..start + w * 4].chunks_exact(4);
                for (to, p) in row.iter_mut().zip(src) {
                    *to = Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]);
                }
            }
        });
    }
    ColorImage::new([w, h as usize], pixels)
}

/// Area `[x, y, w, h]` (cut to its size) of an image of `size` reduced
/// `block` times: each pixel is the average of a block (premultiplied
/// colors). `source` holds the image's pixel (0, 0) at `offset`.
fn reduce(
    source: &RgbaImage,
    offset: (i64, i64),
    size: [u32; 2],
    block: u32,
    area: [u32; 4],
) -> RgbaImage {
    let [lw, lh] = size.map(|s| s.div_ceil(block));
    let (x, y) = (area[0].min(lw), area[1].min(lh));
    let (w, h) = (area[2].min(lw - x) as usize, area[3].min(lh - y) as usize);
    let mut out = vec![0u8; w * h * 4];
    let (stride, b) = (source.width() as usize * 4, block as usize);
    let raw = source.as_raw();
    in_bands(&mut out, w, h, w * 4, |first, band| {
        for (i, row) in band.chunks_exact_mut(w * 4).enumerate() {
            let ly = y as usize + first + i;
            let rows = ly * b..((ly + 1) * b).min(size[1] as usize);
            for (j, px) in row.chunks_exact_mut(4).enumerate() {
                let lx = x as usize + j;
                let cols = lx * b..((lx + 1) * b).min(size[0] as usize);
                let mut sum = [0u32; 4];
                for sy in rows.clone() {
                    let start = (sy as i64 + offset.1) as usize * stride;
                    for sx in cols.clone() {
                        let p = &raw[start + (sx as i64 + offset.0) as usize * 4..][..4];
                        let a = p[3] as u32;
                        sum[0] += p[0] as u32 * a;
                        sum[1] += p[1] as u32 * a;
                        sum[2] += p[2] as u32 * a;
                        sum[3] += a;
                    }
                }
                // Fully transparent: [0, 0, 0, 0].
                for c in 0..3 {
                    px[c] = (sum[c] + sum[3] / 2).checked_div(sum[3]).unwrap_or(0) as u8;
                }
                let n = (rows.len() * cols.len()) as u32;
                px[3] = ((sum[3] + n / 2) / n) as u8;
            }
        }
    });
    RgbaImage::from_raw(w as u32, h as u32, out).expect("size")
}

/// Preview of the open dialog shown on a layer.
pub(super) struct ShownPreview {
    pub(super) preview: LayerPreview,
    pub(super) layer: u64,
    /// Revision of the layer's pixels it was computed from.
    pub(super) rev: u64,
    /// Revision of its own for the previewed pixels (never a layer's).
    pub(super) fingerprint: u64,
    pub(super) image: Arc<RgbaImage>,
    /// When the mask is the target: `image` as the mask, and its canvas
    /// origin.
    pub(super) mask: Option<(Arc<image::GrayImage>, (i64, i64))>,
    /// That mask is the Quick Mask's: shown by its overlay, not on the
    /// layer.
    pub(super) quick_mask: bool,
}

impl ShownPreview {
    pub(super) fn new(preview: LayerPreview, layer: &Layer, image: RgbaImage) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1 << 63);
        Self {
            preview,
            layer: layer.id,
            rev: layer.rev(),
            fingerprint: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            image: Arc::new(image),
            mask: None,
            quick_mask: false,
        }
    }

    /// The previewed image is the Quick Mask's (gray, at `origin`).
    pub(super) fn on_quick_mask(self, origin: (i64, i64)) -> Self {
        Self {
            quick_mask: true,
            ..self.on_mask(origin)
        }
    }

    /// The previewed image is the mask's (gray), placed at `origin`.
    pub(super) fn on_mask(mut self, origin: (i64, i64)) -> Self {
        let (w, h) = self.image.dimensions();
        let mut gray = image::GrayImage::new(w, h);
        // On all the cores: called on the UI thread for every preview.
        let src = self.image.as_raw();
        crate::mask_ops::par_rows(gray.as_mut(), w as usize, |y0, band| {
            let from = &src[y0 * w as usize * 4..];
            for (v, p) in band.iter_mut().zip(from.chunks_exact(4)) {
                let [r, g, b, a] = [p[0], p[1], p[2], p[3]].map(u32::from);
                let luma = (299 * r + 587 * g + 114 * b + 500) / 1000;
                *v = ((luma * a + 127) / 255) as u8;
            }
        });
        self.mask = Some((Arc::new(gray), origin));
        self
    }

    /// What the compositing shows instead of the document (the layer being
    /// at `doc_offset`).
    pub(super) fn options<'a>(&'a self, opts: &mut compose::Options<'a>, doc_offset: (i64, i64)) {
        if self.quick_mask {
            return;
        }
        let (layer, version) = (self.layer, self.fingerprint);
        match &self.mask {
            Some((image, origin)) => {
                let origin = *origin;
                opts.mask = Some(compose::Preview {
                    layer,
                    version,
                    image,
                    origin,
                });
            }
            None => {
                let origin = doc_offset;
                let image = &self.image;
                opts.pixels = Some(compose::Preview {
                    layer,
                    version,
                    image,
                    origin,
                });
            }
        }
    }
}

/// The document as shown: composited tiles of the visible area, at the
/// level of reduction suited to the zoom, kept on the GPU and composited
/// again only where something changed since the last frame.
pub(super) struct CanvasView {
    /// Document shown (its uid), and its canvas (gradient fills span it).
    doc: u64,
    canvas: PixelRect,
    level: u32,
    /// Color channel shown alone, in gray (Channels panel).
    channel: Option<usize>,
    /// What the colors are shown through (see `ColorUi::canvas_lut`).
    display: u64,
    sources: compose::LevelCache,
    /// The reductions of the level shown before, to go back to it at once.
    spare: Option<compose::LevelCache>,
    /// The reductions of this level being made on a worker thread (the
    /// level changed): meanwhile, `stale` shows.
    pending: Option<std::thread::JoinHandle<compose::LevelCache>>,
    /// The layers as last composited.
    sigs: Vec<compose::Sig>,
    /// Tiles (column, row, in tiles of the level).
    tiles: Tiles,
    /// The tiles of the level shown before, drawn where this one has none
    /// yet (never an empty canvas while zooming): their level, and them.
    stale: Option<(u32, Tiles)>,
    hot: Option<Hot>,
    /// Frames drawn: the tiles waiting the longest are composited first.
    frame: u64,
    /// The views of other documents (tabs), to come back to them at once.
    others: Vec<CanvasView>,
    /// A change of much of the view being composited over several frames,
    /// shown at once when done.
    generation: Option<Generation>,
    /// The active layer and the versions of its pixels and mask at the
    /// last frame: where the next frame's brush hints start from.
    seen: Option<(u64, [u64; 2])>,
}

/// The tiles of a view (column, row, in tiles of its level).
type Tiles = HashMap<(i64, i64), Tile>;

/// A tile to composite: which, the part of it, and its urgency (see
/// `draw_document`).
type Todo = ((i64, i64), PixelRect, (bool, u64, i64));

/// Parts of a snapshot made by bands: their first row, and their pixels.
type SnapParts = HashMap<(i64, i64), Vec<(i64, Vec<Snap>)>>;

/// A composited tile on the GPU.
struct Tile {
    tex: TextureHandle,
    /// Part to composite again (pixels of the level), if any.
    dirty: Option<PixelRect>,
    /// Frame since which it waits.
    since: u64,
}

/// Composite of the nodes under the one that keeps changing (opacity
/// slider, drag, brush stroke…), per tile, kept while only that one does.
struct Hot {
    node: usize,
    /// Fingerprint of the layers under it.
    below: u64,
    snaps: HashMap<(i64, i64), Arc<Vec<Snap>>>,
}

/// A premultiplied pixel of a snapshot, in 16 bits (half the memory of
/// floats, and as precise as the screen needs).
type Snap = [u16; 4];

fn to_snap(px: &[compose::blend::Px]) -> Vec<Snap> {
    px.iter()
        .map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16))
        .collect()
}

fn from_snap(snap: &[Snap]) -> Vec<compose::blend::Px> {
    snap.iter().map(|p| p.map(|v| v as f32 / 65535.0)).collect()
}

/// Composites the tiles still to do for at most this long per frame, the
/// rest on the next ones (the view stays responsive).
const TILE_BUDGET: std::time::Duration = std::time::Duration::from_millis(30);

/// Below this many layer pixels, a new level's reductions are made at
/// once (see [`view_for`]).
const AT_ONCE_PIXELS: u64 = 1 << 22;

/// Views of other documents kept (see [`CanvasView::others`]).
const KEPT_VIEWS: usize = 2;

impl CanvasView {
    fn new(doc: u64, canvas: PixelRect, level: u32, channel: Option<usize>, display: u64) -> Self {
        Self {
            doc,
            canvas,
            level,
            channel,
            display,
            sources: compose::LevelCache::new(level),
            spare: None,
            pending: None,
            sigs: Vec::new(),
            tiles: HashMap::new(),
            stale: None,
            hot: None,
            frame: 0,
            others: Vec::new(),
            generation: None,
            seen: None,
        }
    }

    /// Memory its reductions, tiles and snapshots take, with those of the
    /// views of other documents kept.
    fn bytes(&self) -> usize {
        let tile = (compose::TILE * compose::TILE * 4) as usize;
        let tiles = self.tiles.len() + self.stale.as_ref().map_or(0, |(_, t)| t.len());
        let snaps = self.hot.as_ref().map_or(0, |h| {
            h.snaps.len() * (compose::TILE * compose::TILE) as usize * size_of::<Snap>()
        });
        self.sources.bytes()
            + self.spare.as_ref().map_or(0, compose::LevelCache::bytes)
            + tiles * tile
            + snaps
            + self.others.iter().map(Self::bytes).sum::<usize>()
    }

    /// Box of tile `t` (pixels of the level).
    fn tile_rect((tx, ty): (i64, i64)) -> PixelRect {
        let side = compose::TILE as i64;
        PixelRect {
            x: tx * side,
            y: ty * side,
            w: compose::TILE,
            h: compose::TILE,
        }
    }

    /// Marks `area` (pixels of the level) to composite again.
    fn mark(&mut self, area: Option<PixelRect>) {
        let frame = self.frame;
        for (&t, tile) in self.tiles.iter_mut() {
            let own = Self::tile_rect(t);
            let Some(part) = area.map_or(Some(own), |a| intersect(own, a)) else {
                continue;
            };
            if tile.dirty.is_none() {
                tile.since = frame;
            }
            tile.dirty = Some(tile.dirty.map_or(part, |d| d.union(part)));
        }
    }
}

/// Reductions a view of another document keeps (beyond, only its tiles:
/// they are made again on a worker thread when it shows again).
const KEPT_SOURCES_BYTES: usize = 192 << 20;

/// Pixels of `layers` to reduce.
fn layer_pixels(layers: &[Layer]) -> u64 {
    layers
        .iter()
        .map(|l| u64::from(l.image().width()) * u64::from(l.image().height()))
        .sum()
}

/// Makes the reductions of `level` on a worker thread, from those of the
/// level below when given (`seed`).
fn reduce_later(
    seed: Option<compose::LevelCache>,
    level: u32,
    canvas: PixelRect,
    layers: &[Layer],
    held: bool,
) -> Option<std::thread::JoinHandle<compose::LevelCache>> {
    let layers = layers.to_vec();
    let job = std::thread::Builder::new()
        .name("level".into())
        .spawn(move || {
            let mut cache = match seed {
                Some(finer) => finer.coarser(),
                None => compose::LevelCache::new(level),
            };
            let opts = compose::Options {
                held,
                later_styles: true,
                ..Default::default()
            };
            compose::plan(&layers, canvas, &mut cache, &opts);
            cache
        });
    job.ok()
}

/// `slot` brought to show document `uid` at `level`… (see
/// [`CanvasView`]). Another level of the same document: its reductions are
/// made on a worker thread from `layers` while the former tiles show; so
/// are those of a document shown again or for the first time, its kept
/// tiles (if any) showing meanwhile.
#[allow(clippy::too_many_arguments)]
fn view_for(
    slot: &mut Option<CanvasView>,
    uid: u64,
    canvas: PixelRect,
    level: u32,
    channel: Option<usize>,
    display: u64,
    layers: &[Layer],
    held: bool,
) {
    // Another document: its own view if kept, the current one kept.
    if slot.as_ref().is_some_and(|v| v.doc != uid) {
        let mut old = slot.take().expect("checked");
        let mut others = std::mem::take(&mut old.others);
        // Kept lighter: its tiles and the reductions of its level only
        // (those within a budget).
        (old.pending, old.spare, old.stale) = (None, None, None);
        if old.sources.bytes() > KEPT_SOURCES_BYTES {
            old.sources = compose::LevelCache::new(old.level);
        }
        others.insert(0, old);
        let found = others.iter().position(|v| v.doc == uid);
        let mut view = found.map(|k| others.remove(k));
        others.truncate(KEPT_VIEWS);
        if let Some(v) = &mut view {
            v.others = others;
        } else {
            let mut v = CanvasView::new(uid, canvas, level, channel, display);
            v.others = others;
            view = Some(v);
        }
        if let Some(v) = view
            .as_mut()
            .filter(|v| (v.level, v.canvas) == (level, canvas))
            && v.sources.bytes() == 0
            && level > 0
            && layer_pixels(layers) >= AT_ONCE_PIXELS
        {
            v.pending = reduce_later(None, level, canvas, layers, held);
        }
        *slot = view;
    }
    let Some(v) = slot.as_mut() else {
        let mut v = CanvasView::new(uid, canvas, level, channel, display);
        if level > 0 && layer_pixels(layers) >= AT_ONCE_PIXELS {
            v.pending = reduce_later(None, level, canvas, layers, held);
        }
        *slot = Some(v);
        return;
    };
    if v.canvas != canvas {
        let others = std::mem::take(&mut v.others);
        *v = CanvasView::new(uid, canvas, level, channel, display);
        v.others = others;
        return;
    }
    if v.level != level {
        let mut next = CanvasView::new(uid, canvas, level, channel, display);
        next.others = std::mem::take(&mut v.others);
        next.seen = v.seen;
        let old = std::mem::replace(v, next);
        // Its tiles while the new ones come (those of a view that was
        // itself still waiting are older still).
        v.stale = match &old.pending {
            None => Some((old.level, old.tiles)),
            Some(_) => old.stale,
        };
        let (finer, spare) = match (old.pending.is_some(), old.spare) {
            // Still waiting: what it had before is what there is.
            (true, Some(s)) => (s, None),
            (_, spare) => (old.sources, spare),
        };
        // Pixels to reduce (a few milliseconds' worth: at once).
        let work = layer_pixels(layers);
        match spare {
            Some(s) if compose::Sources::level(&s) == level => {
                v.sources = s;
                v.spare = Some(finer);
            }
            _ if level == 0 || work < AT_ONCE_PIXELS => v.spare = Some(finer),
            _ => {
                let seed = (compose::Sources::level(&finer) + 1 == level).then(|| finer.clone());
                v.pending = reduce_later(seed, level, canvas, layers, held);
                v.spare = Some(finer);
            }
        }
        return;
    }
    if (v.channel, v.display) != (channel, display) {
        // The same composite shown otherwise: the tiles stay until redone.
        v.channel = channel;
        v.display = display;
        v.sigs.clear();
    }
}

/// A layer (a group: with its layers) composited on its own, at the level
/// of the view: drawn by the GPU at the place a transform will give it, and
/// faintly where it overflows the canvas while it is dragged.
pub(super) struct Lifted {
    key: u64,
    gpu: GpuImage,
    image: Arc<RgbaImage>,
    level: u32,
    /// Canvas box of the image, and the layer's offset then.
    rect: Rect,
    offset: (i64, i64),
    /// The image moved, scaled and turned as the transform being prepared
    /// will (see [`compose::Placed`]).
    placed: Option<PlacedImage>,
}

/// Largest side of a lifted layer's ghost image on the GPU (see
/// [`Lifted`]).
const GHOST_SIDE: u32 = 2048;

/// A layer as a transform being prepared will place it: fingerprint, image,
/// place (pixels of the level), canvas box.
#[derive(Clone)]
pub(super) struct PlacedImage {
    key: u64,
    image: Arc<RgbaImage>,
    at: (i64, i64),
    rect: PixelRect,
}

/// What the compositing shows differently from the document: the layer
/// left out, a previewed dialog, a blend mode tried, a mask alone.
fn view_options<'a>(
    doc: &'a Document,
    preview: Option<&'a ShownPreview>,
    blend: Option<(u64, BlendMode)>,
    skip: Option<usize>,
    mask_view: bool,
) -> compose::Options<'a> {
    let mut opts = compose::Options {
        held: doc.effects_held(),
        later_styles: true,
        hidden: skip,
        blend,
        mask_view: mask_view.then_some(doc.active),
        ..Default::default()
    };
    if let Some(p) = preview.filter(|p| p.layer == doc.active_layer().id) {
        p.options(&mut opts, doc.active_layer().offset);
    }
    opts
}

impl AtelierApp {
    /// Draws the composited document over `frame` (canvas coordinates),
    /// with layer `skip` left out; composites what changed first.
    pub(super) fn draw_document(
        &mut self,
        ctx: &egui::Context,
        painter: &Painter,
        view: View,
        frame: PixelRect,
        placed: Vec<(usize, PlacedImage)>,
    ) {
        let Self {
            doc,
            canvas_view,
            stroke_dirty,
            preview_shown,
            blend_preview,
            mask_view,
            zoom,
            xform,
            channels,
            color,
            ..
        } = self;
        let Some(doc) = doc else { return };
        let channel = channels.color_shown(doc);
        let (display, lut) = color.canvas_lut(doc);
        // Area painted since the last frame (a mask being painted shows as
        // painted).
        let painted = stroke_dirty.take().map(|r| {
            let layer = doc.active_layer();
            let rect = PixelRect {
                x: layer.offset.0 + r.x as i64,
                y: layer.offset.1 + r.y as i64,
                w: r.w,
                h: r.h,
            };
            doc.sync_paint(Some(rect));
            rect
        });
        let level = render_level(*zoom);
        let doc = &*doc;
        let uid = doc.uid();
        let layers = doc.render_layers();
        let mut opts = view_options(
            doc,
            preview_shown.as_ref(),
            *blend_preview,
            None,
            *mask_view,
        );
        opts.placed = placed
            .iter()
            .map(|(layer, p)| compose::Placed {
                layer: *layer,
                version: p.key,
                image: &p.image,
                at: p.at,
                rect: p.rect,
            })
            .collect();
        opts.placed_mask = xform.mask_preview.as_ref().map(|p| p.placed());
        let canvas = doc.full_rect();
        view_for(
            canvas_view,
            uid,
            canvas,
            level,
            channel,
            display,
            &layers,
            opts.held,
        );
        let cv = canvas_view.as_mut().expect("set above");
        cv.frame += 1;
        // Tiles of the visible part of the frame.
        let b = 1i64 << level;
        let tile = compose::TILE as i64 * b;
        let shown = view
            .rect_to_screen(to_rect(frame))
            .intersect(painter.clip_rect());
        let (a, z) = (view.to_image(shown.min), view.to_image(shown.max));
        let range = |from: f32, to: f32| {
            (from.floor() as i64).div_euclid(tile)..=((to.ceil() as i64 - 1).div_euclid(tile))
        };
        let (cols, rows) = (range(a.x, z.x), range(a.y, z.y));
        let visible: Vec<(i64, i64)> = if shown.is_positive() {
            rows.clone()
                .flat_map(|ty| cols.clone().map(move |tx| (tx, ty)))
                .collect()
        } else {
            Vec::new()
        };
        // The reductions of a new level are being made: the former tiles
        // show meanwhile.
        if let Some(job) = cv.pending.take_if(|j| j.is_finished()) {
            let mut made = job
                .join()
                .unwrap_or_else(|_| compose::LevelCache::new(level));
            // With what was painted meanwhile.
            made.hints_from(&mut cv.sources);
            cv.sources = made;
        }
        // Area painted since the last frame: on the active layer.
        let active_id = doc.active_layer().id;
        let feather = layers[doc.active]
            .mask
            .as_ref()
            .map_or(0, |m| (m.feather * 2.0).ceil() as i64 + 1);
        let strokes: Vec<(u64, PixelRect)> = painted
            .map(|r| {
                let r = PixelRect {
                    x: r.x - feather,
                    y: r.y - feather,
                    w: r.w + 2 * feather as u32,
                    h: r.h + 2 * feather as u32,
                };
                (active_id, r)
            })
            .into_iter()
            .collect();
        // Given to the reductions every frame, also while none are made
        // (nothing to composite in view, a level being made): they apply
        // at the next one.
        let versions = compose::source_versions(&layers[doc.active]);
        let from = cv.seen.filter(|(id, _)| *id == active_id).map(|(_, v)| v);
        cv.seen = Some((active_id, versions));
        for &(layer, rect) in &strokes {
            let hint = compose::Hint {
                layer,
                rect,
                from,
                to: versions,
            };
            cv.sources.hint(hint);
            if let Some(spare) = &mut cv.spare {
                spare.hint(hint);
            }
            // The Channels thumbnails' reductions too.
            if let Some((_, thumbs)) = channels.reduced.as_mut().filter(|(d, _)| *d == uid) {
                thumbs.hint(hint);
            }
        }
        crate::document::note_painted(uid, &strokes);
        if cv.pending.is_some() {
            ctx.request_repaint();
            paint_tiles(painter, view, cv, &visible);
            return;
        }
        let sigs = compose::signatures(&layers, &opts);
        let dirty = if cv.sigs.is_empty() {
            compose::Dirty::All
        } else {
            compose::dirty_area(&cv.sigs, &sigs, &strokes)
        };
        match dirty {
            compose::Dirty::Nothing => {}
            compose::Dirty::All => cv.mark(None),
            compose::Dirty::Area(a) => cv.mark(Some(reduced(a, level))),
        }
        // What to composite, most urgent first: under the brush, then the
        // missing tiles, then those waiting the longest, nearest the
        // pointer first.
        let brush: Vec<PixelRect> = strokes.iter().map(|(_, r)| reduced(*r, level)).collect();
        let focus = ctx
            .pointer_hover_pos()
            .filter(|p| shown.contains(*p))
            .unwrap_or(shown.center());
        let focus = view.to_image(focus) / tile as f32;
        let mut todo: Vec<Todo> = visible
            .iter()
            .filter_map(|&t| {
                let own = CanvasView::tile_rect(t);
                let (part, since) = match cv.tiles.get(&t) {
                    Some(tile) => (tile.dirty?, tile.since),
                    None => (own, 0),
                };
                let under = brush.iter().any(|r| intersect(*r, part).is_some());
                let d = vec2(t.0 as f32 + 0.5 - focus.x, t.1 as f32 + 0.5 - focus.y);
                Some((t, part, (!under, since, (d.length_sq() * 16.0) as i64)))
            })
            .collect();
        todo.sort_by_key(|(_, _, k)| *k);
        let threads = std::thread::available_parallelism().map_or(1, |t| t.get());
        if let Some(mut generation) = cv.generation.take() {
            // A change of much of the view being composited: on with it.
            let over_budget = generation.run(threads, channel, lut.as_deref());
            crate::diag::tiles(generation.last_count, over_budget, level);
            if generation.next < generation.work.len() {
                ctx.request_repaint();
                cv.generation = Some(generation);
            } else {
                upload(ctx, cv, generation.done, false);
            }
        } else if !todo.is_empty() {
            let plan = compose::plan(&layers, canvas, &mut cv.sources, &opts);
            cv.sources.retain_used();
            update_hot(cv, &sigs, &plan, dirty);
            let work = compose_work(cv, &plan, &todo, threads);
            let mut generation = Generation {
                plan,
                work,
                next: 0,
                done: Vec::new(),
                last_count: 0,
            };
            let over_budget = generation.run(threads, channel, lut.as_deref());
            crate::diag::tiles(generation.last_count, over_budget, level);
            if over_budget {
                ctx.request_repaint();
            }
            // A change of much of the view that takes more than a frame:
            // what is done waits for the rest, composited from the same
            // state over the next frames, so the view never shows a
            // patchwork of tiles from several states (an opacity drag, a
            // large layer moved). New tiles show at once.
            let all_shown = generation
                .work
                .iter()
                .all(|w| cv.tiles.contains_key(&w.tile));
            let much = todo.len() >= GENERATION_TILES && todo.len() * 2 > visible.len();
            if over_budget && all_shown && much {
                for w in &generation.work {
                    if let Some(tile) = cv.tiles.get_mut(&w.tile) {
                        tile.dirty = None;
                    }
                }
                cv.generation = Some(generation);
            } else {
                upload(ctx, cv, generation.done, true);
            }
        }
        cv.sigs = sigs;
        // Tiles far from the view go.
        let near = |&(tx, ty): &(i64, i64)| {
            tx >= cols.start() - 2
                && tx <= cols.end() + 2
                && ty >= rows.start() - 2
                && ty <= rows.end() + 2
        };
        if !visible.is_empty() {
            cv.tiles.retain(|t, _| near(t));
        }
        // Snapshots only for what is shown.
        if let Some(h) = &mut cv.hot {
            h.snaps.retain(|t, _| visible.contains(t));
        }
        if visible.iter().all(|t| cv.tiles.contains_key(t)) {
            cv.stale = None;
        }
        // Styles drawn on a worker thread: shown as soon as they are ready.
        if crate::document::styles_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        paint_tiles(painter, view, cv, &visible);
    }
}

/// Draws the tiles of `visible`, and where one is missing, what the former
/// level's tiles show there.
fn paint_tiles(painter: &Painter, view: View, cv: &CanvasView, visible: &[(i64, i64)]) {
    let full = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    let side = |level: u32| (compose::TILE as i64) << level;
    let canvas_rect = |t: (i64, i64), level: u32| {
        let s = side(level);
        Rect::from_min_size(
            pos2((t.0 * s) as f32, (t.1 * s) as f32),
            Vec2::splat(s as f32),
        )
    };
    for &t in visible {
        let r = canvas_rect(t, cv.level);
        if let Some(tile) = cv.tiles.get(&t) {
            painter.image(tile.tex.id(), view.rect_to_screen(r), full, Color32::WHITE);
            continue;
        }
        let Some((level, stale)) = &cv.stale else {
            continue;
        };
        let s = side(*level);
        let (x0, y0) = (
            (r.min.x as i64).div_euclid(s),
            (r.min.y as i64).div_euclid(s),
        );
        let (x1, y1) = (
            (r.max.x as i64 - 1).div_euclid(s),
            (r.max.y as i64 - 1).div_euclid(s),
        );
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                let Some(old) = stale.get(&(tx, ty)) else {
                    continue;
                };
                let o = canvas_rect((tx, ty), *level);
                let part = o.intersect(r);
                if !part.is_positive() {
                    continue;
                }
                let uv = Rect::from_min_max(
                    ((part.min - o.min) / o.size()).to_pos2(),
                    ((part.max - o.min) / o.size()).to_pos2(),
                );
                painter.image(old.tex.id(), view.rect_to_screen(part), uv, Color32::WHITE);
            }
        }
    }
}

/// `r` (canvas pixels) in pixels of `level`: the blocks it touches.
fn reduced(r: PixelRect, level: u32) -> PixelRect {
    let b = 1i64 << level;
    let (x0, y0) = (r.x.div_euclid(b), r.y.div_euclid(b));
    let (x1, y1) = (-(-r.right()).div_euclid(b), -(-r.bottom()).div_euclid(b));
    PixelRect {
        x: x0,
        y: y0,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    }
}

/// Part of a tile to composite: its tile, area (pixels of the level),
/// whether it is the last part of the tile, and the hot node with the
/// snapshot of the nodes under it to start from (`None`: to make).
#[derive(Clone)]
struct Work {
    tile: (i64, i64),
    rect: PixelRect,
    last: bool,
    hot: Option<(usize, Option<Arc<Vec<Snap>>>)>,
}

/// Rows of a band at least (fewer: not worth a thread).
const MIN_BAND_ROWS: u32 = 16;

/// The parts of the tiles `todo` to composite, cut into bands of rows when
/// there are fewer tiles than `threads` (all the cores work on a stroke).
fn compose_work(cv: &CanvasView, plan: &compose::Plan, todo: &[Todo], threads: usize) -> Vec<Work> {
    let n = plan.nodes.len();
    let hot = cv.hot.as_ref().filter(|h| h.node < n);
    let rows: u32 = todo.iter().map(|(_, r, _)| r.h).sum();
    let per = rows.div_ceil(threads.max(1) as u32).max(MIN_BAND_ROWS);
    let mut out = Vec::new();
    for &(t, part, _) in todo {
        let own = CanvasView::tile_rect(t);
        let snap = hot.map(|h| (h.node, h.snaps.get(&t).cloned()));
        // A tile without its snapshot is made whole (the snapshot with it);
        // a new tile too.
        let whole = matches!(snap, Some((_, None))) || !cv.tiles.contains_key(&t);
        let part = if whole { own } else { part };
        let bands = if todo.len() < threads {
            part.h.div_ceil(per)
        } else {
            1
        };
        let h = part.h.div_ceil(bands.max(1));
        let mut y = part.y;
        while y < part.bottom() {
            let rh = h.min((part.bottom() - y) as u32);
            out.push(Work {
                tile: t,
                rect: PixelRect { y, h: rh, ..part },
                last: y + rh as i64 >= part.bottom(),
                hot: snap.clone(),
            });
            y += rh as i64;
        }
    }
    out
}

/// Composites a part of a tile, for the screen.
fn composite_part(
    plan: &compose::Plan,
    work: &Work,
    channel: Option<usize>,
    lut: Option<&crate::cms::Lut>,
) -> TileResult {
    let n = plan.nodes.len();
    let rect = work.rect;
    let mut snap = None;
    let buf = match &work.hot {
        Some((node, saved)) => {
            let mut buf = match saved {
                Some(px) => {
                    let own = CanvasView::tile_rect(work.tile);
                    let (side, w) = (compose::TILE as usize, rect.w as usize);
                    let x0 = (rect.x - own.x) as usize;
                    let rows = (rect.y - own.y) as usize..(rect.bottom() - own.y) as usize;
                    let px: Vec<Snap> = rows
                        .flat_map(|y| px[y * side + x0..y * side + x0 + w].iter().copied())
                        .collect();
                    compose::Buf {
                        rect,
                        px: from_snap(&px),
                    }
                }
                None => {
                    let mut buf = compose::Buf::transparent(rect);
                    plan.draw(0..*node, &mut buf);
                    snap = Some(to_snap(&buf.px));
                    buf
                }
            };
            plan.draw(*node..n, &mut buf);
            buf
        }
        None => {
            let mut buf = compose::Buf::transparent(rect);
            plan.draw(0..n, &mut buf);
            buf
        }
    };
    let pixels = buf
        .px
        .iter()
        .map(|p| {
            let [r, g, b, a] = compose::premultiplied(*p);
            match (channel, lut) {
                // A channel alone: its value in gray, over white where
                // transparent.
                (Some(c), _) => Color32::from_gray([r, g, b][c] + (255 - a)),
                (None, Some(lut)) => color_ui::shown(lut, *p),
                (None, None) => Color32::from_rgba_premultiplied(r, g, b, a),
            }
        })
        .collect();
    TileResult {
        image: ColorImage::new([rect.w as usize, rect.h as usize], pixels),
        snap,
    }
}

impl AtelierApp {
    /// Layer `i` (a group: with its layers) on its own, as the view shows
    /// it (see [`Lifted`]); its canvas box.
    pub(super) fn lifted(&mut self, ctx: &egui::Context, i: usize) -> Option<(&GpuImage, Rect)> {
        let mut slot = self.lifted.take();
        let mut sources = self.take_sources();
        let rect = self.lift(ctx, &mut slot, i, sources.as_mut());
        self.put_sources(sources);
        self.lifted = slot;
        Some((&self.lifted.as_ref()?.gpu, rect?))
    }

    /// The view's reductions, to composite at its level without making
    /// them again (given back with [`Self::put_sources`]).
    fn take_sources(&mut self) -> Option<compose::LevelCache> {
        let uid = self.doc.as_ref()?.uid();
        let v = self
            .canvas_view
            .as_mut()
            .filter(|v| v.doc == uid && v.pending.is_none())?;
        Some(std::mem::replace(
            &mut v.sources,
            compose::LevelCache::new(v.level),
        ))
    }

    fn put_sources(&mut self, sources: Option<compose::LevelCache>) {
        if let (Some(s), Some(v)) = (sources, &mut self.canvas_view) {
            v.sources = s;
        }
    }

    /// `slot` brought to show layer `i` (see [`Self::lifted`]), with the
    /// view's reductions when at its level; its canvas box.
    fn lift(
        &self,
        ctx: &egui::Context,
        slot: &mut Option<Lifted>,
        i: usize,
        view_sources: Option<&mut compose::LevelCache>,
    ) -> Option<Rect> {
        let doc = self.doc.as_ref()?;
        let layers = doc.render_layers();
        let level = render_level(self.zoom);
        let block = doc.block(i);
        let base = layers[i].offset;
        let mut opts = view_options(
            doc,
            self.preview_shown.as_ref(),
            self.blend_preview,
            None,
            false,
        );
        opts.only = Some(i);
        let (display, lut) = self.color.shown();
        // What it looks like, wherever it is.
        let key = hash_of((
            display,
            level,
            layers[block.clone()]
                .iter()
                .map(|l| {
                    let at = (l.offset.0 - base.0, l.offset.1 - base.1);
                    let mask = l.active_mask().map(|m| m.key((0, 0)));
                    (
                        l.rev(),
                        at,
                        mask,
                        format!("{:?}{:?}", l.kind, l.effects),
                        l.visible,
                    )
                })
                .collect::<Vec<_>>(),
            [layers[i].opacity, layers[i].fill].map(f32::to_bits),
            opts.pixels.map(|p| p.version),
            opts.mask.map(|m| m.version),
        ));
        if slot.as_ref().is_none_or(|l| l.key != key) {
            let extent = layers[block.clone()]
                .iter()
                .filter(|l| !l.is_group())
                .map(|l| {
                    let styles = l.styles_shown(opts.held, opts.later_styles);
                    l.extent_with(styles.as_deref()).unwrap_or(doc.full_rect())
                })
                .reduce(PixelRect::union)?;
            let b = 1i64 << level;
            let r = PixelRect {
                x: extent.x.div_euclid(b),
                y: extent.y.div_euclid(b),
                w: ((extent.right() + b - 1).div_euclid(b) - extent.x.div_euclid(b)) as u32,
                h: ((extent.bottom() + b - 1).div_euclid(b) - extent.y.div_euclid(b)) as u32,
            };
            if r.w == 0 || r.h == 0 || r.w as u64 * r.h as u64 > 1 << 26 {
                return None;
            }
            let mut own = compose::LevelCache::new(level);
            let sources = match view_sources {
                Some(s) if compose::Sources::level(s) == level => s,
                _ => &mut own,
            };
            let image = compose::plan(&layers, doc.full_rect(), sources, &opts).render(r);
            let rect = Rect::from_min_size(
                pos2((r.x * b) as f32, (r.y * b) as f32),
                vec2((r.w as i64 * b) as f32, (r.h as i64 * b) as f32),
            );
            // On the GPU for the faint ghost beyond the canvas: about a
            // screen's worth of pixels is plenty (a large layer is
            // grabbed at once).
            let mut b = 1;
            while image.width().max(image.height()) / b > GHOST_SIDE {
                b *= 2;
            }
            let ghost = match b {
                1 => std::borrow::Cow::Borrowed(&image),
                b => std::borrow::Cow::Owned(compose::levels::reduce_rgba(&image, b)),
            };
            let shown = color_ui::shown_image(lut.as_deref(), &ghost);
            let gpu = match slot.take() {
                Some(mut l) => {
                    l.gpu.set(ctx, &shown, 0);
                    l.gpu
                }
                None => GpuImage::new(ctx, "lifted".into(), &shown, 0),
            };
            *slot = Some(Lifted {
                key,
                gpu,
                image: Arc::new(image),
                level,
                rect,
                offset: base,
                placed: None,
            });
        }
        let l = slot.as_ref()?;
        let shift = vec2((base.0 - l.offset.0) as f32, (base.1 - l.offset.1) as f32);
        Some(l.rect.translate(shift))
    }

    /// Layer `i` as the transform being prepared, `t`, will place it (see
    /// [`compose::Placed`]): resampled (bilinear, premultiplied) into pixels
    /// of the level, where `visible` (canvas coordinates). The active
    /// layer's image is the one the ghost of a drag shows; the other layers
    /// transformed with it keep theirs in `xform`.
    pub(super) fn placed(
        &mut self,
        ctx: &egui::Context,
        i: usize,
        t: &xform::Transform,
        visible: Rect,
    ) -> Option<PlacedImage> {
        let doc = self.doc.as_ref()?;
        let (active, id) = (doc.active == i, doc.layers.get(i)?.id);
        let mut slot = if active {
            self.lifted.take()
        } else {
            self.xform.take_lifted(id)
        };
        let mut sources = self.take_sources();
        let rect = self.lift(ctx, &mut slot, i, sources.as_mut());
        self.put_sources(sources);
        let placed = rect.zip(slot.as_mut()).map(|(rect, l)| {
            let bounds = |r: Rect| [r.min.x, r.min.y, r.max.x, r.max.y].map(f64::from);
            let key = hash_of((
                l.key,
                bounds(rect).map(f64::to_bits),
                t.fingerprint(),
                bounds(visible).map(f64::to_bits),
            ));
            if l.placed.as_ref().is_none_or(|p| p.key != key)
                && let Some((dx, dy)) = translation(t)
            {
                // Only moved: the image as it is, in its new place (to the
                // nearest pixel of the level), nothing to resample.
                let b = 1i64 << l.level;
                let (iw, ih) = l.image.dimensions();
                let at = (
                    ((f64::from(rect.min.x) + dx) / b as f64).round() as i64,
                    ((f64::from(rect.min.y) + dy) / b as f64).round() as i64,
                );
                l.placed = Some(PlacedImage {
                    key,
                    image: Arc::clone(&l.image),
                    at,
                    rect: PixelRect {
                        x: at.0 * b,
                        y: at.1 * b,
                        w: iw * b as u32,
                        h: ih * b as u32,
                    },
                });
            }
            if l.placed.as_ref().is_none_or(|p| p.key != key) {
                let b = 1u32 << l.level;
                let reach = t.bounds(bounds(rect)).unwrap_or(bounds(visible));
                let grid = xform::Grid::covering(xform::clip(reach, bounds(visible)), f64::from(b));
                let src = xform::Source {
                    image: &*l.image,
                    origin: [f64::from(rect.min.x), f64::from(rect.min.y)],
                    px: f64::from(b),
                };
                let image = xform::transform_rgba(src, t, grid, xform::Filter::Bilinear);
                let bi = i64::from(b);
                l.placed = Some(PlacedImage {
                    key,
                    image: Arc::new(image),
                    at: (grid.x, grid.y),
                    rect: PixelRect {
                        x: grid.x * bi,
                        y: grid.y * bi,
                        w: grid.w * b,
                        h: grid.h * b,
                    },
                });
            }
            l.placed.clone()
        });
        if active {
            self.lifted = slot;
        } else {
            self.xform.put_lifted(id, slot);
        }
        placed.flatten()
    }
}

/// The offset of a transform that only moves (by whole pixels or not).
fn translation(t: &xform::Transform) -> Option<(f64, f64)> {
    let [x0, y0, x1, y1] = t.src;
    let first = t.forward([x0, y0])?;
    let d = (first[0] - x0, first[1] - y0);
    // At the thirds too (the control points of a warp).
    (0..16)
        .all(|k| {
            let (i, j) = ((k % 4) as f64 / 3.0, (k / 4) as f64 / 3.0);
            let p = [x0 + (x1 - x0) * i, y0 + (y1 - y0) * j];
            t.forward(p).is_some_and(|q| {
                (q[0] - p[0] - d.0).abs() < 1e-6 && (q[1] - p[1] - d.1).abs() < 1e-6
            })
        })
        .then_some(d)
}

/// A composited part of a tile, and the composite of the nodes under the
/// hot one there.
struct TileResult {
    image: ColorImage,
    snap: Option<Vec<Snap>>,
}

#[cfg(test)]
thread_local! {
    /// Tests: another [`TILE_BUDGET`].
    pub(super) static TEST_TILE_BUDGET: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

fn tile_budget() -> std::time::Duration {
    #[cfg(test)]
    if let Some(budget) = TEST_TILE_BUDGET.with(std::cell::Cell::get) {
        return budget;
    }
    TILE_BUDGET
}

/// Fewest tiles changed at once that wait for each other (see
/// [`Generation`]).
const GENERATION_TILES: usize = 8;

/// Tiles composited over several frames from one state of the document,
/// shown together at the end (see `draw_document`).
struct Generation {
    plan: compose::Plan,
    work: Vec<Work>,
    /// First part of `work` still to do.
    next: usize,
    done: Vec<(Work, TileResult)>,
    /// Tiles finished by the last [`Self::run`].
    last_count: u32,
}

impl Generation {
    /// Composites the parts still to do for at most [`TILE_BUDGET`]; whether
    /// some remain.
    fn run(
        &mut self,
        threads: usize,
        channel: Option<usize>,
        lut: Option<&crate::cms::Lut>,
    ) -> bool {
        let start = std::time::Instant::now();
        let first = self.done.len();
        let mut over_budget = false;
        while self.next < self.work.len() {
            if start.elapsed() > tile_budget() && self.done.len() > first {
                over_budget = true;
                break;
            }
            let end = (self.next + threads.max(1)).min(self.work.len());
            let batch = &self.work[self.next..end];
            let results: Vec<std::sync::Mutex<Option<TileResult>>> =
                batch.iter().map(|_| std::sync::Mutex::new(None)).collect();
            compose::parallel(batch.len(), |k| {
                let result = composite_part(&self.plan, &batch[k], channel, lut);
                *results[k].lock().expect("tile") = Some(result);
            });
            for (item, result) in batch.iter().zip(results) {
                if let Some(result) = result.into_inner().expect("tile") {
                    self.done.push((item.clone(), result));
                }
            }
            self.next = end;
        }
        self.last_count = self.done[first..].iter().filter(|(w, _)| w.last).count() as u32;
        over_budget
    }
}

/// Composited parts on the GPU: into their tiles, or as new tiles once
/// whole; the snapshots they made kept. `clean`: a tile done is no longer
/// dirty (a generation's tiles stopped being so when it began; marked
/// since, they stay so).
fn upload(ctx: &egui::Context, cv: &mut CanvasView, done: Vec<(Work, TileResult)>, clean: bool) {
    let mut snaps: SnapParts = HashMap::new();
    let mut whole: HashMap<(i64, i64), Vec<(PixelRect, ColorImage)>> = HashMap::new();
    for (item, result) in done {
        if let Some(snap) = result.snap {
            snaps
                .entry(item.tile)
                .or_default()
                .push((item.rect.y, snap));
        }
        let t = item.tile;
        match cv.tiles.get_mut(&t) {
            Some(tile) => {
                let own = CanvasView::tile_rect(t);
                let pos = [
                    (item.rect.x - own.x) as usize,
                    (item.rect.y - own.y) as usize,
                ];
                tile.tex.set_partial(pos, result.image, IMAGE_TEX_OPTIONS);
                if item.last && clean {
                    tile.dirty = None;
                }
            }
            None => whole.entry(t).or_default().push((item.rect, result.image)),
        }
    }
    // New tiles: whole, from their bands.
    for (t, mut parts) in whole {
        let own = CanvasView::tile_rect(t);
        if parts.iter().map(|(r, _)| r.h).sum::<u32>() != own.h {
            continue;
        }
        parts.sort_by_key(|(r, _)| r.y);
        let pixels: Vec<Color32> = parts.into_iter().flat_map(|(_, i)| i.pixels).collect();
        let image = ColorImage::new([compose::TILE as usize; 2], pixels);
        let name = format!("tile-{}-{}", t.0, t.1);
        let tex = ctx.load_texture(name, image, IMAGE_TEX_OPTIONS);
        let since = cv.frame;
        cv.tiles.insert(
            t,
            Tile {
                tex,
                dirty: None,
                since,
            },
        );
    }
    if let Some(hot) = &mut cv.hot {
        for (t, mut parts) in snaps {
            parts.sort_by_key(|(y, _)| *y);
            let snap: Vec<Snap> = parts.into_iter().flat_map(|(_, s)| s).collect();
            if snap.len() == (compose::TILE * compose::TILE) as usize {
                hot.snaps.insert(t, Arc::new(snap));
            }
        }
    }
}

/// Level of reduction composited for a zoom (physical pixels per image
/// pixel): each of its pixels then covers between half a pixel and one
/// pixel of the screen (more when zoomed in past 100 %).
pub(super) fn render_level(zoom: f32) -> u32 {
    levels_for(zoom) as u32
}

/// Keeps, drops or starts the hot node's snapshots: when a single node
/// changed, those under it are kept from frame to frame.
fn update_hot(
    cv: &mut CanvasView,
    sigs: &[compose::Sig],
    plan: &compose::Plan,
    dirty: compose::Dirty,
) {
    let below = |k: usize| {
        let under: Vec<_> = (0..sigs.len())
            .filter(|&i| plan.owner[i].is_some_and(|o| o < k))
            .map(|i| (sigs[i].state, sigs[i].rev, sigs[i].structure))
            .collect();
        hash_of((under, plan.nodes.len()))
    };
    if let Some(h) = &cv.hot
        && below(h.node) != h.below
    {
        cv.hot = None;
    }
    let changed: Vec<usize> = if cv.sigs.len() == sigs.len() {
        (0..sigs.len()).filter(|&i| cv.sigs[i] != sigs[i]).collect()
    } else {
        Vec::new()
    };
    let nodes: std::collections::BTreeSet<Option<usize>> =
        changed.iter().map(|&i| plan.owner[i]).collect();
    let nodes: Vec<Option<usize>> = nodes.into_iter().collect();
    match (dirty, &nodes[..]) {
        (compose::Dirty::Nothing, _) => {}
        (compose::Dirty::All, _) => cv.hot = None,
        (_, &[Some(k)]) if k > 0 => {
            if cv.hot.as_ref().is_none_or(|h| h.node != k) {
                cv.hot = Some(Hot {
                    node: k,
                    below: below(k),
                    snaps: HashMap::new(),
                });
            }
        }
        _ => cv.hot = None,
    }
}

/// Selection outline as drawn: its edges, relative to the selection's box
/// unless it is inverted (moving the selection then costs nothing), and for
/// big outlines their image at screen resolution.
pub(super) struct Outline {
    selection: Selection,
    canvas: PixelRect,
    /// Empty when `dense`, or while they are found (`pending`).
    pub(super) edges: Vec<Edge>,
    /// Far too many edges (a speckled selection): drawn from the coverage
    /// at screen resolution instead.
    pub(super) dense: bool,
    /// A large selection's edges, found on another thread (`None`: too
    /// many).
    pending: Option<std::thread::JoinHandle<Option<Vec<Edge>>>>,
    /// Image of the outline over the visible area: view it was drawn for,
    /// image and its place on screen.
    raster: Option<(u64, GpuImage, Rect)>,
}

/// Above this many segments, the outline is drawn from the coverage (they
/// would take hundreds of megabytes, and long to draw at each move).
const MAX_DRAWN_EDGES: usize = 400_000;

/// Above this many pixels examined, the outline is found on another thread.
const OUTLINE_SYNC_PIXELS: u64 = 2_000_000;

/// Above this many segments, the outline is drawn as an image: shapes would
/// cost too much per frame, and could overflow the GPU's buffers.
pub(super) const MAX_OUTLINE_SHAPES: usize = 20_000;

/// Length of the outline's dashes (screen points).
const DASH: f32 = 4.0;

impl Outline {
    fn new(sel: &Selection, canvas: PixelRect) -> Self {
        let find = {
            let sel = sel.clone();
            move || {
                // Past this, the coverage is drawn: no need to go on.
                let limit = MAX_DRAWN_EDGES;
                if sel.inverted {
                    return sel.outline_within(canvas, limit);
                }
                let (x, y) = (sel.rect.x, sel.rect.y);
                let mut edges = sel.outline_within(sel.rect, limit)?;
                for [a, b] in &mut edges {
                    *a = (a.0 - x, a.1 - y);
                    *b = (b.0 - x, b.1 - y);
                }
                Some(edges)
            }
        };
        let area = if sel.inverted { canvas } else { sel.rect };
        let mut outline = Self {
            selection: sel.clone(),
            canvas,
            edges: Vec::new(),
            dense: false,
            pending: None,
            raster: None,
        };
        if sel.mask.is_some() && area.w as u64 * area.h as u64 > OUTLINE_SYNC_PIXELS {
            outline.pending = Some(std::thread::spawn(find));
        } else {
            outline.found(find());
        }
        outline
    }

    fn found(&mut self, edges: Option<Vec<Edge>>) {
        self.dense = edges.is_none();
        self.edges = edges.unwrap_or_default();
    }

    /// The edges found on another thread, once there. Returns `false`
    /// while they are not.
    fn ready(&mut self) -> bool {
        if let Some(job) = self.pending.take_if(|j| j.is_finished()) {
            let edges = job.join().unwrap_or(None);
            self.found(edges);
        }
        self.pending.is_none()
    }

    /// Still the outline of `sel`, possibly moved.
    fn fits(&self, sel: &Selection, canvas: PixelRect) -> bool {
        let s = &self.selection;
        if sel.inverted || s.inverted {
            return s.same(sel) && self.canvas == canvas;
        }
        (s.rect.w, s.rect.h) == (sel.rect.w, sel.rect.h)
            && match (&s.mask, &sel.mask) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }

    /// Black line and white dashes, like Photoshop; `at`: canvas position of
    /// the edges' origin.
    fn paint_shapes(&self, painter: &Painter, view: View, at: (i64, i64)) {
        let mut black = Mesh::default();
        let mut white = Mesh::default();
        let visible = painter.clip_rect().expand(1.0);
        for [a, b] in &self.edges {
            let (a, b) = (
                view.to_screen(pos2((a.0 + at.0) as f32, (a.1 + at.1) as f32)),
                view.to_screen(pos2((b.0 + at.0) as f32, (b.1 + at.1) as f32)),
            );
            let line = Rect::from_two_pos(a, b);
            if !line.intersects(visible) {
                continue;
            }
            let line = line.intersect(visible);
            // One-pixel-wide line, centered on the edge.
            black.add_colored_rect(line.expand(0.5), Color32::BLACK);
            // Dashes aligned on the screen position, so they line up.
            let horizontal = line.height() < line.width();
            let (from, to) = if horizontal {
                (line.left(), line.right())
            } else {
                (line.top(), line.bottom())
            };
            let mut t = (from / (2.0 * DASH)).floor() * 2.0 * DASH;
            while t < to {
                let (s, e) = (t.max(from), (t + DASH).min(to));
                if e > s {
                    let dash = if horizontal {
                        Rect::from_x_y_ranges(s..=e, line.y_range())
                    } else {
                        Rect::from_x_y_ranges(line.x_range(), s..=e)
                    };
                    white.add_colored_rect(dash.expand(0.5), Color32::WHITE);
                }
                t += 2.0 * DASH;
            }
        }
        painter.add(black);
        painter.add(white);
    }

    /// A speckled selection's outline, from its coverage: a line where the
    /// pixel on screen differs from the one to its right or below, drawn
    /// into an image of the visible area (physical pixels, by bands of
    /// rows on all the cores), redone only when the view or the
    /// selection's place changes.
    fn paint_dense(&mut self, ctx: &egui::Context, painter: &Painter, view: View, at: (i64, i64)) {
        let ppp = ctx.pixels_per_point();
        let clip = painter.clip_rect();
        let key = hash_of((
            [view.origin.x, view.origin.y, view.zoom, ppp].map(f32::to_bits),
            [clip.min.x, clip.min.y, clip.max.x, clip.max.y].map(f32::to_bits),
            at,
            "dense",
        ));
        if self.raster.as_ref().is_none_or(|(k, ..)| *k != key) {
            let (x0, y0) = (
                (clip.min.x * ppp).floor() as i64,
                (clip.min.y * ppp).floor() as i64,
            );
            let (x1, y1) = (
                (clip.max.x * ppp).ceil() as i64,
                (clip.max.y * ppp).ceil() as i64,
            );
            let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
            // Where the selection is now, from where it was outlined.
            let sel = &self.selection;
            let (dx, dy) = if sel.inverted {
                (0, 0)
            } else {
                (at.0 - sel.rect.x, at.1 - sel.rect.y)
            };
            let canvas = self.canvas;
            // Canvas pixel under each column and row of the image (one more
            // of each, for the neighbors).
            let image_at = |px: i64, py: i64| {
                view.to_image(pos2((px as f32 + 0.5) / ppp, (py as f32 + 0.5) / ppp))
            };
            let columns: Vec<i64> = (0..=w as i64)
                .map(|i| image_at(x0 + i, y0).x.floor() as i64)
                .collect();
            let rows: Vec<i64> = (0..=h as i64)
                .map(|j| image_at(x0, y0 + j).y.floor() as i64)
                .collect();
            let inside =
                |x: i64, y: i64| canvas.contains(x, y) && sel.coverage(x - dx, y - dy) >= 128;
            let dash = (DASH * ppp).round().max(1.0) as i64;
            let mut raw = vec![0u8; w * h * 4];
            crate::mask_ops::par_rows(&mut raw, (w * 4).max(1), |first, band| {
                let mut below: Vec<bool> = Vec::with_capacity(w + 1);
                let mut here: Vec<bool> = Vec::with_capacity(w + 1);
                for (j, line) in band.chunks_exact_mut((w * 4).max(1)).enumerate() {
                    let r = first + j;
                    if j == 0 {
                        here.clear();
                        here.extend(columns.iter().map(|&x| inside(x, rows[r])));
                    } else {
                        std::mem::swap(&mut here, &mut below);
                    }
                    below.clear();
                    below.extend(columns.iter().map(|&x| inside(x, rows[r + 1])));
                    for (i, p) in line.chunks_exact_mut(4).enumerate() {
                        let across = here[i] != below[i];
                        if !across && here[i] == here[i + 1] {
                            continue;
                        }
                        // Dashes along the line, on the screen position.
                        let along = if across { x0 + i as i64 } else { y0 + r as i64 };
                        let c = if along.div_euclid(dash) % 2 == 0 {
                            255
                        } else {
                            0
                        };
                        p.copy_from_slice(&[c, c, c, 255]);
                    }
                }
            });
            let image = RgbaImage::from_raw(w as u32, h as u32, raw).expect("size");
            let rect = Rect::from_min_size(
                pos2(x0 as f32 / ppp, y0 as f32 / ppp),
                vec2(w as f32 / ppp, h as f32 / ppp),
            );
            let gpu = match self.raster.take() {
                Some((_, mut gpu, _)) => {
                    gpu.set(ctx, &image, 0);
                    gpu
                }
                None => GpuImage::new(ctx, "selection".into(), &image, 0),
            };
            self.raster = Some((key, gpu, rect));
        }
        if let Some((_, gpu, rect)) = &self.raster {
            gpu.paint_all(painter, *rect, Color32::WHITE);
        }
    }

    /// The same, drawn into an image of the visible area (physical pixels),
    /// redone only when the view or the selection's place changes.
    fn paint_raster(&mut self, ctx: &egui::Context, painter: &Painter, view: View, at: (i64, i64)) {
        let ppp = ctx.pixels_per_point();
        let clip = painter.clip_rect();
        let key = hash_of((
            [view.origin.x, view.origin.y, view.zoom, ppp].map(f32::to_bits),
            [clip.min.x, clip.min.y, clip.max.x, clip.max.y].map(f32::to_bits),
            at,
        ));
        if self.raster.as_ref().is_none_or(|(k, ..)| *k != key) {
            let (x0, y0) = (
                (clip.min.x * ppp).floor() as i64,
                (clip.min.y * ppp).floor() as i64,
            );
            let (x1, y1) = (
                (clip.max.x * ppp).ceil() as i64,
                (clip.max.y * ppp).ceil() as i64,
            );
            let (w, h) = ((x1 - x0).max(0), (y1 - y0).max(0));
            let mut image = RgbaImage::new(w as u32, h as u32);
            let dash = (DASH * ppp).round().max(1.0) as i64;
            let thick = ppp.round().max(1.0) as i64;
            let to_px = |(x, y): (i64, i64)| {
                let p = view.to_screen(pos2((x + at.0) as f32, (y + at.1) as f32));
                (
                    (p.x * ppp).round() as i64 - x0,
                    (p.y * ppp).round() as i64 - y0,
                )
            };
            // `along`: screen position along the line, for the dashes.
            let mut plot = |x: i64, y: i64, along: i64| {
                if (0..w).contains(&x) && (0..h).contains(&y) {
                    let c = if along.div_euclid(dash) % 2 == 0 {
                        255
                    } else {
                        0
                    };
                    image.put_pixel(x as u32, y as u32, image::Rgba([c, c, c, 255]));
                }
            };
            for &[a, b] in &self.edges {
                let ((ax, ay), (bx, by)) = (to_px(a), to_px(b));
                if a.1 == b.1 {
                    // Horizontal: at least one pixel long, cut to the image.
                    let (from, to) = (ax.min(bx).max(0), ax.max(bx).max(ax.min(bx) + 1).min(w));
                    for y in ay - thick / 2..ay - thick / 2 + thick {
                        for x in from..to {
                            plot(x, y, x + x0);
                        }
                    }
                } else {
                    let (from, to) = (ay.min(by).max(0), ay.max(by).max(ay.min(by) + 1).min(h));
                    for x in ax - thick / 2..ax - thick / 2 + thick {
                        for y in from..to {
                            plot(x, y, y + y0);
                        }
                    }
                }
            }
            let rect = Rect::from_min_size(
                pos2(x0 as f32 / ppp, y0 as f32 / ppp),
                vec2(w as f32 / ppp, h as f32 / ppp),
            );
            let gpu = match self.raster.take() {
                Some((_, mut gpu, _)) => {
                    gpu.set(ctx, &image, 0);
                    gpu
                }
                None => GpuImage::new(ctx, "selection".into(), &image, 0),
            };
            self.raster = Some((key, gpu, rect));
        }
        if let Some((_, gpu, rect)) = &self.raster {
            gpu.paint_all(painter, *rect, Color32::WHITE);
        }
    }
}

impl AtelierApp {
    /// Selection outline: black line and white dashes, like Photoshop.
    /// A mask's outline is computed once per selection, not per move.
    pub(super) fn draw_selection(&mut self, painter: &Painter, view: View) {
        let (Some(sel), Some(doc)) = (&self.selection, &self.doc) else {
            return;
        };
        let canvas = doc.full_rect();
        if !self
            .selection_outline
            .as_ref()
            .is_some_and(|o| o.fits(sel, canvas))
        {
            self.selection_outline = Some(Outline::new(sel, canvas));
        }
        let at = if sel.inverted {
            (0, 0)
        } else {
            (sel.rect.x, sel.rect.y)
        };
        let Some(outline) = &mut self.selection_outline else {
            return;
        };
        if !outline.ready() {
            self.ctx
                .request_repaint_after(std::time::Duration::from_millis(30));
            return;
        }
        if outline.dense {
            outline.paint_dense(&self.ctx, painter, view, at);
        } else if outline.edges.len() <= MAX_OUTLINE_SHAPES {
            outline.paint_shapes(painter, view, at);
        } else {
            outline.paint_raster(&self.ctx, painter, view, at);
        }
    }
}

impl AtelierApp {
    /// Memory the display's copies of the documents take: reduced layers,
    /// tiles (also on the GPU), the Channels thumbnails' and the
    /// histograms' reductions, a layer lifted to be moved.
    pub(super) fn display_bytes(&self) -> usize {
        let view = self.canvas_view.as_ref().map_or(0, CanvasView::bytes);
        let channels = self.channels.reduced.as_ref().map_or(0, |(_, c)| c.bytes());
        let lifted = self
            .lifted
            .as_ref()
            .map_or(0, |l| l.image.as_raw().len() * 2);
        view + channels + self.below_histogram.bytes() + lifted
    }
}

#[cfg(test)]
impl AtelierApp {
    /// The whole canvas composited at the view's level from its kept
    /// reductions, and from fresh ones; `None` while a level is made.
    pub(super) fn reductions_against_fresh(&mut self) -> Option<(RgbaImage, RgbaImage)> {
        let doc = self.doc.as_ref()?;
        let cv = self.canvas_view.as_mut()?;
        if cv.pending.is_some() {
            return None;
        }
        let layers = doc.render_layers();
        let opts = view_options(doc, None, None, None, false);
        let canvas = doc.full_rect();
        let b = 1u32 << cv.level;
        let r = PixelRect {
            w: canvas.w.div_ceil(b),
            h: canvas.h.div_ceil(b),
            ..canvas
        };
        let mut kept = cv.sources.clone();
        let cached = compose::plan(&layers, canvas, &mut kept, &opts).render(r);
        let mut fresh = compose::LevelCache::new(cv.level);
        let fresh = compose::plan(&layers, canvas, &mut fresh, &opts).render(r);
        Some((cached, fresh))
    }

    /// Whether the view's reductions are being made on a worker thread.
    pub(super) fn level_pending(&self) -> bool {
        self.canvas_view
            .as_ref()
            .is_some_and(|v| v.pending.is_some())
    }

    /// Whether a change of much of the view is being composited (see
    /// `Generation`).
    pub(super) fn generation_pending(&self) -> bool {
        self.canvas_view
            .as_ref()
            .is_some_and(|v| v.generation.is_some())
    }
}
