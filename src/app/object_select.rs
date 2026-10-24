//! Object Selection tool, like Photoshop's: the image as displayed is
//! analysed once in the background (SAM 2.1); the object under the pointer
//! is then previewed, a click selects it, a rectangle drawn around an
//! object selects that object.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, mpsc};

use image::GrayImage;

use super::models_window::AfterDownload;
use super::*;
use crate::ai::models::Model;
use crate::ai::objects::{self, Features, Mask, Prompt};
use crate::i18n::t;

/// Distance (screen px) from which a press becomes a rectangle.
const DRAG_DISTANCE: f32 = 4.0;
/// Side (screen px) of the pointer cells: the preview is computed again
/// when the pointer enters another cell.
const HOVER_CELL: f32 = 6.0;
/// Longest side (px) of the preview of the object under the pointer.
const PREVIEW_SIZE: u32 = 1024;
/// Opacity of the preview's fill.
const PREVIEW_ALPHA: f32 = 0.35;

/// State of the Object Selection tool.
#[derive(Default)]
pub(super) struct ObjectSelect {
    /// Analysis of the image: fingerprint of the composite, features.
    features: Option<(u64, Arc<Features>)>,
    /// Analysis in progress, for that fingerprint.
    analysing: Option<(u64, mpsc::Receiver<Result<Features, String>>)>,
    /// Image whose analysis failed: tried again on a click, or once the
    /// model is downloaded again.
    failed: Option<u64>,
    /// Masks computed in the background (started on first use).
    worker: Option<Worker>,
    /// Pointer cell the preview was last asked for (fingerprint, cell).
    hover_cell: Option<(u64, i64, i64)>,
    /// Hovering session, renewed when the pointer leaves or a button is
    /// pressed: previews asked for before are not shown.
    session: u64,
    /// Object under the pointer.
    preview: Option<Shown>,
    /// Clicks made during the analysis, applied once it is done.
    queued: Vec<Pick>,
    /// Selections being computed.
    picking: usize,
    /// Button held: where it was pressed (screen, canvas), Shift and Alt
    /// at that time.
    press: Option<(Pos2, Pos2, bool, bool)>,
}

/// Preview of an object, computed by the worker: fill (at most
/// `PREVIEW_SIZE` px) and outline, in preview pixels.
struct Preview {
    image: ColorImage,
    edges: Vec<[(i64, i64); 2]>,
}

/// Preview on screen.
struct Shown {
    texture: TextureHandle,
    edges: Vec<[(i64, i64); 2]>,
}

/// A click or a rectangle: on the image `fingerprint`, with Shift and Alt.
struct Pick {
    /// The document clicked, its image then and its history step.
    doc: u64,
    fingerprint: u64,
    step: u64,
    prompt: Prompt,
    shift: bool,
    alt: bool,
}

/// Masks to compute: the latest preview asked for (the others are
/// outdated), and the selections, in order.
#[derive(Default)]
struct Queue {
    hover: Option<(u64, Arc<Features>, (f32, f32))>,
    picks: VecDeque<(Arc<Features>, Pick)>,
    closed: bool,
}

enum Done {
    /// Preview, for the image and hovering session `key`.
    Hover(u64, Option<Preview>),
    Pick(Pick, Result<Option<Mask>, String>),
}

/// Thread computing the masks, so the interface never waits for them.
struct Worker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    done: mpsc::Receiver<Done>,
}

