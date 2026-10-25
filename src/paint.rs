//! Brush strokes: dabs of a tip (round or sampled) placed along the
//! pointer's path at a spacing, with flow, opacity and pen pressure. The
//! painting tools are built on them: brush and eraser, Dodge / Burn /
//! Sponge, Blur / Sharpen / Smudge, Clone Stamp and the healing brushes.

pub mod gradient;
pub mod tips;
mod tone;

use std::sync::Arc;

use image::RgbaImage;

use crate::pixel_ops::{Clip, over};
pub use tips::{Preset, Tip};
pub use tone::ToneRange;

/// What a stroke does to the pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StrokeMode {
    Paint,
    Erase,
    /// Dodge (lighten) or burn (darken) `range`; `protect`: Protect Tones.
    Tone {
        burn: bool,
        range: ToneRange,
        protect: bool,
    },
    /// Sponge: saturate or desaturate; `vibrance` spares saturated colors.
    Sponge {
        saturate: bool,
        vibrance: bool,
    },
    Blur,
    Sharpen,
    /// Smudge; `finger` (Finger Painting): the stroke starts with this color.
    Smudge {
        finger: Option<[u8; 4]>,
    },
    /// Clone Stamp: paints the pixels of a source (see [`Stroke::with_clone`]).
    Clone,
}

impl StrokeMode {
    /// Modes recomposited from the pixels before the stroke and a coverage
    /// mask: going over the same spot builds up only with the flow, never
    /// beyond the opacity. The others change the pixels dab after dab.
    fn masked(self) -> bool {
        matches!(
            self,
            StrokeMode::Paint
                | StrokeMode::Erase
                | StrokeMode::Tone { .. }
                | StrokeMode::Sponge { .. }
                | StrokeMode::Clone
        )
    }
}

/// Brush tip and painting settings.
#[derive(Clone)]
pub struct BrushSettings {
    pub color: [u8; 4],
    pub tip: Tip,
    /// Diameter in image pixels.
    pub size: f32,
    /// 0 = very soft, 1 = hard edge (round tip).
    pub hardness: f32,
    /// Distance between dabs, as a fraction of the diameter.
    pub spacing: f32,
    /// Angle of the tip, degrees counterclockwise.
    pub angle: f32,
    /// Short axis over long axis, 0..1.
    pub roundness: f32,
    /// Maximum effect of the stroke, 0..1 (the brush's opacity, the dodge's
    /// exposure).
    pub opacity: f32,
    /// Effect of each dab, 0..1 (the brush's flow, the blur's strength).
    pub flow: f32,
    /// Pen pressure scales the diameter, the opacity.
    pub pressure_size: bool,
    pub pressure_opacity: bool,
}

impl BrushSettings {
    /// Round tip with Photoshop's defaults for the rest.
    pub fn round(color: [u8; 4], size: f32, hardness: f32, opacity: f32) -> Self {
        Self {
            color,
            tip: Tip::Round,
            size,
            hardness,
            spacing: 0.25,
            angle: 0.0,
            roundness: 1.0,
            opacity,
            flow: 1.0,
            pressure_size: false,
            pressure_opacity: false,
        }
    }

    /// Diameter of a dab at `pressure`.
    fn diameter(&self, pressure: f32) -> f32 {
        if self.pressure_size {
            (self.size * pressure).max(1.0)
        } else {
            self.size
        }
    }

    /// Distance to the next dab after one at `pressure`.
    fn step(&self, pressure: f32) -> f32 {
        (self.diameter(pressure) * self.spacing).max(0.5)
    }

    /// Opacity scale of a dab at `pressure`.
    fn pressure_scale(&self, pressure: f32) -> f32 {
        if self.pressure_opacity { pressure } else { 1.0 }
    }
}

/// Rectangle of modified pixels: x, y, width, height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl DirtyRect {
    pub fn union(self, other: DirtyRect) -> DirtyRect {
        let x0 = self.x.min(other.x);
        let y0 = self.y.min(other.y);
        let x1 = (self.x + self.w).max(other.x + other.w);
        let y1 = (self.y + self.h).max(other.y + other.h);
        DirtyRect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }
}

/// The paint picked up by the smudge: a square around the dab's center,
/// premultiplied colors in 0..1.
struct Carried {
    size: usize,
    pixels: Vec<[u16; 4]>,
}

/// A stroke in progress.
pub struct Stroke {
    /// Pixels from before the stroke, shared with the history (no copy).
    base: Arc<RgbaImage>,
    /// Masked modes: coverage of each pixel (0..65535), built up by the dabs.
    mask: Vec<u16>,
    mode: StrokeMode,
    settings: BrushSettings,
    /// Last point reached: x, y, pressure.
    last: Option<(f32, f32, f32)>,
    /// Distance left before the next dab.
    to_next: f32,
    /// Allowed area, in image coordinates.
    clip: Option<Clip>,
    /// Some pixel changed (the stroke modifies the image).
    painted: bool,
    /// Sample All Layers: what the image looks like (layer coordinates),
    /// read by blur, sharpen and smudge and kept up to date.
    source: Option<RgbaImage>,
    carried: Option<Carried>,
    /// Clone Stamp: the image sampled, and where pixel (x, y) of the layer
    /// takes its color in it: (x + dx, y + dy).
    clone: Option<(Arc<RgbaImage>, (i64, i64))>,
    /// Tip coverage of the current dab, row by row over its rectangle.
    scratch: Vec<f32>,
    dabs: usize,
    /// The area the dabs reached so far.
    touched: Option<DirtyRect>,
}

impl Stroke {
    pub fn begin(base: Arc<RgbaImage>, mode: StrokeMode, settings: BrushSettings) -> Self {
        let mask = if mode.masked() {
            // Memory zeroed by the system, touched only where painting happens.
            vec![0; (base.width() * base.height()) as usize]
        } else {
            Vec::new()
        };
        Self {
            base,
            mask,
            mode,
            settings,
            last: None,
            to_next: 0.0,
            clip: None,
            painted: false,
            source: None,
            carried: None,
            clone: None,
            scratch: Vec::new(),
            dabs: 0,
            touched: None,
        }
    }

    /// Whether the stroke has changed any pixel so far.
    pub fn painted(&self) -> bool {
        self.painted
    }

    /// Whether `image`, the stroke's target, differs from the pixels before
    /// the stroke where the dabs reached, or `margin` pixels around (a
    /// stroke can cover pixels without changing them: a transparent clone
    /// source, eraser on empty pixels…).
    pub fn changes(&self, image: &RgbaImage, margin: u32) -> bool {
        if std::ptr::eq(image, &*self.base) {
            return false;
        }
        let Some(r) = self.touched else {
            return false;
        };
        if image.dimensions() != self.base.dimensions() {
            return true;
        }
        let (w, h) = image.dimensions();
        let (x0, y0) = (r.x.saturating_sub(margin), r.y.saturating_sub(margin));
        let (x1, y1) = ((r.x + r.w + margin).min(w), (r.y + r.h + margin).min(h));
        let (a, b) = (image.as_raw(), self.base.as_raw());
        (y0..y1).any(|y| {
            let (from, to) = (((y * w + x0) * 4) as usize, ((y * w + x1) * 4) as usize);
            a[from..to] != b[from..to]
        })
    }

