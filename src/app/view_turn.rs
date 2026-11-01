//! Rotated and flipped views: the one transform between the screen and the
//! canvas's upright space.
//!
//! Tools and overlays work in "upright" coordinates, as if the view weren't
//! turned ([`View`] maps them to the image). The canvas converts the pointer
//! once with [`ViewTurn::to_upright`], and [`ViewTurn::apply`] turns all that
//! was painted, as meshes: tiles, ants, handles and guides alike. Text stays
//! readable: it moves with the view but isn't turned.

use super::*;
use crate::i18n::t;
use egui::epaint::{ClippedShape, Mesh, Shape, Tessellator, Vertex};
use egui::layers::ShapeIdx;
use std::sync::Arc;

/// Rotation (and mirror) of a document's view; not saved, not in the history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ViewTurn {
    /// Degrees clockwise on screen, -180..=180.
    pub angle: f32,
    /// View › Flip Horizontal: mirrored before being rotated.
    pub flipped: bool,
    /// The fixed point: the center of the canvas area.
    pub center: Pos2,
}

impl Default for ViewTurn {
    fn default() -> Self {
        Self {
            angle: 0.0,
            flipped: false,
            center: Pos2::ZERO,
        }
    }
}

/// The image axis a ruler reads. It keeps the view's scale and passes through
/// the view's center: at the quarter turns each mark is what lies under it;
/// between them, a value and the pixel beside it can't agree along a whole
/// ruler, and the pointer's mark goes where its true coordinate is written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RulerAxis {
    pub image_x: bool,
    /// -1 when the values run backwards along the ruler.
    pub sign: f32,
}

impl RulerAxis {
    fn of(self, p: Pos2) -> f32 {
        if self.image_x { p.x } else { p.y }
    }

    /// Where upright point `p` reads on this ruler, whose center on screen
    /// is `middle`, the turn's center being `center`.
    pub(super) fn place(self, p: Pos2, center: Pos2, middle: f32) -> f32 {
        middle + self.sign * (self.of(p) - self.of(center))
    }
}

/// `angle` brought into -180..=180.
fn wrapped(angle: f32) -> f32 {
    let a = (angle + 180.0).rem_euclid(360.0) - 180.0;
    if a == -180.0 && angle > 0.0 { 180.0 } else { a }
}

impl ViewTurn {
    fn is_identity(self) -> bool {
        self.angle == 0.0 && !self.flipped
    }

    /// Cosine and sine, exact at the quarter turns (sharp lines stay sharp).
    fn axes(self) -> (f32, f32) {
        match self.angle {
            0.0 => (1.0, 0.0),
            90.0 => (0.0, 1.0),
            180.0 | -180.0 => (-1.0, 0.0),
            -90.0 => (0.0, -1.0),
            a => (a.to_radians().cos(), a.to_radians().sin()),
        }
    }

    /// A move on screen, in upright coordinates.
    pub(super) fn vec_to_upright(self, v: Vec2) -> Vec2 {
        let (c, s) = self.axes();
        let u = vec2(v.x * c + v.y * s, -v.x * s + v.y * c);
        if self.flipped { vec2(-u.x, u.y) } else { u }
    }

    fn vec_to_screen(self, v: Vec2) -> Vec2 {
        let (c, s) = self.axes();
        let f = if self.flipped { vec2(-v.x, v.y) } else { v };
        vec2(f.x * c - f.y * s, f.x * s + f.y * c)
    }

    /// Screen to upright coordinates.
    pub(super) fn to_upright(self, p: Pos2) -> Pos2 {
        if self.is_identity() {
            return p;
        }
        self.center + self.vec_to_upright(p - self.center)
    }

    /// Upright to screen coordinates.
    pub(super) fn to_screen(self, p: Pos2) -> Pos2 {
        if self.is_identity() {
            return p;
        }
        self.center + self.vec_to_screen(p - self.center)
    }

