//! The compositing engine, for the display and the export alike: the
//! layers (groups, masks, clipping, adjustment and fill layers, styles,
//! blend modes) turned into a [`Plan`] for a level of reduction, then
//! composited tile by tile on all the cores, in premultiplied floats.

pub mod blend;
mod build;
pub mod gradient;
pub mod levels;
pub mod styles;

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use image::{Luma, Rgba, RgbaImage};

pub use blend::BlendMode;
use blend::{Px, atop, blend_color, dissolve, over};
pub use build::{
    Dirty, Options, Placed, PlacedMask, Preview, Sig, dirty_area, plan, signatures, source_versions,
};
use gradient::Ramp;
pub use levels::{FullSize, Hint, LevelCache, SourceKey, Sources, Src};

use crate::document::PixelRect;
use crate::pixel_ops::Adjustment;

/// Side of the tiles the display composites (in pixels of its level).
pub const TILE: u32 = 256;

/// What to composite, at one level of reduction: nodes from the bottom up.
pub struct Plan {
    pub level: u32,
    /// Box of the canvas, in canvas pixels (gradient fills span it).
    canvas: [f32; 4],
    pub nodes: Vec<Node>,
    /// Top-level node each layer belongs to (`None` if it isn't drawn).
    pub owner: Vec<Option<usize>>,
}

pub enum Node {
    Layer(Box<LayerNode>),
    Group(Box<GroupNode>),
}

pub struct GroupNode {
    pub children: Vec<Node>,
    pub blend: BlendMode,
    pub opacity: f32,
    pub mask: Option<MaskSrc>,
    pub board: Option<Board>,
}

/// An artboard: its layers show only within its rectangle (pixels of the
/// level), over its background.
pub struct Board {
    pub rect: PixelRect,
    pub background: Option<[f32; 3]>,
}

pub struct LayerNode {
    pub content: Content,
    pub mask: Option<MaskSrc>,
    pub blend: BlendMode,
    pub opacity: f32,
    pub fill: f32,
    /// Drawn under it, on what is below (drop shadow, outer glow, stroke).
    pub exterior: Vec<FxSrc>,
    /// Drawn over its content, within its shape, from the bottom up.
    pub interior: Vec<Interior>,
    /// Layers clipped to it, from the bottom up.
    pub clipped: Vec<LayerNode>,
}

pub enum Content {
    Pixels(Src<Rgba<u8>>),
    Solid([f32; 3]),
    Gradient(Box<Ramp>),
    Adjust(Box<Adjustment>),
}

pub struct MaskSrc {
    pub src: Src<Luma<u8>>,
    /// Value outside the image (0..1).
    pub default: f32,
    pub density: f32,
}

impl MaskSrc {
    #[inline]
    fn at(&self, x: i64, y: i64) -> f32 {
        let (lx, ly) = (x - self.src.at.0, y - self.src.at.1);
        let image = &self.src.image;
        let v = if lx >= 0 && ly >= 0 && (lx as u32) < image.width() && (ly as u32) < image.height()
        {
            image.as_raw()[ly as usize * image.width() as usize + lx as usize] as f32 / 255.0
        } else {
            self.default
        };
        1.0 - self.density * (1.0 - v)
    }
}

/// A style drawn from its coverage image.
pub struct FxSrc {
    pub coverage: Src<Luma<u8>>,
    pub color: [f32; 3],
    pub opacity: f32,
    pub blend: BlendMode,
}

pub enum Interior {
    Fx(FxSrc),
    Color {
        color: [f32; 3],
        opacity: f32,
        blend: BlendMode,
    },
    Gradient {
        ramp: Box<Ramp>,
        /// Box of the layer's content, in canvas pixels.
        bounds: [f32; 4],
        opacity: f32,
        blend: BlendMode,
    },
}

/// A buffer of premultiplied pixels covering `rect` (in pixels of the
/// plan's level).
pub struct Buf {
    pub rect: PixelRect,
    pub px: Vec<Px>,
}

