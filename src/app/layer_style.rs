//! Layer commands (groups, clipping, masks, adjustment and fill layers,
//! links, blend mode shortcuts) and the Layer Style window: the styles on
//! the left, the settings of the chosen one on the right, like Photoshop's.

use super::dialogs::{DialogSpec, Track, floating_dialog, gap_between_blocks, slider_row};
use super::right_panel::{blend_combo, gradient_fill_fields};
use super::*;
use crate::compose::gradient::GradientFill;
use crate::compose::styles::{GlowFx, ShadowFx};
use crate::paint::gradient::Preset;
use crate::selection::SelectMode;
use ui_kit::color;

/// The Layer Style window: the page shown (a style, or Blending Options),
/// and what the layer had when it opened (Cancel puts it back).
#[derive(Clone)]
pub(super) struct StyleDialog {
    page: Option<Style>,
    layer: u64,
    before: (Effects, BlendMode, f32, f32),
}

impl AtelierApp {
    pub(super) fn layer_action(&mut self, action: Action) {
        let indices = self.selected_indices();
        let selection = self.selection.clone();
        let [fg, bg] = [self.primary, self.secondary].map(|c| {
            let [r, g, b, _] = c.to_srgba_unmultiplied();
            [r, g, b]
        });
        // From the foreground to the background color.
        let fg_to_bg = Preset::ForegroundToBackground.gradient().resolved(fg, bg);
        let Some(doc) = &mut self.doc else { return };
        let mask = doc
            .active_layer()
            .mask
            .as_ref()
            .map(|m| (m.enabled, m.linked()));
        let changed = match action {
            Action::NewGroup => {
                doc.add_group();
                true
            }
            Action::GroupLayers => doc.group_layers(&indices),
            Action::Ungroup => doc.ungroup(),
            Action::ToggleClipping => doc.toggle_clipping(),
            Action::NewAdjustmentLayer(kind) => {
                let adjustment = match kind.initial() {
                    Adjustment::GradientMap(mut g) => {
                        g.gradient = fg_to_bg;
                        Adjustment::GradientMap(g)
                    }
                    a => a,
                };
                doc.add_adjustment_layer(adjustment, selection.as_ref());
                true
            }
            Action::NewFillLayer(gradient) => {
                let fill = if gradient {
                    Fill::Gradient(GradientFill {
                        gradient: fg_to_bg,
                        ..Default::default()
                    })
                } else {
                    Fill::Solid(fg)
                };
                doc.add_fill_layer(fill, selection.as_ref());
                true
            }
            Action::AddMask(kind) => doc.add_mask(kind, selection.as_ref()),
            Action::DeleteMask => doc.delete_mask(),
            Action::ApplyMask => doc.apply_mask(),
            Action::ToggleMaskEnabled => mask.is_some_and(|(on, _)| doc.set_mask_enabled(!on)),
            Action::ToggleMaskLink => mask.is_some_and(|(_, linked)| doc.set_mask_linked(!linked)),
            Action::InvertMask => doc.invert_mask(),
            Action::MaskSelection => {
                let found = doc.mask_selection(doc.active);
                self.sel.step_name = Some(select_menu::step_label(t!("sel-menu-load")));
                self.apply_selection(found, SelectMode::Replace);
                return;
            }
            Action::LayerStyle(page) => {
                let layer = doc.active_layer();
                if layer.has_pixels() {
                    let before = (
                        layer.effects.clone(),
                        layer.blend,
                        layer.opacity,
                        layer.fill,
                    );
                    let layer = layer.id;
                    doc.checkpoint(t!("core-menu-layer-style"));
                    self.layer_style = Some(StyleDialog {
                        page,
                        layer,
                        before,
                    });
                }
                return;
            }
            Action::ClearLayerStyle => {
                let id = doc.active_layer().id;
                if !doc.active_layer().effects.any() {
                    return;
                }
                self.record_change(t!("xchg-hist-clear-style"), Some((id, "styles")));
                if let Some(doc) = &mut self.doc {
                    doc.active_layer_mut().effects = Effects::default();
                }
                return;
            }
            Action::LinkLayers => doc.toggle_link(&indices),
            Action::BlendStep(next) => {
                let layer = doc.active_layer();
                let modes = BlendMode::cycle(layer.is_group());
                let i = modes.iter().position(|&m| m == layer.blend).unwrap_or(0);
                let n = modes.len();
                let mode = modes[if next { (i + 1) % n } else { (i + n - 1) % n }];
                return self.set_blend(mode);
            }
            Action::SetBlend(mode) => return self.set_blend(mode),
            _ => return,
        };
        if changed {
            self.reset_pending();
        }
    }

