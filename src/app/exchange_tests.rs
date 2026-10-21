//! History and Channels panels driven through the app, and the gradient of
//! a fill layer edited in the Gradient Editor.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use image::{Luma, Rgba, RgbaImage};

use super::select_menu::OverlayStyle;
use super::*;
use crate::compose::gradient::GradientFill;
use crate::paint::gradient::Preset;

/// A 60 × 40 image, red on the left, blue on the right.
fn app() -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = RgbaImage::from_fn(60, 40, |x, _| {
                Rgba(if x < 30 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                })
            });
            app.doc = Some(Document::new(image, None));
            app
        });
    h.state_mut().fit_pending = None;
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

#[test]
fn history_panel_goes_back_and_forth() {
    let mut h = app();
    h.state_mut().run(Action::History);
    h.state_mut().run(Action::NewLayer);
    h.state_mut().run(Action::Duplicate);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 3);
    // A click on the first step: the state after it, the second greyed.
    h.get_by_label("New Layer").click();
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 2);
    assert_eq!(doc(&h).history(), (vec!["New Layer", "Duplicate Layer"], 1));
    // The opening state, then forward again.
    h.get_by_label("Untitled").click();
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 1);
    h.get_by_label("Duplicate Layer").click();
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 3);
    // Back, then a new action: the undone steps go.
    h.get_by_label("New Layer").click();
    h.run_steps(1);
    h.state_mut().run(Action::Delete);
    h.run_steps(2);
    let (names, done) = doc(&h).history();
    assert_eq!((names, done), (vec!["New Layer", "Delete Layer"], 2));
    // Window › History closes it.
    h.state_mut().run(Action::History);
    h.run_steps(2);
    assert!(h.query_by_label("Delete Layer").is_none());
}

#[test]
fn channels_panel_views_and_loads_channels() {
    let mut h = app();
    h.state_mut().run(Action::Channels);
    h.run_steps(2);
    // A color channel alone, in gray; Ctrl+2 back to the image.
    h.get_by_label("Red").click();
    h.run_steps(2);
    assert_eq!(h.state().channels.color_shown(doc(&h)), Some(0));
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run_steps(2);
    assert_eq!(h.state().channels.color_shown(doc(&h)), None);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num5);
    h.run_steps(2);
    assert_eq!(h.state().channels.color_shown(doc(&h)), Some(2));
    // A saved selection: shown alone in gray (Ctrl+6), loaded with Ctrl +
    // click.
    let app = h.state_mut();
    let d = app.doc.as_mut().unwrap();
    let i = d.new_channel();
    Arc::make_mut(&mut d.channels[i].mask).put_pixel(5, 5, Luma([255]));
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num6);
    h.run_steps(2);
    let overlay = h.state_mut().channel_overlay().map(|o| o.style);
    assert_eq!(overlay, Some(OverlayStyle::Gray));
    h.get_by_label("Alpha 1")
        .click_modifiers(Modifiers::COMMAND);
    h.run_steps(2);
    let sel = h.state().selection.clone().expect("loaded");
    assert_eq!((sel.coverage(5, 5), sel.coverage(6, 5)), (255, 0));
    // Its eye over the image: a red overlay.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Num2);
    h.run_steps(1);
    assert!(h.state_mut().channel_overlay().is_none());
    // New channel, then an undo takes it away.
    h.state_mut().go_to_history(0);
    h.run_steps(2);
    assert!(doc(&h).channels.is_empty());
}

#[test]
fn a_fill_layer_gradient_is_edited_in_the_gradient_editor() {
    let mut h = app();
    let fill = GradientFill::default();
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .add_fill_layer(Fill::Gradient(fill), None);
    h.run_steps(3);
    // A click on its gradient in the Properties panel opens the editor.
    h.get_by_label("Click to edit the gradient").click();
    h.run_steps(2);
    let editor = h.state_mut().paint.editor.as_mut().expect("the editor");
    editor.gradient = Preset::Copper.gradient();
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(h.state().paint.editor.is_none());
    let LayerKind::Fill(Fill::Gradient(g)) = &doc(&h).active_layer().kind else {
        panic!("a gradient fill");
    };
    assert_eq!(g.gradient.colors.len(), 4);
    // One step, named after the layer's kind.
    let (names, _) = doc(&h).history();
    assert_eq!(names.last(), Some(&"Modify Fill Layer"));
}

/// The History panel is resized by its bottom right corner, which follows
/// the pointer; the list fills it.
#[test]
fn history_panel_resizes() {
    let mut h = app();
    h.state_mut().run(Action::History);
    h.run_steps(3);
    let area = |h: &Harness<'static, AtelierApp>| {
        h.ctx
            .memory(|m| m.area_rect(egui::Id::new("history")))
            .expect("shown")
    };
    let before = area(&h);
    let grip = before.right_bottom() - vec2(6.0, 6.0);
    h.hover_at(grip);
    h.run_steps(1);
    h.drag_at(grip);
    h.run_steps(1);
    for k in 1..=4 {
        h.hover_at(grip + vec2(15.0, 50.0) * k as f32);
        h.run_steps(1);
    }
    h.drop_at(grip + vec2(60.0, 200.0));
    h.run_steps(3);
    let after = area(&h);
    assert!(
        (after.min - before.min).length() < 1.0,
        "{before:?} → {after:?}"
    );
    assert!(
        (after.width() - before.width() - 60.0).abs() < 2.0,
        "{before:?} → {after:?}"
    );
    assert!(
        (after.height() - before.height() - 200.0).abs() < 2.0,
        "{before:?} → {after:?}"
    );
}
