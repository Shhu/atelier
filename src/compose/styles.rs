//! Layer styles (Photoshop's Layer Style window): drop shadow, outer glow
//! and stroke around the layer; color and gradient overlays, inner glow,
//! inner shadow and bevel inside it. They are computed from the layer's
//! visible shape, each with its own color, opacity and blend mode.

use std::sync::Arc;

use image::{GrayImage, RgbaImage};

use super::BlendMode;
use super::gradient::GradientFill;
use crate::i18n::t;
use crate::pixel_ops::{blur, content_box, distance_field};

/// Outside stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeFx {
    pub enabled: bool,
    pub color: [u8; 4],
    /// Width in pixels.
    pub width: f32,
    pub opacity: f32,
    pub blend: BlendMode,
}

/// Drop shadow, or inner shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowFx {
    pub enabled: bool,
    pub color: [u8; 4],
    pub opacity: f32,
    /// Offset in pixels.
    pub dx: f32,
    pub dy: f32,
    /// Blur radius in pixels ("Size").
    pub blur: f32,
    pub blend: BlendMode,
}

/// Outer or inner glow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlowFx {
    pub enabled: bool,
    pub color: [u8; 4],
    pub opacity: f32,
    /// Blur radius in pixels.
    pub size: f32,
    /// 0..1: how much of the glow stays solid ("Spread", "Choke").
    pub spread: f32,
    pub blend: BlendMode,
}

/// Color overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayFx {
    pub enabled: bool,
    pub color: [u8; 4],
    pub opacity: f32,
    pub blend: BlendMode,
}

/// Gradient overlay, over the box of the layer's content.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientFx {
    pub enabled: bool,
    pub fill: GradientFill,
    pub opacity: f32,
    pub blend: BlendMode,
}

/// Bevel & Emboss (inner bevel, smooth): highlights and shadows of a
/// relief lit from `angle` (degrees) at `altitude` (degrees).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BevelFx {
    pub enabled: bool,
    /// Relief strength, 1 = 100 %.
    pub depth: f32,
    /// Width of the slope in pixels.
    pub size: f32,
    pub soften: f32,
    pub angle: f32,
    pub altitude: f32,
    /// Direction: up (`true`) or down.
    pub up: bool,
    pub highlight: [u8; 4],
    pub highlight_opacity: f32,
    pub highlight_blend: BlendMode,
    pub shadow: [u8; 4],
    pub shadow_opacity: f32,
    pub shadow_blend: BlendMode,
}

/// Non-destructive styles of a layer.
#[derive(Clone, Debug, PartialEq)]
pub struct Effects {
    pub stroke: StrokeFx,
    pub shadow: ShadowFx,
    pub inner_shadow: ShadowFx,
    pub outer_glow: GlowFx,
    pub inner_glow: GlowFx,
    pub color_overlay: OverlayFx,
    pub gradient_overlay: GradientFx,
    pub bevel: BevelFx,
}

/// Photoshop's defaults (the stroke and drop shadow: Atelier's own).
impl Default for Effects {
    fn default() -> Self {
        let glow = GlowFx {
            enabled: false,
            color: [255, 255, 190, 255],
            opacity: 0.75,
            size: 5.0,
            spread: 0.0,
            blend: BlendMode::Screen,
        };
        Self {
            stroke: StrokeFx {
                enabled: false,
                color: [255, 255, 255, 255],
                width: 3.0,
                opacity: 1.0,
                blend: BlendMode::Normal,
            },
            shadow: ShadowFx {
                enabled: false,
                color: [0, 0, 0, 255],
                opacity: 0.6,
                dx: 4.0,
                dy: 4.0,
                blur: 6.0,
                blend: BlendMode::Multiply,
            },
            inner_shadow: ShadowFx {
                enabled: false,
                color: [0, 0, 0, 255],
                opacity: 0.75,
                dx: 3.0,
                dy: 4.0,
                blur: 5.0,
                blend: BlendMode::Multiply,
            },
            outer_glow: glow,
            inner_glow: glow,
            color_overlay: OverlayFx {
                enabled: false,
                color: [255, 0, 0, 255],
                opacity: 1.0,
                blend: BlendMode::Normal,
            },
            gradient_overlay: GradientFx {
                enabled: false,
                fill: GradientFill::default(),
                opacity: 1.0,
                blend: BlendMode::Normal,
            },
            bevel: BevelFx {
                enabled: false,
                depth: 1.0,
                size: 5.0,
                soften: 0.0,
                angle: 120.0,
                altitude: 30.0,
                up: true,
                highlight: [255, 255, 255, 255],
                highlight_opacity: 0.75,
                highlight_blend: BlendMode::Screen,
                shadow: [0, 0, 0, 255],
                shadow_opacity: 0.75,
                shadow_blend: BlendMode::Multiply,
            },
        }
    }
}

