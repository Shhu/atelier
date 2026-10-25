//! Selection tools: marquees (rectangle, ellipse), lassos (freehand,
//! polygonal, magnetic), magic wand, subject selection and background
//! removal.

use std::sync::mpsc;

use image::GrayImage;

use super::models_window::AfterDownload;
use super::select_menu::Then;
use super::*;
use crate::ai;
use crate::i18n::t;
use crate::mask_ops::Modify;

/// Magnetic lasso in progress (canvas coordinates).
pub(super) struct Lasso {
    mode: SelectMode,
    edges: Arc<EdgeMap>,
    /// Fixed path, from the first point to the last anchor point.
    path: Vec<(u32, u32)>,
    /// Indices in `path` of the anchor points.
    anchors: Vec<usize>,
    /// Provisional path from the last anchor point to the pointer, and the
    /// target it was computed for.
    live: Vec<(u32, u32)>,
    target: Option<(u32, u32)>,
}

/// Image edges for the magnetic lasso, computed in the background for a
/// state of the layers (fingerprint).
#[derive(Default)]
pub(super) struct LassoEdges {
    ready: Option<(u64, Arc<EdgeMap>)>,
    job: Option<(u64, mpsc::Receiver<EdgeMap>)>,
    /// First click made before they were ready (canvas pixel, mode): the
    /// lasso starts there once they are.
    first_click: Option<((i64, i64), SelectMode)>,
}

impl LassoEdges {
    /// Forgets the click waiting for the edges.
    pub(super) fn cancel_click(&mut self) {
        self.first_click = None;
    }
}

/// Marquee being drawn.
#[derive(Clone, Copy)]
pub(super) struct Marquee {
    from: Pos2,
    mode: SelectMode,
    /// Shift (square) and Alt (from the center) active: false while the key
    /// that chose the mode at click time (Add, Subtract) is still held; once
    /// let go, pressing it again constrains, like Photoshop.
    shift_live: bool,
    alt_live: bool,
    /// Elliptical Marquee.
    ellipse: bool,
    /// Pointer in the previous frame (image): Space held moves the marquee.
    last: Pos2,
}

/// Style of the marquees: free, of fixed proportions, or of fixed size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MarqueeStyle {
    Normal,
    FixedRatio,
    FixedSize,
}

impl MarqueeStyle {
    pub(super) const ALL: [MarqueeStyle; 3] = [
        MarqueeStyle::Normal,
        MarqueeStyle::FixedRatio,
        MarqueeStyle::FixedSize,
    ];

    pub(super) fn label(self) -> String {
        match self {
            MarqueeStyle::Normal => t!("sel-style-normal"),
            MarqueeStyle::FixedRatio => t!("sel-style-fixed-ratio"),
            MarqueeStyle::FixedSize => t!("sel-style-fixed-size"),
        }
    }
}

/// Feather radius (px) and anti-aliasing of a selection tool's new shapes.
#[derive(Clone, Copy)]
pub(super) struct Edges {
    pub(super) feather: f32,
    pub(super) antialias: bool,
}

/// Options of the marquees, lassos and Magic Wand: each tool has its own
/// edges; the marquees share their style.
pub(super) struct ShapeOptions {
    /// Rectangular, Elliptical Marquee; Lasso, Polygonal, Magnetic Lasso;
    /// Magic Wand.
    edges: [Edges; 6],
    pub(super) style: MarqueeStyle,
    /// Width and height of Fixed Ratio.
    pub(super) ratio: (f32, f32),
    /// Width and height (px) of Fixed Size.
    pub(super) size: (u32, u32),
}

impl ShapeOptions {
    /// Edges of `tool`'s shapes (a selection tool).
    pub(super) fn edges(&mut self, tool: Tool) -> &mut Edges {
        &mut self.edges[Self::index(tool)]
    }

    /// The same, to read.
    pub(super) fn edges_of(&self, tool: Tool) -> Edges {
        self.edges[Self::index(tool)]
    }

    fn index(tool: Tool) -> usize {
        match tool {
            Tool::Select => 0,
            Tool::Ellipse => 1,
            Tool::FreeLasso => 2,
            Tool::PolyLasso => 3,
            Tool::Lasso => 4,
            _ => 5,
        }
    }
}

impl Default for ShapeOptions {
    fn default() -> Self {
        let edges = Edges {
            feather: 0.0,
            antialias: true,
        };
        Self {
            edges: [edges; 6],
            style: MarqueeStyle::Normal,
            ratio: (1.0, 1.0),
            size: (64, 64),
        }
    }
}

/// Lasso or Polygonal Lasso in progress (canvas coordinates).
pub(super) struct FreeLasso {
    mode: SelectMode,
    points: Vec<Pos2>,
    /// Indices in `points` of the points placed by clicks (Backspace takes
    /// the last one off).
    corners: Vec<usize>,
    /// Button held: the path follows the pointer.
    drawing: bool,
    /// End of the straight segment from the last point, toward the pointer.
    to: Option<Pos2>,
}

/// What Select Subject or Remove Background is asked on: the document
/// (its id, active layer's id, history position) and the selection.
pub(super) type SubjectContext = (Option<(u64, u64, u64)>, Option<Selection>);

