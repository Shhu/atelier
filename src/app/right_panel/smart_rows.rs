//! A smart object in the Layers list, like Photoshop's: its thumbnail's
//! badge (a broken link when its file is missing), then under its row the
//! Smart Filters row (its eye turns them all off; its thumbnail is the
//! filter mask: a click targets it, Shift + click turns it off) and the
//! filters, the last applied on top, each with its eye and its Blending
//! Options icon. Double-click: edits it; drag: moves it in the list; right
//! click: its menu.

use super::*;
use crate::app::smart_ui::{Command, filter_name};

/// Height of the smart filters' rows.
const SUB_ROW: f32 = 26.0;

/// The smart filters' rows of a frame, to place a dragged one.
#[derive(Default)]
pub(super) struct FilterRows {
    /// Row, layer, index of its filter.
    rows: Vec<(Rect, usize, usize)>,
    /// A filter row starts being dragged: its layer and index.
    drag_started: Option<(u64, usize)>,
}

/// Height of the smart filters' rows of the layers `shown`.
pub(super) fn height(doc: &Document, shown: &[usize]) -> f32 {
    let rows: usize = shown
        .iter()
        .filter_map(|&i| doc.layers[i].smart.as_deref())
        .filter(|s| !s.filters.is_empty() || s.filter_mask.is_some())
        .map(|s| 1 + s.filters.len())
        .sum();
    rows as f32 * (SUB_ROW + ROW_GAP)
}

/// The badge of a smart object's thumbnail `thumb`.
pub(super) fn badge(painter: &egui::Painter, thumb: Rect, layer: &Layer) {
    let Some(s) = layer.smart.as_deref() else {
        return;
    };
    let missing = s.link.as_ref().is_some_and(|l| l.missing);
    let (glyph, fg) = match (&s.link, missing) {
        (_, true) => (icon::LINK_BREAK, Color32::from_rgb(0xe5, 0x48, 0x4d)),
        (Some(_), false) => (icon::LINK_SIMPLE, color::TEXT),
        (None, false) => (icon::FILE, color::TEXT),
    };
    let at = Rect::from_min_size(thumb.right_bottom() - vec2(13.0, 13.0), Vec2::splat(12.0));
    painter.rect_filled(at, 3.0, color::BG1.gamma_multiply(0.9));
    painter.text(
        at.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(10.0),
        fg,
    );
}

/// What the rows of a smart object's filters show.
struct Shown<'a> {
    doc: &'a Document,
    i: usize,
    /// Left of the eyes, and of the thumbnails.
    eyes: f32,
    left: f32,
}

/// The rows of layer `i`'s smart filters, under its row; `eyes`: the left
/// of the eye column, `left`: of the layer's thumbnail. A click selects
/// the layer (`click`), and asks for a command (`action`).
#[allow(clippy::too_many_arguments)]
pub(super) fn filter_rows(
    ui: &mut Ui,
    doc: &Document,
    i: usize,
    (eyes, left): (f32, f32),
    mask_tex: Option<egui::TextureId>,
    out: &mut FilterRows,
    click: &mut Option<(usize, RowPart, Modifiers)>,
    action: &mut Option<Action>,
) {
    let Some(s) = doc.layers[i].smart.as_deref() else {
        return;
    };
    if s.filters.is_empty() && s.filter_mask.is_none() {
        return;
    }
    let shown = Shown { doc, i, eyes, left };
    header_row(ui, &shown, mask_tex, click, action);
    for k in (0..s.filters.len()).rev() {
        filter_row(ui, &shown, k, out, click, action);
    }
}

/// An eye of the column, on or off.
fn eye(ui: &mut Ui, rect: Rect, id: Id, on: bool, dimmed: bool) -> Response {
    let r = Rect::from_min_size(pos2(rect.left(), rect.center().y - 11.0), vec2(28.0, 22.0));
    let (glyph, fg) = match (on, dimmed) {
        (true, false) => (icon::EYE, color::MUTED),
        _ => (icon::EYE_SLASH, color::DISABLED),
    };
    let tip = t!(
        "input3-visibility-tip",
        alt_click = keys::click(ui.ctx(), Modifiers::ALT)
    );
    row_icon(ui, r, id, glyph, fg, &tip)
}