    /// Number of dabs placed.
    #[cfg(test)]
    pub fn dabs(&self) -> usize {
        self.dabs
    }

    /// Limits the stroke to an area of the image.
    pub fn with_clip(mut self, clip: Option<Clip>) -> Self {
        self.clip = clip;
        self
    }

    /// Sample All Layers: `visible` is what the image looks like, in the
    /// layer's coordinates (same size).
    pub fn with_source(mut self, visible: Option<RgbaImage>) -> Self {
        self.source = visible.filter(|v| v.dimensions() == self.base.dimensions());
        self
    }

    /// Clone Stamp: pixel (x, y) is painted with pixel (x + dx, y + dy) of
    /// `source`; nothing where that is outside it.
    pub fn with_clone(mut self, source: Arc<RgbaImage>, offset: (i64, i64)) -> Self {
        self.clone = Some((source, offset));
        self
    }

    /// Clone Stamp: the image sampled and the offset to it.
    pub fn clone_source(&self) -> Option<(&RgbaImage, (i64, i64))> {
        self.clone
            .as_ref()
            .map(|(image, offset)| (&**image, *offset))
    }

    /// Masked modes: the pixels before the stroke.
    pub fn base(&self) -> &Arc<RgbaImage> {
        &self.base
    }

    /// Masked modes: coverage of each pixel (0..65535), row by row.
    pub fn coverage(&self) -> &[u16] {
        &self.mask
    }