/// Subject search in the background (a few hundred milliseconds).
pub(super) struct SubjectJob {
    /// Subject coverage, and a note to display (fallback method).
    result: mpsc::Receiver<(GrayImage, Option<String>)>,
    /// Document searched ([`Document::uid`]).
    doc: u64,
    /// Remove the background of this layer (id, pixel revision), otherwise
    /// select the subject.
    remove_from: Option<(u64, u64)>,
    /// The history generation searched in (see
    /// `Document::history_follows`), and the selection then (Select
    /// Subject: one made since is the user's latest).
    generation: crate::document::HistoryGeneration,
    base: Option<Selection>,
    /// The image searched (see `object_select::fingerprint`): searched
    /// again if it changed meanwhile, and with the AI model or not.
    fingerprint: u64,
    use_ai: bool,
    /// What was found, kept while a text is being typed.
    found: Option<(GrayImage, Option<String>)>,
}

/// Radius (screen pixels) around the first point to close the lasso.
const CLOSE_RADIUS: f32 = 8.0;
/// Distance (screen pixels) beyond which a press inside the selection
/// moves it rather than clicks.
const CLICK_DISTANCE: f32 = 3.0;
/// Magnetic lasso: longest search of a path (image pixels), and most
/// searches per frame toward a far pointer.
const LASSO_REACH: f32 = 128.0;
const LASSO_STEPS: usize = 8;

impl Tool {
    /// A selection tool: the arrows nudge the selection, Esc drops it.
    pub(super) fn selects(self) -> bool {
        matches!(
            self,
            Tool::Select
                | Tool::Ellipse
                | Tool::FreeLasso
                | Tool::PolyLasso
                | Tool::Lasso
                | Tool::ObjectSelect
                | Tool::MagicWand
        )
    }
}

impl AtelierApp {
    /// Combination mode of a new selection: Shift adds, Alt subtracts,
    /// both intersect; otherwise the mode from the options.
    pub(super) fn selection_mode(&self, shift: bool, alt: bool) -> SelectMode {
        // One being computed counts (a Shift+click right after a click).
        if self.selection.is_none() && !self.select_busy() {
            return SelectMode::Replace;
        }
        SelectMode::from_modifiers(shift, alt, self.select_mode)
    }