    /// Blend mode of the active layer, as a series of edits (one undo step).
    fn set_blend(&mut self, mode: BlendMode) {
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            return;
        };
        if layer.blend == mode || (mode == BlendMode::PassThrough && !layer.is_group()) {
            return;
        }
        let id = layer.id;
        self.record_change(t!("xchg-hist-blending"), Some((id, "blend")));
        if let Some(doc) = &mut self.doc {
            let layer = doc.active_layer_mut();
            layer.blend = mode;
            self.status = t!(
                "core-status-blend",
                name = layer.name.as_str(),
                mode = mode.label()
            );
        }
    }

    /// Shift++ / Shift+-: next / previous blend mode; Alt+Shift+letter: a
    /// mode, like Photoshop (with the painting tools, these keys are the
    /// tool's).
    pub(super) fn blend_shortcut(&self, input: &egui::InputState) -> Option<Action> {
        let painting = matches!(
            self.tool,
            Tool::Brush
                | Tool::Eraser
                | Tool::Gradient
                | Tool::Bucket
                | Tool::Dodge
                | Tool::Burn
                | Tool::Sponge
                | Tool::Blur
                | Tool::Sharpen
                | Tool::Smudge
                | Tool::CloneStamp
                | Tool::HealingBrush
                | Tool::SpotHealing
        );
        if painting {
            return None;
        }
        input.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                let shift_only = modifiers.matches_exact(Modifiers::SHIFT);
                match key {
                    Key::Plus | Key::Equals if shift_only => Some(Action::BlendStep(true)),
                    Key::Minus if shift_only => Some(Action::BlendStep(false)),
                    _ if modifiers.matches_exact(Modifiers::ALT.plus(Modifiers::SHIFT)) => {
                        let letter = key
                            .name()
                            .chars()
                            .next()
                            .filter(|_| key.name().len() == 1)?;
                        BlendMode::cycle(true)
                            .into_iter()
                            .find(|m| m.letter() == Some(letter))
                            .map(Action::SetBlend)
                    }
                    _ => None,
                }
            }
            _ => None,
        })
    }

    /// Layer › Layer Style, and the styles button of the Layers panel.
    pub(super) fn style_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let pixels = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active_layer().has_pixels());
        let styled = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active_layer().effects.any());
        let mut entries = vec![
            Item(
                format!("{}…", t!("core-blending-options")),
                pixels,
                None,
                Action::LayerStyle(None),
            ),
            Separator,
        ];
        entries.extend(Style::ALL.into_iter().map(|s| {
            Item(
                format!("{}…", s.label()),
                pixels,
                None,
                Action::LayerStyle(Some(s)),
            )
        }));
        entries.push(Separator);
        entries.push(Item(
            t!("core-clear-layer-style"),
            styled,
            None,
            Action::ClearLayerStyle,
        ));
        entries
    }

    /// The fill and adjustment layers (Layer › New Fill Layer / New
    /// Adjustment Layer, and the panel's button).
    pub(super) fn new_fill_or_adjustment_menu(&self) -> Vec<MenuEntry> {
        let mut entries = self.new_fill_menu();
        entries.push(MenuEntry::Separator);
        entries.extend(self.new_adjustment_menu());
        entries
    }

    pub(super) fn new_fill_menu(&self) -> Vec<MenuEntry> {
        let has_doc = self.doc.is_some();
        vec![
            MenuEntry::Item(
                format!("{}…", t!("core-solid-color")),
                has_doc,
                None,
                Action::NewFillLayer(false),
            ),
            MenuEntry::Item(
                format!("{}…", t!("core-gradient")),
                has_doc,
                None,
                Action::NewFillLayer(true),
            ),
        ]
    }

    /// The adjustments, in the order of Photoshop's menu.
    pub(super) fn new_adjustment_menu(&self) -> Vec<MenuEntry> {
        let has_doc = self.doc.is_some();
        ADJUSTMENTS
            .iter()
            .flat_map(|a| {
                let sep = matches!(a.kind, AdjustKind::Vibrance | AdjustKind::Invert)
                    .then_some(MenuEntry::Separator);
                let item =
                    MenuEntry::Item(a.label(), has_doc, None, Action::NewAdjustmentLayer(a.kind));
                sep.into_iter().chain([item])
            })
            .collect()
    }

    /// The Layer Style window: live on the layer, one undo step on OK.
    pub(super) fn layer_style_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.layer_style.take() else {
            return;
        };
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            self.layer_style = None;
            return;
        };
        if layer.id != dialog.layer {
            self.layer_style = None;
            return;
        }
        let (mut fx, mut blend, mut opacity, mut fill) = (
            layer.effects.clone(),
            layer.blend,
            layer.opacity,
            layer.fill,
        );
        let mut hovered = None;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                width: 640.0,
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new(
                    "layer_style",
                    t!("core-layer-style"),
                    ctx.content_rect().center(),
                )
            },
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 16.0;
                    ui.vertical(|ui| {
                        ui.set_width(190.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        style_list(ui, &mut dialog.page, &mut fx);
                    });
                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let title = dialog
                            .page
                            .map_or_else(|| t!("core-blending-options"), Style::label);
                        ui_kit::section(ui, &title);
                        match dialog.page {
                            None => {
                                hovered = blending_options(ui, &mut blend, &mut opacity, &mut fill);
                            }
                            Some(style) => style_page(ui, style, &mut fx),
                        }
                    });
                });
            },
        );
        self.blend_preview = hovered.map(|m| (dialog.layer, m));
        let now = (fx, blend, opacity, fill);
        let unchanged = now == dialog.before;
        if let Some(doc) = &mut self.doc {
            let layer = doc.active_layer_mut();
            (layer.effects, layer.blend, layer.opacity, layer.fill) = now;
            doc.modified = true;
        }
        self.layer_style = Some(dialog);
        if out.cancel || (out.ok && unchanged) {
            // The state from before the window.
            if let Some(doc) = &mut self.doc {
                doc.discard_step();
            }
        }
        if out.ok || out.cancel {
            self.layer_style = None;
        }
    }
}

