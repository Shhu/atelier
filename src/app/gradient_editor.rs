//! Gradient Editor: presets, the gradient bar with its opacity stops (above)
//! and color stops (below) and their midpoints, and the selected stop's
//! settings. Also draws gradients and shape icons for the options bar.

use super::dialogs::{DialogSpec, Track, floating_dialog, gap_between_blocks, slider_row};
use super::*;
use crate::paint::gradient::{
    ColorStop, Gradient, MIDPOINT_RANGE, OpacityStop, Preset, Shape, StopColor,
};
use ui_kit::color;

/// Part of the gradient bar: a stop or the midpoint after it (indices in
/// `colors` / `opacities`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Color(usize),
    Opacity(usize),
    ColorMidpoint(usize),
    OpacityMidpoint(usize),
}

/// Where the edited gradient goes on OK.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Target {
    /// The Gradient tool's.
    Tool,
    /// A layer's (fill, Gradient Map, Gradient Overlay), through its
    /// [`gradient_field`] of this id.
    Field(egui::Id),
}

/// Id of the request a [`gradient_field`] makes to open the editor.
const OPEN_REQUEST: &str = "gradient_editor_request";

/// Request to open the editor on the gradient of a [`gradient_field`].
#[derive(Clone)]
struct Request(egui::Id, Gradient);

pub(super) struct GradientEditor {
    pub gradient: Gradient,
    target: Target,
    /// The gradient when the editor opened (Reset).
    before: Gradient,
    pub selected: Part,
    /// Part being dragged; `true` once pulled away from the bar (the stop
    /// is removed on release, like Photoshop).
    drag: Option<(Part, bool)>,
}

impl GradientEditor {
    pub fn new(gradient: Gradient) -> Self {
        Self::with_target(gradient, Target::Tool)
    }

    fn with_target(gradient: Gradient, target: Target) -> Self {
        Self {
            before: gradient.clone(),
            gradient,
            target,
            selected: Part::Color(0),
            drag: None,
        }
    }

    /// Location of each stop, and the next stop by location (for midpoints).
    fn next_of(locations: &[f32], i: usize) -> Option<usize> {
        (0..locations.len())
            .filter(|&j| j != i && locations[j] >= locations[i])
            .filter(|&j| locations[j] > locations[i] || j > i)
            .min_by(|&a, &b| locations[a].total_cmp(&locations[b]).then(a.cmp(&b)))
    }

    fn color_locations(&self) -> Vec<f32> {
        self.gradient.colors.iter().map(|s| s.location).collect()
    }

    fn opacity_locations(&self) -> Vec<f32> {
        self.gradient.opacities.iter().map(|s| s.location).collect()
    }

    /// Where a midpoint is drawn (0..1), if its stop has a next one.
    fn midpoint_at(&self, part: Part) -> Option<f32> {
        let (locations, i, m) = match part {
            Part::ColorMidpoint(i) => (self.color_locations(), i, self.gradient.colors[i].midpoint),
            Part::OpacityMidpoint(i) => (
                self.opacity_locations(),
                i,
                self.gradient.opacities[i].midpoint,
            ),
            _ => return None,
        };
        let j = Self::next_of(&locations, i)?;
        Some(locations[i] + (locations[j] - locations[i]) * m)
    }

    /// Midpoints shown: those around the selected stop.
    fn shown_midpoints(&self) -> Vec<Part> {
        let around = |locations: Vec<f32>, i: usize, mid: fn(usize) -> Part| {
            let before = (0..locations.len()).find(|&j| Self::next_of(&locations, j) == Some(i));
            [Some(i), before]
                .into_iter()
                .flatten()
                .filter(|&j| Self::next_of(&locations, j).is_some())
                .map(mid)
                .collect::<Vec<_>>()
        };
        match self.selected {
            Part::Color(i) | Part::ColorMidpoint(i) => {
                around(self.color_locations(), i, Part::ColorMidpoint)
            }
            Part::Opacity(i) | Part::OpacityMidpoint(i) => {
                around(self.opacity_locations(), i, Part::OpacityMidpoint)
            }
        }
    }

