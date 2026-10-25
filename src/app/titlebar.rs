//! Title bar: logo, menus, command search, undo/redo and window buttons.
//! The window has no system frame: the title bar moves it and its borders
//! resize it (`resize_grips`). On macOS the system's window buttons and
//! borders stay: room is left for them, ours are not drawn.

use egui::viewport::ResizeDirection;
use egui::{Align, Align2, FontId, Layout, ViewportCommand};

use super::keys;
use super::menus::{explained_menu_entries, scrolling_menu};
use super::palette::SHORTCUT_PALETTE;
use super::*;
use crate::i18n::t;

/// Window buttons, as wide as Windows'.
const WINDOW_BUTTON_WIDTH: f32 = 46.0;
/// Width of the window border that resizes it.
const RESIZE_BORDER: f32 = 5.0;
/// Length of the corners' grip along each border.
const RESIZE_CORNER: f32 = 12.0;
/// macOS keeps its own window buttons (top left) and borders.
const SYSTEM_BUTTONS: bool = cfg!(target_os = "macos");
/// Room for macOS's three window buttons.
const SYSTEM_BUTTONS_WIDTH: f32 = 72.0;

impl AtelierApp {
    pub(super) fn title_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        // Empty space moves the window; double-click maximizes. Registered
        // first so that the widgets on top of it take precedence.
        let bar = ui.max_rect();
        let drag_rect = Rect::from_x_y_ranges(ui.clip_rect().x_range(), bar.y_range());
        let background = ui.interact(drag_rect, ui.id().with("drag"), Sense::click_and_drag());
        if background.double_clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        } else if background.drag_started_by(PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }

        let menus = self.menus();
        let mut action = None;
        let facts = self.greyed_facts();
        let why = move |a: Action| Some(super::palette::unavailable_reason(&facts, a));
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if SYSTEM_BUTTONS {
                ui.add_space(SYSTEM_BUTTONS_WIDTH);
            }
            logo(ui);
            ui.add_space(8.0);
            ui.scope(|ui| {
                menu_bar_style(ui.style_mut());
                ui.spacing_mut().item_spacing.x = 1.0;
                // A narrow window brings the menus closer together, clear
                // of the window buttons.
                let font = egui::TextStyle::Button.resolve(ui.style());
                let text: f32 = menus
                    .iter()
                    .map(|(title, _)| {
                        let galley = ui.painter().layout_no_wrap(
                            title.clone(),
                            font.clone(),
                            Color32::PLACEHOLDER,
                        );
                        galley.size().x + 1.0
                    })
                    .sum();
                let buttons = if SYSTEM_BUTTONS {
                    10.0
                } else {
                    3.0 * WINDOW_BUTTON_WIDTH + 10.0
                };
                let room = ui.available_width() - buttons - text;
                let padding = room / (2.0 * menus.len() as f32);
                ui.spacing_mut().button_padding.x = padding.clamp(4.0, 9.0);
                for (title, entries) in &menus {
                    let config = egui::containers::menu::MenuConfig::new().style(menu_style);
                    egui::containers::menu::MenuButton::new(title.as_str())
                        .config(config)
                        .ui(ui, |ui| {
                            ui.set_min_width(200.0);
                            // A menu taller than the window scrolls, rather
                            // than covering the menu bar.
                            // (It opens under the title bar, 40 px high.)
                            let below = ui.ctx().content_rect().height() - 40.0;
                            scrolling_menu(ui, title, below - 24.0, |ui| {
                                explained_menu_entries(ui, entries, &mut action, &why);
                            });
                        });
                }
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if SYSTEM_BUTTONS {
                    ui.add_space(10.0);
                } else {
                    if window_button(ui, WindowButton::Close, &t!("title-close")).clicked() {
                        // Through the usual close flow (unsaved changes).
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    let (kind, tip) = if maximized {
                        (WindowButton::Restore, t!("title-restore"))
                    } else {
                        (WindowButton::Maximize, t!("title-maximize"))
                    };
                    if window_button(ui, kind, &tip).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                    }
                    if window_button(ui, WindowButton::Minimize, &t!("title-minimize")).clicked() {
                        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                    }
                    ui.add_space(10.0);
                }

                // When the window is narrow, the search field shrinks to its
                // icon, then goes; then undo and redo go (they are in the
                // Edit menu): the menus never get covered.
                ui.spacing_mut().item_spacing.x = 2.0;
                if ui.available_width() >= 2.0 * 28.0 + 2.0 + 16.0 {
                    let can_redo = self.doc.as_ref().is_some_and(Document::can_redo);
                    let redo = ui_kit::icon_button(ui, icon::ARROW_U_UP_RIGHT, "", can_redo);
                    let redo_key = keys::shortcut(&ctx, &SHORTCUT_REDO);
                    if ui_kit::tip_with_key(redo, &t!("menu-edit-redo"), &redo_key).clicked() {
                        action = Some(Action::Redo);
                    }
                    let can_undo = self.doc.as_ref().is_some_and(Document::can_undo);
                    let undo = ui_kit::icon_button(ui, icon::ARROW_U_UP_LEFT, "", can_undo);
                    let undo_key = keys::shortcut(&ctx, &SHORTCUT_UNDO);
                    if ui_kit::tip_with_key(undo, &t!("menu-edit-undo"), &undo_key).clicked() {
                        action = Some(Action::Undo);
                    }
                }

                // Search, centered in the space left between menus and buttons.
                let free = ui.available_rect_before_wrap().shrink2(vec2(16.0, 0.0));
                if search_button(ui, free).is_some_and(|r| r.clicked()) {
                    self.palette.open();
                }
            });
        });
        if let Some(action) = action {
            self.run(action);
        }
    }
}

