//! "Atelier" theme: colors, fonts and widgets of the UI.

use std::ops::RangeInclusive;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions, FontFamily, FontId,
    Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, TextFormat, TextStyle, Ui, Vec2,
    emath::Numeric, pos2, text::LayoutJob, vec2,
};

pub use egui_phosphor::regular as icon;

use crate::i18n;

/// Palette.
pub mod color {
    use eframe::egui::Color32;

    /// Darkest background: canvas area, input fields.
    pub const BG0: Color32 = Color32::from_rgb(0x12, 0x13, 0x15);
    /// Tool bar, document tabs.
    pub const BG1: Color32 = Color32::from_rgb(0x18, 0x19, 0x1c);
    /// Side panel, dialogs, menus.
    pub const BG2: Color32 = Color32::from_rgb(0x1f, 0x20, 0x24);
    /// Buttons, hovered rows.
    pub const BG3: Color32 = Color32::from_rgb(0x27, 0x29, 0x2e);
    /// Title bar and status bar.
    pub const TITLE: Color32 = Color32::from_rgb(0x0f, 0x10, 0x12);
    /// Options bar of the active tool.
    pub const OPTIONS: Color32 = Color32::from_rgb(0x1a, 0x1b, 0x1e);
    /// Layers list, at the bottom of the side panel.
    pub const LAYERS: Color32 = Color32::from_rgb(0x1b, 0x1c, 0x1f);
    /// Borders and separators.
    pub const LINE: Color32 = Color32::from_rgb(0x2d, 0x2f, 0x35);
    pub const LINE_HOVER: Color32 = Color32::from_rgb(0x3a, 0x3d, 0x44);
    /// Border of floating windows (dialogs, palette).
    pub const WINDOW_LINE: Color32 = Color32::from_rgb(0x36, 0x38, 0x3f);
    /// Keyboard key (kbd) borders, slider rails.
    pub const KBD_LINE: Color32 = Color32::from_rgb(0x34, 0x36, 0x3d);
    /// Hovered button.
    pub const BUTTON_HOVER: Color32 = Color32::from_rgb(0x2f, 0x32, 0x38);
    /// Color swatch borders.
    pub const SWATCH_LINE: Color32 = Color32::from_rgb(0x5a, 0x5e, 0x66);
    /// Outline of an unchecked checkbox, track of a switch that is off: at
    /// least 3:1 against the panels, so the control can be seen.
    pub const CONTROL_LINE: Color32 = Color32::from_rgb(0x6b, 0x6f, 0x79);
    pub const TEXT: Color32 = Color32::from_rgb(0xec, 0xed, 0xef);
    /// Secondary text, idle icons.
    pub const MUTED: Color32 = Color32::from_rgb(0xa3, 0xa6, 0xaf);
    /// Field labels, hints, section titles (4.5:1 on BG3).
    pub const LABEL: Color32 = Color32::from_rgb(0x91, 0x96, 0xa0);
    /// Disabled icons, hidden layers.
    pub const DISABLED: Color32 = Color32::from_rgb(0x6b, 0x6f, 0x79);
    /// Accent: frames, selection, primary button.
    pub const ACCENT: Color32 = Color32::from_rgb(0x2f, 0x6b, 0xe0);
    /// Lit accent: hovered handles and paths on the canvas.
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0x3a, 0x7a, 0xf5);
    /// Hovered primary button: darker, so white text keeps 4.5:1.
    pub const PRIMARY_HOVER: Color32 = Color32::from_rgb(0x2a, 0x62, 0xd0);
    /// Selected state background: the accent at 26 %.
    pub const ACCENT_SOFT: Color32 = Color32::from_rgba_premultiplied(12, 28, 58, 66);
    /// Selected state text: the accent mixed with 55 % white.
    pub const ACCENT_TEXT: Color32 = Color32::from_rgb(0xa1, 0xbc, 0xf1);
    /// Text laid over the accent.
    pub const ON_ACCENT: Color32 = Color32::WHITE;
    /// Close button of the window, hovered.
    pub const CLOSE_HOVER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);
    /// Thumbnail not loaded yet.
    pub const THUMB_EMPTY: Color32 = Color32::from_rgb(0x2a, 0x2c, 0x31);
    /// Dot grid behind the canvas.
    pub const DOT: Color32 = Color32::from_rgb(0x21, 0x23, 0x28);
    /// Transparency checkerboard.
    pub const CHECKER_A: Color32 = Color32::from_rgb(0x3b, 0x3c, 0x40);
    pub const CHECKER_B: Color32 = Color32::from_rgb(0x30, 0x31, 0x34);
    /// Alignment guides.
    pub const GUIDE: Color32 = Color32::from_rgb(0xff, 0x3d, 0xc8);
    /// Ruler guides (cyan, like Photoshop's).
    pub const CANVAS_GUIDE: Color32 = Color32::from_rgb(0x2e, 0xd8, 0xf5);
    /// Grid lines, and their subdivisions.
    pub const GRID: Color32 = Color32::from_rgba_premultiplied(0x5a, 0x5a, 0x5a, 0x8c);
    pub const GRID_MINOR: Color32 = Color32::from_rgba_premultiplied(0x3c, 0x3c, 0x3c, 0x5c);
}

