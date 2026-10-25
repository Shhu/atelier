//! Pen tools, Path Selection and Direct Selection, shape tools: drawing and
//! editing paths on the canvas (the Work Path, a saved path or a shape
//! layer's) and creating shape layers, like Photoshop's.

use super::keys;
use super::*;
use crate::document::PathRef;
use crate::vector::geom::{constrain_45, cubic_at};
use crate::vector::{
    Anchor, Bounds, Custom, Live, Paint, Path as VPath, PathOp, Pt, Shape, ShapeKind, ShapeStroke,
    SubPath, pt,
};

/// Distance (screen px) within which the pointer is on an anchor, a handle
/// or a segment.
const HIT: f32 = 6.0;
/// Freeform Pen: how far (screen px) the path may stray from the pointer's
/// trace (Photoshop's Curve Fit).
const CURVE_FIT: f32 = 2.0;
/// Side of an anchor's square on screen.
const ANCHOR_SIZE: f32 = 7.0;

/// Tools of the Pen group (P).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PenTool {
    Pen,
    Freeform,
    AddAnchor,
    DeleteAnchor,
    Convert,
}

/// Tools of the shape group (U).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ShapeTool {
    Rectangle,
    Ellipse,
    Triangle,
    Polygon,
    Line,
    Custom,
}

impl PenTool {
    pub(super) fn glyph(self) -> &'static str {
        match self {
            PenTool::Pen => icon::PEN_NIB,
            PenTool::Freeform => icon::SCRIBBLE,
            PenTool::AddAnchor => icon::PLUS_CIRCLE,
            PenTool::DeleteAnchor => icon::MINUS_CIRCLE,
            PenTool::Convert => icon::BEZIER_CURVE,
        }
    }

    pub(super) fn name(self) -> String {
        match self {
            PenTool::Pen => t!("vector-tool-pen"),
            PenTool::Freeform => t!("vector-tool-freeform-pen"),
            PenTool::AddAnchor => t!("vector-tool-add-anchor"),
            PenTool::DeleteAnchor => t!("vector-tool-delete-anchor"),
            PenTool::Convert => t!("vector-tool-convert-point"),
        }
    }
}

impl ShapeTool {
    pub(super) fn glyph(self) -> &'static str {
        match self {
            ShapeTool::Rectangle => icon::RECTANGLE,
            ShapeTool::Ellipse => icon::CIRCLE,
            ShapeTool::Triangle => icon::TRIANGLE,
            ShapeTool::Polygon => icon::PENTAGON,
            ShapeTool::Line => icon::LINE_SEGMENT,
            ShapeTool::Custom => icon::SHAPES,
        }
    }

    pub(super) fn name(self) -> String {
        match self {
            ShapeTool::Rectangle => t!("vector-tool-rectangle"),
            ShapeTool::Ellipse => t!("vector-tool-ellipse"),
            ShapeTool::Triangle => t!("vector-tool-triangle"),
            ShapeTool::Polygon => t!("vector-tool-polygon"),
            ShapeTool::Line => t!("vector-tool-line"),
            ShapeTool::Custom => t!("vector-tool-custom-shape"),
        }
    }

    /// Name of the layers it makes ("Rectangle 1"…).
    fn layer_kind(self) -> String {
        match self {
            ShapeTool::Rectangle => t!("vector-kind-rectangle"),
            ShapeTool::Ellipse => t!("vector-kind-ellipse"),
            ShapeTool::Triangle => t!("vector-kind-triangle"),
            ShapeTool::Polygon => t!("vector-kind-polygon"),
            ShapeTool::Line => t!("vector-kind-line"),
            ShapeTool::Custom => t!("vector-kind-shape"),
        }
    }
}

impl Tool {
    /// The Pen, path selection and shape tools.
    pub(super) fn is_vector(self) -> bool {
        matches!(
            self,
            Tool::Pen(_) | Tool::PathSelect | Tool::DirectSelect | Tool::Shape(_)
        )
    }
}

/// What the Pen and the shape tools make (the options bar's tool mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolMode {
    Shape,
    Path,
}

/// A gesture of the vector tools, from the press to the release.
enum Gesture {
    /// Pen: anchor `at` just placed (or, `closing`, the first one, closing
    /// the component): its handles follow the pointer. `broken`: only one
    /// of them (Alt). Convert Point records its step on the first move.
    Handles {
        path: PathRef,
        sub: usize,
        at: usize,
        closing: bool,
        broken: bool,
        moved: bool,
        stepped: bool,
    },
    /// The selected anchors follow the pointer from `from`.
    Anchors {
        path: PathRef,
        from: Pt,
        base: VPath,
        stepped: bool,
    },
    /// A handle dragged (`after`: the outgoing one); `independent`: it no
    /// longer turns the other one.
    Handle {
        path: PathRef,
        sub: usize,
        at: usize,
        after: bool,
        independent: bool,
        base: VPath,
        stepped: bool,
    },
    /// A curved segment bent through the point grabbed, at `t`.
    Segment {
        path: PathRef,
        sub: usize,
        seg: usize,
        t: f32,
        from: Pt,
        base: VPath,
        stepped: bool,
    },
    /// The selected components follow the pointer; `last`: where they
    /// were moved to, `aim`: where the pointer asks (moved whole, a shape
    /// follows by whole pixels and takes the remainder at the end).
    Components {
        path: PathRef,
        from: Pt,
        last: Pt,
        aim: Pt,
        base: VPath,
        stepped: bool,
        /// Alt: the components selected before their copies were made.
        picked: Option<Vec<usize>>,
    },
    /// Rubber band: selects the anchors (Direct Selection) or the
    /// components (Path Selection) it touches.
    Marquee { from: Pt, to: Pt },
    /// A shape tool's box, from `from`; `made`: where its outline went;
    /// `last`: the pointer before (Space held moves the box).
    ShapeBox {
        from: Pt,
        made: Option<(PathRef, usize)>,
        last: Pt,
    },
    /// Freeform Pen: the trace.
    Freehand { points: Vec<Pt> },
}

/// Settings of the vector tools, the paths' selection, the gesture in
/// progress, the Paths panel.
pub(super) struct VectorTools {
    pub pen_mode: ToolMode,
    pub shape_mode: ToolMode,
    /// How the next component combines with the current shape or path;
    /// `None`: a new shape layer.
    pub op: Option<PathOp>,
    /// Pen: a click on a segment adds an anchor, on an anchor deletes it.
    pub auto_add: bool,
    /// Fill of the new shapes; `None`: the foreground color.
    pub fill: Option<Paint>,
    pub stroke: ShapeStroke,
    /// Rectangle, triangle, polygon: corner radius.
    pub radius: f32,
    pub sides: u32,
    /// Polygon: Star Ratio (inner radius over outer one), 1 for none.
    pub star: f32,
    /// Line: weight and arrowheads (start, end).
    pub weight: f32,
    pub arrows: [bool; 2],
    pub custom: Custom,
    /// Path selected in the Paths panel (Work Path or a saved one): the one
    /// the tools edit; otherwise the active shape layer's.
    pub selected: Option<PathRef>,
    /// Selected anchors (component, anchor) and components of the path
    /// being edited.
    pub anchors: Vec<(usize, usize)>,
    pub components: Vec<usize>,
    /// Pen: the open component being drawn.
    pub drawing: Option<(PathRef, usize)>,
    gesture: Option<Gesture>,
    /// The document's effects held for the gesture (see
    /// `AtelierApp::hold_for_gesture`).
    holding: bool,
    /// Active layer at the last frame (a shape layer selected in the Layers
    /// panel takes the place of the selected path, as in Photoshop).
    last_active: Option<u64>,
    /// Paths panel (Window › Paths), its dialogs and settings.
    pub panel: super::paths_panel::PathsPanel,
}

