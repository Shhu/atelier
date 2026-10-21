//! Document edits driven through the app: undo grouping, steps recorded
//! (or not), eyedropper, Flatten Image.

use super::*;
use egui_kittest::Harness;

/// Transparent 8 × 8 document.
fn app() -> Harness<'static, AtelierApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 800.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(image::RgbaImage::new(8, 8), None));
            app
        });
    harness.run_steps(2);
    harness
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn filled(h: &Harness<'static, AtelierApp>) -> bool {
    doc(h).active_layer().pixel_at(0, 0) == Some([255, 0, 0, 255])
}

/// Opacity, fill, opacity again: three undo steps, whatever came before.
fn opacity_fill_opacity(app: &mut AtelierApp) {
    app.tool = Tool::Canvas;
    app.primary = Color32::RED;
    app.opacity_digit(5, 0.0);
    app.run(Action::FillPrimary);
    app.opacity_digit(3, 10.0);
    app.undo();
}

#[test]
fn property_series_does_not_swallow_the_step_after_an_undo() {
    let mut h = app();
    let app = h.state_mut();
    app.tool = Tool::Canvas;
    app.opacity_digit(5, 0.0);
    app.undo();
    opacity_fill_opacity(app);
    assert!(filled(&h), "one Ctrl+Z also undid the fill");
}

#[test]
fn property_series_does_not_swallow_steps_once_the_history_is_full() {
    let mut h = app();
    let app = h.state_mut();
    for _ in 0..40 {
        app.doc.as_mut().unwrap().checkpoint("step".into());
    }
    opacity_fill_opacity(app);
    assert!(filled(&h), "one Ctrl+Z also undid the fill");
}

#[test]
fn locking_is_an_undo_step() {
    let mut h = app();
    let app = h.state_mut();
    app.primary = Color32::RED;
    app.run(Action::FillPrimary);
    app.run(Action::ToggleLock);
    app.undo();
    assert!(!doc(&h).active_layer().locked);
    assert!(filled(&h), "the undo only unlocked the layer");
}

#[test]
fn brush_click_outside_the_selection_records_no_step() {
    let mut h = app();
    let app = h.state_mut();
    app.tool = Tool::Brush;
    app.brush_size = 2.0;
    app.selection = Some(
        PixelRect {
            x: 0,
            y: 0,
            w: 2,
            h: 2,
        }
        .into(),
    );
    let view = View {
        origin: Pos2::ZERO,
        zoom: 1.0,
    };
    let before = app.doc.as_ref().unwrap().history_step();
    let at = Some(pos2(6.0, 6.0));
    app.brush_input(view, at, true, true, false, false);
    app.brush_input(view, at, false, false, true, false);
    let doc = app.doc.as_ref().unwrap();
    assert!(!doc.can_undo(), "empty undo step");
    assert_ne!(doc.history_step(), before);
    // Inside the selection, the stroke is a step.
    let at = Some(pos2(1.0, 1.0));
    app.brush_input(view, at, true, true, false, false);
    app.brush_input(view, at, false, false, true, false);
    assert!(app.doc.as_ref().unwrap().can_undo());
}

#[test]
fn eyedropper_picks_an_opaque_color() {
    let mut h = app();
    let app = h.state_mut();
    let layer = app.doc.as_mut().unwrap().active_layer_mut();
    layer
        .pixels_mut()
        .put_pixel(2, 2, image::Rgba([200, 100, 50, 10]));
    app.pick_color(pos2(2.5, 2.5), false);
    assert_eq!(app.primary, Color32::from_rgb(200, 100, 50));
}

#[test]
fn flatten_asks_before_discarding_hidden_layers() {
    let mut h = app();
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    doc.add_empty_layer();
    doc.active_layer_mut().visible = false;
    app.run(Action::FlattenImage);
    assert!(app.flatten_confirm);
    assert_eq!(doc_layers(app), 2, "nothing flattened before the answer");
    app.flatten_confirm = false;
    app.merge_layers(Action::FlattenImage);
    assert_eq!(doc_layers(app), 1);
    // Without hidden layers, no question.
    app.doc.as_mut().unwrap().add_empty_layer();
    app.run(Action::FlattenImage);
    assert!(!app.flatten_confirm);
    assert_eq!(doc_layers(app), 1);
}

fn doc_layers(app: &AtelierApp) -> usize {
    app.doc.as_ref().unwrap().layers.len()
}