/// Base UI text size.
pub const TEXT_SIZE: f32 = 13.0;

const MEDIUM: &str = "geist-medium";
const SEMIBOLD: &str = "geist-semibold";
const MONO_MEDIUM: &str = "geist-mono-medium";

/// Semibold: titles, headings.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

/// Medium: buttons, field labels, layer names.
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

/// Numeric values (Geist Mono).
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Numeric values, medium weight.
pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_MEDIUM.into()))
}

/// Installs fonts and style.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(style);
    // Ctrl+plus / minus / 0 zoom the image (like Photoshop), not the UI.
    ctx.options_mut(|o| o.zoom_with_keyboard = false);
}

/// Geist for the UI and Geist Mono for numbers (assets/fonts, OFL), with
/// Phosphor icons and egui's fonts as fallback.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let mut mono_fallback = fonts.families[&FontFamily::Monospace].clone();
    // Icons in numbers and keys (kbd) too.
    mono_fallback.push("phosphor".into());
    let faces: [(&str, &'static [u8]); 5] = [
        ("geist", include_bytes!("../assets/fonts/Geist-Regular.ttf")),
        (MEDIUM, include_bytes!("../assets/fonts/Geist-Medium.ttf")),
        (
            SEMIBOLD,
            include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
        ),
        (
            "geist-mono",
            include_bytes!("../assets/fonts/GeistMono-Regular.ttf"),
        ),
        (
            MONO_MEDIUM,
            include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
        ),
    ];
    for (name, data) in faces {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(data)));
    }
    let family = |first: &str, rest: &[String]| {
        std::iter::once(first.to_owned())
            .chain(rest.iter().cloned())
            .collect::<Vec<_>>()
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family("geist", &fallback));
    fonts
        .families
        .insert(FontFamily::Name(MEDIUM.into()), family(MEDIUM, &fallback));
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        family(SEMIBOLD, &fallback),
    );
    fonts
        .families
        .insert(FontFamily::Monospace, family("geist-mono", &mono_fallback));
    fonts.families.insert(
        FontFamily::Name(MONO_MEDIUM.into()),
        family(MONO_MEDIUM, &mono_fallback),
    );
    fonts
}

fn style(style: &mut egui::Style) {
    style.text_styles = [
        (TextStyle::Heading, bold(TEXT_SIZE)),
        (TextStyle::Body, FontId::proportional(TEXT_SIZE)),
        (TextStyle::Button, FontId::proportional(TEXT_SIZE)),
        (TextStyle::Small, FontId::proportional(11.0)),
        (TextStyle::Monospace, mono(12.0)),
    ]
    .into();
    style.drag_value_text_style = TextStyle::Monospace;
    // UI labels are not selectable text: dragging on them moves dialogs, etc.
    style.interaction.selectable_labels = false;

    let s = &mut style.spacing;
    s.item_spacing = vec2(8.0, 6.0);
    s.button_padding = vec2(10.0, 5.0);
    s.interact_size = vec2(40.0, 28.0);
    s.slider_rail_height = 4.0;
    s.menu_margin = egui::Margin::same(6);
    s.window_margin = egui::Margin::same(14);

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    // Crisp text on a dark background: glyphs snapped to the pixel (no
    // fractional positions, which blur) and a softer contrast than egui's
    // default dark setting, which thickens light text.
    v.text_options.subpixel_binning = false;
    v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(0.8);
    v.override_text_color = None;
    v.panel_fill = color::BG2;
    v.window_fill = color::BG2;
    v.extreme_bg_color = color::BG0;
    v.text_edit_bg_color = Some(color::BG0);
    v.faint_bg_color = color::BG3;
    v.code_bg_color = color::BG1;
    v.window_stroke = Stroke::new(1.0, color::WINDOW_LINE);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.weak_text_color = Some(color::LABEL);
    v.selection.bg_fill = color::ACCENT;
    v.selection.stroke = Stroke::new(1.0, color::ON_ACCENT);
    v.hyperlink_color = color::ACCENT_TEXT;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(128),
    };

    let w = &mut v.widgets;
    let radius = CornerRadius::same(6);
    w.noninteractive.bg_fill = color::BG2;
    w.noninteractive.weak_bg_fill = color::BG2;
    w.noninteractive.bg_stroke = Stroke::new(1.0, color::LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, color::TEXT);
    w.noninteractive.corner_radius = radius;
    for (state, weak, border, fg) in [
        (&mut w.inactive, color::BG3, color::LINE, color::TEXT),
        (
            &mut w.hovered,
            color::BUTTON_HOVER,
            color::LINE_HOVER,
            color::TEXT,
        ),
        (
            &mut w.active,
            color::BUTTON_HOVER,
            color::ACCENT,
            color::TEXT,
        ),
        (&mut w.open, color::BG3, color::LINE_HOVER, color::TEXT),
    ] {
        state.bg_fill = color::BG0;
        state.weak_bg_fill = weak;
        state.bg_stroke = Stroke::new(1.0, border);
        state.fg_stroke = Stroke::new(1.0, fg);
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
}

