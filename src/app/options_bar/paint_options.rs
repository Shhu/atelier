//! Options of the painting tools: the brush preset picker (size, hardness,
//! angle, roundness, spacing, presets), each tool's amounts and the pen
//! pressure toggles; the Gradient tool's gradient, shape and blending.

use super::*;
use crate::app::gradient_editor::{gradient_preset_name, paint_ramp, paint_shape_icon};
use crate::app::paint_tools::PresetPreview;
use crate::paint::gradient::{self as grad, Shape};
use crate::paint::{Preset, Stroke, StrokeMode, Tip, ToneRange};

fn preset_name(preset: Preset) -> String {
    match preset {
        Preset::HardRound => t!("paint-preset-hard-round"),
        Preset::SoftRound => t!("paint-preset-soft-round"),
        Preset::Chalk => t!("paint-preset-chalk"),
        Preset::Spatter => t!("paint-preset-spatter"),
        Preset::DryBrush => t!("paint-preset-dry-brush"),
        Preset::Charcoal => t!("paint-preset-charcoal"),
    }
}

fn range_name(range: ToneRange) -> String {
    match range {
        ToneRange::Shadows => t!("paint-range-shadows"),
        ToneRange::Midtones => t!("paint-range-midtones"),
        ToneRange::Highlights => t!("paint-range-highlights"),
    }
}

/// White image (alpha = coverage) for a texture tinted when drawn.
fn coverage_image(w: usize, h: usize, alpha: impl Fn(usize, usize) -> u8) -> egui::ColorImage {
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            pixels.push(Color32::from_white_alpha(alpha(x, y)));
        }
    }
    egui::ColorImage::new([w, h], pixels)
}

/// The previews of every preset, made once.
fn previews<'a>(ctx: &egui::Context, cache: &'a mut Vec<PresetPreview>) -> &'a [PresetPreview] {
    if cache.is_empty() {
        for preset in Preset::ALL {
            let s = preset.settings();
            let (w, h) = (132u32, 34u32);
            let mut image = image::RgbaImage::new(w, h);
            let mut set =
                BrushSettings::round([255, 255, 255, 255], s.size.min(22.0), s.hardness, 1.0);
            (set.tip, set.spacing) = (s.tip.clone(), s.spacing);
            let mut stroke = Stroke::begin(Arc::new(image.clone()), StrokeMode::Paint, set);
            for i in 0..=48 {
                let t = i as f32 / 48.0;
                let x = 14.0 + t * (w as f32 - 28.0);
                let y = h as f32 / 2.0 - (t * std::f32::consts::TAU).sin() * 6.0;
                stroke.add_point(&mut image, x, y, 1.0);
            }
            let stroke_img = coverage_image(w as usize, h as usize, |x, y| {
                image.get_pixel(x as u32, y as u32).0[3]
            });
            let tip_img = match &s.tip {
                Tip::Sampled(t) => {
                    let g = t.image();
                    coverage_image(g.width() as usize, g.height() as usize, |x, y| {
                        g.get_pixel(x as u32, y as u32).0[0]
                    })
                }
                Tip::Round => coverage_image(32, 32, |x, y| {
                    let d = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
                    let inner = 15.0 * s.hardness;
                    let t = ((d - inner) / (16.0 - inner).max(0.5)).clamp(0.0, 1.0);
                    ((1.0 - t * t * (3.0 - 2.0 * t)) * 255.0) as u8
                }),
            };
            let options = egui::TextureOptions::LINEAR;
            cache.push(PresetPreview {
                stroke: ctx.load_texture(format!("brush-stroke-{preset:?}"), stroke_img, options),
                tip: ctx.load_texture(format!("brush-tip-{preset:?}"), tip_img, options),
            });
        }
    }
    cache
}