    /// Whether an upright rectangle shows at all in a screen rectangle: their
    /// two boxes meet in both spaces (separating axes of two rectangles).
    pub(super) fn shows(self, screen: Rect, r: Rect) -> bool {
        self.upright_bounds(screen).intersects(r)
            && Rect::from_points(
                &[
                    r.left_top(),
                    r.right_top(),
                    r.left_bottom(),
                    r.right_bottom(),
                ]
                .map(|p| self.to_screen(p)),
            )
            .intersects(screen)
    }

    /// What the top and left rulers read: each one the image axis nearest its
    /// direction on screen, running as that axis does.
    pub(super) fn ruler_axes(self) -> [RulerAxis; 2] {
        let along = self.vec_to_upright(vec2(1.0, 0.0));
        let down = self.vec_to_upright(vec2(0.0, 1.0));
        let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
        if along.x.abs() >= along.y.abs() {
            [
                RulerAxis { image_x: true, sign: sign(along.x) },
                RulerAxis { image_x: false, sign: sign(down.y) },
            ]
        } else {
            [
                RulerAxis { image_x: false, sign: sign(along.y) },
                RulerAxis { image_x: true, sign: sign(down.x) },
            ]
        }
    }

    /// All the upright space a screen rectangle can show.
    pub(super) fn upright_bounds(self, r: Rect) -> Rect {
        if self.is_identity() {
            return r;
        }
        Rect::from_points(
            &[
                r.left_top(),
                r.right_top(),
                r.left_bottom(),
                r.right_bottom(),
            ]
            .map(|p| self.to_upright(p)),
        )
    }

    /// Size on screen of the box of an upright rectangle (Fit on Screen).
    pub(super) fn turned_size(self, size: Vec2) -> Vec2 {
        let (c, s) = self.axes();
        vec2(
            size.x * c.abs() + size.y * s.abs(),
            size.x * s.abs() + size.y * c.abs(),
        )
    }

    /// A resize cursor for a handle, pointing where the handle now is.
    pub(super) fn cursor(self, icon: CursorIcon) -> CursorIcon {
        use CursorIcon as C;
        if self.is_identity() {
            return icon;
        }
        const ONE_WAY: [C; 8] = [
            C::ResizeNorth,
            C::ResizeNorthEast,
            C::ResizeEast,
            C::ResizeSouthEast,
            C::ResizeSouth,
            C::ResizeSouthWest,
            C::ResizeWest,
            C::ResizeNorthWest,
        ];
        const TWO_WAYS: [C; 4] = [
            C::ResizeVertical,
            C::ResizeNeSw,
            C::ResizeHorizontal,
            C::ResizeNwSe,
        ];
        let turn = |d: i32| {
            let d = if self.flipped { 8 - d } else { d };
            (d + (self.angle / 45.0).round() as i32).rem_euclid(8) as usize
        };
        let two_ways = match icon {
            C::ResizeRow => Some(0),
            C::ResizeColumn => Some(2),
            _ => TWO_WAYS.iter().position(|&i| i == icon).map(|d| d as i32),
        };
        if let Some(d) = two_ways {
            return TWO_WAYS[turn(d) % 4];
        }
        match ONE_WAY.iter().position(|&i| i == icon) {
            Some(d) => ONE_WAY[turn(d as i32)],
            None => icon,
        }
    }

    /// Turns what was painted on `layer` since `start`. Clip rectangles were
    /// upright: anything narrower than `upright` cuts the meshes, and all
    /// then clips to the canvas on screen.
    pub(super) fn apply(
        self,
        ctx: &egui::Context,
        layer: egui::LayerId,
        start: ShapeIdx,
        screen: Rect,
        upright: Rect,
    ) {
        if self.is_identity() {
            return;
        }
        let options = ctx.tessellation_options(|o| *o);
        let font_size = ctx.fonts(|f| f.font_image_size());
        let mut tess = Tessellator::new(ctx.pixels_per_point(), options, font_size, Vec::new());
        ctx.graphics_mut(|g| {
            let list = g.entry(layer);
            let shapes: Vec<&ClippedShape> = list.all_entries().skip(start.0).collect();
            let labels = label_moves(&shapes, self);
            let end = start.0 + shapes.len();
            for (i, moved) in (start.0..end).zip(labels) {
                list.mutate_shape(ShapeIdx(i), |c| {
                    let clip = (!c.clip_rect.contains_rect(upright)).then_some(c.clip_rect);
                    let shape = std::mem::replace(&mut c.shape, Shape::Noop);
                    c.shape = match moved {
                        Some(by) => {
                            let mut shape = shape;
                            shape.translate(by);
                            shape
                        }
                        None => self.turned(shape, clip, &mut tess),
                    };
                    c.clip_rect = screen;
                });
            }
        });
    }

