//! Painting tools: the brush engine's settings (tip, flow, smoothing,
//! airbrush, pen pressure) shared by Brush, Eraser, Dodge / Burn / Sponge
//! and Blur / Sharpen / Smudge; their strokes; and the Gradient tool.

use super::*;
use crate::paint::gradient::{self, Gradient, Shape};
use crate::paint::{Preset, StrokeMode, Tip, ToneRange};

/// Smoothing at 100 %: the brush trails the pointer by this many points.
const SMOOTHING_REACH: f32 = 40.0;
/// Airbrush: a dab every this many seconds while the button is held.
const AIRBRUSH_INTERVAL: f64 = 0.05;

/// Settings of the painting tools (shared tip, per-tool amounts) and the
/// gesture in progress.
pub(super) struct PaintTools {
    /// Brush tip; with the size, hardness (`AtelierApp`) and these, the
    /// shape of the dabs.
    pub tip: Tip,
    /// Distance between dabs, fraction of the diameter.
    pub spacing: f32,
    /// Degrees, counterclockwise.
    pub angle: f32,
    /// 0.01..1.
    pub roundness: f32,
    pub flow: f32,
    pub smoothing: f32,
    pub airbrush: bool,
    /// "Always use pressure" for size, for opacity.
    pub pressure_size: bool,
    pub pressure_opacity: bool,
    /// Dodge and Burn.
    pub range: ToneRange,
    pub exposure: f32,
    pub protect_tones: bool,
    /// Sponge.
    pub saturate: bool,
    pub sponge_flow: f32,
    pub vibrance: bool,
    /// Blur, Sharpen, Smudge.
    pub blur_strength: f32,
    pub sharpen_strength: f32,
    pub smudge_strength: f32,
    pub sample_all: bool,
    pub finger_painting: bool,
    pub gradient: GradientSettings,
    /// Gradient being drawn: start and end (canvas coordinates).
    pub gradient_drag: Option<(Pos2, Pos2)>,
    /// Gradient Editor open.
    pub editor: Option<super::gradient_editor::GradientEditor>,
    /// Pen pressure last reported during the stroke (`None`: no pen).
    pressure: Option<f32>,
    /// Where the brush is (canvas coordinates): with smoothing, it trails
    /// the pointer.
    brush_at: Option<Pos2>,
    /// Time of the last airbrush dab.
    last_dab: f64,
    /// Previews of the brush presets, made on first use.
    pub previews: Vec<PresetPreview>,
    /// A painting tool was used on this text or shape layer (its id): asks
    /// to rasterize it first.
    pub rasterize_confirm: Option<u64>,
    /// The command that asked it (a fill, a clear…): run once rasterized.
    pub rasterize_then: Option<super::Then>,
    /// Alt + right drag under way: where it started (screen), and the size
    /// and hardness then.
    size_drag: Option<(Pos2, f32, f32)>,
    /// That drag ended this frame: the right button's release opens no
    /// menu.
    pub(super) size_drag_ended: bool,
    pub bucket: BucketSettings,
}

/// Options of the Paint Bucket (besides its tolerance and Contiguous).
pub(super) struct BucketSettings {
    pub mode: BlendMode,
    pub opacity: f32,
    pub anti_alias: bool,
    /// Compares the colors seen, all layers together.
    pub all_layers: bool,
}

/// Previews of a brush preset: a stroke and its tip.
pub(super) struct PresetPreview {
    pub stroke: egui::TextureHandle,
    pub tip: egui::TextureHandle,
}

/// Options of the Gradient tool.
#[derive(Clone)]
pub(super) struct GradientSettings {
    pub gradient: Gradient,
    pub shape: Shape,
    pub mode: BlendMode,
    pub opacity: f32,
    pub reverse: bool,
    pub dither: bool,
    /// Uses the opacity stops.
    pub transparency: bool,
}

