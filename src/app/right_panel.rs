//! Side panel: properties of the active layer (scrollable), then the layers
//! list stuck to the bottom.

use super::keys;
use super::*;
use crate::i18n::t;
use egui::{Align2, FontId, Id, Margin, Response, Ui, UiBuilder};
use ui_kit::color;

mod artboard_props;
mod shape_props;
mod smart_rows;
mod type_sections;

/// Height of the panel headings ("Properties", "Layers").
const HEADING_HEIGHT: f32 = 42.0;
/// Blend mode and opacity of the active layer, under the Layers heading.
const BLEND_ROW: f32 = 36.0;
/// Row of the layers list.
const ROW_HEIGHT: f32 = 44.0;
const ROW_GAP: f32 = 2.0;
/// Rows shown before the list scrolls.
const VISIBLE_ROWS: usize = 5;
/// Below this panel height, the list shows this many rows before scrolling.
const SHORT_PANEL: f32 = 650.0;
const SHORT_VISIBLE_ROWS: usize = 3;
/// Temp data: the Properties' height, unscrolled.
const PROPERTIES_NATURAL: &str = "side-properties-natural";
/// Room the Properties keep above the Layers, when the panel has it.
const PROPERTIES_MIN: f32 = 160.0;
/// Buttons under the list, with their top border.
const FOOTER_HEIGHT: f32 = 41.0;
/// Gap between the fields of a row.
const GAP: f32 = 8.0;
/// Coordinates accepted by the X / Y fields.
const MAX_OFFSET: i64 = 2 * MAX_DIMENSION as i64;

impl AtelierApp {
    pub(super) fn right_panel(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let shown = self.doc.as_ref().map(|d| (shown_rows(d), d));
        let Some((n, sub)) = shown.map(|(rows, d)| (rows.len(), smart_rows::height(d, &rows)))
        else {
            heading_bar(ui, &t!("panel-properties"), |_| {});
            let rest = Rect::from_min_max(ui.cursor().min, full.max);
            ui.painter().hline(rest.x_range(), rest.top() + 0.5, line());
            ui.painter().text(
                rest.center(),
                Align2::CENTER_CENTER,
                t!("panel-no-image"),
                FontId::proportional(ui_kit::TEXT_SIZE),
                color::LABEL,
            );
            return;
        };
        // A low panel lists fewer rows before scrolling, for the Properties;
        // a tall one more, as many as the Properties leave room for.
        let fixed = HEADING_HEIGHT + BLEND_ROW + 6.0 + FOOTER_HEIGHT;
        let visible = if full.height() < SHORT_PANEL {
            SHORT_VISIBLE_ROWS
        } else {
            let natural = ui
                .data(|d| d.get_temp::<f32>(Id::new(PROPERTIES_NATURAL)))
                .unwrap_or(f32::INFINITY);
            let room = full.height() - natural - fixed + ROW_GAP;
            VISIBLE_ROWS.max((room / (ROW_HEIGHT + ROW_GAP)).floor().max(0.0) as usize)
        };
        // Not while the pointer is over the panel: a click on a layer whose
        // Properties are taller would move the rows under it.
        let id = Id::new("side-visible-rows");
        let over = ui.rect_contains_pointer(full);
        let visible = match ui.data(|d| d.get_temp::<(f32, usize)>(id)) {
            Some((height, kept)) if over && height == full.height() => kept,
            _ => visible,
        };
        ui.data_mut(|d| d.insert_temp(id, (full.height(), visible)));
        let rows = n.clamp(1, visible) as f32;
        let most = visible as f32 * (ROW_HEIGHT + ROW_GAP) - ROW_GAP;
        let list_height = (rows * ROW_HEIGHT + (rows - 1.0) * ROW_GAP + sub).min(most);
        // The Layers part: as dragged (a share of the panel), or fitted to
        // the list.
        let dragged = self.workspace.layout.layers_share;
        let layers_height = dragged.map_or(list_height + fixed, |share| share * full.height());
        // Properties scroll: when the panel is short, they give up room
        // first, down to the layers' heading, one row and their buttons;
        // otherwise they keep a few lines whatever the split.
        let layers_min = ROW_HEIGHT + fixed;
        let props_min = PROPERTIES_MIN;
        let split = (full.bottom() - layers_height)
            .max(full.top() + props_min)
            .min(full.bottom() - layers_min)
            .max(full.top());
        // The list first: a double-click there focuses the Name field in
        // the same frame.
        let mut layers = ui.new_child(
            UiBuilder::new().max_rect(Rect::from_min_max(pos2(full.left(), split), full.max)),
        );
        self.layers_section(&mut layers);
        let props_rect = Rect::from_min_max(full.min, pos2(full.right(), split));
        let mut props = ui.new_child(UiBuilder::new().max_rect(props_rect));
        props.set_clip_rect(props_rect.intersect(ui.clip_rect()));
        // Their scroll bar shows while they overflow.
        props.spacing_mut().scroll.dormant_handle_opacity = 0.6;
        self.properties(&mut props);
        self.layers_splitter(ui, full, split, layers_min);
    }