impl Default for VectorTools {
    fn default() -> Self {
        Self {
            pen_mode: ToolMode::Path,
            shape_mode: ToolMode::Shape,
            op: None,
            auto_add: true,
            fill: None,
            stroke: ShapeStroke::default(),
            radius: 0.0,
            sides: 5,
            star: 1.0,
            weight: 1.0,
            arrows: [false; 2],
            custom: Custom::Arrow,
            selected: None,
            anchors: Vec::new(),
            components: Vec::new(),
            drawing: None,
            gesture: None,
            holding: false,
            last_active: None,
            panel: Default::default(),
        }
    }
}

impl VectorTools {
    /// Nothing selected in the path being edited.
    pub fn clear_selection(&mut self) {
        self.anchors.clear();
        self.components.clear();
    }
}

fn to_pt(p: Pos2) -> Pt {
    pt(p.x, p.y)
}

fn to_pos(p: Pt) -> Pos2 {
    pos2(p.x, p.y)
}

fn rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

/// The box of a shape tool's drag: Shift constrains it to a square (a line
/// to 45°), Alt draws it from its center.
fn shape_box(from: Pt, to: Pt, shift: bool, alt: bool, line: bool) -> Bounds {
    if line {
        let to = if shift { constrain_45(from, to) } else { to };
        return Bounds { min: from, max: to };
    }
    let mut d = to - from;
    if shift {
        let s = d.x.abs().max(d.y.abs());
        d = pt(s.copysign(d.x), s.copysign(d.y));
    }
    if alt {
        Bounds::of(from - d, from + d)
    } else {
        Bounds::of(from, from + d)
    }
}

/// An open trace turned into a few smooth anchors (Freeform Pen).
fn fit_trace(points: &[Pt], tolerance: f32, closed: bool) -> SubPath {
    // Ramer–Douglas–Peucker, then handles along the neighbors' direction.
    let mut kept = vec![false; points.len()];
    kept[0] = true;
    kept[points.len() - 1] = true;
    let mut spans = vec![(0, points.len() - 1)];
    while let Some((a, b)) = spans.pop() {
        let (pa, pb) = (points[a], points[b]);
        let d = pb - pa;
        let far = (a + 1..b)
            .map(|i| {
                let q = points[i] - pa;
                let l = d.len();
                let dist = if l > 1e-6 {
                    (q.x * d.y - q.y * d.x).abs() / l
                } else {
                    q.len()
                };
                (i, dist)
            })
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((i, dist)) = far
            && dist > tolerance
        {
            kept[i] = true;
            spans.extend([(a, i), (i, b)]);
        }
    }
    let mut pts: Vec<Pt> = (0..points.len())
        .filter(|&i| kept[i])
        .map(|i| points[i])
        .collect();
    if closed && pts.len() > 2 {
        pts.pop();
    }
    let n = pts.len();
    let anchors = (0..n)
        .map(|i| {
            let p = pts[i];
            let (prev, next) = if closed {
                (pts[(i + n - 1) % n], pts[(i + 1) % n])
            } else if i == 0 || i == n - 1 {
                return Anchor::corner(p);
            } else {
                (pts[i - 1], pts[i + 1])
            };
            let dir = next - prev;
            let dir = dir * (1.0 / dir.len().max(1e-6));
            Anchor {
                p,
                before: p - dir * (p.dist(prev) / 3.0),
                after: p + dir * (p.dist(next) / 3.0),
                smooth: true,
            }
        })
        .collect();
    SubPath::new(anchors, closed && n > 2)
}

impl AtelierApp {
    /// The path the vector tools edit: the one selected in the Paths panel,
    /// otherwise the active shape layer's.
    pub(super) fn path_target(&self) -> Option<PathRef> {
        let doc = self.doc.as_ref()?;
        match self.vector.selected {
            Some(PathRef::Work) if doc.work_path.is_some() => Some(PathRef::Work),
            Some(PathRef::Saved(i)) if i < doc.paths.len() => Some(PathRef::Saved(i)),
            _ => {
                let l = doc.active_layer();
                l.shape.is_some().then_some(PathRef::Layer(l.id))
            }
        }
    }

    pub(super) fn target_path(&self) -> Option<(PathRef, VPath)> {
        let r = self.path_target()?;
        Some((r, self.doc.as_ref()?.path(r)?))
    }

    /// Refuses edits of a locked shape layer's path.
    fn path_locked(&mut self, r: PathRef) -> bool {
        matches!(r, PathRef::Layer(_)) && self.refuse_if_locked()
    }

    fn mode(&self) -> ToolMode {
        match self.tool {
            Tool::Shape(_) => self.vector.shape_mode,
            _ => self.vector.pen_mode,
        }
    }

    /// Fill and stroke of a new shape.
    fn new_paints(&self) -> (Paint, ShapeStroke) {
        let fill = self
            .vector
            .fill
            .clone()
            .unwrap_or(Paint::Color(rgb(self.primary)));
        (fill, self.vector.stroke.clone())
    }

    /// The live shape of the shape tool `tool` in `rect` (canvas
    /// coordinates).
    fn live_shape(&self, tool: ShapeTool, rect: Bounds) -> Live {
        let v = &self.vector;
        let kind = match tool {
            ShapeTool::Rectangle => ShapeKind::Rectangle,
            ShapeTool::Ellipse => ShapeKind::Ellipse,
            ShapeTool::Triangle => ShapeKind::Triangle,
            ShapeTool::Polygon => ShapeKind::Polygon,
            ShapeTool::Line => ShapeKind::Line,
            ShapeTool::Custom => ShapeKind::Custom(v.custom),
        };
        Live {
            radii: [v.radius; 4],
            sides: v.sides,
            star: v.star,
            weight: v.weight,
            arrows: v.arrows,
            ..Live::new(kind, rect)
        }
    }