    fn turned(self, shape: Shape, clip: Option<Rect>, tess: &mut Tessellator) -> Shape {
        match shape {
            Shape::Noop => Shape::Noop,
            Shape::Callback(_) => shape,
            Shape::Vec(shapes) => Shape::Vec(
                shapes
                    .into_iter()
                    .map(|s| self.turned(s, clip, tess))
                    .collect(),
            ),
            Shape::Text(mut text) => {
                let center = text.visual_bounding_rect().center();
                if clip.is_some_and(|c| !c.contains(center)) {
                    return Shape::Noop;
                }
                text.pos += self.to_screen(center) - center;
                Shape::Text(text)
            }
            Shape::Mesh(mesh) => self.turned_mesh(Arc::unwrap_or_clone(mesh), clip),
            other => {
                let mut mesh = Mesh::default();
                tess.tessellate_shape(other, &mut mesh);
                self.turned_mesh(mesh, clip)
            }
        }
    }

    fn turned_mesh(self, mesh: Mesh, clip: Option<Rect>) -> Shape {
        let mut mesh = match clip {
            Some(c) if !c.contains_rect(mesh.calc_bounds()) => clipped_mesh(&mesh, c),
            _ => mesh,
        };
        if mesh.is_empty() {
            return Shape::Noop;
        }
        for v in &mut mesh.vertices {
            v.pos = self.to_screen(v.pos);
        }
        Shape::Mesh(Arc::new(mesh))
    }
}

/// Labels (a filled box around a text, possibly underlined) stay upright
/// and move as one: how far each of those shapes goes, `None` for the others.
fn label_moves(shapes: &[&ClippedShape], turn: ViewTurn) -> Vec<Option<Vec2>> {
    let mut moves = vec![None; shapes.len()];
    let holds_text = |i: usize, r: Rect| {
        shapes.get(i).is_some_and(|c| match &c.shape {
            Shape::Text(t) => r.contains_rect(t.visual_bounding_rect()),
            _ => false,
        })
    };
    for (i, c) in shapes.iter().enumerate() {
        let Shape::Rect(rect) = &c.shape else {
            continue;
        };
        let group = if holds_text(i + 1, rect.rect) {
            2
        } else if matches!(
            shapes.get(i + 1).map(|c| &c.shape),
            Some(Shape::LineSegment { .. })
        ) && holds_text(i + 2, rect.rect)
        {
            3
        } else {
            continue;
        };
        let center = rect.rect.center();
        moves[i..i + group].fill(Some(turn.to_screen(center) - center));
    }
    moves
}

/// Center of the part of a convex polygon inside a rectangle; `None` when
/// they don't meet.
pub(super) fn center_within(polygon: &[Pos2], rect: Rect) -> Option<Pos2> {
    let edges: [&dyn Fn(Pos2) -> f32; 4] = [
        &|p| p.x - rect.left(),
        &|p| rect.right() - p.x,
        &|p| p.y - rect.top(),
        &|p| rect.bottom() - p.y,
    ];
    let mut points = polygon.to_vec();
    for edge in edges {
        let mut next = Vec::with_capacity(points.len() + 1);
        for (i, &a) in points.iter().enumerate() {
            let b = points[(i + 1) % points.len()];
            let (da, db) = (edge(a), edge(b));
            if da >= 0.0 {
                next.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                next.push(a + (b - a) * (da / (da - db)));
            }
        }
        points = next;
    }
    let first = *points.first()?;
    let (mut area, mut sum) = (0.0f64, [0.0f64; 2]);
    for pair in points.windows(2).skip(1) {
        let (u, v) = (pair[0] - first, pair[1] - first);
        let cross = f64::from(u.x) * f64::from(v.y) - f64::from(u.y) * f64::from(v.x);
        area += cross;
        sum[0] += cross * f64::from(u.x + v.x);
        sum[1] += cross * f64::from(u.y + v.y);
    }
    (area.abs() > 1e-6).then(|| {
        let at = |s: f64| (s / (3.0 * area)) as f32;
        first + vec2(at(sum[0]), at(sum[1]))
    })
}

