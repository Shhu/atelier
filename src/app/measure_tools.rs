//! Color Sampler and Ruler tools (with the Eyedropper: I, Shift+I). Points
//! are placed, moved and removed on the image, one history step each; the
//! Ruler's line is a measure of the moment, neither saved nor undone.

use super::*;
use crate::document::{ColorSampler, MAX_SAMPLERS, Measure};
use crate::i18n::t;

/// How near a point or a line the pointer grabs it (screen px).
const GRAB: f32 = 7.0;

/// Sample Size of the Color Sampler: a pixel, or an average around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum SampleSize {
    #[default]
    Point,
    Average3,
    Average5,
    Average11,
    Average31,
    Average51,
    Average101,
}

impl SampleSize {
    pub(super) const ALL: [SampleSize; 7] = [
        SampleSize::Point,
        SampleSize::Average3,
        SampleSize::Average5,
        SampleSize::Average11,
        SampleSize::Average31,
        SampleSize::Average51,
        SampleSize::Average101,
    ];

    /// Pixels on each side of the one read.
    pub(super) fn radius(self) -> i64 {
        match self {
            SampleSize::Point => 0,
            SampleSize::Average3 => 1,
            SampleSize::Average5 => 2,
            SampleSize::Average11 => 5,
            SampleSize::Average31 => 15,
            SampleSize::Average51 => 25,
            SampleSize::Average101 => 50,
        }
    }

    pub(super) fn label(self) -> String {
        match self {
            SampleSize::Point => t!("panels8-sample-point"),
            _ => t!("panels8-sample-average", size = self.radius() * 2 + 1),
        }
    }
}

/// A point being dragged: `moved`: its step is recorded.
#[derive(Clone, Copy)]
struct SamplerDrag {
    index: usize,
    moved: bool,
}

/// What a drag of the Ruler tool holds.
#[derive(Clone, Copy, PartialEq, Debug)]
enum RulerDrag {
    Vertex,
    First,
    Second,
    /// The whole line, from the image point where the pointer last was.
    Line((f32, f32)),
}

#[derive(Default)]
pub(super) struct MeasureTools {
    pub sample: SampleSize,
    sampler_drag: Option<SamplerDrag>,
    ruler_drag: Option<RulerDrag>,
}

/// `to` on the nearest 45° direction from `from`, at the same distance.
fn on_45(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = dx.hypot(dy);
    let step = std::f32::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    (from.0 + length * angle.cos(), from.1 + length * angle.sin())
}

fn near_segment(p: Pos2, a: Pos2, b: Pos2) -> bool {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length() <= GRAB
}

fn screen_of(view: View, p: (f32, f32)) -> Pos2 {
    view.to_screen(pos2(p.0, p.1))
}

impl AtelierApp {
    /// The point under `at` (screen), the nearest one.
    fn sampler_at(&self, view: View, at: Pos2) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        doc.samplers
            .iter()
            .enumerate()
            .map(|(i, s)| (i, (screen_of(view, (s.x, s.y)) - at).length()))
            .filter(|&(_, d)| d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    }

    /// A new point on the pixel under `at` (canvas coordinates); `None`
    /// outside the canvas, or when the document has all its points (said).
    pub(super) fn add_sampler(&mut self, at: Pos2) -> Option<usize> {
        let doc = self.doc.as_mut()?;
        let (w, h) = (doc.width() as f32, doc.height() as f32);
        if !(0.0..w).contains(&at.x) || !(0.0..h).contains(&at.y) {
            return None;
        }
        if doc.samplers.len() >= MAX_SAMPLERS {
            self.status = t!("panels8-status-samplers-full", count = MAX_SAMPLERS);
            return None;
        }
        doc.checkpoint(t!("panels8-hist-add-sampler"));
        doc.samplers.push(ColorSampler {
            x: at.x.floor() + 0.5,
            y: at.y.floor() + 0.5,
        });
        Some(doc.samplers.len() - 1)
    }