    /// Adds `sub` (canvas coordinates) as a new component where the tool
    /// puts it: in Shape mode, into the active shape layer with the path
    /// operation, or as a new shape layer (`live`: a live shape); in Path
    /// mode, into the selected path, or a new Work Path. One history step,
    /// `step`.
    fn add_component(
        &mut self,
        mut sub: SubPath,
        live: Option<Live>,
        step: String,
        kind: &str,
    ) -> Option<(PathRef, usize)> {
        let (fill, stroke) = self.new_paints();
        let mode = self.mode();
        let op = self.vector.op;
        let selected = self
            .path_target()
            .filter(|r| !matches!(r, PathRef::Layer(_)));
        let doc = self.doc.as_mut()?;
        let r = match mode {
            ToolMode::Shape => {
                let unlocked = !doc.is_locked(doc.active);
                let active = doc.active_layer();
                match (op, active.shape.is_some() && unlocked) {
                    (Some(op), true) => {
                        sub.op = op;
                        PathRef::Layer(active.id)
                    }
                    _ => {
                        let shape = match live {
                            Some(l) => Shape::live(l, fill, stroke),
                            None => Shape {
                                path: VPath {
                                    subpaths: vec![sub],
                                },
                                fill,
                                stroke,
                                live: None,
                            },
                        };
                        doc.add_shape_layer(step, kind, shape);
                        let id = doc.active_layer().id;
                        self.vector.last_active = Some(id);
                        self.vector.selected = None;
                        return Some((PathRef::Layer(id), 0));
                    }
                }
            }
            ToolMode::Path => {
                sub.op = op.unwrap_or_default();
                // A Work Path that isn't selected is replaced, as in Photoshop.
                selected.unwrap_or(PathRef::Work)
            }
        };
        let mut path = match (r, selected) {
            (PathRef::Work, None) => VPath::default(),
            _ => doc.path(r).unwrap_or_default(),
        };
        path.subpaths.push(sub);
        doc.checkpoint(step);
        doc.set_path(r, path.clone());
        if !matches!(r, PathRef::Layer(_)) {
            self.vector.selected = Some(r);
        }
        Some((r, path.subpaths.len() - 1))
    }