/// A mesh's triangles cut by a rectangle.
fn clipped_mesh(mesh: &Mesh, clip: Rect) -> Mesh {
    let mut out = Mesh::with_texture(mesh.texture_id);
    let mix = |a: Vertex, b: Vertex, t: f32| Vertex {
        pos: a.pos + (b.pos - a.pos) * t,
        uv: a.uv + (b.uv - a.uv) * t,
        color: a.color.lerp_to_gamma(b.color, t),
    };
    // Inside when positive, for each of the four edges.
    let edges: [&dyn Fn(Pos2) -> f32; 4] = [
        &|p| p.x - clip.left(),
        &|p| clip.right() - p.x,
        &|p| p.y - clip.top(),
        &|p| clip.bottom() - p.y,
    ];
    let mut polygon = Vec::new();
    let mut next = Vec::new();
    for tri in mesh.indices.chunks_exact(3) {
        polygon.clear();
        polygon.extend(tri.iter().map(|&i| mesh.vertices[i as usize]));
        for edge in edges {
            next.clear();
            for (i, &a) in polygon.iter().enumerate() {
                let b: Vertex = polygon[(i + 1) % polygon.len()];
                let (da, db) = (edge(a.pos), edge(b.pos));
                if da >= 0.0 {
                    next.push(a);
                }
                if (da >= 0.0) != (db >= 0.0) {
                    next.push(mix(a, b, da / (da - db)));
                }
            }
            std::mem::swap(&mut polygon, &mut next);
            if polygon.is_empty() {
                break;
            }
        }
        let base = out.vertices.len() as u32;
        for i in 2..polygon.len() as u32 {
            out.indices.extend([base, base + i - 1, base + i]);
        }
        if polygon.len() >= 3 {
            out.vertices.extend_from_slice(&polygon);
        }
    }
    out
}

/// The Rotate View tool's gesture.
#[derive(Default)]
pub(super) struct RotateView {
    /// The angle at the press, and the pointer's around the center.
    drag: Option<(f32, f32)>,
    /// Hand, Rotate View or Zoom chosen over a transform or a crop box being
    /// prepared: it works, and the tool (with what it prepares) stays.
    pub(super) over: Option<Tool>,
    /// R held down: the tool to come back to when the key goes up, once the
    /// tool was used for a moment.
    spring_from: Option<Tool>,
    spring_used: bool,
}

impl AtelierApp {
    /// Something a change of tool would apply, that a view tool leaves alone.
    pub(super) fn view_tool_keeps_pending(&self) -> bool {
        self.transform_rect.is_some() || self.crop_box_pending() || self.sel.transform.is_some()
    }

    /// The tool that takes the pointer, for what the toolbar shows.
    pub(super) fn shown_tool(&self) -> Tool {
        self.rotate_view.over.unwrap_or(self.tool)
    }

    /// Sets the view's angle (with Rotate All Windows, of all the documents).
    pub(super) fn set_view_angle(&mut self, angle: f32) {
        let angle = wrapped(angle);
        self.view_turn.angle = angle;
        if self.rulers.settings.rotate_all_windows {
            for p in &mut self.parked {
                p.turn.angle = angle;
            }
        }
    }

    /// View › Flip Horizontal: the view only; the image doesn't change.
    pub(super) fn flip_view(&mut self) {
        self.view_turn.flipped = !self.view_turn.flipped;
    }

    /// The canvas area for this frame: the view turns around its center, on
    /// the pixel grid.
    pub(super) fn set_view_center(&mut self, area: Rect, ppp: f32) {
        self.view_turn.center = (area.center().to_vec2() * ppp).round().to_pos2() / ppp;
    }