    /// The border between Properties and Layers: dragging it gives one
    /// more room (kept for the next sessions); a double click fits the
    /// Layers to their list again.
    fn layers_splitter(&mut self, ui: &mut Ui, full: Rect, split: f32, layers_min: f32) {
        let rect = Rect::from_x_y_ranges(full.x_range(), split - 3.0..=split + 3.0);
        let response = ui
            .interact(rect, Id::new("layers-splitter"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
        // From the height and the pointer at the press: past a limit, the
        // border comes back with the pointer.
        let start_id = Id::new("layers-splitter-start");
        if response.drag_started()
            && let Some(press) = ui.input(|i| i.pointer.press_origin())
        {
            ui.data_mut(|d| d.insert_temp(start_id, (full.bottom() - split, press.y)));
        }
        let layout = &mut self.workspace.layout;
        if response.double_clicked() {
            layout.layers_share = None;
            self.workspace.save_layout();
        } else if response.dragged()
            && let Some((start, press)) = ui.data(|d| d.get_temp::<(f32, f32)>(start_id))
            && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            let most = (full.height() - PROPERTIES_MIN).max(layers_min);
            let height = start - (pointer.y - press);
            let share = height.clamp(layers_min, most) / full.height().max(1.0);
            layout.layers_share = Some(share.clamp(0.0, 1.0));
        }
        if response.drag_stopped() {
            self.workspace.save_layout();
        }
        if response.hovered() || response.dragged() {
            ui.painter().hline(
                full.x_range(),
                split + 0.5,
                egui::Stroke::new(2.0, color::ACCENT),
            );
        } else {
            // A grip in the middle, so that the border can be found.
            let grip = Rect::from_center_size(pos2(full.center().x, split + 3.0), vec2(28.0, 2.0));
            ui.painter().rect_filled(grip, 1.0, color::LINE_HOVER);
        }
    }

    // ------------------------------------------------------------ properties

    fn properties(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let locked = doc.is_locked(doc.active);
        let (layer_id, is_text) = (layer.id, layer.text.is_some());
        let kind = kind_label(layer);
        heading_bar(ui, &t!("panel-properties"), |ui| badge(ui, &kind));

        // We edit a copy; if it changes, the previous state is recorded
        // (once per series of edits to the same property).
        let mut name = layer.name.clone();
        let opacity = layer.opacity;
        let mut fill_opacity = layer.fill;
        let blend = layer.blend;
        let mut effects = layer.effects.clone();
        let mut content = layer.kind.clone();
        let mut mask = layer.mask.as_ref().map(|m| (m.density, m.feather));
        let (group, pixels) = (layer.is_group(), layer.has_pixels());
        let artboard = layer.artboard.is_some();
        let mut fill = None;
        let mut action = None;
        let top = ui.min_rect().height();
        let scrolled = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                section(ui, None, |ui| {
                    let focus = std::mem::take(&mut self.focus_name);
                    name_field(ui, &mut name, focus);
                    if locked {
                        locked_notice(ui);
                    }
                });
                if pixels || (group && !artboard) {
                    self.transform_section(ui);
                }
                // (Opacity and blend mode: under the Layers heading.)
                if !group {
                    section(ui, Some(&t!("side-layer")), |ui| {
                        ui.add_enabled_ui(!locked, |ui| fill_field(ui, &mut fill_opacity));
                    });
                }
                if is_text {
                    self.character_section(ui, locked);
                    self.paragraph_section(ui, locked);
                }
                self.shape_section(ui, locked);
                self.artboard_section(ui, locked);
                if let LayerKind::Adjustment(a) = &mut content {
                    let title = a.title();
                    let histogram = self.below_histogram(ui.ctx(), a);
                    let primary = self.primary.to_srgba_unmultiplied();
                    let state = &mut self.panel_editor;
                    if state.0 != layer_id {
                        *state = (
                            layer_id,
                            usize::from(a.kind() == AdjustKind::ColorBalance),
                            None,
                        );
                    }
                    section(ui, Some(&title), |ui| {
                        ui.add_enabled_ui(!locked, |ui| {
                            let (range, point) = (&mut state.1, &mut state.2);
                            dialogs::adjustment_editor(
                                ui,
                                a,
                                range,
                                point,
                                histogram.as_ref(),
                                primary,
                            );
                        });
                    });
                }
                if let LayerKind::Fill(f) = &mut content {
                    section(ui, Some(&t!("core-fill-content")), |ui| {
                        ui.add_enabled_ui(!locked, |ui| fill_content_fields(ui, f));
                    });
                }
                if let Some((density, feather)) = &mut mask {
                    section(ui, Some(&t!("core-mask")), |ui| {
                        ui.add_enabled_ui(!locked, |ui| {
                            action = mask_fields(ui, density, feather, &self.doc);
                        });
                    });
                }
                if pixels {
                    section(ui, Some(&t!("panel-effects")), |ui| {
                        ui.add_enabled_ui(!locked, |ui| {
                            if let Some(style) = effect_fields(ui, &mut effects) {
                                action = Some(Action::LayerStyle(Some(style)));
                            }
                        });
                    });
                    let (primary, secondary) = (self.primary, self.secondary);
                    section(ui, Some(&t!("side-fill-layer")), |ui| {
                        ui.add_enabled_ui(!locked, |ui| {
                            fill = fill_buttons(ui, primary, secondary);
                        });
                    });
                }
            });
        // Their height unscrolled: what a tall panel leaves to the Layers.
        let natural = top + scrolled.content_size.y;
        ui.data_mut(|d| d.insert_temp(Id::new(PROPERTIES_NATURAL), natural));

        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        if layer.id != layer_id {
            return;
        }
        let old_mask = layer.mask.as_ref().map(|m| (m.density, m.feather));
        let changes = [
            (name != layer.name, "name"),
            (opacity != layer.opacity, "opacity"),
            (fill_opacity != layer.fill, "fill"),
            (blend != layer.blend, "blend"),
            (effects != layer.effects, "styles"),
            (content != layer.kind, "content"),
            (mask.map(|m| m.0) != old_mask.map(|m| m.0), "density"),
            (mask.map(|m| m.1) != old_mask.map(|m| m.1), "feather"),
        ];
        let steps: Vec<(String, &str)> = changes
            .into_iter()
            .filter(|c| c.0)
            .map(|(_, prop)| (property_step(prop, layer), prop))
            .collect();
        // A text being typed ends first (an empty new one goes away).
        if !steps.is_empty() {
            self.commit_text();
            if self
                .doc
                .as_ref()
                .is_none_or(|d| d.active_layer().id != layer_id)
            {
                return;
            }
        }
        for (step, prop) in steps {
            self.record_change(step, Some((layer_id, prop)));
        }
        if let Some(doc) = &mut self.doc {
            let layer = doc.active_layer_mut();
            (
                layer.name,
                layer.opacity,
                layer.fill,
                layer.blend,
                layer.effects,
            ) = (name, opacity, fill_opacity, blend, effects);
            layer.kind = content;
            if let (Some(m), Some((density, feather))) = (&mut layer.mask, mask) {
                (m.density, m.feather) = (density, feather);
            }
        }
        if let Some(act) = fill.or(action) {
            self.run(act);
        }
    }

    /// Histograms of what an adjustment layer adjusts (Levels, Curves): the
    /// layers below it as composited, made again on a worker thread when
    /// they change (the former ones shown meanwhile).
    pub(super) fn below_histogram(
        &mut self,
        ctx: &egui::Context,
        a: &Adjustment,
    ) -> Option<Histograms> {
        if !matches!(a, Adjustment::Levels(_) | Adjustment::Curves(_)) {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let layers = doc.render_layers();
        let below = &layers[..doc.active];
        let held = doc.effects_held();
        let opts = crate::compose::Options {
            held,
            later_styles: true,
            ..Default::default()
        };
        let sigs = crate::compose::signatures(below, &opts);
        let (uid, layer) = (doc.uid(), doc.active_layer().id);
        let key = hash_of((uid, layer, format!("{sigs:?}")));
        let h = &mut self.below_histogram;
        if let Some(job) = h.job.take_if(|j| j.1.is_finished())
            && let Ok((cache, histograms)) = job.1.join()
        {
            if job.0 == (uid, layer) {
                h.shown = Some((job.2, (uid, layer), histograms));
            }
            h.cache = Some((uid, cache));
        }
        if h.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        } else if h.shown.as_ref().is_none_or(|(k, _, _)| *k != key) {
            // The layers below it, reduced to about 1000 px.
            let canvas = doc.full_rect();
            let side = canvas.w.max(canvas.h) as f32;
            let level = (side / 1024.0).log2().ceil().max(0.0) as u32;
            let mut sources = h
                .cache
                .take()
                .filter(|(d, c)| *d == uid && crate::compose::Sources::level(c) == level)
                .map_or_else(|| crate::compose::LevelCache::new(level), |(_, c)| c);
            let below = below.to_vec();
            let job = std::thread::Builder::new()
                .name("histogram".into())
                .spawn(move || {
                    let opts = crate::compose::Options {
                        held,
                        later_styles: true,
                        ..Default::default()
                    };
                    let plan = crate::compose::plan(&below, canvas, &mut sources, &opts);
                    sources.retain_used();
                    let b = 1u32 << level;
                    let r = PixelRect {
                        x: 0,
                        y: 0,
                        w: canvas.w.div_ceil(b),
                        h: canvas.h.div_ceil(b),
                    };
                    let histograms = crate::pixel_ops::histogram(&plan.render(r));
                    (sources, histograms)
                });
            if let Ok(job) = job {
                h.job = Some(((uid, layer), job, key));
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
        }
        // Those of another layer are not shown meanwhile.
        h.shown
            .as_ref()
            .filter(|(_, of, _)| *of == (uid, layer))
            .map(|(_, _, h)| *h)
    }

    /// X, Y, W, H and rotation of the active layer. With a transform pending
    /// in the Move tool, they show and edit it; otherwise an edit is applied
    /// right away, as one undo step.
    fn transform_section(&mut self, ui: &mut Ui) {
        let Some(layer_id) = self.doc.as_ref().map(|d| d.active_layer().id) else {
            return;
        };
        // While a stroke paints the layer, the values from before it stay:
        // finding the content's bounds again at every dab scans the whole
        // layer (tens of ms on a photo).
        let cache = Id::new("side_transform_source");
        let cached = ui
            .data(|d| d.get_temp::<(u64, Rect)>(cache))
            .filter(|&(id, _)| self.stroke.is_some() && id == layer_id);
        let src = match cached {
            Some((_, src)) => src,
            None => {
                let Some(src) = self.transform_source() else {
                    return;
                };
                ui.data_mut(|d| d.insert_temp(cache, (layer_id, src)));
                src
            }
        };
        // A distorted frame has no X, Y, W, H and angle to edit.
        let locked = self.transform_locked() || self.transform_distorted();
        let pending = self.tool == Tool::Transform
            && (self.transform_rect.is_some() || self.transform_angle != 0.0);
        let (r, angle) = if pending {
            (self.transform_rect.unwrap_or(src), self.transform_angle)
        } else {
            (src, 0.0)
        };
        // As applied: the frame's edges rounded to pixels.
        let shown = round_rect(r);
        let old = (
            shown.min.x as i64,
            shown.min.y as i64,
            shown.width().max(1.0) as u32,
            shown.height().max(1.0) as u32,
        );
        let (mut x, mut y, mut w, mut h) = old;
        let mut new_angle = angle;
        let mut turn = 0.0;
        let mut keep_ratio = self.keep_ratio;
        // In Quick Mask Mode, the box and the alignment are the mask's.
        let quick = self.in_quick_mask();
        let count = if quick {
            1
        } else {
            self.selected_indices().len()
        };
        let to_selection = !quick && self.selection.is_some();
        let mut align = None;
        let title = match quick {
            true => t!("sel6-side-transform-quick-mask"),
            false => t!("side-transform"),
        };

        section(ui, Some(&title), |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                let half = (ui.available_width() - GAP) / 2.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    ui_kit::number_field(ui, "X", &mut x, -MAX_OFFSET..=MAX_OFFSET, "", half);
                    ui_kit::number_field(ui, "Y", &mut y, -MAX_OFFSET..=MAX_OFFSET, "", half);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    // Resampling: without a pending transform, applied
                    // once the value is final.
                    let size = 1..=MAX_DIMENSION;
                    let (w_label, h_label) = (t!("optbar-width-short"), t!("optbar-height-short"));
                    deferred(ui, Id::new("side_w"), &mut w, pending, |ui, v| {
                        ui_kit::number_field(ui, &w_label, v, size.clone(), "", half)
                    });
                    deferred(ui, Id::new("side_h"), &mut h, pending, |ui, v| {
                        ui_kit::number_field(ui, &h_label, v, size.clone(), "", half)
                    });
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    let field = ui.available_width() - 3.0 * (28.0 + GAP);
                    deferred(
                        ui,
                        Id::new("side_angle"),
                        &mut new_angle,
                        pending,
                        |ui, v| {
                            ui_kit::number_field(
                                ui,
                                icon::ARROW_CLOCKWISE,
                                v,
                                -180.0..=180.0,
                                "°",
                                field,
                            )
                            .on_hover_text(t!("options-rotation"))
                        },
                    );
                    let left = t!("side-rotate-left-tip");
                    if ui_kit::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, &left, true).clicked()
                    {
                        turn = -90.0;
                    }
                    let right = t!("side-rotate-right-tip");
                    if ui_kit::icon_button(ui, icon::ARROW_CLOCKWISE, &right, true).clicked() {
                        turn = 90.0;
                    }
                    let tip = t!("options-constrain-proportions");
                    ui_kit::icon_toggle(ui, icon::LINK, &tip, &mut keep_ratio);
                });
            });
            align = align_row(ui, count, !(locked && count == 1), to_selection);
        });
        self.keep_ratio = keep_ratio;
        if let Some(op) = align {
            self.run(Action::Align(op));
            return;
        }
        if locked {
            return;
        }

        let ratio = src.width() / src.height();
        if keep_ratio && w != old.2 {
            h = ((w as f32 / ratio).round() as u32).max(1);
        } else if keep_ratio && h != old.3 {
            w = ((h as f32 * ratio).round() as u32).max(1);
        }
        let mut target = r;
        if (x, y) != (old.0, old.1) {
            target = Rect::from_min_size(pos2(x as f32, y as f32), r.size());
        }
        if (w, h) != (old.2, old.3) {
            target = Rect::from_center_size(target.center(), vec2(w as f32, h as f32));
        }
        let new_angle = normalize_angle(new_angle + turn);
        if target == r && new_angle == angle {
            return;
        }
        if pending {
            self.transform_rect = Some(target);
            self.transform_angle = new_angle;
        } else if target.size() == r.size() && new_angle == 0.0 {
            // Only moved: no resampling, one undo step for the series.
            let (dx, dy) = (x - old.0, y - old.1);
            self.record_change(t!("xchg-hist-move"), Some((layer_id, "position")));
            if let Some(doc) = &mut self.doc {
                let layer = doc.active_layer_mut();
                layer.offset.0 += dx;
                layer.offset.1 += dy;
            }
        } else {
            // Applied in place: the canvas never follows here.
            let fit = std::mem::replace(&mut self.fit_canvas, false);
            self.transform_rect = Some(target);
            self.transform_angle = new_angle;
            self.apply_transform();
            self.fit_canvas = fit;
        }
    }

    // ---------------------------------------------------------------- layers

    fn layers_section(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, color::LAYERS);
        ui.painter().hline(rect.x_range(), rect.top() + 0.5, line());
        let Some((n, locked)) = self
            .doc
            .as_ref()
            .map(|d| (d.layers.len(), d.active_layer().locked))
        else {
            return;
        };
        let mut lock = locked;
        heading_bar(ui, &t!("panel-layers"), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui_kit::icon_toggle(ui, icon::LOCK_SIMPLE, &t!("side-lock-tip"), &mut lock);
            ui.label(
                egui::RichText::new(t!("side-layer-count", count = n))
                    .font(ui_kit::mono(12.0))
                    .color(color::LABEL),
            );
        });
        if lock != locked {
            self.run(Action::ToggleLock);
        }
        // Right under the heading (not after its spacing): the list starts
        // below it.
        let top = rect.top() + HEADING_HEIGHT + BLEND_ROW;
        let blend = Rect::from_x_y_ranges(rect.x_range(), rect.top() + HEADING_HEIGHT..=top);
        self.blend_row(&mut ui.new_child(UiBuilder::new().max_rect(blend)));
        let footer = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FOOTER_HEIGHT), rect.max);
        let list = Rect::from_min_max(
            pos2(rect.left() + 6.0, top),
            pos2(rect.right() - 6.0, (footer.top() - 6.0).max(top)),
        );
        self.layer_rows(&mut ui.new_child(UiBuilder::new().max_rect(list)));
        ui.painter()
            .hline(footer.x_range(), footer.top() + 0.5, line());
        let mut buttons = ui.new_child(
            UiBuilder::new()
                .max_rect(footer.shrink2(vec2(8.0, 6.0)).translate(vec2(0.0, 0.5)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        self.layer_buttons(&mut buttons);
    }

    /// The selected layers are all hidden (Layer › Show Layers).
    pub(super) fn selection_hidden(&self) -> bool {
        let Some(doc) = &self.doc else { return false };
        self.selected_indices()
            .iter()
            .all(|&i| !doc.layers[i].visible)
    }

    /// Layer › Hide Layers / Show Layers (Ctrl+,): the selected layers all
    /// hidden, or all shown again when they all were; one step.
    pub(super) fn toggle_visibility(&mut self) {
        // A text being typed ends first (an empty new one goes away).
        self.commit_text();
        let show = self.selection_hidden();
        let picked = self.selected_indices();
        if picked.is_empty() {
            return;
        }
        // Several: "Hide Layers", like the menu.
        let step = match (show, picked.len() > 1) {
            (true, false) => t!("xchg-hist-show-layer"),
            (false, false) => t!("xchg-hist-hide-layer"),
            (true, true) => t!("ui5-menu-show-layers"),
            (false, true) => t!("ui5-menu-hide-layers"),
        };
        self.record_change(step, None);
        if let Some(doc) = &mut self.doc {
            for i in picked {
                doc.layers[i].visible = show;
            }
        }
    }

    /// Blend mode and opacity of the active layer, above the list (as in
    /// the reference editor); a change is one step.
    fn blend_row(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        // The changes go to the selected layers that aren't locked.
        let locked = self.selected_indices().into_iter().all(|i| doc.is_locked(i));
        let (layer_id, group) = (layer.id, layer.is_group());
        let (mut opacity, mut blend) = (layer.opacity, layer.blend);
        let row = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), BLEND_ROW));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(row.shrink2(vec2(14.0, 4.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let hovered = child
            .add_enabled_ui(!locked, |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                let half = (ui.available_width() - GAP) / 2.0;
                let hovered = blend_combo(ui, "side_blend", &mut blend, group, half);
                let text = i18n::percent(format!("{:.0}", opacity * 100.0));
                ui_kit::scrub(ui, &t!("panel-opacity"), &mut opacity, 0.0..=1.0, &text, half);
                hovered
            })
            .inner;
        ui.advance_cursor_after_rect(row);
        // (The Layer Style window has its own list.)
        if self.layer_style.is_none() {
            self.blend_preview = hovered.map(|m| (layer_id, m));
        }
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            return;
        };
        let opacity = (opacity != layer.opacity).then_some(opacity);
        let blend = (blend != layer.blend).then_some(blend);
        if opacity.is_none() && blend.is_none() {
            return;
        }
        // A text being typed ends first (an empty new one goes away).
        self.commit_text();
        if self
            .doc
            .as_ref()
            .is_none_or(|d| d.active_layer().id != layer_id)
        {
            return;
        }
        self.set_layers_look(opacity, blend);
    }

    fn layer_rows(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else { return };
        let active = doc.active;
        let shown = shown_rows(doc);
        let picked = self.selected_indices();
        let target_mask = doc.mask_targeted();
        let filter_mask = doc.filter_mask_targeted();
        let mut click = None;
        let mut rename = false;
        let mut visibility = None;
        let mut unlock = None;
        let mut fold = None;
        let mut drag_started = None;
        let mut menu_action = None;
        let layer_menu = self.layer_menu();
        // Rows of the list, in display order (top first), and their layer.
        let mut rows: Vec<(Rect, usize)> = Vec::with_capacity(shown.len());
        let list_drag = self.list_drag;
        let height = ui.available_height();
        let ctx = ui.ctx().clone();
        let thumbs: Vec<(Option<egui::TextureId>, Option<egui::TextureId>)> = shown
            .iter()
            .map(|&i| {
                (
                    self.thumbnail(&ctx, i, false),
                    self.thumbnail(&ctx, i, true),
                )
            })
            .collect();
        let filter_masks = self.filter_mask_thumbs(&ctx, &shown);
        let mut filter_rows = smart_rows::FilterRows::default();
        let Some(doc) = &self.doc else { return };
        egui::ScrollArea::vertical()
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = ROW_GAP;
                for (&i, (thumb_tex, mask_tex)) in shown.iter().zip(&thumbs) {
                    let layer = &doc.layers[i];
                    let (rect, row) = ui.allocate_exact_size(
                        vec2(ui.available_width(), ROW_HEIGHT),
                        Sense::click_and_drag(),
                    );
                    let painter = ui.painter().clone();
                    let hovered = ui.rect_contains_pointer(rect);
                    let fill = if i == active {
                        color::ACCENT_SOFT
                    } else if picked.contains(&i) {
                        color::ACCENT_SOFT.gamma_multiply(0.5)
                    } else if hovered {
                        color::BG3
                    } else {
                        Color32::TRANSPARENT
                    };
                    painter.rect_filled(rect, 8.0, fill);

                    // Visibility (Alt + click: this one only).
                    let eye = Rect::from_min_size(
                        pos2(rect.left() + 4.0, rect.center().y - 14.0),
                        Vec2::splat(28.0),
                    );
                    // Shown, but in a hidden group: its eye dimmed.
                    let (glyph, fg) = match layer.visible {
                        true if hidden_above(doc, i) => (icon::EYE, color::DISABLED),
                        true => (icon::EYE, color::MUTED),
                        false => (icon::EYE_SLASH, color::DISABLED),
                    };
                    let tip = t!(
                        "input3-visibility-tip",
                        alt_click = keys::click(ui.ctx(), Modifiers::ALT)
                    );
                    if row_icon(ui, eye, Id::new(("side_eye", layer.id)), glyph, fg, &tip).clicked()
                    {
                        let solo = ui.input(|inp| inp.modifiers.alt);
                        visibility = Some((i, !layer.visible, solo));
                    }

                    // Nesting: indentation, groups' disclosure, clipping arrow.
                    let mut x = eye.right() + layer.depth as f32 * INDENT;
                    let caret =
                        Rect::from_min_size(pos2(x, rect.center().y - 9.0), Vec2::splat(18.0));
                    if let LayerKind::Group { expanded } = layer.kind {
                        let glyph = if expanded {
                            icon::CARET_DOWN
                        } else {
                            icon::CARET_RIGHT
                        };
                        let tip = t!("core-group-fold-tip");
                        if row_icon(
                            ui,
                            caret,
                            Id::new(("side_fold", layer.id)),
                            glyph,
                            color::MUTED,
                            &tip,
                        )
                        .clicked()
                        {
                            fold = Some(i);
                        }
                    }
                    x = caret.right();
                    if layer.clipped && !layer.is_group() {
                        painter.text(
                            pos2(x + 6.0, rect.center().y),
                            Align2::CENTER_CENTER,
                            icon::ARROW_ELBOW_LEFT_DOWN,
                            FontId::proportional(13.0),
                            color::MUTED,
                        );
                        x += 14.0;
                    }

                    // Thumbnail, and the mask's with the chain between them.
                    let thumb = Rect::from_min_size(
                        pos2(x + 2.0, rect.center().y - 13.0),
                        vec2(38.0, 26.0),
                    );
                    let clipped = painter.with_clip_rect(thumb.intersect(painter.clip_rect()));
                    draw_thumb(&clipped, thumb, layer, *thumb_tex);
                    smart_rows::badge(&clipped, thumb, layer);
                    let targeted = i == active && !target_mask && !layer.is_group();
                    thumb_frame(&painter, thumb, targeted);
                    let mut right_of_thumbs = thumb.right();
                    let mut mask_rect = None;
                    let mut chain_rect = None;
                    if let Some(m) = &layer.mask {
                        let chain = Rect::from_min_size(
                            pos2(thumb.right(), rect.center().y - 8.0),
                            vec2(14.0, 16.0),
                        );
                        let mr =
                            Rect::from_min_size(pos2(chain.right(), thumb.top()), thumb.size());
                        if m.linked() {
                            painter.text(
                                chain.center(),
                                Align2::CENTER_CENTER,
                                icon::LINK_SIMPLE,
                                FontId::proportional(11.0),
                                color::LABEL,
                            );
                        }
                        let clipped = painter.with_clip_rect(mr.intersect(painter.clip_rect()));
                        clipped.rect_filled(mr, 4.0, Color32::from_gray(m.default));
                        if let Some(tex) = mask_tex {
                            let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                            clipped.image(
                                *tex,
                                fit_rect(mr, doc.width(), doc.height()),
                                uv,
                                Color32::WHITE,
                            );
                        }
                        if !m.enabled {
                            let stroke = UiStroke::new(2.0, Color32::from_rgb(0xe5, 0x48, 0x4d));
                            clipped.line_segment([mr.left_top(), mr.right_bottom()], stroke);
                            clipped.line_segment([mr.left_bottom(), mr.right_top()], stroke);
                        }
                        thumb_frame(
                            &painter,
                            mr,
                            i == active && ((target_mask && !filter_mask) || layer.is_group()),
                        );
                        right_of_thumbs = mr.right();
                        (mask_rect, chain_rect) = (Some(mr), Some(chain));
                    }

                    // Lock (shown when locked: a click unlocks), styles.
                    let mut right = rect.right() - 6.0;
                    if layer.locked {
                        let lock = Rect::from_min_size(
                            pos2(right - 24.0, rect.center().y - 12.0),
                            Vec2::splat(24.0),
                        );
                        let tip = t!("panel-unlock-tip");
                        let glyph = icon::LOCK_SIMPLE;
                        if row_icon(
                            ui,
                            lock,
                            Id::new(("side_lock", layer.id)),
                            glyph,
                            color::MUTED,
                            &tip,
                        )
                        .clicked()
                        {
                            unlock = Some(i);
                        }
                        right = lock.left() - 4.0;
                    } else if let Some(g) = doc.locking_group(i) {
                        // Locked with its group: dimmed, the group unlocks.
                        let lock = Rect::from_min_size(
                            pos2(right - 24.0, rect.center().y - 12.0),
                            Vec2::splat(24.0),
                        );
                        painter.text(
                            lock.center(),
                            Align2::CENTER_CENTER,
                            icon::LOCK_SIMPLE,
                            FontId::proportional(13.0),
                            color::DISABLED,
                        );
                        let tip = t!("tools6-panel-group-lock-tip", group = doc.layers[g].name.as_str());
                        ui.interact(lock, Id::new(("side_group_lock", layer.id)), Sense::hover())
                            .on_hover_text(tip);
                        right = lock.left() - 4.0;
                    }
                    if layer.has_pixels() && layer.effects.any() {
                        let fx = painter.text(
                            pos2(right - 2.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            "fx",
                            ui_kit::bold(11.0),
                            color::MUTED,
                        );
                        right = fx.left() - 6.0;
                    }

                    // Name and kind.
                    let text = Rect::from_min_max(
                        pos2(right_of_thumbs + 10.0, rect.top()),
                        pos2(right, rect.bottom()),
                    );
                    let name_color = if !layer.visible || list_drag == Some(i) {
                        color::LABEL
                    } else {
                        color::TEXT
                    };
                    let kind = kind_label(layer);
                    let kind = if layer.locked {
                        t!("side-kind-locked", kind = kind)
                    } else {
                        kind
                    };
                    // Too long, the name and kind end with "…" (the tooltip
                    // shows the name whole).
                    let line = |s: String, font: FontId, color: Color32| {
                        let mut job = egui::text::LayoutJob::single_section(
                            s,
                            egui::TextFormat::simple(font, color),
                        );
                        job.wrap =
                            egui::text::TextWrapping::truncate_at_width(text.width().max(0.0));
                        painter.layout_job(job)
                    };
                    let name_galley = line(
                        layer.name.clone(),
                        ui_kit::medium(ui_kit::TEXT_SIZE),
                        name_color,
                    );
                    let name_elided = name_galley.elided;
                    let kind_galley = line(kind, FontId::proportional(11.0), color::LABEL);
                    let text_painter = painter.with_clip_rect(text.intersect(painter.clip_rect()));
                    let name_y = rect.center().y - 7.0 - name_galley.size().y / 2.0;
                    let name_rect =
                        Rect::from_min_size(pos2(text.left(), name_y), name_galley.size());
                    text_painter.galley(pos2(text.left(), name_y), name_galley, name_color);
                    let kind_y = rect.center().y + 9.0 - kind_galley.size().y / 2.0;
                    text_painter.galley(pos2(text.left(), kind_y), kind_galley, color::LABEL);

                    let tip = layer_row_tip(ui.ctx());
                    let row = if name_elided {
                        row.on_hover_ui(|ui| {
                            ui.label(egui::RichText::new(&layer.name).strong());
                            ui.label(tip);
                        })
                    } else {
                        row.on_hover_text(tip)
                    };
                    let modifiers = ui.input(|inp| inp.modifiers);
                    let at = row.interact_pointer_pos();
                    let on = |r: Option<Rect>| {
                        at.is_some_and(|p| r.is_some_and(|r| r.expand(2.0).contains(p)))
                    };
                    // Alt over the line between two rows: clipping the one
                    // above to the one below, like Photoshop (its cursor
                    // says so before the click).
                    let clip_line = |p: Pos2| {
                        if (p.y - rect.bottom()).abs() <= CLIP_LINE {
                            Some(i)
                        } else if (p.y - rect.top()).abs() <= CLIP_LINE && i + 1 < doc.layers.len()
                        {
                            Some(i + 1)
                        } else {
                            None
                        }
                    };
                    if modifiers.alt
                        && !modifiers.command
                        && row.hover_pos().is_some_and(|p| clip_line(p).is_some())
                    {
                        ui.ctx().set_cursor_icon(CursorIcon::Alias);
                    }
                    let clip = at.filter(|_| modifiers.alt).and_then(clip_line);
                    let part = if let Some(above) = clip {
                        RowPart::ClipLine(above)
                    } else if on(mask_rect) {
                        RowPart::Mask
                    } else if on(chain_rect) {
                        RowPart::Chain
                    } else if on(Some(thumb)) {
                        RowPart::Thumbnail
                    } else {
                        RowPart::Name
                    };
                    // Double-click: on the name, renames it; beside it, the
                    // layer's style, like Photoshop.
                    let on_name = on(Some(name_rect)) || !layer.has_pixels();
                    if row.double_clicked() {
                        if part == RowPart::Name && on_name {
                            rename = true;
                        } else if layer.artboard.is_some() {
                            // Zoom to fit the artboard.
                            let fit = artboard_tool::Command::Fit(i);
                            menu_action = Some(Action::Artboard(fit));
                        } else if part == RowPart::Thumbnail && layer.is_smart() {
                            let edit = smart_ui::Command::EditContents;
                            menu_action = Some(Action::Smart(edit));
                        } else if part == RowPart::Mask {
                            // The mask, in Properties (its density and
                            // feather), not the layer's style.
                            click = Some((i, RowPart::Mask, Modifiers::NONE));
                        } else if layer.has_pixels() {
                            menu_action = Some(Action::LayerStyle(None));
                        }
                    } else if row.clicked() {
                        click = Some((i, part, modifiers));
                    }
                    if row.drag_started() {
                        drag_started = Some(i);
                    }
                    // Right click: the layer's menu, for that layer.
                    if row.secondary_clicked() && !picked.contains(&i) {
                        click = Some((i, RowPart::Name, Modifiers::NONE));
                    }
                    row.context_menu(|ui| {
                        super::menus::menu_entries(ui, &layer_menu, &mut menu_action);
                    });
                    rows.push((rect, i));
                    smart_rows::filter_rows(
                        ui,
                        doc,
                        i,
                        (rect.left() + 4.0, thumb.left()),
                        filter_masks.get(&i).copied(),
                        &mut filter_rows,
                        &mut click,
                        &mut menu_action,
                    );
                }
            });
        self.drag_filter_rows(ui, filter_rows, &mut menu_action);

        // Reordering: the insertion line follows the pointer.
        if let Some(i) = drag_started {
            self.list_drag = Some(i);
        }
        let mut reorder = None;
        let mut into_group = None;
        if let Some(from) = self.list_drag {
            let (pointer, down) =
                ui.input(|inp| (inp.pointer.interact_pos(), inp.pointer.primary_down()));
            // Over the middle of a group's row: into it, at its top (a
            // closed one too), like Photoshop.
            let group = pointer.and_then(|p| {
                let doc = self.doc.as_ref()?;
                rows.iter().copied().find(|&(r, i)| {
                    let middle = r.shrink2(vec2(0.0, r.height() / 4.0));
                    i != from && doc.layers[i].is_group() && middle.contains(p)
                })
            });
            // `gap`: insertion position in display order (0 = very top).
            let gap = pointer
                .filter(|_| group.is_none())
                .map(|p| rows.iter().filter(|(r, _)| p.y > r.center().y).count());
            if let Some((r, _)) = group {
                let stroke = UiStroke::new(2.0, color::ACCENT);
                ui.painter()
                    .rect_stroke(r, 8.0, stroke, egui::StrokeKind::Inside);
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            if let Some(gap) = gap {
                let y = rows.get(gap).map_or_else(
                    || rows.last().map_or(0.0, |(r, _)| r.bottom() + 1.0),
                    |(r, _)| r.top() - 1.0,
                );
                // Indented like where it goes.
                let depth = match (gap.checked_sub(1), &self.doc) {
                    (Some(g), Some(d)) => {
                        let a = &d.layers[rows[g].1];
                        a.depth + u32::from(matches!(a.kind, LayerKind::Group { expanded: true }))
                    }
                    _ => 0,
                };
                let x = rows
                    .first()
                    .map_or(ui.min_rect().x_range(), |(r, _)| r.x_range());
                let x = (x.min + 50.0 + depth as f32 * INDENT).min(x.max)..=x.max;
                ui.painter().hline(x, y, UiStroke::new(2.0, color::ACCENT));
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
            if !down {
                self.list_drag = None;
                if let Some(gap) = gap {
                    reorder = Some((from, gap.checked_sub(1).map(|g| rows[g].1)));
                }
                into_group = group.map(|(_, g)| (from, g));
            }
        }

        // A text being typed ends first; an empty new one goes away, so the
        // rows are found again by layer.
        let ids = self.doc.as_ref().map(|d| {
            let id = |i: usize| d.layers.get(i).map(|l| l.id);
            let click = click.and_then(|c: (usize, _, _)| id(c.0));
            (
                visibility.and_then(|v| id(v.0)),
                fold.and_then(id),
                unlock.and_then(id),
                click,
            )
        });
        if (visibility.is_some() || unlock.is_some()) && self.type_tool.edit.is_some() {
            self.commit_text();
            let find = |id: Option<u64>| {
                let d = self.doc.as_ref()?;
                d.layers.iter().position(|l| Some(l.id) == id)
            };
            let (v, f, u, c) = ids.unwrap_or_default();
            visibility = visibility.and_then(|(_, visible, solo)| Some((find(v)?, visible, solo)));
            fold = fold.and(find(f));
            unlock = unlock.and(find(u));
            click = click.and_then(|(_, part, modifiers)| Some((find(c)?, part, modifiers)));
        }
        if let Some((i, visible, solo)) = visibility {
            let step = if visible && !solo {
                t!("xchg-hist-show-layer")
            } else {
                t!("xchg-hist-hide-layer")
            };
            self.record_change(step, None);
            if let Some(doc) = &mut self.doc {
                if solo {
                    // Alt + click, like Photoshop: only this one visible, or
                    // all shown again if it already was.
                    let alone = doc
                        .layers
                        .iter()
                        .enumerate()
                        .all(|(j, l)| (j == i) == l.visible);
                    for (j, l) in doc.layers.iter_mut().enumerate() {
                        l.visible = alone || j == i;
                    }
                } else {
                    doc.layers[i].visible = visible;
                }
            }
        }
        if let Some(i) = fold
            && let Some(doc) = &mut self.doc
            && let LayerKind::Group { expanded } = &mut doc.layers[i].kind
        {
            // Not an undo step, like Photoshop (and undo keeps it).
            *expanded = !*expanded;
            doc.mark_modified();
        }
        if let Some((i, part, modifiers)) = click {
            self.click_row(i, part, modifiers);
        }
        if let Some(action) = menu_action {
            self.run(action);
        }
        if rename {
            self.focus_name = true;
        }
        if let Some(i) = unlock {
            self.record_change(t!("xchg-hist-unlock-layer"), None);
            if let Some(doc) = &mut self.doc {
                let layer = &mut doc.layers[i];
                layer.locked = false;
                let name = layer.name.as_str();
                self.status = format!(
                    "{} {}",
                    icon::LOCK_SIMPLE_OPEN,
                    t!("status-layer-unlocked", name = name)
                );
            }
        }
        // What is pending is applied first (an empty new text goes away):
        // the layers are found again by id, and the drop is given up if
        // one of them is gone.
        let id_of = |app: &Self, i: usize| app.doc.as_ref()?.layers.get(i).map(|l| l.id);
        let index_of = |app: &Self, id: u64| {
            let d = app.doc.as_ref()?;
            d.layers.iter().position(|l| l.id == id)
        };
        if let Some((from, above)) = reorder {
            let ids = id_of(self, from).zip(above.map_or(Some(None), |a| id_of(self, a).map(Some)));
            self.commit_pending_but_crop();
            let found = ids.and_then(|(from, above)| {
                let above = match above {
                    Some(a) => Some(index_of(self, a)?),
                    None => None,
                };
                Some((index_of(self, from)?, above))
            });
            if let Some((from, above)) = found
                && let Some(doc) = &mut self.doc
                && doc.drop_layer(from, above)
            {
                self.reset_pending_but_crop();
            }
        }
        if let Some((from, group)) = into_group {
            let ids = id_of(self, from).zip(id_of(self, group));
            self.commit_pending_but_crop();
            let found = ids.and_then(|(from, group)| {
                Some((index_of(self, from)?, index_of(self, group)?))
            });
            if let Some((from, group)) = found
                && let Some(doc) = &mut self.doc
                && doc.drop_into(from, group)
            {
                self.reset_pending_but_crop();
            }
        }
    }

    /// A click on a row of the list: on the name (Alt: clipping to the
    /// layer below), the thumbnail (Ctrl: its pixels as the selection), the
    /// mask (targets it; Shift: on / off, Alt: shown alone, Ctrl: as the
    /// selection), the chain (link).
    pub(super) fn click_row(&mut self, i: usize, part: RowPart, modifiers: Modifiers) {
        match part {
            RowPart::ClipLine(above) if !modifiers.command => {
                if let Some(doc) = &mut self.doc
                    && above < doc.layers.len()
                {
                    let clip = !doc.layers[above].clipped;
                    if doc.set_clipping(above, clip) {
                        self.reset_pending();
                    }
                }
                return;
            }
            RowPart::Thumbnail if modifiers.command => return self.select_layer_pixels(i),
            RowPart::Mask if modifiers.command => {
                if let Some(found) = self.doc.as_ref().and_then(|d| d.mask_selection(i)) {
                    let mode = self.selection_mode(modifiers.shift, modifiers.alt);
                    self.sel.step_name = Some(select_menu::step_label(t!("sel-menu-load")));
                    self.apply_selection(Some(found), mode);
                }
                return;
            }
            _ => {}
        }
        if !(part == RowPart::Name && (modifiers.command || modifiers.shift)) {
            self.click_layer(i, Modifiers::NONE);
        } else {
            self.click_layer(i, modifiers);
        }
        let Some(doc) = &mut self.doc else { return };
        if doc.active != i {
            return;
        }
        match part {
            RowPart::Mask if modifiers.shift => {
                let on = doc.layers[i].mask.as_ref().is_some_and(|m| !m.enabled);
                doc.set_mask_enabled(on);
            }
            RowPart::Mask if modifiers.alt => {
                doc.target_mask(true);
                self.mask_view = !self.mask_view;
            }
            RowPart::Mask => {
                doc.target_mask(true);
                self.mask_view = false;
            }
            RowPart::Chain => {
                let linked = doc.layers[i].mask.as_ref().is_some_and(|m| m.linked());
                doc.set_mask_linked(!linked);
            }
            RowPart::Thumbnail => {
                doc.target_mask(false);
                self.mask_view = false;
            }
            RowPart::Name | RowPart::ClipLine(_) => {}
        }
    }

    /// Link, styles, mask, adjustment, group, new … delete (Photoshop's
    /// buttons).
    fn layer_buttons(&mut self, ui: &mut Ui) {
        let Some(doc) = &self.doc else { return };
        let n = doc.layers.len();
        let layer = doc.active_layer();
        let (pixels, has_mask) = (layer.has_pixels(), layer.mask.is_some());
        let linked = layer.link.is_some();
        let several = self.selected_indices().len() > 1;
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut action = None;
        let tip = if linked && !several {
            t!("core-unlink-layers-tip")
        } else {
            t!("core-link-layers-tip")
        };
        if ui_kit::icon_button(ui, icon::LINK, &tip, several || linked).clicked() {
            action = Some(Action::LinkLayers);
        }
        let fx = ui_kit::icon_button(ui, icon::SPARKLE, &t!("core-styles-tip"), pixels);
        egui::Popup::menu(&fx).show(|ui| {
            let entries = self.style_menu();
            super::menus::menu_entries(ui, &entries, &mut action);
        });
        let alt = ui.input(|i| i.modifiers.alt);
        let mask_tip = t!(
            "input3-add-mask-tip",
            alt = keys::modifiers(ui.ctx(), Modifiers::ALT)
        );
        if ui_kit::icon_button(ui, icon::SQUARE_HALF, &mask_tip, !has_mask).clicked() {
            // Alt: hiding all (or the selection), like Photoshop.
            action = Some(Action::AddMask(match (alt, self.selection.is_some()) {
                (false, false) => NewMask::RevealAll,
                (true, false) => NewMask::HideAll,
                (false, true) => NewMask::RevealSelection,
                (true, true) => NewMask::HideSelection,
            }));
        }
        let adjust =
            ui_kit::icon_button(ui, icon::CIRCLE_HALF, &t!("core-new-adjustment-tip"), true);
        egui::Popup::menu(&adjust).show(|ui| {
            let entries = self.new_fill_or_adjustment_menu();
            super::menus::menu_entries(ui, &entries, &mut action);
        });
        for (glyph, tip, shortcut, act) in [
            (
                icon::FOLDER_SIMPLE,
                t!("core-new-group-tip"),
                None,
                Action::NewGroup,
            ),
            (
                icon::PLUS_SQUARE,
                t!("panel-new-layer-tip"),
                Some(SHORTCUT_NEW_LAYER),
                Action::NewLayer,
            ),
        ] {
            let response = ui_kit::icon_button(ui, glyph, "", true);
            let key = shortcut.map_or(String::new(), |s| keys::shortcut(ui.ctx(), &s));
            if ui_kit::tip_with_key(response, &tip, &key).clicked() {
                action = Some(act);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tip = t!("panel-delete-layer-tip");
            let deletable = self
                .doc
                .as_ref()
                .is_some_and(|d| d.block(d.active).len() < n);
            if ui_kit::icon_button(ui, icon::TRASH, &tip, deletable).clicked() {
                action = Some(Action::Delete);
            }
        });
        if let Some(action) = action {
            self.run(action);
        }
    }

    /// Thumbnail of layer `i` (`mask`: of its mask) as a texture, made again
    /// when what it shows changes (at most every quarter second during a
    /// brush stroke).
    fn thumbnail(&mut self, ctx: &egui::Context, i: usize, mask: bool) -> Option<egui::TextureId> {
        let doc = self.doc.as_ref()?;
        let layer = doc.render_layer(i);
        // The pixels as the canvas shows them (display profile, proof).
        let (lut_key, lut) = self.color.shown();
        let key = match (&layer.mask, mask) {
            (Some(m), true) => hash_of((m.rev(), m.origin(layer.offset), doc.full_rect())),
            (None, true) => return None,
            _ if !layer.has_pixels() || layer.text.is_some() => return None,
            _ => hash_of((layer.rev(), lut_key)),
        };
        let now = ctx.input(|inp| inp.time);
        let entry = (layer.id, mask);
        if let Some((k, at, tex)) = self.thumbs.get(&entry)
            && (*k == key || (self.stroke.is_some() && now - at < 0.25))
        {
            return Some(tex.id());
        }
        let ppp = ctx.pixels_per_point();
        let (tw, th) = ((38.0 * ppp).round() as u32, (26.0 * ppp).round() as u32);
        let image = match &layer.mask {
            Some(m) if mask => mask_thumbnail(m, layer.offset, doc.full_rect(), tw, th),
            _ => {
                let mut image = layer_thumbnail(layer.image(), tw, th);
                color_ui::show_through(lut.as_deref(), image.as_mut(), false);
                image
            }
        };
        let image = ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        );
        match self.thumbs.get_mut(&entry) {
            Some(t) => {
                t.2.set(image, IMAGE_TEX_OPTIONS);
                (t.0, t.1) = (key, now);
            }
            None => {
                let name = format!("thumb-{}-{mask}", layer.id);
                let tex = ctx.load_texture(name, image, IMAGE_TEX_OPTIONS);
                self.thumbs.insert(entry, (key, now, tex));
            }
        }
        self.thumbs.get(&entry).map(|t| t.2.id())
    }
}