    fn remove(&mut self, part: Part) {
        match part {
            Part::Color(i) if self.gradient.colors.len() > 2 => {
                self.gradient.colors.remove(i);
                self.selected = Part::Color(i.saturating_sub(1));
            }
            Part::Opacity(i) if self.gradient.opacities.len() > 2 => {
                self.gradient.opacities.remove(i);
                self.selected = Part::Opacity(i.saturating_sub(1));
            }
            _ => {}
        }
    }

    fn can_remove(&self) -> bool {
        match self.selected {
            Part::Color(_) => self.gradient.colors.len() > 2,
            Part::Opacity(_) => self.gradient.opacities.len() > 2,
            _ => false,
        }
    }

    /// The gradient bar: opacity stops above, color stops below. A click
    /// beside the bar adds a stop; stops and midpoints are dragged; a stop
    /// pulled away from the bar is removed.
    fn bar(&mut self, ui: &mut egui::Ui, fg: [u8; 3], bg: [u8; 3]) {
        const LANE: f32 = 16.0;
        const RAMP: f32 = 28.0;
        let width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(vec2(width, 2.0 * LANE + RAMP), Sense::click_and_drag());
        let ramp = Rect::from_min_size(rect.min + vec2(7.0, LANE), vec2(width - 14.0, RAMP));
        let x_of = |t: f32| ramp.left() + t.clamp(0.0, 1.0) * ramp.width();
        let t_of = |x: f32| ((x - ramp.left()) / ramp.width()).clamp(0.0, 1.0);
        let opacity_y = ramp.top() - LANE / 2.0;
        let color_y = ramp.bottom() + LANE / 2.0;

        // Press: picks a midpoint or a stop, or adds a stop in that lane.
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        if pressed
            && response.is_pointer_button_down_on()
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
        {
            let midpoint = self.shown_midpoints().into_iter().find(|&m| {
                let y = if matches!(m, Part::ColorMidpoint(_)) {
                    color_y
                } else {
                    opacity_y
                };
                self.midpoint_at(m)
                    .is_some_and(|t| (x_of(t) - p.x).abs() <= 5.0 && (y - p.y).abs() <= 7.0)
            });
            let near = |locations: Vec<f32>| {
                (0..locations.len())
                    .filter(|&i| (x_of(locations[i]) - p.x).abs() <= 7.0)
                    .min_by(|&a, &b| {
                        (x_of(locations[a]) - p.x)
                            .abs()
                            .total_cmp(&(x_of(locations[b]) - p.x).abs())
                    })
            };
            let t = t_of(p.x);
            let part = if let Some(m) = midpoint {
                Some(m)
            } else if p.y >= ramp.bottom() {
                Some(near(self.color_locations()).map_or_else(
                    || {
                        let [r, g, b, _] = self.gradient.sample(t, fg, bg, true);
                        let color = StopColor::Rgb([r, g, b].map(|v| v.round() as u8));
                        self.gradient.colors.push(ColorStop {
                            location: t,
                            midpoint: 0.5,
                            color,
                        });
                        Part::Color(self.gradient.colors.len() - 1)
                    },
                    Part::Color,
                ))
            } else if p.y <= ramp.top() {
                Some(near(self.opacity_locations()).map_or_else(
                    || {
                        let opacity = self.gradient.sample(t, fg, bg, true)[3];
                        self.gradient.opacities.push(OpacityStop {
                            location: t,
                            midpoint: 0.5,
                            opacity,
                        });
                        Part::Opacity(self.gradient.opacities.len() - 1)
                    },
                    Part::Opacity,
                ))
            } else {
                None
            };
            if let Some(part) = part {
                self.selected = part;
                self.drag = Some((part, false));
            }
        }
        if let Some((part, _)) = self.drag
            && let Some(p) = ui.input(|i| i.pointer.interact_pos())
            && ui.input(|i| i.pointer.primary_down())
        {
            let t = t_of(p.x);
            let pulled = match part {
                Part::Color(i) => {
                    self.gradient.colors[i].location = t;
                    p.y - color_y > 24.0 && self.gradient.colors.len() > 2
                }
                Part::Opacity(i) => {
                    self.gradient.opacities[i].location = t;
                    opacity_y - p.y > 24.0 && self.gradient.opacities.len() > 2
                }
                Part::ColorMidpoint(i) | Part::OpacityMidpoint(i) => {
                    let locations = if matches!(part, Part::ColorMidpoint(_)) {
                        self.color_locations()
                    } else {
                        self.opacity_locations()
                    };
                    if let Some(j) = Self::next_of(&locations, i) {
                        let span = locations[j] - locations[i];
                        if span > 1e-6 {
                            let m = ((t - locations[i]) / span)
                                .clamp(*MIDPOINT_RANGE.start(), *MIDPOINT_RANGE.end());
                            match part {
                                Part::ColorMidpoint(_) => self.gradient.colors[i].midpoint = m,
                                _ => self.gradient.opacities[i].midpoint = m,
                            }
                        }
                    }
                    false
                }
            };
            self.drag = Some((part, pulled));
        }
        if !ui.input(|i| i.pointer.primary_down())
            && let Some((part, pulled)) = self.drag.take()
            && pulled
        {
            self.remove(part);
        }

        // Drawing.
        let painter = ui.painter();
        paint_ramp(painter, ramp, &self.gradient, fg, bg, true);
        painter.rect_stroke(
            ramp,
            0.0,
            UiStroke::new(1.0, color::LINE_HOVER),
            StrokeKind::Outside,
        );
        let pulled = |part: Part| self.drag == Some((part, true));
        for (i, s) in self.gradient.opacities.iter().enumerate() {
            let part = Part::Opacity(i);
            let fill = Color32::from_gray((s.opacity.clamp(0.0, 1.0) * 255.0).round() as u8);
            let y = if pulled(part) {
                opacity_y - 26.0
            } else {
                opacity_y
            };
            stop_marker(
                painter,
                pos2(x_of(s.location), y),
                fill,
                false,
                self.selected == part,
            );
        }
        for (i, s) in self.gradient.colors.iter().enumerate() {
            let part = Part::Color(i);
            let [r, g, b] = s.color.resolve(fg, bg);
            let y = if pulled(part) {
                color_y + 26.0
            } else {
                color_y
            };
            let fill = Color32::from_rgb(r, g, b);
            stop_marker(
                painter,
                pos2(x_of(s.location), y),
                fill,
                true,
                self.selected == part,
            );
        }
        for m in self.shown_midpoints() {
            let Some(t) = self.midpoint_at(m) else {
                continue;
            };
            let y = if matches!(m, Part::ColorMidpoint(_)) {
                color_y
            } else {
                opacity_y
            };
            let c = pos2(x_of(t), y);
            let d = 4.0;
            let points = vec![
                c - vec2(0.0, d),
                c + vec2(d, 0.0),
                c + vec2(0.0, d),
                c - vec2(d, 0.0),
            ];
            let fill = if self.selected == m {
                color::ACCENT
            } else {
                color::BG3
            };
            painter.add(egui::Shape::convex_polygon(
                points,
                fill,
                UiStroke::new(1.0, color::MUTED),
            ));
        }
    }

