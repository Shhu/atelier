//! Painting tools driven through the app: tool groups, the Gradient tool
//! and its editor, pen pressure, smoothing, airbrush, the toning and focus
//! tools, painting a layer mask.

use super::*;
use crate::document::NewMask;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A 200 × 100 white document, at 100 %, with `tool`.
fn app(tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(
                RgbaImage::from_pixel(200, 100, Rgba(WHITE)),
                None,
            ));
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    app.set_tool(tool);
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn pixel(h: &Harness<'static, AtelierApp>, x: i64, y: i64) -> [u8; 4] {
    doc(h).active_layer().pixel_at(x, y).unwrap()
}

/// Screen position of canvas point (x, y).
fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state().last_view.unwrap().to_screen(pos2(x, y))
}

/// Press at `from`, move through `path`, release at the last point (canvas
/// coordinates); `before_move` queues events before each move (pen).
fn drag(
    h: &mut Harness<'static, AtelierApp>,
    from: (f32, f32),
    path: &[(f32, f32)],
    before_move: impl Fn(&mut Harness<'static, AtelierApp>, Pos2),
) {
    let start = screen(h, from.0, from.1);
    h.hover_at(start);
    h.run_steps(1);
    before_move(&mut *h, start);
    h.drag_at(start);
    h.run_steps(1);
    let mut end = start;
    for &(x, y) in path {
        end = screen(h, x, y);
        before_move(&mut *h, end);
        h.hover_at(end);
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
}

fn no_pen(_: &mut Harness<'static, AtelierApp>, _: Pos2) {}

/// A pen at `force`: what egui-winit reports for Windows Ink, in the same
/// frame as the pointer event that follows.
fn pen(force: f32) -> impl Fn(&mut Harness<'static, AtelierApp>, Pos2) {
    move |h, pos| {
        h.input_mut().events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(7),
            id: egui::TouchId(1),
            phase: egui::TouchPhase::Move,
            pos,
            force: Some(force),
        })
    }
}

/// Height of the painted band in column `x`.
fn band(h: &Harness<'static, AtelierApp>, x: i64) -> usize {
    (0..100).filter(|&y| pixel(h, x, y) != WHITE).count()
}

#[test]
fn tool_groups_and_keys() {
    let mut h = app(Tool::Transform);
    let press = |h: &mut Harness<'static, AtelierApp>, shift: bool, key: Key| {
        let m = if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::NONE
        };
        h.key_press_modifiers(m, key);
        h.run_steps(1);
        h.state().tool
    };
    assert!(
        press(&mut h, false, Key::G) == Tool::Gradient,
        "the Gradient first"
    );
    assert!(press(&mut h, true, Key::G) == Tool::Bucket);
    assert!(press(&mut h, true, Key::G) == Tool::Gradient);
    assert!(press(&mut h, false, Key::O) == Tool::Dodge);
    assert!(press(&mut h, true, Key::O) == Tool::Burn);
    assert!(press(&mut h, true, Key::O) == Tool::Sponge);
    assert!(press(&mut h, true, Key::O) == Tool::Dodge);
    // The last of a group comes back with its key.
    press(&mut h, true, Key::O);
    press(&mut h, false, Key::B);
    assert!(press(&mut h, false, Key::O) == Tool::Burn);
    // Blur, Sharpen and Smudge have no key, like Photoshop (nor the anchor
    // point tools).
    assert!(Tool::Blur.shortcut().is_none());
    assert!(
        Tool::ALL
            .into_iter()
            .filter(|t| t.shortcut().is_none())
            .count()
            == 6
    );
}

#[test]
fn digits_set_the_tools_amounts() {
    let mut h = app(Tool::Brush);
    let app = h.state_mut();
    app.opacity_digit(5, 0.0);
    app.flow_digit(3, 10.0);
    assert_eq!((app.opacity, app.paint.flow), (0.5, 0.3));
    app.set_tool(Tool::Dodge);
    app.opacity_digit(2, 20.0);
    assert_eq!(app.paint.exposure, 0.2);
    app.set_tool(Tool::Smudge);
    app.opacity_digit(0, 30.0);
    assert_eq!(app.paint.smudge_strength, 1.0);
    app.set_tool(Tool::Gradient);
    app.opacity_digit(7, 40.0);
    assert_eq!(app.paint.gradient.opacity, 0.7);
    // The layer's opacity did not change.
    assert_eq!(app.doc.as_ref().unwrap().active_layer().opacity, 1.0);
}

#[test]
fn gradient_drag_draws_one_undo_step() {
    let mut h = app(Tool::Gradient);
    h.state_mut().paint.gradient.dither = false;
    drag(
        &mut h,
        (20.0, 50.0),
        &[(100.0, 50.0), (180.0, 50.0)],
        no_pen,
    );
    // Foreground (black) to background (white), from x = 20 to x = 180.
    assert_eq!(pixel(&h, 5, 10), [0, 0, 0, 255]);
    assert_eq!(pixel(&h, 195, 90), WHITE);
    let mid = pixel(&h, 100, 50)[0];
    assert!((120..136).contains(&mid), "{mid}");
    assert!(h.state().paint.gradient_drag.is_none());
    h.state_mut().undo();
    assert_eq!(pixel(&h, 5, 10), WHITE, "one step");
    assert!(!doc(&h).can_undo());
}