// ------------------------------------------------------------------ widgets

/// Text with a single format, with letter spacing.
fn job(text: &str, font: FontId, color: Color32, letter_spacing: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            extra_letter_spacing: letter_spacing,
            ..Default::default()
        },
    );
    job
}

/// Section title of a panel: small spaced capitals.
pub fn section(ui: &mut Ui, title: &str) -> Response {
    ui.label(job(&title.to_uppercase(), bold(11.0), color::LABEL, 0.77))
}

/// Small, discreet explanation text.
pub fn hint(ui: &mut Ui, text: impl Into<String>) -> Response {
    ui.label(egui::RichText::new(text).size(12.0).color(color::LABEL))
}

/// Small label above or before a field.
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(12.0).color(color::LABEL));
}

/// Vertical separator between groups of a bar.
pub fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::LINE);
}

/// Keyboard key, as shown in tooltips, the status bar and menus.
pub fn kbd(ui: &mut Ui, text: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), mono(11.0), color::MUTED);
    let (rect, response) =
        ui.allocate_exact_size(vec2(galley.size().x + 10.0, 18.0), Sense::hover());
    paint_kbd(ui.painter(), rect, galley);
    response
}

fn paint_kbd(painter: &Painter, rect: Rect, galley: Arc<egui::Galley>) {
    painter.rect(
        rect,
        4.0,
        color::BG1,
        Stroke::new(1.0, color::KBD_LINE),
        StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, color::MUTED);
}

/// Tooltip with a name and its key, like the tool bar's.
pub fn tip_with_key(response: Response, name: &str, key: &str) -> Response {
    response.on_hover_ui(|ui| {
        ui.horizontal(|ui| {
            ui.label(name);
            if !key.is_empty() {
                kbd(ui, key);
            }
        });
    })
}

/// Frameless icon button, 28 × 28 (hover: discreet background).
pub fn icon_button(ui: &mut Ui, glyph: &str, tip: &str, enabled: bool) -> Response {
    let response = icon_cell(ui, glyph, tip, enabled, false, 28.0, 16.0);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, spoken_name(glyph, tip))
    });
    response
}

/// Icon button that stays highlighted while `on` (e.g. constrain proportions).
pub fn icon_toggle(ui: &mut Ui, glyph: &str, tip: &str, on: &mut bool) -> Response {
    let mut response = icon_cell(ui, glyph, tip, true, *on, 28.0, 16.0);
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    let on = *on;
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            true,
            on,
            spoken_name(glyph, tip),
        )
    });
    response
}

/// Name given to screen readers: the label, or the tooltip when the label
/// is only an icon (Phosphor glyphs are in the Private Use Area).
fn spoken_name<'a>(label: &'a str, tip: &'a str) -> &'a str {
    let glyph_only = label
        .chars()
        .all(|c| c.is_whitespace() || ('\u{e000}'..='\u{f8ff}').contains(&c));
    if glyph_only && !tip.is_empty() {
        tip
    } else {
        label
    }
}

/// Gives a tooltip that isn't the widget's name to screen readers too.
fn describe(ui: &Ui, response: &Response, name: &str, tip: &str) {
    if !tip.is_empty() && tip != name {
        ui.ctx()
            .accesskit_node_builder(response.id, |node| node.set_description(tip));
    }
}

fn icon_cell(
    ui: &mut Ui,
    glyph: &str,
    tip: &str,
    enabled: bool,
    on: bool,
    size: f32,
    glyph_size: f32,
) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let hovered = enabled && response.hovered();
    let (fill, fg) = if on {
        (color::ACCENT_SOFT, color::ACCENT_TEXT)
    } else if hovered {
        (color::BG3, color::TEXT)
    } else if enabled {
        (Color32::TRANSPARENT, color::MUTED)
    } else {
        (Color32::TRANSPARENT, color::MUTED.gamma_multiply(0.35))
    };
    ui.painter()
        .rect_filled(rect, if size > 30.0 { 8.0 } else { 6.0 }, fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(glyph_size),
        fg,
    );
    if tip.is_empty() {
        response
    } else {
        response.on_hover_text(tip)
    }
}

/// Tool bar button: 36 × 36 icon, tooltip with the tool's name and key.
pub fn tool_button(ui: &mut Ui, glyph: &str, name: &str, key: &str, selected: bool) -> Response {
    let response = icon_cell(ui, glyph, "", true, selected, 36.0, 18.0);
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    tip_with_key(response, name, key)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Normal,
    /// Main action: accent background.
    Primary,
    /// AI action: the icon in the accent's text color.
    Ai,
}