/// Name of the history step of an edit of `prop` (see `properties`) of
/// `layer` in the Properties panel.
fn property_step(prop: &str, layer: &Layer) -> String {
    match prop {
        "name" => t!("xchg-hist-rename-layer"),
        "opacity" => t!("xchg-hist-opacity"),
        "fill" => t!("xchg-hist-fill-opacity"),
        "blend" => t!("xchg-hist-blending"),
        "styles" => t!("core-menu-layer-style"),
        "content" => t!("xchg-hist-modify-layer", name = kind_label(layer)),
        _ => t!("xchg-hist-mask-properties"),
    }
}

/// Where a click on a row of the Layers list landed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowPart {
    Name,
    Thumbnail,
    Chain,
    Mask,
    /// Alt on the line between two rows: the layer above it (clipping).
    ClipLine(usize),
}

/// How far from the line between two rows (points) Alt+click clips.
const CLIP_LINE: f32 = 4.0;

/// Indentation of one level of group in the list.
const INDENT: f32 = 16.0;

/// A group holding layer `i` is hidden.
fn hidden_above(doc: &Document, i: usize) -> bool {
    std::iter::successors(doc.parent(i), |&j| doc.parent(j)).any(|j| !doc.layers[j].visible)
}

/// The layers the list shows, top first (those in closed groups aside).
fn shown_rows(doc: &Document) -> Vec<usize> {
    (0..doc.layers.len())
        .rev()
        .filter(|&i| !doc.folded(i))
        .collect()
}