    /// Pointer on the canvas with a vector tool. Returns the cursor.
    pub(super) fn vector_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
    ) -> CursorIcon {
        let m = self.ctx.input(|i| i.modifiers);
        let tol = HIT / view.zoom;
        let cursor = match self.tool {
            Tool::PathSelect | Tool::DirectSelect => CursorIcon::Default,
            Tool::Pen(_) if m.command => CursorIcon::Default,
            _ => CursorIcon::Crosshair,
        };
        let Some(p) = pointer.map(|q| to_pt(view.to_image(q))) else {
            if !down {
                self.end_vector_gesture(None, view);
            }
            return cursor;
        };
        if start {
            self.vector_press(p, tol, m);
        } else if down {
            self.vector_drag(p, tol, m);
        }
        if !down && self.vector.gesture.is_some() {
            // Where it was released counts.
            self.vector_drag(p, tol, m);
            self.end_vector_gesture(Some(p), view);
        }
        if self.vector.gesture.is_some() {
            self.ctx.request_repaint();
        }
        self.hold_for_gesture();
        cursor
    }

    /// While a gesture edits a shape, its layer styles keep their images
    /// (drawn again once, at the end) instead of being drawn again and
    /// again on worker threads that slow the edit down.
    fn hold_for_gesture(&mut self) {
        let editing = self.vector.gesture.is_some();
        if self.vector.holding != editing
            && let Some(doc) = &mut self.doc
        {
            doc.hold_effects(editing);
            self.vector.holding = editing;
        }
    }

    fn vector_press(&mut self, p: Pt, tol: f32, m: Modifiers) {
        if self.doc.is_none() {
            return;
        }
        match self.tool {
            // Ctrl: Direct Selection for a moment.
            Tool::Pen(_) if m.command => self.direct_press(p, tol, m),
            Tool::Pen(PenTool::Pen) => self.pen_press(p, tol, m),
            Tool::Pen(PenTool::Freeform) => {
                self.vector.gesture = Some(Gesture::Freehand { points: vec![p] });
            }
            Tool::Pen(PenTool::AddAnchor) => {
                if let Some((r, path)) = self.target_path()
                    && let Some(h) = path.nearest(p, tol)
                {
                    self.add_anchor(r, path, h);
                }
            }
            Tool::Pen(PenTool::DeleteAnchor) => {
                if let Some((r, path)) = self.target_path()
                    && let Some((s, a)) = path.anchor_at(p, tol)
                {
                    self.delete_anchors(r, path, &[(s, a)]);
                }
            }
            Tool::Pen(PenTool::Convert) => self.convert_press(p, tol),
            Tool::DirectSelect => self.direct_press(p, tol, m),
            Tool::PathSelect => self.path_select_press(p, tol, m),
            Tool::Shape(_) => {
                self.vector.gesture = Some(Gesture::ShapeBox {
                    from: p,
                    made: None,
                    last: p,
                });
            }
            _ => {}
        }
    }

    /// Pen: a click adds a corner anchor, a drag pulls out its handles; on
    /// the first anchor it closes the component; Alt on the last one takes
    /// its handle back; on the selected path, Auto Add/Delete adds an anchor
    /// on a segment or deletes one; on an open end, drawing goes on from it.
    fn pen_press(&mut self, p: Pt, tol: f32, m: Modifiers) {
        if let Some((r, sub)) = self.vector.drawing
            && let Some(mut path) = self.doc.as_ref().and_then(|d| d.path(r))
            && let Some(s) = path.subpaths.get_mut(sub)
            && !s.closed
            && !s.anchors.is_empty()
        {
            if self.path_locked(r) {
                return;
            }
            let last = s.anchors.len() - 1;
            let doc = self.doc.as_mut().expect("document");
            if m.alt && s.anchors[last].p.dist(p) <= tol {
                // The next segment leaves it straight, or by a new handle.
                doc.checkpoint(t!("vector-hist-convert-point"));
                let a = &mut s.anchors[last];
                (a.after, a.smooth) = (a.p, false);
                doc.set_path(r, path);
                self.vector.gesture = Some(Gesture::Handles {
                    path: r,
                    sub,
                    at: last,
                    closing: false,
                    broken: true,
                    moved: false,
                    stepped: true,
                });
                return;
            }
            if s.anchors.len() >= 2 && s.anchors[0].p.dist(p) <= tol {
                doc.checkpoint(t!("vector-hist-close-path"));
                s.closed = true;
                doc.set_path(r, path);
                self.vector.drawing = None;
                self.vector.anchors = vec![(sub, 0)];
                self.vector.gesture = Some(Gesture::Handles {
                    path: r,
                    sub,
                    at: 0,
                    closing: true,
                    broken: m.alt,
                    moved: false,
                    stepped: true,
                });
                return;
            }
            let q = if m.shift {
                constrain_45(s.anchors[last].p, p)
            } else {
                p
            };
            doc.checkpoint(t!("vector-hist-new-anchor"));
            s.anchors.push(Anchor::corner(q));
            doc.set_path(r, path);
            self.vector.anchors = vec![(sub, last + 1)];
            self.vector.gesture = Some(Gesture::Handles {
                path: r,
                sub,
                at: last + 1,
                closing: false,
                broken: false,
                moved: false,
                stepped: true,
            });
            return;
        }
        self.vector.drawing = None;
        if let Some((r, mut path)) = self.target_path() {
            if let Some((si, ai)) = path.anchor_at(p, tol) {
                let s = &mut path.subpaths[si];
                let end = s.anchors.len() - 1;
                if !s.closed && (ai == 0 || ai == end) {
                    if self.path_locked(r) {
                        return;
                    }
                    // Drawing goes on from that end (the first: reversed).
                    if ai == 0 && end > 0 {
                        s.anchors.reverse();
                        for a in &mut s.anchors {
                            std::mem::swap(&mut a.before, &mut a.after);
                        }
                        if let Some(doc) = &mut self.doc {
                            doc.set_path(r, path);
                        }
                    }
                    self.vector.drawing = Some((r, si));
                    self.vector.anchors = vec![(si, end)];
                    return;
                }
                if self.vector.auto_add {
                    self.delete_anchors(r, path, &[(si, ai)]);
                    return;
                }
            }
            if self.vector.auto_add
                && let Some(h) = path.nearest(p, tol)
            {
                self.add_anchor(r, path, h);
                return;
            }
        }
        let first = SubPath::new(vec![Anchor::corner(p)], false);
        let kind = t!("vector-kind-shape");
        if let Some((r, sub)) = self.add_component(first, None, t!("vector-hist-new-anchor"), &kind)
        {
            self.vector.drawing = Some((r, sub));
            self.vector.anchors = vec![(sub, 0)];
            self.vector.components.clear();
            self.vector.gesture = Some(Gesture::Handles {
                path: r,
                sub,
                at: 0,
                closing: false,
                broken: false,
                moved: false,
                stepped: true,
            });
        }
    }

    /// Adds an anchor on the segment hit (Add Anchor Point, the Pen's Auto
    /// Add); dragging then moves it.
    fn add_anchor(&mut self, r: PathRef, mut path: VPath, h: crate::vector::geom::SegmentHit) {
        if self.path_locked(r) {
            return;
        }
        let at = path.insert_anchor(h.sub, h.seg, h.t);
        let Some(doc) = &mut self.doc else { return };
        doc.checkpoint(t!("vector-hist-add-anchor"));
        doc.set_path(r, path.clone());
        self.vector.anchors = vec![(h.sub, at)];
        let from = path.subpaths[h.sub].anchors[at].p;
        self.vector.gesture = Some(Gesture::Anchors {
            path: r,
            from,
            base: path,
            stepped: true,
        });
    }

    /// Deletes anchors, as one step (a component left without any goes).
    fn delete_anchors(&mut self, r: PathRef, mut path: VPath, which: &[(usize, usize)]) {
        if self.path_locked(r) || which.is_empty() {
            return;
        }
        let mut which = which.to_vec();
        which.sort_unstable();
        for &(s, a) in which.iter().rev() {
            if s < path.subpaths.len() && a < path.subpaths[s].anchors.len() {
                path.remove_anchor(s, a);
            }
        }
        if !self.drop_empty_shape(r, &path) {
            let Some(doc) = &mut self.doc else { return };
            doc.checkpoint(t!("vector-hist-delete-anchor"));
            doc.set_path(r, path);
        }
        self.vector.clear_selection();
        self.vector.drawing = None;
    }

    /// A shape layer left without any path is deleted (as the step):
    /// whether it was.
    fn drop_empty_shape(&mut self, r: PathRef, path: &VPath) -> bool {
        let PathRef::Layer(id) = r else { return false };
        let Some(doc) = self.doc.as_mut().filter(|_| path.is_empty()) else {
            return false;
        };
        let Some(i) = doc.layer_index(id) else {
            return false;
        };
        let active = doc.active;
        doc.active = i;
        if doc.delete_active() {
            return true;
        }
        doc.active = active;
        false
    }

    /// Convert Point: on a handle, drags it on its own; on an anchor, takes
    /// its handles back (a corner), or pulls out new ones with a drag.
    fn convert_press(&mut self, p: Pt, tol: f32) {
        let Some((r, mut path)) = self.target_path() else {
            return;
        };
        if let Some((sub, at, after)) = self.handle_at(&path, p, tol) {
            self.vector.gesture = Some(Gesture::Handle {
                path: r,
                sub,
                at,
                after,
                independent: true,
                base: path,
                stepped: false,
            });
            return;
        }
        let Some((sub, at)) = path.anchor_at(p, tol) else {
            return;
        };
        if self.path_locked(r) {
            return;
        }
        let a = &mut path.subpaths[sub].anchors[at];
        // With handles: a corner now; without: new ones if a drag follows.
        let had = a.before != a.p || a.after != a.p;
        if had {
            (a.before, a.after, a.smooth) = (a.p, a.p, false);
            let Some(doc) = &mut self.doc else { return };
            doc.checkpoint(t!("vector-hist-convert-point"));
            doc.set_path(r, path);
        }
        self.vector.anchors = vec![(sub, at)];
        self.vector.gesture = Some(Gesture::Handles {
            path: r,
            sub,
            at,
            closing: false,
            broken: false,
            moved: false,
            stepped: had,
        });
    }

    /// The handle of a selected anchor at `p`: component, anchor, whether
    /// it is the outgoing one.
    fn handle_at(&self, path: &VPath, p: Pt, tol: f32) -> Option<(usize, usize, bool)> {
        self.vector.anchors.iter().find_map(|&(s, i)| {
            let a = path.subpaths.get(s)?.anchors.get(i)?;
            [(a.after, true), (a.before, false)]
                .into_iter()
                .find(|(h, _)| *h != a.p && h.dist(p) <= tol)
                .map(|(_, after)| (s, i, after))
        })
    }

    /// The topmost shown, unlocked shape layer whose shape is at `p`.
    fn shape_layer_at(&self, p: Pt, tol: f32) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        (0..doc.layers.len()).rev().find(|&i| {
            let l = &doc.layers[i];
            l.shape.is_some()
                && !doc.is_locked(i)
                && doc.shown(i)
                && doc
                    .path(PathRef::Layer(l.id))
                    .is_some_and(|path| path.component_at(p, tol).is_some())
        })
    }

    /// Makes shape layer `i` active, its path the one edited.
    fn pick_shape_layer(&mut self, i: usize) {
        self.select_layer(i);
        self.vector.last_active = self.doc.as_ref().map(|d| d.active_layer().id);
        self.vector.selected = None;
        self.vector.clear_selection();
    }

    /// Direct Selection: handles, anchors and segments of the edited path;
    /// elsewhere, another shape layer or a rubber band.
    fn direct_press(&mut self, p: Pt, tol: f32, m: Modifiers) {
        let target = self.target_path();
        if let Some((r, path)) = target.clone() {
            if let Some((sub, at, after)) = self.handle_at(&path, p, tol) {
                self.vector.gesture = Some(Gesture::Handle {
                    path: r,
                    sub,
                    at,
                    after,
                    independent: m.alt,
                    base: path,
                    stepped: false,
                });
                return;
            }
            if let Some(hit) = path.anchor_at(p, tol) {
                let sel = &mut self.vector.anchors;
                if m.shift {
                    match sel.iter().position(|&a| a == hit) {
                        Some(k) => {
                            sel.remove(k);
                        }
                        None => sel.push(hit),
                    }
                } else if !sel.contains(&hit) {
                    *sel = vec![hit];
                }
                self.vector.components.clear();
                self.vector.gesture = Some(Gesture::Anchors {
                    path: r,
                    from: p,
                    base: path,
                    stepped: false,
                });
                return;
            }
            if let Some(h) = path.nearest(p, tol) {
                let s = &path.subpaths[h.sub];
                let [p0, c1, c2, p3] = s.segment(h.seg);
                let next = (h.seg + 1) % s.anchors.len();
                if c1 == p0 && c2 == p3 {
                    // A straight segment moves whole.
                    self.vector.anchors = vec![(h.sub, h.seg), (h.sub, next)];
                    self.vector.gesture = Some(Gesture::Anchors {
                        path: r,
                        from: p,
                        base: path,
                        stepped: false,
                    });
                } else {
                    self.vector.anchors.clear();
                    self.vector.gesture = Some(Gesture::Segment {
                        path: r,
                        sub: h.sub,
                        seg: h.seg,
                        t: h.t,
                        from: p,
                        base: path,
                        stepped: false,
                    });
                }
                self.vector.components.clear();
                return;
            }
        }
        if let Some(i) = self.shape_layer_at(p, tol) {
            let id = self.doc.as_ref().map(|d| d.layers[i].id);
            if target.is_none_or(|(r, _)| Some(r) != id.map(PathRef::Layer)) {
                self.pick_shape_layer(i);
                return;
            }
        }
        if !m.shift {
            self.vector.clear_selection();
        }
        self.vector.gesture = Some(Gesture::Marquee { from: p, to: p });
    }

    /// Path Selection: a component of the edited path (Shift: added to the
    /// selection; Alt: duplicated), or another shape layer's; elsewhere a
    /// rubber band.
    fn path_select_press(&mut self, p: Pt, tol: f32, m: Modifiers) {
        let mut target = self.target_path();
        let hit = target
            .as_ref()
            .and_then(|(_, path)| path.component_at(p, tol));
        if hit.is_none()
            && let Some(i) = self.shape_layer_at(p, tol)
        {
            self.pick_shape_layer(i);
            target = self.target_path();
        }
        let Some((r, mut path)) = target else {
            self.vector.gesture = Some(Gesture::Marquee { from: p, to: p });
            return;
        };
        let Some(c) = path.component_at(p, tol) else {
            if !m.shift {
                self.vector.clear_selection();
            }
            self.vector.gesture = Some(Gesture::Marquee { from: p, to: p });
            return;
        };
        let sel = &mut self.vector.components;
        if m.shift {
            match sel.iter().position(|&k| k == c) {
                Some(k) => {
                    sel.remove(k);
                    return;
                }
                None => sel.push(c),
            }
        } else if !sel.contains(&c) {
            *sel = vec![c];
        }
        self.vector.anchors.clear();
        let mut stepped = false;
        let mut picked = None;
        if m.alt && !self.path_locked(r) {
            // Alt-drag: copies of the components move.
            picked = Some(self.vector.components.clone());
            let sel = &mut self.vector.components;
            let copies: Vec<SubPath> = sel.iter().map(|&k| path.subpaths[k].clone()).collect();
            let n = path.subpaths.len();
            path.subpaths.extend(copies);
            *sel = (n..path.subpaths.len()).collect();
            let doc = self.doc.as_mut().expect("document");
            doc.checkpoint(t!("vector-hist-duplicate-path"));
            doc.set_path(r, path.clone());
            stepped = true;
        }
        self.vector.gesture = Some(Gesture::Components {
            path: r,
            from: p,
            last: p,
            aim: p,
            base: path,
            stepped,
            picked,
        });
    }

    /// Records the step of a drag on its first move.
    fn step_once(&mut self, stepped: &mut bool, r: PathRef, name: String) -> bool {
        if !*stepped {
            if self.path_locked(r) {
                return false;
            }
            if let Some(doc) = &mut self.doc {
                doc.checkpoint(name);
            }
            *stepped = true;
        }
        true
    }

    fn vector_drag(&mut self, p: Pt, tol: f32, m: Modifiers) {
        let Some(mut g) = self.vector.gesture.take() else {
            return;
        };
        let small = tol / 3.0;
        match &mut g {
            Gesture::Handles {
                path: r,
                sub,
                at,
                closing,
                broken,
                moved,
                stepped,
            } => {
                let near = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.path(*r))
                    .and_then(|path| Some(path.subpaths.get(*sub)?.anchors.get(*at)?.p))
                    .is_none_or(|a| a.dist(p) <= small);
                if (*moved || !near)
                    && self.step_once(stepped, *r, t!("vector-hist-convert-point"))
                    && let Some(mut path) = self.doc.as_ref().and_then(|d| d.path(*r))
                    && let Some(a) = path
                        .subpaths
                        .get_mut(*sub)
                        .and_then(|s| s.anchors.get_mut(*at))
                {
                    *moved = true;
                    let h = if m.shift { constrain_45(a.p, p) } else { p };
                    let mirror = a.p * 2.0 - h;
                    match (*closing, *broken || m.alt) {
                        (_, false) => (a.after, a.before, a.smooth) = (h, mirror, true),
                        (false, true) => (a.after, a.smooth) = (h, false),
                        (true, true) => (a.before, a.smooth) = (mirror, false),
                    }
                    if let Some(doc) = &mut self.doc {
                        doc.set_path(*r, path);
                    }
                }
            }
            Gesture::Anchors {
                path: r,
                from,
                base,
                stepped,
            } => {
                let to = if m.shift { constrain_45(*from, p) } else { p };
                let d = to - *from;
                if (*stepped || d.len() > small)
                    && self.step_once(stepped, *r, t!("vector-hist-drag-anchor"))
                {
                    let mut path = base.clone();
                    for &(s, i) in &self.vector.anchors {
                        if let Some(a) = path.subpaths.get_mut(s).and_then(|s| s.anchors.get_mut(i))
                        {
                            (a.p, a.before, a.after) = (a.p + d, a.before + d, a.after + d);
                        }
                    }
                    if let Some(doc) = &mut self.doc {
                        doc.set_path(*r, path);
                    }
                }
            }
            Gesture::Handle {
                path: r,
                sub,
                at,
                after,
                independent,
                base,
                stepped,
            } => {
                if self.step_once(stepped, *r, t!("vector-hist-drag-handle")) {
                    let mut path = base.clone();
                    if let Some(a) = path
                        .subpaths
                        .get_mut(*sub)
                        .and_then(|s| s.anchors.get_mut(*at))
                    {
                        let free = *independent || m.alt || !a.smooth;
                        let (moved, other) = if *after {
                            (&mut a.after, &mut a.before)
                        } else {
                            (&mut a.before, &mut a.after)
                        };
                        *moved = p;
                        if !free {
                            // The other one turns, keeping its length.
                            let len = other.dist(a.p);
                            let dir = a.p - p;
                            *other = a.p + dir * (len / dir.len().max(1e-6));
                        }
                        if *independent || m.alt {
                            a.smooth = false;
                        }
                    }
                    if let Some(doc) = &mut self.doc {
                        doc.set_path(*r, path);
                    }
                }
            }
            Gesture::Segment {
                path: r,
                sub,
                seg,
                t,
                from,
                base,
                stepped,
            } => {
                let d = p - *from;
                if (*stepped || d.len() > small)
                    && self.step_once(stepped, *r, t!("vector-hist-drag-segment"))
                {
                    let mut path = base.clone();
                    let s = &mut path.subpaths[*sub];
                    let next = (*seg + 1) % s.anchors.len();
                    // Both handles move so that the grabbed point follows.
                    let k = (3.0 * *t * (1.0 - *t)).max(0.05);
                    let delta = d * (1.0 / k);
                    for (i, after) in [(*seg, true), (next, false)] {
                        let a = &mut s.anchors[i];
                        let (h, other) = if after {
                            (&mut a.after, &mut a.before)
                        } else {
                            (&mut a.before, &mut a.after)
                        };
                        *h = *h + delta;
                        if a.smooth && *other != a.p {
                            let len = other.dist(a.p);
                            let dir = a.p - *h;
                            *other = a.p + dir * (len / dir.len().max(1e-6));
                        }
                    }
                    if let Some(doc) = &mut self.doc {
                        doc.set_path(*r, path);
                    }
                }
            }
            Gesture::Components {
                path: r,
                from,
                last,
                aim,
                base,
                stepped,
                ..
            } => {
                let to = if m.shift { constrain_45(*from, p) } else { p };
                *aim = to;
                let d = to - *from;
                if (*stepped || d.len() > small)
                    && self.step_once(stepped, *r, t!("vector-hist-move-path"))
                {
                    let all = (0..base.subpaths.len()).all(|k| self.vector.components.contains(&k));
                    if let Some(doc) = &mut self.doc {
                        if all {
                            // Moved whole: a live shape stays one. A
                            // layer's by whole pixels during the drag
                            // (its image then just moves, even zoomed
                            // in); the remainder at the end.
                            let mut step = to - *last;
                            if matches!(r, PathRef::Layer(_)) || self.zoom <= 1.0 {
                                step = pt(step.x.round(), step.y.round());
                            }
                            if step != pt(0.0, 0.0) {
                                doc.move_path(*r, step);
                            }
                            *last = *last + step;
                        } else {
                            let mut path = base.clone();
                            for &k in &self.vector.components {
                                if let Some(s) = path.subpaths.get_mut(k) {
                                    s.map(&|q| q + d);
                                }
                            }
                            doc.set_path(*r, path);
                            *last = to;
                        }
                    }
                }
            }
            Gesture::Marquee { to, .. } => *to = p,
            Gesture::ShapeBox { from, made, last } => {
                let Tool::Shape(tool) = self.tool else {
                    return;
                };
                // Space held: the box moves with the pointer, like Photoshop.
                if self.ctx.input(|i| i.key_down(Key::Space)) {
                    *from = *from + (p - *last);
                }
                *last = p;
                let line = tool == ShapeTool::Line;
                let rect = shape_box(*from, p, m.shift, m.alt, line);
                if made.is_none() && from.dist(p) > small {
                    let live = self.live_shape(tool, rect);
                    let sub = live.path().subpaths.remove(0);
                    *made = self.add_component(sub, Some(live), tool.name(), &tool.layer_kind());
                } else if let Some((r, k)) = *made {
                    let live = self.live_shape(tool, rect);
                    self.update_shape_component(r, k, live);
                }
            }
            Gesture::Freehand { points } => {
                if points.last().is_none_or(|l| l.dist(p) > small) {
                    points.push(p);
                }
            }
        }
        self.vector.gesture = Some(g);
    }

    /// The shape being drawn takes its new box: a live shape layer's
    /// settings, or component `k` of path `r`.
    fn update_shape_component(&mut self, r: PathRef, k: usize, live: Live) {
        let Some(doc) = &mut self.doc else { return };
        if let PathRef::Layer(id) = r
            && let Some(i) = doc.layer_index(id)
            && let Some(old) = doc.layers[i].shape.as_deref().filter(|s| s.live.is_some())
        {
            let (x, y) = doc.layers[i].offset;
            let mut local = live;
            let d = pt(-x as f32, -y as f32);
            local.rect = Bounds {
                min: live.rect.min + d,
                max: live.rect.max + d,
            };
            let shape = Shape::live(local, old.fill.clone(), old.stroke.clone());
            doc.set_shape(i, shape);
            return;
        }
        let Some(mut path) = doc.path(r) else { return };
        if let Some(s) = path.subpaths.get_mut(k) {
            let op = s.op;
            *s = live.path().subpaths.remove(0);
            s.op = op;
        }
        doc.set_path(r, path);
    }

    fn end_vector_gesture(&mut self, p: Option<Pt>, view: View) {
        let Some(g) = self.vector.gesture.take() else {
            return;
        };
        // Components dragged back to where they were (or Alt + click):
        // no step, and no copies.
        if let Gesture::Components {
            path: r,
            from,
            aim,
            base,
            stepped: true,
            picked,
            ..
        } = &g
            && aim == from
            && let Some(doc) = &mut self.doc
            && doc.path(*r).as_ref() == Some(base)
        {
            doc.discard_step();
            if let Some(picked) = picked {
                self.vector.components = picked.clone();
            }
            return;
        }
        match g {
            Gesture::Marquee { from, to } => {
                let b = Bounds::of(from, to);
                let Some((_, path)) = self.target_path() else {
                    return;
                };
                if self.tool == Tool::PathSelect {
                    for (k, s) in path.subpaths.iter().enumerate() {
                        if s.bounds().is_some_and(|sb| sb.intersects(&b))
                            && !self.vector.components.contains(&k)
                        {
                            self.vector.components.push(k);
                        }
                    }
                } else {
                    for (si, s) in path.subpaths.iter().enumerate() {
                        for (ai, a) in s.anchors.iter().enumerate() {
                            if b.contains(a.p) && !self.vector.anchors.contains(&(si, ai)) {
                                self.vector.anchors.push((si, ai));
                            }
                        }
                    }
                }
            }
            Gesture::Components {
                path: r,
                last,
                aim,
                stepped: true,
                ..
            } if aim != last && self.zoom > 1.0 && matches!(r, PathRef::Layer(_)) => {
                // Zoomed in, the fraction of a pixel the drag left.
                let all = self
                    .doc
                    .as_ref()
                    .and_then(|d| d.path(r))
                    .is_some_and(|path| {
                        (0..path.subpaths.len()).all(|k| self.vector.components.contains(&k))
                    });
                if all && let Some(doc) = &mut self.doc {
                    doc.move_path(r, aim - last);
                }
            }
            Gesture::ShapeBox {
                made: Some((r, k)), ..
            } => {
                if !matches!(r, PathRef::Layer(_)) {
                    self.vector.components = vec![k];
                }
            }
            Gesture::Freehand { points } => {
                let Some(end) = p.or(points.last().copied()) else {
                    return;
                };
                let mut points = points;
                points.push(end);
                if points.len() < 3 {
                    return;
                }
                let tol = CURVE_FIT / view.zoom;
                let closed = points.len() > 3 && points[0].dist(end) <= HIT / view.zoom;
                let sub = fit_trace(&points, tol, closed);
                let kind = t!("vector-kind-shape");
                if let Some((_, k)) =
                    self.add_component(sub, None, t!("vector-tool-freeform-pen"), &kind)
                {
                    self.vector.components = vec![k];
                    self.vector.anchors.clear();
                }
            }
            _ => {}
        }
    }

    /// Housekeeping each frame: what refers to a path that changed (undo,
    /// another layer) is dropped; another tool ends the drawing.
    fn sync_vector_state(&mut self) {
        let active = self.doc.as_ref().map(|d| d.active_layer().id);
        if active != self.vector.last_active {
            self.vector.last_active = active;
            self.vector.clear_selection();
            self.vector.drawing = None;
            // A shape layer chosen in the Layers panel: its path shows.
            if self
                .doc
                .as_ref()
                .is_some_and(|d| d.active_layer().shape.is_some())
            {
                self.vector.selected = None;
            }
        }
        if !self.tool.is_vector() {
            self.vector.drawing = None;
            self.vector.gesture = None;
        }
        self.hold_for_gesture();
        let path = self.target_path().map(|(_, p)| p);
        let Some(path) = path else {
            self.vector.clear_selection();
            self.vector.drawing = None;
            return;
        };
        let n = path.subpaths.len();
        self.vector.components.retain(|&k| k < n);
        self.vector
            .anchors
            .retain(|&(s, a)| s < n && a < path.subpaths[s].anchors.len());
        if let Some((r, s)) = self.vector.drawing
            && (Some(r) != self.path_target()
                || path
                    .subpaths
                    .get(s)
                    .is_none_or(|s| s.closed || s.anchors.is_empty()))
        {
            self.vector.drawing = None;
        }
    }

    /// What a Pen click at `p` would do, as a mark beside the cursor.
    fn pen_mark(&self, p: Pt, tol: f32, alt: bool) -> Option<&'static str> {
        if let Some((r, sub)) = self.vector.drawing {
            let path = self.doc.as_ref()?.path(r)?;
            let s = path.subpaths.get(sub)?;
            let last = s.anchors.last()?;
            return if alt && last.p.dist(p) <= tol {
                Some("^")
            } else if s.anchors.len() >= 2 && s.anchors[0].p.dist(p) <= tol {
                Some("o")
            } else {
                None
            };
        }
        let (_, path) = self.target_path()?;
        if let Some((si, ai)) = path.anchor_at(p, tol) {
            let s = &path.subpaths[si];
            if !s.closed && (ai == 0 || ai + 1 == s.anchors.len()) {
                return Some("/");
            }
            return self.vector.auto_add.then_some("−");
        }
        (self.vector.auto_add && path.nearest(p, tol).is_some()).then_some("+")
    }

    /// The edited path, its anchors and handles, the Pen's next segment,
    /// the rubber band. Also keeps the tools' state in step with the
    /// document.
    pub(super) fn draw_vector_overlay(
        &mut self,
        painter: &Painter,
        view: View,
        pointer: Option<Pos2>,
    ) {
        self.sync_vector_state();
        let vector_tool = self.tool.is_vector();
        if !vector_tool && self.vector.selected.is_none() {
            return;
        }
        let Some((_, path)) = self.target_path() else {
            if let Some(Gesture::Marquee { from, to }) = &self.vector.gesture {
                draw_marquee(painter, view, *from, *to);
            }
            return;
        };
        let line = UiStroke::new(1.0, ui_kit::color::ACCENT_HOVER);
        for s in &path.subpaths {
            let points: Vec<Pos2> = s
                .flatten(0.3 / view.zoom)
                .into_iter()
                .map(|q| view.to_screen(to_pos(q)))
                .collect();
            if s.closed {
                painter.add(egui::Shape::closed_line(points, line));
            } else {
                painter.add(egui::Shape::line(points, line));
            }
        }
        if !vector_tool {
            return;
        }
        // The Pen's next segment, from the last anchor to the pointer.
        if let (Some((_, sub)), None, Some(q), Tool::Pen(PenTool::Pen)) = (
            self.vector.drawing,
            &self.vector.gesture,
            pointer,
            self.tool,
        ) && let Some(last) = path.subpaths.get(sub).and_then(|s| s.anchors.last())
        {
            let to = to_pt(view.to_image(q));
            let seg = [last.p, last.after, to, to];
            let points: Vec<Pos2> = (0..=24)
                .map(|i| view.to_screen(to_pos(cubic_at(&seg, i as f32 / 24.0))))
                .collect();
            painter.add(egui::Shape::line(
                points,
                UiStroke::new(1.0, ui_kit::color::ACCENT_HOVER.gamma_multiply(0.6)),
            ));
        }
        let path_select = self.tool == Tool::PathSelect;
        let shows_handles = !path_select;
        for (si, s) in path.subpaths.iter().enumerate() {
            let whole = self.vector.components.contains(&si);
            if path_select && !whole {
                continue;
            }
            for (ai, a) in s.anchors.iter().enumerate() {
                let selected = whole || self.vector.anchors.contains(&(si, ai));
                if selected && shows_handles {
                    for h in [a.before, a.after] {
                        if h != a.p {
                            let (sa, sh) = (view.to_screen(to_pos(a.p)), view.to_screen(to_pos(h)));
                            painter.line_segment([sa, sh], line);
                            painter.circle(sh, 3.0, ui_kit::color::ACCENT_HOVER, UiStroke::NONE);
                        }
                    }
                }
                let c = view.to_screen(to_pos(a.p));
                let r = Rect::from_center_size(c, Vec2::splat(ANCHOR_SIZE));
                let fill = if selected {
                    ui_kit::color::ACCENT_HOVER
                } else {
                    Color32::WHITE
                };
                painter.rect(r, 0.0, fill, line, StrokeKind::Inside);
            }
        }
        if let Some(Gesture::Marquee { from, to }) = &self.vector.gesture {
            draw_marquee(painter, view, *from, *to);
        }
        // The Pen's mark: what a click would do there.
        if let (Tool::Pen(PenTool::Pen), Some(q), None) = (self.tool, pointer, &self.vector.gesture)
        {
            let alt = self.ctx.input(|i| i.modifiers.alt);
            if let Some(mark) = self.pen_mark(to_pt(view.to_image(q)), HIT / view.zoom, alt) {
                painter.text(
                    q + vec2(9.0, 9.0),
                    egui::Align2::LEFT_TOP,
                    mark,
                    egui::FontId::proportional(12.0),
                    Color32::WHITE,
                );
            }
        }
    }

    /// Keys of the vector tools: Enter / Esc end the path being drawn (Esc
    /// then drops the selection of anchors, then hides the path), Delete /
    /// Backspace delete the last anchor drawn or what is selected,
    /// Ctrl+Enter loads the path as a selection. Whether they took a key.
    pub(super) fn vector_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.doc.is_none() || !(self.tool.is_vector() || self.vector.selected.is_some()) {
            return false;
        }
        let (selection, enter, escape, delete) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Enter),
                i.key_pressed(Key::Enter) && i.modifiers.is_none(),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
            )
        });
        if selection {
            let command = super::paths_panel::PathCommand::MakeSelection;
            self.run(Action::Path(command, false));
            return true;
        }
        let consume = |ctx: &egui::Context, key: Key| {
            ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        };
        if (enter || escape) && self.vector.drawing.is_some() {
            self.vector.drawing = None;
            consume(ctx, Key::Enter);
            consume(ctx, Key::Escape);
            return true;
        }
        if !self.tool.is_vector() {
            return false;
        }
        if escape {
            if !self.vector.anchors.is_empty() || !self.vector.components.is_empty() {
                self.vector.clear_selection();
            } else if self.vector.selected.is_some() {
                self.vector.selected = None;
            } else {
                return false;
            }
            consume(ctx, Key::Escape);
            return true;
        }
        if !delete {
            return false;
        }
        let Some((r, mut path)) = self.target_path() else {
            return false;
        };
        let took = if let Some((_, sub)) = self.vector.drawing {
            let last = path.subpaths.get(sub).map_or(0, |s| s.anchors.len());
            self.delete_anchors(r, path, &[(sub, last.saturating_sub(1))]);
            if last > 1 {
                self.vector.drawing = Some((r, sub));
                self.vector.anchors = vec![(sub, last - 2)];
            }
            true
        } else if !self.vector.anchors.is_empty() && self.tool != Tool::PathSelect {
            let which = self.vector.anchors.clone();
            self.delete_anchors(r, path, &which);
            true
        } else if !self.vector.components.is_empty() && !self.path_locked(r) {
            let mut which = self.vector.components.clone();
            which.sort_unstable();
            for &k in which.iter().rev() {
                path.subpaths.remove(k);
            }
            if self.drop_empty_shape(r, &path) {
                self.vector.clear_selection();
                consume(ctx, Key::Delete);
                consume(ctx, Key::Backspace);
                return true;
            }
            let doc = self.doc.as_mut().expect("document");
            doc.checkpoint(t!("vector-hist-delete-path"));
            if r == PathRef::Work && path.is_empty() {
                doc.work_path = None;
            } else {
                doc.set_path(r, path);
            }
            self.vector.clear_selection();
            true
        } else {
            false
        };
        if took {
            consume(ctx, Key::Delete);
            consume(ctx, Key::Backspace);
        }
        took
    }

    /// Arrow keys with Path / Direct Selection: the selected components or
    /// anchors move. Whether they did.
    pub(super) fn vector_nudge(&mut self, (dx, dy): (i64, i64)) -> bool {
        if !matches!(self.tool, Tool::PathSelect | Tool::DirectSelect) {
            return false;
        }
        let Some((r, mut path)) = self.target_path() else {
            return false;
        };
        let d = pt(dx as f32, dy as f32);
        if self.vector.anchors.is_empty() && self.vector.components.is_empty() {
            return false;
        }
        if self.path_locked(r) {
            return true;
        }
        let (anchors, components) = (&self.vector.anchors, &self.vector.components);
        let all = (0..path.subpaths.len()).all(|k| components.contains(&k));
        for &k in components {
            path.subpaths[k].map(&|q| q + d);
        }
        for &(s, i) in anchors {
            let a = &mut path.subpaths[s].anchors[i];
            (a.p, a.before, a.after) = (a.p + d, a.before + d, a.after + d);
        }
        let Some(doc) = &mut self.doc else {
            return true;
        };
        doc.checkpoint(t!("vector-hist-nudge"));
        if all {
            doc.move_path(r, d);
        } else {
            doc.set_path(r, path);
        }
        true
    }
}