    /// Combines `new` with the current selection.
    pub(super) fn apply_selection(&mut self, new: Option<Selection>, mode: SelectMode) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let result = combined(self.selection.as_ref(), new, mode, canvas);
        if result.is_none() {
            self.deselect();
        }
        self.selection = result;
        self.commit_selection(None);
    }

    /// Marquee tools: drawing, or moving the selection (outline; Ctrl: its
    /// pixels, Ctrl+Alt: a copy).
    pub(super) fn select_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        shift: bool,
        alt: bool,
    ) -> CursorIcon {
        let Some(full) = self.doc.as_ref().map(Document::full_rect) else {
            return CursorIcon::Crosshair;
        };
        if self.select_move.is_some() {
            return self.move_selection(view, pointer, down, shift);
        }
        // A drag of the quick mask goes on as the Move tool's for a moment
        // (see `canvas`).
        if self.xform.quick.dragging() {
            return CursorIcon::Grabbing;
        }
        let Some(p) = pointer else {
            if !down {
                self.end_marquee();
            }
            return CursorIcon::Crosshair;
        };
        let ip = view.to_image(p);
        let ctrl = self.ctx.input(|i| i.modifiers.command);
        // Like Photoshop: dragging inside the selection moves its outline (in
        // New mode, without Shift or Alt); Ctrl, anywhere, its pixels.
        let plain = self.select_mode == SelectMode::Replace && !shift && !alt;
        let grab = self
            .selection
            .clone()
            .filter(|s| ctrl || (plain && s.contains(ip.x, ip.y)));
        if start {
            if let Some(selection) = grab {
                // In Quick Mask Mode, Ctrl moves the mask's selected part.
                if ctrl && self.in_quick_mask() {
                    return self.quick_mask_move_input(view, pointer, start, down);
                }
                if ctrl && !self.can_lift() {
                    return CursorIcon::NotAllowed;
                }
                self.select_move = Some(SelectMove {
                    from: ip,
                    selection,
                    pixels: ctrl.then_some(alt),
                    floating: None,
                    moved: false,
                });
                return self.move_selection(view, pointer, down, shift);
            }
            let mode = self.selection_mode(shift, alt);
            let modifiers_chose_mode = self.selection.is_some() && (shift || alt);
            if mode == SelectMode::Replace {
                self.deselect();
            }
            self.marquee = Some(Marquee {
                from: ip,
                mode,
                shift_live: !(modifiers_chose_mode && shift),
                alt_live: !(modifiers_chose_mode && alt),
                ellipse: self.tool == Tool::Ellipse,
                last: ip,
            });
        }
        // Space held during the drag moves the marquee, like Photoshop.
        if let Some(m) = &mut self.marquee {
            if self.ctx.input(|i| i.key_down(Key::Space)) {
                m.from += ip - m.last;
            }
            m.last = ip;
            m.shift_live |= !shift;
            m.alt_live |= !alt;
        }
        let Some(m) = self.marquee else {
            // The hand: what is under it can be taken and moved.
            return if grab.is_some() {
                CursorIcon::Grab
            } else {
                CursorIcon::Crosshair
            };
        };
        let rect = self.marquee_rect(m, ip, shift, alt);
        // A plain rectangle in New mode is the selection as it is drawn;
        // the other shapes are previewed, and made on release.
        let feathered = self.sel.shape.edges(self.tool).feather > 0.0;
        if m.mode == SelectMode::Replace && !m.ellipse && !feathered {
            self.selection = intersect(rect, full).map(Into::into);
        } else {
            self.marquee_preview = Some(rect);
        }
        if !down {
            self.end_marquee();
        }
        CursorIcon::Crosshair
    }

    /// Box of the marquee from `m.from` to the pointer `to`, as the Style
    /// option says; Shift: a square (circle), Alt: from the center.
    fn marquee_rect(&self, m: Marquee, to: Pos2, shift: bool, alt: bool) -> PixelRect {
        let o = &self.sel.shape;
        let centered = alt && m.alt_live;
        if o.style == MarqueeStyle::FixedSize {
            // The fixed box follows the pointer: by its corner, or its center.
            let (w, h) = o.size;
            let at = if centered {
                to - vec2(w as f32, h as f32) / 2.0
            } else {
                to
            };
            return PixelRect {
                x: at.x.round() as i64,
                y: at.y.round() as i64,
                w,
                h,
            };
        }
        let from = m.from;
        let mut d = to - from;
        if o.style == MarqueeStyle::FixedRatio {
            let (rw, rh) = (o.ratio.0.max(0.001), o.ratio.1.max(0.001));
            let k = (d.x.abs() / rw).max(d.y.abs() / rh);
            d = vec2((k * rw).copysign(d.x), (k * rh).copysign(d.y));
        } else if shift && m.shift_live {
            let side = d.x.abs().max(d.y.abs());
            d = vec2(side.copysign(d.x), side.copysign(d.y));
        }
        let corner = if centered { from - d } else { from };
        to_pixel_rect(Rect::from_two_pos(corner, from + d))
    }

    /// End of the marquee being drawn: a plain rectangle in New mode is
    /// already the selection; the other shapes are made (feathered) and
    /// combine with it.
    fn end_marquee(&mut self) {
        let Some(m) = self.marquee.take() else { return };
        self.record_marquee(m.ellipse, m.mode, self.marquee_preview);
        let Some(rect) = self.marquee_preview.take() else {
            // Drawn as it went (or a click: deselected).
            return self.commit_selection(None);
        };
        let Edges { antialias, feather } = *self.sel.shape.edges(self.tool);
        self.select_shape(rect, m.ellipse, m.mode, feather, antialias);
    }

    /// The step of a selection tool's shape is named after the tool now:
    /// computed in the background, it may land once another tool is
    /// chosen.
    fn name_tool_step(&mut self) {
        if self.tool.selects() && self.sel.step_name.is_none() {
            self.sel.step_name = Some(self.tool.name());
        }
    }

    /// A marquee's shape, made (feathered) and combined with the selection
    /// by `mode`.
    pub(super) fn select_shape(
        &mut self,
        rect: PixelRect,
        ellipse: bool,
        mode: SelectMode,
        feather: f32,
        antialias: bool,
    ) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let pixels = rect.w as u64 * rect.h as u64;
        self.name_tool_step();
        self.select_later(Then::Select(mode), pixels, move || {
            let shape = if ellipse {
                Selection::ellipse(rect, canvas, antialias)
            } else {
                intersect(rect, canvas).map(Into::into)
            };
            feathered(shape, feather, canvas)
        });
    }

    /// Ctrl+drag: the active layer's pixels can be lifted (otherwise says
    /// why not).
    pub(super) fn can_lift(&mut self) -> bool {
        if self.refuse_if_locked() {
            return false;
        }
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            return false;
        };
        if !layer.visible {
            self.status = t!("select-layer-hidden", name = layer.name.as_str());
        }
        layer.visible
    }

    /// Dragging of the current selection (Shift: horizontal or vertical).
    /// Pixels moved with Ctrl are lifted at the first real move, and put
    /// back on their layer on release. A click without moving deselects.
    pub(super) fn move_selection(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        down: bool,
        shift: bool,
    ) -> CursorIcon {
        let Some(mut m) = self.select_move.take() else {
            return CursorIcon::Crosshair;
        };
        if let Some(p) = pointer {
            let mut d = view.to_image(p) - m.from;
            if !m.moved && d.length() * view.zoom >= CLICK_DISTANCE {
                m.moved = true;
                if let Some(copy) = m.pixels {
                    let Some(id) = self
                        .doc
                        .as_mut()
                        .and_then(|doc| doc.lift_region(&m.selection, copy))
                    else {
                        self.status = t!("status-selection-empty");
                        return CursorIcon::NotAllowed;
                    };
                    let offset = self
                        .doc
                        .as_ref()
                        .map_or((0, 0), |d| d.active_layer().offset);
                    m.floating = Some((id, offset));
                }
            }
            if m.moved {
                if shift {
                    d = handles::constrain_axis(d);
                }
                let (dx, dy) = (d.x.round() as i64, d.y.round() as i64);
                self.selection = Some(m.selection.clone().translate(dx, dy));
                if let (Some((id, (x, y))), Some(doc)) = (m.floating, &mut self.doc)
                    && let Some(layer) = doc.layers.iter_mut().find(|l| l.id == id)
                {
                    layer.offset = (x + dx, y + dy);
                }
            }
        }
        let moved = m.moved;
        let outline_only = m.pixels.is_none();
        self.select_move = Some(m);
        if down {
            return CursorIcon::Grabbing;
        }
        if moved {
            self.finish_select_move();
        } else {
            self.select_move = None;
            if outline_only {
                self.deselect();
                self.commit_selection(None);
            }
        }
        CursorIcon::Grab
    }

    /// Arrows with a selection tool: the outline moves, and with `pixels`
    /// (Ctrl, the Move tool) the selected pixels too, one undo step per
    /// series of presses.
    pub(super) fn nudge_selection(&mut self, selection: Selection, d: (i64, i64), pixels: bool) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let Some(moved) = selection.clone().translate(d.0, d.1).within(canvas) else {
            return;
        };
        if pixels && self.in_quick_mask() {
            return self.nudge_quick_mask(d);
        }
        if pixels {
            if !self.can_lift() {
                return;
            }
            let Some(doc) = &mut self.doc else { return };
            let (before, layer_id) = (doc.history_step(), doc.active_layer().id);
            let Some(id) = doc.lift_region(&selection, false) else {
                self.status = t!("status-selection-empty");
                return;
            };
            let layer = doc.active_layer_mut();
            layer.offset = (layer.offset.0 + d.0, layer.offset.1 + d.1);
            doc.drop_floating(id);
            // The presses of a series: one step, like the layer's arrows.
            let key = (layer_id, "nudged pixels");
            if self.prop_session == Some((key, before)) {
                doc.join_last_step();
            }
            self.prop_session = Some((key, doc.history_step()));
            // Moved with the pixels, in their step.
            self.selection = Some(moved);
            return self.sync_selection();
        }
        self.selection = Some(moved);
        self.commit_selection(Some(t!("hist3-move-selection")));
    }

    /// Magic Wand: on click, the pixels of similar color.
    pub(super) fn wand_input(&mut self, view: View, pointer: Option<Pos2>, start: bool) {
        let (Some(p), Some(doc), true) = (pointer, &self.doc, start) else {
            return;
        };
        let ip = view.to_image(p);
        let (x, y) = (ip.x.floor() as i64, ip.y.floor() as i64);
        let tolerance = (self.wand_tolerance * 255.0).round() as u8;
        let canvas = doc.full_rect();
        // Active layer, or the whole image as displayed: flattened, then
        // filled, with the selection, in the background when large.
        type Pixels = Box<dyn FnOnce() -> Arc<image::RgbaImage> + Send>;
        let ((w, h), origin, pixels): (_, _, Pixels) = if self.wand_all_layers {
            let flatten = doc.flatten_later();
            (
                (canvas.w, canvas.h),
                (0, 0),
                Box::new(move || Arc::new(flatten())),
            )
        } else {
            let layer = doc.active_layer();
            let image = layer.shared_image();
            (
                layer.image().dimensions(),
                layer.offset,
                Box::new(move || image),
            )
        };
        let (lx, ly) = (x - origin.0, y - origin.1);
        if lx < 0 || ly < 0 || lx >= w as i64 || ly >= h as i64 {
            self.status = t!("select-wand-outside-layer");
            return;
        }
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let mode = self.selection_mode(shift, alt);
        // Anti-alias: edges softened by about a pixel.
        let feather = if self.sel.shape.edges(self.tool).antialias {
            1.0
        } else {
            0.0
        };
        let contiguous = self.wand_contiguous;
        let area = w as u64 * h as u64;
        self.name_tool_step();
        self.select_later(Then::Select(mode), area, move || {
            let image = pixels();
            let mask =
                crate::pixel_ops::magic_wand(&image, lx as u32, ly as u32, tolerance, contiguous);
            let found = Selection::from_mask(&mask, origin).and_then(|s| s.within(canvas));
            feathered(found, feather, canvas)
        });
    }

    /// Edges of the displayed image, for the magnetic lasso: computed in
    /// the background, again when a layer changes. `None` until ready.
    fn edge_map(&mut self) -> Option<Arc<EdgeMap>> {
        let doc = self.doc.as_ref()?;
        let key = object_select::fingerprint(doc);
        let edges = &mut self.edges;
        if let Some((k, map)) = &edges.ready
            && *k == key
        {
            return Some(Arc::clone(map));
        }
        if let Some((k, job)) = &edges.job {
            match job.try_recv() {
                Ok(map) if *k == key => {
                    let map = Arc::new(map);
                    edges.ready = Some((key, Arc::clone(&map)));
                    edges.job = None;
                    return Some(map);
                }
                // One computation at a time (the layers may change on every
                // frame of a slider).
                Err(mpsc::TryRecvError::Empty) => return None,
                // Outdated, or failed: computed again below.
                _ => {}
            }
        }
        // The layers share their pixels: the copy is cheap.
        let (layers, canvas) = (doc.layers.clone(), doc.full_rect());
        let (send, job) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Magnetic Lasso edges");
            let mut image = image::RgbaImage::new(canvas.w, canvas.h);
            crate::document::composite_layers(&mut image, canvas, &layers, false);
            let _ = send.send(EdgeMap::new(&image));
            ctx.request_repaint();
        });
        edges.job = Some((key, job));
        None
    }

    /// Magnetic lasso: a click sets the first point, the path then follows
    /// the edges up to the pointer; click: anchor point, double-click,
    /// Enter or click on the first point: close.
    pub(super) fn lasso_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        double_click: bool,
    ) -> CursorIcon {
        let width = self.lasso_width.round().max(1.0) as u32;
        if self.lasso.is_none() {
            // Edges prepared as soon as the tool is used; a first click
            // made before they are ready waits for them.
            let ready = self.edge_map();
            if start && let Some(p) = pointer {
                let ip = view.to_image(p);
                let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
                let mode = self.selection_mode(shift, alt);
                self.edges.first_click = Some(((ip.x.floor() as i64, ip.y.floor() as i64), mode));
            }
            let Some(((x, y), mode)) = self.edges.first_click else {
                return CursorIcon::Crosshair;
            };
            let Some(edges) = ready else {
                return CursorIcon::Progress;
            };
            self.edges.first_click = None;
            let first = edges.snap(x, y, width);
            self.lasso = Some(Lasso {
                mode,
                edges,
                path: vec![first],
                anchors: vec![0],
                live: Vec::new(),
                target: None,
            });
            return CursorIcon::Crosshair;
        }
        let Some(p) = pointer else {
            return CursorIcon::Crosshair;
        };
        let ip = view.to_image(p);
        let (x, y) = (ip.x.floor() as i64, ip.y.floor() as i64);
        let Some(lasso) = &mut self.lasso else {
            return CursorIcon::Crosshair;
        };

        // Provisional path up to the edge closest to the pointer. A far
        // pointer (fast move, zoomed out) is reached in steps, each with
        // automatic anchor points, so that each search stays small.
        let target = lasso.edges.snap(x, y, width);
        // Automatic anchor points, closer together as frequency increases.
        let spacing = (110.0 - self.lasso_frequency).max(10.0);
        let reach = (spacing * 2.0).max(LASSO_REACH);
        let mut reached = false;
        for _ in 0..LASSO_STEPS {
            let last = *lasso.path.last().expect("at least one point");
            let (dx, dy) = (
                target.0 as f32 - last.0 as f32,
                target.1 as f32 - last.1 as f32,
            );
            let distance = dx.hypot(dy);
            reached = distance <= reach;
            let goal = if reached {
                target
            } else {
                let t = reach / distance;
                let (gx, gy) = (last.0 as f32 + dx * t, last.1 as f32 + dy * t);
                lasso.edges.snap(gx as i64, gy as i64, width)
            };
            if lasso.target != Some(goal) {
                lasso.live = lasso.edges.path(last, goal, width * 2 + 8);
                lasso.target = Some(goal);
            }
            while scissors::length(&lasso.live) > spacing {
                let mut length = 0.0;
                let cut = lasso
                    .live
                    .windows(2)
                    .position(|w| {
                        length += scissors::length(w);
                        length > spacing * 0.7
                    })
                    .map_or(lasso.live.len() - 1, |i| i + 1);
                lasso.path.extend_from_slice(&lasso.live[1..=cut]);
                lasso.anchors.push(lasso.path.len() - 1);
                lasso.live.drain(..cut);
            }
            if reached {
                break;
            }
        }
        if !reached {
            // The rest at the next frames.
            self.ctx.request_repaint();
        }

        let first = lasso.path[0];
        let near_start = view
            .to_screen(pos2(first.0 as f32 + 0.5, first.1 as f32 + 0.5))
            .distance(p)
            <= CLOSE_RADIUS;
        if double_click || (start && near_start && lasso.path.len() > 2) {
            self.close_lasso();
        } else if start {
            // Manual anchor point at the end of the provisional path.
            let live = std::mem::take(&mut lasso.live);
            if live.len() > 1 {
                lasso.path.extend_from_slice(&live[1..]);
                lasso.anchors.push(lasso.path.len() - 1);
            }
            lasso.target = None;
        }
        if near_start {
            CursorIcon::PointingHand
        } else {
            CursorIcon::Crosshair
        }
    }

    /// Closes the lasso with the path along the edges back to the first
    /// point, and applies the selection.
    pub(super) fn close_lasso(&mut self) {
        let (Some(mut lasso), Some(canvas)) = (
            self.lasso.take(),
            self.doc.as_ref().map(Document::full_rect),
        ) else {
            return;
        };
        let live = std::mem::take(&mut lasso.live);
        lasso.path.extend(live.into_iter().skip(1));
        let (first, last) = (lasso.path[0], *lasso.path.last().expect("one point"));
        // Along the edges back to the first point; from far, a straight
        // line (the polygon closes itself).
        let far = first.0.abs_diff(last.0).max(first.1.abs_diff(last.1));
        if far as f32 <= LASSO_REACH * 4.0 {
            let back = lasso.edges.path(last, first, 16);
            lasso.path.extend(back.into_iter().skip(1));
        }
        let points: Vec<(f32, f32)> = lasso
            .path
            .iter()
            .map(|&(x, y)| (x as f32 + 0.5, y as f32 + 0.5))
            .collect();
        self.select_polygon(points, lasso.mode, canvas);
    }

    /// The closed lasso `points` (canvas coordinates) as a new selection,
    /// anti-aliased and feathered as the options say.
    fn select_polygon(&mut self, points: Vec<(f32, f32)>, mode: SelectMode, canvas: PixelRect) {
        let Edges { antialias, feather } = *self.sel.shape.edges(self.tool);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in &points {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        let pixels = ((x1 - x0).max(0.0) * (y1 - y0).max(0.0)) as u64;
        self.name_tool_step();
        self.select_later(Then::Select(mode), pixels, move || {
            feathered(
                Selection::from_polygon(&points, canvas, antialias),
                feather,
                canvas,
            )
        });
    }

    /// Lasso and Polygonal Lasso, like Photoshop. The Lasso follows the
    /// pointer while the button is held and closes on release; with Alt
    /// held at the release, clicks add straight segments until Alt is let
    /// go. The Polygonal Lasso adds a point per click (Shift: at 45°
    /// steps; Alt held: freehand while dragging) and closes on the first
    /// point, by a double-click or Enter.
    pub(super) fn free_lasso_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        start: bool,
        down: bool,
        double_click: bool,
    ) -> CursorIcon {
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let polygonal = self.tool == Tool::PolyLasso;
        let ip = pointer.map(|p| view.to_image(p));
        let Some(lasso) = &mut self.sel.free_lasso else {
            if let (true, Some(ip)) = (start, ip) {
                // The selection stays until the lasso closes, as with the
                // Magnetic Lasso.
                let mode = self.selection_mode(shift, alt);
                self.sel.free_lasso = Some(FreeLasso {
                    mode,
                    points: vec![ip],
                    corners: vec![0],
                    drawing: !polygonal,
                    to: None,
                });
            }
            return CursorIcon::Crosshair;
        };
        let first = lasso.points[0];
        let near_start = pointer.is_some_and(|p| view.to_screen(first).distance(p) <= CLOSE_RADIUS)
            && lasso.points.len() > 2;
        let mut close = false;
        if lasso.drawing {
            // Freehand: a point each time the pointer moves a screen pixel.
            if down {
                if let Some(ip) = ip
                    && lasso
                        .points
                        .last()
                        .is_none_or(|l| l.distance(ip) * view.zoom >= 1.0)
                {
                    lasso.points.push(ip);
                }
            } else {
                // Released: the Lasso closes, unless Alt goes on with
                // straight segments.
                close = !polygonal && !alt;
                lasso.drawing = false;
                let last = lasso.points.len() - 1;
                if lasso.corners.last() != Some(&last) {
                    lasso.corners.push(last);
                }
            }
        } else {
            let last = *lasso.points.last().expect("a first point");
            lasso.to = ip.map(|ip| {
                if polygonal && shift {
                    at_45(last, ip)
                } else {
                    ip
                }
            });
            if !polygonal && !alt {
                // Lasso: letting Alt go closes the straight segments.
                close = true;
            } else if double_click {
                close = true;
            } else if start {
                if near_start {
                    close = true;
                } else if let Some(to) = lasso.to {
                    lasso.points.push(to);
                    lasso.corners.push(lasso.points.len() - 1);
                    // Dragging from there: freehand (Alt held with the
                    // Polygonal Lasso, always with the Lasso).
                    lasso.drawing = !polygonal || alt;
                }
            }
        }
        if close {
            self.close_free_lasso();
        }
        if near_start {
            CursorIcon::PointingHand
        } else {
            CursorIcon::Crosshair
        }
    }

    /// Closes the Lasso or Polygonal Lasso in progress and applies its
    /// selection.
    pub(super) fn close_free_lasso(&mut self) {
        let (Some(lasso), Some(canvas)) = (
            self.sel.free_lasso.take(),
            self.doc.as_ref().map(Document::full_rect),
        ) else {
            return;
        };
        let points = lasso.points.iter().map(|p| (p.x, p.y)).collect();
        self.select_polygon(points, lasso.mode, canvas);
    }

    /// Keys of a gesture in progress. A lasso: Enter closes it, Esc cancels
    /// it, Backspace or Delete removes its last point; Transform Selection:
    /// Enter applies, Esc cancels. Returns `true` if one is in progress
    /// (other shortcuts are then ignored).
    pub(super) fn gesture_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.lasso.is_none() && self.sel.free_lasso.is_none() && self.sel.transform.is_none() {
            return false;
        }
        let (enter, escape, back) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Backspace)
                    | i.consume_key(Modifiers::NONE, Key::Delete),
            )
        });
        if self.sel.transform.is_some() {
            if enter {
                self.apply_selection_transform();
            } else if escape {
                self.sel.transform = None;
            }
            return true;
        }
        if let Some(lasso) = &mut self.sel.free_lasso {
            if back {
                if lasso.corners.len() > 1 {
                    lasso.corners.pop();
                    let keep = *lasso.corners.last().expect("first point");
                    lasso.points.truncate(keep + 1);
                    lasso.drawing = false;
                } else {
                    self.sel.free_lasso = None;
                }
            }
            if escape {
                self.sel.free_lasso = None;
            }
            if enter {
                self.close_free_lasso();
            }
            return true;
        }
        let Some(lasso) = &mut self.lasso else {
            return false;
        };
        if back {
            if lasso.anchors.len() > 1 {
                lasso.anchors.pop();
                let keep = *lasso.anchors.last().expect("first point");
                lasso.path.truncate(keep + 1);
                lasso.target = None;
            } else {
                self.lasso = None;
            }
        }
        if escape {
            self.lasso = None;
        }
        if enter {
            self.close_lasso();
        }
        true
    }

    /// Path of the lasso in progress: fixed in white outlined in black,
    /// provisional dashed, anchor points as small squares.
    pub(super) fn draw_lasso(&self, painter: &Painter, view: View) {
        if let Some(lasso) = &self.sel.free_lasso {
            let fixed: Vec<Pos2> = lasso.points.iter().map(|&p| view.to_screen(p)).collect();
            let live: Vec<Pos2> = match (lasso.drawing, lasso.to, fixed.last()) {
                (false, Some(to), Some(&last)) => vec![last, view.to_screen(to)],
                _ => Vec::new(),
            };
            draw_path(painter, fixed, false);
            draw_path(painter, live, true);
            if self.tool == Tool::PolyLasso {
                for &i in &lasso.corners {
                    draw_anchor(painter, view.to_screen(lasso.points[i]));
                }
            }
        }
        let Some(lasso) = &self.lasso else { return };
        let screen = |&(x, y): &(u32, u32)| view.to_screen(pos2(x as f32 + 0.5, y as f32 + 0.5));
        let fixed: Vec<Pos2> = lasso.path.iter().map(screen).collect();
        let live: Vec<Pos2> = lasso.live.iter().map(screen).collect();
        draw_path(painter, fixed, false);
        draw_path(painter, live, true);
        for &i in &lasso.anchors {
            draw_anchor(painter, screen(&lasso.path[i]));
        }
    }

    /// Marquee being drawn when it isn't the selection yet: a rectangle
    /// (within the canvas) or an ellipse.
    pub(super) fn draw_marquee_preview(&self, painter: &Painter, view: View) {
        let (Some(rect), Some(m), Some(doc)) = (self.marquee_preview, self.marquee, &self.doc)
        else {
            return;
        };
        if m.ellipse {
            let r = view.rect_to_screen(to_rect(rect));
            let n = 96;
            let points: Vec<Pos2> = (0..=n)
                .map(|i| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    r.center() + vec2(a.cos() * r.width(), a.sin() * r.height()) / 2.0
                })
                .collect();
            draw_path(painter, points, true);
        } else if let Some(rect) = intersect(rect, doc.full_rect()) {
            let r = view.rect_to_screen(to_rect(rect));
            painter.rect_stroke(
                r,
                0.0,
                UiStroke::new(1.0, Color32::BLACK),
                StrokeKind::Middle,
            );
            ui_kit::dashed_rect(painter, r, UiStroke::new(1.0, Color32::WHITE), 4.0, 4.0);
        }
    }

    /// Starts the subject search (Select › Subject), or, with
    /// `remove_background`, the background removal of the active layer.
    pub(super) fn find_subject(&mut self, remove_background: bool) {
        if self.subject_job.is_some() || self.doc.is_none() {
            return;
        }
        if remove_background && self.refuse_if_locked() {
            return;
        }
        if remove_background
            && self
                .doc
                .as_ref()
                .is_some_and(|d| !d.active_layer().has_pixels())
        {
            self.status = t!("tools3-status-remove-background-pixels");
            return;
        }
        let then = if remove_background {
            AfterDownload::RemoveBackground
        } else {
            AfterDownload::SelectSubject
        };
        // Model not downloaded yet: asked first, then continued once there
        // (if nothing changed meanwhile).
        if self.request_model(then) {
            self.start_subject(remove_background, true);
        } else {
            self.subject_waiting = Some(self.subject_context());
        }
    }

    pub(super) fn subject_context(&self) -> SubjectContext {
        let doc = self.doc.as_ref();
        (
            doc.map(|d| (d.uid(), d.active_layer().id, d.history_step())),
            self.selection.clone(),
        )
    }

    /// Starts the search in the background: with the AI model, or by
    /// GrabCut if `use_ai` is false or the AI fails.
    pub(super) fn start_subject(&mut self, remove_background: bool, use_ai: bool) {
        // From the selection being computed, as if it was there.
        self.wait_select_job();
        let Some(doc) = &self.doc else { return };
        // The image is made in the background (flattening takes a while).
        let (image, remove_from): (Box<dyn FnOnce() -> _ + Send>, _) = if remove_background {
            let layer = doc.active_layer();
            let pixels = layer.shared_image();
            (Box::new(move || pixels), Some((layer.id, layer.rev())))
        } else {
            let flatten = doc.flatten_later();
            (Box::new(move || Arc::new(flatten())), None)
        };
        let (send, result) = mpsc::channel();
        std::thread::spawn(move || {
            let _job = crate::diag::job(if use_ai {
                "Select Subject (AI)"
            } else {
                "Select Subject"
            });
            let image = image();
            let found = if use_ai {
                let found = ai::catch_panic(|| ai::subject::select_subject(&image));
                found.map(|mask| (mask, None)).unwrap_or_else(|e| {
                    let note = t!("select-ai-unavailable", error = e.shown);
                    (crate::grabcut::select_subject(&image), Some(note))
                })
            } else {
                (crate::grabcut::select_subject(&image), None)
            };
            // A selection only where something shows; a layer mask (Remove
            // Background) is applied over the layer's opacity already.
            let (mut mask, note) = found;
            if !remove_background {
                for (m, p) in mask.pixels_mut().zip(image.pixels()) {
                    m.0[0] = ((m.0[0] as u32 * p.0[3] as u32) / 255) as u8;
                }
            }
            let _ = send.send((mask, note));
        });
        self.subject_job = Some(SubjectJob {
            result,
            doc: doc.uid(),
            remove_from,
            generation: doc.history_generation(),
            base: self.selection.clone(),
            fingerprint: super::object_select::fingerprint(doc),
            use_ai,
            found: None,
        });
        self.status = t!("select-subject-searching");
    }

    /// The subject search is done (its result may wait for a text).
    #[cfg(test)]
    pub(super) fn subject_found(&self) -> bool {
        self.subject_job.as_ref().is_some_and(|j| j.found.is_some())
    }

    /// Applies the subject search result when it is ready.
    pub(super) fn poll_subject(&mut self, ctx: &egui::Context) {
        let Some(job) = &mut self.subject_job else { return };
        if job.found.is_none() {
            match job.result.try_recv() {
                Ok(found) => job.found = Some(found),
                // The status says so from the start (not every frame: other
                // messages stay readable).
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.subject_job = None;
                    self.status_failed(tm!("select-subject-failed"));
                    return;
                }
            }
        }
        // A text being typed ends first: its step is never recorded among
        // the text's own (given up with them if it is left empty).
        if self.type_tool.edit.is_some() {
            return;
        }
        let job = self.subject_job.take().expect("checked");
        let (mask, note) = job.found.expect("received");
        // Never on another document.
        if self.doc.as_ref().is_none_or(|d| d.uid() != job.doc) {
            return;
        }
        // A selection made before it lands first, in order.
        self.wait_select_job();
        let Some(doc) = self.doc.as_mut() else { return };
        // Undone or redone meanwhile: found in another image, and recording
        // it would wipe the redo list. Steps recorded forward don't matter.
        if !doc.history_follows(job.generation) {
            self.status = t!("hist5-subject-undone");
            return;
        }
        // The user selected something else meanwhile: theirs stays.
        let unchanged = match (&job.base, &self.selection) {
            (None, None) => true,
            (Some(a), Some(b)) => a.equal(b),
            _ => false,
        };
        if job.remove_from.is_none() && !unchanged {
            self.status = t!("hist5-subject-selection-changed");
            return;
        }
        // The image changed during the search (rotated, resized, painted,
        // a layer moved): found where it no longer is, searched again.
        if job.remove_from.is_none() && super::object_select::fingerprint(doc) != job.fingerprint {
            self.start_subject(false, job.use_ai);
            self.status = t!("hist6-subject-searching-again");
            return;
        }
        let remove_from = job.remove_from;
        if mask.pixels().all(|p| p.0[0] == 0) {
            self.status = t!("select-subject-not-found");
            return;
        }
        match remove_from {
            Some((id, rev)) => {
                // The layer must be the original one, still active and intact.
                let layer = doc.active_layer();
                if layer.id != id || layer.rev() != rev || !doc.mask_active(&mask) {
                    self.status = t!("select-layer-changed");
                    return;
                }
                let name = doc.active_layer().name.as_str();
                self.status = t!("select-background-removed", name = name);
                if let Some(note) = note {
                    self.status = format!("{} — {note}", self.status);
                }
                self.reset_pending();
            }
            None => {
                let canvas = doc.full_rect();
                let found = Selection::from_mask(&mask, (0, 0)).and_then(|s| s.within(canvas));
                self.sel.step_name = Some(t!("hist3-select-subject"));
                self.apply_selection(found, SelectMode::Replace);
                self.status = match note {
                    Some(note) => t!("select-subject-selected-note", note = note),
                    None => t!("select-subject-selected"),
                };
            }
        }
    }
}