/// Left column of the Layer Style window: Blending Options, then each style
/// with its checkbox.
fn style_list(ui: &mut egui::Ui, page: &mut Option<Style>, fx: &mut Effects) {
    let item = |ui: &mut egui::Ui, selected: bool, label: &str| {
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
        let fill = if selected {
            color::ACCENT_SOFT
        } else if response.hovered() {
            color::BG3
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 6.0, fill);
        ui.painter().text(
            rect.left_center() + vec2(36.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            ui_kit::medium(ui_kit::TEXT_SIZE),
            color::TEXT,
        );
        (rect, response)
    };
    let (_, response) = item(ui, page.is_none(), &t!("core-blending-options"));
    if response.clicked() {
        *page = None;
    }
    for style in Style::ALL {
        let (rect, response) = item(ui, *page == Some(style), &style.label());
        let mut on = fx.enabled(style);
        let check = Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), Vec2::splat(16.0));
        let toggled = ui.interact(
            check,
            ui.id().with(("style_on", style as u8)),
            Sense::click(),
        );
        ui.painter().rect(
            check,
            4.0,
            if on { color::ACCENT } else { color::BG0 },
            UiStroke::new(1.0, color::LINE_HOVER),
            StrokeKind::Inside,
        );
        if on {
            ui.painter().text(
                check.center(),
                egui::Align2::CENTER_CENTER,
                icon::CHECK,
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
        if toggled.clicked() {
            on = !on;
            *fx.enabled_mut(style) = on;
        } else if response.clicked() {
            // Choosing a style turns it on, like Photoshop.
            *page = Some(style);
            *fx.enabled_mut(style) = true;
        }
    }
}

/// Blend mode, opacity and fill opacity; returns the mode under the
/// pointer in the open list.
fn blending_options(
    ui: &mut egui::Ui,
    blend: &mut BlendMode,
    opacity: &mut f32,
    fill: &mut f32,
) -> Option<BlendMode> {
    let hovered = labeled(ui, &t!("panel-blend-mode"), |ui, w| {
        blend_combo(ui, "style_blend", blend, false, w)
    });
    gap_between_blocks(ui);
    percent(ui, &t!("panel-opacity"), opacity, 100.0);
    gap_between_blocks(ui);
    percent(ui, &t!("core-fill-opacity"), fill, 100.0);
    hovered
}

/// Label on the left, then a control of the rest of the width.
fn labeled<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui, f32) -> R) -> R {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(110.0, 28.0), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(ui_kit::TEXT_SIZE),
            color::MUTED,
        );
        let w = ui.available_width();
        add(ui, w)
    })
    .inner
}