impl Buf {
    pub fn transparent(rect: PixelRect) -> Self {
        Self {
            rect,
            px: vec![[0.0; 4]; rect.w as usize * rect.h as usize],
        }
    }
}

/// Non-premultiplied 8-bit color of a pixel.
#[inline]
pub fn unpremultiply(p: Px) -> [u8; 4] {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 0.5 / 255.0 {
        return [0; 4];
    }
    let c = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8;
    [c(p[0]), c(p[1]), c(p[2]), (a * 255.0).round() as u8]
}

/// Premultiplied 8-bit color of a pixel (for the GPU).
#[inline]
pub fn premultiplied(p: Px) -> [u8; 4] {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [c(p[0]), c(p[1]), c(p[2]), c(p[3])]
}

/// Where a pixel of the level is, in the canvas: the center of its block.
struct Ctx {
    level: u32,
    canvas: [f32; 4],
}

impl Ctx {
    fn canvas_point(&self, x: i64, y: i64) -> (f32, f32) {
        let b = (1u32 << self.level) as f32;
        ((x as f32 + 0.5) * b, (y as f32 + 0.5) * b)
    }
}

impl Plan {
    /// Composites the nodes `nodes` (all with `0..self.nodes.len()`) onto
    /// `buf`.
    pub fn draw(&self, nodes: Range<usize>, buf: &mut Buf) {
        let ctx = Ctx {
            level: self.level,
            canvas: self.canvas,
        };
        for node in &self.nodes[nodes] {
            draw_node(node, &ctx, buf, false);
        }
    }

    /// The composite of area `rect` (pixels of the level), non-premultiplied,
    /// computed in bands on all the cores.
    pub fn render(&self, rect: PixelRect) -> RgbaImage {
        let mut out = RgbaImage::new(rect.w, rect.h);
        in_bands(&mut out, |y0, band| {
            let h = (band.len() / (rect.w as usize * 4)) as u32;
            let r = PixelRect {
                y: rect.y + y0 as i64,
                h,
                ..rect
            };
            let mut buf = Buf::transparent(r);
            self.draw(0..self.nodes.len(), &mut buf);
            for (o, p) in band.chunks_exact_mut(4).zip(&buf.px) {
                o.copy_from_slice(&unpremultiply(*p));
            }
        });
        out
    }
}

/// Runs `f(first row, rows)` on bands of 32 rows of `image`, on all the
/// cores.
pub fn in_bands(image: &mut RgbaImage, f: impl Fn(u32, &mut [u8]) + Sync) {
    const BAND: usize = 32;
    let row = image.width() as usize * 4;
    if row == 0 || image.height() == 0 {
        return;
    }
    let bands: Vec<std::sync::Mutex<Option<&mut [u8]>>> = image
        .as_mut()
        .chunks_mut(row * BAND)
        .map(|b| std::sync::Mutex::new(Some(b)))
        .collect();
    parallel(bands.len(), |k| {
        if let Some(band) = bands[k].lock().expect("band").take() {
            f((k * BAND) as u32, band);
        }
    });
}

/// Runs `f(0..n)` on all the cores (each index once); the pixel loops of
/// those threads stay on them.
pub fn parallel(n: usize, f: impl Fn(usize) + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(1, |t| t.get())
        .min(n);
    if threads <= 1 {
        return (0..n).for_each(f);
    }
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                crate::pixel_ops::run_serially();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    f(i);
                }
            });
        }
    });
}

fn draw_node(node: &Node, ctx: &Ctx, buf: &mut Buf, clip: bool) {
    match node {
        Node::Layer(l) => draw_layer(l, ctx, buf, clip),
        Node::Group(g) => draw_group(g, ctx, buf),
    }
}

/// Composites `s` (color, opacity) onto `d`: over it, or atop it for a
/// clipped layer; Dissolve decides per pixel.
#[inline]
fn put(d: &mut Px, s: [f32; 3], a: f32, mode: BlendMode, clip: bool, x: i64, y: i64) {
    let (a, mode) = match mode {
        BlendMode::Dissolve => (dissolve(a, x, y), BlendMode::Normal),
        m => (a, m),
    };
    if clip {
        atop(d, s, a, mode);
    } else {
        over(d, s, a, mode);
    }
}