/// Thumbnail of a layer's row: its pixels, or what it is.
fn draw_thumb(painter: &egui::Painter, thumb: Rect, layer: &Layer, tex: Option<egui::TextureId>) {
    let glyph = |painter: &egui::Painter, g: &str| {
        painter.rect_filled(thumb, 4.0, ui_kit::color::THUMB_EMPTY);
        painter.text(
            thumb.center(),
            Align2::CENTER_CENTER,
            g,
            FontId::proportional(15.0),
            color::TEXT,
        );
    };
    match &layer.kind {
        LayerKind::Pixels if layer.text.is_some() => {
            painter.rect_filled(thumb, 4.0, ui_kit::color::THUMB_EMPTY);
            painter.text(
                thumb.center(),
                Align2::CENTER_CENTER,
                "T",
                ui_kit::bold(14.0),
                color::TEXT,
            );
        }
        LayerKind::Pixels => {
            draw_checker(painter, thumb);
            if let Some(tex) = tex {
                let size = (layer.image().width(), layer.image().height());
                let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                painter.image(tex, fit_rect(thumb, size.0, size.1), uv, Color32::WHITE);
            }
        }
        LayerKind::Group { .. } if layer.artboard.is_some() => glyph(painter, icon::FRAME_CORNERS),
        LayerKind::Group { expanded } => glyph(
            painter,
            if *expanded {
                icon::FOLDER_OPEN
            } else {
                icon::FOLDER
            },
        ),
        LayerKind::Adjustment(a) => glyph(painter, adjustment_icon(a.kind())),
        LayerKind::Fill(Fill::Solid(c)) => {
            painter.rect_filled(thumb, 4.0, Color32::from_rgb(c[0], c[1], c[2]));
        }
        LayerKind::Fill(Fill::Gradient(g)) => {
            super::gradient_editor::paint_ramp(painter, thumb, &g.gradient, [0; 3], [255; 3], true);
        }
    }
}