/// The Smart Filters row: eye, filter mask thumbnail, title.
fn header_row(
    ui: &mut Ui,
    shown: &Shown,
    mask_tex: Option<egui::TextureId>,
    click: &mut Option<(usize, RowPart, Modifiers)>,
    action: &mut Option<Action>,
) {
    let (doc, i) = (shown.doc, shown.i);
    let layer = &doc.layers[i];
    let Some(s) = layer.smart.as_deref() else {
        return;
    };
    let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), SUB_ROW), Sense::click());
    let painter = ui.painter().clone();
    if ui.rect_contains_pointer(rect) {
        painter.rect_filled(rect, 6.0, color::BG3);
    }
    let eye_rect = Rect::from_min_max(
        pos2(shown.eyes, rect.top()),
        pos2(shown.eyes + 28.0, rect.bottom()),
    );
    let select = |c: &mut Option<_>| *c = Some((i, RowPart::Name, Modifiers::NONE));
    if eye(
        ui,
        eye_rect,
        Id::new(("smart_eye", layer.id)),
        s.filters_enabled,
        false,
    )
    .clicked()
    {
        select(click);
        *action = Some(Action::Smart(Command::ToggleFilters));
    }
    let mut x = shown.left;
    if let Some(m) = &s.filter_mask {
        let thumb = Rect::from_min_size(pos2(x, rect.center().y - 10.0), vec2(30.0, 20.0));
        let clipped = painter.with_clip_rect(thumb.intersect(painter.clip_rect()));
        clipped.rect_filled(thumb, 3.0, Color32::from_gray(m.default));
        if let Some(tex) = mask_tex {
            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
            clipped.image(
                tex,
                fit_rect(thumb, doc.width(), doc.height()),
                uv,
                Color32::WHITE,
            );
        }
        if !m.enabled {
            let stroke = UiStroke::new(2.0, Color32::from_rgb(0xe5, 0x48, 0x4d));
            clipped.line_segment([thumb.left_top(), thumb.right_bottom()], stroke);
            clipped.line_segment([thumb.left_bottom(), thumb.right_top()], stroke);
        }
        thumb_frame(
            &painter,
            thumb,
            doc.active == i && doc.filter_mask_targeted(),
        );
        let response = ui
            .interact(thumb, Id::new(("smart_mask", layer.id)), Sense::click())
            .on_hover_text(t!(
                "input3-filter-mask-tip",
                shift_click = keys::click(ui.ctx(), Modifiers::SHIFT)
            ));
        if response.clicked() {
            select(click);
            let shift = ui.input(|inp| inp.modifiers.shift);
            *action = Some(Action::Smart(if shift {
                Command::ToggleFilterMask
            } else {
                Command::TargetFilterMask
            }));
        }
        x = thumb.right() + 8.0;
    }
    painter.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        t!("smart-filters"),
        FontId::proportional(12.0),
        if s.filters_enabled {
            color::TEXT
        } else {
            color::LABEL
        },
    );
    if row.clicked() || row.secondary_clicked() {
        select(click);
    }
    let has_mask = s.filter_mask.is_some();
    let entries = [
        if has_mask {
            MenuEntry::Item(
                if s.filter_mask.as_ref().is_some_and(|m| m.enabled) {
                    t!("smart-menu-disable-filter-mask")
                } else {
                    t!("smart-menu-enable-filter-mask")
                },
                true,
                None,
                Action::Smart(Command::ToggleFilterMask),
            )
        } else {
            MenuEntry::Item(
                t!("smart-menu-add-filter-mask"),
                true,
                None,
                Action::Smart(Command::AddFilterMask),
            )
        },
        MenuEntry::Item(
            t!("smart-menu-delete-filter-mask"),
            has_mask,
            None,
            Action::Smart(Command::DeleteFilterMask),
        ),
        MenuEntry::Separator,
        MenuEntry::Item(
            if s.filters_enabled {
                t!("smart-menu-disable-filters")
            } else {
                t!("smart-menu-enable-filters")
            },
            true,
            None,
            Action::Smart(Command::ToggleFilters),
        ),
        MenuEntry::Item(
            t!("smart-menu-clear-filters"),
            true,
            None,
            Action::Smart(Command::ClearFilters),
        ),
    ];
    row.context_menu(|ui| super::super::menus::menu_entries(ui, &entries, action));
}