/// The part of `buf` covered by `src` (buffer indices x0..x1, y0..y1).
fn overlap<P: image::Pixel<Subpixel = u8>>(buf: &Buf, src: &Src<P>) -> Option<[usize; 4]> {
    let r = PixelRect {
        x: src.at.0,
        y: src.at.1,
        w: src.image.width(),
        h: src.image.height(),
    };
    let i = crate::document::intersect(r, buf.rect)?;
    Some([
        (i.x - buf.rect.x) as usize,
        (i.y - buf.rect.y) as usize,
        (i.right() - buf.rect.x) as usize,
        (i.bottom() - buf.rect.y) as usize,
    ])
}

/// Color and opacity of a content pixel (opacity 0 outside an image).
#[inline]
fn content_at(content: &Content, ctx: &Ctx, x: i64, y: i64) -> ([f32; 3], f32) {
    match content {
        Content::Pixels(src) => {
            let (lx, ly) = (x - src.at.0, y - src.at.1);
            let image = &src.image;
            if lx < 0 || ly < 0 || lx as u32 >= image.width() || ly as u32 >= image.height() {
                return ([0.0; 3], 0.0);
            }
            let i = (ly as usize * image.width() as usize + lx as usize) * 4;
            let p = &image.as_raw()[i..i + 4];
            (
                [p[0], p[1], p[2]].map(|v| v as f32 / 255.0),
                p[3] as f32 / 255.0,
            )
        }
        Content::Solid(c) => (*c, 1.0),
        Content::Gradient(g) => {
            let (cx, cy) = ctx.canvas_point(x, y);
            let [r, gr, b, a] = g.at(cx, cy, ctx.canvas);
            ([r, gr, b], a)
        }
        Content::Adjust(_) => ([0.0; 3], 0.0),
    }
}

fn draw_fx(fx: &FxSrc, opacity: f32, buf: &mut Buf, clip: bool) {
    let Some([x0, y0, x1, y1]) = overlap(buf, &fx.coverage) else {
        return;
    };
    let (w, cw) = (buf.rect.w as usize, fx.coverage.image.width() as usize);
    // Coverage pixel of buffer pixel (0, 0) (may be negative).
    let (ox, oy) = (buf.rect.x - fx.coverage.at.0, buf.rect.y - fx.coverage.at.1);
    let k = fx.opacity * opacity / 255.0;
    let raw = fx.coverage.image.as_raw();
    for y in y0..y1 {
        let line = &raw[(y as i64 + oy) as usize * cw..];
        for x in x0..x1 {
            let c = line[(x as i64 + ox) as usize];
            if c > 0 {
                let d = &mut buf.px[y * w + x];
                let (cx, cy) = (buf.rect.x + x as i64, buf.rect.y + y as i64);
                put(d, fx.color, c as f32 * k, fx.blend, clip, cx, cy);
            }
        }
    }
}