/// Border of a thumbnail: white and thick when the tools edit it.
fn thumb_frame(painter: &egui::Painter, r: Rect, targeted: bool) {
    let stroke = if targeted {
        UiStroke::new(2.0, Color32::WHITE)
    } else {
        UiStroke::new(1.0, color::LINE_HOVER)
    };
    painter.rect_stroke(r, 4.0, stroke, StrokeKind::Inside);
}

/// `w` × `h` fitted in `r`, centered.
fn fit_rect(r: Rect, w: u32, h: u32) -> Rect {
    let size = vec2(w.max(1) as f32, h.max(1) as f32);
    let scale = (r.width() / size.x).min(r.height() / size.y);
    Rect::from_center_size(r.center(), size * scale)
}

/// Icon of an adjustment (Layers list, menus).
pub(super) fn adjustment_icon(kind: AdjustKind) -> &'static str {
    ADJUSTMENTS
        .iter()
        .find(|a| a.kind == kind)
        .map_or(icon::CIRCLE_HALF, |a| a.icon)
}

/// `image` reduced to fit `w` × `h` (each pixel the average of a few
/// samples of its block, in premultiplied colors).
fn layer_thumbnail(image: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    let (iw, ih) = image.dimensions();
    if iw == 0 || ih == 0 {
        return image::RgbaImage::new(1, 1);
    }
    let scale = (w as f32 / iw as f32).min(h as f32 / ih as f32).min(1.0);
    let (tw, th) = (
        ((iw as f32 * scale).round() as u32).max(1),
        ((ih as f32 * scale).round() as u32).max(1),
    );
    const SAMPLES: u32 = 4;
    image::RgbaImage::from_fn(tw, th, |x, y| {
        let mut sum = [0u32; 4];
        for sy in 0..SAMPLES {
            for sx in 0..SAMPLES {
                let px = (((x * SAMPLES + sx) as u64 * iw as u64) / (tw * SAMPLES) as u64) as u32;
                let py = (((y * SAMPLES + sy) as u64 * ih as u64) / (th * SAMPLES) as u64) as u32;
                let p = image.get_pixel(px.min(iw - 1), py.min(ih - 1)).0;
                let a = p[3] as u32;
                for c in 0..3 {
                    sum[c] += p[c] as u32 * a;
                }
                sum[3] += a;
            }
        }
        let n = SAMPLES * SAMPLES;
        let c = |v: u32| v.checked_div(sum[3]).unwrap_or(0) as u8;
        image::Rgba([c(sum[0]), c(sum[1]), c(sum[2]), (sum[3] / n) as u8])
    })
}