/// The row of smart filter `k`: eye, name, Blending Options icon.
fn filter_row(
    ui: &mut Ui,
    shown: &Shown,
    k: usize,
    out: &mut FilterRows,
    click: &mut Option<(usize, RowPart, Modifiers)>,
    action: &mut Option<Action>,
) {
    let (doc, i) = (shown.doc, shown.i);
    let layer = &doc.layers[i];
    let Some(s) = layer.smart.as_deref() else {
        return;
    };
    let f = &s.filters[k];
    let (rect, row) =
        ui.allocate_exact_size(vec2(ui.available_width(), SUB_ROW), Sense::click_and_drag());
    out.rows.push((rect, i, k));
    let painter = ui.painter().clone();
    if ui.rect_contains_pointer(rect) {
        painter.rect_filled(rect, 6.0, color::BG3);
    }
    let select = |c: &mut Option<_>| *c = Some((i, RowPart::Name, Modifiers::NONE));
    let eye_rect = Rect::from_min_max(
        pos2(shown.eyes, rect.top()),
        pos2(shown.eyes + 28.0, rect.bottom()),
    );
    let id = Id::new(("smart_filter_eye", layer.id, k));
    if eye(ui, eye_rect, id, f.visible, !s.filters_enabled).clicked() {
        select(click);
        *action = Some(Action::Smart(Command::ToggleFilter(k)));
    }
    let blending = Rect::from_min_size(
        pos2(rect.right() - 30.0, rect.center().y - 11.0),
        vec2(24.0, 22.0),
    );
    let tip = t!("smart-blending-tip");
    let id = Id::new(("smart_filter_blending", layer.id, k));
    let b = row_icon(
        ui,
        blending,
        id,
        icon::SLIDERS_HORIZONTAL,
        color::MUTED,
        &tip,
    );
    if b.clicked() || b.double_clicked() {
        select(click);
        *action = Some(Action::Smart(Command::FilterBlending(k)));
    }
    let name = filter_name(&f.filter);
    let text = Rect::from_min_max(
        pos2(shown.left + 10.0, rect.top()),
        pos2(blending.left() - 4.0, rect.bottom()),
    );
    let fg = if f.visible && s.filters_enabled {
        color::TEXT
    } else {
        color::LABEL
    };
    painter
        .with_clip_rect(text.intersect(painter.clip_rect()))
        .text(
            pos2(text.left(), rect.center().y),
            Align2::LEFT_CENTER,
            name,
            FontId::proportional(12.0),
            fg,
        );
    let row = row.on_hover_text(t!("smart-filter-row-tip"));
    if row.double_clicked() {
        select(click);
        *action = Some(Action::Smart(Command::EditFilter(k)));
    } else if row.clicked() || row.secondary_clicked() {
        select(click);
    }
    if row.drag_started() {
        out.drag_started = Some((layer.id, k));
    }
    let entries = [
        MenuEntry::Item(
            t!("smart-menu-edit-filter"),
            f.filter.editable(),
            None,
            Action::Smart(Command::EditFilter(k)),
        ),
        MenuEntry::Item(
            t!("smart-menu-filter-blending"),
            true,
            None,
            Action::Smart(Command::FilterBlending(k)),
        ),
        MenuEntry::Separator,
        MenuEntry::Item(
            if f.visible {
                t!("smart-menu-disable-filter")
            } else {
                t!("smart-menu-enable-filter")
            },
            true,
            None,
            Action::Smart(Command::ToggleFilter(k)),
        ),
        MenuEntry::Item(
            t!("smart-menu-delete-filter"),
            true,
            None,
            Action::Smart(Command::DeleteFilter(k)),
        ),
    ];
    row.context_menu(|ui| super::super::menus::menu_entries(ui, &entries, action));
}