    /// Settings of the selected stop or midpoint.
    fn stop_fields(&mut self, ui: &mut egui::Ui, fg: [u8; 3], bg: [u8; 3]) {
        let percent = i18n::percent("");
        let location =
            |ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<u32>| {
                let mut v = (*value * 100.0).round() as u32;
                let label = t!("paint-gradient-location");
                if ui_kit::number_field(ui, &label, &mut v, range, &percent, 160.0).changed() {
                    *value = v as f32 / 100.0;
                }
            };
        ui_kit::field_label(ui, &t!("paint-gradient-stops"));
        let mut remove = false;
        match self.selected {
            Part::Color(i) => {
                let stop = &mut self.gradient.colors[i];
                ui.horizontal(|ui| {
                    let kinds = [
                        (StopColor::Foreground, t!("paint-gradient-foreground")),
                        (StopColor::Background, t!("paint-gradient-background")),
                    ];
                    let custom = t!("paint-gradient-custom-color");
                    let shown = kinds
                        .iter()
                        .find(|(k, _)| *k == stop.color)
                        .map_or(custom.clone(), |(_, l)| l.clone());
                    ui_kit::combo(ui, "gradient_stop_kind", None, &shown, 150.0, |ui| {
                        for (kind, label) in &kinds {
                            if ui.selectable_label(stop.color == *kind, label).clicked() {
                                stop.color = *kind;
                            }
                        }
                        let rgb = stop.color.resolve(fg, bg);
                        let is_custom = matches!(stop.color, StopColor::Rgb(_));
                        if ui.selectable_label(is_custom, &custom).clicked() {
                            stop.color = StopColor::Rgb(rgb);
                        }
                    });
                    let [r, g, b] = stop.color.resolve(fg, bg);
                    let mut c = Color32::from_rgb(r, g, b);
                    if ui_kit::swatch(ui, &mut c, 28.0, &t!("paint-gradient-stop-color")).changed()
                    {
                        let [r, g, b, _] = c.to_srgba_unmultiplied();
                        stop.color = StopColor::Rgb([r, g, b]);
                    }
                    location(ui, &mut stop.location, 0..=100);
                });
            }
            Part::Opacity(i) => {
                let stop = &mut self.gradient.opacities[i];
                let mut opacity = stop.opacity * 100.0;
                slider_row(
                    ui,
                    &t!("paint-gradient-opacity"),
                    None,
                    &mut opacity,
                    0.0..=100.0,
                    &percent,
                    Track::Accent,
                    100.0,
                );
                stop.opacity = opacity / 100.0;
                location(ui, &mut stop.location, 0..=100);
            }
            Part::ColorMidpoint(i) => {
                location(ui, &mut self.gradient.colors[i].midpoint, 5..=95);
            }
            Part::OpacityMidpoint(i) => {
                location(ui, &mut self.gradient.opacities[i].midpoint, 5..=95);
            }
        }
        if matches!(self.selected, Part::Color(_) | Part::Opacity(_)) {
            let label = t!("paint-gradient-delete-stop");
            let enabled = self.can_remove();
            remove = ui_kit::button_with_icon(
                ui,
                Some(icon::TRASH),
                &label,
                Kind::Normal,
                None,
                enabled,
            )
            .clicked();
        }
        if remove {
            self.remove(self.selected);
        }
    }
}

