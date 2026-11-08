//! Memory and background jobs: what a closed document, a superseded job or
//! a deleted layer leaves behind, and jobs kept across tab switches.

use super::*;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

fn harness(doc: Document) -> Harness<'static, AtelierApp> {
    i18n::set_lang(Lang::En);
    let mut h = Harness::builder()
        .with_size([1200.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(doc);
            app
        });
    h.run_steps(3);
    h
}

/// Runs frames until `done` (a minute at most).
fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in crate::testing::patience() {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("background work not done");
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(c))
}

/// A red disc on blue: the non-AI subject search finds it.
fn disc() -> RgbaImage {
    RgbaImage::from_fn(400, 300, |x, y| {
        let (dx, dy) = (x as f32 - 200.0, y as f32 - 150.0);
        if dx * dx + dy * dy < 75.0f32.powi(2) {
            Rgba([200, 30, 30, 255])
        } else {
            Rgba([30, 60, 200, 255])
        }
    })
}

/// The composite Sample All Layers keeps between strokes leaves with its
/// document.
#[test]
fn closing_forgets_the_shared_composite() {
    let mut h = harness(Document::new(solid(300, 200, [9, 9, 9, 255]), None));
    let app = h.state_mut();
    let doc = app.doc.as_ref().unwrap();
    let uid = doc.uid();
    doc.composite_shared(doc.full_rect());
    app.close_document();
    assert!(!crate::document::shared_composite_kept(uid));
}

/// The areas painted between two composites stay one per layer, however
/// many brush frames come.
#[test]
fn painted_areas_stay_one_per_layer() {
    let doc = Document::new(solid(300, 200, [9, 9, 9, 255]), None);
    let (uid, id) = (doc.uid(), doc.active_layer().id);
    doc.composite_shared(doc.full_rect());
    for i in 0..500 {
        let r = crate::document::PixelRect {
            x: i % 300,
            y: 5,
            w: 2,
            h: 2,
        };
        crate::document::note_painted(uid, &[(id, r)]);
    }
    let painted = crate::document::shared_painted(uid);
    // (Empty if another test's composite replaced it meanwhile.)
    assert!(painted.len() <= 1, "{} areas", painted.len());
    if let Some(&(layer, r)) = painted.first() {
        assert_eq!(layer, id);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 5, 301, 2));
    }
}

/// A subject search started on a document, then a switch to another tab
/// and back: the subject is selected in its own document, never in the
/// other one.
#[test]
fn a_subject_search_lands_after_a_tab_round_trip() {
    let mut h = harness(Document::new(solid(400, 300, [90, 90, 90, 255]), None));
    let app = h.state_mut();
    app.add_tab(Document::new(disc(), None));
    h.run_steps(2);
    let app = h.state_mut();
    let searched = app.doc.as_ref().unwrap().uid();
    app.start_subject(false, false);
    app.switch_tab(0);
    // Done while the other document shows: it waits.
    settle(&mut h, AtelierApp::subject_found);
    for _ in 0..5 {
        h.run_steps(1);
    }
    let app = h.state_mut();
    assert!(app.selection.is_none(), "not on the other document");
    assert!(app.subject_found(), "kept for its document");
    app.switch_tab(1);
    assert_eq!(app.doc.as_ref().unwrap().uid(), searched);
    settle(&mut h, |app| app.selection.is_some());
    let app = h.state();
    let canvas = app.doc.as_ref().unwrap().full_rect();
    let b = app.selection.as_ref().unwrap().bounds(canvas).expect("selected");
    assert!(b.w > 100 && b.w < 200, "the disc: {b:?}");
}

/// Closed meanwhile: its search is dropped, the next tab gets nothing.
#[test]
fn a_subject_search_of_a_closed_document_is_dropped() {
    let mut h = harness(Document::new(solid(400, 300, [90, 90, 90, 255]), None));
    let app = h.state_mut();
    app.add_tab(Document::new(disc(), None));
    h.run_steps(2);
    let app = h.state_mut();
    app.start_subject(false, false);
    app.close_document();
    settle(&mut h, |app| app.subject_job.is_none());
    for _ in 0..5 {
        h.run_steps(1);
    }
    assert!(h.state().selection.is_none());
}