/// A selection path: white over black, dashed if `dashed`.
fn draw_path(painter: &Painter, points: Vec<Pos2>, dashed: bool) {
    if points.len() < 2 {
        return;
    }
    painter.add(egui::Shape::line(
        points.clone(),
        UiStroke::new(3.0, Color32::BLACK),
    ));
    let white = UiStroke::new(1.0, Color32::WHITE);
    if dashed {
        painter.extend(egui::Shape::dashed_line(&points, white, 4.0, 4.0));
    } else {
        painter.add(egui::Shape::line(points, white));
    }
}

/// A point of a lasso: a small white square.
fn draw_anchor(painter: &Painter, at: Pos2) {
    let r = Rect::from_center_size(at, Vec2::splat(5.0));
    painter.rect_filled(r, 0.0, Color32::WHITE);
    painter.rect_stroke(
        r,
        0.0,
        UiStroke::new(1.0, Color32::BLACK),
        StrokeKind::Outside,
    );
}

/// `to` brought onto the nearest 45° direction from `from` (Shift).
fn at_45(from: Pos2, to: Pos2) -> Pos2 {
    let d = to - from;
    let step = std::f32::consts::FRAC_PI_4;
    let a = (d.y.atan2(d.x) / step).round() * step;
    from + vec2(a.cos(), a.sin()) * d.length()
}