/// Stop marker: a square and a point toward the bar (`below`: under it).
fn stop_marker(painter: &Painter, tip: Pos2, fill: Color32, below: bool, selected: bool) {
    let dir = if below { -1.0 } else { 1.0 };
    let line = if selected {
        color::ACCENT
    } else {
        color::MUTED
    };
    let square = Rect::from_center_size(tip - vec2(0.0, dir * 2.0), Vec2::splat(11.0));
    let point = vec![
        pos2(tip.x, tip.y + dir * 7.0),
        pos2(tip.x + 5.5, square.center().y + dir * 2.0),
        pos2(tip.x - 5.5, square.center().y + dir * 2.0),
    ];
    painter.add(egui::Shape::convex_polygon(point, line, UiStroke::NONE));
    painter.rect(
        square,
        2.0,
        fill,
        UiStroke::new(if selected { 2.0 } else { 1.0 }, line),
        StrokeKind::Inside,
    );
}

/// A gradient across `rect`, over a checkerboard where it is transparent.
pub(super) fn paint_ramp(
    painter: &Painter,
    rect: Rect,
    gradient: &Gradient,
    fg: [u8; 3],
    bg: [u8; 3],
    transparency: bool,
) {
    draw_checker(
        &painter.with_clip_rect(rect.intersect(painter.clip_rect())),
        rect,
    );
    const STEPS: usize = 96;
    let mut mesh = egui::epaint::Mesh::default();
    for i in 0..=STEPS {
        let t = i as f32 / STEPS as f32;
        let [r, g, b, a] = gradient.sample(t, fg, bg, transparency);
        let c = Color32::from_rgba_unmultiplied(
            r.round() as u8,
            g.round() as u8,
            b.round() as u8,
            (a * 255.0).round() as u8,
        );
        let x = rect.left() + t * rect.width();
        mesh.colored_vertex(pos2(x, rect.top()), c);
        mesh.colored_vertex(pos2(x, rect.bottom()), c);
        if i > 0 {
            let k = 2 * i as u32;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(mesh);
}

/// Icon of a gradient shape: black to white drawn that way in `rect`.
pub(super) fn paint_shape_icon(painter: &Painter, rect: Rect, shape: Shape, fg: Color32) {
    const N: usize = 14;
    // The line each icon is drawn along, in 0..1 of the square.
    let (start, end) = match shape {
        Shape::Linear => ([0.0, 0.5], [1.0, 0.5]),
        Shape::Reflected => ([0.5, 0.5], [1.0, 0.5]),
        _ => ([0.5, 0.5], [1.0, 0.5]),
    };
    let mut mesh = egui::epaint::Mesh::default();
    let tint = |t: f32| {
        let v = (t * 255.0).round() as u8;
        let [r, g, b, _] = fg.to_array();
        Color32::from_rgb(
            ((r as u16 * v as u16) / 255) as u8,
            ((g as u16 * v as u16) / 255) as u8,
            ((b as u16 * v as u16) / 255) as u8,
        )
    };
    for j in 0..=N {
        for i in 0..=N {
            let p = [i as f32 / N as f32, j as f32 / N as f32];
            let t = shape.position(start, end, p);
            mesh.colored_vertex(
                rect.min + vec2(p[0] * rect.width(), p[1] * rect.height()),
                tint(t),
            );
            if i > 0 && j > 0 {
                let k = (j * (N + 1) + i) as u32;
                let row = (N + 1) as u32;
                mesh.add_triangle(k - row - 1, k - row, k);
                mesh.add_triangle(k - row - 1, k, k - 1);
            }
        }
    }
    painter.add(mesh);
}

impl AtelierApp {
    /// Gradient Editor (a click on the gradient in the options bar, or on
    /// a layer's [`gradient_field`]).
    pub(super) fn gradient_editor_window(&mut self, ctx: &egui::Context) {
        let request = ctx.data_mut(|d| take::<Request>(d, OPEN_REQUEST.into()));
        if let Some(Request(id, gradient)) = request.filter(|_| self.paint.editor.is_none()) {
            self.paint.editor = Some(GradientEditor::with_target(gradient, Target::Field(id)));
        }
        let Some(mut editor) = self.paint.editor.take() else {
            return;
        };
        let [fr, fg, fb, _] = self.primary.to_srgba_unmultiplied();
        let [br, bg, bb, _] = self.secondary.to_srgba_unmultiplied();
        let (fg, bg) = ([fr, fg, fb], [br, bg, bb]);
        let changed = editor.gradient != editor.before;
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 460.0,
                reset: Some(changed),
                ..DialogSpec::new("gradient_editor", t!("paint-gradient-editor-title"), center)
            },
            |ui| {
                ui_kit::field_label(ui, &t!("paint-gradient-presets"));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    for preset in Preset::ALL {
                        let (r, response) =
                            ui.allocate_exact_size(vec2(64.0, 30.0), Sense::click());
                        let g = preset.gradient();
                        paint_ramp(ui.painter(), r, &g, fg, bg, true);
                        let line = if response.hovered() || editor.gradient == g {
                            color::TEXT
                        } else {
                            color::LINE_HOVER
                        };
                        ui.painter().rect_stroke(
                            r,
                            3.0,
                            UiStroke::new(1.0, line),
                            StrokeKind::Outside,
                        );
                        if response
                            .on_hover_text(gradient_preset_name(preset))
                            .clicked()
                        {
                            editor.gradient = g;
                            editor.selected = Part::Color(0);
                        }
                    }
                });
                gap_between_blocks(ui);
                editor.bar(ui, fg, bg);
                gap_between_blocks(ui);
                editor.stop_fields(ui, fg, bg);
            },
        );
        if out.reset {
            editor.gradient = editor.before.clone();
            editor.selected = Part::Color(0);
        }
        if out.ok {
            match editor.target {
                Target::Tool => self.paint.gradient.gradient = editor.gradient.clone(),
                Target::Field(id) => {
                    let gradient = editor.gradient.resolved(fg, bg);
                    ctx.data_mut(|d| d.insert_temp(id, gradient));
                    ctx.request_repaint();
                }
            }
        }
        if !(out.ok || out.cancel) {
            self.paint.editor = Some(editor);
        }
    }
}

