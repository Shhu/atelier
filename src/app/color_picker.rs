//! Color Picker, opened by every color button (see `ui_kit::picker_link`),
//! and the Swatches panel. Like Photoshop's: a saturation / brightness
//! field and a hue strip (or any other component of HSB or RGB), new and
//! current colors, HSB, RGB, Lab, CMYK (working CMYK) and hex values, the
//! out-of-gamut warning; a click on the image samples it.

use super::dialogs::{DialogSpec, floating_dialog, panel_pos};
use super::keys;
use super::*;
use crate::color::{self, Hsb};
use crate::i18n::t;
use crate::swatches::Swatches;
use ui_kit::color as ui_color;

/// Field and strip height (and field width).
const FIELD: f32 = 268.0;
const STRIP_WIDTH: f32 = 20.0;
/// Swatches panel: a cell, the gap between cells.
const CELL: f32 = 18.0;
const CELL_GAP: f32 = 3.0;
const PICKER_ID: &str = "color_picker";

/// The color tools of the app.
pub(super) struct Colors {
    pub picker: Option<ColorPicker>,
    /// Component shown by the strip, kept between openings.
    channel: Channel,
    pub swatches: Swatches,
    /// Swatches panel open (Window menu).
    pub panel_open: bool,
}

impl Colors {
    pub(super) fn load() -> Self {
        Self {
            picker: None,
            channel: Channel::Hue,
            swatches: Swatches::load(),
            panel_open: false,
        }
    }
}

/// Component shown by the strip; the field shows the two others of the
/// same model (Photoshop's radio buttons).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Channel {
    Hue,
    Saturation,
    Brightness,
    Red,
    Green,
    Blue,
}

impl Channel {
    const ALL: [Channel; 6] = [
        Channel::Hue,
        Channel::Saturation,
        Channel::Brightness,
        Channel::Red,
        Channel::Green,
        Channel::Blue,
    ];
}

/// The color being chosen, in both models: HSB keeps the hue of a gray and
/// the saturation of black while RGB values are typed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Picked {
    hsb: Hsb,
    rgb: [f32; 3],
}

impl Picked {
    fn from_rgb(rgb: [u8; 3]) -> Self {
        let rgb = color::to_f32(rgb);
        Self {
            hsb: color::rgb_to_hsb(rgb),
            rgb,
        }
    }

    fn set_hsb(&mut self, hsb: Hsb) {
        self.hsb = Hsb {
            h: hsb.h.clamp(0.0, 360.0),
            s: hsb.s.clamp(0.0, 1.0),
            b: hsb.b.clamp(0.0, 1.0),
        };
        self.rgb = color::hsb_to_rgb(self.hsb);
    }

    fn set_rgb(&mut self, rgb: [f32; 3]) {
        self.rgb = rgb.map(|c| c.clamp(0.0, 1.0));
        let new = color::rgb_to_hsb(self.rgb);
        let keep_hue = new.s == 0.0 || new.b == 0.0;
        self.hsb = Hsb {
            h: if keep_hue { self.hsb.h } else { new.h },
            s: if new.b == 0.0 { self.hsb.s } else { new.s },
            b: new.b,
        };
    }

    pub(super) fn rgb8(&self) -> [u8; 3] {
        color::to_u8(self.rgb)
    }

    /// Position of the color on the strip and in the field (0..1, up and
    /// right).
    fn axes(&self, channel: Channel) -> (f32, f32, f32) {
        let Hsb { h, s, b } = self.hsb;
        let h = h / 360.0;
        let [r, g, bl] = self.rgb;
        match channel {
            Channel::Hue => (h, s, b),
            Channel::Saturation => (s, h, b),
            Channel::Brightness => (b, h, s),
            Channel::Red => (r, bl, g),
            Channel::Green => (g, bl, r),
            Channel::Blue => (bl, r, g),
        }
    }