/// `new` combined with `current` as `mode` says; with nothing new, an
/// addition or a subtraction leaves the selection as it was.
pub(super) fn combined(
    current: Option<&Selection>,
    new: Option<Selection>,
    mode: SelectMode,
    canvas: PixelRect,
) -> Option<Selection> {
    match new {
        Some(new) => Selection::combine(current, new, mode, canvas),
        None if matches!(mode, SelectMode::Add | SelectMode::Subtract) => current.cloned(),
        None => None,
    }
}

/// `shape` feathered by `radius` px (none: as it is).
pub(super) fn feathered(
    shape: Option<Selection>,
    radius: f32,
    canvas: PixelRect,
) -> Option<Selection> {
    match shape {
        Some(s) if radius > 0.0 => s.modified(Modify::Feather(radius), canvas, false),
        shape => shape,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    /// The edges are computed in the background: a click made meanwhile
    /// starts the lasso once they are ready. A far pointer is then reached
    /// in steps, over a few frames, along the edge.
    #[test]
    fn lasso_waits_for_edges_and_reaches_far_pointers() {
        // Dark top, light bottom: a horizontal edge at y = 50.
        let image = image::RgbaImage::from_fn(2000, 100, |_, y| {
            let v = if y < 50 { 20 } else { 230 };
            image::Rgba([v, v, v, 255])
        });
        let mut h = Harness::builder().build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(image, None));
            app.tool = Tool::Lasso;
            app
        });
        let view = View {
            origin: Pos2::ZERO,
            zoom: 1.0,
        };
        let app = h.state_mut();
        let first = pos2(10.5, 48.5);
        // The edges may already be ready (computed since the first frame);
        // otherwise the click waits for them.
        if app.lasso_input(view, Some(first), true, false) != CursorIcon::Progress {
            assert!(
                app.lasso.is_some(),
                "click neither waited nor started the lasso"
            );
        }
        let waited = std::time::Instant::now();
        while app.lasso.is_none() {
            assert!(waited.elapsed().as_secs() < 30, "edges never ready");
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.lasso_input(view, None, false, false);
        }
        let far = pos2(1990.5, 51.5);
        let mut frames = 0;
        loop {
            app.lasso_input(view, Some(far), false, false);
            frames += 1;
            let lasso = app.lasso.as_ref().expect("lasso in progress");
            if lasso.live.last().is_some_and(|p| p.0 >= 1985) {
                assert!(lasso.anchors.len() > 10, "{} anchors", lasso.anchors.len());
                let on_edge = |p: &(u32, u32)| (49..=50).contains(&p.1);
                assert!(lasso.path.iter().chain(&lasso.live).all(on_edge));
                break;
            }
            assert!(frames < 10, "far pointer not reached");
        }
        assert!(frames > 1, "reached in steps");
    }
}