    /// Masked modes: [`Self::coverage`], taken out of the stroke (ended).
    pub fn take_coverage(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.mask)
    }

    /// Masked modes: the coverage of a pixel at full opacity.
    pub fn full_coverage(&self) -> f32 {
        self.settings.opacity.clamp(0.0, 1.0) * 65535.0
    }

    /// Extends the stroke to the point (x, y) in image coordinates, at pen
    /// `pressure` (1 without a pen): dabs every `spacing` along the way.
    pub fn add_point(
        &mut self,
        image: &mut RgbaImage,
        x: f32,
        y: f32,
        pressure: f32,
    ) -> Option<DirtyRect> {
        let pressure = pressure.clamp(0.0, 1.0);
        let mut dirty = None;
        match self.last {
            None => {
                self.dab(image, x, y, pressure, &mut dirty);
                self.to_next = self.settings.step(pressure);
            }
            Some((lx, ly, lp)) => {
                let dist = (x - lx).hypot(y - ly);
                let mut along = 0.0;
                while self.to_next <= dist - along {
                    along += self.to_next;
                    let t = along / dist;
                    let p = lp + (pressure - lp) * t;
                    self.dab(image, lx + (x - lx) * t, ly + (y - ly) * t, p, &mut dirty);
                    self.to_next = self.settings.step(p);
                }
                self.to_next -= dist - along;
            }
        }
        self.last = Some((x, y, pressure));
        self.finish(image, dirty)
    }

    /// Airbrush: one more dab where the stroke is (the pointer held still
    /// keeps building up).
    pub fn repeat(&mut self, image: &mut RgbaImage) -> Option<DirtyRect> {
        let (x, y, p) = self.last?;
        let mut dirty = None;
        self.dab(image, x, y, p, &mut dirty);
        self.finish(image, dirty)
    }

    fn finish(&mut self, image: &mut RgbaImage, dirty: Option<DirtyRect>) -> Option<DirtyRect> {
        if let Some(r) = dirty
            && self.mode.masked()
        {
            self.composite(image, r);
        }
        dirty
    }

    /// Rectangle of a dab of `diameter` at (cx, cy), within the image and the
    /// clip, and whether the clip has a shape (per-pixel coverage).
    fn footprint(&self, cx: f32, cy: f32, diameter: f32) -> Option<(DirtyRect, bool)> {
        let s = &self.settings;
        let radius = (diameter * 0.5).max(0.5);
        let (ru, rv) = (radius, radius * self.minor_scale());
        let (sin, cos) = s.angle.to_radians().sin_cos();
        let hx = ((ru * cos).powi(2) + (rv * sin).powi(2)).sqrt() + 1.0;
        let hy = ((ru * sin).powi(2) + (rv * cos).powi(2)).sqrt() + 1.0;
        let (w, h) = self.base.dimensions();
        let x0 = (cx - hx).floor().max(0.0) as u32;
        let y0 = (cy - hy).floor().max(0.0) as u32;
        let x1 = ((cx + hx).ceil().max(0.0) as u32).min(w);
        let y1 = ((cy + hy).ceil().max(0.0) as u32).min(h);
        self.within_clip(x0, y0, x1, y1)
    }

    fn within_clip(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> Option<(DirtyRect, bool)> {
        let (w, h) = self.base.dimensions();
        let ([cx0, cy0, cx1, cy1], shaped) = match &self.clip {
            Some(c) => (c.rect, c.shape.is_some() || c.inverted),
            None => ([0, 0, w, h], false),
        };
        let (x0, y0, x1, y1) = (x0.max(cx0), y0.max(cy0), x1.min(cx1), y1.min(cy1));
        (x0 < x1 && y0 < y1).then(|| {
            let r = DirtyRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            };
            (r, shaped)
        })
    }

    /// Short axis of the dab over its long axis.
    fn minor_scale(&self) -> f32 {
        let s = &self.settings;
        let aspect = match &s.tip {
            Tip::Round => 1.0,
            Tip::Sampled(t) => t.aspect(),
        };
        s.roundness.clamp(0.01, 1.0) * aspect
    }

    /// Fills `scratch` with the tip's coverage (0..1) over `r`, for a dab
    /// of `diameter` centered on (cx, cy).
    fn tip_coverage(&mut self, r: DirtyRect, cx: f32, cy: f32, diameter: f32) {
        let s = &self.settings;
        let radius = (diameter * 0.5).max(0.5);
        let minor = self.minor_scale();
        let (sin, cos) = s.angle.to_radians().sin_cos();
        // Falloff from `inner` to `outer`: at full hardness, the one-pixel
        // anti-aliasing of the edge.
        let outer = radius + 0.5;
        let inner = (radius - 0.5).max(0.0) * s.hardness.clamp(0.0, 1.0);
        // The pixel under the center is always covered (a 1 px brush paints).
        let center = (cx.floor(), cy.floor());
        let tip = &s.tip;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize((r.w * r.h) as usize, 0.0);
        dab_rows(&mut scratch, r.w as usize, |first, band| {
            for (j, row) in band.chunks_exact_mut(r.w as usize).enumerate() {
                let py = r.y + (first + j) as u32;
                let dy = py as f32 + 0.5 - cy;
                for (px, out) in (r.x..r.x + r.w).zip(row) {
                    let dx = px as f32 + 0.5 - cx;
                    // In the tip's frame: u along its long axis.
                    let u = dx * cos - dy * sin;
                    let v = dx * sin + dy * cos;
                    *out = match tip {
                        Tip::Round => {
                            let d = (u * u + (v / minor).powi(2)).sqrt();
                            if d <= inner || (px as f32, py as f32) == center {
                                1.0
                            } else {
                                let t = ((d - inner) / (outer - inner)).clamp(0.0, 1.0);
                                1.0 - t * t * (3.0 - 2.0 * t)
                            }
                        }
                        Tip::Sampled(tip) => tip.sample(u / radius, v / (radius * minor), diameter),
                    };
                }
            }
        });
        self.scratch = scratch;
    }

    fn dab(
        &mut self,
        image: &mut RgbaImage,
        cx: f32,
        cy: f32,
        p: f32,
        dirty: &mut Option<DirtyRect>,
    ) {
        let diameter = self.settings.diameter(p);
        let changed = match self.mode {
            StrokeMode::Smudge { finger } => self.smudge_dab(image, cx, cy, diameter, p, finger),
            _ => {
                let Some((r, shaped)) = self.footprint(cx, cy, diameter) else {
                    return;
                };
                self.tip_coverage(r, cx, cy, diameter);
                match self.mode {
                    StrokeMode::Blur => self.filter_dab(image, r, shaped, p, false),
                    StrokeMode::Sharpen => self.filter_dab(image, r, shaped, p, true),
                    _ => self.accumulate(r, shaped, p),
                }
                Some(r)
            }
        };
        self.dabs += 1;
        if let Some(r) = changed {
            *dirty = Some(dirty.map_or(r, |d| d.union(r)));
            self.touched = Some(self.touched.map_or(r, |d| d.union(r)));
        }
    }

    /// Masked modes: each pixel's coverage goes toward the dab's opacity, by
    /// the flow (times the tip).
    fn accumulate(&mut self, r: DirtyRect, shaped: bool, p: f32) {
        let s = &self.settings;
        let cap = s.opacity.clamp(0.0, 1.0) * s.pressure_scale(p) * 65535.0;
        let flow = s.flow.clamp(0.0, 1.0);
        let w = self.base.width() as usize;
        let (scratch, clip) = (&self.scratch, &self.clip);
        let painted = std::sync::atomic::AtomicBool::new(false);
        let rows = &mut self.mask[r.y as usize * w..(r.y + r.h) as usize * w];
        let rw = r.w as usize;
        dab_rows_work(rows, w, rw * r.h as usize, |first, band| {
            let mut any = false;
            for (j, row) in band.chunks_exact_mut(w).enumerate() {
                let py = r.y + (first + j) as u32;
                let tips = &scratch[(first + j) * rw..][..rw];
                for (px, &tip) in (r.x..r.x + r.w).zip(tips) {
                    let a = tip * flow;
                    if a <= 0.0 {
                        continue;
                    }
                    let target = cap * limit(clip, px, py, shaped);
                    let m = &mut row[px as usize];
                    let now = *m as f32;
                    if now < target {
                        let next = (now + (target - now) * a).round() as u16;
                        if next > *m {
                            *m = next;
                            any = true;
                        }
                    }
                }
            }
            if any {
                painted.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        self.painted |= painted.into_inner();
    }

    /// Recomposites area `r` from the original pixels and the mask.
    fn composite(&self, image: &mut RgbaImage, r: DirtyRect) {
        let w = self.base.width() as usize;
        let [cr, cg, cb, _] = self.settings.color;
        let ca = self.settings.color[3] as f32 / 255.0;
        let base = self.base.as_raw();
        let out: &mut [u8] = image.as_mut();
        let rows = &mut out[r.y as usize * w * 4..(r.y + r.h) as usize * w * 4];
        dab_rows_work(rows, w * 4, (r.w * r.h) as usize, |first, band| {
            for (j, line) in band.chunks_exact_mut(w * 4).enumerate() {
                let y = r.y as usize + first + j;
                let (from, to) = (y * w + r.x as usize, y * w + (r.x + r.w) as usize);
                let mask = &self.mask[from..to];
                let src = &base[from * 4..to * 4];
                let dst = &mut line[r.x as usize * 4..(r.x + r.w) as usize * 4];
                for (x, ((&m, b), d)) in (r.x as i64..).zip(
                    mask.iter()
                        .zip(src.chunks_exact(4))
                        .zip(dst.chunks_exact_mut(4)),
                ) {
                    let b = [b[0], b[1], b[2], b[3]];
                    let px = if m == 0 {
                        b
                    } else {
                        let k = m as f32 / 65535.0;
                        let rgb = || [b[0], b[1], b[2]].map(|v| v as f32 / 255.0);
                        let toned = |c: [f32; 3]| [to_u8(c[0]), to_u8(c[1]), to_u8(c[2]), b[3]];
                        match self.mode {
                            StrokeMode::Erase => match to_u8(b[3] as f32 / 255.0 * (1.0 - k)) {
                                0 => [0, 0, 0, 0],
                                a => [b[0], b[1], b[2], a],
                            },
                            StrokeMode::Tone {
                                burn,
                                range,
                                protect,
                            } => toned(tone::dodge_burn(rgb(), k, burn, range, protect)),
                            StrokeMode::Sponge { saturate, vibrance } => {
                                toned(tone::sponge(rgb(), k, saturate, vibrance))
                            }
                            StrokeMode::Clone => match self.cloned(x, y as i64) {
                                Some([sr, sg, sb, sa]) => {
                                    over(b, [sr, sg, sb, to_u8(sa as f32 / 255.0 * k)])
                                }
                                None => b,
                            },
                            _ => over(b, [cr, cg, cb, to_u8(ca * k)]),
                        }
                    };
                    d.copy_from_slice(&px);
                }
            }
        });
    }

    /// Clone Stamp: the source pixel for layer pixel (x, y), if any.
    fn cloned(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        let (source, (dx, dy)) = self.clone.as_ref()?;
        let (sx, sy) = (x + dx, y + dy);
        (sx >= 0 && sy >= 0 && sx < source.width() as i64 && sy < source.height() as i64)
            .then(|| source.get_pixel(sx as u32, sy as u32).0)
    }

    /// Blur or sharpen under the dab: each pixel moves toward (or away from)
    /// the mean of its 3 × 3 neighbors, by the strength times the tip.
    fn filter_dab(
        &mut self,
        image: &mut RgbaImage,
        r: DirtyRect,
        shaped: bool,
        p: f32,
        sharpen: bool,
    ) {
        let (w, h) = image.dimensions();
        // The area and a one-pixel border, read before writing (as they
        // are: 4 bytes a pixel, a large dab covers megapixels).
        let (x0, y0) = (r.x.saturating_sub(1), r.y.saturating_sub(1));
        let (x1, y1) = ((r.x + r.w + 1).min(w), (r.y + r.h + 1).min(h));
        let rw = (x1 - x0) as usize;
        let read: &RgbaImage = self.source.as_ref().unwrap_or(&*image);
        let mut region = vec![[0u8; 4]; rw * (y1 - y0) as usize];
        dab_rows(&mut region, rw, |first, band| {
            for (j, row) in band.chunks_exact_mut(rw).enumerate() {
                let y = y0 + (first + j) as u32;
                let from = ((y * w + x0) * 4) as usize;
                let line = &read.as_raw()[from..from + rw * 4];
                for (out, px) in row.iter_mut().zip(line.chunks_exact(4)) {
                    out.copy_from_slice(px);
                }
            }
        });
        let at = |x: u32, y: u32| premultiply(region[(y - y0) as usize * rw + (x - x0) as usize]);
        let filtered = |px: u32, py: u32, amount: f32| {
            let mut mean = [0.0f32; 4];
            let mut n = 0.0;
            for ny in py.saturating_sub(1).max(y0)..(py + 2).min(y1) {
                for nx in px.saturating_sub(1).max(x0)..(px + 2).min(x1) {
                    let q = at(nx, ny);
                    for c in 0..4 {
                        mean[c] += q[c];
                    }
                    n += 1.0;
                }
            }
            let cur = at(px, py);
            let mut out = [0.0f32; 4];
            for c in 0..4 {
                let m = mean[c] / n;
                out[c] = if sharpen {
                    cur[c] + (cur[c] - m) * amount
                } else {
                    cur[c] + (m - cur[c]) * amount
                };
            }
            out[3] = out[3].clamp(0.0, 1.0);
            for c in 0..3 {
                out[c] = out[c].clamp(0.0, out[3]);
            }
            out
        };
        let strength = self.settings.flow.clamp(0.0, 1.0) * self.settings.pressure_scale(p);
        let rwd = r.w as usize;
        let (scratch, clip) = (&self.scratch, &self.clip);
        let width = w as usize;
        let painted = std::sync::atomic::AtomicBool::new(false);
        let raw: &mut [u8] = image.as_mut();
        let span = r.y as usize * width * 4..(r.y + r.h) as usize * width * 4;
        if let Some(source) = &mut self.source {
            // Sampling all layers: the result into the sampled image, and
            // as paint over the layer, row by row on all the cores (each
            // pixel only meets its own two).
            let src: &mut [u8] = source.as_mut();
            let (rows, srows) = (&mut raw[span.clone()], &mut src[span]);
            dab_row_pairs(rows, srows, width * 4, rwd * r.h as usize, |j, line, sline| {
                let py = r.y + j as u32;
                let mut any = false;
                for (i, px) in (r.x..r.x + r.w).enumerate() {
                    let amount = scratch[j * rwd + i] * strength * limit(clip, px, py, shaped);
                    if amount > 0.0 {
                        let out = unpremultiply(filtered(px, py, amount));
                        let o = px as usize * 4;
                        sline[o..o + 4].copy_from_slice(&out);
                        let before: [u8; 4] = line[o..o + 4].try_into().expect("four");
                        let after = over(before, [out[0], out[1], out[2], to_u8(amount)]);
                        any |= after != before;
                        line[o..o + 4].copy_from_slice(&after);
                    }
                }
                any
            }, &painted);
            self.painted |= painted.into_inner();
            return;
        }
        // Into the layer, on all the cores (the reads are in `region`).
        let rows = &mut raw[span];
        dab_rows_work(rows, width * 4, rwd * r.h as usize, |first, band| {
            let mut any = false;
            for (j, line) in band.chunks_exact_mut(width * 4).enumerate() {
                let py = r.y + (first + j) as u32;
                let pixels = line[r.x as usize * 4..(r.x as usize + rwd) * 4].chunks_exact_mut(4);
                for ((i, px), p) in (r.x..r.x + r.w).enumerate().zip(pixels) {
                    let amount =
                        scratch[(first + j) * rwd + i] * strength * limit(clip, px, py, shaped);
                    if amount > 0.0 {
                        let out = unpremultiply(filtered(px, py, amount));
                        any |= p != out;
                        p.copy_from_slice(&out);
                    }
                }
            }
            if any {
                painted.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        });
        self.painted |= painted.into_inner();
    }

    /// Smudge: the paint carried by the finger is laid down under the dab
    /// (by the strength times the tip), and the finger picks up the result.
    fn smudge_dab(
        &mut self,
        image: &mut RgbaImage,
        cx: f32,
        cy: f32,
        diameter: f32,
        p: f32,
        finger: Option<[u8; 4]>,
    ) -> Option<DirtyRect> {
        let size = self.settings.size.ceil() as usize + 2;
        let half = (size / 2) as i64;
        let (ix, iy) = (cx.floor() as i64, cy.floor() as i64);
        let (w, h) = self.base.dimensions();
        let x0 = (ix - half).clamp(0, w as i64) as u32;
        let y0 = (iy - half).clamp(0, h as i64) as u32;
        let x1 = (ix - half + size as i64).clamp(0, w as i64) as u32;
        let y1 = (iy - half + size as i64).clamp(0, h as i64) as u32;
        let Some((r, shaped)) = self.within_clip(x0, y0, x1, y1) else {
            // The finger still moves: what it carries is kept.
            return None;
        };
        let first = self.carried.is_none();
        // Slots the first dab cannot pick up (outside the image or the
        // selection) are empty: they lay nothing down until filled.
        let carried = self.carried.get_or_insert_with(|| Carried {
            size,
            pixels: vec![finger.map_or(EMPTY_SLOT, |c| slot(premultiply(c))); size * size],
        });
        let cs = carried.size;
        let rw = r.w as usize;
        // The dab's rows of slots, and where it starts in them.
        let (i0, j0) = ((r.x as i64 - ix + half) as usize, (r.y as i64 - iy + half) as usize);
        let slots = &mut carried.pixels[j0 * cs..(j0 + r.h as usize) * cs];
        if first && finger.is_none() {
            // Picks up what is under the first dab, lays nothing down.
            let read: &RgbaImage = self.source.as_ref().unwrap_or(&*image);
            dab_rows_work(slots, cs, rw * r.h as usize, |first, band| {
                for (jj, line) in band.chunks_exact_mut(cs).enumerate() {
                    let y = r.y + (first + jj) as u32;
                    for (k, s) in line[i0..i0 + rw].iter_mut().enumerate() {
                        *s = slot(premultiply(read.get_pixel(r.x + k as u32, y).0));
                    }
                }
            });
            return None;
        }
        self.tip_coverage(r, cx, cy, diameter);
        let strength = self.settings.flow.clamp(0.0, 1.0) * self.settings.pressure_scale(p);
        // What a pixel becomes (`cur` under the slot `s`): the new slot,
        // and the amount laid down (0: none). Each pixel meets only its own
        // slot: computed in place, row by row.
        let (scratch, clip) = (&self.scratch, &self.clip);
        let smudged = |k: usize, x: u32, y: u32, cur: [f32; 4], s: &mut [u16; 4]| {
            let amount = scratch[k] * strength * limit(clip, x, y, shaped);
            let held = unslot(*s);
            if amount <= 0.0 || held.is_none() {
                *s = slot(cur);
                return None;
            }
            let held = held.expect("checked");
            let out: [f32; 4] = std::array::from_fn(|c| cur[c] + (held[c] - cur[c]) * amount);
            *s = slot(out);
            Some((out, amount))
        };
        let carried = self.carried.as_mut().expect("carried paint");
        let slots = &mut carried.pixels[j0 * cs..(j0 + r.h as usize) * cs];
        let width = image.width() as usize;
        let painted = std::sync::atomic::AtomicBool::new(false);
        let raw: &mut [u8] = image.as_mut();
        let span = r.y as usize * width * 4..(r.y + r.h) as usize * width * 4;
        let work = rw * r.h as usize;
        if let Some(source) = &mut self.source {
            // Sampling all layers: the result into the sampled image, and
            // as paint over the layer, row by row on all the cores (each
            // pixel only meets its own two and its slot).
            let src: &mut [u8] = source.as_mut();
            let (rows, srows) = (&mut raw[span.clone()], &mut src[span]);
            type Row<'a> = ((&'a mut [u8], &'a mut [u8]), &'a mut [[u16; 4]]);
            let row = |jj: usize, ((line, sline), slots): Row| {
                let y = r.y + jj as u32;
                let mut any = false;
                for (k, s) in slots[i0..i0 + rw].iter_mut().enumerate() {
                    let x = r.x + k as u32;
                    let o = x as usize * 4;
                    let cur = premultiply(sline[o..o + 4].try_into().expect("four"));
                    if let Some((out, amount)) = smudged(jj * rw + k, x, y, cur, s) {
                        let px = unpremultiply(out);
                        sline[o..o + 4].copy_from_slice(&px);
                        let before: [u8; 4] = line[o..o + 4].try_into().expect("four");
                        let after = over(before, [px[0], px[1], px[2], to_u8(amount)]);
                        any |= after != before;
                        line[o..o + 4].copy_from_slice(&after);
                    }
                }
                if any {
                    painted.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            };
            if work < SMUDGE_PARALLEL {
                let lines = rows.chunks_exact_mut(width * 4).zip(srows.chunks_exact_mut(width * 4));
                for (jj, item) in lines.zip(slots.chunks_exact_mut(cs)).enumerate() {
                    row(jj, item);
                }
            } else {
                use rayon::prelude::*;
                rows.par_chunks_exact_mut(width * 4)
                    .zip(srows.par_chunks_exact_mut(width * 4))
                    .zip(slots.par_chunks_exact_mut(cs))
                    .enumerate()
                    .with_min_len((DAB_BAND / rw.max(1)).max(1))
                    .for_each(|(jj, item)| row(jj, item));
            }
            self.painted |= painted.into_inner();
            return Some(r);
        }
        // Into the layer, on all the cores.
        let rows = &mut raw[span];
        let row = |jj: usize, line: &mut [u8], slots: &mut [[u16; 4]]| {
            let y = r.y + jj as u32;
            let pixels = line[r.x as usize * 4..(r.x as usize + rw) * 4].chunks_exact_mut(4);
            let mut any = false;
            for ((k, s), p) in slots[i0..i0 + rw].iter_mut().enumerate().zip(pixels) {
                let x = r.x + k as u32;
                let cur = premultiply([p[0], p[1], p[2], p[3]]);
                if let Some((out, _)) = smudged(jj * rw + k, x, y, cur, s) {
                    let px = unpremultiply(out);
                    any |= p != px;
                    p.copy_from_slice(&px);
                }
            }
            if any {
                painted.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        };
        if work < SMUDGE_PARALLEL {
            for (jj, (line, s)) in rows
                .chunks_exact_mut(width * 4)
                .zip(slots.chunks_exact_mut(cs))
                .enumerate()
            {
                row(jj, line, s);
            }
        } else {
            use rayon::prelude::*;
            rows.par_chunks_exact_mut(width * 4)
                .zip(slots.par_chunks_exact_mut(cs))
                .enumerate()
                // Bands of rows: one row each costs more than it saves.
                .with_min_len((DAB_BAND / rw.max(1)).max(1))
                .for_each(|(jj, (line, s))| row(jj, line, s));
        }
        self.painted |= painted.into_inner();
        Some(r)
    }
}

/// A slot of the paint the Smudge tool's finger carries: premultiplied,
/// 0..1 in 16 bits a channel (a large finger holds megapixels).
fn slot(p: [f32; 4]) -> [u16; 4] {
    p.map(|v| (v.clamp(0.0, 1.0) * SLOT_ONE).round() as u16)
}

/// The paint a slot holds (`None`: empty).
fn unslot(s: [u16; 4]) -> Option<[f32; 4]> {
    (s != EMPTY_SLOT).then(|| s.map(|v| f32::from(v) / SLOT_ONE))
}

/// Value of 1.0 in a slot, below the empty slot's.
const SLOT_ONE: f32 = 65534.0;
const EMPTY_SLOT: [u16; 4] = [u16::MAX; 4];

/// Selection coverage of pixel (x, y), 0..1.
fn limit(clip: &Option<Clip>, x: u32, y: u32, shaped: bool) -> f32 {
    match clip {
        Some(c) if shaped => c.coverage(x, y) as f32 / 255.0,
        _ => 1.0,
    }
}

/// Below this many pixels, a dab is worked on here, in one go (threads
/// would cost more than they save).
const DAB_PARALLEL: usize = 1 << 16;

/// `f(first_row, band)` over the rows (`row` long) of a dab's buffer, on
/// all the cores when large.
fn dab_rows<T: Send>(data: &mut [T], row: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let work = data.len();
    dab_rows_work(data, row, work, f);
}

/// Pixels of a dab worked on by one thread, at least.
const DAB_BAND: usize = 1 << 14;

/// [`DAB_PARALLEL`] for the Smudge tool, whose pixels cost less.
const SMUDGE_PARALLEL: usize = 1 << 17;

/// `f(row, line, source_line)` over the rows (`row` long) of the layer and
/// of the image it samples, the same size, on all the cores when large
/// (`work` pixels in all); `painted` set when one returns `true`.
fn dab_row_pairs(
    rows: &mut [u8],
    sources: &mut [u8],
    row: usize,
    work: usize,
    f: impl Fn(usize, &mut [u8], &mut [u8]) -> bool + Sync,
    painted: &std::sync::atomic::AtomicBool,
) {
    use rayon::prelude::*;
    let mark = |any: bool| {
        if any {
            painted.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    };
    if work < DAB_PARALLEL {
        let lines = rows.chunks_exact_mut(row).zip(sources.chunks_exact_mut(row));
        for (j, (line, sline)) in lines.enumerate() {
            mark(f(j, line, sline));
        }
        return;
    }
    let per = (DAB_BAND / (work / (rows.len() / row.max(1)).max(1)).max(1)).max(1);
    rows.par_chunks_exact_mut(row)
        .zip(sources.par_chunks_exact_mut(row))
        .enumerate()
        .with_min_len(per)
        .for_each(|(j, (line, sline))| mark(f(j, line, sline)));
}

/// [`dab_rows`] for rows wider than the dab: `work` pixels in all. Large
/// dabs go to a pool of threads kept from one dab to the next (starting
/// threads for each would cost more than a medium dab).
fn dab_rows_work<T: Send>(
    data: &mut [T],
    row: usize,
    work: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) {
    use rayon::prelude::*;
    let rows = data.len() / row.max(1);
    let bands = (work / DAB_BAND)
        .min(rayon::current_num_threads())
        .min(rows);
    if work < DAB_PARALLEL || bands <= 1 {
        return f(0, data);
    }
    let per = rows.div_ceil(bands);
    let f = &f;
    data.par_chunks_mut(per * row)
        .enumerate()
        .for_each(|(i, band)| f(i * per, band));
}

fn premultiply(p: [u8; 4]) -> [f32; 4] {
    let a = p[3] as f32 / 255.0;
    [
        p[0] as f32 / 255.0 * a,
        p[1] as f32 / 255.0 * a,
        p[2] as f32 / 255.0 * a,
        a,
    ]
}

fn unpremultiply(p: [f32; 4]) -> [u8; 4] {
    if p[3] <= 1.0 / 510.0 {
        return [0, 0, 0, 0];
    }
    [
        to_u8(p[0] / p[3]),
        to_u8(p[1] / p[3]),
        to_u8(p[2] / p[3]),
        to_u8(p[3]),
    ]
}

fn to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn settings(color: [u8; 4]) -> BrushSettings {
        BrushSettings::round(color, 6.0, 1.0, 1.0)
    }

    fn paint(img: &mut RgbaImage, mode: StrokeMode, set: BrushSettings) -> Stroke {
        Stroke::begin(Arc::new(img.clone()), mode, set)
    }

    #[test]
    fn paint_covers_center() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let mut s = paint(&mut img, StrokeMode::Paint, settings([255, 0, 0, 255]));
        let dirty = s.add_point(&mut img, 10.0, 10.0, 1.0).unwrap();
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(0, 0).0, WHITE);
        assert!(dirty.x <= 7 && dirty.x + dirty.w >= 13);
    }

    /// The Clone Stamp paints the source's pixels at the offset, up to the
    /// opacity, and nothing where the source has none.
    #[test]
    fn clone_paints_the_source_at_the_offset() {
        let mut img = RgbaImage::from_pixel(40, 20, image::Rgba(WHITE));
        let source = Arc::new(RgbaImage::from_fn(40, 20, |x, _| {
            image::Rgba(if x < 10 {
                [0, 0, 255, 255]
            } else {
                [255, 0, 0, 255]
            })
        }));
        let mut set = settings(WHITE);
        set.opacity = 0.5;
        let mut s = paint(&mut img, StrokeMode::Clone, set).with_clone(source, (-20, 0));
        s.add_point(&mut img, 25.0, 10.0, 1.0);
        // (25, 10) takes (5, 10): blue, half opaque over white.
        assert_eq!(img.get_pixel(25, 10).0, [127, 127, 255, 255]);
        // Again over the same spot: no more than the opacity.
        s.add_point(&mut img, 25.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(25, 10).0, [127, 127, 255, 255]);
        // Its source would be left of the image: untouched.
        s.add_point(&mut img, 3.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(3, 10).0, WHITE);
        assert!(s.coverage()[10 * 40 + 3] > 0);
    }

    #[test]
    fn erase_makes_transparent() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 255, 255]));
        let mut s = paint(&mut img, StrokeMode::Erase, settings([0, 0, 0, 255]));
        s.add_point(&mut img, 2.0, 10.0, 1.0);
        s.add_point(&mut img, 18.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(10, 10).0[3], 0);
        assert_eq!(img.get_pixel(10, 0).0[3], 255);
    }

    #[test]
    fn overlapping_stamps_do_not_accumulate_opacity() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        set.opacity = 0.5;
        let mut s = paint(&mut img, StrokeMode::Paint, set);
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        let once = img.get_pixel(10, 10).0;
        s.add_point(&mut img, 10.5, 10.0, 1.0);
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(10, 10).0, once);
    }

    #[test]
    fn clip_limits_the_stroke() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let mut s = paint(&mut img, StrokeMode::Paint, settings([0, 0, 0, 255]))
            .with_clip(Some([0, 0, 10, 20].into()));
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(9, 10).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(10, 10).0, WHITE);
    }

    #[test]
    fn clip_hole_is_left_untouched() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let clip = Clip {
            rect: [0, 0, 20, 20],
            shape: Some(crate::pixel_ops::ClipShape::Rect([10, 0, 20, 20])),
            inverted: true,
        };
        let mut s =
            paint(&mut img, StrokeMode::Paint, settings([0, 0, 0, 255])).with_clip(Some(clip));
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(9, 10).0, [0, 0, 0, 255]);
        assert_eq!(img.get_pixel(10, 10).0, WHITE);
    }

    #[test]
    fn hard_brush_edge_is_anti_aliased() {
        let mut img = RgbaImage::from_pixel(40, 40, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        set.size = 20.0;
        let mut s = paint(&mut img, StrokeMode::Paint, set);
        s.add_point(&mut img, 20.5, 20.5, 1.0);
        // Pixel centers at 9, 10 (the radius) and 11 px from the stamp's center.
        let row: Vec<u8> = (29..32).map(|x| img.get_pixel(x, 20).0[0]).collect();
        assert!(row[0] < 64, "{row:?}");
        assert!(
            row[1] > 64 && row[1] < 192,
            "intermediate edge pixel: {row:?}"
        );
        assert_eq!(row[2], 255, "{row:?}");
    }

    #[test]
    fn one_pixel_brush_always_paints() {
        let mut img = RgbaImage::from_pixel(5, 5, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        set.size = 1.0;
        let mut s = paint(&mut img, StrokeMode::Paint, set);
        // Exactly on a pixel corner.
        s.add_point(&mut img, 2.0, 2.0, 1.0);
        assert!(s.painted());
        assert_eq!(img.get_pixel(2, 2).0, [0, 0, 0, 255]);
    }

    #[test]
    fn erased_pixels_lose_their_color() {
        let mut img = RgbaImage::from_pixel(10, 10, image::Rgba([0, 0, 255, 255]));
        let mut s = paint(&mut img, StrokeMode::Erase, settings([0, 0, 0, 255]));
        s.add_point(&mut img, 5.0, 5.0, 1.0);
        assert_eq!(img.get_pixel(5, 5).0, [0, 0, 0, 0]);
    }

    #[test]
    fn stroke_outside_the_clip_paints_nothing() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let mut s = paint(&mut img, StrokeMode::Paint, settings([0, 0, 0, 255]))
            .with_clip(Some([0, 0, 0, 0].into()));
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        assert!(!s.painted());
    }

    #[test]
    fn stroke_outside_image_is_ignored() {
        let mut img = RgbaImage::new(10, 10);
        let mut s = paint(&mut img, StrokeMode::Paint, settings([255, 0, 0, 255]));
        assert!(s.add_point(&mut img, -50.0, -50.0, 1.0).is_none());
    }

    #[test]
    fn dabs_are_evenly_spaced_whatever_the_input_points() {
        // 20 px tip, 25 %: a dab every 5 px, 100 px → 1 + 20 dabs.
        let count = |points: &[f32]| {
            let mut img = RgbaImage::new(200, 40);
            let mut set = settings([0, 0, 0, 255]);
            set.size = 20.0;
            let mut s = paint(&mut img, StrokeMode::Paint, set);
            for &x in points {
                s.add_point(&mut img, x, 20.0, 1.0);
            }
            s.dabs()
        };
        assert_eq!(count(&[10.0, 110.0]), 21);
        let small: Vec<f32> = (0..=100).map(|i| 10.0 + i as f32).collect();
        assert_eq!(count(&small), 21, "the remainder carries over");
        let uneven = [10.0, 13.0, 13.5, 40.0, 41.0, 77.75, 110.0];
        assert_eq!(count(&uneven), 21);
    }

    #[test]
    fn spacing_follows_the_setting_and_the_pressure() {
        let count = |spacing: f32, pressure: f32| {
            let mut img = RgbaImage::new(200, 40);
            let mut set = settings([0, 0, 0, 255]);
            (set.size, set.spacing, set.pressure_size) = (20.0, spacing, true);
            let mut s = paint(&mut img, StrokeMode::Paint, set);
            s.add_point(&mut img, 10.0, 20.0, pressure);
            s.add_point(&mut img, 110.0, 20.0, pressure);
            s.dabs()
        };
        assert_eq!(count(1.0, 1.0), 1 + 5);
        assert_eq!(count(0.5, 1.0), 1 + 10);
        // Half the pressure: half the diameter, twice the dabs.
        assert_eq!(count(1.0, 0.5), 1 + 10);
    }

    #[test]
    fn flow_builds_up_to_the_opacity() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        (set.flow, set.opacity) = (0.5, 0.6);
        let mut s = paint(&mut img, StrokeMode::Paint, set);
        let grey = |img: &RgbaImage| 255 - img.get_pixel(10, 10).0[0] as i32;
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        // Half of the 60 %.
        assert!((grey(&img) - 77).abs() <= 1, "{}", grey(&img));
        s.repeat(&mut img);
        assert!((grey(&img) - 115).abs() <= 1, "{}", grey(&img));
        for _ in 0..30 {
            s.repeat(&mut img);
        }
        assert!(
            (grey(&img) - 153).abs() <= 1,
            "never beyond 60 %: {}",
            grey(&img)
        );
    }

    #[test]
    fn pressure_scales_the_opacity() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        set.pressure_opacity = true;
        let mut s = paint(&mut img, StrokeMode::Paint, set);
        s.add_point(&mut img, 10.0, 10.0, 0.25);
        assert!((img.get_pixel(10, 10).0[0] as i32 - 191).abs() <= 1);
    }

    #[test]
    fn roundness_and_angle_shape_the_dab() {
        let extent = |angle: f32| {
            let mut img = RgbaImage::from_pixel(40, 40, image::Rgba(WHITE));
            let mut set = settings([0, 0, 0, 255]);
            (set.size, set.roundness, set.angle) = (20.0, 0.25, angle);
            let mut s = paint(&mut img, StrokeMode::Paint, set);
            s.add_point(&mut img, 20.5, 20.5, 1.0);
            let w = (0..40).filter(|&x| img.get_pixel(x, 20).0[0] < 255).count();
            let h = (0..40).filter(|&y| img.get_pixel(20, y).0[0] < 255).count();
            (w, h)
        };
        let (w, h) = extent(0.0);
        assert!(w >= 19 && h <= 6, "{w} × {h}");
        assert_eq!(extent(90.0), (h, w));
    }

    #[test]
    fn sampled_tip_paints_within_its_diameter() {
        let mut img = RgbaImage::from_pixel(80, 80, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        set.tip = Preset::Spatter.settings().tip;
        set.size = 40.0;
        let mut s = paint(&mut img, StrokeMode::Paint, set);
        s.add_point(&mut img, 40.0, 40.0, 1.0);
        assert!(s.painted());
        for (x, y, p) in img.enumerate_pixels() {
            let d = ((x as f32 + 0.5 - 40.0).powi(2) + (y as f32 + 0.5 - 40.0).powi(2)).sqrt();
            if d > 21.5 {
                assert_eq!(p.0, WHITE, "{x}, {y}");
            }
        }
    }

    #[test]
    fn dodge_lightens_and_burn_darkens_within_the_exposure() {
        let tone = |burn: bool| {
            let mut img = RgbaImage::from_pixel(20, 20, image::Rgba([100, 100, 100, 255]));
            let mut set = settings([0, 0, 0, 255]);
            set.opacity = 0.5;
            let mode = StrokeMode::Tone {
                burn,
                range: ToneRange::Midtones,
                protect: true,
            };
            let mut s = paint(&mut img, mode, set);
            s.add_point(&mut img, 10.0, 10.0, 1.0);
            let once = img.get_pixel(10, 10).0[0];
            s.add_point(&mut img, 11.5, 10.0, 1.0);
            assert_eq!(
                img.get_pixel(10, 10).0[0],
                once,
                "no build-up in one stroke"
            );
            assert_eq!(img.get_pixel(10, 10).0[3], 255);
            once
        };
        assert!(tone(false) > 115);
        assert!(tone(true) < 85);
    }

    #[test]
    fn sponge_desaturates() {
        let mut img = RgbaImage::from_pixel(20, 20, image::Rgba([200, 60, 60, 255]));
        let mode = StrokeMode::Sponge {
            saturate: false,
            vibrance: false,
        };
        let mut s = paint(&mut img, mode, settings([0, 0, 0, 255]));
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        let [r, g, b, _] = img.get_pixel(10, 10).0;
        assert!(
            r.abs_diff(g) <= 1 && g.abs_diff(b) <= 1,
            "grey: {r} {g} {b}"
        );
    }

    /// Black left half, white right half: the contrast across the edge.
    fn edge_contrast(mode: StrokeMode, passes: usize) -> i32 {
        let mut img = RgbaImage::from_fn(40, 20, |x, _| {
            image::Rgba(if x < 20 { [0, 0, 0, 255] } else { WHITE })
        });
        let mut set = settings([0, 0, 0, 255]);
        (set.size, set.flow) = (10.0, 0.5);
        let mut s = paint(&mut img, mode, set);
        for i in 0..passes {
            let y = if i % 2 == 0 { 2.0 } else { 18.0 };
            s.add_point(&mut img, 20.0, y, 1.0);
        }
        img.get_pixel(20, 10).0[0] as i32 - img.get_pixel(19, 10).0[0] as i32
    }

    #[test]
    fn blur_softens_and_builds_up() {
        let once = edge_contrast(StrokeMode::Blur, 2);
        assert!(once < 255 && once > 0, "{once}");
        assert!(
            edge_contrast(StrokeMode::Blur, 6) < once,
            "more passes, more blur"
        );
    }

    /// Large dabs (worked on in bands, on all the cores) over an edge that
    /// varies only across: every row of the dab's core comes out the same.
    #[test]
    fn large_blur_and_smudge_dabs_are_seamless() {
        for mode in [StrokeMode::Blur, StrokeMode::Smudge { finger: None }] {
            let mut img = RgbaImage::from_fn(600, 600, |x, _| {
                image::Rgba(if x < 300 { [0, 0, 0, 255] } else { WHITE })
            });
            let mut set = settings([0, 0, 0, 255]);
            (set.size, set.flow) = (400.0, 0.8);
            let mut s = paint(&mut img, mode, set);
            for x in [150.0, 250.0, 350.0, 450.0] {
                s.add_point(&mut img, x, 300.0, 1.0);
            }
            let row = |y: u32| (280..420).map(|x| img.get_pixel(x, y).0).collect::<Vec<_>>();
            let middle = row(300);
            assert!(middle.iter().any(|p| p[0] != 0 && p[0] != 255), "{mode:?} changed the edge");
            for y in (200..400).step_by(7) {
                assert_eq!(row(y), middle, "{mode:?}: row {y}");
            }
        }
    }

    #[test]
    fn sharpen_increases_the_contrast() {
        let mut img = RgbaImage::from_fn(40, 20, |x, _| {
            image::Rgba(if x < 20 {
                [60, 60, 60, 255]
            } else {
                [180, 180, 180, 255]
            })
        });
        let mut set = settings([0, 0, 0, 255]);
        (set.size, set.flow) = (10.0, 1.0);
        let mut s = paint(&mut img, StrokeMode::Sharpen, set);
        s.add_point(&mut img, 20.0, 2.0, 1.0);
        s.add_point(&mut img, 20.0, 18.0, 1.0);
        let (dark, light) = (img.get_pixel(19, 10).0[0], img.get_pixel(20, 10).0[0]);
        assert!(dark < 60 && light > 180, "{dark} {light}");
    }

    #[test]
    fn smudge_drags_the_color_along() {
        let mut img = RgbaImage::from_fn(60, 20, |x, _| {
            image::Rgba(if x < 15 { [255, 0, 0, 255] } else { WHITE })
        });
        let mut set = settings([0, 0, 0, 255]);
        (set.size, set.flow) = (10.0, 0.8);
        let mut s = paint(&mut img, StrokeMode::Smudge { finger: None }, set);
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        s.add_point(&mut img, 30.0, 10.0, 1.0);
        let [r, g, _, _] = img.get_pixel(25, 10).0;
        assert!(r == 255 && g < 200, "red dragged to the right: {r} {g}");
        assert_eq!(img.get_pixel(50, 10).0, WHITE, "not beyond the stroke");
        assert_eq!(img.get_pixel(25, 1).0, WHITE, "not beside it");
    }

    #[test]
    fn smudge_from_the_image_edge_keeps_the_layer_opaque() {
        let mut img = RgbaImage::from_pixel(100, 40, image::Rgba([90, 90, 90, 255]));
        let mut set = settings([0, 0, 0, 255]);
        (set.size, set.flow) = (30.0, 0.8);
        let mut s = paint(&mut img, StrokeMode::Smudge { finger: None }, set);
        s.add_point(&mut img, 2.0, 20.0, 1.0);
        s.add_point(&mut img, 60.0, 20.0, 1.0);
        assert!(img.pixels().all(|p| p.0 == [90, 90, 90, 255]));
    }

    #[test]
    fn finger_painting_starts_with_the_color() {
        let mut img = RgbaImage::from_pixel(40, 20, image::Rgba(WHITE));
        let mut set = settings([0, 0, 0, 255]);
        (set.size, set.flow) = (10.0, 1.0);
        let finger = Some([0, 0, 255, 255]);
        let mut s = paint(&mut img, StrokeMode::Smudge { finger }, set);
        s.add_point(&mut img, 10.0, 10.0, 1.0);
        assert_eq!(img.get_pixel(10, 10).0, [0, 0, 255, 255]);
    }

    #[test]
    fn sampling_all_layers_paints_what_is_seen() {
        // An empty layer over a black and white image: the blur lays the
        // blurred colors on the layer.
        let mut layer = RgbaImage::new(40, 20);
        let visible = RgbaImage::from_fn(40, 20, |x, _| {
            image::Rgba(if x < 20 { [0, 0, 0, 255] } else { WHITE })
        });
        let mut set = settings([0, 0, 0, 255]);
        (set.size, set.flow) = (10.0, 1.0);
        let mut s = Stroke::begin(Arc::new(layer.clone()), StrokeMode::Blur, set)
            .with_source(Some(visible));
        s.add_point(&mut layer, 20.0, 10.0, 1.0);
        let p = layer.get_pixel(20, 10).0;
        assert!(p[3] > 0 && p[0] > 0 && p[0] < 255, "{p:?}");
    }

    /// Large dabs sampling all layers (worked on in bands of rows, on all
    /// the cores): the paint laid on the empty layer is as symmetric as
    /// the stroke, band edges included.
    #[test]
    fn large_dabs_sampling_all_layers_are_seamless() {
        for mode in [StrokeMode::Blur, StrokeMode::Smudge { finger: None }] {
            let mut layer = RgbaImage::new(600, 600);
            let visible = RgbaImage::from_fn(600, 600, |x, _| {
                image::Rgba(if x < 300 { [0, 0, 0, 255] } else { WHITE })
            });
            let mut set = settings([0, 0, 0, 255]);
            (set.size, set.flow) = (400.0, 0.8);
            let mut s = Stroke::begin(Arc::new(layer.clone()), mode, set)
                .with_source(Some(visible));
            for x in [150.0, 250.0, 350.0, 450.0] {
                s.add_point(&mut layer, x, 300.0, 1.0);
            }
            let row = |y: u32| (280..420).map(|x| layer.get_pixel(x, y).0).collect::<Vec<_>>();
            assert!(row(300).iter().any(|p| p[3] > 0), "{mode:?} painted");
            for k in 0..120 {
                assert_eq!(row(300 + k), row(299 - k), "{mode:?}: rows ±{k}");
            }
        }
    }
}