/// A mask over the canvas `canvas`, reduced to fit `w` × `h`.
fn mask_thumbnail(
    mask: &crate::document::Mask,
    layer_offset: (i64, i64),
    canvas: PixelRect,
    w: u32,
    h: u32,
) -> image::RgbaImage {
    let scale = (w as f32 / canvas.w.max(1) as f32).min(h as f32 / canvas.h.max(1) as f32);
    let (tw, th) = (
        ((canvas.w as f32 * scale).round() as u32).max(1),
        ((canvas.h as f32 * scale).round() as u32).max(1),
    );
    let (ox, oy) = mask.origin(layer_offset);
    let image = mask.image();
    image::RgbaImage::from_fn(tw, th, |x, y| {
        let cx = canvas.x + ((x as f32 + 0.5) / scale) as i64 - ox;
        let cy = canvas.y + ((y as f32 + 0.5) / scale) as i64 - oy;
        let inside =
            cx >= 0 && cy >= 0 && (cx as u32) < image.width() && (cy as u32) < image.height();
        let v = if inside {
            image.get_pixel(cx as u32, cy as u32).0[0]
        } else {
            mask.default
        };
        image::Rgba([v, v, v, 255])
    })
}

// ------------------------------------------------------------------ helpers

fn line() -> UiStroke {
    UiStroke::new(1.0, color::LINE)
}