fn draw_layer(n: &LayerNode, ctx: &Ctx, buf: &mut Buf, clip: bool) {
    if let Content::Adjust(adj) = &n.content {
        return adjust(n, adj, buf);
    }
    for fx in &n.exterior {
        draw_fx(fx, n.opacity, buf, clip);
    }
    let (w, rect) = (buf.rect.w as usize, buf.rect);
    // Rows and columns where the content can show.
    let [x0, y0, x1, y1] = match &n.content {
        // Nothing of it here: nothing clipped to it either.
        Content::Pixels(src) => match overlap(buf, src) {
            Some(r) => r,
            None => return,
        },
        _ => [0, 0, w, rect.h as usize],
    };
    if n.interior.is_empty() && n.clipped.is_empty() {
        let k = n.opacity * n.fill;
        for y in y0..y1 {
            let cy = rect.y + y as i64;
            for x in x0..x1 {
                let cx = rect.x + x as i64;
                let (c, a) = content_at(&n.content, ctx, cx, cy);
                if a <= 0.0 {
                    continue;
                }
                let m = n.mask.as_ref().map_or(1.0, |m| m.at(cx, cy));
                put(&mut buf.px[y * w + x], c, a * m * k, n.blend, clip, cx, cy);
            }
        }
        return;
    }
    // The layer apart: its content, then what is drawn within its shape.
    let mut layer = Buf::transparent(rect);
    let mut shape = vec![0.0f32; layer.px.len()];
    for y in y0..y1 {
        let cy = rect.y + y as i64;
        for x in x0..x1 {
            let cx = rect.x + x as i64;
            let (c, a) = content_at(&n.content, ctx, cx, cy);
            let a = a * n.mask.as_ref().map_or(1.0, |m| m.at(cx, cy));
            let i = y * w + x;
            shape[i] = a;
            let f = a * n.fill;
            layer.px[i] = [c[0] * f, c[1] * f, c[2] * f, f];
        }
    }
    for fx in &n.interior {
        match fx {
            Interior::Fx(f) => {
                let Some([fx0, fy0, fx1, fy1]) = overlap(&layer, &f.coverage) else {
                    continue;
                };
                let cw = f.coverage.image.width() as i64;
                let (ox, oy) = (rect.x - f.coverage.at.0, rect.y - f.coverage.at.1);
                let raw = f.coverage.image.as_raw();
                for y in fy0..fy1 {
                    for x in fx0..fx1 {
                        let i = y * w + x;
                        let at = ((y as i64 + oy) * cw + x as i64 + ox) as usize;
                        let a = raw[at] as f32 / 255.0 * shape[i] * f.opacity;
                        over(&mut layer.px[i], f.color, a, f.blend);
                    }
                }
            }
            Interior::Color {
                color,
                opacity,
                blend,
            } => {
                for (p, s) in layer.px.iter_mut().zip(&shape) {
                    over(p, *color, s * opacity, *blend);
                }
            }
            Interior::Gradient {
                ramp,
                bounds,
                opacity,
                blend,
            } => {
                for (i, (p, s)) in layer.px.iter_mut().zip(&shape).enumerate() {
                    if *s > 0.0 {
                        let (x, y) = (rect.x + (i % w) as i64, rect.y + (i / w) as i64);
                        let (cx, cy) = ctx.canvas_point(x, y);
                        let [r, g, b, a] = ramp.at(cx, cy, *bounds);
                        over(p, [r, g, b], s * opacity * a, *blend);
                    }
                }
            }
        }
    }
    for c in &n.clipped {
        draw_layer(c, ctx, &mut layer, true);
    }
    composite_onto(buf, &layer, n.opacity, None, n.blend, clip);
}

/// Composites `top` (same area as `buf`) onto it, with `opacity` (and a
/// mask).
fn composite_onto(
    buf: &mut Buf,
    top: &Buf,
    opacity: f32,
    mask: Option<&MaskSrc>,
    mode: BlendMode,
    clip: bool,
) {
    let w = buf.rect.w as usize;
    for (i, (d, s)) in buf.px.iter_mut().zip(&top.px).enumerate() {
        if s[3] <= 0.0 {
            continue;
        }
        let (x, y) = (buf.rect.x + (i % w) as i64, buf.rect.y + (i / w) as i64);
        let m = mask.map_or(1.0, |m| m.at(x, y));
        let c = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
        put(d, c, s[3] * opacity * m, mode, clip, x, y);
    }
}

