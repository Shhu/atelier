//! From the layers of a document to a [`Plan`], and the fingerprints that
//! tell the display what to composite again.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::*;
use crate::document::{Fill, Layer, LayerKind, intersect};

/// What to show differently from the document.
#[derive(Default)]
pub struct Options<'a> {
    /// Brush stroke in progress: the styles stay as they were.
    pub held: bool,
    /// The display: styles not yet drawn for a layer's current pixels and
    /// settings are drawn on a worker thread, the former ones shown
    /// meanwhile (see `Layer::styles_later`).
    pub later_styles: bool,
    /// Layer left out (a group: with its layers), drawn on its own while it
    /// is transformed.
    pub hidden: Option<usize>,
    /// Only this layer (a group: with its layers), alone, as if in Normal
    /// mode: what a transform previews.
    pub only: Option<usize>,
    /// Previewed pixels of a layer (window open).
    pub pixels: Option<Preview<'a, RgbaImage>>,
    /// Previewed mask of a layer.
    pub mask: Option<Preview<'a, GrayImage>>,
    /// Blend mode tried on a layer (pointer over the list of modes).
    pub blend: Option<(u64, BlendMode)>,
    /// The mask of this layer shown alone, in gray, instead of the image.
    pub mask_view: Option<usize>,
    /// Layers (a group: with its layers) shown as these images instead, in
    /// their place in the stack: the transform being prepared.
    pub placed: Vec<Placed<'a>>,
    /// A layer's mask shown as this image instead: the transform of the
    /// mask alone being prepared.
    pub placed_mask: Option<PlacedMask<'a>>,
}

/// Pixels shown instead of a layer's (or of its mask's) while a window
/// previews its processing.
pub struct Preview<'a, I> {
    pub layer: u64,
    /// Changes whenever the image does.
    pub version: u64,
    pub image: &'a Arc<I>,
    /// Canvas position of its top-left corner.
    pub origin: (i64, i64),
}

impl<I> Clone for Preview<'_, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for Preview<'_, I> {}

/// See [`Options::placed`].
#[derive(Clone, Copy)]
pub struct Placed<'a> {
    pub layer: usize,
    pub version: u64,
    pub image: &'a Arc<RgbaImage>,
    /// Top-left corner, in pixels of the plan's level.
    pub at: (i64, i64),
    /// Canvas area it covers.
    pub rect: PixelRect,
}

/// See [`Options::placed_mask`].
#[derive(Clone, Copy)]
pub struct PlacedMask<'a> {
    pub layer: u64,
    pub version: u64,
    pub image: &'a Arc<GrayImage>,
    /// Top-left corner, in pixels of the plan's level.
    pub at: (i64, i64),
    /// Canvas area it covers.
    pub rect: PixelRect,
}

/// Index ranges of the layers of one level of the stack, from the bottom
/// up: each layer, with the range of its own layers for a group.
pub(crate) fn siblings(
    layers: &[Layer],
    range: Range<usize>,
) -> Vec<(usize, Option<Range<usize>>)> {
    let mut out = Vec::new();
    let mut i = range.end;
    while i > range.start {
        i -= 1;
        let l = &layers[i];
        if l.is_group() {
            let mut j = i;
            while j > range.start && layers[j - 1].depth > l.depth {
                j -= 1;
            }
            out.push((i, Some(j..i)));
            i = j;
        } else {
            out.push((i, None));
        }
    }
    out.reverse();
    out
}

