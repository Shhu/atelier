//! Channels panel (Window › Channels), like Photoshop's: the RGB composite
//! and its red, green and blue channels (one shown alone, in gray), then
//! the saved selections (alpha channels): shown alone or as a red overlay,
//! loaded as the selection (Ctrl + click), renamed (double-click),
//! duplicated, deleted; New Channel and Save Selection as Channel.

use image::{GrayImage, RgbaImage};

use super::dialogs::{DialogSpec, floating_dialog};
use super::select_menu::{OverlayMask, OverlaySource, OverlayStyle};
use super::*;
use egui::{Align2, FontId, Response};

/// Row of the list.
const ROW: f32 = 40.0;
/// Largest side of the thumbnails.
const THUMB: u32 = 64;

/// What the canvas shows of the channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ChannelView {
    /// The image (RGB).
    #[default]
    Composite,
    /// Red, green or blue alone, in gray.
    Color(usize),
    /// An alpha channel alone, in gray.
    Alpha(usize),
}

#[derive(Default)]
pub(super) struct ChannelsPanel {
    pub open: bool,
    /// Of the document `doc` (uid): the view, and an alpha channel shown
    /// over it in red (its eye).
    doc: u64,
    view: ChannelView,
    overlay: Option<usize>,
    /// Alpha channel selected in the list (the buttons act on it).
    selected: Option<usize>,
    /// Alpha channel being renamed, and its new name.
    rename: Option<(usize, String)>,
    /// Thumbnails for a state of the document (fingerprint), and when they
    /// were made: RGB, R, G, B, then the alphas.
    thumbs: Option<(u64, f64, Vec<TextureHandle>)>,
    /// The layers reduced for the thumbnails, kept between them (only
    /// what changed is reduced again): document (uid), and them.
    reduced: Option<(u64, crate::compose::LevelCache)>,
}

/// Thumbnails are made again at most this often (seconds) while the image
/// keeps changing (brush stroke, slider).
const THUMBS_EVERY: f64 = 0.25;

/// What a click in the panel asks for.
enum Click {
    /// A row: the composite (`None`), a color or an alpha channel.
    Row(Option<ChannelRow>, Modifiers),
    Eye(Option<ChannelRow>),
    Rename(usize),
    Renamed(usize, String),
    New,
    Delete,
    Duplicate(usize),
    LoadSelection,
    SaveSelection,
}

#[derive(Clone, Copy, PartialEq)]
enum ChannelRow {
    Color(usize),
    Alpha(usize),
}

impl ChannelsPanel {
    /// The view of document `doc`, dropping what no longer exists in it
    /// (another tab, channels gone with an undo).
    fn sync(&mut self, doc: &Document) {
        if self.doc != doc.uid() {
            *self = Self {
                open: self.open,
                doc: doc.uid(),
                ..Default::default()
            };
        }
        let n = doc.channels.len();
        if matches!(self.view, ChannelView::Alpha(i) if i >= n) {
            self.view = ChannelView::Composite;
        }
        self.overlay = self.overlay.filter(|&i| i < n);
        self.selected = self.selected.filter(|&i| i < n);
        self.rename = self.rename.take().filter(|r| r.0 < n);
    }

    /// The color channel the canvas shows alone, if any.
    pub(super) fn color_shown(&self, doc: &Document) -> Option<usize> {
        match self.view {
            ChannelView::Color(c) if self.doc == doc.uid() => Some(c),
            _ => None,
        }
    }
}

impl AtelierApp {
    /// Ctrl+2 (composite), Ctrl+3, 4, 5 (red, green, blue), Ctrl+6… (the
    /// alpha channels): what the canvas shows, like Photoshop.
    pub(super) fn show_channel(&mut self, n: usize) {
        let Some(doc) = &self.doc else { return };
        self.channels.sync(doc);
        self.channels.view = match n {
            0 => ChannelView::Composite,
            1..=3 => ChannelView::Color(n - 1),
            _ if n - 4 < doc.channels.len() => {
                self.channels.selected = Some(n - 4);
                ChannelView::Alpha(n - 4)
            }
            _ => return,
        };
    }