/// An adjustment layer: what is below, adjusted, through its mask and
/// opacity (its opacity alone: transparent areas stay so).
fn adjust(n: &LayerNode, adj: &Adjustment, buf: &mut Buf) {
    let mut image = RgbaImage::new(buf.rect.w, buf.rect.h);
    for (o, p) in image.pixels_mut().zip(&buf.px) {
        // Full color precision of the opaque pixels kept in the mix below.
        o.0 = unpremultiply(*p);
    }
    adj.apply(&mut image);
    let w = buf.rect.w as usize;
    let k = n.opacity * n.fill;
    for (i, (d, a)) in buf.px.iter_mut().zip(image.pixels()).enumerate() {
        let da = d[3];
        if da <= 0.0 {
            continue;
        }
        let (x, y) = (buf.rect.x + (i % w) as i64, buf.rect.y + (i / w) as i64);
        let mut t = k * n.mask.as_ref().map_or(1.0, |m| m.at(x, y));
        if n.blend == BlendMode::Dissolve {
            t = dissolve(t, x, y);
        }
        if t <= 0.0 {
            continue;
        }
        let s = [a.0[0], a.0[1], a.0[2]].map(|v| v as f32 / 255.0);
        let b = [d[0] / da, d[1] / da, d[2] / da];
        let m = match n.blend {
            BlendMode::Normal | BlendMode::Dissolve | BlendMode::PassThrough => s,
            mode => blend_color(mode, b, s),
        };
        for c in 0..3 {
            d[c] += t * (m[c] * da - d[c]);
        }
    }
}

fn draw_group(g: &GroupNode, ctx: &Ctx, buf: &mut Buf) {
    if let Some(board) = &g.board {
        return draw_board(g, board, ctx, buf);
    }
    if g.blend == BlendMode::PassThrough {
        if g.opacity >= 1.0 && g.mask.is_none() {
            for c in &g.children {
                draw_node(c, ctx, buf, false);
            }
            return;
        }
        // Its layers act on what is below, the result faded by the group.
        let mut copy = Buf {
            rect: buf.rect,
            px: buf.px.clone(),
        };
        for c in &g.children {
            draw_node(c, ctx, &mut copy, false);
        }
        let w = buf.rect.w as usize;
        for (i, (d, s)) in buf.px.iter_mut().zip(&copy.px).enumerate() {
            let (x, y) = (buf.rect.x + (i % w) as i64, buf.rect.y + (i / w) as i64);
            let t = g.opacity * g.mask.as_ref().map_or(1.0, |m| m.at(x, y));
            for c in 0..4 {
                d[c] += t * (s[c] - d[c]);
            }
        }
        return;
    }
    let mut own = Buf::transparent(buf.rect);
    for c in &g.children {
        draw_node(c, ctx, &mut own, false);
    }
    composite_onto(buf, &own, g.opacity, g.mask.as_ref(), g.blend, false);
}