    /// R went down: if it stays down while the tool is used, its release
    /// gives back `from`.
    pub(super) fn arm_rotate_spring(&mut self, from: Tool) {
        if from != Tool::RotateView {
            self.rotate_view.spring_from = Some(from);
            self.rotate_view.spring_used = false;
        }
    }

    /// The tool's drag: turns the view with the pointer, Shift by 15°.
    /// Takes the pointer in screen coordinates.
    pub(super) fn rotate_view_input(
        &mut self,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
    ) -> CursorIcon {
        let center = self.view_turn.center;
        if start && let Some(p) = pointer {
            let at = canvas::pointer_angle(p, center);
            self.rotate_view.drag = Some((self.view_turn.angle, at));
            self.rotate_view.spring_used = true;
        }
        let Some((from, grab)) = self.rotate_view.drag else {
            return CursorIcon::Grab;
        };
        if !down {
            self.rotate_view.drag = None;
            return CursorIcon::Grab;
        }
        if let Some(p) = pointer.filter(|&p| p != center) {
            let angle = from + canvas::pointer_angle(p, center) - grab;
            let angle = if shift {
                (angle / 15.0).round() * 15.0
            } else {
                angle
            };
            self.set_view_angle(angle);
        }
        CursorIcon::Grabbing
    }

    /// End of a spring-loaded use: the previous tool comes back when R is up
    /// and the drag over; R alone just keeps the tool.
    pub(super) fn rotate_spring_release(&mut self, ctx: &egui::Context) {
        let Some(from) = self.rotate_view.spring_from else {
            return;
        };
        if ctx.input(|i| i.key_down(Key::R)) || self.rotate_view.drag.is_some() {
            return;
        }
        self.rotate_view.spring_from = None;
        if !self.rotate_view.spring_used {
            return;
        }
        if self.rotate_view.over == Some(Tool::RotateView) {
            self.rotate_view.over = None;
        } else if self.tool == Tool::RotateView {
            self.set_tool(from);
        }
    }

    /// While the tool turns the view: the compass, its north where the
    /// top of the image now is.
    pub(super) fn draw_view_compass(&self, painter: &Painter) {
        if self.rotate_view.drag.is_none() {
            return;
        }
        const RADIUS: f32 = 44.0;
        let center = self.view_turn.center;
        painter.circle(
            center,
            RADIUS,
            Color32::from_black_alpha(120),
            UiStroke::new(1.5, Color32::from_white_alpha(160)),
        );
        let north = self.view_turn.vec_to_screen(vec2(0.0, -1.0));
        let side = north.rot90() * 7.0;
        let tip = center + north * (RADIUS - 5.0);
        let mesh = |points: Vec<Pos2>, color| {
            egui::epaint::PathShape::convex_polygon(points, color, UiStroke::NONE)
        };
        painter.add(mesh(
            vec![center + side, tip, center - side],
            Color32::from_rgb(220, 50, 40),
        ));
        painter.add(mesh(
            vec![
                center - side,
                center - north * (RADIUS - 5.0),
                center + side,
            ],
            Color32::from_gray(200),
        ));
    }