    /// An alpha channel over the canvas: alone in gray, or in red over the
    /// image (its eye on).
    pub(super) fn channel_overlay(&mut self) -> Option<OverlaySource> {
        let doc = self.doc.as_ref()?;
        self.channels.sync(doc);
        let (i, style) = match (self.channels.view, self.channels.overlay) {
            (ChannelView::Alpha(i), _) => (i, OverlayStyle::Gray),
            (_, Some(i)) => (i, OverlayStyle::Tint([255, 0, 0], 0.5)),
            _ => return None,
        };
        let mask = Arc::clone(&doc.channels[i].mask);
        Some(OverlaySource {
            key: hash_of((Arc::as_ptr(&mask) as usize, i)),
            mask: OverlayMask::Gray(mask),
            scale: 1.0,
            style,
        })
    }

    pub(super) fn channels_window(&mut self, ctx: &egui::Context) {
        if !self.channels.open {
            return;
        }
        let Some(doc) = &self.doc else { return };
        self.channels.sync(doc);
        self.channel_thumbnails(ctx);
        let Some(doc) = &self.doc else { return };
        let panel = &mut self.channels;
        let thumbs: Vec<egui::TextureId> = panel
            .thumbs
            .as_ref()
            .map(|(_, _, t)| t.iter().map(TextureHandle::id).collect())
            .unwrap_or_default();
        let names: Vec<String> = doc.channels.iter().map(|c| c.name.clone()).collect();
        let has_selection = self.selection.is_some();
        let mut click = None;
        let pos = dialogs::panel_pos(ctx, "channels");
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                ..DialogSpec::new("channels", t!("xchg-channels-title"), pos)
            },
            |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let colors = [
                    (None, t!("xchg-channel-rgb")),
                    (Some(0), t!("xchg-channel-red")),
                    (Some(1), t!("xchg-channel-green")),
                    (Some(2), t!("xchg-channel-blue")),
                ];
                let composite = panel.view == ChannelView::Composite;
                for (k, (c, name)) in colors.into_iter().enumerate() {
                    let row = c.map(ChannelRow::Color);
                    let shown = match (c, panel.view) {
                        (_, ChannelView::Composite) => true,
                        (Some(c), ChannelView::Color(v)) => c == v,
                        _ => false,
                    };
                    let selected = match c {
                        None => composite,
                        Some(c) => panel.view == ChannelView::Color(c),
                    };
                    let key = channel_key(ui.ctx(), k);
                    let r = channel_row(ui, thumbs.get(k).copied(), &name, &key, shown, selected);
                    if r.eye {
                        click = Some(Click::Eye(row));
                    } else if r.row.clicked() {
                        click = Some(Click::Row(row, ui.input(|i| i.modifiers)));
                    }
                }
                for (i, name) in names.iter().enumerate() {
                    if let Some((j, text)) = panel.rename.as_mut()
                        && *j == i
                    {
                        let id = egui::Id::new("channel_rename");
                        let edit = ui_kit::text_field(ui, &t!("xchg-channel-name"), text, id);
                        edit.response.request_focus();
                        if edit.response.lost_focus() {
                            click = Some(Click::Renamed(i, text.clone()));
                        }
                        continue;
                    }
                    let shown = panel.view == ChannelView::Alpha(i) || panel.overlay == Some(i);
                    let selected = panel.selected == Some(i);
                    let key = channel_key(ui.ctx(), i + 4);
                    let r =
                        channel_row(ui, thumbs.get(i + 4).copied(), name, &key, shown, selected);
                    if r.eye {
                        click = Some(Click::Eye(Some(ChannelRow::Alpha(i))));
                    } else if r.row.double_clicked() {
                        click = Some(Click::Rename(i));
                    } else if r.row.clicked() {
                        click = Some(Click::Row(
                            Some(ChannelRow::Alpha(i)),
                            ui.input(|i| i.modifiers),
                        ));
                    }
                    r.row.context_menu(|ui| {
                        if ui.button(t!("xchg-channel-duplicate")).clicked() {
                            click = Some(Click::Duplicate(i));
                            ui.close();
                        }
                        if ui.button(t!("xchg-channel-delete")).clicked() {
                            panel.selected = Some(i);
                            click = Some(Click::Delete);
                            ui.close();
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let alpha = panel.selected.is_some();
                    let buttons = [
                        (
                            icon::CIRCLE_DASHED,
                            t!("xchg-channel-load-tip"),
                            Click::LoadSelection,
                            alpha,
                        ),
                        (
                            icon::SELECTION_PLUS,
                            t!("xchg-channel-save-tip"),
                            Click::SaveSelection,
                            has_selection,
                        ),
                        (icon::PLUS, t!("xchg-channel-new-tip"), Click::New, true),
                        (
                            icon::TRASH,
                            t!("xchg-channel-delete-tip"),
                            Click::Delete,
                            alpha,
                        ),
                    ];
                    for (glyph, tip, c, enabled) in buttons {
                        if ui_kit::icon_button(ui, glyph, &tip, enabled).clicked() {
                            click = Some(c);
                        }
                    }
                });
            },
        );
        if out.cancel {
            self.channels.open = false;
        }
        if let Some(c) = click {
            self.channel_click(c);
        }
    }

    fn channel_click(&mut self, click: Click) {
        let Some(doc) = &mut self.doc else { return };
        let p = &mut self.channels;
        match click {
            // Ctrl + click: as the selection (Shift adds, Alt subtracts,
            // both intersect).
            Click::Row(row, m) if m.command => {
                let mode = self.selection_mode(m.shift, m.alt);
                let Some(doc) = &self.doc else { return };
                let found = match row {
                    Some(ChannelRow::Alpha(i)) => doc.channel_selection(i),
                    Some(ChannelRow::Color(c)) => {
                        Selection::from_mask(&doc.color_channel(c), (0, 0))
                    }
                    None => Selection::from_mask(&luminosity(&doc.flatten()), (0, 0)),
                };
                self.sel.step_name = Some(select_menu::step_label(t!("sel-menu-load")));
                self.apply_selection(found, mode);
            }
            Click::Row(row, _) => {
                p.view = match row {
                    None => ChannelView::Composite,
                    Some(ChannelRow::Color(c)) => ChannelView::Color(c),
                    Some(ChannelRow::Alpha(i)) => {
                        p.selected = Some(i);
                        ChannelView::Alpha(i)
                    }
                };
                if row.is_some() {
                    p.overlay = None;
                }
            }
            Click::Eye(row) => match row {
                // The composite's eye: the image back, or off for the
                // alpha channel shown over it.
                None => match (p.view, p.overlay) {
                    (ChannelView::Composite, Some(i)) => {
                        (p.view, p.overlay) = (ChannelView::Alpha(i), None);
                    }
                    (ChannelView::Alpha(i), None) => {
                        (p.view, p.overlay) = (ChannelView::Composite, Some(i));
                    }
                    _ => p.view = ChannelView::Composite,
                },
                Some(ChannelRow::Color(c)) => {
                    p.view = if p.view == ChannelView::Color(c) {
                        ChannelView::Composite
                    } else {
                        ChannelView::Color(c)
                    };
                }
                Some(ChannelRow::Alpha(i)) => {
                    if p.view == ChannelView::Alpha(i) {
                        p.view = ChannelView::Composite;
                    } else {
                        p.overlay = (p.overlay != Some(i)).then_some(i);
                    }
                }
            },
            Click::Rename(i) => p.rename = Some((i, doc.channels[i].name.clone())),
            Click::Renamed(i, name) => {
                p.rename = None;
                doc.rename_channel(i, &name);
            }
            Click::New => p.selected = Some(doc.new_channel()),
            Click::Duplicate(i) => p.selected = doc.duplicate_channel(i),
            Click::Delete => {
                if let Some(i) = p.selected.take() {
                    doc.delete_channel(i);
                    if p.view == ChannelView::Alpha(i) {
                        p.view = ChannelView::Composite;
                    }
                    p.overlay = None;
                }
            }
            Click::LoadSelection => {
                if let Some(i) = p.selected {
                    let found = doc.channel_selection(i);
                    self.sel.step_name = Some(select_menu::step_label(t!("sel-menu-load")));
                    self.apply_selection(found, SelectMode::Replace);
                }
            }
            Click::SaveSelection => {
                if let Some(sel) = &self.selection {
                    let mask = sel.to_mask(doc.full_rect());
                    p.selected = Some(doc.save_selection_channel(mask));
                }
            }
        }
    }

    /// Thumbnails of the channels, made again when what they show changes
    /// (at most every [`THUMBS_EVERY`]).
    fn channel_thumbnails(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.doc else { return };
        let layers = doc.render_layers();
        let opts = crate::compose::Options {
            later_styles: true,
            ..Default::default()
        };
        let sigs = crate::compose::signatures(&layers, &opts);
        let alphas: Vec<usize> = doc
            .channels
            .iter()
            .map(|c| Arc::as_ptr(&c.mask) as usize)
            .collect();
        let key = hash_of((doc.uid(), format!("{sigs:?}"), alphas));
        let now = ctx.input(|i| i.time);
        if let Some((k, at, _)) = &self.channels.thumbs {
            if *k == key {
                return;
            }
            let wait = THUMBS_EVERY - (now - at);
            if wait > 0.0 {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
                return;
            }
        }
        let uid = doc.uid();
        let reduced = &mut self.channels.reduced;
        if reduced.as_ref().is_none_or(|(d, _)| *d != uid) {
            *reduced = Some((uid, crate::compose::LevelCache::new(thumb_level(doc))));
        }
        let cache = &mut reduced.as_mut().expect("set above").1;
        let small = small_composite(doc, cache);
        let (w, h) = small.dimensions();
        let mut images: Vec<ColorImage> = vec![to_color_image(&small)];
        for c in 0..3 {
            let gray = RgbaImage::from_fn(w, h, |x, y| {
                let p = small.get_pixel(x, y).0;
                let a = p[3] as u32;
                let v = ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
                image::Rgba([v, v, v, 255])
            });
            images.push(to_color_image(&gray));
        }
        for ch in &doc.channels {
            let t = image::imageops::thumbnail(&*ch.mask, w, h);
            let rgba = RgbaImage::from_fn(t.width(), t.height(), |x, y| {
                let v = t.get_pixel(x, y).0[0];
                image::Rgba([v, v, v, 255])
            });
            images.push(to_color_image(&rgba));
        }
        let textures = images
            .into_iter()
            .enumerate()
            .map(|(i, image)| {
                ctx.load_texture(format!("channel-{i}"), image, TextureOptions::LINEAR)
            })
            .collect();
        self.channels.thumbs = Some((key, now, textures));
    }
}