impl AtelierApp {
    /// Brush tools: the preset picker, then the active tool's amounts.
    pub(super) fn brush_options(&mut self, ui: &mut egui::Ui) {
        self.brush_picker(ui);
        sep(ui);
        let percent = |v: f32| i18n::percent(format!("{:.0}", v * 100.0));
        let pressure_size = |ui: &mut egui::Ui, on: &mut bool| {
            ui_kit::icon_toggle(ui, icon::PENCIL_CIRCLE, &t!("paint-pressure-size-tip"), on);
        };
        let airbrush = |ui: &mut egui::Ui, on: &mut bool| {
            ui_kit::icon_toggle(ui, icon::SPRAY_BOTTLE, &t!("paint-airbrush-tip"), on);
        };
        let p = &mut self.paint;
        match self.tool {
            Tool::Dodge | Tool::Burn => {
                let shown = range_name(p.range);
                ui_kit::combo(
                    ui,
                    "tone_range",
                    Some(&t!("paint-range")),
                    &shown,
                    170.0,
                    |ui| {
                        for r in ToneRange::ALL {
                            ui.selectable_value(&mut p.range, r, range_name(r));
                        }
                    },
                );
                let text = percent(p.exposure);
                ui_kit::scrub(
                    ui,
                    &t!("paint-exposure"),
                    &mut p.exposure,
                    0.01..=1.0,
                    &text,
                    150.0,
                );
                airbrush(ui, &mut p.airbrush);
                ui_kit::checkbox(ui, &mut p.protect_tones, &t!("paint-protect-tones"))
                    .on_hover_text(t!("paint-protect-tones-tip"));
                pressure_size(ui, &mut p.pressure_size);
            }
            Tool::Sponge => {
                let label = |saturate: bool| {
                    if saturate {
                        t!("paint-sponge-saturate")
                    } else {
                        t!("paint-sponge-desaturate")
                    }
                };
                let shown = label(p.saturate);
                ui_kit::combo(
                    ui,
                    "sponge_mode",
                    Some(&t!("paint-mode")),
                    &shown,
                    170.0,
                    |ui| {
                        for s in [false, true] {
                            ui.selectable_value(&mut p.saturate, s, label(s));
                        }
                    },
                );
                let text = percent(p.sponge_flow);
                ui_kit::scrub(
                    ui,
                    &t!("paint-flow"),
                    &mut p.sponge_flow,
                    0.01..=1.0,
                    &text,
                    150.0,
                );
                airbrush(ui, &mut p.airbrush);
                ui_kit::checkbox(ui, &mut p.vibrance, &t!("paint-vibrance"))
                    .on_hover_text(t!("paint-vibrance-tip"));
                pressure_size(ui, &mut p.pressure_size);
            }
            Tool::Blur | Tool::Sharpen | Tool::Smudge => {
                let strength = match self.tool {
                    Tool::Blur => &mut p.blur_strength,
                    Tool::Sharpen => &mut p.sharpen_strength,
                    _ => &mut p.smudge_strength,
                };
                let text = percent(*strength);
                ui_kit::scrub(
                    ui,
                    &t!("paint-strength"),
                    strength,
                    0.01..=1.0,
                    &text,
                    150.0,
                );
                ui_kit::checkbox(ui, &mut p.sample_all, &t!("options-sample-all-layers"))
                    .on_hover_text(t!("paint-sample-all-tip"));
                if self.tool == Tool::Smudge {
                    ui_kit::checkbox(ui, &mut p.finger_painting, &t!("paint-finger-painting"))
                        .on_hover_text(t!("paint-finger-painting-tip"));
                }
                pressure_size(ui, &mut p.pressure_size);
            }
            _ => {
                let text = percent(self.opacity);
                ui_kit::scrub(
                    ui,
                    &t!("options-opacity"),
                    &mut self.opacity,
                    0.01..=1.0,
                    &text,
                    150.0,
                );
                ui_kit::icon_toggle(
                    ui,
                    icon::PEN_NIB,
                    &t!("paint-pressure-opacity-tip"),
                    &mut p.pressure_opacity,
                );
                let text = percent(p.flow);
                ui_kit::scrub(ui, &t!("paint-flow"), &mut p.flow, 0.01..=1.0, &text, 150.0);
                airbrush(ui, &mut p.airbrush);
                let text = percent(p.smoothing);
                ui_kit::scrub(
                    ui,
                    &t!("paint-smoothing"),
                    &mut p.smoothing,
                    0.0..=1.0,
                    &text,
                    150.0,
                );
                pressure_size(ui, &mut p.pressure_size);
            }
        }
        let hint = match self.tool {
            Tool::Brush => t!("options-brush-hint"),
            Tool::Eraser => t!("optbar-eraser-hint"),
            Tool::Dodge => t!("paint-dodge-hint"),
            Tool::Burn => t!("paint-burn-hint"),
            Tool::Sponge => t!("paint-sponge-hint"),
            Tool::Blur => t!("paint-blur-hint"),
            Tool::Sharpen => t!("paint-sharpen-hint"),
            _ => t!("paint-smudge-hint"),
        };
        trailing_hint(ui, &hint);
    }