    /// The color at that position of the strip and the field.
    fn set_axes(&mut self, channel: Channel, strip: f32, x: f32, y: f32) {
        let hsb = |h: f32, s, b| Hsb { h: h * 360.0, s, b };
        match channel {
            Channel::Hue => self.set_hsb(hsb(strip, x, y)),
            Channel::Saturation => self.set_hsb(hsb(x, strip, y)),
            Channel::Brightness => self.set_hsb(hsb(x, y, strip)),
            Channel::Red => self.set_rgb([strip, y, x]),
            Channel::Green => self.set_rgb([y, strip, x]),
            Channel::Blue => self.set_rgb([x, y, strip]),
        }
    }
}

/// sRGB color at a position of the strip and the field.
fn color_at(channel: Channel, strip: f32, x: f32, y: f32) -> Color32 {
    let mut p = Picked {
        hsb: Hsb {
            h: 0.0,
            s: 0.0,
            b: 0.0,
        },
        rgb: [0.0; 3],
    };
    p.set_axes(channel, strip, x, y);
    let [r, g, b] = p.rgb8();
    Color32::from_rgb(r, g, b)
}

/// Open Color Picker.
pub(super) struct ColorPicker {
    /// Color button that opened it.
    target: egui::Id,
    pub(super) title: String,
    /// Color of the button when it opened ("current").
    current: Color32,
    pub(super) picked: Picked,
    /// Text of the hex field while it is being typed in.
    hex: Option<String>,
    /// Field and strip images, with the key of what they show.
    field: Option<(u64, egui::TextureHandle)>,
    strip: Option<(u64, egui::TextureHandle)>,
    /// A press on the image samples its colors until released.
    sampling: bool,
    /// The picked color in the working CMYK, once known.
    pub(super) ink: Option<Ink>,
}

/// A color in the working CMYK: its inks (%), whether it prints, and the
/// closest color that does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ink {
    pub(super) rgb: [u8; 3],
    pub(super) cmyk: [f32; 4],
    pub(super) printable: bool,
    pub(super) closest: [u8; 3],
}

impl ColorPicker {
    fn new(request: ui_kit::PickerRequest) -> Self {
        let [r, g, b, _] = request.color.to_srgba_unmultiplied();
        Self {
            target: request.target,
            title: request.title,
            current: request.color,
            picked: Picked::from_rgb([r, g, b]),
            hex: None,
            field: None,
            strip: None,
            sampling: false,
            ink: None,
        }
    }

    fn color(&self) -> Color32 {
        let [r, g, b] = self.picked.rgb8();
        Color32::from_rgb(r, g, b)
    }
}

/// Texture `slot` showing `size` pixels of `paint(u, v)` (0..1, v up), made
/// again when `key` changes.
fn texture(
    ctx: &egui::Context,
    slot: &mut Option<(u64, egui::TextureHandle)>,
    name: &str,
    key: u64,
    [w, h]: [usize; 2],
    paint: impl Fn(f32, f32) -> Color32,
) -> egui::TextureId {
    if slot.as_ref().is_none_or(|(k, _)| *k != key) {
        let step = |n: usize| 1.0 / (n.max(2) - 1) as f32;
        let (du, dv) = (step(w), step(h));
        let pixels = (0..h)
            .flat_map(|j| (0..w).map(move |i| (i, j)))
            .map(|(i, j)| paint(i as f32 * du, 1.0 - j as f32 * dv))
            .collect();
        let image = ColorImage::new([w, h], pixels);
        match slot {
            Some((k, tex)) => {
                tex.set(image, TextureOptions::LINEAR);
                *k = key;
            }
            None => *slot = Some((key, ctx.load_texture(name, image, TextureOptions::LINEAR))),
        }
    }
    slot.as_ref().expect("made above").1.id()
}