/// Level of reduction the thumbnails are composited at (quick even for a
/// large image).
fn thumb_level(doc: &Document) -> u32 {
    let (w, h) = (doc.width(), doc.height());
    let mut level = 0;
    while (w.max(h) >> (level + 1)) >= THUMB && level < 12 {
        level += 1;
    }
    level
}

/// The flattened image reduced to [`THUMB`], from the layers reduced in
/// `cache` (at [`thumb_level`]).
fn small_composite(doc: &Document, cache: &mut crate::compose::LevelCache) -> RgbaImage {
    let (w, h) = (doc.width(), doc.height());
    let level = crate::compose::Sources::level(cache);
    let layers = doc.render_layers();
    let options = crate::compose::Options {
        later_styles: true,
        ..Default::default()
    };
    let plan = crate::compose::plan(&layers, doc.full_rect(), cache, &options);
    cache.retain_used();
    let rect = PixelRect {
        x: 0,
        y: 0,
        w: w.div_ceil(1 << level),
        h: h.div_ceil(1 << level),
    };
    let image = plan.render(rect);
    let scale = (THUMB as f32 / image.width().max(image.height()) as f32).min(1.0);
    let (tw, th) = (
        ((image.width() as f32 * scale).round() as u32).max(1),
        ((image.height() as f32 * scale).round() as u32).max(1),
    );
    image::imageops::thumbnail(&image, tw, th)
}