    /// Brush preset picker: the tip (size, hardness, angle, roundness,
    /// spacing) and the presets.
    pub(super) fn brush_picker(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let size = format!("{:.0} px", self.brush_size);
        let current = Preset::of(&self.paint.tip, self.hardness);
        let menu = dropdown(ui, "brush_size", None, &size, true, 22.0, |ui| {
            ui.set_width(260.0);
            let width = ui.available_width();
            let mut px = self.brush_size.round() as u32;
            if ui_kit::number_field(ui, &t!("options-size"), &mut px, 1..=5000, " px", width)
                .changed()
            {
                self.brush_size = px as f32;
            }
            ui.spacing_mut().slider_width = width;
            ui.add(
                egui::Slider::new(&mut self.brush_size, 1.0..=super::super::MAX_BRUSH_SIZE)
                    .logarithmic(true)
                    .show_value(false),
            );
            if matches!(self.paint.tip, Tip::Round) {
                let text = i18n::percent(format!("{:.0}", self.hardness * 100.0));
                ui_kit::scrub(
                    ui,
                    &t!("options-hardness"),
                    &mut self.hardness,
                    0.0..=1.0,
                    &text,
                    width,
                );
            }
            ui.horizontal(|ui| {
                let half = (width - 8.0) / 2.0;
                let mut angle = self.paint.angle.round() as i32;
                if ui_kit::number_field(ui, &t!("paint-angle"), &mut angle, -180..=180, "°", half)
                    .changed()
                {
                    self.paint.angle = angle as f32;
                }
                let mut round = (self.paint.roundness * 100.0).round() as u32;
                let pct = i18n::percent("");
                if ui_kit::number_field(ui, &t!("paint-roundness"), &mut round, 1..=100, &pct, half)
                    .changed()
                {
                    self.paint.roundness = round as f32 / 100.0;
                }
            });
            let mut spacing = (self.paint.spacing * 100.0).round() as u32;
            let pct = i18n::percent("");
            if ui_kit::number_field(
                ui,
                &t!("paint-spacing"),
                &mut spacing,
                1..=1000,
                &pct,
                width,
            )
            .on_hover_text(t!("paint-spacing-tip"))
            .changed()
            {
                self.paint.spacing = spacing as f32 / 100.0;
            }
            ui.add_space(4.0);
            let previews = previews(&ctx, &mut self.paint.previews);
            let mut chosen = None;
            // Three tiles a row: the preset's stroke, its name under it.
            let (grid, _) = ui.allocate_exact_size(vec2(width, 2.0 * 52.0 + 4.0), Sense::hover());
            let tile_w = (width - 2.0 * 4.0) / 3.0;
            for (i, preset) in Preset::ALL.into_iter().enumerate() {
                let at = grid.min + vec2((i % 3) as f32 * (tile_w + 4.0), (i / 3) as f32 * 56.0);
                let rect = Rect::from_min_size(at, vec2(tile_w, 52.0));
                let response = ui
                    .interact(rect, ui.id().with(("brush_preset", i)), Sense::click())
                    .on_hover_text(preset_name(preset));
                let painter = ui.painter();
                if current == Some(preset) {
                    painter.rect_filled(rect, 6.0, color::ACCENT_SOFT);
                } else if response.hovered() {
                    painter.rect_filled(rect, 6.0, color::BG3);
                }
                let thumb = Rect::from_center_size(
                    pos2(rect.center().x, rect.top() + 18.0),
                    vec2(tile_w - 8.0, (tile_w - 8.0) * 34.0 / 132.0),
                );
                let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                painter.image(previews[i].stroke.id(), thumb, uv, color::TEXT);
                let name = painter.layout(
                    preset_name(preset),
                    egui::FontId::proportional(11.0),
                    color::MUTED,
                    tile_w - 4.0,
                );
                let pos = pos2(rect.center().x - name.size().x / 2.0, rect.bottom() - 17.0);
                painter.galley(pos, name, color::MUTED);
                if response.clicked() {
                    chosen = Some(preset);
                }
            }
            if let Some(preset) = chosen {
                self.paint
                    .apply_preset(preset, &mut self.brush_size, &mut self.hardness);
            }
        });
        let response = menu.response.on_hover_text(t!("paint-brush-picker-tip"));
        // The tip, at its angle and roundness.
        let center = pos2(response.rect.left() + 17.0, response.rect.center().y);
        let painter = ui.painter();
        match current.filter(|_| !matches!(self.paint.tip, Tip::Round)) {
            Some(preset) => {
                let tip = &previews(&ctx, &mut self.paint.previews)
                    [Preset::ALL.iter().position(|&p| p == preset).unwrap_or(0)]
                .tip;
                let r = Rect::from_center_size(center, Vec2::splat(16.0));
                let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                painter.image(tip.id(), r, uv, color::TEXT);
            }
            None => {
                painter.circle_filled(center, 7.0, color::TEXT.gamma_multiply(0.25));
                painter.circle_filled(center, 4.0 + 2.5 * self.hardness, color::TEXT);
            }
        }
    }