impl AtelierApp {
    /// The Color Picker asked for by a color button, while it is open.
    pub(super) fn color_picker_window(&mut self, ctx: &egui::Context) {
        if let Some(request) = ui_kit::take_picker_request(ctx) {
            self.colors.picker = Some(ColorPicker::new(request));
        }
        let Some(mut picker) = self.colors.picker.take() else {
            return;
        };
        let before = picker.picked;
        let mut channel = self.colors.channel;
        let (mut add, mut closest, mut typed) = (false, false, None);
        let rgb = picker.picked.rgb8();
        if picker.ink.is_none_or(|i| i.rgb != rgb) {
            picker.ink = self.ink_of(rgb);
        }
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 572.0,
                note: Some(t!("uip-picker-sample-hint")),
                ..DialogSpec::new(PICKER_ID, picker.title.clone(), center)
            },
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    field_and_strip(ui, &mut picker, channel);
                    ui.add_space(6.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                        (add, closest) = compare(ui, &mut picker);
                        ui.add_space(2.0);
                        typed = values(ui, &mut picker, &mut channel);
                    });
                });
            },
        );
        self.colors.channel = channel;
        if let Some(rgb) = typed.and_then(|cmyk| self.rgb_of_ink(cmyk)) {
            picker.picked.set_rgb(color::to_f32(rgb));
        }
        if let Some(ink) = picker.ink.filter(|_| closest) {
            picker.picked.set_rgb(color::to_f32(ink.closest));
        }
        self.sample_for_picker(ctx, &mut picker);
        if add {
            let rgb = picker.picked.rgb8();
            self.colors.swatches.add(rgb);
            self.status = t!("uip-status-swatch-added", hex = hex_color(rgb));
        }
        if picker.picked != before {
            ui_kit::send_picked(ctx, picker.target, picker.color());
        }
        if out.cancel {
            ui_kit::send_picked(ctx, picker.target, picker.current);
        } else if out.ok {
            self.colors.swatches.use_color(picker.picked.rgb8());
        } else {
            self.colors.picker = Some(picker);
        }
    }

    /// While the Color Picker is open, the pointer samples the image (all
    /// visible layers), like Photoshop's.
    fn sample_for_picker(&mut self, ctx: &egui::Context, picker: &mut ColorPicker) {
        let (pointer, pressed, down) = ctx.input(|i| {
            (
                i.pointer.latest_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
            )
        });
        let dialog = ctx.memory(|m| m.area_rect(egui::Id::new(PICKER_ID)));
        let over_image = pointer.filter(|&p| {
            !dialog.is_some_and(|d| d.contains(p))
                && self.rulers.canvas_area().is_some_and(|a| a.contains(p))
        });
        if !down {
            picker.sampling = false;
        } else if pressed {
            picker.sampling = over_image.is_some();
        }
        if over_image.is_some() || picker.sampling {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
        }
        let (Some(p), Some(view), Some(doc)) = (pointer, self.last_view, &self.doc) else {
            return;
        };
        if picker.sampling {
            let ip = view.to_image(p);
            if let Some([r, g, b, _]) = doc.color_at(ip.x.floor() as i64, ip.y.floor() as i64) {
                picker.picked.set_rgb(color::to_f32([r, g, b]));
            }
        }
    }

    /// Swatches panel (Window › Swatches): a click picks the foreground
    /// color, Ctrl+click the background; Alt+click deletes a swatch.
    pub(super) fn swatches_window(&mut self, ctx: &egui::Context) {
        if !self.colors.panel_open {
            return;
        }
        let mut chosen = None;
        let mut delete = None;
        let mut add = false;
        let swatches = &self.colors.swatches;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                ..DialogSpec::new(
                    "swatches",
                    t!("uip-swatches-title"),
                    panel_pos(ctx, "swatches"),
                )
            },
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(CELL_GAP);
                // Rows of swatches as high as a swatch.
                ui.spacing_mut().interact_size.y = CELL;
                if !swatches.recent.is_empty() {
                    ui_kit::field_label(ui, &t!("uip-swatches-recent"));
                    ui.horizontal(|ui| {
                        for &rgb in &swatches.recent {
                            if let Some(m) = swatch_cell(ui, rgb) {
                                chosen = Some((rgb, m));
                            }
                        }
                    });
                    ui.add_space(8.0);
                }
                ui.horizontal_wrapped(|ui| {
                    for (i, &rgb) in swatches.colors.iter().enumerate() {
                        match swatch_cell(ui, rgb) {
                            Some(m) if m.alt => delete = Some(i),
                            Some(m) => chosen = Some((rgb, m)),
                            None => {}
                        }
                    }
                });
                ui.add_space(8.0);
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let tip = t!("uip-swatches-new-tip");
                    add = ui_kit::icon_button(ui, icon::PLUS, &tip, true).clicked();
                    let click = t!("hint-key-click");
                    let hint = t!(
                        "uip-swatches-hint",
                        background = keys::combo(ui.ctx(), Modifiers::COMMAND, &click),
                        delete = keys::combo(ui.ctx(), Modifiers::ALT, &click)
                    );
                    let text = egui::RichText::new(hint).size(12.0).color(ui_color::LABEL);
                    ui.add(egui::Label::new(text).wrap());
                });
            },
        );
        if let Some(([r, g, b], m)) = chosen {
            let color = Color32::from_rgb(r, g, b);
            if m.command {
                self.secondary = color;
            } else {
                self.primary = color;
            }
            self.colors.swatches.use_color([r, g, b]);
        }
        if let Some(i) = delete {
            self.colors.swatches.delete(i);
        }
        if add {
            let [r, g, b, _] = self.primary.to_srgba_unmultiplied();
            self.colors.swatches.add([r, g, b]);
        }
        if out.cancel {
            self.colors.panel_open = false;
        }
    }
}

