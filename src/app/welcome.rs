//! Start screen, in the canvas area while no image is open: drop zone with
//! Open and Paste, then the recent files.

use super::keys;
use super::*;
use crate::i18n::t;

/// Height of a recent file row, and the gap between rows.
const ROW: f32 = 50.0;
const ROW_GAP: f32 = 4.0;

impl AtelierApp {
    pub(super) fn welcome(&mut self, ui: &mut egui::Ui, area: Rect) {
        use ui_kit::color;
        let ctx = ui.ctx().clone();
        let recent: Vec<(PathBuf, (u32, u32), Option<egui::TextureId>)> = self
            .recent
            .entries(&ctx)
            .iter()
            .map(|e| (e.path.clone(), e.size, e.thumb.as_ref().map(|t| t.id())))
            .collect();
        let mut action = None;
        let mut open_path = None;
        let mut forget_path = None;
        // Scrolls when the window is too small for it all.
        let mut scroll = ui.new_child(egui::UiBuilder::new().max_rect(area));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut scroll, |ui| {
                let painter = ui.painter().clone();
                let width = 520.0_f32.min(ui.available_width() - 32.0);

                // Texts of the drop zone, centered.
                let centered = |text: String, font: egui::FontId, color: Color32| {
                    let mut job = egui::text::LayoutJob::simple(text, font, color, width - 64.0);
                    job.halign = egui::Align::Center;
                    painter.layout_job(job)
                };
                let title = centered(t!("welcome-title"), ui_kit::bold(22.0), color::TEXT);
                let subtitle = centered(
                    t!("welcome-subtitle"),
                    egui::FontId::proportional(ui_kit::TEXT_SIZE),
                    color::MUTED,
                );
                let recent_height = if recent.is_empty() {
                    0.0
                } else {
                    28.0 + 20.0 + recent.len() as f32 * (ROW + ROW_GAP)
                };
                let open = t!("menu-file-open");
                let paste = t!("menu-edit-paste");
                let open_key = keys::shortcut(&ctx, &SHORTCUT_OPEN);
                let paste_key = keys::shortcut(&ctx, &SHORTCUT_PASTE);
                let (open_w, paste_w) = (
                    big_button_width(&painter, &open, &open_key),
                    big_button_width(&painter, &paste, &paste_key),
                );
                // Side by side, or one above the other in a narrow window.
                let stacked = open_w + 8.0 + paste_w > width - 24.0;
                let buttons = if stacked { 36.0 + 8.0 + 36.0 } else { 36.0 };
                let texts = title.size().y + 14.0 + subtitle.size().y + 20.0 + buttons;
                // A small window: the card loses its icon and some padding.
                let roomy =
                    44.0 + 52.0 + 14.0 + texts + 44.0 + recent_height + 32.0 <= area.height();
                let (padding, tile) = if roomy {
                    (44.0, 52.0 + 14.0)
                } else {
                    (24.0, 0.0)
                };
                let card_height = padding + tile + texts + padding;
                let total = card_height + recent_height;
                let (content, _) = ui.allocate_exact_size(
                    vec2(ui.available_width(), (total + 32.0).max(area.height())),
                    Sense::hover(),
                );
                let center_x = content.center().x;
                let top = (content.center().y - total / 2.0).max(content.top() + 16.0);

                // Drop zone: dashed card, lit while files are dragged over the window.
                let card = Rect::from_min_size(
                    pos2(center_x - width / 2.0, top),
                    vec2(width, card_height),
                );
                let dragging = ctx.input(|i| !i.raw.hovered_files.is_empty());
                painter.rect_filled(card, 16.0, color::BG1.gamma_multiply(0.7));
                let border = if dragging {
                    color::ACCENT
                } else {
                    color::LINE_HOVER
                };
                painter.add(egui::Shape::dashed_line(
                    &rounded_outline(card, 16.0),
                    UiStroke::new(1.5, border),
                    5.0,
                    4.0,
                ));

                let mut y = card.top() + padding;
                if roomy {
                    let tile = Rect::from_min_size(pos2(center_x - 26.0, y), Vec2::splat(52.0));
                    painter.rect_filled(tile, 14.0, color::BG3);
                    painter.text(
                        tile.center(),
                        egui::Align2::CENTER_CENTER,
                        icon::IMAGE,
                        egui::FontId::proportional(26.0),
                        color::ACCENT_TEXT,
                    );
                    y = tile.bottom() + 14.0;
                }
                let title_height = title.size().y;
                painter.galley(pos2(center_x, y), title, color::TEXT);
                y += title_height + 14.0;
                let subtitle_height = subtitle.size().y;
                painter.galley(pos2(center_x, y), subtitle, color::MUTED);
                y += subtitle_height + 20.0;

                let (open_rect, paste_rect) = if stacked {
                    let open = Rect::from_center_size(pos2(center_x, y + 18.0), vec2(open_w, 36.0));
                    let paste = Rect::from_center_size(
                        pos2(center_x, open.bottom() + 8.0 + 18.0),
                        vec2(paste_w, 36.0),
                    );
                    (open, paste)
                } else {
                    let left = center_x - (open_w + 8.0 + paste_w) / 2.0;
                    let open = Rect::from_min_size(pos2(left, y), vec2(open_w, 36.0));
                    let paste =
                        Rect::from_min_size(pos2(open.right() + 8.0, y), vec2(paste_w, 36.0));
                    (open, paste)
                };
                if big_button(ui, open_rect, &open, &open_key, true).clicked() {
                    action = Some(Action::Open);
                }
                if big_button(ui, paste_rect, &paste, &paste_key, false).clicked() {
                    action = Some(Action::Paste);
                }

                // Recent files.
                if recent.is_empty() {
                    return;
                }
                let heading = Rect::from_min_size(
                    pos2(card.left() + 10.0, card.bottom() + 28.0),
                    vec2(width - 10.0, 14.0),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(heading)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| ui_kit::section(ui, &t!("welcome-recent")),
                );
                let mut y = heading.bottom() + 6.0;
                for (path, (w, h), thumb) in &recent {
                    let row = Rect::from_min_size(pos2(card.left(), y), vec2(width, ROW));
                    y += ROW + ROW_GAP;
                    let response = ui
                        .interact(row, ui.id().with(("welcome-recent", path)), Sense::click())
                        .on_hover_text(path.display().to_string());
                    if response.hovered() {
                        painter.rect_filled(row, 8.0, color::BG3);
                    }
                    let mid = row.center().y;
                    // Remove from the list: a cross on the hovered row.
                    let cross =
                        Rect::from_center_size(pos2(row.right() - 22.0, mid), Vec2::splat(24.0));
                    let mut forget = false;
                    if ui.rect_contains_pointer(row) {
                        let remove = ui.interact(
                            cross,
                            ui.id().with(("welcome-forget", path)),
                            Sense::click(),
                        );
                        if remove.hovered() {
                            painter.rect_filled(cross, 6.0, color::BUTTON_HOVER);
                        }
                        let fg = if remove.hovered() {
                            color::TEXT
                        } else {
                            color::LABEL
                        };
                        painter.text(
                            cross.center(),
                            egui::Align2::CENTER_CENTER,
                            icon::X,
                            egui::FontId::proportional(14.0),
                            fg,
                        );
                        forget = remove.on_hover_text(t!("welcome-forget-recent")).clicked();
                    }
                    if forget {
                        forget_path = Some(path.clone());
                    } else if response.clicked() {
                        open_path = Some(path.clone());
                    }
                    let frame =
                        Rect::from_min_size(pos2(row.left() + 10.0, mid - 17.0), vec2(54.0, 34.0));
                    match thumb {
                        Some(id) => {
                            painter.rect_filled(frame, 5.0, color::BG0);
                            let (iw, ih) = (*w as f32, *h as f32);
                            let fit = (frame.width() / iw).min(frame.height() / ih);
                            let image = Rect::from_center_size(frame.center(), vec2(iw, ih) * fit);
                            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                            painter.image(*id, image, uv, Color32::WHITE);
                        }
                        None => {
                            painter.rect_filled(frame, 5.0, color::THUMB_EMPTY);
                        }
                    }
                    painter.rect_stroke(
                        frame,
                        5.0,
                        UiStroke::new(1.0, color::LINE_HOVER),
                        StrokeKind::Inside,
                    );

                    let dims = painter.layout_no_wrap(
                        format!("{w} × {h}"),
                        ui_kit::mono(12.0),
                        color::LABEL,
                    );
                    // Room for the cross on the right.
                    let dims_pos = pos2(
                        row.right() - 44.0 - dims.size().x,
                        mid - dims.size().y / 2.0,
                    );
                    let text_left = frame.right() + 12.0;
                    let text_width = (dims_pos.x - 12.0 - text_left).max(0.0);
                    let line = |text: String, font: egui::FontId, color: Color32| {
                        let mut job = egui::text::LayoutJob::single_section(
                            text,
                            egui::TextFormat::simple(font, color),
                        );
                        job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
                        painter.layout_job(job)
                    };
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let folder = path
                        .parent()
                        .and_then(Path::file_name)
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| {
                            path.parent()
                                .map(|p| p.display().to_string())
                                .unwrap_or_default()
                        });
                    let name = line(name, ui_kit::medium(ui_kit::TEXT_SIZE), color::TEXT);
                    let folder = line(folder, egui::FontId::proportional(12.0), color::LABEL);
                    let text_top = mid - (name.size().y + folder.size().y) / 2.0;
                    let name_height = name.size().y;
                    painter.galley(pos2(text_left, text_top), name, color::TEXT);
                    painter.galley(
                        pos2(text_left, text_top + name_height),
                        folder,
                        color::LABEL,
                    );
                    painter.galley(dims_pos, dims, color::LABEL);
                }
            });

        if let Some(path) = forget_path {
            self.recent.forget(&path);
        }
        if let Some(path) = open_path {
            self.open_path(&path);
            if self.doc.is_none() {
                self.recent.remove(&path);
            }
        }
        if let Some(action) = action {
            self.run(action);
        }
    }
}

