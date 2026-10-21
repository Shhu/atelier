//! Layers driven through the app: masks (targeting, painting, Shift / Alt /
//! Ctrl + click), groups and clipping shortcuts, blend mode shortcuts,
//! adjustment layers, the Layer Style window.

use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

use super::right_panel::RowPart;
use super::*;

const WHITE: [u8; 4] = [255; 4];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// White 200 × 100 background, a blue layer over it (active).
fn app() -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let mut doc = Document::new(RgbaImage::from_pixel(200, 100, Rgba(WHITE)), None);
            doc.add_layer(
                "blue".into(),
                RgbaImage::from_pixel(200, 100, Rgba(BLUE)),
                (0, 0),
            );
            app.doc = Some(doc);
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

/// Canvas point (x, y) on screen.
fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state().last_view.unwrap().to_screen(pos2(x, y))
}

#[test]
fn mask_thumbnail_clicks() {
    let mut h = app();
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    assert!(doc.add_mask(NewMask::HideAll, None));
    // A click on the layer's thumbnail targets its pixels, on the mask the mask.
    app.click_row(1, RowPart::Thumbnail, Modifiers::NONE);
    assert!(!doc_of(app).mask_targeted());
    app.click_row(1, RowPart::Mask, Modifiers::NONE);
    assert!(doc_of(app).mask_targeted());
    // Shift: off, then on again.
    app.click_row(1, RowPart::Mask, Modifiers::SHIFT);
    assert!(!doc_of(app).active_layer().mask.as_ref().unwrap().enabled);
    assert_eq!(doc_of(app).flatten().get_pixel(5, 5).0, BLUE);
    app.click_row(1, RowPart::Mask, Modifiers::SHIFT);
    assert_eq!(doc_of(app).flatten().get_pixel(5, 5).0, WHITE);
    // Alt: the mask alone on the canvas.
    app.click_row(1, RowPart::Mask, Modifiers::ALT);
    assert!(app.mask_view);
    // Ctrl: what it shows as the selection (nothing: no selection).
    app.click_row(1, RowPart::Mask, Modifiers::COMMAND);
    assert!(app.selection.is_none());
    // The chain unlinks it.
    app.click_row(1, RowPart::Chain, Modifiers::NONE);
    assert!(!doc_of(app).active_layer().mask.as_ref().unwrap().linked());
    h.run_steps(2);
}

fn doc_of(app: &AtelierApp) -> &Document {
    app.doc.as_ref().unwrap()
}