/// A 0..1 value as a percent slider.
fn percent(ui: &mut egui::Ui, label: &str, value: &mut f32, default: f32) {
    let mut v = (*value * 100.0).round();
    let unit = i18n::percent("");
    slider_row(
        ui,
        label,
        None,
        &mut v,
        0.0..=100.0,
        &unit,
        Track::Accent,
        default,
    );
    *value = v / 100.0;
}

/// Pixel size slider.
fn pixels(ui: &mut egui::Ui, label: &str, value: &mut f32, max: f32, default: f32) {
    slider_row(
        ui,
        label,
        None,
        value,
        0.0..=max,
        " px",
        Track::Accent,
        default,
    );
}

/// Blend mode and color of a style.
fn mode_and_color(ui: &mut egui::Ui, id: &str, blend: &mut BlendMode, rgba: &mut [u8; 4]) {
    labeled(ui, &t!("panel-blend-mode"), |ui, w| {
        ui.spacing_mut().item_spacing.x = 8.0;
        blend_combo(ui, id, blend, false, (w - 36.0).max(40.0));
        let [r, g, b, a] = *rgba;
        let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
        if ui_kit::swatch(ui, &mut c, 28.0, &t!("core-style-color-tip")).changed() {
            *rgba = c.to_srgba_unmultiplied();
        }
    });
}

/// Settings of one style.
fn style_page(ui: &mut egui::Ui, style: Style, fx: &mut Effects) {
    let d = Effects::default();
    match style {
        Style::Bevel => {
            let b = &mut fx.bevel;
            labeled(ui, &t!("core-bevel-direction"), |ui, _| {
                let (up, down) = (t!("core-bevel-up"), t!("core-bevel-down"));
                ui_kit::segmented(ui, &mut b.up, &[(true, &up, &up), (false, &down, &down)]);
            });
            gap_between_blocks(ui);
            let mut depth = (b.depth * 100.0).round();
            let unit = i18n::percent("");
            slider_row(
                ui,
                &t!("core-bevel-depth"),
                None,
                &mut depth,
                1.0..=1000.0,
                &unit,
                Track::Accent,
                100.0,
            );
            b.depth = depth / 100.0;
            gap_between_blocks(ui);
            pixels(ui, &t!("core-style-size"), &mut b.size, 250.0, d.bevel.size);
            gap_between_blocks(ui);
            pixels(ui, &t!("core-bevel-soften"), &mut b.soften, 16.0, 0.0);
            gap_between_blocks(ui);
            let angle = t!("core-style-angle");
            slider_row(
                ui,
                &angle,
                None,
                &mut b.angle,
                -180.0..=180.0,
                "°",
                Track::Accent,
                120.0,
            );
            gap_between_blocks(ui);
            let altitude = t!("core-bevel-altitude");
            slider_row(
                ui,
                &altitude,
                None,
                &mut b.altitude,
                0.0..=90.0,
                "°",
                Track::Accent,
                30.0,
            );
            gap_between_blocks(ui);
            ui_kit::field_label(ui, &t!("core-bevel-highlight"));
            mode_and_color(
                ui,
                "bevel_highlight",
                &mut b.highlight_blend,
                &mut b.highlight,
            );
            percent(ui, &t!("panel-opacity"), &mut b.highlight_opacity, 75.0);
            gap_between_blocks(ui);
            ui_kit::field_label(ui, &t!("core-bevel-shadow"));
            mode_and_color(ui, "bevel_shadow", &mut b.shadow_blend, &mut b.shadow);
            percent(ui, &t!("panel-opacity"), &mut b.shadow_opacity, 75.0);
        }
        Style::Stroke => {
            let s = &mut fx.stroke;
            pixels(
                ui,
                &t!("core-style-size"),
                &mut s.width,
                250.0,
                d.stroke.width,
            );
            gap_between_blocks(ui);
            mode_and_color(ui, "stroke_blend", &mut s.blend, &mut s.color);
            gap_between_blocks(ui);
            percent(ui, &t!("panel-opacity"), &mut s.opacity, 100.0);
        }
        Style::InnerShadow => shadow_page(ui, "inner_shadow", &mut fx.inner_shadow, d.inner_shadow),
        Style::DropShadow => shadow_page(ui, "drop_shadow", &mut fx.shadow, d.shadow),
        Style::InnerGlow => glow_page(ui, "inner_glow", &mut fx.inner_glow, &t!("core-glow-choke")),
        Style::OuterGlow => glow_page(
            ui,
            "outer_glow",
            &mut fx.outer_glow,
            &t!("core-glow-spread"),
        ),
        Style::ColorOverlay => {
            let o = &mut fx.color_overlay;
            mode_and_color(ui, "overlay_blend", &mut o.blend, &mut o.color);
            gap_between_blocks(ui);
            percent(ui, &t!("panel-opacity"), &mut o.opacity, 100.0);
        }
        Style::GradientOverlay => {
            let g = &mut fx.gradient_overlay;
            labeled(ui, &t!("panel-blend-mode"), |ui, w| {
                blend_combo(ui, "gradient_overlay_blend", &mut g.blend, false, w)
            });
            gap_between_blocks(ui);
            percent(ui, &t!("panel-opacity"), &mut g.opacity, 100.0);
            gap_between_blocks(ui);
            gradient_fill_fields(ui, &mut g.fill);
        }
    }
}