/// The styles, in the order of the Layer Style window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bevel,
    Stroke,
    InnerShadow,
    InnerGlow,
    ColorOverlay,
    GradientOverlay,
    OuterGlow,
    DropShadow,
}

impl Style {
    pub const ALL: [Style; 8] = [
        Style::Bevel,
        Style::Stroke,
        Style::InnerShadow,
        Style::InnerGlow,
        Style::ColorOverlay,
        Style::GradientOverlay,
        Style::OuterGlow,
        Style::DropShadow,
    ];

    pub fn label(self) -> String {
        match self {
            Style::Bevel => t!("core-style-bevel"),
            Style::Stroke => t!("panel-stroke"),
            Style::InnerShadow => t!("core-style-inner-shadow"),
            Style::InnerGlow => t!("core-style-inner-glow"),
            Style::ColorOverlay => t!("core-style-color-overlay"),
            Style::GradientOverlay => t!("core-style-gradient-overlay"),
            Style::OuterGlow => t!("core-style-outer-glow"),
            Style::DropShadow => t!("panel-drop-shadow"),
        }
    }
}

impl Effects {
    pub fn any(&self) -> bool {
        Style::ALL.into_iter().any(|s| self.enabled(s))
    }

    pub fn enabled(&self, style: Style) -> bool {
        match style {
            Style::Bevel => self.bevel.enabled,
            Style::Stroke => self.stroke.enabled,
            Style::InnerShadow => self.inner_shadow.enabled,
            Style::InnerGlow => self.inner_glow.enabled,
            Style::ColorOverlay => self.color_overlay.enabled,
            Style::GradientOverlay => self.gradient_overlay.enabled,
            Style::OuterGlow => self.outer_glow.enabled,
            Style::DropShadow => self.shadow.enabled,
        }
    }

    pub fn enabled_mut(&mut self, style: Style) -> &mut bool {
        match style {
            Style::Bevel => &mut self.bevel.enabled,
            Style::Stroke => &mut self.stroke.enabled,
            Style::InnerShadow => &mut self.inner_shadow.enabled,
            Style::InnerGlow => &mut self.inner_glow.enabled,
            Style::ColorOverlay => &mut self.color_overlay.enabled,
            Style::GradientOverlay => &mut self.gradient_overlay.enabled,
            Style::OuterGlow => &mut self.outer_glow.enabled,
            Style::DropShadow => &mut self.shadow.enabled,
        }
    }

    /// Whether some style needs images computed from the shape (all but the
    /// overlays, drawn straight from the pixels).
    pub fn rendered(&self) -> bool {
        self.stroke.enabled
            || self.shadow.enabled
            || self.inner_shadow.enabled
            || self.outer_glow.enabled
            || self.inner_glow.enabled
            || self.bevel.enabled
    }