/// Fingerprint of anything (Debug for the settings holding floats).
fn hash_of(value: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

fn debug_hash(value: &impl std::fmt::Debug) -> u64 {
    hash_of(format!("{value:?}"))
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

fn rgb(c: [u8; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]].map(|v| v as f32 / 255.0)
}

/// The plan of `layers` for the level of `sources`. `canvas`: the canvas
/// (gradient fills span it).
pub fn plan(
    layers: &[Layer],
    canvas: PixelRect,
    sources: &mut dyn Sources,
    opts: &Options,
) -> Plan {
    let mut owner = vec![None; layers.len()];
    let canvas_box = [
        canvas.x as f32,
        canvas.y as f32,
        canvas.w as f32,
        canvas.h as f32,
    ];
    let mut b = Builder {
        layers,
        sources,
        opts,
        owner: &mut owner,
    };
    if let Some(i) = opts
        .mask_view
        .filter(|&i| layers.get(i).is_some_and(|l| l.mask.is_some()))
    {
        // White through the mask, over black.
        let solid = |c: f32, mask| LayerNode {
            content: Content::Solid([c; 3]),
            mask,
            blend: BlendMode::Normal,
            opacity: 1.0,
            fill: 1.0,
            exterior: Vec::new(),
            interior: Vec::new(),
            clipped: Vec::new(),
        };
        let mut layer = layers[i].clone();
        if let Some(m) = &mut layer.mask {
            m.enabled = true;
        }
        let mask = b.mask(&layer);
        let nodes = vec![
            Node::Layer(Box::new(solid(0.0, None))),
            Node::Layer(Box::new(solid(1.0, mask))),
        ];
        return Plan {
            level: b.sources.level(),
            canvas: canvas_box,
            nodes,
            owner: vec![Some(1); layers.len()],
        };
    }
    let nodes = match opts.only.filter(|&i| i < layers.len()) {
        Some(i) => {
            let range = siblings(layers, 0..layers.len())
                .into_iter()
                .chain(nested(layers))
                .find(|(j, _)| *j == i)
                .and_then(|(_, r)| r);
            let node = match range {
                Some(children) => b.group(i, children),
                None => b.layer(i).map(|n| Node::Layer(Box::new(n))),
            };
            node.map(|mut n| {
                if let Node::Layer(l) = &mut n {
                    l.blend = BlendMode::Normal;
                } else if let Node::Group(g) = &mut n {
                    g.blend = match g.blend {
                        BlendMode::PassThrough => BlendMode::PassThrough,
                        _ => BlendMode::Normal,
                    };
                }
                n
            })
            .into_iter()
            .collect()
        }
        None => b.level(0..layers.len(), Some(0)),
    };
    Plan {
        level: sources.level(),
        canvas: canvas_box,
        nodes,
        owner,
    }
}

/// Every group of the stack, with its range (to find one by index).
fn nested(layers: &[Layer]) -> impl Iterator<Item = (usize, Option<Range<usize>>)> + '_ {
    (0..layers.len())
        .filter(|&i| layers[i].is_group())
        .map(move |i| {
            let mut j = i;
            while j > 0 && layers[j - 1].depth > layers[i].depth {
                j -= 1;
            }
            (i, Some(j..i))
        })
}

struct Builder<'a, 'o> {
    layers: &'a [Layer],
    sources: &'a mut dyn Sources,
    opts: &'a Options<'o>,
    owner: &'a mut [Option<usize>],
}

impl Builder<'_, '_> {
    /// Nodes of one level of the stack; `top`: numbering of the top-level
    /// nodes (the owners), from this index.
    fn level(&mut self, range: Range<usize>, top: Option<usize>) -> Vec<Node> {
        let mut nodes: Vec<Node> = Vec::new();
        // The clipping base below, if it is drawn (index in `nodes`), and
        // whether it is hidden (its clipped layers are then hidden too).
        let mut base: Option<usize> = None;
        let mut base_hidden = false;
        for (i, children) in siblings(self.layers, range) {
            let l = &self.layers[i];
            let shown = l.visible && self.opts.hidden != Some(i);
            let owned = children.clone().unwrap_or(i..i).chain([i]);
            let mark = |b: &mut Self, n: usize| {
                if let Some(t) = top {
                    for j in owned {
                        b.owner[j] = Some(t + n);
                    }
                }
            };
            if l.clipped && !l.is_group() && (base.is_some() || base_hidden) {
                if base_hidden || !shown {
                    continue;
                }
                if let Some(n) = self.layer(i)
                    && let Some(Node::Layer(b)) = base.and_then(|k| nodes.get_mut(k))
                {
                    b.clipped.push(n);
                    let k = base.expect("checked");
                    mark(self, k);
                }
                continue;
            }
            base = None;
            base_hidden = !shown;
            if !shown {
                continue;
            }
            let node = match children {
                Some(c) => self.group(i, c),
                None => self.layer(i).map(|n| Node::Layer(Box::new(n))),
            };
            if let Some(node) = node {
                let clippable =
                    matches!(&node, Node::Layer(n) if !matches!(n.content, Content::Adjust(_)));
                if clippable {
                    base = Some(nodes.len());
                }
                mark(self, nodes.len());
                nodes.push(node);
            }
        }
        nodes
    }