impl Default for PaintTools {
    fn default() -> Self {
        Self {
            tip: Tip::Round,
            spacing: 0.25,
            angle: 0.0,
            roundness: 1.0,
            flow: 1.0,
            smoothing: 0.1,
            airbrush: false,
            pressure_size: false,
            pressure_opacity: false,
            range: ToneRange::Midtones,
            exposure: 0.5,
            protect_tones: true,
            saturate: false,
            sponge_flow: 0.5,
            vibrance: true,
            blur_strength: 0.5,
            sharpen_strength: 0.5,
            smudge_strength: 0.5,
            sample_all: false,
            finger_painting: false,
            gradient: GradientSettings {
                gradient: gradient::Preset::ForegroundToBackground.gradient(),
                shape: Shape::Linear,
                mode: BlendMode::Normal,
                opacity: 1.0,
                reverse: false,
                dither: true,
                transparency: true,
            },
            gradient_drag: None,
            editor: None,
            pressure: None,
            brush_at: None,
            last_dab: 0.0,
            previews: Vec::new(),
            rasterize_confirm: None,
            rasterize_then: None,
            size_drag: None,
            size_drag_ended: false,
            bucket: BucketSettings {
                mode: BlendMode::Normal,
                opacity: 1.0,
                anti_alias: true,
                all_layers: false,
            },
        }
    }
}

impl PaintTools {
    /// Takes a brush preset's tip, size, hardness and spacing.
    pub fn apply_preset(&mut self, preset: Preset, size: &mut f32, hardness: &mut f32) {
        let s = preset.settings();
        self.tip = s.tip;
        self.spacing = s.spacing;
        (self.angle, self.roundness) = (0.0, 1.0);
        *size = s.size;
        *hardness = s.hardness;
    }
}

impl Tool {
    /// Tools with the Airbrush option.
    pub(super) fn has_airbrush(self) -> bool {
        matches!(
            self,
            Tool::Brush | Tool::Eraser | Tool::Dodge | Tool::Burn | Tool::Sponge
        )
    }
}

/// Pointer positions of this frame while the button is down, in order (the
/// system may report several per frame), each with the pen pressure known
/// at that point. Ends at `pointer` (or at the release).
pub(super) fn frame_samples(
    ctx: &egui::Context,
    pointer: Option<Pos2>,
) -> Vec<(Pos2, Option<f32>)> {
    let mut force = None;
    let mut samples = Vec::new();
    let mut ended = false;
    ctx.input(|i| {
        for e in &i.events {
            match e {
                // A pen (Windows Ink) reports its pressure with each position.
                egui::Event::Touch { force: Some(f), .. } => force = Some(*f),
                egui::Event::PointerMoved(p) => samples.push((*p, force)),
                egui::Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    ..
                } => {
                    if *pressed {
                        // Moves before the press are not part of the stroke.
                        samples.clear();
                    }
                    samples.push((*pos, force));
                    if !*pressed {
                        ended = true;
                        break;
                    }
                }
                _ => {}
            }
        }
    });
    if let Some(p) = pointer
        && !ended
        && samples.last().map(|s| s.0) != Some(p)
    {
        samples.push((p, force));
    }
    samples
}

/// A stroke changed area `r` of the active layer (at `offset`): a targeted
/// mask follows it.
fn sync_paint(doc: &mut Document, offset: (i64, i64), r: DirtyRect) {
    doc.sync_paint(Some(PixelRect {
        x: offset.0 + r.x as i64,
        y: offset.1 + r.y as i64,
        w: r.w,
        h: r.h,
    }));
}

/// Gradient pixel `s` over layer pixel `d` in `mode` (Dissolve: at canvas
/// pixel (x, y)).
fn blend_pixel(d: [u8; 4], s: [u8; 4], mode: BlendMode, x: i64, y: i64) -> [u8; 4] {
    use crate::compose::blend;
    let da = d[3] as f32 / 255.0;
    let c = |i: usize| d[i] as f32 / 255.0 * da;
    let mut px = [c(0), c(1), c(2), da];
    let mut a = s[3] as f32 / 255.0;
    let mode = if mode == BlendMode::Dissolve {
        a = blend::dissolve(a, x, y);
        BlendMode::Normal
    } else {
        mode
    };
    blend::over(&mut px, [0, 1, 2].map(|i| s[i] as f32 / 255.0), a, mode);
    if px[3] <= 0.0 {
        return [0, 0, 0, 0];
    }
    let c = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        c(px[0] / px[3]),
        c(px[1] / px[3]),
        c(px[2] / px[3]),
        c(px[3]),
    ]
}