#[test]
fn painting_a_targeted_mask() {
    let mut h = app();
    let app = h.state_mut();
    app.doc.as_mut().unwrap().add_mask(NewMask::HideAll, None);
    app.primary = Color32::WHITE;
    (app.brush_size, app.hardness) = (10.0, 1.0);
    app.set_tool(Tool::Brush);
    let rev = doc_of(app).active_layer().rev();
    h.run_steps(1);
    let at = screen(&h, 50.0, 50.0);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.hover_at(screen(&h, 60.0, 50.0));
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: screen(&h, 60.0, 50.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    let painted = doc(&h);
    let layer = painted.active_layer();
    // The mask got the paint, not the pixels.
    assert_eq!(layer.rev(), rev);
    let mask = layer.mask.as_ref().unwrap();
    assert_eq!(mask.value_at(55, 50, layer.offset), 1.0);
    assert_eq!(mask.value_at(5, 5, layer.offset), 0.0);
    assert_eq!(painted.flatten().get_pixel(55, 50).0, BLUE);
    assert_eq!(painted.flatten().get_pixel(5, 5).0, WHITE);
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    let layer = doc(&h).active_layer();
    assert_eq!(
        layer.mask.as_ref().unwrap().value_at(55, 50, layer.offset),
        0.0
    );
}

#[test]
fn group_clipping_and_blend_shortcuts() {
    let mut h = app();
    let cmd = Modifiers::COMMAND;
    // Clip the blue layer to the background, release it.
    h.key_press_modifiers(cmd.plus(Modifiers::ALT), Key::G);
    h.run_steps(1);
    assert!(doc(&h).active_layer().clipped);
    h.key_press_modifiers(cmd.plus(Modifiers::ALT), Key::G);
    h.run_steps(1);
    assert!(!doc(&h).active_layer().clipped);
    // Group it, ungroup it.
    h.key_press_modifiers(cmd, Key::G);
    h.run_steps(1);
    assert!(doc(&h).active_layer().is_group());
    assert_eq!(doc(&h).layers[1].depth, 1);
    h.key_press_modifiers(cmd.plus(Modifiers::SHIFT), Key::G);
    h.run_steps(1);
    assert_eq!(doc(&h).layers.len(), 2);
    // Blend modes, from the Move tool: next with Shift++, by letter with
    // Alt+Shift.
    h.state_mut().set_tool(Tool::Transform);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Plus);
    h.run_steps(1);
    assert_eq!(doc(&h).active_layer().blend, BlendMode::Dissolve);
    h.key_press_modifiers(Modifiers::ALT.plus(Modifiers::SHIFT), Key::M);
    h.run_steps(1);
    assert_eq!(doc(&h).active_layer().blend, BlendMode::Multiply);
    h.key_press_modifiers(Modifiers::SHIFT, Key::Minus);
    h.run_steps(1);
    assert_eq!(doc(&h).active_layer().blend, BlendMode::Darken);
    // One undo step per series of blend changes.
    h.state_mut().undo();
    assert_eq!(doc(&h).active_layer().blend, BlendMode::Normal);
}

#[test]
fn adjustment_and_fill_layers_from_the_menu() {
    let mut h = app();
    let app = h.state_mut();
    app.run(Action::NewAdjustmentLayer(AdjustKind::Invert));
    let doc = doc_of(app);
    assert_eq!(doc.layers.len(), 3);
    assert!(matches!(
        doc.active_layer().kind,
        LayerKind::Adjustment(Adjustment::Invert)
    ));
    assert_eq!(doc.flatten().get_pixel(0, 0).0, [255, 255, 0, 255]);
    // The panel shows its settings; painting goes to its mask.
    h.run_steps(2);
    let app = h.state_mut();
    app.run(Action::NewFillLayer(false));
    assert!(matches!(
        doc_of(app).active_layer().kind,
        LayerKind::Fill(Fill::Solid([0, 0, 0]))
    ));
    app.undo();
    app.undo();
    assert_eq!(doc_of(app).layers.len(), 2);
    // Invert on the pixels: at once, no window.
    app.run(Action::Adjust(AdjustKind::Invert));
    assert!(app.adjust.is_none());
    assert_eq!(doc_of(app).flatten().get_pixel(0, 0).0, [255, 255, 0, 255]);
}

#[test]
fn layer_style_window_cancels_or_records_one_step() {
    let mut h = app();
    h.state_mut()
        .run(Action::LayerStyle(Some(Style::DropShadow)));
    h.run_steps(1);
    assert!(h.state().layer_style.is_some());
    // Edited live, as the window does.
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .effects
        .shadow
        .enabled = true;
    h.run_steps(1);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().layer_style.is_none());
    assert!(!doc(&h).active_layer().effects.shadow.enabled, "cancelled");
    // No step left: the last one is still the blue layer's.
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    assert_eq!(doc(&h).layers.len(), 1);
    assert!(h.state_mut().doc.as_mut().unwrap().redo());
    // OK: one step.
    h.state_mut().run(Action::LayerStyle(None));
    h.run_steps(1);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .opacity = 0.5;
    h.run_steps(1);
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(doc(&h).active_layer().opacity, 0.5);
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    assert_eq!(doc(&h).active_layer().opacity, 1.0);
    assert_eq!(doc(&h).layers.len(), 2);
}