impl Worker {
    fn new(ctx: egui::Context) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let shared = Arc::clone(&queue);
        let (send, done) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let (lock, wake) = &*shared;
                let mut q = lock.lock().unwrap_or_else(|e| e.into_inner());
                let job = loop {
                    if q.closed {
                        return;
                    }
                    if let Some(pick) = q.picks.pop_front() {
                        break Err(pick);
                    }
                    if let Some(hover) = q.hover.take() {
                        break Ok(hover);
                    }
                    q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                };
                drop(q);
                // A panic still answers: the interface never waits forever.
                let result = match job {
                    Ok((key, features, (x, y))) => {
                        let _job = crate::diag::job("Object Selection: hover preview");
                        let prompt = Prompt {
                            points: vec![(x, y, true)],
                            rect: None,
                        };
                        let size = preview_size(features.size());
                        let mask = crate::ai::catch_panic(|| {
                            objects::segment_preview(&features, &prompt, size)
                        });
                        Done::Hover(key, mask.ok().flatten().and_then(|m| preview(&m)))
                    }
                    Err((features, pick)) => {
                        let _job = crate::diag::job("Object Selection: pick");
                        let mask =
                            crate::ai::catch_panic(|| objects::segment(&features, &pick.prompt));
                        Done::Pick(pick, mask)
                    }
                };
                if send.send(result).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { queue, done }
    }

    fn push(&self, change: impl FnOnce(&mut Queue)) {
        let (lock, wake) = &*self.queue;
        change(&mut lock.lock().unwrap_or_else(|e| e.into_inner()));
        wake.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.push(|q| q.closed = true);
    }
}

