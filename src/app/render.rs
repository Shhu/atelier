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
/// pixel): enough to never reduce one more than twice.
pub(super) fn levels_for(zoom: f32) -> usize {
    if zoom >= 0.5 {
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
        }
    }

    /// The previewed image is the mask's (gray), placed at `origin`.
    pub(super) fn on_mask(mut self, origin: (i64, i64)) -> Self {
        let gray = image::GrayImage::from_fn(self.image.width(), self.image.height(), |x, y| {
            let [r, g, b, a] = self.image.get_pixel(x, y).0;
            let luma = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
            image::Luma([((luma * a as u32 + 127) / 255) as u8])
        });
        self.mask = Some((Arc::new(gray), origin));
        self
    }

    /// What the compositing shows instead of the document (the layer being
    /// at `doc_offset`).
    fn options<'a>(&'a self, opts: &mut compose::Options<'a>, doc_offset: (i64, i64)) {
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
    /// The layers as last composited.
    sigs: Vec<compose::Sig>,
    /// Tiles (column, row, in tiles of the level) and whether each must be
    /// composited again.
    tiles: HashMap<(i64, i64), (TextureHandle, bool)>,
    hot: Option<Hot>,
}

/// Composite of the nodes under the one that keeps changing (opacity
/// slider, drag, brush stroke…), per tile, kept while only that one does.
struct Hot {
    node: usize,
    /// Fingerprint of the layers under it.
    below: u64,
    snaps: HashMap<(i64, i64), Arc<Vec<compose::blend::Px>>>,
}

/// Composites the tiles still to do for at most this long per frame, the
/// rest on the next ones (the view stays responsive).
const TILE_BUDGET: std::time::Duration = std::time::Duration::from_millis(30);