fn shadow_page(ui: &mut egui::Ui, id: &str, s: &mut ShadowFx, d: ShadowFx) {
    mode_and_color(ui, id, &mut s.blend, &mut s.color);
    gap_between_blocks(ui);
    percent(ui, &t!("panel-opacity"), &mut s.opacity, d.opacity * 100.0);
    gap_between_blocks(ui);
    let half = (ui.available_width() - 8.0) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui_kit::number_field(ui, "X", &mut s.dx, -250.0..=250.0, " px", half);
        ui_kit::number_field(ui, "Y", &mut s.dy, -250.0..=250.0, " px", half);
    });
    gap_between_blocks(ui);
    pixels(ui, &t!("core-style-size"), &mut s.blur, 250.0, d.blur);
}

fn glow_page(ui: &mut egui::Ui, id: &str, g: &mut GlowFx, spread: &str) {
    mode_and_color(ui, id, &mut g.blend, &mut g.color);
    gap_between_blocks(ui);
    percent(ui, &t!("panel-opacity"), &mut g.opacity, 75.0);
    gap_between_blocks(ui);
    percent(ui, spread, &mut g.spread, 0.0);
    gap_between_blocks(ui);
    pixels(ui, &t!("core-style-size"), &mut g.size, 250.0, 5.0);
}

/// The capture's document with every kind of layer (see the capture test):
/// `mask` targets a mask, `pixels` a layer's pixels, `adjustment` shows an
/// adjustment layer, `style` opens the Layer Style window.
#[cfg(test)]
pub(super) fn capture_layers(app: &mut AtelierApp, what: &str) {
    use crate::pixel_ops::HueSaturation;
    let Some(doc) = &mut app.doc else { return };
    let red = image::RgbaImage::from_pixel(160, 120, image::Rgba([220, 60, 40, 255]));
    doc.add_layer("Shape".into(), red, (60, 300));
    doc.active_layer_mut().effects.shadow.enabled = true;
    doc.active_layer_mut().effects.stroke.enabled = true;
    let stripes = image::RgbaImage::from_fn(512, 512, |x, _| {
        image::Rgba([40, 160, 220, if (x / 16) % 2 == 0 { 255 } else { 0 }])
    });
    doc.add_layer("Stripes".into(), stripes, (0, 0));
    doc.toggle_clipping();
    let warm = Adjustment::HueSaturation(HueSaturation {
        hue: 30.0,
        saturation: 0.3,
        ..Default::default()
    });
    doc.add_adjustment_layer(warm, None);
    let n = doc.layers.len();
    doc.group_layers(&[n - 3, n - 2, n - 1]);
    doc.active_layer_mut().blend = BlendMode::PassThrough;
    doc.add_fill_layer(Fill::Solid([250, 230, 120]), None);
    doc.active_layer_mut().blend = BlendMode::Multiply;
    doc.active_layer_mut().opacity = 0.4;
    let sel: Selection = PixelRect {
        x: 0,
        y: 0,
        w: 256,
        h: 512,
    }
    .into();
    doc.delete_mask();
    doc.add_mask(NewMask::RevealSelection, Some(&sel));
    if what == "style" || what == "pixels" {
        doc.active = 1;
        doc.target_mask(false);
    }
    if what == "adjustment" {
        doc.active = 3;
    }
    if what == "style" {
        app.run(Action::LayerStyle(Some(Style::DropShadow)));
    }
}