impl AtelierApp {
    /// Keys of the vector tools and what they do (status bar).
    pub(super) fn vector_hints(&self, ctx: &egui::Context) -> Vec<(String, String)> {
        let shift = keys::modifiers(ctx, Modifiers::SHIFT);
        let alt = keys::modifiers(ctx, Modifiers::ALT);
        let ctrl = keys::modifiers(ctx, Modifiers::COMMAND);
        let click = t!("hint-key-click");
        let drag = t!("hint-key-drag");
        let alt_drag = keys::combo(ctx, Modifiers::ALT, &drag);
        let enter = keys::key(Key::Enter);
        let delete = keys::key(Key::Backspace);
        let to_selection = keys::combo(ctx, Modifiers::COMMAND, &enter);
        match self.tool {
            Tool::Pen(PenTool::Pen) => vec![
                (click, t!("vector-hint-corner")),
                (drag, t!("vector-hint-smooth")),
                (alt_drag, t!("vector-hint-one-handle")),
                (shift, t!("paint-hint-45")),
                (ctrl, t!("vector-tool-direct-selection")),
                (enter, t!("vector-hint-end-path")),
                (delete, t!("hint-remove-point")),
                (to_selection, t!("vector-hint-selection")),
            ],
            Tool::Pen(PenTool::Freeform) => vec![
                (drag, t!("sel-hint-draw")),
                (ctrl, t!("vector-tool-direct-selection")),
                (to_selection, t!("vector-hint-selection")),
            ],
            Tool::Pen(PenTool::AddAnchor) => vec![(click, t!("vector-hint-add-anchor"))],
            Tool::Pen(PenTool::DeleteAnchor) => vec![(click, t!("vector-hint-delete-anchor"))],
            Tool::Pen(PenTool::Convert) => vec![
                (click, t!("vector-hint-corner")),
                (drag, t!("vector-hint-smooth")),
            ],
            Tool::PathSelect => vec![
                (click, t!("vector-hint-select-component")),
                (shift, t!("hint-add")),
                (alt_drag, t!("vector-hint-duplicate")),
                (delete, t!("vector-hint-delete")),
                (to_selection, t!("vector-hint-selection")),
            ],
            Tool::DirectSelect => vec![
                (click, t!("vector-hint-select-anchor")),
                (shift, t!("hint-add")),
                (alt_drag, t!("vector-hint-one-handle")),
                (delete, t!("vector-hint-delete")),
                (to_selection, t!("vector-hint-selection")),
            ],
            _ => {
                let constrain = if self.tool == Tool::Shape(ShapeTool::Line) {
                    t!("paint-hint-45")
                } else {
                    t!("vector-hint-square")
                };
                vec![
                    (drag, t!("vector-hint-draw-shape")),
                    (shift, constrain),
                    (alt, t!("hint-from-center")),
                ]
            }
        }
    }
}

