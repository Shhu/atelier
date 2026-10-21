//! Tool bar: the tools as icons in groups, then the foreground and
//! background colors.

use super::keys;
use super::*;
use crate::i18n::t;

impl AtelierApp {
    pub(super) fn toolbar(&mut self, ui: &mut egui::Ui) {
        // The colors stay at the bottom; the tools scroll above them when
        // the window is too low.
        let full = ui.max_rect();
        let (tools, colors) = full.split_top_bottom_at_y(full.bottom() - 40.0);
        ui.scope_builder(egui::UiBuilder::new().max_rect(colors), |ui| {
            self.colors(ui);
        });
        let mut tools =
            ui.new_child(egui::UiBuilder::new().max_rect(tools.with_max_y(tools.bottom() - 8.0)));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut tools, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for (index, group) in Tool::GROUPS.iter().enumerate() {
                    // A line before each section: selection, painting, paths
                    // and text, view.
                    if matches!(
                        group[0],
                        Tool::Select | Tool::SpotHealing | Tool::Pen(_) | Tool::Hand
                    ) {
                        let (rect, _) = ui.allocate_exact_size(vec2(36.0, 11.0), Sense::hover());
                        let line = Rect::from_center_size(rect.center(), vec2(22.0, 1.0));
                        ui.painter().rect_filled(line, 0.0, ui_kit::color::LINE);
                    }
                    let tool = self.group_shown(index);
                    let key = tool.shortcut().map_or("", |k| k.name());
                    let response =
                        ui_kit::tool_button(ui, tool.glyph(), &tool.name(), key, self.tool == tool);
                    // A long press opens the group's menu instead.
                    let menu_press = group.len() > 1 && self.group_menu(ui, &response, group);
                    if response.clicked() && !menu_press {
                        self.set_tool(tool);
                    }
                    // Double click, like Photoshop: the Hand fits the image on
                    // screen, the Zoom shows it at 100 %.
                    if response.double_clicked() {
                        match tool {
                            Tool::Hand => self.run(Action::Fit),
                            Tool::Zoom => self.run(Action::ActualSize),
                            _ => {}
                        }
                    }
                }
            });
    }

    /// A group's other tools: a small corner mark on the slot, and its menu
    /// on a right click or a long press, like Photoshop. The menu stays open
    /// when the long press ends: releasing it over a tool picks that tool,
    /// otherwise a click picks one and a press elsewhere closes it. Returns
    /// whether the slot's press opened the menu (its release isn't a click).
    fn group_menu(&mut self, ui: &mut egui::Ui, slot: &egui::Response, group: &[Tool]) -> bool {
        let corner = slot.rect.right_bottom() - vec2(4.0, 4.0);
        ui.painter().add(egui::Shape::convex_polygon(
            vec![corner, corner - vec2(4.0, 0.0), corner - vec2(0.0, 4.0)],
            ui_kit::color::MUTED,
            UiStroke::NONE,
        ));
        let ctx = ui.ctx().clone();
        let id = slot.id.with("tools");
        let holding_id = id.with("holding");
        let down = slot.is_pointer_button_down_on();
        if down {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let held = down
            && ctx.input(|i| {
                i.pointer
                    .press_start_time()
                    .is_some_and(|start| i.time - start > 0.3)
            });
        let mut holding = ctx.data(|d| d.get_temp::<bool>(holding_id).unwrap_or(false));
        if held && !holding {
            holding = true;
            ctx.data_mut(|d| d.insert_temp(holding_id, true));
            egui::Popup::open_id(&ctx, id);
        }
        if slot.secondary_clicked() {
            egui::Popup::open_id(&ctx, id);
        }
        let released = ctx.input(|i| i.pointer.primary_released());
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let shown = egui::Popup::from_response(slot)
            .id(id)
            .open_memory(None)
            .kind(egui::PopupKind::Menu)
            .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .align(egui::RectAlign::RIGHT_START)
            .show(|ui| {
                ui.set_min_width(200.0);
                for &tool in group {
                    let key = tool.shortcut().map_or("", |k| k.name());
                    let label = format!("{}   {}", tool.glyph(), tool.name());
                    let button = egui::Button::new(label)
                        .shortcut_text(key)
                        .selected(tool == self.tool);
                    let item = ui.add(button);
                    let dropped_on =
                        holding && released && pointer.is_some_and(|p| item.rect.contains(p));
                    if item.clicked() || dropped_on {
                        self.set_tool(tool);
                        ui.close();
                    }
                }
            });
        // A new press outside the menu closes it.
        if let Some(menu) = shown {
            let pressed_outside = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .press_origin()
                        .is_some_and(|p| !menu.response.rect.contains(p))
            });
            if pressed_outside {
                egui::Popup::close_id(&ctx, id);
            }
        }
        // The press ends with its release, wherever the pointer went.
        if holding && !ctx.input(|i| i.pointer.primary_down()) {
            ctx.data_mut(|d| d.remove::<bool>(holding_id));
        }
        holding
    }

    /// Overlapping foreground and background colors (each opens the
    /// picker), swap (X) and default colors (D), like Photoshop.
    fn colors(&mut self, ui: &mut egui::Ui) {
        let (area, _) = ui.allocate_exact_size(vec2(36.0, 40.0), Sense::hover());
        let at = |x: f32, y: f32, size: f32| {
            Rect::from_min_size(area.min + vec2(x, y), Vec2::splat(size))
        };

        let background = at(14.0, 16.0, 22.0);
        let key = keys::shortcut(ui.ctx(), &SHORTCUT_FILL_SECONDARY);
        color_square(ui, background, &mut self.secondary, false).on_hover_ui(|ui| {
            ui.label(t!("panel-background"));
            ui.horizontal(|ui| {
                ui_kit::hint(ui, t!("panel-background-tip"));
                ui_kit::kbd(ui, &key);
            });
        });
        let foreground = at(0.0, 0.0, 22.0);
        let key = keys::shortcut(ui.ctx(), &SHORTCUT_FILL_PRIMARY);
        color_square(ui, foreground, &mut self.primary, true).on_hover_ui(|ui| {
            ui.label(t!("panel-foreground"));
            ui.horizontal(|ui| {
                ui_kit::hint(ui, t!("panel-foreground-tip"));
                ui_kit::kbd(ui, &key);
            });
        });

        // Swap, in the free top right corner.
        let swap = at(23.0, -1.0, 14.0);
        let response = ui.interact(swap, ui.id().with("swap-colors"), Sense::click());
        let color = if response.hovered() {
            ui_kit::color::TEXT
        } else {
            ui_kit::color::MUTED
        };
        ui.painter().text(
            swap.center(),
            egui::Align2::CENTER_CENTER,
            icon::ARROWS_LEFT_RIGHT,
            egui::FontId::proportional(12.0),
            color,
        );
        if ui_kit::tip_with_key(response, &t!("panel-swap-colors"), "X").clicked() {
            std::mem::swap(&mut self.primary, &mut self.secondary);
        }

        // Default colors, in the free bottom left corner: black over white.
        let reset = at(0.0, 27.0, 12.0);
        let response = ui.interact(reset, ui.id().with("default-colors"), Sense::click());
        let ring = UiStroke::new(
            1.0,
            if response.hovered() {
                ui_kit::color::TEXT
            } else {
                ui_kit::color::LABEL
            },
        );
        let painter = ui.painter();
        let white = Rect::from_min_size(reset.min + vec2(4.0, 4.0), Vec2::splat(8.0));
        let black = Rect::from_min_size(reset.min, Vec2::splat(8.0));
        painter.rect(white, 2.0, Color32::WHITE, ring, StrokeKind::Inside);
        painter.rect(black, 2.0, Color32::BLACK, ring, StrokeKind::Inside);
        if ui_kit::tip_with_key(response, &t!("panel-default-colors"), "D").clicked() {
            (self.primary, self.secondary) = (Color32::BLACK, Color32::WHITE);
        }
    }
}