#[test]
fn gradient_click_without_drag_does_nothing() {
    let mut h = app(Tool::Gradient);
    drag(&mut h, (50.0, 50.0), &[(50.0, 50.0)], no_pen);
    assert!(!doc(&h).can_undo());
    assert_eq!(pixel(&h, 50, 50), WHITE);
}

#[test]
fn gradient_respects_the_selection_and_shift() {
    let mut h = app(Tool::Gradient);
    let app = h.state_mut();
    app.selection = Some(
        PixelRect {
            x: 0,
            y: 0,
            w: 100,
            h: 100,
        }
        .into(),
    );
    app.paint.gradient.dither = false;
    h.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    // Nearly horizontal: Shift makes it horizontal, the columns are even.
    drag(&mut h, (0.0, 50.0), &[(150.0, 58.0), (200.0, 60.0)], no_pen);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    assert_eq!(pixel(&h, 150, 50), WHITE, "outside the selection");
    assert_eq!(pixel(&h, 50, 0), pixel(&h, 50, 99), "Shift: by 45°");
    assert_ne!(pixel(&h, 50, 0), WHITE);
}

#[test]
fn gradient_editor_edits_the_tool_gradient() {
    let mut h = app(Tool::Gradient);
    h.state_mut().paint.editor = Some(gradient_editor::GradientEditor::new(
        crate::paint::gradient::Preset::BlackToWhite.gradient(),
    ));
    h.run_steps(2);
    assert!(h.state().modal_open());
    // While it is open, the keys don't reach the tools.
    h.key_press(Key::B);
    h.run_steps(1);
    assert!(h.state().tool == Tool::Gradient);
    let editor = h.state_mut().paint.editor.as_mut().unwrap();
    editor.gradient.colors[1].color = crate::paint::gradient::StopColor::Rgb([255, 0, 0]);
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert!(h.state().paint.editor.is_none());
    let g = &h.state().paint.gradient.gradient;
    assert_eq!(g.sample(1.0, [0; 3], [0; 3], true), [255.0, 0.0, 0.0, 1.0]);
}

#[test]
fn pen_pressure_changes_the_size_only_when_asked() {
    let width = |pressure_size: bool, force: f32| {
        let mut h = app(Tool::Brush);
        let app = h.state_mut();
        (app.brush_size, app.hardness, app.paint.smoothing) = (20.0, 1.0, 0.0);
        app.paint.pressure_size = pressure_size;
        drag(
            &mut h,
            (20.0, 50.0),
            &[(100.0, 50.0), (180.0, 50.0)],
            pen(force),
        );
        band(&h, 100)
    };
    let full = width(false, 0.25);
    assert!((20..=21).contains(&full), "pressure ignored: {full}");
    assert_eq!(width(true, 1.0), full);
    let light = width(true, 0.25);
    assert!((5..=6).contains(&light), "a quarter of the size: {light}");
}

#[test]
fn pen_pressure_changes_the_opacity() {
    let mut h = app(Tool::Brush);
    let app = h.state_mut();
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.paint.pressure_opacity = true;
    drag(
        &mut h,
        (20.0, 50.0),
        &[(100.0, 50.0), (180.0, 50.0)],
        pen(0.5),
    );
    let v = pixel(&h, 100, 50)[0];
    assert!((125..=130).contains(&v), "half opacity: {v}");
}

#[test]
fn a_mouse_stroke_after_a_pen_stroke_has_full_pressure() {
    let mut h = app(Tool::Brush);
    let app = h.state_mut();
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.paint.pressure_size = true;
    drag(&mut h, (20.0, 20.0), &[(180.0, 20.0)], pen(0.25));
    drag(
        &mut h,
        (20.0, 70.0),
        &[(100.0, 70.0), (180.0, 70.0)],
        no_pen,
    );
    let rows = |from: i64, to: i64| (from..to).filter(|&y| pixel(&h, 100, y) != WHITE).count();
    assert!(rows(0, 45) <= 6, "pen stroke");
    assert!(
        (20..=21).contains(&rows(45, 100)),
        "full size: {}",
        rows(45, 100)
    );
}

#[test]
fn smoothing_trails_then_catches_up() {
    let mut h = app(Tool::Brush);
    let app = h.state_mut();
    (app.brush_size, app.hardness, app.paint.smoothing) = (6.0, 1.0, 1.0);
    drag(&mut h, (20.0, 50.0), &[(60.0, 50.0), (150.0, 50.0)], no_pen);
    // Released at x = 150: the stroke reaches it.
    assert_ne!(pixel(&h, 150, 50), WHITE);
    assert_eq!(pixel(&h, 160, 50), WHITE);
    let end = h.state().last_paint.unwrap();
    assert!(end.distance(pos2(150.0, 50.0)) < 0.01, "{end:?}");
}