    /// Color Sampler: a click adds a point, a drag moves one (out of the
    /// canvas it goes), Alt+click removes one.
    pub(super) fn sampler_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        alt: bool,
    ) -> CursorIcon {
        let Some(p) = pointer else {
            self.measure.sampler_drag = None;
            return CursorIcon::Crosshair;
        };
        let at = view.to_image(p);
        if start {
            match self.sampler_at(view, p) {
                Some(i) if alt => {
                    if let Some(doc) = &mut self.doc {
                        doc.checkpoint(t!("panels8-hist-delete-sampler"));
                        doc.samplers.remove(i);
                    }
                }
                Some(index) => {
                    self.measure.sampler_drag = Some(SamplerDrag {
                        index,
                        moved: false,
                    });
                }
                None if alt => {}
                None => {
                    // Follows the pointer in the step that adds it.
                    self.measure.sampler_drag = self
                        .add_sampler(at)
                        .map(|index| SamplerDrag { index, moved: true });
                }
            }
        }
        let Some(mut drag) = self.measure.sampler_drag else {
            return if alt && self.sampler_at(view, p).is_some() {
                CursorIcon::Default
            } else if self.sampler_at(view, p).is_some() {
                CursorIcon::Move
            } else {
                CursorIcon::Crosshair
            };
        };
        let Some(doc) = &mut self.doc else {
            return CursorIcon::Crosshair;
        };
        let spot = ColorSampler {
            x: at.x.floor() + 0.5,
            y: at.y.floor() + 0.5,
        };
        if drag.index >= doc.samplers.len() {
            self.measure.sampler_drag = None;
            return CursorIcon::Crosshair;
        }
        // (At the release the view may move with the bars: it stays.)
        if down && doc.samplers[drag.index] != spot {
            if !drag.moved {
                doc.checkpoint(t!("panels8-hist-move-sampler"));
                drag.moved = true;
            }
            doc.samplers[drag.index] = spot;
        }
        if down {
            self.measure.sampler_drag = Some(drag);
            return CursorIcon::Move;
        }
        self.measure.sampler_drag = None;
        let (w, h) = (doc.width() as f32, doc.height() as f32);
        let s = doc.samplers[drag.index];
        if !(0.0..w).contains(&s.x) || !(0.0..h).contains(&s.y) {
            doc.samplers.remove(drag.index);
        }
        CursorIcon::Crosshair
    }

    /// Options bar: every point goes, in one step.
    pub(super) fn clear_samplers(&mut self) {
        let Some(doc) = self.doc.as_mut().filter(|d| !d.samplers.is_empty()) else {
            return;
        };
        doc.checkpoint(t!("panels8-hist-clear-samplers"));
        doc.samplers.clear();
    }

    /// The part of the Ruler's line under `at` (screen).
    fn ruler_part(&self, view: View, at: Pos2) -> Option<RulerDrag> {
        let m = self.doc.as_ref()?.ruler?;
        let close = |p: (f32, f32)| (screen_of(view, p) - at).length() <= GRAB;
        if close(m.first) {
            return Some(RulerDrag::First);
        }
        if m.second.is_some_and(close) {
            return Some(RulerDrag::Second);
        }
        if close(m.vertex) {
            return Some(RulerDrag::Vertex);
        }
        let vertex = screen_of(view, m.vertex);
        let on_arm = near_segment(at, vertex, screen_of(view, m.first))
            || m.second
                .is_some_and(|s| near_segment(at, vertex, screen_of(view, s)));
        on_arm.then(|| {
            let p = view.to_image(at);
            RulerDrag::Line((p.x, p.y))
        })
    }

    /// Ruler tool: a drag draws the line (Shift: on 45° steps), its ends and
    /// the line can be dragged, Alt+drag from an end draws the protractor's
    /// other arm. A click off the line clears it.
    pub(super) fn ruler_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
        alt: bool,
    ) -> CursorIcon {
        let Some(p) = pointer else {
            self.measure.ruler_drag = None;
            return CursorIcon::Crosshair;
        };
        let at = view.to_image(p);
        let here = (at.x, at.y);
        if start {
            let part = self.ruler_part(view, p);
            let Some(doc) = &mut self.doc else {
                return CursorIcon::Crosshair;
            };
            self.measure.ruler_drag = Some(match (part, doc.ruler.as_mut()) {
                (Some(RulerDrag::First | RulerDrag::Vertex), Some(m))
                    if alt && m.second.is_none() =>
                {
                    // The end grabbed is the protractor's vertex.
                    if part == Some(RulerDrag::First) {
                        std::mem::swap(&mut m.vertex, &mut m.first);
                    }
                    m.second = Some(here);
                    RulerDrag::Second
                }
                (Some(part), _) => part,
                _ => {
                    doc.ruler = Some(Measure {
                        vertex: here,
                        first: here,
                        second: None,
                    });
                    RulerDrag::First
                }
            });
        }
        let Some(drag) = self.measure.ruler_drag else {
            return match self.ruler_part(view, p) {
                Some(RulerDrag::Line(_)) => CursorIcon::Move,
                Some(_) => CursorIcon::Crosshair,
                None => CursorIcon::Crosshair,
            };
        };
        let Some(m) = self.doc.as_mut().and_then(|d| d.ruler.as_mut()) else {
            self.measure.ruler_drag = None;
            return CursorIcon::Crosshair;
        };
        // (At the release the bar may wrap and move the view: it stays.)
        match drag {
            _ if !down => {}
            RulerDrag::Vertex => m.vertex = if shift { on_45(m.first, here) } else { here },
            RulerDrag::First => m.first = if shift { on_45(m.vertex, here) } else { here },
            RulerDrag::Second => {
                m.second = Some(if shift { on_45(m.vertex, here) } else { here });
            }
            RulerDrag::Line(from) => {
                let (dx, dy) = (here.0 - from.0, here.1 - from.1);
                let by = |p: (f32, f32)| (p.0 + dx, p.1 + dy);
                m.vertex = by(m.vertex);
                m.first = by(m.first);
                m.second = m.second.map(by);
                self.measure.ruler_drag = Some(RulerDrag::Line(here));
            }
        }
        if !down {
            self.measure.ruler_drag = None;
            // A click alone leaves no line (under half a screen pixel).
            let length = (screen_of(view, m.first) - screen_of(view, m.vertex)).length();
            if length < 0.5
                && m.second.is_none()
                && let Some(doc) = &mut self.doc
            {
                doc.ruler = None;
            }
        }
        CursorIcon::Crosshair
    }

    /// Options bar's Straighten Layer: the active layer turns so that the
    /// line lies on the nearest axis, and the canvas is cropped to what stays
    /// filled (`keep_canvas`, Alt: not cropped). The line goes. One step.
    pub(super) fn straighten_layer(&mut self, keep_canvas: bool) {
        if self.in_quick_mask() {
            self.status = t!("panels8-status-straighten-quick-mask");
            return;
        }
        let Some(m) = self.doc.as_ref().and_then(|d| d.ruler) else {
            return;
        };
        let angle = m.straighten_angle();
        let tool = self.tool;
        // The whole layer, whatever is selected (the selection stays).
        let selection = self.selection.take();
        let fit = std::mem::replace(&mut self.fit_canvas, false);
        let entered = self.enter_transform();
        let mut applied = false;
        if entered && angle.abs() > 0.01 {
            self.transform_angle = normalize_angle(angle);
            self.xform.step = Some(t!("panels8-straighten-layer"));
            self.commit_pending_aside();
            applied = true;
        } else if entered {
            self.reset_pending();
        }
        self.fit_canvas = fit;
        if self.selection.is_none() {
            self.selection = selection;
        }
        if entered {
            self.set_tool(tool);
        }
        if !applied {
            return;
        }
        if let Some(doc) = &mut self.doc {
            doc.ruler = None;
        }
        if !keep_canvas {
            self.after_transform(move |app| app.crop_to_straightened(angle));
        }
    }

    /// The canvas becomes what a turn by `angle` left filled, in the step of
    /// the turn.
    fn crop_to_straightened(&mut self, angle: f32) {
        let Some(doc) = &mut self.doc else { return };
        let (w, h) = (doc.width() as f32, doc.height() as f32);
        let (a, b) = crate::document::inner_size(w, h, angle);
        let (a, b) = (a.floor().max(1.0), b.floor().max(1.0));
        let r = PixelRect {
            x: ((w - a) / 2.0).round() as i64,
            y: ((h - b) / 2.0).round() as i64,
            w: a as u32,
            h: b as u32,
        };
        doc.fit_canvas_to(r);
        self.forget_selection();
        self.reset_pending();
    }

    /// The points (with the Eyedropper's tools, or the Info panel showing
    /// their values) and the Ruler's line (with its tool).
    pub(super) fn draw_measures(&self, painter: &Painter, view: View) {
        let Some(doc) = &self.doc else { return };
        let dark = UiStroke::new(3.0, Color32::from_black_alpha(170));
        let light = UiStroke::new(1.0, Color32::WHITE);
        let both = |a: Pos2, b: Pos2| {
            painter.line_segment([a, b], dark);
            painter.line_segment([a, b], light);
        };
        let tools = matches!(
            self.tool,
            Tool::Eyedropper | Tool::ColorSampler | Tool::Ruler
        );
        if tools || self.info.open {
            for (i, s) in doc.samplers.iter().enumerate() {
                let c = screen_of(view, (s.x, s.y));
                painter.circle_stroke(c, 5.0, dark);
                painter.circle_stroke(c, 5.0, light);
                both(c - vec2(9.0, 0.0), c + vec2(9.0, 0.0));
                both(c - vec2(0.0, 9.0), c + vec2(0.0, 9.0));
                let at = c + vec2(9.0, 3.0);
                let font = ui_kit::medium(11.0);
                let label = (i + 1).to_string();
                painter.text(
                    at + vec2(1.0, 1.0),
                    egui::Align2::LEFT_TOP,
                    &label,
                    font.clone(),
                    Color32::BLACK,
                );
                painter.text(at, egui::Align2::LEFT_TOP, label, font, Color32::WHITE);
            }
        }
        if self.tool == Tool::Ruler
            && let Some(m) = doc.ruler
        {
            let vertex = screen_of(view, m.vertex);
            let arms = [Some(m.first), m.second];
            for end in arms.into_iter().flatten() {
                let end = screen_of(view, end);
                both(vertex, end);
                both(end - vec2(5.0, 0.0), end + vec2(5.0, 0.0));
                both(end - vec2(0.0, 5.0), end + vec2(0.0, 5.0));
            }
            both(vertex - vec2(5.0, 0.0), vertex + vec2(5.0, 0.0));
            both(vertex - vec2(0.0, 5.0), vertex + vec2(0.0, 5.0));
        }
    }

    /// What the measure says, by short name (X, Y, W, H, A, L1, L2): the
    /// options bar's and the Info panel's words.
    pub(super) fn measure_values(&self) -> Option<Vec<(String, String)>> {
        let m = self.doc.as_ref()?.ruler?;
        let number = |v: f32| i18n::short_decimal(f64::from(v));
        let mut values = vec![
            (t!("panels8-short-x"), number(m.vertex.0)),
            (t!("panels8-short-y"), number(m.vertex.1)),
            (t!("panels8-short-w"), number(m.first.0 - m.vertex.0)),
            (t!("panels8-short-h"), number(m.first.1 - m.vertex.1)),
            (t!("panels8-short-angle"), format!("{}°", number(m.angle()))),
            (t!("panels8-short-l1"), number(m.length())),
        ];
        if let Some(l2) = m.second_length() {
            values.push((t!("panels8-short-l2"), number(l2)));
        }
        Some(values)
    }
}