/// Takes the value of type `T` stored at `id`.
fn take<T: Clone + Send + Sync + 'static>(
    data: &mut egui::util::IdTypeMap,
    id: egui::Id,
) -> Option<T> {
    let value = data.get_temp::<T>(id);
    data.remove::<T>(id);
    value
}

/// A layer's gradient: its bar. A click opens the Gradient Editor on it,
/// which gives it back on OK.
pub(super) fn gradient_field(ui: &mut egui::Ui, gradient: &mut Gradient) {
    let id = ui.make_persistent_id("gradient_field");
    if let Some(edited) = ui.data_mut(|d| take::<Gradient>(d, id)) {
        *gradient = edited;
    }
    let size = vec2(ui.available_width(), 24.0);
    let (bar, response) = ui.allocate_exact_size(size, Sense::click());
    paint_ramp(ui.painter(), bar, gradient, [0; 3], [255; 3], true);
    let line = if response.hovered() {
        color::TEXT
    } else {
        color::LINE
    };
    ui.painter()
        .rect_stroke(bar, 4.0, UiStroke::new(1.0, line), StrokeKind::Inside);
    let tip = t!("xchg-gradient-edit-tip");
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &tip));
    if response.on_hover_text(tip).clicked() {
        let request = Request(id, gradient.clone());
        ui.data_mut(|d| d.insert_temp(OPEN_REQUEST.into(), request));
    }
}