    /// Rotate View tool: the angle, Reset View, Rotate All Windows.
    pub(super) fn rotate_view_options(&mut self, ui: &mut egui::Ui) {
        let label = t!("view8-rotation-angle");
        let label_w = ui
            .painter()
            .layout_no_wrap(label.clone(), ui_kit::medium(11.0), ui_kit::color::LABEL)
            .size()
            .x;
        let mut angle = self.view_turn.angle;
        let has_doc = self.doc.is_some();
        let field = ui.add_enabled_ui(has_doc, |ui| {
            ui_kit::number_field(ui, &label, &mut angle, -180.0..=180.0, "°", label_w + 64.0)
        });
        if field.inner.changed() {
            self.set_view_angle(angle);
        }
        if ui_kit::button(
            ui,
            &t!("view8-reset-view"),
            ui_kit::Kind::Normal,
            None,
            has_doc && self.view_turn.angle != 0.0,
        )
        .clicked()
        {
            self.set_view_angle(0.0);
        }
        let mut all = self.rulers.settings.rotate_all_windows;
        if ui_kit::checkbox(ui, &mut all, &t!("view8-rotate-all-windows")).changed() {
            self.rulers.set_rotate_all_windows(all);
            if all {
                self.set_view_angle(self.view_turn.angle);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(angle: f32, flipped: bool) -> ViewTurn {
        ViewTurn {
            angle,
            flipped,
            center: pos2(100.0, 50.0),
        }
    }

    #[test]
    fn the_mapping_round_trips() {
        for t in [
            turn(0.0, true),
            turn(33.0, false),
            turn(-120.0, true),
            turn(90.0, false),
        ] {
            let p = pos2(17.0, 230.0);
            let back = t.to_upright(t.to_screen(p));
            assert!(back.distance(p) < 1e-3, "{t:?}: {back:?}");
            assert_eq!(t.to_screen(t.center), t.center);
        }
    }

    #[test]
    fn positive_angles_turn_clockwise_and_the_flip_mirrors_first() {
        // On screen, y goes down: to the right becomes downward.
        let t = turn(90.0, false);
        assert_eq!(t.to_screen(pos2(110.0, 50.0)), pos2(100.0, 60.0));
        let t = turn(90.0, true);
        assert_eq!(t.to_screen(pos2(110.0, 50.0)), pos2(100.0, 40.0));
        let t = turn(0.0, true);
        assert_eq!(t.to_screen(pos2(110.0, 50.0)), pos2(90.0, 50.0));
    }

    #[test]
    fn angles_wrap_into_the_field_range() {
        assert_eq!(wrapped(190.0), -170.0);
        assert_eq!(wrapped(-190.0), 170.0);
        assert_eq!(wrapped(180.0), 180.0);
        assert_eq!(wrapped(-180.0), -180.0);
        assert_eq!(wrapped(540.0), 180.0);
    }

    #[test]
    fn upright_bounds_hold_the_whole_screen_rect() {
        let t = turn(45.0, false);
        let r = Rect::from_center_size(t.center, vec2(100.0, 100.0));
        let b = t.upright_bounds(r);
        assert!((b.width() - 141.42).abs() < 0.1, "{b:?}");
        assert_eq!(turn(0.0, false).upright_bounds(r), r);
    }

    #[test]
    fn the_box_of_a_turned_image_fits_both_sides() {
        let s = turn(90.0, false).turned_size(vec2(300.0, 100.0));
        assert_eq!(s, vec2(100.0, 300.0));
        let s = turn(45.0, false).turned_size(vec2(100.0, 100.0));
        assert!((s.x - 141.42).abs() < 0.1, "{s:?}");
    }

    #[test]
    fn resize_cursors_follow_the_handles() {
        use CursorIcon as C;
        let t = turn(90.0, false);
        assert_eq!(t.cursor(C::ResizeEast), C::ResizeSouth);
        assert_eq!(t.cursor(C::ResizeVertical), C::ResizeHorizontal);
        assert_eq!(t.cursor(C::ResizeRow), C::ResizeHorizontal);
        assert_eq!(t.cursor(C::Move), C::Move);
        let t = turn(0.0, true);
        assert_eq!(t.cursor(C::ResizeEast), C::ResizeWest);
        assert_eq!(t.cursor(C::ResizeNwSe), C::ResizeNeSw);
    }

    #[test]
    fn a_clip_cuts_triangles_and_interpolates() {
        let mut mesh = Mesh::default();
        mesh.add_rect_with_uv(
            Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0)),
            Rect::from_min_size(Pos2::ZERO, vec2(1.0, 1.0)),
            Color32::WHITE,
        );
        let cut = clipped_mesh(&mesh, Rect::from_min_max(Pos2::ZERO, pos2(5.0, 10.0)));
        assert!(cut.calc_bounds().max.x <= 5.0);
        let u = cut.vertices.iter().map(|v| v.uv.x).fold(0.0, f32::max);
        assert!((u - 0.5).abs() < 1e-5);
        let outside = clipped_mesh(&mesh, Rect::from_min_size(pos2(20.0, 20.0), vec2(5.0, 5.0)));
        assert!(outside.is_empty());
    }
}