/// Button, 28 px high. `width`: fixed width (otherwise fitted).
pub fn button(ui: &mut Ui, label: &str, kind: Kind, width: Option<f32>, enabled: bool) -> Response {
    button_with_icon(ui, None, label, kind, width, enabled)
}

/// Button with an optional icon before the label.
pub fn button_with_icon(
    ui: &mut Ui,
    glyph: Option<&str>,
    label: &str,
    kind: Kind,
    width: Option<f32>,
    enabled: bool,
) -> Response {
    keyed_button(ui, glyph, label, "", kind, width, enabled)
}

/// Button whose shortcut `key` shows only in its tooltip, with the whole
/// label (the button may cut it).
pub fn keyed_button(
    ui: &mut Ui,
    glyph: Option<&str>,
    label: &str,
    key: &str,
    kind: Kind,
    width: Option<f32>,
    enabled: bool,
) -> Response {
    let icon_w = if glyph.is_some() { 23.0 } else { 0.0 };
    // A fixed width too narrow for the label cuts it with "…"; the
    // tooltip then shows it whole.
    let mut job = LayoutJob::single_section(
        label.to_owned(),
        TextFormat::simple(medium(TEXT_SIZE), Color32::PLACEHOLDER),
    );
    if let Some(w) = width {
        job.wrap = egui::text::TextWrapping::truncate_at_width((w - icon_w - 16.0).max(0.0));
    }
    let galley = ui.painter().layout_job(job);
    let w = width.unwrap_or(galley.size().x + icon_w + 24.0);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, mut response) = ui.allocate_exact_size(vec2(w, 28.0), sense);
    if galley.elided || !key.is_empty() {
        response = tip_with_key(response, label, key);
    }
    let (fill, border, fg) = button_colors(kind, enabled && response.hovered(), enabled);
    let painter = ui.painter();
    painter.rect(
        rect,
        6.0,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let content_w = galley.size().x + icon_w;
    let mut x = rect.center().x - content_w / 2.0;
    if let Some(glyph) = glyph {
        let icon_color = if kind == Kind::Ai && enabled {
            color::ACCENT_TEXT
        } else {
            fg
        };
        painter.text(
            pos2(x + 8.0, rect.center().y),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(16.0),
            icon_color,
        );
        x += icon_w;
    }
    painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
}

/// Fill, border and text colors of a button. Disabled, any kind is a flat
/// grey: a disabled main action must not look ready.
pub fn button_colors(kind: Kind, hovered: bool, enabled: bool) -> (Color32, Color32, Color32) {
    match kind {
        _ if !enabled => (color::BG3.gamma_multiply(0.6), color::LINE, color::DISABLED),
        Kind::Primary if hovered => (color::PRIMARY_HOVER, Color32::TRANSPARENT, color::ON_ACCENT),
        Kind::Primary => (color::ACCENT, Color32::TRANSPARENT, color::ON_ACCENT),
        _ if hovered => (color::BUTTON_HOVER, color::LINE, color::TEXT),
        Kind::Normal | Kind::Ai => (color::BG3, color::LINE, color::TEXT),
    }
}

/// Frame of an input: dark background, border lit when hovered or focused.
fn field_frame(painter: &Painter, rect: Rect, hovered: bool, focused: bool) {
    painter.add(field_shape(rect, hovered, focused));
}

fn field_shape(rect: Rect, hovered: bool, focused: bool) -> egui::Shape {
    let border = if focused {
        color::ACCENT
    } else if hovered {
        color::LINE_HOVER
    } else {
        color::LINE
    };
    egui::Shape::Rect(egui::epaint::RectShape::new(
        rect,
        6.0,
        color::BG0,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    ))
}