/// An artboard: its background, then its layers (Pass Through: acting on
/// what is below), all cut to its rectangle.
fn draw_board(g: &GroupNode, board: &Board, ctx: &Ctx, buf: &mut Buf) {
    let Some(area) = crate::document::intersect(board.rect, buf.rect) else {
        return;
    };
    let w = buf.rect.w as usize;
    let (x0, y0) = (
        (area.x - buf.rect.x) as usize,
        (area.y - buf.rect.y) as usize,
    );
    let index = |i: usize| (y0 + i / area.w as usize) * w + x0 + i % area.w as usize;
    let pass = g.blend == BlendMode::PassThrough;
    let mut own = Buf::transparent(area);
    if pass {
        for (i, p) in own.px.iter_mut().enumerate() {
            *p = buf.px[index(i)];
        }
    }
    if let Some(c) = board.background {
        for p in &mut own.px {
            over(p, c, 1.0, BlendMode::Normal);
        }
    }
    for c in &g.children {
        draw_node(c, ctx, &mut own, false);
    }
    let aw = area.w as usize;
    for (i, s) in own.px.iter().enumerate() {
        let (x, y) = (area.x + (i % aw) as i64, area.y + (i / aw) as i64);
        let t = g.opacity * g.mask.as_ref().map_or(1.0, |m| m.at(x, y));
        let d = &mut buf.px[index(i)];
        if pass {
            for c in 0..4 {
                d[c] += t * (s[c] - d[c]);
            }
        } else if s[3] > 0.0 {
            let color = [s[0] / s[3], s[1] / s[3], s[2] / s[3]];
            put(d, color, s[3] * t, g.blend, false, x, y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Document, Fill, NewMask};
    use crate::pixel_ops::AdjustKind;
    use image::Rgba;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    fn solid(c: Rgba<u8>) -> RgbaImage {
        RgbaImage::from_pixel(4, 4, c)
    }

    fn at(doc: &Document, x: u32, y: u32) -> [u8; 4] {
        doc.flatten().get_pixel(x, y).0
    }

    /// Red background; a group (made of the layers given) above it.
    fn grouped(layers: &[(RgbaImage, BlendMode)]) -> Document {
        let mut doc = Document::new(solid(RED), None);
        for (image, mode) in layers {
            doc.add_layer("l".into(), image.clone(), (0, 0));
            doc.active_layer_mut().blend = *mode;
        }
        let n = doc.layers.len();
        assert!(doc.group_layers(&(1..n).collect::<Vec<_>>()));
        doc
    }

    #[test]
    fn groups_pass_through_or_isolate_their_layers() {
        let gray = RgbaImage::from_pixel(4, 4, Rgba([128, 128, 128, 255]));
        let mut doc = grouped(&[(gray, BlendMode::Multiply)]);
        // Pass Through: the Multiply layer acts on the background.
        assert_eq!(at(&doc, 0, 0), [128, 0, 0, 255]);
        // Normal: it multiplies inside the group only (nothing there).
        let g = doc.active;
        doc.layers[g].blend = BlendMode::Normal;
        assert_eq!(at(&doc, 0, 0), [128, 128, 128, 255]);
        // An adjustment in an isolated group adjusts the group alone.
        let mut doc = Document::new(solid(RED), None);
        let mut small = RgbaImage::new(4, 4);
        small.put_pixel(0, 0, BLUE);
        doc.add_layer("blue".into(), small, (0, 0));
        doc.add_adjustment_layer(AdjustKind::Invert.initial(), None);
        doc.group_layers(&[1, 2]);
        assert_eq!(
            at(&doc, 1, 1),
            [0, 255, 255, 255],
            "pass through: all inverted"
        );
        doc.active_layer_mut().blend = BlendMode::Normal;
        assert_eq!(at(&doc, 1, 1), RED.0, "isolated: the background stays");
        assert_eq!(at(&doc, 0, 0), [255, 255, 0, 255]);
        // Opacity of a Pass Through group fades what its layers do.
        doc.active_layer_mut().blend = BlendMode::PassThrough;
        doc.active_layer_mut().opacity = 0.5;
        assert_eq!(at(&doc, 1, 1), [128, 128, 128, 255]);
        // Its mask hides it.
        doc.active_layer_mut().opacity = 1.0;
        doc.add_mask(NewMask::HideAll, None);
        assert_eq!(at(&doc, 1, 1), RED.0);
    }

    #[test]
    fn styles_fill_and_dissolve() {
        let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba([255; 4])), None);
        let mut square = RgbaImage::new(10, 10);
        for y in 3..7 {
            for x in 3..7 {
                square.put_pixel(x, y, BLUE);
            }
        }
        doc.add_layer("sq".into(), square.clone(), (0, 0));
        let fx = &mut doc.active_layer_mut().effects;
        fx.color_overlay.enabled = true;
        fx.color_overlay.color = [0, 255, 0, 255];
        fx.stroke.enabled = true;
        fx.stroke.color = [255, 0, 0, 255];
        fx.stroke.width = 1.0;
        assert_eq!(at(&doc, 4, 4), [0, 255, 0, 255], "overlay");
        assert_eq!(at(&doc, 2, 4), RED.0, "stroke");
        // Fill 0: the content goes, the styles stay.
        doc.active_layer_mut().fill = 0.0;
        assert_eq!(at(&doc, 4, 4), [0, 255, 0, 255]);
        doc.active_layer_mut().effects.color_overlay.enabled = false;
        assert_eq!(at(&doc, 4, 4), [255; 4]);
        assert_eq!(at(&doc, 2, 4), RED.0);
        // Opacity fades both.
        doc.active_layer_mut().opacity = 0.0;
        assert_eq!(at(&doc, 2, 4), [255; 4]);
        // Dissolve: pixels whole or not at all.
        let mut doc = Document::new(RgbaImage::from_pixel(64, 64, Rgba([255; 4])), None);
        doc.add_layer("d".into(), RgbaImage::from_pixel(64, 64, BLUE), (0, 0));
        doc.active_layer_mut().blend = BlendMode::Dissolve;
        doc.active_layer_mut().opacity = 0.5;
        let flat = doc.flatten();
        let blue = flat.pixels().filter(|p| p.0 == BLUE.0).count();
        let white = flat.pixels().filter(|p| p.0 == [255; 4]).count();
        assert_eq!(blue + white, 64 * 64);
        assert!((1700..2400).contains(&blue), "{blue}");
    }

    #[test]
    fn tiles_and_levels_match_the_whole_image() {
        let mut doc = grouped(&[
            (
                RgbaImage::from_fn(300, 200, |x, y| Rgba([x as u8, y as u8, 100, 200])),
                BlendMode::Overlay,
            ),
            (
                RgbaImage::from_fn(300, 200, |x, _| Rgba([0, 90, x as u8, 255])),
                BlendMode::Screen,
            ),
        ]);
        doc.resize_canvas(0, 0, 300, 200);
        doc.add_adjustment_layer(AdjustKind::Posterize.initial(), None);
        let flat = doc.flatten();
        let whole = plan(
            &doc.layers,
            doc.full_rect(),
            &mut FullSize,
            &Options::default(),
        );
        let mut tiled = RgbaImage::new(300, 200);
        for (tx, ty) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let rect = PixelRect {
                x: tx * 150,
                y: ty * 100,
                w: 150,
                h: 100,
            };
            let mut buf = Buf::transparent(rect);
            whole.draw(0..whole.nodes.len(), &mut buf);
            for (i, p) in buf.px.iter().enumerate() {
                let (x, y) = (
                    rect.x as u32 + i as u32 % 150,
                    rect.y as u32 + i as u32 / 150,
                );
                tiled.put_pixel(x, y, Rgba(unpremultiply(*p)));
            }
        }
        assert!(tiled == flat);
        // Reduced, a uniform document stays uniform.
        let mut doc = Document::new(
            RgbaImage::from_pixel(64, 64, Rgba([10, 200, 30, 255])),
            None,
        );
        doc.add_layer(
            "m".into(),
            RgbaImage::from_pixel(64, 64, Rgba([128, 128, 128, 255])),
            (0, 0),
        );
        doc.active_layer_mut().blend = BlendMode::Multiply;
        let reduced = plan(
            &doc.layers,
            doc.full_rect(),
            &mut LevelCache::new(2),
            &Options::default(),
        )
        .render(PixelRect {
            x: 0,
            y: 0,
            w: 16,
            h: 16,
        });
        assert!(
            reduced
                .pixels()
                .all(|p| p.0 == doc.flatten().get_pixel(0, 0).0)
        );
    }

    #[test]
    fn dirty_areas_follow_what_changed() {
        let mut doc = Document::new(RgbaImage::from_pixel(100, 100, RED), None);
        doc.add_layer("s".into(), RgbaImage::from_pixel(10, 10, BLUE), (5, 5));
        let opts = Options::default();
        let before = signatures(&doc.layers, &opts);
        assert_eq!(dirty_area(&before, &before, &[]), Dirty::Nothing);
        doc.layers[1].offset = (50, 50);
        let moved = signatures(&doc.layers, &opts);
        let r = |x, y, w, h| PixelRect { x, y, w, h };
        assert_eq!(
            dirty_area(&before, &moved, &[]),
            Dirty::Area(r(5, 5, 55, 55))
        );
        // A brush stroke: only what it painted.
        doc.layers[1].pixels_mut().put_pixel(0, 0, RED);
        let painted = signatures(&doc.layers, &opts);
        let id = doc.layers[1].id;
        let stroke = [(id, r(50, 50, 1, 1))];
        assert_eq!(
            dirty_area(&moved, &painted, &stroke),
            Dirty::Area(r(50, 50, 1, 1))
        );
        // An adjustment reaches everywhere; a new layer changes the structure.
        doc.add_adjustment_layer(AdjustKind::Invert.initial(), None);
        let adjusted = signatures(&doc.layers, &opts);
        assert_eq!(dirty_area(&painted, &adjusted, &[]), Dirty::All);
        doc.layers[2].opacity = 0.5;
        assert_eq!(
            dirty_area(&adjusted, &signatures(&doc.layers, &opts), &[]),
            Dirty::All
        );
    }

    /// 6000 × 4000, 20 layers in various modes, an adjustment, a mask and
    /// styles: `cargo test --release bench_large -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing"]
    fn bench_large_document() {
        let (w, h) = (6000, 4000);
        let photo = RgbaImage::from_fn(w, h, |x, y| {
            Rgba([(x / 23) as u8, (y / 15) as u8, ((x + y) / 40) as u8, 255])
        });
        let mut doc = Document::new(photo, None);
        let modes = [
            BlendMode::Normal,
            BlendMode::Multiply,
            BlendMode::Screen,
            BlendMode::Overlay,
            BlendMode::SoftLight,
        ];
        for k in 0..18 {
            let image = RgbaImage::from_fn(w / 2, h / 2, |x, y| {
                Rgba([(x * k) as u8, (y * 3) as u8, 90, ((x + y) % 255) as u8])
            });
            doc.add_layer(
                format!("L{k}"),
                image,
                ((k as i64 * 150) % 3000, (k as i64 * 90) % 2000),
            );
            doc.active_layer_mut().blend = modes[k as usize % modes.len()];
            if k == 5 {
                doc.add_mask(NewMask::HideAll, None);
            }
            if k == 7 {
                doc.active_layer_mut().effects.shadow.enabled = true;
                doc.active_layer_mut().effects.stroke.enabled = true;
            }
        }
        doc.add_adjustment_layer(AdjustKind::Curves.initial(), None);
        doc.add_fill_layer(Fill::Solid([10, 20, 200]), None);
        doc.active_layer_mut().opacity = 0.2;
        let opts = Options::default();
        let t = std::time::Instant::now();
        for l in &doc.layers {
            l.styles(false);
        }
        println!("styles: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let flat = doc.flatten();
        println!(
            "flatten 6000×4000, {} layers: {:?}",
            doc.layers.len(),
            t.elapsed()
        );
        assert_eq!(flat.dimensions(), (w, h));
        for level in [2, 0] {
            let mut sources = LevelCache::new(level);
            let t = std::time::Instant::now();
            let plan = plan(&doc.layers, doc.full_rect(), &mut sources, &opts);
            println!("level {level}: plan (reductions): {:?}", t.elapsed());
            // About a full-HD screen of tiles.
            let tiles: Vec<(i64, i64)> = (0..5).flat_map(|y| (0..8).map(move |x| (x, y))).collect();
            let t = std::time::Instant::now();
            parallel(tiles.len(), |k| {
                let (tx, ty) = tiles[k];
                let rect = PixelRect {
                    x: tx * TILE as i64,
                    y: ty * TILE as i64,
                    w: TILE,
                    h: TILE,
                };
                let mut buf = Buf::transparent(rect);
                plan.draw(0..plan.nodes.len(), &mut buf);
            });
            println!("level {level}: 40 tiles: {:?}", t.elapsed());
            let t = std::time::Instant::now();
            let n = plan.nodes.len();
            parallel(tiles.len(), |k| {
                let (tx, ty) = tiles[k];
                let rect = PixelRect {
                    x: tx * TILE as i64,
                    y: ty * TILE as i64,
                    w: TILE,
                    h: TILE,
                };
                let mut buf = Buf::transparent(rect);
                plan.draw(n - 3..n, &mut buf);
            });
            println!(
                "level {level}: 40 tiles, top 3 nodes (hot): {:?}",
                t.elapsed()
            );
        }
    }
}