fn draw_marquee(painter: &Painter, view: View, from: Pt, to: Pt) {
    let r = Rect::from_two_pos(view.to_screen(to_pos(from)), view.to_screen(to_pos(to)));
    ui_kit::dashed_rect(
        painter,
        r,
        UiStroke::new(1.0, ui_kit::color::TEXT),
        4.0,
        3.0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_boxes_follow_shift_and_alt() {
        let (a, b) = (pt(10.0, 10.0), pt(30.0, 20.0));
        let r = shape_box(a, b, false, false, false);
        assert_eq!((r.min, r.max), (a, b));
        let sq = shape_box(a, b, true, false, false);
        assert_eq!(sq.max, pt(30.0, 30.0));
        let centered = shape_box(a, b, false, true, false);
        assert_eq!((centered.min, centered.max), (pt(-10.0, 0.0), b));
        let line = shape_box(a, pt(30.0, 12.0), true, false, true);
        assert!((line.max.y - 10.0).abs() < 1e-4);
    }

    #[test]
    fn a_freehand_trace_becomes_few_smooth_anchors() {
        let points: Vec<Pt> = (0..=100)
            .map(|i| {
                let a = i as f32 / 100.0 * std::f32::consts::PI;
                pt(50.0 - 40.0 * a.cos(), 50.0 - 40.0 * a.sin())
            })
            .collect();
        let s = fit_trace(&points, 1.0, false);
        assert!(
            s.anchors.len() < 15 && s.anchors.len() >= 3,
            "{}",
            s.anchors.len()
        );
        assert!(s.anchors[1].smooth && !s.closed);
        // The arc stays near the trace.
        for q in s.flatten(0.1) {
            assert!((q.dist(pt(50.0, 50.0)) - 40.0).abs() < 2.0);
        }
    }
}
