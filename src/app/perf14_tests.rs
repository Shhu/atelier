//! The UI thread kept light: layer fingerprints, the Levels histogram,
//! adjustment previews reduced aside, the Layers list laid out where it
//! shows, the Quick Mask overlay drawn again where painted, a large layer's
//! stroke copied aside.

use super::*;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

type H = Harness<'static, AtelierApp>;

/// A document of `w` × `h` noise, fitted in a 1400 × 900 window, with `tool`.
fn app(w: u32, h: u32, tool: Tool) -> H {
    let image = RgbaImage::from_fn(w, h, |x, y| {
        Rgba([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 239) as u8, 255])
    });
    let mut harness = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(image.clone(), None));
            app
        });
    harness.state_mut().set_tool(tool);
    harness.run_steps(3);
    harness
}

/// Steps until `done` holds (at most 30 s).
fn step_until(h: &mut H, what: &str, mut done: impl FnMut(&mut H) -> bool) {
    let start = std::time::Instant::now();
    while !done(h) {
        assert!(start.elapsed().as_secs() < 30, "{what}: never");
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn screen(h: &H, x: f32, y: f32) -> Pos2 {
    h.state().last_view.unwrap().to_screen(pos2(x, y))
}

fn release(h: &mut H, at: Pos2) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
}

/// A layer's fingerprint follows its styles and its kind's settings (no
/// longer formatted at every frame: kept until they change).
#[test]
fn fingerprints_follow_styles_and_settings() {
    use crate::pixel_ops::{Adjustment, Levels};
    let mut doc = Document::new(RgbaImage::from_pixel(20, 10, Rgba([9, 9, 9, 255])), None);
    let opts = crate::compose::Options::default();
    let sigs = |doc: &Document| crate::compose::signatures(&doc.layers, &opts);
    let first = sigs(&doc);
    assert_eq!(first, sigs(&doc), "the same layers");
    doc.active_layer_mut().effects.shadow.enabled = true;
    let shadow = sigs(&doc);
    assert_ne!(first[0].state, shadow[0].state, "a style turned on");
    doc.active_layer_mut().effects.shadow.opacity = 0.25;
    assert_ne!(shadow[0].state, sigs(&doc)[0].state, "its opacity");
    doc.active_layer_mut().effects = crate::compose::styles::Effects::default();
    assert_eq!(first, sigs(&doc), "back as it was");

    let mut levels = Levels::default();
    doc.active_layer_mut().kind = LayerKind::Adjustment(Adjustment::Levels(levels));
    let before = sigs(&doc);
    levels.rgb.gamma = 1.5;
    doc.active_layer_mut().kind = LayerKind::Adjustment(Adjustment::Levels(levels));
    assert_ne!(before[0].state, sigs(&doc)[0].state, "an adjustment's setting");
}

/// Opening Levels counts the layer's histogram on a worker thread: the
/// first frame shows none, then the layer's own.
#[test]
fn levels_counts_its_histogram_aside() {
    let mut h = app(1600, 1200, Tool::Hand);
    h.state_mut().run(Action::Adjust(AdjustKind::Levels));
    h.step();
    assert!(h.state().adjust.is_some(), "Levels open");
    assert!(h.state().histogram.is_none(), "not counted on the UI thread");
    step_until(&mut h, "histogram", |h| h.state().histogram.is_some());
    let app = h.state();
    let layer = app.doc.as_ref().unwrap().active_layer();
    let expected = crate::pixel_ops::histogram_within(layer.image(), None);
    assert_eq!(app.histogram.as_ref().unwrap().1, expected);
}

/// An adjustment's preview comes reduced for the canvas from the worker
/// that computed it; applied, its pixels are the preview's.
#[test]
fn adjustment_preview_comes_reduced() {
    use crate::pixel_ops::{Adjustment, Levels};
    let mut h = app(3000, 2000, Tool::Hand);
    let level = render::render_level(h.state().zoom);
    assert!(level > 0, "a reduced view");
    h.state_mut().run(Action::Adjust(AdjustKind::Levels));
    h.run_steps(2);
    let mut levels = Levels::default();
    levels.rgb.gamma = 1.8;
    let adjustment = Adjustment::Levels(levels);
    h.state_mut().adjust.as_mut().unwrap().adjustment = adjustment.clone();
    step_until(&mut h, "preview", |h| {
        h.state()
            .preview_shown
            .as_ref()
            .is_some_and(|p| p.preview == LayerPreview::Adjust(adjustment.clone()))
    });
    let shown = h.state().preview_shown.as_ref().unwrap();
    let (at, reduced) = shown.reduced.as_ref().expect("reduced aside");
    assert_eq!(*at, level);
    let expected = crate::compose::levels::reduce_rgba(&shown.image, 1 << level);
    assert!(**reduced == expected, "the preview reduced");
    let image = shown.image.clone();
    h.state_mut().apply_adjustment(adjustment);
    h.state_mut().adjust = None;
    h.run_steps(2);
    let doc = h.state().doc.as_ref().unwrap();
    assert!(doc.active_layer().image() == &*image, "the preview applied");
}

/// The Layers list lays out the rows in view only: a long list's last rows
/// are not there until scrolled to.
#[test]
fn layers_list_lays_out_rows_in_view() {
    let mut h = app(200, 100, Tool::Hand);
    let doc = h.state_mut().doc.as_mut().unwrap();
    for i in 0..200 {
        doc.add_layer(format!("L{i}"), RgbaImage::new(4, 4), (0, 0));
    }
    let (top, bottom) = (doc.layers.last().unwrap().id, doc.layers[0].id);
    h.run_steps(3);
    let eye = |h: &H, id: u64| h.ctx.read_response(egui::Id::new(("side_eye", id)));
    let at = eye(&h, top).expect("the top row is laid out").rect.center();
    assert!(eye(&h, bottom).is_none(), "the bottom one is not");
    h.hover_at(at);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: Modifiers::NONE,
    });
    step_until(&mut h, "scrolled to the bottom", |h| eye(h, bottom).is_some());
    h.run_steps(3);
    let view = h.state().layer_list_view;
    assert!(view.0 > 0.0, "scrolled ({view:?})");
    assert!(eye(&h, top).is_none(), "the top row is out of view ({view:?})");
}