    /// Sizes scaled by `s`, offsets by (`sx`, `sy`) (Image Size).
    pub fn scale(&mut self, s: f32, sx: f32, sy: f32) {
        self.stroke.width *= s;
        for shadow in [&mut self.shadow, &mut self.inner_shadow] {
            shadow.dx *= sx;
            shadow.dy *= sy;
            shadow.blur *= s;
        }
        self.outer_glow.size *= s;
        self.inner_glow.size *= s;
        self.bevel.size *= s;
        self.bevel.soften *= s;
    }

    /// Offsets and light direction transformed by `f` (canvas rotation,
    /// flip).
    pub fn turn(&mut self, f: impl Fn(f32, f32) -> (f32, f32)) {
        for shadow in [&mut self.shadow, &mut self.inner_shadow] {
            (shadow.dx, shadow.dy) = f(shadow.dx, shadow.dy);
        }
        let (s, c) = self.bevel.angle.to_radians().sin_cos();
        let (x, y) = f(c, -s);
        self.bevel.angle = (-y).atan2(x).to_degrees();
    }
}

/// One style drawn as a coverage image, placed relative to the layer.
#[derive(Clone)]
pub struct FxImage {
    pub coverage: Arc<GrayImage>,
    pub offset: (i64, i64),
    pub color: [u8; 3],
    /// Color opacity × the style's opacity.
    pub opacity: f32,
    pub blend: BlendMode,
}

/// Images of a layer's styles: under the layer (drop shadow, outer glow,
/// stroke: knocked out by the content, so they never show through it), and
/// inside it (inner glow, inner shadow, bevel: to be multiplied by the
/// layer's shape), each list from the bottom up.
#[derive(Clone, Default)]
pub struct Styles {
    pub exterior: Vec<FxImage>,
    pub interior: Vec<FxImage>,
    /// Unique per drawing (cache key of the reduced images).
    pub version: u64,
}

impl Styles {
    /// Box (relative to the layer) of what the styles cover.
    pub fn extent(&self) -> Option<[i64; 4]> {
        self.exterior
            .iter()
            .map(|f| {
                let (w, h) = f.coverage.dimensions();
                [
                    f.offset.0,
                    f.offset.1,
                    f.offset.0 + w as i64,
                    f.offset.1 + h as i64,
                ]
            })
            .reduce(|a, b| {
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ]
            })
    }
}

fn alpha_of(c: [u8; 4], opacity: f32) -> f32 {
    c[3] as f32 / 255.0 * opacity.clamp(0.0, 1.0)
}

