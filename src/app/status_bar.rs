//! Status bar: document size, zoom, the active tool's keys (or the last
//! message while it is fresh), pointer position.

use super::keys;
use super::*;
use crate::i18n::t;

/// Seconds a message stays fully visible before fading out (in 1 s).
const MESSAGE_TIME: f64 = 5.0;
/// The same for an error, shown in the warning color.
const ERROR_TIME: f64 = 20.0;

impl AtelierApp {
    pub(super) fn status_bar(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        if self.status != self.status_shown {
            self.status_shown = self.status.clone();
            self.status_since = now;
        }
        let age = now - self.status_since;
        let error = self
            .workspace
            .error_status
            .as_ref()
            .is_some_and(|e| *e == self.status);
        let time = if error { ERROR_TIME } else { MESSAGE_TIME };
        let alpha = (1.0 - (age - time).max(0.0)).clamp(0.0, 1.0) as f32;
        // Repaints only when the message starts fading, then while it fades.
        if !self.status.is_empty() && alpha > 0.0 {
            let wait = (time - age).max(0.1);
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }

        let text = |ui: &mut egui::Ui, s: String, font: egui::FontId, color: Color32| {
            ui.label(egui::RichText::new(s).font(font).color(color));
        };
        let muted = ui_kit::color::MUTED;
        let message = (alpha > 0.0 && !self.status.is_empty()).then(|| {
            if error {
                (
                    format!("{}  {}", icon::WARNING, self.status),
                    workspace::WARNING.gamma_multiply(alpha),
                )
            } else {
                (
                    self.status.clone(),
                    ui_kit::color::ACCENT_TEXT.gamma_multiply(alpha),
                )
            }
        });
        let Some(doc) = &self.doc else {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                super::models_window::download_chips(ui, &mut self.downloads);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let welcome = t!(
                        "ui3-status-welcome",
                        new = keys::shortcut(ui.ctx(), &new_document::SHORTCUT_NEW),
                        open = keys::shortcut(ui.ctx(), &SHORTCUT_OPEN)
                    );
                    let (s, color) = message.unwrap_or((welcome, muted));
                    text(ui, s, egui::FontId::proportional(12.0), color);
                });
            });
            return;
        };
        let size = format!("{} × {} px", doc.width(), doc.height());
        let hints = self.key_hints(ui.ctx());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            super::models_window::download_chips(ui, &mut self.downloads);
            let pointer = self.hover_pixel.map_or(String::new(), |(x, y)| {
                format!("x {} · y {}", signed(x as f32), signed(y as f32))
            });
            text(ui, pointer, ui_kit::mono(12.0), ui_kit::color::LABEL);
            // Color under the pointer (all visible layers), like the Info panel.
            if let Some([r, g, b, _]) = self.hover_pixel.and_then(|(x, y)| doc.color_at(x, y)) {
                let rgb = t!(
                    "uip-status-rgb",
                    r = r.to_string(),
                    g = g.to_string(),
                    b = b.to_string()
                );
                text(ui, rgb, ui_kit::mono(12.0), ui_kit::color::LABEL);
                let (chip, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
                ui_kit::color_chip(ui.painter(), chip.center(), Color32::from_rgb(r, g, b));
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                text(ui, size, ui_kit::mono(12.0), muted);
                text(
                    ui,
                    super::canvas::zoom_label(self.zoom),
                    ui_kit::mono(12.0),
                    muted,
                );
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, 14.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, ui_kit::color::LINE);
                if let Some((s, color)) = message {
                    text(ui, s, egui::FontId::proportional(12.0), color);
                    return;
                }
                // As many hints as fit, whole.
                for (key, what) in hints {
                    let font = egui::FontId::proportional(12.0);
                    let painter = ui.painter();
                    let width = painter
                        .layout_no_wrap(key.clone(), ui_kit::mono(11.0), muted)
                        .size()
                        .x
                        + painter
                            .layout_no_wrap(what.clone(), font.clone(), muted)
                            .size()
                            .x
                        + 16.0;
                    if width > ui.available_width() {
                        break;
                    }
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui_kit::kbd(ui, &key);
                        text(ui, what, font, muted);
                    });
                }
            });
        });
    }

    /// Keys of the active tool and what they do, like the mockup's hints.
    fn key_hints(&self, ctx: &egui::Context) -> Vec<(String, String)> {
        let shift = keys::modifiers(ctx, Modifiers::SHIFT);
        let alt = keys::modifiers(ctx, Modifiers::ALT);
        let ctrl = keys::modifiers(ctx, Modifiers::COMMAND);
        let click = t!("hint-key-click");
        let shortcut = |s: &KeyboardShortcut| keys::shortcut(ctx, s);
        let enter = (keys::key(Key::Enter), t!("hint-apply"));
        let esc = (keys::key(Key::Escape), t!("hint-cancel"));
        let shift_click = keys::combo(ctx, Modifiers::SHIFT, &click);
        let alt_click = keys::combo(ctx, Modifiers::ALT, &click);
        let pair = |key: &str, what: String| (key.to_owned(), what);
        let handle = t!("xform-hint-handle");
        match self.tool {
            // The Move tool: Alt copies, Ctrl picks (or not) the layer
            // under the pointer; in Free Transform, from the center, no
            // snapping.
            // A drag is applied on release: Enter and Esc only for what the
            // options bar prepares.
            Tool::Transform if !self.free_transform => {
                let mut hints = vec![
                    (shift, t!("input3-hint-axis")),
                    (alt, t!("input3-hint-copy")),
                    (
                        ctrl,
                        if self.auto_select {
                            t!("input3-hint-selected-layers")
                        } else {
                            t!("input3-hint-pick-layer")
                        },
                    ),
                    pair("1…0", t!("hint-layer-opacity")),
                ];
                if self.transform_pending() {
                    hints.extend([enter, esc]);
                }
                hints
            }
            Tool::Transform => vec![
                (shift, t!("hint-move-shift")),
                (alt, t!("hint-from-center")),
                (ctrl, t!("hint-no-snap")),
                (
                    keys::combo(ctx, Modifiers::COMMAND, &handle),
                    t!("xform-hint-distort"),
                ),
                (
                    keys::combo(ctx, Modifiers::COMMAND | Modifiers::SHIFT, &handle),
                    t!("xform-hint-skew"),
                ),
                (
                    keys::combo(
                        ctx,
                        Modifiers::COMMAND | Modifiers::SHIFT | Modifiers::ALT,
                        &handle,
                    ),
                    t!("xform-hint-perspective"),
                ),
                pair("1…0", t!("hint-layer-opacity")),
                enter,
                esc,
            ],
            Tool::Select | Tool::Ellipse | Tool::MagicWand => vec![
                (shift, t!("hint-add")),
                (alt, t!("hint-subtract")),
                (shortcut(&SHORTCUT_DESELECT), t!("hint-deselect")),
                (shortcut(&SHORTCUT_INVERT_SELECTION), t!("hint-inverse")),
                (shortcut(&SHORTCUT_DUPLICATE), t!("hint-layer-via-copy")),
            ],
            Tool::ObjectSelect => vec![
                (click, t!("objsel-hint-object")),
                (t!("hint-key-drag"), t!("objsel-hint-rectangle")),
                (shift, t!("hint-add")),
                (alt, t!("hint-subtract")),
                (
                    keys::modifiers(ctx, Modifiers::SHIFT | Modifiers::ALT),
                    t!("objsel-hint-intersect"),
                ),
                (shortcut(&SHORTCUT_DESELECT), t!("hint-deselect")),
            ],
            Tool::FreeLasso => vec![
                (t!("hint-key-drag"), t!("sel-hint-draw")),
                (alt_click, t!("sel-hint-straight")),
                (shift, t!("hint-add")),
                (alt, t!("hint-subtract")),
                (shortcut(&SHORTCUT_DESELECT), t!("hint-deselect")),
            ],
            Tool::PolyLasso => vec![
                (click, t!("sel-hint-point")),
                (shift, t!("sel-hint-45")),
                (alt, t!("sel-hint-freehand")),
                (keys::key(Key::Backspace), t!("hint-remove-point")),
                (keys::key(Key::Enter), t!("hint-close-path")),
                esc,
            ],
            Tool::Lasso => vec![
                (click, t!("hint-anchor-point")),
                (keys::key(Key::Backspace), t!("hint-remove-point")),
                (keys::key(Key::Enter), t!("hint-close-path")),
                esc,
                (shift, t!("hint-add")),
                (alt, t!("hint-subtract")),
            ],
            Tool::Brush
            | Tool::Eraser
            | Tool::Dodge
            | Tool::Burn
            | Tool::Sponge
            | Tool::Blur
            | Tool::Sharpen
            | Tool::Smudge => {
                let amount = match self.tool {
                    Tool::Brush | Tool::Eraser => t!("hint-opacity"),
                    Tool::Dodge | Tool::Burn => t!("paint-hint-exposure"),
                    Tool::Sponge => t!("paint-hint-flow"),
                    _ => t!("paint-hint-strength"),
                };
                let mut hints = vec![
                    pair("[ ]", t!("hint-size")),
                    (
                        keys::combo(ctx, Modifiers::SHIFT, "[ ]"),
                        t!("hint-hardness"),
                    ),
                    pair("1…0", amount),
                ];
                if matches!(self.tool, Tool::Brush | Tool::Eraser) {
                    let digits = keys::combo(ctx, Modifiers::SHIFT, "1…0");
                    hints.push((digits, t!("paint-hint-flow")));
                }
                hints.push((shift_click, t!("hint-straight-line")));
                let alt_hint = match self.tool {
                    Tool::Brush => Some(t!("hint-pick-color")),
                    Tool::Dodge => Some(t!("paint-hint-burn")),
                    Tool::Burn => Some(t!("paint-hint-dodge")),
                    Tool::Blur => Some(t!("paint-hint-sharpen")),
                    Tool::Sharpen => Some(t!("paint-hint-blur")),
                    Tool::Smudge => Some(t!("paint-hint-finger-painting")),
                    _ => None,
                };
                if let Some(hint) = alt_hint {
                    let alt_key = if self.tool == Tool::Brush {
                        alt_click
                    } else {
                        alt
                    };
                    hints.push((alt_key, hint));
                }
                hints.push((keys::key(Key::Space), t!("hint-pan")));
                hints
            }
            Tool::CloneStamp
            | Tool::SpotHealing
            | Tool::HealingBrush
            | Tool::Patch
            | Tool::RedEye => self.retouch_hints(ctx),
            Tool::Gradient => vec![
                (t!("hint-key-drag"), t!("paint-hint-gradient")),
                (shift, t!("paint-hint-45")),
                pair("1…0", t!("hint-opacity")),
            ],
            Tool::Bucket => vec![(click, t!("hint-fill")), (alt_click, t!("hint-pick-color"))],
            Tool::Eyedropper => vec![
                (click, t!("hint-foreground")),
                (alt_click, t!("hint-background")),
            ],
            Tool::Text if self.typing_on_canvas() => vec![
                (
                    keys::combo(ctx, Modifiers::COMMAND, &keys::key(Key::Enter)),
                    t!("type-hint-commit"),
                ),
                (keys::key(Key::Escape), t!("type-hint-commit")),
                (shift_click, t!("type-hint-extend")),
                (
                    keys::combo(ctx, Modifiers::COMMAND, &t!("hint-key-drag")),
                    t!("type-hint-move"),
                ),
            ],
            Tool::Text => vec![
                (click, t!("type-hint-point")),
                (t!("hint-key-drag"), t!("type-hint-paragraph")),
                (shift_click, t!("type-hint-new")),
                (
                    keys::combo(ctx, Modifiers::COMMAND, &t!("hint-key-drag")),
                    t!("type-hint-move"),
                ),
            ],
            Tool::Artboard => vec![
                (t!("hint-key-drag"), t!("artboard-hint-draw")),
                (shift, t!("hint-keep-ratio")),
                (alt, t!("hint-from-center")),
                (alt_click, t!("artboard-hint-duplicate")),
            ],
            Tool::Canvas => vec![
                (ctrl, t!("hint-no-snap")),
                (shift, t!("hint-keep-ratio")),
                (alt, t!("hint-from-center")),
                enter,
                esc,
            ],
            Tool::Pen(_) | Tool::PathSelect | Tool::DirectSelect | Tool::Shape(_) => {
                self.vector_hints(ctx)
            }
            Tool::Hand | Tool::Zoom => {
                let mut hints = Vec::new();
                if self.tool == Tool::Zoom {
                    hints.push((click, t!("hint-zoom-in")));
                    hints.push((alt_click, t!("hint-zoom-out")));
                } else {
                    hints.push((keys::key(Key::Space), t!("hint-hand-any-tool")));
                    // The Zoom tool for a moment, from any tool.
                    let space_click = format!("{}+{click}", keys::key(Key::Space));
                    let zoom_in = keys::combo(ctx, Modifiers::COMMAND, &space_click);
                    let zoom_out = keys::combo(ctx, Modifiers::ALT, &space_click);
                    hints.push((zoom_in, t!("hint-zoom-in")));
                    hints.push((zoom_out, t!("hint-zoom-out")));
                }
                let alt_wheel = keys::combo(ctx, Modifiers::ALT, &t!("key-wheel"));
                hints.push((alt_wheel, t!("hint-zoom")));
                hints.push((shortcut(&SHORTCUT_FIT), t!("hint-fit")));
                hints.push((shortcut(&SHORTCUT_ACTUAL), t!("zoombar-actual-pixels")));
                hints
            }
        }
    }
}