/// A swatch of the panel; the modifiers of a click on it.
fn swatch_cell(ui: &mut egui::Ui, [r, g, b]: [u8; 3]) -> Option<Modifiers> {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(CELL), Sense::click());
    let border = if response.hovered() {
        ui_color::TEXT
    } else {
        ui_color::SWATCH_LINE
    };
    ui.painter().rect(
        rect,
        3.0,
        Color32::from_rgb(r, g, b),
        UiStroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let hex = hex_color([r, g, b]);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &hex));
    let response = response.on_hover_text(hex);
    response.clicked().then(|| ui.input(|i| i.modifiers))
}

/// The color field and the strip beside it; dragging either picks.
fn field_and_strip(ui: &mut egui::Ui, picker: &mut ColorPicker, channel: Channel) {
    let ctx = ui.ctx().clone();
    let (strip_value, x, y) = picker.picked.axes(channel);
    let quantize = |v: f32| (v * 4096.0).round() as i64;

    let (field_rect, field) = ui.allocate_exact_size(Vec2::splat(FIELD), Sense::click_and_drag());
    let key = hash_of((channel, quantize(strip_value)));
    let tex = texture(
        &ctx,
        &mut picker.field,
        "picker-field",
        key,
        [256, 256],
        |u, v| color_at(channel, strip_value, u, v),
    );
    let painter = ui.painter();
    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
    painter.image(tex, field_rect, uv, Color32::WHITE);
    painter.rect_stroke(
        field_rect,
        0.0,
        UiStroke::new(1.0, ui_color::LINE),
        StrokeKind::Outside,
    );

    let (strip_rect, strip) =
        ui.allocate_exact_size(vec2(STRIP_WIDTH, FIELD), Sense::click_and_drag());
    let key = hash_of((channel, quantize(x), quantize(y)));
    let tex = texture(
        &ctx,
        &mut picker.strip,
        "picker-strip",
        key,
        [1, 256],
        |_, v| {
            // The hue strip shows pure hues, like Photoshop's.
            if channel == Channel::Hue {
                color_at(channel, v, 1.0, 1.0)
            } else {
                color_at(channel, v, x, y)
            }
        },
    );
    let painter = ui.painter();
    painter.image(tex, strip_rect, uv, Color32::WHITE);
    painter.rect_stroke(
        strip_rect,
        0.0,
        UiStroke::new(1.0, ui_color::LINE),
        StrokeKind::Outside,
    );

    let unit = |p: Pos2, r: Rect| {
        (
            ((p.x - r.left()) / r.width()).clamp(0.0, 1.0),
            ((r.bottom() - p.y) / r.height()).clamp(0.0, 1.0),
        )
    };
    if let Some(p) = field.interact_pointer_pos() {
        let (u, v) = unit(p, field_rect);
        picker.picked.set_axes(channel, strip_value, u, v);
    }
    if let Some(p) = strip.interact_pointer_pos() {
        let (_, v) = unit(p, strip_rect);
        picker.picked.set_axes(channel, v, x, y);
    }
    if field.hovered() || field.dragged() {
        ctx.set_cursor_icon(CursorIcon::Crosshair);
    }

    // Markers: a ring in the field, arrows beside the strip.
    let (strip_value, x, y) = picker.picked.axes(channel);
    let painter = ui.painter().with_clip_rect(field_rect.expand(6.0));
    let at = pos2(
        field_rect.left() + x * field_rect.width(),
        field_rect.bottom() - y * field_rect.height(),
    );
    painter.circle_stroke(at, 6.0, UiStroke::new(1.0, Color32::BLACK));
    painter.circle_stroke(at, 5.0, UiStroke::new(1.5, Color32::WHITE));
    let sy = strip_rect.bottom() - strip_value * strip_rect.height();
    let painter = ui.painter();
    for (edge, dir) in [
        (strip_rect.left() - 1.0, -1.0),
        (strip_rect.right() + 1.0, 1.0),
    ] {
        let points = vec![
            pos2(edge, sy),
            pos2(edge + 6.0 * dir, sy - 5.0),
            pos2(edge + 6.0 * dir, sy + 5.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            points,
            ui_color::TEXT,
            UiStroke::NONE,
        ));
    }
}