#[test]
fn airbrush_builds_up_while_held() {
    let held = |airbrush: bool| {
        let mut h = app(Tool::Brush);
        let app = h.state_mut();
        (app.brush_size, app.paint.flow, app.paint.airbrush) = (10.0, 0.1, airbrush);
        let at = screen(&h, 100.0, 50.0);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(10);
        h.drop_at(at);
        h.run_steps(2);
        pixel(&h, 100, 50)[0]
    };
    let (still, sprayed) = (held(false), held(true));
    assert!(still > 220, "one dab at 10 %: {still}");
    assert!(sprayed < still - 40, "{sprayed} vs {still}");
}

#[test]
fn dodge_lightens_and_alt_burns() {
    let tone = |alt: bool| {
        let mut h = app(Tool::Dodge);
        let doc = h.state_mut().doc.as_mut().unwrap();
        doc.active_layer_mut().replace_pixels(RgbaImage::from_pixel(
            200,
            100,
            Rgba([100, 100, 100, 255]),
        ));
        h.state_mut().brush_size = 30.0;
        h.run_steps(1);
        if alt {
            h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
        }
        drag(&mut h, (20.0, 50.0), &[(180.0, 50.0)], no_pen);
        h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        let v = pixel(&h, 100, 50)[0];
        assert!(doc_can_undo(&h));
        v
    };
    assert!(tone(false) > 110);
    assert!(tone(true) < 90, "Alt: Burn");
}

fn doc_can_undo(h: &Harness<'static, AtelierApp>) -> bool {
    doc(h).can_undo()
}

#[test]
fn blur_with_sample_all_layers_paints_on_an_empty_layer() {
    let mut h = app(Tool::Blur);
    let app = h.state_mut();
    let d = app.doc.as_mut().unwrap();
    d.active_layer_mut()
        .replace_pixels(RgbaImage::from_fn(200, 100, |x, _| {
            Rgba(if x < 100 { [0, 0, 0, 255] } else { WHITE })
        }));
    d.add_empty_layer();
    app.paint.sample_all = true;
    (app.brush_size, app.paint.blur_strength) = (20.0, 1.0);
    h.run_steps(1);
    drag(&mut h, (100.0, 10.0), &[(100.0, 90.0)], no_pen);
    let p = pixel(&h, 100, 50);
    assert!(
        p[3] > 0 && p[0] > 0 && p[0] < 255,
        "blurred edge on the new layer: {p:?}"
    );
    assert_eq!(pixel(&h, 150, 50)[3], 0, "nothing away from the stroke");
    // Without it, the empty layer stays empty.
    h.state_mut().undo();
    h.state_mut().paint.sample_all = false;
    drag(&mut h, (100.0, 10.0), &[(100.0, 90.0)], no_pen);
    assert_eq!(pixel(&h, 100, 50)[3], 0);
}

#[test]
fn painting_a_targeted_mask_in_grayscale() {
    let mut h = app(Tool::Brush);
    let app = h.state_mut();
    let d = app.doc.as_mut().unwrap();
    d.active_layer_mut()
        .replace_pixels(RgbaImage::from_pixel(200, 100, Rgba([200, 30, 30, 255])));
    assert!(d.add_mask(NewMask::RevealAll, None));
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.primary = Color32::from_rgb(255, 0, 0);
    h.run_steps(1);
    drag(&mut h, (20.0, 50.0), &[(180.0, 50.0)], no_pen);
    let layer = doc(&h).active_layer();
    let mask = layer.mask.as_ref().expect("the mask stays");
    // Red is painted as its gray (76), the layer's pixels are untouched.
    let v = mask.value_at(100, 50, layer.offset);
    assert!((v * 255.0 - 76.0).abs() < 2.0, "{}", v * 255.0);
    assert_eq!(mask.value_at(100, 5, layer.offset), 1.0);
    assert_eq!(layer.pixel_at(100, 50), Some([200, 30, 30, 255]));
    // The eraser paints the background color (white) back.
    h.state_mut().set_tool(Tool::Eraser);
    drag(&mut h, (20.0, 50.0), &[(180.0, 50.0)], no_pen);
    let layer = doc(&h).active_layer();
    assert_eq!(
        layer.mask.as_ref().unwrap().value_at(100, 50, layer.offset),
        1.0
    );
    // One undo step per stroke.
    h.state_mut().undo();
    let layer = doc(&h).active_layer();
    let v = layer.mask.as_ref().unwrap().value_at(100, 50, layer.offset);
    assert!(v < 0.5);
}

#[test]
fn gradient_on_a_targeted_mask() {
    let mut h = app(Tool::Gradient);
    let app = h.state_mut();
    assert!(app.doc.as_mut().unwrap().add_mask(NewMask::RevealAll, None));
    app.paint.gradient.dither = false;
    h.run_steps(1);
    drag(&mut h, (0.0, 50.0), &[(100.0, 50.0), (200.0, 50.0)], no_pen);
    let layer = doc(&h).active_layer();
    let mask = layer.mask.as_ref().unwrap();
    assert!(
        mask.value_at(5, 50, layer.offset) < 0.05,
        "black at the start"
    );
    assert!(mask.value_at(195, 50, layer.offset) > 0.95);
    assert_eq!(layer.pixel_at(5, 50), Some(WHITE));
}