/// Numeric field with its label inside on the left ("X", "W", "Tolerance"…)
/// and the value on the right, in Geist Mono; drag or type to change it.
pub fn number_field<N: Numeric>(
    ui: &mut Ui,
    label: &str,
    value: &mut N,
    range: RangeInclusive<N>,
    suffix: &str,
    width: f32,
) -> Response {
    // A longer (translated) label widens the field rather than running
    // into the value.
    let label_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), medium(11.0), color::LABEL)
        .size()
        .x;
    let width = width.max(8.0 + label_w + 4.0 + 44.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
    let hovered = ui.rect_contains_pointer(rect);
    let painter = ui.painter().clone();
    // The frame goes under the value, but its state is known only after it.
    let frame = painter.add(egui::Shape::Noop);
    let label_rect = painter.text(
        pos2(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        medium(11.0),
        color::LABEL,
    );
    let value_rect = Rect::from_min_max(
        pos2(label_rect.right() + 4.0, rect.top() + 1.0),
        pos2(rect.right() - 4.0, rect.bottom() - 1.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(value_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    frameless(&mut child);
    let mut drag = egui::DragValue::new(value)
        .range(range)
        .suffix(suffix)
        .speed(0.5)
        .custom_parser(|text| i18n::parse_decimal(text.trim_end_matches(suffix)));
    if !N::INTEGRAL {
        // "48" rather than "48.0", but "48.5" stays.
        drag = drag.custom_formatter(|v, _| i18n::short_decimal(v));
    }
    let response = child.add(drag);
    // The painted label names the value for screen readers.
    ui.ctx()
        .accesskit_node_builder(response.id, |node| node.set_label(label));
    painter.set(frame, field_shape(rect, hovered, response.has_focus()));
    response
}

/// Single-line text field with its label inside on the left.
pub fn text_field(
    ui: &mut Ui,
    label: &str,
    text: &mut String,
    id: egui::Id,
) -> egui::text_edit::TextEditOutput {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
    let hovered = ui.rect_contains_pointer(rect);
    let painter = ui.painter().clone();
    let frame = painter.add(egui::Shape::Noop);
    let label_w = painter
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), color::LABEL)
        .size()
        .x;
    let edit_rect = Rect::from_min_max(
        pos2(rect.left() + 16.0 + label_w, rect.top()),
        pos2(rect.right() - 6.0, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(edit_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    frameless(&mut child);
    let output = egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .desired_width(edit_rect.width())
        .font(FontId::proportional(TEXT_SIZE))
        .show(&mut child);
    painter.set(
        frame,
        field_shape(rect, hovered, output.response.has_focus()),
    );
    painter.text(
        pos2(rect.left() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        color::LABEL,
    );
    output
}

/// Widgets without their own background or border, inside a field frame.
fn frameless(ui: &mut Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.bg_fill = Color32::TRANSPARENT;
        state.weak_bg_fill = Color32::TRANSPARENT;
        state.bg_stroke = Stroke::NONE;
    }
    ui.visuals_mut().text_edit_bg_color = Some(Color32::TRANSPARENT);
}

/// "Scrub" field: label on the left, value on the right, a 2 px accent bar
/// along the bottom showing where the value sits in `range`. Drag sideways
/// to change it; double-click to type it. Reached with Tab, it becomes its
/// text field, where ↑/↓ step the value (Shift: ×10).
pub fn scrub(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: RangeInclusive<f32>,
    value_text: &str,
    width: f32,
) -> Response {
    let (min, max) = (*range.start(), *range.end());
    let width = width.max(scrub_min_width(ui, label, value_text));
    let (rect, mut response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    let id = response.id.with("typing");
    let mut typing: Option<String> = ui.data(|d| d.get_temp(id));

    // Keyboard focus (Tab) goes straight to the text field: the app keeps
    // the keyboard for its shortcuts everywhere else.
    if response.double_clicked() || (response.has_focus() && typing.is_none()) {
        typing = Some(
            value_text
                .trim_end_matches(['%', ' ', '\u{a0}', 'p', 'x'])
                .to_owned(),
        );
        ui.memory_mut(|m| m.request_focus(id));
    }
    if let Some(text) = &mut typing {
        // Percent ranges (0..1) are shown and typed as 0..100.
        let scale = if max <= 1.0 { 100.0 } else { 1.0 };
        if ui.memory(|m| m.has_focus(id)) {
            let steps = ui.input_mut(|i| {
                let big = i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp)
                    as f32
                    - i.count_and_consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown) as f32;
                let small = i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                    as f32
                    - i.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) as f32;
                10.0 * big + small
            });
            if steps != 0.0 {
                let shown = i18n::parse_decimal(text).map_or(*value * scale, |v| v as f32);
                let new = ((shown + steps) / scale).clamp(min, max);
                *text = i18n::short_decimal(f64::from(new * scale));
                if new != *value {
                    *value = new;
                    response.mark_changed();
                }
            }
        }
        let edit_rect = rect.shrink2(vec2(8.0, 2.0));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(edit_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        frameless(&mut child);
        let out = egui::TextEdit::singleline(text)
            .id(id)
            .frame(egui::Frame::NONE)
            .horizontal_align(egui::Align::Max)
            .desired_width(edit_rect.width())
            .font(mono_medium(12.0))
            .show(&mut child);
        if out.response.lost_focus() {
            // Esc gives up the typed value, like Photoshop.
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if !escape && let Some(typed) = i18n::parse_decimal(text).map(|v| v as f32) {
                *value = (typed / scale).clamp(min, max);
                response.mark_changed();
            }
            typing = None;
        }
        field_frame(ui.painter(), rect, true, true);
    } else {
        if response.dragged() {
            let delta = response.drag_delta().x / rect.width() * (max - min);
            let new = (*value + delta).clamp(min, max);
            if new != *value {
                *value = new;
                response.mark_changed();
            }
        }
        let painter = ui.painter();
        field_frame(painter, rect, response.hovered(), response.dragged());
        painter.text(
            pos2(rect.left() + 10.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.0),
            color::LABEL,
        );
        painter.text(
            pos2(rect.right() - 10.0, rect.center().y),
            Align2::RIGHT_CENTER,
            value_text,
            mono_medium(12.0),
            color::TEXT,
        );
        let t = ((*value - min) / (max - min)).clamp(0.0, 1.0);
        let bar = Rect::from_min_max(
            pos2(rect.left() + 1.0, rect.bottom() - 3.0),
            pos2(
                rect.left() + 1.0 + t * (rect.width() - 2.0),
                rect.bottom() - 1.0,
            ),
        );
        painter.rect_filled(bar, 0.0, color::ACCENT);
    }
    ui.data_mut(|d| {
        if let Some(text) = typing {
            d.insert_temp(id, text);
        } else {
            d.remove::<String>(id);
        }
    });
    let current = f64::from(*value);
    response.widget_info(|| egui::WidgetInfo::slider(true, current, label));
    response.on_hover_cursor(CursorIcon::ResizeHorizontal)
}

/// Narrowest scrub field showing `label` and a value as long as
/// `value_text` (at least 5 characters, so the width doesn't follow the
/// value), with a gap between them.
fn scrub_min_width(ui: &Ui, label: &str, value_text: &str) -> f32 {
    let painter = ui.painter();
    let label_w = painter
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), color::LABEL)
        .size()
        .x;
    let digits = "0".repeat(value_text.chars().count().max(5));
    let value_w = painter
        .layout_no_wrap(digits, mono_medium(12.0), color::TEXT)
        .size()
        .x;
    10.0 + label_w + 12.0 + value_w + 10.0
}

/// Drop-down list: optional label in grey, then the chosen value.
pub fn combo<R>(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    label: Option<&str>,
    selected: &str,
    width: f32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let mut text = LayoutJob::default();
    if let Some(label) = label {
        text.append(
            label,
            0.0,
            TextFormat {
                font_id: FontId::proportional(TEXT_SIZE),
                color: color::LABEL,
                ..Default::default()
            },
        );
    }
    text.append(
        selected,
        if label.is_some() { 8.0 } else { 0.0 },
        TextFormat {
            font_id: FontId::proportional(TEXT_SIZE),
            color: color::TEXT,
            ..Default::default()
        },
    );
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(text)
        .width(width)
        .height(320.0)
        .show_ui(ui, add_contents)
}

/// Compact segmented control: `(value, label or glyph, tooltip)`.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str, &str)],
) -> bool {
    let font = FontId::proportional(TEXT_SIZE);
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, label, _)| {
            let w = ui
                .painter()
                .layout_no_wrap((*label).to_owned(), font.clone(), color::TEXT)
                .size()
                .x;
            (w + 12.0).max(26.0)
        })
        .collect();
    let total = widths.iter().sum::<f32>() + 2.0 * (options.len() as f32 - 1.0) + 6.0;
    let (rect, group) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    ui.painter().rect(
        rect,
        7.0,
        color::BG0,
        Stroke::new(1.0, color::LINE),
        StrokeKind::Inside,
    );
    let mut changed = false;
    let mut x = rect.left() + 3.0;
    for (i, ((option, label, tip), w)) in options.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(w, 22.0));
        x += w + 2.0;
        let mut response = ui.interact(r, group.id.with(i), Sense::click());
        let selected = *value == *option;
        let name = spoken_name(label, tip);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, name)
        });
        describe(ui, &response, name, tip);
        if !tip.is_empty() {
            response = response.on_hover_text(*tip);
        }
        let painter = ui.painter();
        if selected {
            painter.rect(
                r,
                5.0,
                color::BG3,
                Stroke::new(1.0, color::LINE_HOVER),
                StrokeKind::Inside,
            );
        }
        let fg = if selected || response.hovered() {
            color::TEXT
        } else {
            color::MUTED
        };
        painter.text(r.center(), Align2::CENTER_CENTER, *label, font.clone(), fg);
        if response.clicked() && !selected {
            *value = *option;
            changed = true;
        }
    }
    changed
}