/// The app icon, 22 px; loaded once into a texture.
fn logo(ui: &mut egui::Ui) {
    let id = egui::Id::new("title_logo");
    let texture = ui.ctx().data_mut(|d| d.get_temp::<TextureHandle>(id));
    let texture = texture.unwrap_or_else(|| {
        let texture = load_logo(ui.ctx());
        ui.ctx().data_mut(|d| d.insert_temp(id, texture.clone()));
        texture
    });
    ui.add(
        egui::Image::new(&texture)
            .fit_to_exact_size(Vec2::splat(22.0))
            .corner_radius(6),
    );
}

/// assets/icon.png without its transparent margin, scaled down once
/// (sharper than leaving it to the GPU).
fn load_logo(ctx: &egui::Context) -> TextureHandle {
    let icon = image::load_from_memory(include_bytes!("../../assets/icon.png"))
        .expect("valid icon PNG")
        .into_rgba8();
    let (mut x0, mut y0, mut x1, mut y1) = (icon.width(), icon.height(), 0, 0);
    for (x, y, p) in icon.enumerate_pixels() {
        if p[3] > 128 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    let square = image::imageops::crop_imm(&icon, x0, y0, x1 + 1 - x0, y1 + 1 - y0).to_image();
    let size = 66;
    let small = image::imageops::resize(&square, size, size, FilterType::Lanczos3);
    ctx.load_texture(
        "title_logo",
        ColorImage::from_rgba_unmultiplied([size as usize; 2], small.as_raw()),
        TextureOptions::LINEAR,
    )
}

/// Menu titles: frameless, muted text that lights up on hover.
fn menu_bar_style(style: &mut egui::Style) {
    use ui_kit::color;
    style.spacing.button_padding = vec2(9.0, 0.0);
    let w = &mut style.visuals.widgets;
    for (state, fill, text) in [
        (&mut w.inactive, Color32::TRANSPARENT, color::MUTED),
        (&mut w.hovered, color::BG3, color::TEXT),
        (&mut w.active, color::BG3, color::TEXT),
        (&mut w.open, color::BG3, color::TEXT),
    ] {
        state.weak_bg_fill = fill;
        state.bg_stroke = UiStroke::NONE;
        state.fg_stroke.color = text;
    }
}

/// Menu popups and their submenus.
fn menu_style(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = vec2(8.0, 3.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.item_spacing.y = 1.0;
    style.visuals.widgets.hovered.weak_bg_fill = ui_kit::color::BG3;
    style.visuals.widgets.active.weak_bg_fill = ui_kit::color::BG3;
    style.visuals.widgets.open.weak_bg_fill = ui_kit::color::BG3;
}

/// "Search commands, tools…" field that opens the palette, centered in
/// `free`: up to 380 px wide; only its icon and key when the placeholder
/// doesn't fit whole; nothing when even that doesn't fit.
fn search_button(ui: &mut egui::Ui, free: Rect) -> Option<egui::Response> {
    use ui_kit::color;
    let painter = ui.painter().clone();
    let key = painter.layout_no_wrap(
        keys::shortcut(ui.ctx(), &SHORTCUT_PALETTE),
        ui_kit::mono(11.0),
        color::MUTED,
    );
    let placeholder = painter.layout_no_wrap(
        t!("palette-placeholder"),
        FontId::proportional(13.0),
        color::LABEL,
    );
    // Icon, then the key chip at the right end.
    let compact = 10.0 + 16.0 + 10.0 + key.size().x + 10.0 + 6.0;
    let full = compact + placeholder.size().x + 10.0;
    let width = if free.width() >= full {
        free.width().min(380.0)
    } else if free.width() >= compact {
        compact
    } else {
        return None;
    };
    let rect = Rect::from_center_size(free.center(), vec2(width, 28.0));
    let response = ui.interact(rect, ui.id().with("search"), Sense::click());
    let border = if response.hovered() {
        color::LINE_HOVER
    } else {
        color::LINE
    };
    painter.rect(
        rect,
        7.0,
        color::OPTIONS,
        UiStroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let y = rect.center().y;
    painter.text(
        pos2(rect.left() + 10.0, y),
        Align2::LEFT_CENTER,
        icon::MAGNIFYING_GLASS,
        FontId::proportional(16.0),
        color::LABEL,
    );
    let key_rect = Rect::from_min_max(
        pos2(rect.right() - 6.0 - key.size().x - 10.0, y - 9.0),
        pos2(rect.right() - 6.0, y + 9.0),
    );
    painter.rect(
        key_rect,
        4.0,
        color::BG1,
        UiStroke::new(1.0, color::KBD_LINE),
        StrokeKind::Inside,
    );
    painter.galley(key_rect.center() - key.size() / 2.0, key, color::MUTED);
    if width >= full {
        let left = rect.left() + 10.0 + 16.0 + 10.0;
        painter.galley(
            pos2(left, y - placeholder.size().y / 2.0),
            placeholder,
            color::LABEL,
        );
    }
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    // A button that opens the palette, named as the field it looks like.
    let name = t!("palette-placeholder");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    Some(if width < full {
        response.on_hover_text(t!("palette-placeholder"))
    } else {
        response
    })
}

#[derive(Clone, Copy, PartialEq)]
enum WindowButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Window button, full title bar height, its glyph drawn in a 12 px box.
fn window_button(ui: &mut egui::Ui, kind: WindowButton, tip: &str) -> egui::Response {
    use ui_kit::color;
    let size = vec2(WINDOW_BUTTON_WIDTH, ui.available_height());
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let (fill, fg) = match (response.hovered(), kind) {
        (true, WindowButton::Close) => (color::CLOSE_HOVER, Color32::WHITE),
        (true, _) => (color::BG3, color::TEXT),
        (false, _) => (Color32::TRANSPARENT, color::MUTED),
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, fill);
    let stroke = UiStroke::new(1.2, fg);
    // Glyph coordinates in a 12 × 12 box centered in the button.
    let o = rect.center() - vec2(6.0, 6.0);
    let p = |x: f32, y: f32| o + vec2(x, y);
    match kind {
        WindowButton::Minimize => {
            painter.line_segment([p(1.5, 6.0), p(10.5, 6.0)], stroke);
        }
        WindowButton::Maximize => {
            painter.rect_stroke(
                Rect::from_min_max(p(1.5, 1.5), p(10.5, 10.5)),
                1.0,
                stroke,
                StrokeKind::Middle,
            );
        }
        WindowButton::Restore => {
            painter.rect_stroke(
                Rect::from_min_max(p(1.5, 3.5), p(8.5, 10.5)),
                1.0,
                stroke,
                StrokeKind::Middle,
            );
            painter.add(egui::Shape::line(
                vec![
                    p(3.5, 3.5),
                    p(3.5, 1.5),
                    p(10.5, 1.5),
                    p(10.5, 8.5),
                    p(8.5, 8.5),
                ],
                stroke,
            ));
        }
        WindowButton::Close => {
            painter.line_segment([p(2.0, 2.0), p(10.0, 10.0)], stroke);
            painter.line_segment([p(10.0, 2.0), p(2.0, 10.0)], stroke);
        }
    }
    // Painted: named by its tooltip for screen readers.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
    response.on_hover_text(tip)
}

/// Invisible strips along the window borders that resize it (the window
/// has no system frame). Nothing to do when maximized or full screen.
pub(super) fn resize_grips(ctx: &egui::Context) {
    let fixed = ctx.input(|i| {
        let v = i.viewport();
        v.maximized.unwrap_or(false) || v.fullscreen.unwrap_or(false)
    });
    if fixed || SYSTEM_BUTTONS {
        return;
    }
    let r = ctx.content_rect();
    let b = RESIZE_BORDER;
    let strips = [
        ("n", Rect::from_min_max(r.min, pos2(r.max.x, r.min.y + b))),
        ("s", Rect::from_min_max(pos2(r.min.x, r.max.y - b), r.max)),
        ("w", Rect::from_min_max(r.min, pos2(r.min.x + b, r.max.y))),
        ("e", Rect::from_min_max(pos2(r.max.x - b, r.min.y), r.max)),
    ];
    for (name, strip) in strips {
        egui::Area::new(egui::Id::new("resize_grip").with(name))
            .order(egui::Order::Foreground)
            .fixed_pos(strip.min)
            .show(ctx, |ui| {
                let (_, response) = ui.allocate_exact_size(strip.size(), Sense::drag());
                let Some(pos) = response.hover_pos() else {
                    return;
                };
                let direction = resize_direction(r, pos);
                ctx.set_cursor_icon(resize_cursor(direction));
                if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed())
                {
                    ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
                }
            });
    }
}

/// Which border or corner of `window` is under `pos`.
fn resize_direction(window: Rect, pos: Pos2) -> ResizeDirection {
    let c = RESIZE_CORNER;
    let west = pos.x < window.left() + c;
    let east = pos.x > window.right() - c;
    let north = pos.y < window.top() + c;
    let south = pos.y > window.bottom() - c;
    match (north, south, west, east) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (true, _, _, true) => ResizeDirection::NorthEast,
        (_, true, true, _) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::North,
        (_, true, ..) => ResizeDirection::South,
        (_, _, true, _) => ResizeDirection::West,
        _ => ResizeDirection::East,
    }
}

fn resize_cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}