/// Painting in Quick Mask Mode draws its overlay again only where the
/// brush went, and that gives the same pixels as drawing it whole.
#[test]
fn quick_mask_overlay_redrawn_where_painted() {
    let mut h = app(1200, 800, Tool::Brush);
    h.state_mut().brush_size = 40.0;
    h.state_mut()
        .run(Action::Select(select_menu::Command::QuickMask));
    h.run_steps(3);
    assert!(h.state().doc.as_ref().unwrap().quick_mask().is_some());
    let start = screen(&h, 300.0, 400.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    select_menu::OVERLAYS_DRAWN.set((0, 0));
    let mut end = start;
    for k in 1..=8 {
        end = screen(&h, 300.0 + 60.0 * k as f32, 400.0);
        h.hover_at(end);
        h.run_steps(1);
    }
    let (whole, parts) = select_menu::OVERLAYS_DRAWN.get();
    assert_eq!(whole, 0, "never drawn whole while painting");
    assert!(parts >= 4, "drawn again in part ({parts})");
    release(&mut h, end);
    h.run_steps(2);
    let painted = h.state().sel.overlay_kept.as_ref().unwrap().image.clone();
    // Drawn whole again for the same view: the same pixels.
    let app = h.state_mut();
    (app.sel.overlay, app.sel.overlay_kept) = (None, None);
    h.run_steps(1);
    let whole = &h.state().sel.overlay_kept.as_ref().unwrap().image;
    assert!(*whole == painted, "partial redraws match a whole one");
}

/// A stroke on a large layer doesn't copy its pixels on the UI thread: a
/// worker does, the dabs waiting for it; the step keeps the pixels from
/// before.
#[test]
fn large_layer_stroke_copies_aside() {
    use std::sync::atomic::Ordering;
    let mut h = app(2000, 1200, Tool::Brush);
    h.state_mut().brush_size = 30.0;
    let before = h.state().doc.as_ref().unwrap().active_layer().shared_image();
    let start = screen(&h, 500.0, 600.0);
    h.hover_at(start);
    h.run_steps(1);
    paint_tools::HOLD_STROKE_COPY.store(true, Ordering::SeqCst);
    h.drag_at(start);
    h.run_steps(1);
    let mut end = start;
    for k in 1..=3 {
        end = screen(&h, 500.0 + 50.0 * k as f32, 600.0);
        h.hover_at(end);
        h.run_steps(1);
    }
    let layer = h.state().doc.as_ref().unwrap().active_layer().shared_image();
    let waited = Arc::ptr_eq(&layer, &before);
    paint_tools::HOLD_STROKE_COPY.store(false, Ordering::SeqCst);
    assert!(waited, "nothing copied nor painted on the UI thread meanwhile");
    drop(layer);
    step_until(&mut h, "copied", |h| {
        let doc = h.state().doc.as_ref().unwrap();
        !Arc::ptr_eq(&doc.active_layer().shared_image(), &before)
    });
    for k in 4..=6 {
        end = screen(&h, 500.0 + 50.0 * k as f32, 600.0);
        h.hover_at(end);
        h.run_steps(1);
    }
    release(&mut h, end);
    h.run_steps(2);
    let doc = h.state().doc.as_ref().unwrap();
    let painted = |x: i64| doc.active_layer().pixel_at(x, 600).unwrap() != before.get_pixel(x as u32, 600).0;
    assert!(painted(520) && painted(700) && painted(790), "the samples held were painted");
    let doc = h.state_mut().doc.as_mut().unwrap();
    assert!(doc.undo());
    assert!(doc.active_layer().image() == &*before, "the step kept the pixels before it");
}