/// Width of a start screen button: label and its key.
fn big_button_width(painter: &Painter, label: &str, key: &str) -> f32 {
    let label = painter.layout_no_wrap(
        label.to_owned(),
        ui_kit::medium(ui_kit::TEXT_SIZE),
        Color32::WHITE,
    );
    let key = painter.layout_no_wrap(key.to_owned(), ui_kit::mono(11.0), Color32::WHITE);
    16.0 + label.size().x + 7.0 + key.size().x + 10.0 + 16.0
}

/// Start screen button, 36 px high, with its key on the right of the label.
fn big_button(
    ui: &mut egui::Ui,
    rect: Rect,
    label: &str,
    key: &str,
    primary: bool,
) -> egui::Response {
    use ui_kit::color;
    let response = ui.interact(
        rect,
        ui.id().with(("welcome-button", label)),
        Sense::click(),
    );
    let hovered = response.hovered();
    let painter = ui.painter();
    let kind = if primary { Kind::Primary } else { Kind::Normal };
    let (fill, border, fg) = ui_kit::button_colors(kind, hovered, true);
    painter.rect(
        rect,
        6.0,
        fill,
        UiStroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let label = painter.layout_no_wrap(label.to_owned(), ui_kit::medium(ui_kit::TEXT_SIZE), fg);
    let (key_fill, key_border, key_fg) = if primary {
        (
            Color32::from_black_alpha(64),
            Color32::from_white_alpha(64),
            Color32::WHITE,
        )
    } else {
        (color::BG1, color::KBD_LINE, color::MUTED)
    };
    let key = painter.layout_no_wrap(key.to_owned(), ui_kit::mono(11.0), key_fg);
    let mid = rect.center().y;
    let x = rect.left() + 16.0;
    let label_width = label.size().x;
    painter.galley(pos2(x, mid - label.size().y / 2.0), label, fg);
    let key_rect = Rect::from_min_size(
        pos2(x + label_width + 7.0, mid - 9.0),
        vec2(key.size().x + 10.0, 18.0),
    );
    painter.rect(
        key_rect,
        4.0,
        key_fill,
        UiStroke::new(1.0, key_border),
        StrokeKind::Inside,
    );
    painter.galley(key_rect.center() - key.size() / 2.0, key, key_fg);
    response
}

/// Closed outline of a rounded rectangle, for a dashed border.
fn rounded_outline(r: Rect, radius: f32) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let r = r.shrink(0.75);
    let corners = [
        (pos2(r.right() - radius, r.top() + radius), -FRAC_PI_2),
        (pos2(r.right() - radius, r.bottom() - radius), 0.0),
        (pos2(r.left() + radius, r.bottom() - radius), FRAC_PI_2),
        (pos2(r.left() + radius, r.top() + radius), PI),
    ];
    let mut points: Vec<Pos2> = corners
        .iter()
        .flat_map(|&(c, start)| {
            (0..=8).map(move |i| {
                let a = start + i as f32 / 8.0 * FRAC_PI_2;
                c + vec2(a.cos(), a.sin()) * radius
            })
        })
        .collect();
    points.push(points[0]);
    points
}