/// Toggle buttons in a segmented frame (e.g. bold / italic): each one
/// switches its own boolean. `(value, label, font, tooltip)`.
pub fn toggles(ui: &mut Ui, options: &mut [(&mut bool, &str, FontId, &str)]) -> bool {
    let n = options.len();
    let total = 26.0 * n as f32 + 2.0 * (n as f32 - 1.0) + 6.0;
    let (rect, group) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    ui.painter().rect(
        rect,
        7.0,
        color::BG0,
        Stroke::new(1.0, color::LINE),
        StrokeKind::Inside,
    );
    let mut changed = false;
    for (i, (value, label, font, tip)) in options.iter_mut().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.left() + 3.0 + 28.0 * i as f32, rect.top() + 3.0),
            vec2(26.0, 22.0),
        );
        let response = ui.interact(r, group.id.with(i), Sense::click());
        let on = **value;
        // Their labels are single letters or glyphs: the tooltip names them.
        let name = if tip.is_empty() { *label } else { *tip };
        response
            .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name));
        let response = response.on_hover_text(*tip);
        if **value {
            ui.painter().rect(
                r,
                5.0,
                color::BG3,
                Stroke::new(1.0, color::LINE_HOVER),
                StrokeKind::Inside,
            );
        }
        let fg = if **value || response.hovered() {
            color::TEXT
        } else {
            color::MUTED
        };
        let galley = ui
            .painter()
            .layout_no_wrap((*label).to_owned(), font.clone(), fg);
        ui.painter()
            .galley(r.center() - galley.size() / 2.0, galley, fg);
        if response.clicked() {
            **value = !**value;
            changed = true;
        }
    }
    changed
}