/// New color over the current one (a click on it brings it back), and Add
/// to Swatches beside them; under it, when the new color doesn't print,
/// the warning and the closest color that does. Returns whether Add to
/// Swatches, the closest color, were clicked.
fn compare(ui: &mut egui::Ui, picker: &mut ColorPicker) -> (bool, bool) {
    let (mut add, mut closest) = (false, false);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 2.0);
        ui.vertical(|ui| {
            let label = |ui: &mut egui::Ui, text: String| {
                ui.label(egui::RichText::new(text).size(11.0).color(ui_color::LABEL));
            };
            label(ui, t!("uip-picker-new"));
            let (rect, _) = ui.allocate_exact_size(vec2(64.0, 48.0), Sense::hover());
            label(ui, t!("uip-picker-current"));
            let (new, current) = rect.split_top_bottom_at_fraction(0.5);
            let back = ui
                .interact(current, ui.id().with("current"), Sense::click())
                .on_hover_text(t!("uip-picker-current-tip"));
            if back.clicked() {
                let [r, g, b, _] = picker.current.to_srgba_unmultiplied();
                picker.picked.set_rgb(color::to_f32([r, g, b]));
            }
            let [r, g, b, _] = picker.current.to_srgba_unmultiplied();
            let painter = ui.painter();
            painter.rect_filled(new, 0.0, picker.color());
            painter.rect_filled(current, 0.0, Color32::from_rgb(r, g, b));
            painter.rect_stroke(
                rect,
                0.0,
                UiStroke::new(1.0, ui_color::SWATCH_LINE),
                StrokeKind::Outside,
            );
        });
        ui.vertical(|ui| {
            ui.add_space(15.0);
            let label = t!("uip-picker-add-to-swatches");
            let button = ui_kit::button(ui, &label, Kind::Normal, None, true);
            button.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.as_str())
            });
            add = button.clicked();
            if let Some(ink) = picker.ink.filter(|i| !i.printable) {
                closest = gamut_warning(ui, ink.closest);
            }
        });
    });
    (add, closest)
}