/// `to`, moved so that the line from `from` is a multiple of 45°.
fn constrain_45(from: Pos2, to: Pos2) -> Pos2 {
    let d = to - from;
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (d.y.atan2(d.x) / step).round() * step;
    // Along the chosen direction, as far as the pointer projects onto it.
    let dir = vec2(angle.cos(), angle.sin());
    from + dir * d.dot(dir).max(0.0)
}

impl AtelierApp {
    /// What the active tool's stroke does, and with which settings. `alt`
    /// switches Dodge and Burn, Blur and Sharpen, and Finger Painting.
    fn stroke_setup(&self, alt: bool) -> (StrokeMode, BrushSettings) {
        let p = &self.paint;
        let mut s = BrushSettings::round(
            self.primary.to_srgba_unmultiplied(),
            self.brush_size,
            self.hardness,
            1.0,
        );
        (s.tip, s.spacing, s.angle, s.roundness) = (p.tip.clone(), p.spacing, p.angle, p.roundness);
        s.pressure_size = p.pressure_size;
        let mode = match self.tool {
            Tool::Dodge | Tool::Burn => {
                // The airbrush makes the exposure build up.
                if p.airbrush {
                    s.flow = p.exposure;
                } else {
                    s.opacity = p.exposure;
                }
                StrokeMode::Tone {
                    burn: (self.tool == Tool::Burn) != alt,
                    range: p.range,
                    protect: p.protect_tones,
                }
            }
            Tool::Sponge => {
                s.flow = p.sponge_flow;
                StrokeMode::Sponge {
                    saturate: p.saturate,
                    vibrance: p.vibrance,
                }
            }
            Tool::Blur | Tool::Sharpen => {
                if (self.tool == Tool::Sharpen) != alt {
                    s.flow = p.sharpen_strength;
                    StrokeMode::Sharpen
                } else {
                    s.flow = p.blur_strength;
                    StrokeMode::Blur
                }
            }
            Tool::Smudge => {
                s.flow = p.smudge_strength;
                let finger = p.finger_painting != alt;
                StrokeMode::Smudge {
                    finger: finger.then(|| self.primary.to_srgba_unmultiplied()),
                }
            }
            Tool::CloneStamp | Tool::HealingBrush | Tool::SpotHealing => {
                self.retouch_stroke_mode(&mut s)
            }
            _ => {
                (s.opacity, s.flow) = (self.opacity, p.flow);
                s.pressure_opacity = p.pressure_opacity;
                if self.tool == Tool::Eraser {
                    StrokeMode::Erase
                } else {
                    StrokeMode::Paint
                }
            }
        };
        (mode, s)
    }