impl CanvasView {
    fn new(doc: u64, canvas: PixelRect, level: u32, channel: Option<usize>, display: u64) -> Self {
        Self {
            doc,
            canvas,
            level,
            channel,
            display,
            sources: compose::LevelCache::new(level),
            sigs: Vec::new(),
            tiles: HashMap::new(),
            hot: None,
        }
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
        let sigs = compose::signatures(&layers, &opts);
        let canvas = doc.full_rect();
        if canvas_view.as_ref().is_none_or(|v| {
            (v.doc, v.canvas, v.level, v.channel, v.display)
                != (uid, canvas, level, channel, display)
        }) {
            *canvas_view = Some(CanvasView::new(uid, canvas, level, channel, display));
        }
        let cv = canvas_view.as_mut().expect("set above");
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
        let dirty = if cv.sigs.is_empty() {
            compose::Dirty::All
        } else {
            compose::dirty_area(&cv.sigs, &sigs, &strokes)
        };
        let b = 1i64 << level;
        let tile = compose::TILE as i64 * b;
        match dirty {
            compose::Dirty::Nothing => {}
            compose::Dirty::All => cv.tiles.values_mut().for_each(|t| t.1 = true),
            compose::Dirty::Area(a) => {
                for (&(tx, ty), t) in cv.tiles.iter_mut() {
                    let r = PixelRect {
                        x: tx * tile,
                        y: ty * tile,
                        w: tile as u32,
                        h: tile as u32,
                    };
                    if intersect(r, a).is_some() {
                        t.1 = true;
                    }
                }
            }
        }
        // Tiles of the visible part of the frame.
        let shown = view
            .rect_to_screen(to_rect(frame))
            .intersect(painter.clip_rect());
        if !shown.is_positive() {
            cv.sigs = sigs;
            return;
        }
        let (a, z) = (view.to_image(shown.min), view.to_image(shown.max));
        let range = |from: f32, to: f32| {
            (from.floor() as i64).div_euclid(tile)..=((to.ceil() as i64 - 1).div_euclid(tile))
        };
        let (cols, rows) = (range(a.x, z.x), range(a.y, z.y));
        let visible: Vec<(i64, i64)> = rows
            .clone()
            .flat_map(|ty| cols.clone().map(move |tx| (tx, ty)))
            .collect();
        let todo: Vec<(i64, i64)> = visible
            .iter()
            .copied()
            .filter(|t| cv.tiles.get(t).is_none_or(|t| t.1))
            .collect();
        if !todo.is_empty() {
            for (id, rect) in &strokes {
                cv.sources.hint(compose::Hint {
                    layer: *id,
                    rect: *rect,
                });
            }
            let plan = compose::plan(&layers, doc.full_rect(), &mut cv.sources, &opts);
            cv.sources.retain_used();
            update_hot(cv, &sigs, &plan, dirty);
            let n = plan.nodes.len();
            let hot = cv.hot.as_ref().map(|h| (h.node, &h.snaps));
            let start = std::time::Instant::now();
            let threads = std::thread::available_parallelism().map_or(1, |t| t.get());
            let mut done = Vec::new();
            for batch in todo.chunks(threads.max(1)) {
                if start.elapsed() > TILE_BUDGET && !done.is_empty() {
                    ctx.request_repaint();
                    break;
                }
                let results: Vec<std::sync::Mutex<Option<TileResult>>> =
                    batch.iter().map(|_| std::sync::Mutex::new(None)).collect();
                compose::parallel(batch.len(), |k| {
                    let (tx, ty) = batch[k];
                    let rect = PixelRect {
                        x: tx * compose::TILE as i64,
                        y: ty * compose::TILE as i64,
                        w: compose::TILE,
                        h: compose::TILE,
                    };
                    let mut snap = None;
                    let buf = match hot {
                        Some((node, snaps)) if node < n => {
                            let mut buf = match snaps.get(&(tx, ty)) {
                                Some(px) => compose::Buf {
                                    rect,
                                    px: px.to_vec(),
                                },
                                None => {
                                    let mut buf = compose::Buf::transparent(rect);
                                    plan.draw(0..node, &mut buf);
                                    snap = Some(Arc::new(buf.px.clone()));
                                    buf
                                }
                            };
                            plan.draw(node..n, &mut buf);
                            buf
                        }
                        _ => {
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
                            match (channel, &lut) {
                                // A channel alone: its value in gray, over
                                // white where transparent.
                                (Some(c), _) => Color32::from_gray([r, g, b][c] + (255 - a)),
                                (None, Some(lut)) => color_ui::shown(lut, *p),
                                (None, None) => Color32::from_rgba_premultiplied(r, g, b, a),
                            }
                        })
                        .collect();
                    let size = [compose::TILE as usize; 2];
                    *results[k].lock().expect("tile") = Some(TileResult {
                        image: ColorImage::new(size, pixels),
                        snap,
                    });
                });
                done.extend(batch.iter().copied().zip(results));
            }
            for ((tx, ty), result) in done {
                let Some(result) = result.into_inner().expect("tile") else {
                    continue;
                };
                if let (Some(snap), Some(hot)) = (result.snap, &mut cv.hot) {
                    hot.snaps.insert((tx, ty), snap);
                }
                match cv.tiles.get_mut(&(tx, ty)) {
                    Some(t) => {
                        t.0.set(result.image, IMAGE_TEX_OPTIONS);
                        t.1 = false;
                    }
                    None => {
                        let name = format!("tile-{tx}-{ty}");
                        let tex = ctx.load_texture(name, result.image, IMAGE_TEX_OPTIONS);
                        cv.tiles.insert((tx, ty), (tex, false));
                    }
                }
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
        cv.tiles.retain(|t, _| near(t));
        // Snapshots (1 MiB each) only for what is shown.
        if let Some(h) = &mut cv.hot {
            h.snaps.retain(|t, _| visible.contains(t));
        }
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        for t in &visible {
            if let Some((tex, _)) = cv.tiles.get(t) {
                let r = Rect::from_min_size(
                    pos2((t.0 * tile) as f32, (t.1 * tile) as f32),
                    Vec2::splat(tile as f32),
                );
                painter.image(tex.id(), view.rect_to_screen(r), uv, Color32::WHITE);
            }
        }
    }

    /// Layer `i` (a group: with its layers) on its own, as the view shows
    /// it (see [`Lifted`]); its canvas box.
    pub(super) fn lifted(&mut self, ctx: &egui::Context, i: usize) -> Option<(&GpuImage, Rect)> {
        let mut slot = self.lifted.take();
        let rect = self.lift(ctx, &mut slot, i);
        self.lifted = slot;
        Some((&self.lifted.as_ref()?.gpu, rect?))
    }

    /// `slot` brought to show layer `i` (see [`Self::lifted`]); its canvas
    /// box.
    fn lift(&self, ctx: &egui::Context, slot: &mut Option<Lifted>, i: usize) -> Option<Rect> {
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
                .map(|l| l.extent(opts.held).unwrap_or(doc.full_rect()))
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
            let mut sources = compose::LevelCache::new(level);
            let image = compose::plan(&layers, doc.full_rect(), &mut sources, &opts).render(r);
            let rect = Rect::from_min_size(
                pos2((r.x * b) as f32, (r.y * b) as f32),
                vec2((r.w as i64 * b) as f32, (r.h as i64 * b) as f32),
            );
            let shown = color_ui::shown_image(lut.as_deref(), &image);
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
        let rect = self.lift(ctx, &mut slot, i);
        let placed = rect.zip(slot.as_mut()).map(|(rect, l)| {
            let bounds = |r: Rect| [r.min.x, r.min.y, r.max.x, r.max.y].map(f64::from);
            let key = hash_of((
                l.key,
                bounds(rect).map(f64::to_bits),
                t.fingerprint(),
                bounds(visible).map(f64::to_bits),
            ));
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

/// A composited tile, and the composite of the nodes under the hot one.
struct TileResult {
    image: ColorImage,
    snap: Option<Arc<Vec<compose::blend::Px>>>,
}

/// Level of reduction composited for a zoom (physical pixels per image
/// pixel): each of its pixels then covers one or two of the screen.
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
    pub(super) edges: Vec<Edge>,
    /// Image of the outline over the visible area: view it was drawn for,
    /// image and its place on screen.
    raster: Option<(u64, GpuImage, Rect)>,
}

/// Above this many segments, the outline is drawn as an image: shapes would
/// cost too much per frame, and could overflow the GPU's buffers.
pub(super) const MAX_OUTLINE_SHAPES: usize = 20_000;

/// Length of the outline's dashes (screen points).
const DASH: f32 = 4.0;

impl Outline {
    fn new(sel: &Selection, canvas: PixelRect) -> Self {
        let edges = if sel.inverted {
            sel.outline(canvas)
        } else {
            let (x, y) = (sel.rect.x, sel.rect.y);
            let mut edges = sel.outline(sel.rect);
            for [a, b] in &mut edges {
                *a = (a.0 - x, a.1 - y);
                *b = (b.0 - x, b.1 - y);
            }
            edges
        };
        Self {
            selection: sel.clone(),
            canvas,
            edges,
            raster: None,
        }
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
        if outline.edges.len() <= MAX_OUTLINE_SHAPES {
            outline.paint_shapes(painter, view, at);
        } else {
            outline.paint_raster(&self.ctx, painter, view, at);
        }
    }
}