    /// The image of a transform being prepared, for layer `i`.
    fn placed(&self, i: usize) -> Option<LayerNode> {
        let p = self.opts.placed.iter().find(|p| p.layer == i)?;
        let l = &self.layers[i];
        Some(LayerNode {
            content: Content::Pixels(Src {
                image: Arc::clone(p.image),
                at: p.at,
            }),
            mask: None,
            blend: match self.blend(l) {
                BlendMode::PassThrough => BlendMode::Normal,
                m => m,
            },
            opacity: 1.0,
            fill: 1.0,
            exterior: Vec::new(),
            interior: Vec::new(),
            clipped: Vec::new(),
        })
    }

    fn group(&mut self, i: usize, children: Range<usize>) -> Option<Node> {
        if let Some(n) = self.placed(i) {
            return Some(Node::Layer(Box::new(n)));
        }
        let l = &self.layers[i];
        let children = self.level(children, None);
        let level = self.sources.level();
        let board = l.artboard_rect().map(|r| Board {
            rect: reduced(r, level),
            background: l
                .artboard
                .and_then(|a| a.background.color())
                .map(|c| c.map(|v| v as f32 / 255.0)),
        });
        Some(Node::Group(Box::new(GroupNode {
            children,
            blend: self.blend(l),
            opacity: l.opacity,
            mask: self.mask(l),
            board,
        })))
    }

    fn blend(&self, l: &Layer) -> BlendMode {
        match self.opts.blend {
            Some((id, mode)) if id == l.id => mode,
            _ => l.blend,
        }
    }

    fn mask(&mut self, l: &Layer) -> Option<MaskSrc> {
        let m = l.active_mask()?;
        let key = SourceKey {
            layer: l.id,
            slot: 1,
        };
        let src = match (self.opts.placed_mask, self.opts.mask) {
            (Some(p), _) if p.layer == l.id => Src {
                image: Arc::clone(p.image),
                at: p.at,
            },
            (_, Some(p)) if p.layer == l.id => self.sources.gray(key, p.version, p.image, p.origin),
            _ => {
                let (image, origin) = m.shaped(l.offset);
                let version = hash_of((m.rev(), m.feather.to_bits()));
                self.sources.gray(key, version, &image, origin)
            }
        };
        Some(MaskSrc {
            src,
            default: m.default as f32 / 255.0,
            density: m.density.clamp(0.0, 1.0),
        })
    }