/// Color square with a thin grey border, so any color stands out from the
/// bar; the front one also gets a gap of the bar's color around it. A click
/// opens the Color Picker.
fn color_square(ui: &mut egui::Ui, rect: Rect, color: &mut Color32, front: bool) -> egui::Response {
    let response = ui.interact(rect, ui.id().with(("color-square", front)), Sense::click());
    let painter = ui.painter();
    if front {
        painter.rect_filled(rect.expand(2.0), 7.0, ui_kit::color::BG1);
    }
    let border = if response.hovered() {
        ui_kit::color::MUTED
    } else {
        ui_kit::color::SWATCH_LINE
    };
    painter.rect(
        rect,
        5.0,
        *color,
        UiStroke::new(1.0, border),
        StrokeKind::Inside,
    );
    let title = if front {
        t!("uip-picker-foreground-title")
    } else {
        t!("uip-picker-background-title")
    };
    ui_kit::picker_link(ui.ctx(), response, color, title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    fn app() -> Harness<'static, AtelierApp> {
        i18n::set_lang(Lang::En);
        let mut h = Harness::builder()
            .with_size([1600.0, 1100.0])
            .build_eframe(|cc| AtelierApp::new(cc, None));
        h.run_steps(3);
        h
    }

    /// The marquee group's slot.
    fn slot(h: &Harness<'static, AtelierApp>) -> Pos2 {
        h.get_by_label(&Tool::Select.name()).rect().center()
    }

    /// Presses the slot for `steps` frames of 0.25 s, then releases at `to`.
    fn hold(h: &mut Harness<'static, AtelierApp>, steps: usize, to: Pos2) {
        let at = slot(h);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(steps);
        h.hover_at(to);
        h.run_steps(1);
        h.drop_at(to);
        h.run_steps(2);
    }

    fn item(h: &Harness<'static, AtelierApp>, tool: Tool) -> Pos2 {
        h.get_by_label_contains(&tool.name()).rect().center()
    }

    #[test]
    fn a_long_press_opens_the_group_menu_and_keeps_it_open() {
        let mut h = app();
        h.state_mut().set_tool(Tool::Brush);
        // Released within the click delay: the menu stays, no tool changes.
        let at = slot(&h);
        hold(&mut h, 3, at);
        assert!(h.state().tool == Tool::Brush);
        let ellipse = item(&h, Tool::Ellipse);
        h.hover_at(ellipse);
        h.run_steps(1);
        h.drag_at(ellipse);
        h.run_steps(1);
        h.drop_at(ellipse);
        h.run_steps(2);
        assert!(h.state().tool == Tool::Ellipse);
        assert!(
            h.query_by_label(&format!(
                "{}   {}",
                Tool::Select.glyph(),
                Tool::Select.name()
            ))
            .is_none()
        );
    }

    #[test]
    fn releasing_the_long_press_over_a_tool_picks_it() {
        let mut h = app();
        h.state_mut().set_tool(Tool::Brush);
        let at = slot(&h);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(3);
        let ellipse = item(&h, Tool::Ellipse);
        h.hover_at(ellipse);
        h.run_steps(1);
        h.drop_at(ellipse);
        h.run_steps(2);
        assert!(h.state().tool == Tool::Ellipse);
    }

    #[test]
    fn a_press_elsewhere_closes_the_menu() {
        let mut h = app();
        let at = slot(&h);
        hold(&mut h, 3, at);
        let label = Tool::Ellipse.name();
        assert!(h.query_by_label_contains(&label).is_some());
        let away = pos2(800.0, 600.0);
        h.hover_at(away);
        h.run_steps(1);
        h.drag_at(away);
        h.run_steps(1);
        h.drop_at(away);
        h.run_steps(2);
        assert!(h.query_by_label_contains(&label).is_none());
    }

    #[test]
    fn a_long_press_on_another_group_switches_menus() {
        let mut h = app();
        let at = slot(&h);
        let lassos = h.get_by_label(&Tool::FreeLasso.name()).rect().center();
        hold(&mut h, 3, at);
        assert!(h.query_by_label_contains(&Tool::Ellipse.name()).is_some());
        h.hover_at(lassos);
        h.run_steps(1);
        h.drag_at(lassos);
        h.run_steps(3);
        h.drop_at(lassos);
        h.run_steps(2);
        assert!(h.query_by_label_contains(&Tool::Ellipse.name()).is_none());
        let polygonal = h
            .get_by_label_contains(&Tool::PolyLasso.name())
            .rect()
            .center();
        h.hover_at(polygonal);
        h.run_steps(1);
        h.drag_at(polygonal);
        h.run_steps(1);
        h.drop_at(polygonal);
        h.run_steps(2);
        assert!(h.state().tool == Tool::PolyLasso);
    }
}