    /// Brush tools: a stroke from the press to the release, through every
    /// position reported (with the pen's pressure), trailing the pointer by
    /// the smoothing and catching up on release.
    pub(super) fn brush_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        released: bool,
        shift: bool,
    ) {
        if start && (self.paint_refuses() || self.retouch_refuses()) {
            return;
        }
        // Shift + click: straight line from the end of the previous stroke.
        let line_from = self.last_paint.filter(|_| start && shift);
        let samples = frame_samples(&self.ctx, pointer);
        let (now, alt) = self.ctx.input(|i| (i.time, i.modifiers.alt));
        if start && let Some(doc) = &self.doc {
            let on_mask = doc.mask_targeted();
            let (mut mode, mut settings) = self.stroke_setup(alt);
            // On a mask, like Photoshop, the eraser paints the background color.
            if on_mask && mode == StrokeMode::Erase {
                mode = StrokeMode::Paint;
                settings.color = self.secondary.to_srgba_unmultiplied();
            }
            let sample_all = self.paint.sample_all
                && !on_mask
                && matches!(
                    mode,
                    StrokeMode::Blur | StrokeMode::Sharpen | StrokeMode::Smudge { .. }
                );
            let step = self.tool.name();
            let Some(doc) = &mut self.doc else { return };
            doc.checkpoint(step);
            // The layer grows to the canvas, or its targeted mask stands in for it.
            if !doc.begin_paint() {
                doc.discard_step();
                self.status = self.nothing_to_paint();
                return;
            }
            doc.hold_effects(true);
            let canvas = doc.full_rect();
            let clip = self.selection.as_ref().map(|sel| {
                sel.clip(doc.active_layer(), canvas)
                    .unwrap_or([0; 4].into())
            });
            let visible =
                sample_all.then(|| (*doc.composite_shared(doc.active_layer().rect())).clone());
            let stroke = Stroke::begin(doc.active_layer().shared_image(), mode, settings)
                .with_clip(clip)
                .with_source(visible);
            let at = line_from.or(pointer.map(|p| view.to_image(p)));
            let stroke =
                self.retouch
                    .prepare(self.tool, doc, &self.parked, stroke, at.unwrap_or_default());
            self.stroke = Some(stroke);
            self.paint.pressure = None;
            self.paint.brush_at = None;
            self.paint.last_dab = now;
            if let Some(from) = line_from {
                self.paint.brush_at = Some(from);
                self.stroke_to(from);
            }
        }
        if self.stroke.is_none() || !(down || released) {
            if released || (!down && self.stroke.is_some()) {
                self.finish_stroke();
            }
            return;
        }
        // Lazy brush: it follows the pointer only beyond the smoothing's reach.
        let reach = self.paint.smoothing * SMOOTHING_REACH / view.zoom;
        let last = samples.last().map(|s| s.0).or(pointer);
        for (pos, force) in samples {
            if force.is_some() {
                self.paint.pressure = force;
            }
            let target = view.to_image(pos);
            let at = match self.paint.brush_at {
                Some(b) if b.distance(target) > reach => target + (b - target).normalized() * reach,
                Some(b) => b,
                None => target,
            };
            if self.paint.brush_at != Some(at) {
                self.paint.brush_at = Some(at);
                self.stroke_to(at);
            }
        }
        if released && let Some(p) = last {
            // Catch-up: the stroke ends where the pointer was released.
            let end = view.to_image(p);
            self.paint.brush_at = Some(end);
            self.stroke_to(end);
        }
        if self.tool.has_airbrush()
            && self.paint.airbrush
            && down
            && now - self.paint.last_dab >= AIRBRUSH_INTERVAL
        {
            self.paint.last_dab = now;
            if let (Some(stroke), Some(doc)) = (&mut self.stroke, &mut self.doc)
                && let Some(r) = doc.active_layer_mut().paint(|px| stroke.repeat(px))
            {
                self.stroke_dirty = Some(self.stroke_dirty.map_or(r, |d| d.union(r)));
                let offset = doc.active_layer().offset;
                sync_paint(doc, offset, r);
            }
        }
        self.heal_pending();
        self.last_paint = self.paint.brush_at;
        if released || !down {
            self.finish_stroke();
        }
    }

    /// Extends the stroke to `at` (canvas coordinates) at the current pressure.
    fn stroke_to(&mut self, at: Pos2) {
        let pressure = self.paint.pressure.unwrap_or(1.0);
        let (Some(stroke), Some(doc)) = (&mut self.stroke, &mut self.doc) else {
            return;
        };
        let layer = doc.active_layer_mut();
        let offset = layer.offset;
        let (lx, ly) = (at.x - offset.0 as f32, at.y - offset.1 as f32);
        if let Some(r) = layer.paint(|px| stroke.add_point(px, lx, ly, pressure)) {
            self.stroke_dirty = Some(self.stroke_dirty.map_or(r, |d| d.union(r)));
            self.retouch.dabbed(r);
            sync_paint(doc, offset, r);
        }
    }

    /// Alt + right drag with a brush tool, like Photoshop: left and right
    /// change its size, up and down its hardness. Whether one is under way.
    pub(super) fn brush_size_drag(
        &mut self,
        hovered: bool,
        pointer: Option<Pos2>,
        zoom: f32,
    ) -> bool {
        let (pressed, down, alt) = self.ctx.input(|i| {
            let p = &i.pointer;
            (p.secondary_pressed(), p.secondary_down(), i.modifiers.alt)
        });
        if pressed
            && alt
            && hovered
            && self.stroke.is_none()
            && self.tool.sized_brush()
            && let Some(p) = pointer
        {
            self.paint.size_drag = Some((p, self.brush_size, self.hardness));
        }
        let Some((from, size, hardness)) = self.paint.size_drag else {
            return false;
        };
        if !down {
            self.paint.size_drag = None;
            self.paint.size_drag_ended = true;
            return false;
        }
        if let Some(p) = pointer {
            let d = p - from;
            // The tip's edge follows the pointer.
            self.brush_size = (size + 2.0 * d.x / zoom).round().clamp(1.0, MAX_BRUSH_SIZE);
            if matches!(self.paint.tip, Tip::Round) {
                self.hardness = (hardness + d.y / 200.0).clamp(0.0, 1.0);
            }
            let size = t!(
                "status-brush-size",
                value = format!("{:.0}", self.brush_size)
            );
            let hardness = t!(
                "status-hardness",
                value = format!("{:.0}", self.hardness * 100.0)
            );
            self.status = format!("{size} — {hardness}");
        }
        true
    }

    /// Outline of the brush tip under the pointer (its angle and roundness);
    /// `false` when too small to show (a crosshair then).
    pub(super) fn draw_brush_cursor(&self, painter: &Painter, pos: Pos2, view: View) -> bool {
        // Resized with Alt + right drag: where the drag started.
        let pos = self.paint.size_drag.map_or(pos, |d| d.0);
        let radius = self.brush_size * 0.5 * view.zoom;
        if radius < 3.0 {
            return false;
        }
        let minor = match &self.paint.tip {
            Tip::Round => 1.0,
            Tip::Sampled(t) => t.aspect(),
        } * self.paint.roundness;
        let (sin, cos) = (-self.paint.angle.to_radians()).sin_cos();
        // Segments of about 4 screen pixels: round at any size.
        let n = ((radius * std::f32::consts::TAU / 4.0) as usize).clamp(48, 512);
        let outline = |r: f32| -> Vec<Pos2> {
            (0..n)
                .map(|i| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    let (u, v) = (a.cos() * r, a.sin() * r * minor);
                    pos + vec2(u * cos - v * sin, u * sin + v * cos)
                })
                .collect()
        };
        painter.add(egui::Shape::closed_line(
            outline(radius),
            UiStroke::new(1.5, Color32::BLACK),
        ));
        painter.add(egui::Shape::closed_line(
            outline(radius - 1.0),
            UiStroke::new(1.0, Color32::WHITE),
        ));
        true
    }

    /// Digits: the active painting tool's main amount (Shift: the flow).
    /// Returns `false` for the other tools.
    pub(super) fn tool_percent(&mut self, percent: u32, shift: bool) -> bool {
        let value = (percent as f32 / 100.0).max(0.01);
        let p = &mut self.paint;
        let (target, key) = match (self.tool, shift) {
            (Tool::Brush | Tool::Eraser, false) => (&mut self.opacity, "opacity"),
            (Tool::Brush | Tool::Eraser, true) => (&mut p.flow, "flow"),
            (Tool::Dodge | Tool::Burn, false) => (&mut p.exposure, "exposure"),
            (Tool::Sponge, false) => (&mut p.sponge_flow, "flow"),
            (Tool::Blur, false) => (&mut p.blur_strength, "strength"),
            (Tool::Sharpen, false) => (&mut p.sharpen_strength, "strength"),
            (Tool::Smudge, false) => (&mut p.smudge_strength, "strength"),
            (Tool::Gradient, false) => (&mut p.gradient.opacity, "opacity"),
            (Tool::CloneStamp, false) => (&mut self.retouch.clone_opacity, "opacity"),
            (Tool::CloneStamp, true) => (&mut self.retouch.clone_flow, "flow"),
            _ => return false,
        };
        *target = value;
        self.status = match key {
            "opacity" => t!("status-tool-opacity", percent = percent),
            "flow" => t!("paint-status-flow", percent = percent),
            "exposure" => t!("paint-status-exposure", percent = percent),
            _ => t!("paint-status-strength", percent = percent),
        };
        true
    }

    /// Before a painting tool changes the active layer: refused, with a
    /// message, when it is locked or hidden (with its group); a text or
    /// shape layer must first be rasterized, which is asked.
    pub(super) fn paint_refuses(&mut self) -> bool {
        // Quick Mask Mode: the tools paint the mask.
        if self.doc.as_ref().is_some_and(|d| d.quick_mask().is_some()) {
            return false;
        }
        if self.refuse_if_target_locked() {
            return true;
        }
        let Some(doc) = &self.doc else { return true };
        let layer = doc.active_layer();
        if !doc.shown(doc.active) {
            let message = match self.hidden_group() {
                Some(group) => tm!("ui5-status-group-hidden", name = layer.name.as_str(), group = group),
                None => tm!("tools3-status-layer-hidden", name = layer.name.as_str()),
            };
            self.status_failed(message);
            return true;
        }
        if !doc.mask_targeted() && (layer.text.is_some() || layer.shape.is_some()) {
            self.paint.rasterize_confirm = Some(layer.id);
            self.paint.rasterize_then = None;
            return true;
        }
        false
    }

    /// Before `action` changes the pixels of a text or shape layer (a
    /// fill, a clear): asks to rasterize it first, and runs it then.
    pub(super) fn rasterize_first(&mut self, action: Action) -> bool {
        // Clear and Cut do nothing without a selection.
        let needs_selection = matches!(action, Action::ClearSelection | Action::Cut);
        if needs_selection && self.selection.is_none() {
            return false;
        }
        self.rasterize_then(move |app| app.run(action))
    }

    /// Before `then` changes the pixels of a text or shape layer (Fill…,
    /// a color made transparent): asks to rasterize it first, and runs
    /// `then` once rasterized. Returns whether it asks.
    pub(super) fn rasterize_then(&mut self, then: impl FnOnce(&mut Self) + 'static) -> bool {
        let Some(doc) = &self.doc else { return false };
        let layer = doc.active_layer();
        let vector = layer.text.is_some() || layer.shape.is_some();
        if !vector || doc.quick_mask().is_some() || doc.mask_targeted() {
            return false;
        }
        self.paint.rasterize_confirm = Some(layer.id);
        self.paint.rasterize_then = Some(Box::new(then));
        true
    }

    /// Paint Bucket at `at` (canvas), with the foreground color.
    pub(super) fn bucket_fill(&mut self, at: Pos2) {
        if self.paint_refuses() {
            return;
        }
        if self
            .doc
            .as_ref()
            .is_some_and(|d| !d.mask_targeted() && !d.active_layer().paintable())
        {
            self.status = self.nothing_to_paint();
            return;
        }
        let color = self.primary.to_srgba_unmultiplied();
        let tolerance = (self.bucket_tolerance * 255.0).round() as u8;
        let contiguous = self.bucket_contiguous;
        let b = &self.paint.bucket;
        let mode = b.mode;
        let blend = move |d, s, x, y| blend_pixel(d, s, mode, x, y);
        let style = crate::pixel_ops::BucketStyle {
            sample: None,
            anti_alias: b.anti_alias,
            opacity: b.opacity,
            blend: &blend,
        };
        let Some(doc) = &mut self.doc else { return };
        if doc.fill_at_with(
            at.x.floor() as i64,
            at.y.floor() as i64,
            color,
            tolerance,
            self.selection.as_ref(),
            contiguous,
            &style,
            b.all_layers,
        ) {
            self.reset_pending();
        }
    }

    /// A painting tool is chosen and the active layer is locked or hidden:
    /// it would be refused (see `paint_refuses`).
    pub(super) fn paint_blocked(&self) -> bool {
        self.paint_block().is_some()
    }

    /// Asks to rasterize the text or shape layer a painting tool was used on.
    pub(super) fn rasterize_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.paint.rasterize_confirm else {
            return;
        };
        let Some(layer) = self
            .doc
            .as_ref()
            .map(Document::active_layer)
            .filter(|l| l.id == id)
        else {
            self.paint.rasterize_confirm = None;
            self.paint.rasterize_then = None;
            return;
        };
        let text = if layer.text.is_some() {
            t!("tools3-rasterize-text", name = layer.name.as_str())
        } else {
            t!("tools3-rasterize-shape", name = layer.name.as_str())
        };
        let center = ctx.content_rect().center();
        let out = super::dialogs::floating_dialog(
            ctx,
            super::dialogs::DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 400.0,
                ok: t!("tools3-rasterize-ok"),
                ..super::dialogs::DialogSpec::new(
                    "rasterize_confirm",
                    t!("vector-menu-rasterize"),
                    center,
                )
            },
            |ui| {
                ui.label(text);
            },
        );
        if out.ok || out.cancel {
            self.paint.rasterize_confirm = None;
        }
        let then = self.paint.rasterize_then.take_if(|_| out.ok || out.cancel);
        if out.ok {
            self.run(Action::Rasterize);
            if let Some(then) = then {
                then(self);
            }
        }
    }

    // ------------------------------------------------------------ gradient

    /// Gradient tool: drag from the start to the end (Shift: by 45°), the
    /// gradient is drawn on release.
    pub(super) fn gradient_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        released: bool,
        shift: bool,
    ) {
        if start
            && !self.paint_refuses()
            && let Some(p) = pointer
        {
            let at = view.to_image(p);
            self.paint.gradient_drag = Some((at, at));
        }
        if let (Some((from, _)), Some(p)) = (self.paint.gradient_drag, pointer)
            && (down || released)
        {
            let to = view.to_image(p);
            let to = if shift { constrain_45(from, to) } else { to };
            self.paint.gradient_drag = Some((from, to));
        }
        if !down && let Some((from, to)) = self.paint.gradient_drag.take() {
            // A click without a drag draws nothing.
            if released && from.distance(to) * view.zoom >= 2.0 {
                self.apply_gradient(from, to);
            }
        }
    }

    /// Draws the gradient from `from` to `to` (canvas coordinates) on the
    /// active layer, inside the selection: one undo step.
    pub(super) fn apply_gradient(&mut self, from: Pos2, to: Pos2) {
        let [fr, fg, fb, _] = self.primary.to_srgba_unmultiplied();
        let [br, bg, bb, _] = self.secondary.to_srgba_unmultiplied();
        let g = &self.paint.gradient;
        let lut = g.gradient.lut([fr, fg, fb], [br, bg, bb], g.transparency);
        let params = gradient::Params {
            shape: g.shape,
            reverse: g.reverse,
            dither: g.dither,
            opacity: g.opacity,
        };
        let mode = g.mode;
        let Some(doc) = &mut self.doc else { return };
        doc.checkpoint(t!("paint-tool-gradient"));
        if !doc.begin_paint() {
            doc.discard_step();
            self.status = self.nothing_to_paint();
            return;
        }
        let canvas = doc.full_rect();
        let selection = self.selection.clone().unwrap_or_else(|| canvas.into());
        let clip = selection.clip(doc.active_layer(), canvas);
        let layer = doc.active_layer_mut();
        let offset = layer.offset;
        let changed = clip.is_some_and(|clip| {
            gradient::render(
                layer.pixels_mut(),
                offset,
                &clip,
                &lut,
                &params,
                [from.x, from.y],
                [to.x, to.y],
                &|d, s, x, y| blend_pixel(d, s, mode, x, y),
            )
        });
        doc.end_paint();
        if !changed {
            doc.discard_step();
            return;
        }
        self.reset_pending();
    }

    /// The gradient line being drawn, like Photoshop: from the start to the
    /// pointer, with its two ends.
    pub(super) fn draw_gradient_line(&self, painter: &Painter, view: View) {
        let Some((from, to)) = self.paint.gradient_drag else {
            return;
        };
        let (a, b) = (view.to_screen(from), view.to_screen(to));
        painter.line_segment([a, b], UiStroke::new(3.0, Color32::from_black_alpha(140)));
        painter.line_segment([a, b], UiStroke::new(1.5, Color32::WHITE));
        for p in [a, b] {
            painter.circle(p, 4.0, Color32::WHITE, UiStroke::new(1.0, Color32::BLACK));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_constrains_to_45_degrees() {
        let from = pos2(10.0, 10.0);
        let c = constrain_45(from, pos2(30.0, 12.0));
        assert!((c.y - 10.0).abs() < 1e-4 && c.x > 29.0);
        let c = constrain_45(from, pos2(20.0, 21.0));
        assert!((c.x - c.y).abs() < 1e-4, "diagonal: {c:?}");
        let c = constrain_45(from, pos2(9.0, -30.0));
        assert!((c.x - 10.0).abs() < 1e-4 && c.y < -29.0);
    }
}