    fn layer(&mut self, i: usize) -> Option<LayerNode> {
        if let Some(n) = self.placed(i) {
            return Some(n);
        }
        let l = &self.layers[i];
        let content = match &l.kind {
            LayerKind::Pixels => {
                let key = SourceKey {
                    layer: l.id,
                    slot: 0,
                };
                let src = match self.opts.pixels {
                    Some(p) if p.layer == l.id => {
                        self.sources.rgba(key, p.version, p.image, p.origin)
                    }
                    _ => self.sources.rgba(key, l.rev(), &l.shared_image(), l.offset),
                };
                if src.image.width() == 0 || src.image.height() == 0 {
                    return None;
                }
                Content::Pixels(src)
            }
            LayerKind::Adjustment(a) => Content::Adjust(Box::new(a.clone())),
            LayerKind::Fill(Fill::Solid(c)) => Content::Solid(c.map(|v| v as f32 / 255.0)),
            LayerKind::Fill(Fill::Gradient(g)) => Content::Gradient(Box::new(Ramp::new(g))),
            LayerKind::Group { .. } => return None,
        };
        let mask = self.mask(l);
        let mut exterior = Vec::new();
        let mut interior = Vec::new();
        if let Content::Pixels(_) = content {
            let fx = &l.effects;
            if fx.gradient_overlay.enabled {
                let g = &fx.gradient_overlay;
                let b = l.content_bounds().unwrap_or(l.rect());
                interior.push(Interior::Gradient {
                    ramp: Box::new(Ramp::new(&g.fill)),
                    bounds: [b.x as f32, b.y as f32, b.w as f32, b.h as f32],
                    opacity: g.opacity,
                    blend: g.blend,
                });
            }
            if fx.color_overlay.enabled {
                let c = &fx.color_overlay;
                interior.push(Interior::Color {
                    color: rgb(c.color),
                    opacity: c.color[3] as f32 / 255.0 * c.opacity,
                    blend: c.blend,
                });
            }
            if let Some(styles) = l.styles_shown(self.opts.held, self.opts.later_styles) {
                let mut slot = 2;
                let mut src = |s: &mut Self, f: &crate::compose::styles::FxImage| {
                    let key = SourceKey { layer: l.id, slot };
                    slot += 1;
                    let at = (l.offset.0 + f.offset.0, l.offset.1 + f.offset.1);
                    FxSrc {
                        coverage: s.sources.gray(key, styles.version, &f.coverage, at),
                        color: f.color.map(|v| v as f32 / 255.0),
                        opacity: f.opacity,
                        blend: f.blend,
                    }
                };
                exterior = styles.exterior.iter().map(|f| src(self, f)).collect();
                interior.extend(styles.interior.iter().map(|f| Interior::Fx(src(self, f))));
            }
        }
        Some(LayerNode {
            content,
            mask,
            blend: self.blend(l),
            opacity: l.opacity,
            fill: l.fill,
            exterior,
            interior,
            clipped: Vec::new(),
        })
    }
}

/// What the display compares from one frame to the next, for one layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sig {
    pub id: u64,
    /// Where it is in the structure (a change: everything again).
    pub structure: u64,
    /// Everything else it shows depends on.
    pub state: u64,
    /// Version of its pixels.
    pub rev: u64,
    /// Area it can change (canvas pixels); `None`: everywhere.
    pub extent: Option<PixelRect>,
}