/// The out-of-gamut triangle and the closest printable color: a click
/// on either picks that one.
fn gamut_warning(ui: &mut egui::Ui, [r, g, b]: [u8; 3]) -> bool {
    ui.add_space(4.0);
    let (rect, response) = ui.allocate_exact_size(vec2(18.0, 30.0), Sense::click());
    let response = response.on_hover_text(t!("color-picker-gamut"));
    let painter = ui.painter();
    let color = if response.hovered() {
        ui_color::TEXT
    } else {
        ui_color::MUTED
    };
    let (top, chip) = rect.split_top_bottom_at_y(rect.top() + 16.0);
    painter.text(
        top.center(),
        egui::Align2::CENTER_CENTER,
        icon::WARNING,
        egui::FontId::proportional(15.0),
        color,
    );
    let chip = Rect::from_center_size(chip.center(), vec2(12.0, 10.0));
    painter.rect_filled(chip, 0.0, Color32::from_rgb(r, g, b));
    painter.rect_stroke(
        chip,
        0.0,
        UiStroke::new(1.0, ui_color::SWATCH_LINE),
        StrokeKind::Outside,
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t!("color-picker-gamut"))
    });
    response.clicked()
}

/// HSB and RGB values with their radio buttons (the strip's component)
/// and the hex code; beside them, the Lab and CMYK values. Returns CMYK
/// values typed (to convert).
fn values(ui: &mut egui::Ui, picker: &mut ColorPicker, channel: &mut Channel) -> Option<[f32; 4]> {
    let width = (ui.available_width() - 8.0) / 2.0;
    let field = width - 22.0 - 4.0;
    let p = &mut picker.picked;
    let Hsb { h, s, b } = p.hsb;
    let rgb = p.rgb8();
    let lab = color::rgb_to_lab(p.rgb);
    let ink = picker.ink.filter(|i| i.rgb == rgb).map(|i| i.cmyk);
    let mut typed = None;
    for row in 0..Channel::ALL.len() + 1 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let Some(&ch) = Channel::ALL.get(row) else {
                // Under the channels: the hex code.
                ui.add_space(22.0);
                if let Some(rgb) = hex_field(ui, &mut picker.hex, rgb, field) {
                    p.set_rgb(color::to_f32(rgb));
                }
                ui.add_space(4.0);
                cmyk_field(ui, ink, 3, width, &mut typed);
                return;
            };
            if radio(ui, *channel == ch).clicked() {
                *channel = ch;
            }
            match ch {
                Channel::Hue => {
                    let mut v = h.round() as i32;
                    let label = t!("uip-picker-h");
                    if ui_kit::number_field(ui, &label, &mut v, 0..=360, "°", field).changed() {
                        p.set_hsb(Hsb {
                            h: v as f32,
                            ..p.hsb
                        });
                    }
                }
                Channel::Saturation | Channel::Brightness => {
                    let sat = ch == Channel::Saturation;
                    let mut v = (if sat { s } else { b } * 100.0).round() as i32;
                    let label = if sat {
                        t!("uip-picker-s")
                    } else {
                        t!("uip-picker-b")
                    };
                    let suffix = crate::i18n::percent("");
                    if ui_kit::number_field(ui, &label, &mut v, 0..=100, &suffix, field).changed() {
                        let v = v as f32 / 100.0;
                        p.set_hsb(if sat {
                            Hsb { s: v, ..p.hsb }
                        } else {
                            Hsb { b: v, ..p.hsb }
                        });
                    }
                }
                Channel::Red | Channel::Green | Channel::Blue => {
                    let i = row - 3;
                    let mut v = i32::from(rgb[i]);
                    let label = match ch {
                        Channel::Red => t!("uip-picker-red"),
                        Channel::Green => t!("uip-picker-green"),
                        _ => t!("uip-picker-blue"),
                    };
                    if ui_kit::number_field(ui, &label, &mut v, 0..=255, "", field).changed() {
                        let mut new = color::to_f32(rgb);
                        new[i] = v as f32 / 255.0;
                        p.set_rgb(new);
                    }
                }
            }
            ui.add_space(4.0);
            match row {
                0..=2 => {
                    let (label, range) = match row {
                        0 => ("L", 0..=100),
                        1 => ("a", -128..=127),
                        _ => ("b", -128..=127),
                    };
                    let mut v = lab[row].round() as i32;
                    if ui_kit::number_field(ui, label, &mut v, range, "", width).changed() {
                        let mut new = lab.map(f32::round);
                        new[row] = v as f32;
                        p.set_rgb(color::lab_to_rgb(new));
                    }
                }
                _ => cmyk_field(ui, ink, row - 3, width, &mut typed),
            }
        });
    }
    typed
}