/// Checkbox with label, clickable as a whole.
pub fn checkbox(ui: &mut Ui, value: &mut bool, label: &str) -> Response {
    let font = FontId::proportional(TEXT_SIZE);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, color::MUTED);
    let (rect, mut response) =
        ui.allocate_exact_size(vec2(21.0 + galley.size().x, 28.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let on = *value;
    response
        .widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, label));
    let boxed = Rect::from_center_size(pos2(rect.left() + 7.0, rect.center().y), Vec2::splat(14.0));
    paint_check(ui.painter(), boxed, *value, response.hovered());
    let fg = if response.hovered() {
        color::TEXT
    } else {
        color::MUTED
    };
    ui.painter().galley(
        pos2(rect.left() + 21.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    response
}

fn paint_check(painter: &Painter, r: Rect, on: bool, hovered: bool) {
    if on {
        painter.rect_filled(r, 3.0, color::ACCENT);
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            icon::CHECK,
            FontId::proportional(11.0),
            color::ON_ACCENT,
        );
    } else {
        let border = if hovered {
            color::MUTED
        } else {
            color::CONTROL_LINE
        };
        painter.rect(
            r,
            3.0,
            color::BG0,
            Stroke::new(1.0, border),
            StrokeKind::Inside,
        );
    }
}

/// On/off switch, 30 × 18.
pub fn switch(ui: &mut Ui, value: &mut bool, tip: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(30.0, 18.0), Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    let t = ui.ctx().animate_bool(response.id, *value);
    let track = egui::lerp(
        egui::Rgba::from(color::CONTROL_LINE)..=egui::Rgba::from(color::ACCENT),
        t,
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 9.0, Color32::from(track));
    let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
    painter.circle_filled(pos2(x, rect.center().y), 7.0, Color32::WHITE);
    let on = *value;
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, tip));
    response.on_hover_text(tip)
}

/// Color button: a click opens the Color Picker, whose color comes back
/// here (see [`picker_link`]).
pub fn swatch(ui: &mut Ui, color: &mut Color32, size: f32, tip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let border = if response.hovered() {
        color::MUTED
    } else {
        color::SWATCH_LINE
    };
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    ui.painter().rect(
        rect,
        6.0,
        Color32::from_rgb(r, g, b),
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ColorButton, true, tip));
    picker_link(ui.ctx(), response, color, i18n::t!("uip-picker-title")).on_hover_text(tip)
}

/// A color button asking the Color Picker window (drawn by the app) to
/// edit its color.
#[derive(Clone)]
pub struct PickerRequest {
    /// The button: the picked color is sent back to it.
    pub target: egui::Id,
    pub color: Color32,
    pub title: String,
}

fn picker_request_id() -> egui::Id {
    egui::Id::new("color_picker_request")
}

fn picked_id(target: egui::Id) -> egui::Id {
    target.with("picked")
}

/// Makes `response` a color button: a click asks for the Color Picker
/// (titled `title`), and the color it picks lands in `color` (alpha kept),
/// marking the response changed.
pub fn picker_link(
    ctx: &egui::Context,
    mut response: Response,
    color: &mut Color32,
    title: String,
) -> Response {
    if response.clicked() {
        let request = PickerRequest {
            target: response.id,
            color: *color,
            title,
        };
        request_picker(ctx, request);
    }
    if let Some(picked) = ctx.data_mut(|d| d.remove_temp::<Color32>(picked_id(response.id))) {
        let [r, g, b, _] = picked.to_srgba_unmultiplied();
        let new = Color32::from_rgba_unmultiplied(r, g, b, color.a());
        if new != *color {
            *color = new;
            response.mark_changed();
        }
    }
    response
}

/// Asks for the Color Picker.
pub fn request_picker(ctx: &egui::Context, request: PickerRequest) {
    ctx.data_mut(|d| d.insert_temp(picker_request_id(), request));
}

/// The Color Picker asked for since the last call, if any.
pub fn take_picker_request(ctx: &egui::Context) -> Option<PickerRequest> {
    ctx.data_mut(|d| {
        let request = d.get_temp(picker_request_id());
        d.remove::<PickerRequest>(picker_request_id());
        request
    })
}

/// Sends the picked color to the button `target` (it takes it on its next
/// frame).
pub fn send_picked(ctx: &egui::Context, target: egui::Id, color: Color32) {
    ctx.data_mut(|d| d.insert_temp(picked_id(target), color));
    ctx.request_repaint();
}