/// Zooming through several levels of reduction while their reductions are
/// made: the superseded ones stop instead of reducing every layer for
/// nothing; the last one alone does the work.
#[test]
fn superseded_level_reductions_stop() {
    // A size of its own: the counts are this test's alone.
    let size = (3001, 2003);
    let mut doc = Document::new(solid(size.0, size.1, [255, 255, 255, 255]), None);
    let layers = 5;
    for k in 0..layers - 1 {
        doc.add_layer(format!("l{k}"), solid(size.0, size.1, [0, 90, 0, 40]), (0, 0));
    }
    let mut h = harness(doc);
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(2);
    // The reductions wait until all have started.
    render::reduce_watch::HELD.lock().unwrap().push(size);
    let spin = [0.4, 0.2, 0.1, 0.05, 0.03, 0.4, 0.2, 0.1, 0.05, 0.03, 0.2, 0.1];
    for zoom in spin {
        h.state_mut().zoom = zoom;
        h.run_steps(1);
    }
    render::reduce_watch::HELD.lock().unwrap().retain(|s| *s != size);
    settle(&mut h, |app| !app.level_pending());
    let reduced = render::reduce_watch::reduced(size);
    assert!(
        reduced <= 2 * layers,
        "{reduced} layers reduced for the last level ({layers} layers)"
    );
}

/// Applying a filter whose preview is shown: the preview becomes the
/// layer's pixels, moved rather than copied.
#[test]
fn a_filter_applied_moves_its_preview() {
    let image = RgbaImage::from_fn(64, 64, |x, _| match x < 32 {
        true => Rgba([0, 0, 0, 255]),
        false => Rgba([255, 255, 255, 255]),
    });
    let mut h = harness(Document::new(image.clone(), None));
    h.state_mut()
        .run(Action::Filter(crate::filters::FilterKind::GaussianBlur));
    settle(&mut h, |app| {
        matches!(
            app.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(_))
        )
    });
    let shown = h.state().preview_shown.as_ref().unwrap().image.as_raw().as_ptr();
    h.key_press(egui::Key::Enter);
    settle(&mut h, |app| !app.filters.busy());
    let layer = h.state().doc.as_ref().unwrap().active_layer();
    assert_ne!(*layer.image(), image, "filtered");
    assert_eq!(layer.image().as_raw().as_ptr(), shown, "the preview's own pixels");
}

/// A step costs nothing when recorded (it shares every buffer): once its
/// change is in, the history goes back within its budget at the next
/// frames, not at the next step.
#[test]
fn the_history_is_trimmed_once_a_step_is_in() {
    use crate::document::HISTORY_BUDGET;
    // 64 MiB per step (untouched zero pages: not really allocated).
    let mut h = harness(Document::new(RgbaImage::new(4096, 4096), None));
    let step = 4096 * 4096 * 4;
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    for _ in 0..HISTORY_BUDGET / step + 1 {
        doc.checkpoint("step".into());
        doc.active_layer_mut()
            .replace_pixels(RgbaImage::new(4096, 4096));
    }
    assert!(doc.history_bytes() > HISTORY_BUDGET, "one step over");
    app.history.trimmed_at = None;
    h.run_steps(1);
    let doc = h.state().doc.as_ref().unwrap();
    assert!(doc.history_bytes() <= HISTORY_BUDGET);
    assert!(doc.can_undo());
}

/// Layer thumbnails of deleted layers leave.
#[test]
fn thumbnails_of_deleted_layers_leave() {
    let mut doc = Document::new(solid(200, 100, [9, 9, 9, 255]), None);
    for k in 0..6 {
        doc.add_layer(format!("l{k}"), solid(50, 50, [200, 0, 0, 255]), (k, k));
    }
    let mut h = harness(doc);
    h.run_steps(2);
    let before = h.state().thumbs.len();
    assert!(before >= 6, "{before} thumbnails shown");
    let doc = h.state_mut().doc.as_mut().unwrap();
    let all: Vec<usize> = (1..doc.layers.len()).collect();
    assert!(doc.delete_layers(&all));
    h.run_steps(2);
    let app = h.state();
    let layers = app.doc.as_ref().unwrap().layers.len();
    assert!(app.thumbs.len() <= 2 * layers, "{} thumbnails", app.thumbs.len());
}