/// Size of the preview of an image of `(w, h)`: at most `PREVIEW_SIZE` px.
fn preview_size((w, h): (u32, u32)) -> (u32, u32) {
    let scale = (PREVIEW_SIZE as f32 / w.max(h) as f32).min(1.0);
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

/// Fill and outline of `mask` (at the preview size); `None` if it is empty.
fn preview(mask: &GrayImage) -> Option<Preview> {
    let (pw, ph) = mask.dimensions();
    let area = PixelRect {
        x: 0,
        y: 0,
        w: pw,
        h: ph,
    };
    let edges = Selection::from_mask(mask, (0, 0))?.outline(area);
    let accent = ui_kit::color::ACCENT;
    let rgba: Vec<u8> = mask
        .pixels()
        .flat_map(|p| {
            let alpha = (p.0[0] as f32 * PREVIEW_ALPHA).round() as u8;
            [accent.r(), accent.g(), accent.b(), alpha]
        })
        .collect();
    Some(Preview {
        image: ColorImage::from_rgba_unmultiplied([pw as usize, ph as usize], &rgba),
        edges,
    })
}

/// What the analysis depends on: the canvas and the displayed layers,
/// with their masks, effects, settings and artboards (not what only the
/// Layers panel shows: a group folded). Pixel revisions are unique across
/// documents.
pub(super) fn fingerprint(doc: &Document) -> u64 {
    hash_of((
        doc.width(),
        doc.height(),
        doc.layers
            .iter()
            .map(|l| {
                (
                    (l.id, l.rev(), l.offset, l.visible, l.depth),
                    (
                        l.opacity.to_bits(),
                        l.fill.to_bits(),
                        l.blend as u8,
                        l.clipped,
                    ),
                    l.effects.any().then(|| format!("{:?}", l.effects)),
                    l.mask.as_ref().map(|m| m.key(l.offset)),
                    // Adjustment and fill layers: their settings.
                    match &l.kind {
                        LayerKind::Pixels | LayerKind::Group { .. } => None,
                        kind => Some(format!("{kind:?}")),
                    },
                    l.artboard,
                )
            })
            .collect::<Vec<_>>(),
    ))
}

impl ObjectSelect {
    /// Features of the image `fingerprint`, if it has been analysed.
    fn features_for(&self, fingerprint: u64) -> Option<Arc<Features>> {
        self.features
            .as_ref()
            .filter(|(f, _)| *f == fingerprint)
            .map(|(_, features)| Arc::clone(features))
    }

    /// Key of the previews to show, for the image `fingerprint`.
    fn hover_key(&self, fingerprint: u64) -> u64 {
        hash_of((fingerprint, self.session))
    }

    /// No preview, until the pointer moves over the image again.
    fn reset_hover(&mut self) {
        self.preview = None;
        self.hover_cell = None;
        self.session += 1;
    }

    /// Forgets the button held (tool changed in the middle of a click).
    pub(super) fn cancel_press(&mut self) {
        self.press = None;
    }

    fn worker(&mut self, ctx: &egui::Context) -> &Worker {
        self.worker.get_or_insert_with(|| Worker::new(ctx.clone()))
    }

    /// Something is being computed (analysis or selection).
    pub(super) fn busy(&self) -> bool {
        self.analysing.is_some() || self.picking > 0 || !self.queued.is_empty()
    }

    /// Forgets the analyses (the model was deleted, or downloaded again: an
    /// analysis that failed is tried again).
    pub(super) fn forget(&mut self) {
        self.features = None;
        self.failed = None;
        self.queued.clear();
        self.reset_hover();
    }

    /// The active document goes away: its gesture and its waiting clicks.
    pub(super) fn leave_document(&mut self) {
        self.cancel_press();
        self.queued.clear();
        self.reset_hover();
    }
}

impl AtelierApp {
    /// Object Selection tool: analysis of the image, preview of the object
    /// under the pointer, click (that object) or rectangle (the object
    /// inside it).
    pub(super) fn object_select_input(
        &mut self,
        view: View,
        pointer: Option<Pos2>,
        hovered: bool,
        start: bool,
        down: bool,
    ) -> CursorIcon {
        let Some(doc) = &self.doc else {
            return CursorIcon::Crosshair;
        };
        let canvas = doc.full_rect();
        let current = fingerprint(doc);
        self.analyse(current);

        if start && let Some(p) = pointer {
            let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
            self.object_select.press = Some((p, view.to_image(p), shift, alt));
        }
        if let Some((from, from_image, shift, alt)) = self.object_select.press {
            let to = pointer.unwrap_or(from);
            let dragged = from.distance(to) >= DRAG_DISTANCE;
            let rect = dragged
                .then(|| {
                    let r = Rect::from_two_pos(from_image, view.to_image(to));
                    intersect(to_pixel_rect(r), canvas)
                })
                .flatten();
            self.marquee_preview = rect;
            self.object_select.reset_hover();
            if !down {
                self.object_select.press = None;
                self.marquee_preview = None;
                let prompt = if dragged {
                    rect.map(|r| Prompt {
                        points: Vec::new(),
                        rect: Some([r.x as f32, r.y as f32, r.right() as f32, r.bottom() as f32]),
                    })
                } else {
                    let on_canvas = from_image.x >= 0.0
                        && from_image.y >= 0.0
                        && from_image.x < canvas.w as f32
                        && from_image.y < canvas.h as f32;
                    on_canvas.then(|| Prompt {
                        points: vec![(from_image.x, from_image.y, true)],
                        rect: None,
                    })
                };
                if let Some(prompt) = prompt {
                    self.pick_object(prompt, shift, alt, current);
                }
            }
        } else {
            self.hover_object(view, pointer.filter(|_| hovered), canvas, current);
        }

        if self.object_select.busy() {
            CursorIcon::Progress
        } else {
            CursorIcon::Crosshair
        }
    }

    /// Starts the analysis of the displayed image if it has changed and
    /// the model is there.
    fn analyse(&mut self, current: u64) {
        let os = &mut self.object_select;
        let done = os.features.as_ref().is_some_and(|(f, _)| *f == current);
        if done
            || os.analysing.is_some()
            || os.failed == Some(current)
            || self.downloads.is_downloading(Model::Objects)
            || !Model::Objects.ready()
        {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let flatten = doc.flatten_later();
        let (send, result) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _job = crate::diag::job("Object Selection: image analysis");
            let _ = send.send(crate::ai::catch_panic(|| objects::encode(&flatten())));
            ctx.request_repaint();
        });
        os.analysing = Some((current, result));
        os.reset_hover();
        self.status = t!("objsel-analysing");
    }

    /// Asks for the preview of the object under the pointer, when it goes
    /// to another cell.
    fn hover_object(&mut self, view: View, pointer: Option<Pos2>, canvas: PixelRect, current: u64) {
        let os = &mut self.object_select;
        let point = pointer.map(|p| view.to_image(p)).filter(|ip| {
            ip.x >= 0.0 && ip.y >= 0.0 && ip.x < canvas.w as f32 && ip.y < canvas.h as f32
        });
        let (Some(ip), Some(features)) = (point, os.features_for(current)) else {
            if os.hover_cell.is_some() || os.preview.is_some() {
                os.reset_hover();
            }
            return;
        };
        let side = (HOVER_CELL / view.zoom).max(1.0);
        let cell = (
            current,
            (ip.x / side).floor() as i64,
            (ip.y / side).floor() as i64,
        );
        if os.hover_cell == Some(cell) {
            return;
        }
        os.hover_cell = Some(cell);
        let key = os.hover_key(current);
        let ctx = self.ctx.clone();
        os.worker(&ctx)
            .push(|q| q.hover = Some((key, features, (ip.x, ip.y))));
    }

    /// Selects the object designated by `prompt` (Shift: add, Alt:
    /// subtract, both: intersect), once the image is analysed.
    fn pick_object(&mut self, prompt: Prompt, shift: bool, alt: bool, current: u64) {
        // Model not downloaded yet: asked first (the analysis starts once
        // it is there).
        if !self.request_model(AfterDownload::ObjectSelection) {
            return;
        }
        let Some((doc, step)) = self.doc.as_ref().map(|d| (d.uid(), d.history_step())) else {
            return;
        };
        let pick = Pick {
            doc,
            fingerprint: current,
            step,
            prompt,
            shift,
            alt,
        };
        let os = &mut self.object_select;
        match os.features_for(current) {
            Some(features) => {
                os.picking += 1;
                let ctx = self.ctx.clone();
                os.worker(&ctx)
                    .push(|q| q.picks.push_back((features, pick)));
            }
            None => {
                // After a failure, each click tries again (the error is
                // shown again if it fails again).
                os.failed = None;
                os.queued.push(pick);
                self.status = t!("objsel-analysing");
            }
        }
    }

    /// Results of the analysis and of the masks computed in the background
    /// (every frame, whatever the tool).
    pub(super) fn poll_object_select(&mut self) {
        let tool = self.tool == Tool::ObjectSelect;
        // Nothing asked for: the fingerprint (a hash of every layer) is not
        // paid every frame.
        if !tool && !self.object_select.busy() {
            return;
        }
        // Clicks waiting for an analysis that only the tool starts: the
        // tool was left, they are dropped rather than applied later.
        let os = &mut self.object_select;
        if !tool && os.analysing.is_none() && !os.queued.is_empty() {
            os.queued.clear();
            self.status = t!("sel4-objsel-dropped");
        }
        let current = self.doc.as_ref().map(fingerprint);
        let uid = self.doc.as_ref().map(Document::uid);
        let os = &mut self.object_select;
        if let Some((key, result)) = &os.analysing {
            let key = *key;
            match result.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {}
                Ok(Ok(features)) => {
                    os.analysing = None;
                    os.features = Some((key, Arc::new(features)));
                    if self.status == t!("objsel-analysing") {
                        self.status.clear();
                    }
                }
                outcome => {
                    os.analysing = None;
                    os.failed = Some(key);
                    os.queued.clear();
                    let error = match outcome {
                        Ok(Err(e)) => e,
                        _ => t!("ai-error-internal"),
                    };
                    let failed = tm!("objsel-failed", error = error);
                    self.status = failed.shown.clone();
                    crate::diag::error(failed);
                }
            }
        }
        // Clicks made during the analysis (forgotten if the image changed).
        if let Some(features) = current.and_then(|c| os.features_for(c))
            && !os.queued.is_empty()
        {
            let mut queued = std::mem::take(&mut os.queued);
            let before = queued.len();
            queued.retain(|pick| Some(pick.fingerprint) == current);
            if queued.len() < before {
                self.status = t!("sel4-objsel-dropped");
            }
            os.picking += queued.len();
            let ctx = self.ctx.clone();
            os.worker(&ctx).push(|q| {
                q.picks
                    .extend(queued.into_iter().map(|pick| (Arc::clone(&features), pick)));
            });
        }
        let Some(worker) = &os.worker else { return };
        let mut done = Vec::new();
        let ended = loop {
            match worker.done.try_recv() {
                Ok(result) => done.push(result),
                Err(mpsc::TryRecvError::Empty) => break false,
                Err(mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        for result in done {
            match result {
                Done::Hover(key, preview) => {
                    let os = &mut self.object_select;
                    // Only for the image and hovering session it was asked for.
                    if os.hover_cell.is_none() || current.map(|c| os.hover_key(c)) != Some(key) {
                        continue;
                    }
                    os.preview = preview.map(|p| {
                        let texture = match os.preview.take() {
                            Some(mut shown) => {
                                shown.texture.set(p.image, TextureOptions::LINEAR);
                                shown.texture
                            }
                            None => self.ctx.load_texture(
                                "object-preview",
                                p.image,
                                TextureOptions::LINEAR,
                            ),
                        };
                        Shown {
                            texture,
                            edges: p.edges,
                        }
                    });
                }
                Done::Pick(pick, mask) => {
                    self.object_select.picking -= 1;
                    let step = self.doc.as_ref().map(Document::history_step);
                    if uid == Some(pick.doc) && step != Some(pick.step) {
                        // Undone, redone or edited meanwhile: the click was
                        // about another state (never recorded over it).
                        self.status = t!("hist4-selection-history-changed");
                    } else if Some(pick.fingerprint) == current {
                        self.apply_object(mask, pick.shift, pick.alt);
                    } else if let Some(now) = current
                        && uid == Some(pick.doc)
                        && self.tool == Tool::ObjectSelect
                    {
                        // The image changed meanwhile (an undo): the click
                        // is made again on what shows now, rather than lost
                        // (never on another tab, nor once the tool is left:
                        // it would land later, unexpected).
                        crate::diag::warn(tm!("ui4-log-objsel-picked-again"));
                        self.pick_object(pick.prompt, pick.shift, pick.alt, now);
                    } else if uid == Some(pick.doc) {
                        self.status = t!("sel4-objsel-dropped");
                    }
                }
            }
        }
        if ended {
            // The thread is gone: a new one on the next request, nothing
            // left waiting.
            let os = &mut self.object_select;
            os.worker = None;
            os.picking = 0;
            os.reset_hover();
            self.status_failed(tm!("objsel-failed", error = t!("ai-error-internal")));
        }
    }

    /// Combines the object found with the selection.
    fn apply_object(&mut self, mask: Result<Option<Mask>, String>, shift: bool, alt: bool) {
        let Some(canvas) = self.doc.as_ref().map(Document::full_rect) else {
            return;
        };
        let found = match mask {
            Ok(mask) => mask.and_then(|m| Selection::from_mask(&m.coverage, m.offset)),
            Err(error) => {
                self.status_failed(tm!("objsel-failed", error = error));
                return;
            }
        };
        let Some(found) = found.and_then(|s| s.within(canvas)) else {
            self.status = t!("objsel-not-found");
            return;
        };
        let mode = self.selection_mode(shift, alt);
        self.apply_selection(Some(found), mode);
    }

    /// The object under the pointer: accent fill and outline.
    pub(super) fn draw_object_preview(&self, painter: &Painter, view: View) {
        let (Tool::ObjectSelect, Some(shown), Some(doc)) =
            (self.tool, &self.object_select.preview, &self.doc)
        else {
            return;
        };
        let canvas = view.rect_to_screen(to_rect(doc.full_rect()));
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
        painter.image(shown.texture.id(), canvas, uv, Color32::WHITE);
        // Outline: edges in preview pixels, stretched over the canvas.
        let [pw, ph] = shown.texture.size();
        let scale = vec2(canvas.width() / pw as f32, canvas.height() / ph as f32);
        let mut mesh = egui::epaint::Mesh::default();
        let visible = painter.clip_rect();
        for [a, b] in &shown.edges {
            let a = canvas.min + vec2(a.0 as f32, a.1 as f32) * scale;
            let b = canvas.min + vec2(b.0 as f32, b.1 as f32) * scale;
            let line = Rect::from_two_pos(a, b).expand(0.75);
            if line.intersects(visible) {
                mesh.add_colored_rect(line, ui_kit::color::ACCENT);
            }
        }
        painter.add(mesh);
    }

    /// Hint of the options bar: what to do, or what is going on.
    pub(super) fn object_select_hint(&self) -> String {
        if let Some(fraction) = self.downloads.progress(Model::Objects) {
            let percent = format!("{:.0}", fraction * 100.0);
            t!("objsel-downloading", percent = percent)
        } else if self.object_select.analysing.is_some() {
            t!("objsel-analysing")
        } else {
            t!("objsel-hint")
        }
    }
}

/// A photo-like test image (900 × 600): sky, ground, a red ball on the
/// left and a teal box on the right, shaded, with grain.
#[cfg(test)]
pub(super) fn demo_photo() -> image::RgbaImage {
    let grain = |x: u32, y: u32| {
        let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
        h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        ((h >> 24) as f32 / 255.0 - 0.5) * 12.0
    };
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| [0, 1, 2].map(|c| a[c] + (b[c] - a[c]) * t);
    image::RgbaImage::from_fn(900, 600, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let horizon = 340.0 + 10.0 * (fx / 150.0).sin();
        let mut c = if fy < horizon {
            mix([120.0, 165.0, 225.0], [225.0, 232.0, 240.0], fy / horizon)
        } else {
            let depth = (fy - horizon) / (600.0 - horizon);
            let stripe = 8.0 * ((fx + fy * 0.6) / 9.0).sin();
            mix([150.0, 160.0, 95.0], [80.0, 115.0, 50.0], depth).map(|v| v + stripe)
        };
        // Shadows on the ground.
        let shadow = |cx: f32, cy: f32, rx: f32, ry: f32| {
            let d = ((fx - cx) / rx).powi(2) + ((fy - cy) / ry).powi(2);
            if d < 1.0 { 0.55 + 0.45 * d } else { 1.0 }
        };
        let s = shadow(310.0, 478.0, 150.0, 22.0) * shadow(680.0, 470.0, 130.0, 18.0);
        c = c.map(|v| v * s);
        // Ball: shaded sphere lit from the top left, with a highlight.
        let (dx, dy) = ((fx - 290.0) / 120.0, (fy - 360.0) / 120.0);
        if dx * dx + dy * dy < 1.0 {
            let dz = (1.0 - dx * dx - dy * dy).sqrt();
            let light = (-0.5 * dx - 0.6 * dy + 0.62 * dz).max(0.0);
            let shine = light.powi(30) * 200.0;
            c = [205.0, 45.0, 40.0].map(|v| v * (0.3 + 0.7 * light) + shine);
        }
        // Box: rounded corners, front face shaded from left to right.
        let (bx, by) = ((fx - 660.0).abs() - 85.0, (fy - 320.0).abs() - 140.0);
        let corner = (bx.max(0.0).powi(2) + by.max(0.0).powi(2)).sqrt();
        if corner < 14.0 && bx < 14.0 && by < 14.0 {
            let t = (fx - 560.0) / 200.0;
            c = mix([40.0, 150.0, 160.0], [20.0, 85.0, 100.0], t);
        }
        let n = grain(x, y);
        let [r, g, b] = c.map(|v| (v + n).clamp(0.0, 255.0) as u8);
        image::Rgba([r, g, b, 255])
    })
}

/// Lets the Object Selection tool finish what it computes (analysis,
/// masks), for captures and tests.
#[cfg(test)]
pub(super) fn settle(harness: &mut egui_kittest::Harness<'_, AtelierApp>) {
    for _ in 0..500 {
        harness.run_steps(1);
        let app = harness.state();
        let os = &app.object_select;
        let hovered = os.hover_cell.is_none() || os.preview.is_some();
        if app.tool != Tool::ObjectSelect || (os.features.is_some() && !os.busy() && hovered) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    harness.run_steps(2);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn app_with(image: image::RgbaImage) -> Harness<'static, AtelierApp> {
        let mut harness = Harness::builder()
            .with_size([1400.0, 1000.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                app.doc = Some(Document::new(image, None));
                app
            });
        harness.run_steps(2);
        harness
    }

    #[test]
    fn fingerprint_follows_masks_adjustments_and_clipping() {
        let photo =
            || image::RgbaImage::from_fn(64, 48, |x, y| image::Rgba([x as u8, y as u8, 9, 255]));
        let mut doc = Document::new(photo(), None);
        let before = fingerprint(&doc);
        assert!(doc.add_mask(crate::document::NewMask::HideAll, None));
        assert_ne!(fingerprint(&doc), before, "mask");

        let mut doc = Document::new(photo(), None);
        doc.add_adjustment_layer(crate::pixel_ops::Adjustment::Invert, None);
        let before = fingerprint(&doc);
        let i = doc.active;
        doc.layers[i].kind = crate::document::LayerKind::Adjustment(
            crate::pixel_ops::Adjustment::PhotoFilter(Default::default()),
        );
        let adjusted = fingerprint(&doc);
        assert_ne!(adjusted, before, "adjustment");
        doc.layers[i].clipped = true;
        assert_ne!(fingerprint(&doc), adjusted, "clipping");
        doc.layers[i].fill = 0.5;
        assert_ne!(fingerprint(&doc), adjusted, "fill");
    }

    /// Press and release at `to` (from `from`, dragging), with `modifiers`.
    fn press(harness: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
        harness.event(egui::Event::ModifiersChanged(modifiers));
        harness.hover_at(from);
        harness.run_steps(1);
        harness.drag_at(from);
        harness.run_steps(1);
        for i in 1..=6 {
            harness.hover_at(from.lerp(to, i as f32 / 6.0));
            harness.run_steps(1);
        }
        harness.event(egui::Event::PointerButton {
            pos: to,
            button: PointerButton::Primary,
            pressed: false,
            modifiers,
        });
        harness.run_steps(1);
        harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        settle(harness);
    }

    fn screen(harness: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
        let view = harness.state().last_view.expect("canvas shown");
        view.to_screen(pos2(x, y))
    }

    fn assert_near(rect: PixelRect, [x, y, w, h]: [i64; 4]) {
        let got = [rect.x, rect.y, rect.w as i64, rect.h as i64];
        let close = got
            .iter()
            .zip([x, y, w, h])
            .all(|(a, b)| (a - b).abs() <= 12);
        assert!(
            close,
            "selection {got:?}, expected about {:?}",
            [x, y, w, h]
        );
    }

    /// A layer effect changes the image analysed (flattening draws it).
    #[test]
    fn fingerprint_includes_layer_effects() {
        let mut doc = Document::new(image::RgbaImage::new(64, 64), None);
        let before = fingerprint(&doc);
        doc.active_layer_mut().effects.stroke.enabled = true;
        let stroked = fingerprint(&doc);
        assert_ne!(before, stroked);
        doc.active_layer_mut().effects.stroke.width += 1.0;
        assert_ne!(fingerprint(&doc), stroked);
        // Settings of a disabled effect draw nothing.
        doc.active_layer_mut().effects.stroke.enabled = false;
        assert_eq!(fingerprint(&doc), before);
        doc.active_layer_mut().effects.shadow.blur += 1.0;
        assert_eq!(fingerprint(&doc), before);
    }

    /// W gives the group's last used tool, Shift+W the other one.
    #[test]
    fn w_switches_within_the_group() {
        let mut harness = app_with(image::RgbaImage::new(64, 64));
        let mut key = |modifiers: Modifiers| {
            harness.key_press_modifiers(modifiers, Key::W);
            harness.run_steps(1);
            harness.state().tool
        };
        assert!(key(Modifiers::NONE) == Tool::ObjectSelect);
        assert!(key(Modifiers::SHIFT) == Tool::MagicWand);
        assert!(key(Modifiers::NONE) == Tool::MagicWand);
        assert!(key(Modifiers::SHIFT) == Tool::ObjectSelect);
    }

    /// A click selects the ball; Shift and a rectangle around the box adds
    /// the box; Alt and a click on the ball leaves only the box. Needs the
    /// downloaded model.
    #[test]
    #[ignore = "needs the SAM model"]
    fn click_and_rectangle_select_objects() {
        let mut harness = app_with(demo_photo());
        harness.state_mut().tool = Tool::ObjectSelect;
        settle(&mut harness);
        assert!(harness.state().object_select.features.is_some());

        let ball = screen(&harness, 290.0, 360.0);
        press(&mut harness, ball, ball, Modifiers::NONE);
        let rect = harness.state().selection.as_ref().expect("ball").rect;
        assert_near(rect, [170, 240, 240, 240]);

        let (from, to) = (
            screen(&harness, 545.0, 165.0),
            screen(&harness, 775.0, 475.0),
        );
        press(&mut harness, from, to, Modifiers::SHIFT);
        let rect = harness.state().selection.as_ref().expect("both").rect;
        assert_near(rect, [170, 166, 590, 314]);

        press(&mut harness, ball, ball, Modifiers::ALT);
        let selection = harness.state().selection.clone().expect("box");
        // Soft edges leave a faint ring: the selected pixels count.
        let canvas = PixelRect {
            x: 0,
            y: 0,
            w: 900,
            h: 600,
        };
        let coverage = selection.to_mask(canvas);
        let selected = GrayImage::from_fn(900, 600, |x, y| {
            image::Luma([if coverage.get_pixel(x, y).0[0] >= 128 {
                255
            } else {
                0
            }])
        });
        let mask = Selection::from_mask(&selected, (0, 0));
        assert_near(mask.expect("box").rect, [561, 166, 198, 308]);
    }

    /// Folding a group changes nothing in the image: no new analysis. An
    /// artboard's background does.
    #[test]
    fn fingerprint_ignores_group_folding_and_follows_artboards() {
        let mut doc = Document::new(image::RgbaImage::new(64, 64), None);
        doc.add_group();
        let before = fingerprint(&doc);
        for l in &mut doc.layers {
            if let crate::document::LayerKind::Group { expanded } = &mut l.kind {
                *expanded = !*expanded;
            }
        }
        assert_eq!(fingerprint(&doc), before, "folded");
        let i = doc.active;
        doc.layers[i].artboard = Some(crate::document::Artboard {
            size: (32, 32),
            background: crate::document::ArtboardBackground::White,
        });
        let board = fingerprint(&doc);
        assert_ne!(board, before, "artboard");
        doc.layers[i].artboard.as_mut().unwrap().background =
            crate::document::ArtboardBackground::Black;
        assert_ne!(fingerprint(&doc), board, "its background");
    }

    /// A click waiting for an analysis that only the tool starts is
    /// dropped when the tool is left, never applied later.
    #[test]
    fn a_waiting_click_is_dropped_when_the_tool_is_left() {
        let mut harness = app_with(image::RgbaImage::new(64, 64));
        let app = harness.state_mut();
        let doc = app.doc.as_ref().unwrap();
        let pick = Pick {
            doc: doc.uid(),
            fingerprint: fingerprint(doc),
            step: doc.history_step(),
            prompt: Prompt {
                points: vec![(10.0, 10.0, true)],
                rect: None,
            },
            shift: false,
            alt: false,
        };
        app.object_select.queued.push(pick);
        app.tool = Tool::Brush;
        harness.run_steps(2);
        let app = harness.state();
        assert!(app.object_select.queued.is_empty());
        assert!(!app.object_select.busy());
        assert_eq!(app.status, t!("sel4-objsel-dropped"));
        assert!(app.selection.is_none());
    }
}