fn to_color_image(image: &RgbaImage) -> ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, image.as_raw())
}

/// Luminosity of an image (Ctrl + click on the composite): transparency
/// counts as white.
fn luminosity(image: &RgbaImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b, a] = image.get_pixel(x, y).0;
        let y = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
        let a = a as u32;
        image::Luma([((y * a + 255 * (255 - a) + 127) / 255) as u8])
    })
}

/// Shortcut of the `n`-th row (Ctrl+2 for the composite…), up to Ctrl+9.
fn channel_key(ctx: &egui::Context, n: usize) -> String {
    const DIGITS: [Key; 8] = [
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    DIGITS.get(n).map_or_else(String::new, |&k| {
        keys::shortcut(ctx, &KeyboardShortcut::new(Modifiers::COMMAND, k))
    })
}

/// The `n`-th channel shortcut pressed this frame (exactly Ctrl+2…9, the
/// digit row by its place: see `shortcut_key`), if any.
pub(super) fn channel_shortcut(input: &mut egui::InputState) -> Option<usize> {
    const DIGITS: [Key; 8] = [
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    DIGITS
        .iter()
        .position(|&k| consume_exact(input, KeyboardShortcut::new(Modifiers::COMMAND, k)))
}

/// A row's responses: the row, and whether its eye was clicked.
struct RowResponse {
    row: Response,
    eye: bool,
}

/// A row of the list: eye, thumbnail, name, shortcut.
fn channel_row(
    ui: &mut egui::Ui,
    thumb: Option<egui::TextureId>,
    name: &str,
    key: &str,
    shown: bool,
    selected: bool,
) -> RowResponse {
    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(rect, 6.0, ui_kit::color::ACCENT_SOFT);
    } else if row.hovered() {
        painter.rect_filled(rect, 6.0, ui_kit::color::BG3);
    }
    let eye = Rect::from_min_size(
        pos2(rect.left() + 2.0, rect.center().y - 12.0),
        Vec2::splat(24.0),
    );
    let eye_response = ui.interact(eye, row.id.with("eye"), Sense::click());
    let (glyph, fg) = if shown {
        (icon::EYE, ui_kit::color::MUTED)
    } else {
        (icon::EYE_SLASH, ui_kit::color::DISABLED)
    };
    painter.text(
        eye.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(14.0),
        fg,
    );
    let frame = Rect::from_min_size(pos2(eye.right() + 4.0, rect.top() + 5.0), vec2(40.0, 30.0));
    painter.rect_filled(frame, 3.0, ui_kit::color::THUMB_EMPTY);
    if let Some(tex) = thumb {
        let size = ui.ctx().tex_manager().read().meta(tex).map(|m| m.size);
        let r = size.map_or(frame, |[w, h]| {
            let k = (frame.width() / w as f32).min(frame.height() / h as f32);
            Rect::from_center_size(frame.center(), vec2(w as f32 * k, h as f32 * k))
        });
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(tex, r, uv, Color32::WHITE);
    }
    let shortcut = painter.text(
        pos2(rect.right() - 8.0, rect.center().y),
        Align2::RIGHT_CENTER,
        key,
        FontId::proportional(11.0),
        ui_kit::color::LABEL,
    );
    let name_area =
        Rect::from_x_y_ranges(frame.right() + 10.0..=shortcut.left() - 6.0, rect.y_range());
    painter
        .with_clip_rect(name_area.intersect(painter.clip_rect()))
        .text(
            pos2(name_area.left(), rect.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(ui_kit::TEXT_SIZE),
            ui_kit::color::TEXT,
        );
    let eye_clicked = eye_response.clicked();
    let tip = t!(
        "input3-channel-row-tip",
        ctrl_click = keys::click(ui.ctx(), Modifiers::COMMAND)
    );
    let row = row.on_hover_text(tip);
    row.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, name));
    RowResponse {
        row,
        eye: eye_clicked,
    }
}