/// What a layer is: "Text", "Image", "Group", its adjustment, "Fill".
/// Help of a row of the Layers panel.
fn layer_row_tip(ctx: &egui::Context) -> String {
    t!(
        "input4-layer-row-tip",
        alt_click = keys::click(ctx, Modifiers::ALT),
        ctrl_click = keys::click(ctx, Modifiers::COMMAND),
        shift_click = keys::click(ctx, Modifiers::SHIFT)
    )
}

fn kind_label(layer: &Layer) -> String {
    match &layer.kind {
        LayerKind::Pixels if layer.text.is_some() => t!("side-kind-text"),
        LayerKind::Pixels if layer.shape.is_some() => t!("vector-kind-shape"),
        LayerKind::Pixels if layer.smart.is_some() => t!("smart-kind"),
        LayerKind::Pixels => t!("side-kind-image"),
        LayerKind::Group { .. } if layer.artboard.is_some() => t!("artboard-kind"),
        LayerKind::Group { .. } => t!("core-kind-group"),
        LayerKind::Adjustment(a) => a.title(),
        LayerKind::Fill(_) => t!("core-kind-fill"),
    }
}

/// Panel heading, 42 px: title on the left, `right` aligned on the right.
fn heading_bar(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui)) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADING_HEIGHT), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, title));
    ui.painter().text(
        pos2(rect.left() + 14.0, rect.center().y),
        Align2::LEFT_CENTER,
        title,
        ui_kit::bold(ui_kit::TEXT_SIZE),
        color::TEXT,
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(10.0, 0.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right(&mut child);
}

/// Small grey label (layer kind in the Properties heading).
fn badge(ui: &mut Ui, text: &str) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), ui_kit::medium(11.0), color::MUTED);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(14.0, 4.0), Sense::hover());
    ui.painter().rect_filled(rect, 5.0, color::BG3);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color::MUTED);
}

/// Section of the Properties panel: top border, small-caps title.
fn section(ui: &mut Ui, title: Option<&str>, add: impl FnOnce(&mut Ui)) {
    let top = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), top + 0.5, line());
    egui::Frame::new()
        .inner_margin(Margin {
            left: 14,
            right: 14,
            top: 12,
            bottom: 14,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(GAP, 10.0);
            if let Some(title) = title {
                ui_kit::section(ui, title);
            }
            add(ui);
        });
}

/// Layer name, label inside. `focus`: whole name selected, ready to replace
/// (double-click in the list).
fn name_field(ui: &mut Ui, name: &mut String, focus: bool) {
    let id = Id::new("layer_name");
    let mut output = ui_kit::text_field(ui, &t!("panel-name"), name, id);
    if focus {
        let len = name.chars().count();
        output
            .state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(len),
            )));
        output.state.store(ui.ctx(), id);
        ui.ctx().memory_mut(|m| m.request_focus(id));
    }
}

fn locked_notice(ui: &mut Ui) {
    egui::Frame::new()
        .fill(color::BG3)
        .corner_radius(7)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = ui.available_width();
            let format = |size| egui::TextFormat {
                font_id: FontId::proportional(size),
                color: color::MUTED,
                ..Default::default()
            };
            job.append(icon::LOCK_SIMPLE, 0.0, format(12.0));
            job.append(&t!("side-locked-notice"), 8.0, format(12.0));
            ui.label(job);
        });
}

/// The list of blend modes, in Photoshop's blocks; returns the mode under
/// the pointer while it is open.
pub(super) fn blend_combo(
    ui: &mut Ui,
    id: &str,
    blend: &mut BlendMode,
    group: bool,
    width: f32,
) -> Option<BlendMode> {
    let mut hovered = None;
    let label = blend.label();
    let shown = ui_kit::combo(ui, id, None, &label, width, |ui| {
        if group {
            let r = ui.selectable_value(
                blend,
                BlendMode::PassThrough,
                BlendMode::PassThrough.label(),
            );
            hovered = hovered.or(r.hovered().then_some(BlendMode::PassThrough));
        }
        for (k, modes) in BlendMode::GROUPS.iter().enumerate() {
            if k > 0 || group {
                ui.separator();
            }
            for &mode in *modes {
                let r = ui.selectable_value(blend, mode, mode.label());
                hovered = hovered.or(r.hovered().then_some(mode));
            }
        }
    })
    .response;
    let name = t!("panel-blend-mode");
    ui_kit::name_control(ui, shown.id, &name, &label);
    shown.on_hover_text(name);
    hovered
}

/// Fill opacity (like the opacity, without the styles).
fn fill_field(ui: &mut Ui, fill: &mut f32) {
    let half = (ui.available_width() - GAP) / 2.0;
    let text = i18n::percent(format!("{:.0}", *fill * 100.0));
    ui_kit::scrub(ui, &t!("core-fill-opacity"), fill, 0.0..=1.0, &text, half)
        .on_hover_text(t!("core-fill-opacity-tip"));
}

/// Solid color or gradient of a fill layer.
fn fill_content_fields(ui: &mut Ui, fill: &mut Fill) {
    match fill {
        Fill::Solid(c) => {
            ui.horizontal(|ui| {
                ui_kit::field_label(ui, &t!("core-fill-color"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut color = Color32::from_rgb(c[0], c[1], c[2]);
                    if ui_kit::swatch(ui, &mut color, 28.0, &t!("core-fill-color")).changed() {
                        *c = [color.r(), color.g(), color.b()];
                    }
                });
            });
        }
        Fill::Gradient(g) => gradient_fill_fields(ui, g),
    }
}

/// Gradient, style, angle, scale and direction of a gradient fill or
/// overlay.
pub(super) fn gradient_fill_fields(ui: &mut Ui, g: &mut crate::compose::gradient::GradientFill) {
    use crate::paint::gradient::Shape;
    super::gradient_editor::gradient_field(ui, &mut g.gradient);
    let half = (ui.available_width() - GAP) / 2.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        ui_kit::combo(
            ui,
            ("gradient_style", ui.id()),
            None,
            &g.style.label(),
            half,
            |ui| {
                for style in Shape::ALL {
                    ui.selectable_value(&mut g.style, style, style.label());
                }
            },
        )
        .response
        .on_hover_text(t!("core-gradient-style"));
        ui_kit::number_field(ui, icon::ANGLE, &mut g.angle, -180.0..=180.0, "°", half)
            .on_hover_text(t!("core-gradient-angle-tip"));
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let mut scale = (g.scale * 100.0).round();
        let unit = i18n::percent("");
        let label = t!("core-gradient-scale");
        if ui_kit::number_field(ui, &label, &mut scale, 10.0..=1000.0, &unit, half).changed() {
            g.scale = scale / 100.0;
        }
        ui_kit::checkbox(ui, &mut g.reverse, &t!("core-gradient-reverse"));
    });
}