/// Name of a gradient preset.
pub(super) fn gradient_preset_name(preset: Preset) -> String {
    match preset {
        Preset::ForegroundToBackground => t!("paint-gradient-fg-to-bg"),
        Preset::ForegroundToTransparent => t!("paint-gradient-fg-to-transparent"),
        Preset::BlackToWhite => t!("paint-gradient-black-white"),
        Preset::Spectrum => t!("paint-gradient-spectrum"),
        Preset::VioletOrange => t!("paint-gradient-violet-orange"),
        Preset::Copper => t!("paint-gradient-copper"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn midpoints_belong_to_the_segment_after_their_stop() {
        let mut editor = GradientEditor::new(Preset::BlackToWhite.gradient());
        editor.gradient.colors[0].location = 0.2;
        editor.gradient.colors[0].midpoint = 0.25;
        editor.gradient.colors[1].location = 0.6;
        assert_eq!(editor.midpoint_at(Part::ColorMidpoint(0)), Some(0.3));
        assert_eq!(
            editor.midpoint_at(Part::ColorMidpoint(1)),
            None,
            "last stop"
        );
        // Stops swapped by a drag: the midpoint follows its stop.
        editor.gradient.colors[0].location = 0.9;
        assert_eq!(editor.midpoint_at(Part::ColorMidpoint(0)), None);
        assert!(editor.midpoint_at(Part::ColorMidpoint(1)).is_some());
    }

    #[test]
    fn a_gradient_keeps_two_stops_of_each_kind() {
        let mut editor = GradientEditor::new(Preset::BlackToWhite.gradient());
        editor.remove(Part::Color(0));
        editor.remove(Part::Opacity(1));
        assert_eq!(editor.gradient.colors.len(), 2);
        assert_eq!(editor.gradient.opacities.len(), 2);
        let mut spectrum = GradientEditor::new(Preset::Spectrum.gradient());
        spectrum.selected = Part::Color(3);
        assert!(spectrum.can_remove());
        spectrum.remove(Part::Color(3));
        assert_eq!(spectrum.gradient.colors.len(), 6);
    }
}