/// Small color square (in a "Fill" button).
pub fn color_chip(painter: &Painter, center: Pos2, color: Color32) {
    let r = Rect::from_center_size(center, Vec2::splat(12.0));
    painter.rect(
        r,
        3.0,
        color,
        Stroke::new(1.0, color::SWATCH_LINE),
        StrokeKind::Inside,
    );
}

/// "key value" row.
pub fn key_value(ui: &mut Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(44.0, 18.0), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            Align2::LEFT_CENTER,
            key,
            FontId::proportional(12.0),
            color::LABEL,
        );
        ui.label(egui::RichText::new(value).font(mono(12.0)));
    });
}

/// Dashed rectangle outline.
pub fn dashed_rect(painter: &Painter, r: Rect, stroke: Stroke, dash: f32, gap: f32) {
    let points = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.add(egui::Shape::dashed_line(&points, stroke, dash, gap));
}

/// Label on an accent pill (e.g. "W × H" under the transform frame).
pub fn pill(painter: &Painter, pos: Pos2, anchor: Align2, text: &str) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), mono(11.0), color::ON_ACCENT);
    let rect = anchor.anchor_size(pos, galley.size() + vec2(14.0, 4.0));
    painter.rect_filled(rect, 5.0, color::ACCENT);
    painter.galley(
        rect.center() - galley.size() / 2.0,
        galley,
        color::ON_ACCENT,
    );
    rect
}

#[cfg(test)]
mod tests {
    use egui::accesskit::Toggled;
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;

    /// The first frame installs the fonts the widgets use.
    fn with_fonts(ui: &mut Ui, installed: &mut bool) -> bool {
        if !*installed {
            install(ui.ctx());
            *installed = true;
            return false;
        }
        true
    }

    #[test]
    fn custom_widgets_have_accessible_names_and_states() {
        let mut fonts = false;
        let mut h = Harness::new_ui(move |ui| {
            if !with_fonts(ui, &mut fonts) {
                return;
            }
            button(ui, "Okay", Kind::Primary, None, true);
            icon_button(ui, icon::TRASH, "Delete Layer", true);
            let mut on = true;
            icon_toggle(ui, icon::LINK, "Constrain", &mut on);
            let mut checked = true;
            checkbox(ui, &mut checked, "Preview");
            let mut off = false;
            switch(ui, &mut off, "Visible");
            let mut v = 0.5f32;
            scrub(ui, "Opacity", &mut v, 0.0..=1.0, "50%", 120.0);
            let mut n = 10.0f32;
            number_field(ui, "Width", &mut n, 0.0..=100.0, " px", 120.0);
            let mut c = Color32::RED;
            swatch(ui, &mut c, 20.0, "Shadow color");
            let mut seg = 0;
            segmented(
                ui,
                &mut seg,
                &[(0, "Pixels", "In pixels"), (1, icon::PERCENT, "Percent")],
            );
            let (mut b, mut i) = (true, false);
            toggles(
                ui,
                &mut [
                    (&mut b, "B", bold(13.0), "Bold"),
                    (&mut i, "I", medium(13.0), "Italic"),
                ],
            );
        });
        h.run();
        for name in [
            "Okay",
            "Delete Layer",
            "Opacity",
            "Width",
            "Shadow color",
            "Percent",
        ] {
            h.get_by_label(name);
        }
        let toggled = |h: &Harness, name: &str| h.get_by_label(name).accesskit_node().toggled();
        assert_eq!(toggled(&h, "Constrain"), Some(Toggled::True));
        assert_eq!(toggled(&h, "Preview"), Some(Toggled::True));
        assert_eq!(toggled(&h, "Visible"), Some(Toggled::False));
        assert_eq!(toggled(&h, "Bold"), Some(Toggled::True));
        assert_eq!(toggled(&h, "Italic"), Some(Toggled::False));
        let pixels = h.get_by_label("Pixels");
        assert_eq!(pixels.accesskit_node().toggled(), Some(Toggled::True));
        assert_eq!(
            pixels.accesskit_node().description().as_deref(),
            Some("In pixels")
        );
    }

    #[test]
    fn scrub_reached_with_tab_steps_with_the_arrows() {
        let mut fonts = false;
        let mut h = Harness::new_ui_state(
            move |ui, v: &mut f32| {
                if !with_fonts(ui, &mut fonts) {
                    return;
                }
                scrub(
                    ui,
                    "Opacity",
                    v,
                    0.0..=1.0,
                    &format!("{:.0}%", *v * 100.0),
                    120.0,
                );
            },
            0.5f32,
        );
        h.run();
        h.key_press(egui::Key::Tab);
        h.run();
        h.key_press(egui::Key::ArrowUp);
        h.run();
        assert!((h.state() - 0.51).abs() < 1e-4, "{}", h.state());
        h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowDown);
        h.run();
        assert!((h.state() - 0.41).abs() < 1e-4, "{}", h.state());
        // Esc leaves the field: the stepped value stays.
        h.key_press(egui::Key::Escape);
        h.run();
        assert!((h.state() - 0.41).abs() < 1e-4, "{}", h.state());
    }
}