/// Density and feather of the active layer's mask, and its buttons; returns
/// the action of the button clicked.
fn mask_fields(
    ui: &mut Ui,
    density: &mut f32,
    feather: &mut f32,
    doc: &Option<Document>,
) -> Option<Action> {
    let mut percent = (*density * 100.0).round();
    let unit = i18n::percent("");
    let track = dialogs::Track::Accent;
    let label = t!("core-mask-density");
    dialogs::slider_row(
        ui,
        &label,
        None,
        &mut percent,
        0.0..=100.0,
        &unit,
        track,
        100.0,
    );
    *density = percent / 100.0;
    let label = t!("core-mask-feather");
    dialogs::slider_row(ui, &label, None, feather, 0.0..=250.0, " px", track, 0.0);
    let mask = doc.as_ref().and_then(|d| d.active_layer().mask.as_ref());
    let (enabled, linked) = mask.map_or((true, true), |m| (m.enabled, m.linked()));
    let pixels = doc.as_ref().is_some_and(|d| d.active_layer().has_pixels());
    let mut action = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let (eye, eye_tip) = if enabled {
            (icon::EYE, t!("core-mask-disable"))
        } else {
            (icon::EYE_SLASH, t!("core-mask-enable"))
        };
        let (chain, chain_tip) = if linked {
            (icon::LINK, t!("core-mask-unlink"))
        } else {
            (icon::LINK_BREAK, t!("core-mask-link"))
        };
        for (glyph, tip, act, on) in [
            (
                icon::SELECTION,
                t!("core-mask-load-selection"),
                Action::MaskSelection,
                true,
            ),
            (icon::SWAP, t!("core-mask-invert"), Action::InvertMask, true),
            (eye, eye_tip, Action::ToggleMaskEnabled, true),
            (chain, chain_tip, Action::ToggleMaskLink, true),
            (
                icon::CHECK,
                t!("core-mask-apply"),
                Action::ApplyMask,
                pixels,
            ),
        ] {
            if ui_kit::icon_button(ui, glyph, &tip, on).clicked() {
                action = Some(act);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui_kit::icon_button(ui, icon::TRASH, &t!("core-mask-delete"), true).clicked() {
                action = Some(Action::DeleteMask);
            }
        });
    });
    action
}

/// The styles, a switch each; a click on a name opens it in the Layer
/// Style window (returned).
fn effect_fields(ui: &mut Ui, effects: &mut Effects) -> Option<Style> {
    let mut open = None;
    for style in Style::ALL {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui_kit::switch(ui, effects.enabled_mut(style), &style.label());
            let label = ui.add(
                egui::Label::new(egui::RichText::new(style.label()).color(color::TEXT))
                    .sense(Sense::click())
                    .selectable(false),
            );
            if label.on_hover_text(t!("core-style-open-tip")).clicked() {
                open = Some(style);
            }
        });
    }
    open
}

/// Foreground and Background fill buttons; returns the action clicked.
fn fill_buttons(ui: &mut Ui, primary: Color32, secondary: Color32) -> Option<Action> {
    let half = (ui.available_width() - GAP) / 2.0;
    let mut action = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (label, chip, key, act, name) in [
            (
                t!("panel-fill-foreground"),
                primary,
                keys::shortcut(ui.ctx(), &SHORTCUT_FILL_PRIMARY),
                Action::FillPrimary,
                t!("menu-edit-fill-foreground"),
            ),
            (
                t!("panel-fill-background"),
                secondary,
                keys::shortcut(ui.ctx(), &SHORTCUT_FILL_SECONDARY),
                Action::FillSecondary,
                t!("menu-edit-fill-background"),
            ),
        ] {
            if fill_button(ui, (&label, &name), chip, &key, half).clicked() {
                action = Some(act);
            }
        }
    });
    action
}

/// Button: color chip, label, and its key on the right (in the tooltip when
/// it doesn't fit); `name` for screen readers.
fn fill_button(
    ui: &mut Ui,
    (label, name): (&str, &str),
    chip: Color32,
    key: &str,
    width: f32,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    let painter = ui.painter();
    let fill = if response.hovered() {
        color::BUTTON_HOVER
    } else {
        color::BG3
    };
    painter.rect(rect, 6.0, fill, line(), StrokeKind::Inside);
    ui_kit::color_chip(painter, pos2(rect.left() + 16.0, rect.center().y), chip);
    let text = painter.layout_no_wrap(
        label.to_owned(),
        ui_kit::medium(ui_kit::TEXT_SIZE),
        color::TEXT,
    );
    let text_x = rect.left() + 29.0;
    let key_galley = painter.layout_no_wrap(key.to_owned(), ui_kit::mono(11.0), color::MUTED);
    let key_rect = Rect::from_min_size(
        pos2(
            rect.right() - 8.0 - key_galley.size().x - 10.0,
            rect.center().y - 9.0,
        ),
        vec2(key_galley.size().x + 10.0, 18.0),
    );
    let key_fits = text_x + text.size().x + 6.0 <= key_rect.left();
    if key_fits {
        painter.rect(
            key_rect,
            4.0,
            color::BG1,
            UiStroke::new(1.0, ui_kit::color::KBD_LINE),
            StrokeKind::Inside,
        );
        painter.galley(
            key_rect.center() - key_galley.size() / 2.0,
            key_galley,
            color::MUTED,
        );
    }
    let clip = Rect::from_min_max(rect.min, pos2(rect.right() - 6.0, rect.bottom()));
    painter
        .with_clip_rect(clip.intersect(painter.clip_rect()))
        .galley(
            pos2(text_x, rect.center().y - text.size().y / 2.0),
            text,
            color::TEXT,
        );
    // Painted: named for screen readers.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    if key_fits {
        response
    } else {
        ui_kit::tip_with_key(response, label, key)
    }
}

/// "Align to canvas" (or "Align N layers") and the align / distribute
/// buttons; the label goes on its own line when they don't fit beside it.
fn align_row(ui: &mut Ui, count: usize, enabled: bool, to_selection: bool) -> Option<AlignOp> {
    let label = if to_selection {
        t!("tools6-side-align-to-selection")
    } else if count > 1 {
        t!("side-align-layers", count = count)
    } else {
        t!("side-align-to-canvas")
    };
    let mut ops = vec![
        (icon::ALIGN_LEFT, t!("panel-align-left-tip"), AlignOp::Left),
        (
            icon::ALIGN_CENTER_HORIZONTAL,
            t!("panel-align-center-h-tip"),
            AlignOp::CenterH,
        ),
        (
            icon::ALIGN_RIGHT,
            t!("panel-align-right-tip"),
            AlignOp::Right,
        ),
        (icon::ALIGN_TOP, t!("panel-align-top-tip"), AlignOp::Top),
        (
            icon::ALIGN_CENTER_VERTICAL,
            t!("panel-align-center-v-tip"),
            AlignOp::CenterV,
        ),
        (
            icon::ALIGN_BOTTOM,
            t!("panel-align-bottom-tip"),
            AlignOp::Bottom,
        ),
    ];
    if count >= 3 {
        ops.push((
            icon::COLUMNS,
            t!("panel-distribute-h-tip"),
            AlignOp::DistributeH,
        ));
        ops.push((
            icon::ROWS,
            t!("panel-distribute-v-tip"),
            AlignOp::DistributeV,
        ));
    }
    let font = FontId::proportional(12.0);
    let label_w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), color::LABEL)
        .size()
        .x;
    let fits = label_w + GAP + 28.0 * ops.len() as f32 <= ui.available_width();
    let text = egui::RichText::new(label).font(font).color(color::LABEL);
    if !fits {
        ui.label(text.clone());
    }
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let mut buttons = |ui: &mut Ui, ops: &mut dyn Iterator<Item = &(&str, String, AlignOp)>| {
            for (glyph, tip, op) in ops {
                if ui_kit::icon_button(ui, glyph, tip, enabled).clicked() {
                    clicked = Some(*op);
                }
            }
        };
        if fits {
            ui.label(text);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                buttons(ui, &mut ops.iter().rev());
            });
        } else {
            buttons(ui, &mut ops.iter());
        }
    });
    clicked
}

/// Number field whose edits are only handed over once final (drag released,
/// typing confirmed), unless `live`: for edits that resample pixels.
fn deferred<N: egui::emath::Numeric + Send + Sync>(
    ui: &mut Ui,
    id: Id,
    value: &mut N,
    live: bool,
    field: impl FnOnce(&mut Ui, &mut N) -> Response,
) {
    if live {
        field(ui, value);
        return;
    }
    let mut v = ui.data(|d| d.get_temp::<N>(id)).unwrap_or(*value);
    let response = field(ui, &mut v);
    if response.dragged() || response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, v));
    } else {
        ui.data_mut(|d| d.remove::<N>(id));
        *value = v;
    }
}

/// Small icon button over a row of the list (eye, lock).
fn row_icon(ui: &mut Ui, rect: Rect, id: Id, glyph: &str, fg: Color32, tip: &str) -> Response {
    let response = ui.interact(rect, id, Sense::click());
    let fg = if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, color::BG3);
        color::TEXT
    } else {
        fg
    };
    let size = if rect.width() < 26.0 { 13.0 } else { 16.0 };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(size),
        fg,
    );
    response.on_hover_text(tip)
}

/// The histograms of what an adjustment layer adjusts (see
/// `AtelierApp::below_histogram`).
#[derive(Default)]
pub(super) struct BelowHistogram {
    /// Those shown: fingerprint of the layers below, document and layer.
    shown: Option<(u64, (u64, u64), Histograms)>,
    /// Being made: for which document and layer, and fingerprint.
    #[allow(clippy::type_complexity)]
    job: Option<(
        (u64, u64),
        std::thread::JoinHandle<(crate::compose::LevelCache, Histograms)>,
        u64,
    )>,
    /// The layers reduced, kept between them (of a document).
    cache: Option<(u64, crate::compose::LevelCache)>,
}

impl BelowHistogram {
    /// Memory its reductions take.
    pub(super) fn bytes(&self) -> usize {
        self.cache.as_ref().map_or(0, |(_, c)| c.bytes())
    }

    /// Whether they are being made.
    #[cfg(test)]
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }
}