    /// Gradient tool: the gradient (a click edits it, the arrow lists the
    /// presets), the shape, the mode, opacity, Reverse, Dither, Transparency.
    pub(super) fn gradient_options(&mut self, ui: &mut egui::Ui) {
        let [fr, fg, fb, _] = self.primary.to_srgba_unmultiplied();
        let [br, bg, bb, _] = self.secondary.to_srgba_unmultiplied();
        let (fg, bg) = ([fr, fg, fb], [br, bg, bb]);
        let g = &mut self.paint.gradient;
        let (rect, response) = ui.allocate_exact_size(vec2(112.0, 28.0), Sense::click());
        paint_ramp(
            ui.painter(),
            rect.shrink(3.0),
            &g.gradient,
            fg,
            bg,
            g.transparency,
        );
        let line = if response.hovered() {
            color::MUTED
        } else {
            color::LINE_HOVER
        };
        ui.painter().rect_stroke(
            rect.shrink(3.0),
            3.0,
            UiStroke::new(1.0, line),
            StrokeKind::Outside,
        );
        if response
            .on_hover_text(t!("paint-gradient-edit-tip"))
            .clicked()
        {
            self.paint.editor = Some(crate::app::gradient_editor::GradientEditor::new(
                g.gradient.clone(),
            ));
        }
        let g = &mut self.paint.gradient;
        let presets = dropdown(ui, "gradient_presets", None, "", false, 0.0, |ui| {
            ui.set_width(220.0);
            for preset in grad::Preset::ALL {
                let (r, response) = ui.allocate_exact_size(vec2(220.0, 30.0), Sense::click());
                let ramp = Rect::from_min_size(r.min + vec2(4.0, 5.0), vec2(60.0, 20.0));
                if response.hovered() {
                    ui.painter().rect_filled(r, 6.0, color::BG3);
                }
                paint_ramp(ui.painter(), ramp, &preset.gradient(), fg, bg, true);
                ui.painter().text(
                    pos2(ramp.right() + 8.0, r.center().y),
                    egui::Align2::LEFT_CENTER,
                    gradient_preset_name(preset),
                    egui::FontId::proportional(ui_kit::TEXT_SIZE),
                    color::TEXT,
                );
                if response.clicked() {
                    g.gradient = preset.gradient();
                    ui.close();
                }
            }
        });
        presets.response.on_hover_text(t!("paint-gradient-presets"));
        sep(ui);

        // Shapes, each drawn as a small gradient.
        let (row, _) = ui.allocate_exact_size(vec2(5.0 * 28.0 + 4.0 * 2.0, 28.0), Sense::hover());
        for (i, shape) in Shape::ALL.into_iter().enumerate() {
            let cell = Rect::from_min_size(row.min + vec2(i as f32 * 30.0, 0.0), Vec2::splat(28.0));
            let response = ui
                .interact(cell, ui.id().with(("gradient_shape", i)), Sense::click())
                .on_hover_text(shape.label());
            let selected = g.shape == shape;
            let painter = ui.painter();
            if selected {
                painter.rect_filled(cell, 6.0, color::ACCENT_SOFT);
            } else if response.hovered() {
                painter.rect_filled(cell, 6.0, color::BG3);
            }
            let icon = cell.shrink(6.0);
            let tint = if selected {
                color::ACCENT_TEXT
            } else {
                color::TEXT
            };
            paint_shape_icon(painter, icon, shape, tint);
            painter.rect_stroke(
                icon,
                0.0,
                UiStroke::new(1.0, color::LINE_HOVER),
                StrokeKind::Outside,
            );
            if response.clicked() {
                g.shape = shape;
            }
        }
        sep(ui);

        let mode = g.mode.label();
        ui_kit::combo(
            ui,
            "gradient_mode",
            Some(&t!("paint-mode")),
            &mode,
            150.0,
            |ui| {
                for (i, group) in BlendMode::GROUPS.iter().enumerate() {
                    if i > 0 {
                        ui.separator();
                    }
                    for &m in *group {
                        ui.selectable_value(&mut g.mode, m, m.label());
                    }
                }
            },
        );
        let text = i18n::percent(format!("{:.0}", g.opacity * 100.0));
        ui_kit::scrub(
            ui,
            &t!("options-opacity"),
            &mut g.opacity,
            0.01..=1.0,
            &text,
            150.0,
        );
        ui_kit::checkbox(ui, &mut g.reverse, &t!("paint-gradient-reverse"));
        ui_kit::checkbox(ui, &mut g.dither, &t!("paint-gradient-dither"))
            .on_hover_text(t!("paint-gradient-dither-tip"));
        ui_kit::checkbox(ui, &mut g.transparency, &t!("paint-gradient-transparency"))
            .on_hover_text(t!("paint-gradient-transparency-tip"));
        trailing_hint(ui, &t!("paint-gradient-hint"));
    }
}