/// Fingerprints of the layers as `opts` shows them.
pub fn signatures(layers: &[Layer], opts: &Options) -> Vec<Sig> {
    // Shown, its groups included.
    let mut shown = vec![true; layers.len()];
    for (i, l) in layers.iter().enumerate().rev() {
        if !l.visible || opts.hidden == Some(i) {
            shown[i] = false;
        }
        if l.is_group() && !shown[i] {
            let mut j = i;
            while j > 0 && layers[j - 1].depth > l.depth {
                j -= 1;
                shown[j] = false;
            }
        }
    }
    if let Some(only) = opts.only {
        shown
            .iter_mut()
            .enumerate()
            .for_each(|(i, s)| *s &= i == only);
    }
    // A placed group stands for its layers.
    for p in opts
        .placed
        .iter()
        .filter(|p| layers.get(p.layer).is_some_and(Layer::is_group))
    {
        let mut j = p.layer;
        while j > 0 && layers[j - 1].depth > layers[p.layer].depth {
            j -= 1;
            shown[j] = false;
        }
    }
    let mut sigs: Vec<Sig> = Vec::with_capacity(layers.len());
    for (i, l) in layers.iter().enumerate() {
        let kind = std::mem::discriminant(&l.kind);
        let structure = hash_of((
            l.id,
            l.depth,
            l.clipped,
            kind,
            opts.mask_view,
            l.artboard.is_some(),
        ));
        let (pixels_rev, pixels) = match opts.pixels {
            Some(p) if p.layer == l.id => (p.version, true),
            _ => (l.rev(), false),
        };
        // The mask's pixels count with the layer's (a brush stroke on
        // either changes only what it painted); its settings with the rest.
        let placed_mask = opts.placed_mask.filter(|p| p.layer == l.id);
        let (mask_rev, mask) = match (placed_mask, opts.mask, l.active_mask()) {
            (Some(p), _, Some(_)) => (p.version, hash_of(p.rect)),
            (_, Some(p), _) if p.layer == l.id => (p.version, hash_of(p.origin)),
            (_, _, Some(m)) => (
                m.rev(),
                debug_hash(&(m.origin(l.offset), m.default, m.density, m.feather)),
            ),
            _ => (0, 0),
        };
        let placed = opts.placed.iter().find(|p| p.layer == i);
        let rev = hash_of((pixels_rev, mask_rev, placed.map(|p| p.version)));
        let blend = match opts.blend {
            Some((id, mode)) if id == l.id => mode,
            _ => l.blend,
        };
        // The styles shown (a hidden layer: as last drawn).
        let styles = if shown[i] {
            l.styles_shown(opts.held, opts.later_styles)
        } else {
            l.styles_cached()
        };
        let state = hash_of((
            shown[i],
            [l.opacity.to_bits(), l.fill.to_bits()],
            blend,
            l.offset,
            (l.image().width(), l.image().height()),
            mask,
            debug_hash(&l.kind),
            debug_hash(&l.effects),
            (
                styles.as_ref().map(|s| s.version),
                pixels,
                opts.only.is_some(),
            ),
            l.artboard,
        ));
        let extent = if let Some(p) = placed {
            Some(p.rect)
        } else if let Some(p) =
            placed_mask.filter(|_| l.mask.as_ref().is_some_and(|m| m.default == 0))
        {
            // Hidden outside the mask: only where it now is.
            Some(p.rect)
        } else if let Some(r) = l.artboard_rect() {
            // Its layers and background, cut to it.
            Some(r)
        } else if l.is_group() {
            // Its layers: just below it.
            let mut j = i;
            let mut area: Option<Option<PixelRect>> = None;
            while j > 0 && layers[j - 1].depth > l.depth {
                j -= 1;
                let e = sigs[j].extent;
                area = Some(match (area, e) {
                    (None, e) => e,
                    (Some(Some(a)), Some(e)) => Some(a.union(e)),
                    _ => None,
                });
            }
            let area = area.unwrap_or(Some(PixelRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            }));
            match (area, l.active_mask().filter(|m| m.default == 0)) {
                (Some(a), Some(m)) => {
                    intersect(a, m.rect(l.offset)).or(Some(PixelRect { w: 0, h: 0, ..a }))
                }
                (None, Some(m)) => Some(m.rect(l.offset)),
                (a, None) => a,
            }
        } else {
            l.extent_with(styles.as_deref())
        };
        sigs.push(Sig {
            id: l.id,
            structure,
            state,
            rev,
            extent,
        });
    }
    sigs
}

/// What changed between two frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dirty {
    Nothing,
    All,
    Area(PixelRect),
}

/// Area to composite again between the fingerprints `old` and `new`.
/// `strokes`: canvas areas the brush painted on layers (id) since then,
/// on their pixels or mask (their styles are held meanwhile).
pub fn dirty_area(old: &[Sig], new: &[Sig], strokes: &[(u64, PixelRect)]) -> Dirty {
    if old.len() != new.len() || old.iter().zip(new).any(|(a, b)| a.structure != b.structure) {
        return Dirty::All;
    }
    let mut area: Option<PixelRect> = None;
    let mut add = |r: PixelRect| {
        area = Some(area.map_or(r, |a| a.union(r)));
    };
    for (a, b) in old.iter().zip(new) {
        if a == b {
            continue;
        }
        let stroke = strokes
            .iter()
            .filter(|s| s.0 == b.id)
            .map(|s| s.1)
            .reduce(PixelRect::union);
        if let Some(r) = stroke
            && a.state == b.state
        {
            add(r);
            continue;
        }
        match (a.extent, b.extent) {
            (Some(x), Some(y)) => {
                add(x);
                add(y);
            }
            _ => return Dirty::All,
        }
    }
    match area {
        Some(a) if a.w > 0 && a.h > 0 => Dirty::Area(a),
        _ => Dirty::Nothing,
    }
}