/// Draws the styles of a layer image (its mask already applied). Like
/// Photoshop, the stroke is outside the shape and the content knocks out
/// the shadow and glow.
pub fn render_styles(image: &RgbaImage, fx: &Effects) -> Styles {
    let mut out = Styles::default();
    if !fx.rendered() {
        return out;
    }
    let Some([bx, by, bw, bh]) = content_box(image) else {
        return out;
    };
    let stroke_w = if fx.stroke.enabled {
        fx.stroke.width.max(0.0)
    } else {
        0.0
    };
    let reach = |s: &ShadowFx| s.blur * 1.5 + s.dx.abs().max(s.dy.abs());
    let mut outside = stroke_w;
    if fx.shadow.enabled {
        outside += reach(&fx.shadow);
    }
    if fx.outer_glow.enabled {
        outside = outside.max(stroke_w + fx.outer_glow.size * 1.5);
    }
    let inner = [
        fx.inner_shadow.enabled.then(|| reach(&fx.inner_shadow)),
        fx.inner_glow.enabled.then_some(fx.inner_glow.size * 1.5),
        fx.bevel.enabled.then_some(fx.bevel.size + fx.bevel.soften),
    ]
    .into_iter()
    .flatten()
    .fold(0.0f32, f32::max);
    let margin = outside.max(inner).ceil() as usize + 2;
    let (bx, by, bw, bh) = (bx as usize, by as usize, bw as usize, bh as usize);
    let (w, h) = (bw + 2 * margin, bh + 2 * margin);
    let iw = image.width() as usize;

    // Silhouette of the content, enlarged by the margin.
    let mut alpha = vec![0.0f32; w * h];
    for (y, row) in image
        .as_raw()
        .chunks_exact(iw * 4)
        .skip(by)
        .take(bh)
        .enumerate()
    {
        let line = &mut alpha[(y + margin) * w + margin..][..bw];
        for (a, p) in line.iter_mut().zip(row[bx * 4..].chunks_exact(4)) {
            *a = p[3] as f32 / 255.0;
        }
    }
    let inside: Vec<bool> = alpha.iter().map(|&a| a >= 0.5).collect();
    // Placement of a field of the whole area, and of the content box.
    let around = (bx as i64 - margin as i64, by as i64 - margin as i64);
    let to_image = |field: &[f32], whole: bool| -> (Arc<GrayImage>, (i64, i64)) {
        let (x0, y0, fw, fh) = if whole {
            (0, 0, w, h)
        } else {
            (margin, margin, bw, bh)
        };
        let image = GrayImage::from_fn(fw as u32, fh as u32, |x, y| {
            let v = field[(y0 + y as usize) * w + x0 + x as usize];
            image::Luma([(v.clamp(0.0, 1.0) * 255.0).round() as u8])
        });
        let at = (around.0 + x0 as i64, around.1 + y0 as i64);
        (Arc::new(image), at)
    };
    let fx_image = |field: &[f32], whole: bool, color: [u8; 4], opacity: f32, blend| {
        let (coverage, offset) = to_image(field, whole);
        FxImage {
            coverage,
            offset,
            color: [color[0], color[1], color[2]],
            opacity: alpha_of(color, opacity),
            blend,
        }
    };

    // Stroke: coverage based on the distance to the shape (smoothed edges).
    // The distance goes from center to center; the edge of the shape is half
    // a pixel beyond the center of its last pixel, hence the "+ 1". Inside
    // the shape, only its edge pixels keep the stroke behind them (no gap
    // under their anti-aliasing).
    let stroke: Option<Vec<f32>> = (stroke_w > 0.0).then(|| {
        let mut field = distance_field(&inside, w, h);
        for (i, d) in field.iter_mut().enumerate() {
            let edge = !inside[i] || [i - 1, i + 1, i - w, i + w].iter().any(|&n| !inside[n]);
            *d = if edge {
                (stroke_w + 1.0 - *d).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
        field
    });
    // What the outside styles follow: the shape and its stroke.
    let outline = || {
        let mut shape = alpha.clone();
        if let Some(st) = &stroke {
            for (a, c) in shape.iter_mut().zip(st) {
                *a = a.max(*c);
            }
        }
        shape
    };

    let s = &fx.shadow;
    if s.enabled {
        let mut shape = outline();
        blur(&mut shape, w, h, s.blur);
        let shifted = shift(&shape, w, h, s.dx, s.dy);
        let field: Vec<f32> = shifted
            .iter()
            .zip(&alpha)
            .map(|(v, a)| v * (1.0 - a))
            .collect();
        out.exterior
            .push(fx_image(&field, true, s.color, s.opacity, s.blend));
    }
    let g = &fx.outer_glow;
    if g.enabled {
        let mut shape = outline();
        blur(&mut shape, w, h, g.size);
        let gain = 1.0 / (1.0 - g.spread.clamp(0.0, 0.99));
        let field: Vec<f32> = shape
            .iter()
            .zip(&alpha)
            .map(|(v, a)| (v * gain).min(1.0) * (1.0 - a))
            .collect();
        out.exterior
            .push(fx_image(&field, true, g.color, g.opacity, g.blend));
    }
    if let Some(st) = &stroke {
        let s = &fx.stroke;
        out.exterior
            .push(fx_image(st, true, s.color, s.opacity, s.blend));
    }

    let g = &fx.inner_glow;
    if g.enabled {
        let mut shape = alpha.clone();
        blur(&mut shape, w, h, g.size);
        let gain = 2.0 / (1.0 - g.spread.clamp(0.0, 0.99));
        let field: Vec<f32> = shape.iter().map(|v| ((1.0 - v) * gain).min(1.0)).collect();
        out.interior
            .push(fx_image(&field, false, g.color, g.opacity, g.blend));
    }
    let s = &fx.inner_shadow;
    if s.enabled {
        let mut shape = alpha.clone();
        blur(&mut shape, w, h, s.blur);
        let shifted = shift(&shape, w, h, s.dx, s.dy);
        let field: Vec<f32> = shifted.iter().map(|v| 1.0 - v).collect();
        out.interior
            .push(fx_image(&field, false, s.color, s.opacity, s.blend));
    }
    let b = &fx.bevel;
    if b.enabled {
        let (highlight, shadow) = bevel(&inside, w, h, b);
        out.interior.push(fx_image(
            &shadow,
            false,
            b.shadow,
            b.shadow_opacity,
            b.shadow_blend,
        ));
        out.interior.push(fx_image(
            &highlight,
            false,
            b.highlight,
            b.highlight_opacity,
            b.highlight_blend,
        ));
    }
    out
}

/// `field` moved by (`dx`, `dy`) (rounded), zero where nothing comes in.
fn shift(field: &[f32], w: usize, h: usize, dx: f32, dy: f32) -> Vec<f32> {
    let (dx, dy) = (dx.round() as i64, dy.round() as i64);
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i64 {
        let sy = y - dy;
        if sy < 0 || sy >= h as i64 {
            continue;
        }
        for x in 0..w as i64 {
            let sx = x - dx;
            if sx >= 0 && sx < w as i64 {
                out[(y * w as i64 + x) as usize] = field[(sy * w as i64 + sx) as usize];
            }
        }
    }
    out
}

/// Highlight and shadow coverages of an inner bevel: the height rises
/// over `size` pixels from the edge inwards; its slope, lit from the
/// bevel's angle and altitude, is brighter or darker than a flat surface.
fn bevel(inside: &[bool], w: usize, h: usize, b: &BevelFx) -> (Vec<f32>, Vec<f32>) {
    let outside: Vec<bool> = inside.iter().map(|i| !i).collect();
    let size = b.size.max(1.0);
    let mut height: Vec<f32> = distance_field(&outside, w, h)
        .into_iter()
        .map(|d| {
            let t = (d / size).min(1.0);
            // Smooth: rounded at the top and at the edge.
            t * t * (3.0 - 2.0 * t)
        })
        .collect();
    blur(&mut height, w, h, b.soften.max(1.0));
    let (alt, ang) = (b.altitude.to_radians(), b.angle.to_radians());
    let light = [alt.cos() * ang.cos(), -alt.cos() * ang.sin(), alt.sin()];
    let flat = light[2];
    let depth = b.depth.max(0.0) * size * if b.up { 1.0 } else { -1.0 };
    let mut highlight = vec![0.0f32; w * h];
    let mut shadow = vec![0.0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let gx = (height[i + 1] - height[i - 1]) / 2.0 * depth;
            let gy = (height[i + w] - height[i - w]) / 2.0 * depth;
            let n = (gx * gx + gy * gy + 1.0).sqrt();
            let lit = (-gx * light[0] - gy * light[1] + light[2]) / n;
            if lit > flat {
                highlight[i] = ((lit - flat) / (1.0 - flat).max(1e-3)).min(1.0);
            } else {
                shadow[i] = ((flat - lit) / flat.max(1e-3)).min(1.0);
            }
        }
    }
    (highlight, shadow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn square() -> RgbaImage {
        let mut img = RgbaImage::new(30, 30);
        for y in 10..20 {
            for x in 10..20 {
                img.put_pixel(x, y, Rgba([200, 0, 0, 255]));
            }
        }
        img
    }

    fn at(f: &FxImage, x: i64, y: i64) -> u8 {
        let (lx, ly) = (x - f.offset.0, y - f.offset.1);
        f.coverage.get_pixel(lx as u32, ly as u32).0[0]
    }

    #[test]
    fn outside_styles_stay_out_of_the_content() {
        let mut fx = Effects::default();
        fx.stroke.enabled = true;
        fx.shadow.enabled = true;
        fx.outer_glow.enabled = true;
        let styles = render_styles(&square(), &fx);
        assert_eq!(styles.exterior.len(), 3);
        for f in &styles.exterior {
            assert_eq!(at(f, 15, 15), 0, "{:?}", f.blend);
        }
        let [shadow, glow, stroke] = &styles.exterior[..] else {
            unreachable!()
        };
        assert_eq!(shadow.blend, BlendMode::Multiply);
        assert_eq!(glow.blend, BlendMode::Screen);
        assert!(at(stroke, 8, 15) > 200);
        // The shadow goes down-right.
        assert!(at(shadow, 22, 22) > at(shadow, 7, 7));
        assert!(at(glow, 7, 15) > 0);
    }

    #[test]
    fn stroke_surrounds_the_shape() {
        let img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        let mut fx = Effects::default();
        fx.stroke.enabled = true;
        fx.stroke.width = 2.0;
        let styles = render_styles(&img, &fx);
        let stroke = &styles.exterior[0];
        assert_eq!(at(stroke, -1, 1), 255); // 1 px from the edge: full
        assert_eq!(at(stroke, -2, 1), 255); // at 2 px: still covered
        assert_eq!(at(stroke, -4, 1), 0); // beyond: nothing
        assert!(render_styles(&img, &Effects::default()).exterior.is_empty());
        // Semi-transparent square: no stroke under it, but under its edge.
        let img = RgbaImage::from_pixel(8, 8, Rgba([255, 255, 255, 160]));
        let styles = render_styles(&img, &fx);
        let stroke = &styles.exterior[0];
        assert_eq!(at(stroke, 4, 4), 0, "under the content");
        assert_eq!(
            at(stroke, 0, 4),
            255,
            "edge pixel: the stroke joins the content"
        );
    }

    #[test]
    fn shadow_is_offset_and_tight() {
        let img = RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255]));
        let mut fx = Effects::default();
        fx.shadow = ShadowFx {
            enabled: true,
            opacity: 1.0,
            dx: 10.0,
            dy: 0.0,
            blur: 0.0,
            ..fx.shadow
        };
        let styles = render_styles(&img, &fx);
        let shadow = &styles.exterior[0];
        assert_eq!(at(shadow, 11, 1), 255);
        assert_eq!(at(shadow, 1, 1), 0);
        // The styles only cover the content and its margin.
        let mut big = RgbaImage::new(500, 500);
        big.put_pixel(250, 250, Rgba([255; 4]));
        let styles = render_styles(&big, &fx);
        let [x0, _, x1, _] = styles.extent().unwrap();
        assert!(x1 - x0 < 100 && x0 > 200, "{x0}..{x1}");
        assert!(
            render_styles(&RgbaImage::new(5, 5), &fx)
                .exterior
                .is_empty()
        );
    }

    #[test]
    fn inside_styles_follow_the_edges() {
        let mut fx = Effects::default();
        fx.inner_glow.enabled = true;
        fx.inner_shadow.enabled = true;
        fx.bevel.enabled = true;
        let styles = render_styles(&square(), &fx);
        assert_eq!(styles.interior.len(), 4);
        let glow = &styles.interior[0];
        assert!(at(glow, 10, 15) > at(glow, 15, 15));
        // Inner shadow: on the side the light comes from (top left).
        let shadow = &styles.interior[1];
        assert!(at(shadow, 10, 10) > at(shadow, 19, 19));
        // Bevel lit from the top left: highlight there, shadow opposite.
        let (dark, light) = (&styles.interior[2], &styles.interior[3]);
        assert!(at(light, 11, 11) > at(light, 18, 18));
        assert!(at(dark, 18, 18) > at(dark, 11, 11));
        assert!(styles.extent().is_none());
    }
}