/// Ink `i` (0 cyan … 3 black) of `ink`, in % (disabled until known); a
/// value typed gives `typed`.
fn cmyk_field(
    ui: &mut egui::Ui,
    ink: Option<[f32; 4]>,
    i: usize,
    width: f32,
    typed: &mut Option<[f32; 4]>,
) {
    let label = ["C", "M", "Y", "K"][i];
    let suffix = crate::i18n::percent("");
    let Some(cmyk) = ink else {
        ui.add_enabled_ui(false, |ui| {
            ui_kit::number_field(ui, label, &mut 0, 0..=100, &suffix, width);
        });
        return;
    };
    let mut v = cmyk[i].round() as i32;
    if ui_kit::number_field(ui, label, &mut v, 0..=100, &suffix, width).changed() {
        let mut new = cmyk.map(f32::round);
        new[i] = v as f32;
        *typed = Some(new);
    }
}

/// "#" field: shows `rgb`; a valid code typed or pasted (3 or 6 digits)
/// gives a new color.
fn hex_field(
    ui: &mut egui::Ui,
    typing: &mut Option<String>,
    rgb: [u8; 3],
    width: f32,
) -> Option<[u8; 3]> {
    let id = egui::Id::new("picker_hex");
    let mut text = typing.clone().unwrap_or_else(|| color::hex(rgb));
    let out = ui
        .allocate_ui(vec2(width, 28.0), |ui| {
            ui.set_width(width);
            ui_kit::text_field(ui, "#", &mut text, id)
        })
        .inner;
    let picked = out
        .response
        .changed()
        .then(|| color::parse_hex(&text))
        .flatten();
    *typing = out.response.has_focus().then_some(text);
    picked
}

/// Radio button of a component.
fn radio(ui: &mut egui::Ui, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(22.0, 28.0), Sense::click());
    let center = rect.center();
    let painter = ui.painter();
    if selected {
        painter.circle_filled(center, 7.0, ui_color::ACCENT);
        painter.circle_filled(center, 3.0, ui_color::ON_ACCENT);
    } else {
        let border = if response.hovered() {
            ui_color::MUTED
        } else {
            ui_color::LINE_HOVER
        };
        painter.circle(center, 6.5, ui_color::BG0, UiStroke::new(1.0, border));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gray_keeps_its_hue_while_rgb_is_typed() {
        let mut p = Picked::from_rgb([255, 0, 0]);
        p.set_hsb(Hsb { s: 0.0, ..p.hsb });
        p.set_hsb(Hsb { h: 200.0, ..p.hsb });
        // Typed gray: the hue stays, and comes back with the saturation.
        p.set_rgb([0.5; 3]);
        assert_eq!(p.hsb.h, 200.0);
        p.set_hsb(Hsb { s: 1.0, ..p.hsb });
        assert_eq!(p.rgb8(), [0, 85, 128]);
    }

    #[test]
    fn every_channel_round_trips_through_its_axes() {
        let p = Picked::from_rgb([200, 120, 40]);
        for ch in Channel::ALL {
            let (s, x, y) = p.axes(ch);
            let mut q = Picked::from_rgb([0, 0, 0]);
            q.set_axes(ch, s, x, y);
            assert_eq!(q.rgb8(), [200, 120, 40], "{ch:?}");
        }
        // Hue mode: white top left, the pure hue top right, black at the bottom.
        let red = 0.0;
        assert_eq!(color_at(Channel::Hue, red, 0.0, 1.0), Color32::WHITE);
        assert_eq!(color_at(Channel::Hue, red, 1.0, 1.0), Color32::RED);
        assert_eq!(color_at(Channel::Hue, red, 0.5, 0.0), Color32::BLACK);
        // Red mode: x is blue, y is green.
        let c = color_at(Channel::Red, 1.0, 1.0, 0.0);
        assert_eq!(c, Color32::from_rgb(255, 0, 255));
    }
}