impl AtelierApp {
    /// Thumbnails of the filter masks of the smart objects among `shown`.
    pub(super) fn filter_mask_thumbs(
        &mut self,
        ctx: &egui::Context,
        shown: &[usize],
    ) -> HashMap<usize, egui::TextureId> {
        let mut out = HashMap::new();
        let Some(doc) = &self.doc else { return out };
        let layers = doc.render_layers();
        let canvas = doc.full_rect();
        let ppp = ctx.pixels_per_point();
        let (tw, th) = ((30.0 * ppp).round() as u32, (20.0 * ppp).round() as u32);
        for &i in shown {
            let layer = &layers[i];
            let Some(m) = layer.smart.as_ref().and_then(|s| s.filter_mask.as_ref()) else {
                continue;
            };
            let key = hash_of((m.rev(), m.origin(layer.offset), canvas));
            let thumbs = &mut self.smart.thumbs;
            match thumbs.get_mut(&layer.id) {
                Some((k, _)) if *k == key => {}
                entry => {
                    let image = mask_thumbnail(m, layer.offset, canvas, tw, th);
                    let size = [image.width() as usize, image.height() as usize];
                    let image = ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    match entry {
                        Some((k, tex)) => {
                            tex.set(image, IMAGE_TEX_OPTIONS);
                            *k = key;
                        }
                        None => {
                            let name = format!("filter-mask-{}", layer.id);
                            let tex = ctx.load_texture(name, image, IMAGE_TEX_OPTIONS);
                            thumbs.insert(layer.id, (key, tex));
                        }
                    }
                }
            }
            if let Some((_, tex)) = self.smart.thumbs.get(&layer.id) {
                out.insert(i, tex.id());
            }
        }
        out
    }

    /// A smart filter row being dragged: the insertion line follows the
    /// pointer; released, the filter moves there.
    pub(super) fn drag_filter_rows(
        &mut self,
        ui: &Ui,
        rows: FilterRows,
        action: &mut Option<Action>,
    ) {
        if let Some(started) = rows.drag_started {
            self.smart.drag = Some(started);
        }
        let Some((layer, from)) = self.smart.drag else {
            return;
        };
        let (pointer, down) =
            ui.input(|inp| (inp.pointer.interact_pos(), inp.pointer.primary_down()));
        let Some(i) = self.doc.as_ref().and_then(|d| d.layer_index(layer)) else {
            self.smart.drag = None;
            return;
        };
        // The rows of its layer, top first: from the last filter down.
        let own: Vec<(Rect, usize)> = rows
            .rows
            .iter()
            .filter(|(_, l, _)| *l == i)
            .map(|&(r, _, k)| (r, k))
            .collect();
        let gap = pointer.map(|p| own.iter().filter(|(r, _)| p.y > r.center().y).count());
        if let (Some(gap), Some((first, _))) = (gap, own.first()) {
            let y = own.get(gap).map_or_else(
                || own.last().map_or(0.0, |(r, _)| r.bottom() + 1.0),
                |(r, _)| r.top() - 1.0,
            );
            let x = first.x_range();
            ui.painter().hline(x, y, UiStroke::new(2.0, color::ACCENT));
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if down {
            return;
        }
        self.smart.drag = None;
        // `gap` rows above it: the filter at index n − 1 − gap, counting
        // the dragged one where it was.
        if let Some(gap) = gap {
            let n = own.len();
            let to = n.saturating_sub(gap).min(n);
            let to = if to > from { to - 1 } else { to };
            if to != from {
                *action = Some(Action::Smart(Command::MoveFilter(from, to)));
            }
        }
    }
}
