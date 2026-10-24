//! Move tool, Free Transform, Crop, painting and retouching, driven
//! through the real app: the second review's findings.

use super::retouch_tools::{FillRequest, Sample};
use super::transform::{Command, Mode};
use super::*;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: Rgba<u8> = Rgba([200, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 200, 255]);

fn app_doc(doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut h = Harness::builder()
        .with_size([1400.0, 1000.0])
        // Gestures and key series at a real pace.
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn app_with(image: RgbaImage, tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = app_doc(Document::new(image, None));
    let app = h.state_mut();
    app.set_tool(tool);
    (app.brush_size, app.hardness) = (10.0, 1.0);
    h.run_steps(1);
    h
}

/// Red 400 × 300 background; "Logo", blue 100 × 50 at (100, 100), active,
/// with the Move tool.
fn logo_app() -> Harness<'static, AtelierApp> {
    let mut doc = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    doc.add_layer(
        "Logo".into(),
        RgbaImage::from_pixel(100, 50, BLUE),
        (100, 100),
    );
    let mut h = app_doc(doc);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    h
}

/// Red background, "A" (50, 50) and "B" (200, 50), blue 60 × 40, both
/// selected, B active.
fn two_layers_selected() -> Harness<'static, AtelierApp> {
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    d.add_layer("A".into(), RgbaImage::from_pixel(60, 40, BLUE), (50, 50));
    d.add_layer("B".into(), RgbaImage::from_pixel(60, 40, BLUE), (200, 50));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    let ids: Vec<u64> = doc(&h).layers[1..].iter().map(|l| l.id).collect();
    h.state_mut().layer_set = ids;
    h.run_steps(1);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn pixel(h: &Harness<'static, AtelierApp>, x: i64, y: i64) -> [u8; 4] {
    doc(h)
        .active_layer()
        .pixel_at(x, y)
        .expect("inside the layer")
}

fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn steps(h: &Harness<'static, AtelierApp>) -> usize {
    doc(h).history().1
}

fn button(h: &mut Harness<'_, AtelierApp>, at: Pos2, pressed: bool, m: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: m,
    });
    h.run_steps(1);
}

/// Press at the first point (canvas), through the others, release at the
/// last, with `m` held.
fn drag(h: &mut Harness<'_, AtelierApp>, path: &[(f32, f32)], m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    let start = screen(h, path[0].0, path[0].1);
    h.hover_at(start);
    h.run_steps(1);
    button(h, start, true, m);
    let mut end = start;
    for &(x, y) in &path[1..] {
        end = screen(h, x, y);
        h.hover_at(end);
        h.run_steps(1);
    }
    button(h, end, false, m);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn click(h: &mut Harness<'_, AtelierApp>, at: (f32, f32), m: Modifiers) {
    drag(h, &[at], m);
}

fn key(h: &mut Harness<'_, AtelierApp>, key: Key, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        });
    }
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn wait_for_fill(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..3000 {
        if !h.state().retouch.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run_steps(1);
    }
    panic!("the fill never ended");
}

fn has_fonts(h: &mut Harness<'static, AtelierApp>) -> bool {
    !h.state_mut().text_engine().families().is_empty()
}

// ------------------------------------------------- a large pending distort

/// A 2600 × 2400 layer (computed on another thread when transformed) with
/// a distort pending, its bottom narrowed.
fn big_pending(top: bool) -> Harness<'static, AtelierApp> {
    let mut d = Document::new(RgbaImage::from_pixel(2800, 2600, RED), None);
    d.add_layer(
        "Big".into(),
        RgbaImage::from_pixel(2600, 2400, BLUE),
        (100, 50),
    );
    if !top {
        d.add_layer(
            "Top".into(),
            RgbaImage::from_pixel(50, 50, Rgba([0, 200, 0, 255])),
            (10, 10),
        );
        d.active = 1;
    }
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    let narrowed = [[0.0, 0.0], [1.0, 0.0], [0.9, 1.0], [0.1, 1.0]];
    assert!(h.state_mut().set_transform_frame(narrowed));
    h.run_steps(1);
    h
}

#[test]
fn deleting_the_layer_of_a_large_pending_distort_does_not_crash() {
    let mut h = big_pending(true);
    let steps0 = steps(&h);
    h.state_mut().run(Action::Delete);
    assert!(!h.state().transform_busy(), "applied before the delete");
    h.run_steps(3);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 1);
    assert_eq!(d.history().1, steps0 + 2);
    assert_eq!(d.history().0[steps0 + 1], t!("menu-layer-delete"));
}

#[test]
fn a_large_pending_distort_is_applied_before_the_next_command() {
    let mut h = big_pending(false);
    let steps0 = steps(&h);
    h.state_mut().run(Action::MergeDown);
    assert!(!h.state().transform_busy());
    h.run_steps(3);
    let d = doc(&h);
    let (names, done) = d.history();
    assert_eq!(done, steps0 + 2, "{names:?}");
    assert_eq!(names[steps0 + 1], t!("menu-layer-merge-down"));
    // Merged as distorted: the narrowed bottom shows the background.
    assert_eq!(d.flatten().get_pixel(150, 2440).0, RED.0);
    assert_eq!(d.flatten().get_pixel(1400, 1000).0, BLUE.0);
}

#[test]
fn committing_waits_for_a_large_pending_distort() {
    // What a save, an export or a tab switch does first.
    let mut h = big_pending(true);
    let steps0 = steps(&h);
    h.state_mut().commit_pending();
    assert!(!h.state().transform_busy());
    assert!(h.state().transform_rect.is_none());
    let d = doc(&h);
    assert_eq!(d.history().1, steps0 + 1);
    assert_eq!(d.flatten().get_pixel(150, 2440).0, RED.0);
}

// ------------------------------------------------------ Free Transform undo

#[test]
fn ctrl_z_after_switching_to_warp_keeps_the_drag_before_it() {
    let mut h = logo_app();
    key(&mut h, Key::T, Modifiers::COMMAND);
    drag(
        &mut h,
        &[(150.0, 125.0), (165.0, 125.0), (180.0, 125.0)],
        Modifiers::NONE,
    );
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.run_steps(1);
    assert!(h.state().transform_warping());
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(!h.state().transform_warping());
    let r = h.state().transform_rect.expect("still transforming");
    assert_eq!(r.min, pos2(130.0, 100.0));
    // Applied as moved, not warped.
    let s = steps(&h);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(steps(&h), s + 1);
    let d = doc(&h);
    assert_eq!(d.active_layer().offset, (130, 100));
    assert_eq!(d.active_layer().image().dimensions(), (100, 50));
}

#[test]
fn ctrl_z_in_free_transform_undoes_nudges_and_turns_one_by_one() {
    let mut h = logo_app();
    key(&mut h, Key::T, Modifiers::COMMAND);
    drag(
        &mut h,
        &[(150.0, 125.0), (165.0, 125.0), (180.0, 125.0)],
        Modifiers::NONE,
    );
    for _ in 0..5 {
        key(&mut h, Key::ArrowRight, Modifiers::NONE);
    }
    let x = |h: &Harness<'static, AtelierApp>| h.state().transform_rect.map(|r| r.min.x);
    assert_eq!(x(&h), Some(135.0));
    // The series of arrows, then the drag.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(x(&h), Some(130.0));
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(x(&h), Some(100.0));
    // A quarter turn after a drag.
    drag(
        &mut h,
        &[(150.0, 125.0), (165.0, 125.0), (180.0, 125.0)],
        Modifiers::NONE,
    );
    h.state_mut().transform_command(Command::RotateCw);
    h.run_steps(1);
    assert_eq!(h.state().transform_angle, 90.0);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(h.state().transform_angle, 0.0);
    assert_eq!(x(&h), Some(130.0));
}

// --------------------------------------------------------- Transform Again

/// An ellipse, transparent around, `w` × `h`.
fn ellipse(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let (u, v) = (
            (x as f32 + 0.5) / w as f32 - 0.5,
            (y as f32 + 0.5) / h as f32 - 0.5,
        );
        if u * u + v * v <= 0.25 {
            BLUE
        } else {
            Rgba([0, 0, 0, 0])
        }
    })
}

fn center_of(d: &Document) -> (f64, f64) {
    let b = d.active_layer().content_bounds().expect("pixels");
    (b.x as f64 + b.w as f64 / 2.0, b.y as f64 + b.h as f64 / 2.0)
}

/// Step and repeat around a pivot below the element (a flower's petals):
/// the second turn goes around the same pivot.
fn step_and_repeat(petal: RgbaImage, same_layer: bool) {
    let mut d = Document::new(RgbaImage::from_pixel(600, 600, Rgba(WHITE)), None);
    d.add_layer("Petal".into(), petal, (280, 100));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    let s = h.state().transform_source().expect("a petal");
    let (w, hh) = (s.width() as f64, s.height() as f64);
    // The box turned 30° around (0.5, 4.0) (box units).
    let pivot = [0.5 * w, 4.0 * hh];
    let a = 30f64.to_radians();
    let turn = |[x, y]: [f64; 2], c: [f64; 2]| {
        let (dx, dy) = (x - c[0], y - c[1]);
        [
            c[0] + dx * a.cos() - dy * a.sin(),
            c[1] + dx * a.sin() + dy * a.cos(),
        ]
    };
    let unit = [[0.0, 0.0], [w, 0.0], [w, hh], [0.0, hh]].map(|p| {
        let [x, y] = turn(p, pivot);
        [x / w, y / hh]
    });
    assert!(h.state_mut().set_transform_frame(unit));
    key(&mut h, Key::Enter, Modifiers::NONE);
    let first = center_of(doc(&h));
    if !same_layer {
        h.state_mut().run(Action::Duplicate);
        h.run_steps(1);
    }
    h.state_mut().transform_command(Command::Again);
    h.run_steps(2);
    let again = center_of(doc(&h));
    let canvas_pivot = [280.0 + pivot[0], 100.0 + pivot[1]];
    let [ex, ey] = turn([first.0, first.1], canvas_pivot);
    assert!(
        (again.0 - ex).abs() < 3.0 && (again.1 - ey).abs() < 3.0,
        "same layer {same_layer}: {again:?}, expected ({ex:.1}, {ey:.1})"
    );
}

#[test]
fn transform_again_turns_an_ellipse_around_the_same_pivot() {
    step_and_repeat(RgbaImage::from_pixel(40, 80, BLUE), false);
    step_and_repeat(ellipse(40, 80), false);
    step_and_repeat(ellipse(40, 80), true);
}

// ------------------------------------------------------------------ typing

#[test]
fn ctrl_y_while_typing_toggles_proof_colors_and_goes_on_typing() {
    let mut h = app_doc(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
    if !has_fonts(&mut h) {
        return;
    }
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(1);
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.run_steps(1);
    h.event(egui::Event::Text("Hi".into()));
    h.run_steps(1);
    let proof = doc(&h).proof.colors;
    key(&mut h, Key::Y, Modifiers::COMMAND);
    assert!(h.state().typing_on_canvas());
    assert_eq!(doc(&h).proof.colors, !proof);
}

#[test]
fn an_edit_typed_back_to_its_text_leaves_no_step() {
    let mut h = app_doc(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
    if !has_fonts(&mut h) {
        return;
    }
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(1);
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.state_mut().type_text("Hello");
    h.state_mut().commit_text();
    h.run_steps(2);
    // An opacity change after it.
    let d = h.state_mut().doc.as_mut().unwrap();
    d.checkpoint("Opacity".into());
    d.active_layer_mut().opacity = 0.5;
    h.run_steps(1);
    let i = doc(&h).active;
    h.state_mut().edit_text_layer(i, pos2(60.0, 80.0));
    h.run_steps(1);
    h.event(egui::Event::Text("X".into()));
    h.run_steps(1);
    // The typing, then the opacity.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(!h.state().typing_on_canvas());
    let d = doc(&h);
    assert_eq!(d.active_layer().opacity, 1.0);
    assert_eq!(
        d.active_layer().text.as_ref().map(|t| t.text.as_str()),
        Some("Hello")
    );
}

// ---------------------------------------------------------------- Move tool

#[test]
fn a_move_near_a_corner_moves_without_transform_controls() {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    let s = steps(&h);
    drag(
        &mut h,
        &[(198.0, 148.0), (218.0, 158.0), (238.0, 168.0)],
        Modifiers::NONE,
    );
    let d = doc(&h);
    assert_eq!(d.active_layer().offset, (140, 120));
    assert_eq!(d.active_layer().image().dimensions(), (100, 50));
    assert!(h.state().transform_rect.is_none());
    assert_eq!(steps(&h), s + 1);
    // Delete then removes the layer moved.
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(doc(&h).layers.len(), 1);
}

#[test]
fn show_transform_controls_lets_a_corner_scale() {
    let mut h = logo_app();
    h.state_mut().show_transform_controls = true;
    h.run_steps(1);
    drag(
        &mut h,
        &[(200.0, 150.0), (220.0, 160.0), (240.0, 170.0)],
        Modifiers::NONE,
    );
    assert!(h.state().free_transform);
    let r = h.state().transform_rect.expect("scaling");
    assert_eq!(round_rect(r).size(), vec2(140.0, 70.0));
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(doc(&h).active_layer().image().dimensions(), (140, 70));
}

#[test]
fn with_a_selection_the_handles_scale_the_selected_pixels() {
    let mut h = logo_app();
    h.state_mut().show_transform_controls = true;
    h.state_mut().selection = Some(
        PixelRect {
            x: 110,
            y: 105,
            w: 60,
            h: 40,
        }
        .into(),
    );
    h.run_steps(1);
    assert_eq!(
        h.state().selection_frame(),
        Some(Rect::from_min_size(pos2(110.0, 105.0), vec2(60.0, 40.0)))
    );
    // The selection's corner, not the layer's.
    drag(
        &mut h,
        &[(169.0, 144.0), (184.0, 154.0), (199.0, 164.0)],
        Modifiers::NONE,
    );
    let r = h.state().transform_rect.expect("transforming the pixels");
    assert_eq!(round_rect(r).min, pos2(110.0, 105.0));
    assert_eq!(round_rect(r).size(), vec2(90.0, 60.0));
    key(&mut h, Key::Enter, Modifiers::NONE);
    let flat = doc(&h).flatten();
    // Scaled beyond the layer's edge, the rest of it in place.
    assert_eq!(flat.get_pixel(195, 160).0, BLUE.0);
    assert_eq!(flat.get_pixel(105, 102).0, BLUE.0);
    assert_eq!(flat.get_pixel(195, 168).0, RED.0);
}

#[test]
fn alt_drag_copies_in_one_step() {
    let mut h = logo_app();
    let s = steps(&h);
    drag(
        &mut h,
        &[(150.0, 125.0), (200.0, 125.0), (250.0, 125.0)],
        Modifiers::ALT,
    );
    let d = doc(&h);
    assert_eq!(d.layers.len(), 3);
    assert_eq!(d.layers[1].offset, (100, 100));
    assert_eq!(d.layers[2].offset, (200, 100));
    assert_eq!(d.history().1, s + 1);
    // One Ctrl+Z: the copy goes.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 2);
    assert_eq!(d.layers[1].offset, (100, 100));
}

#[test]
fn alt_drag_copies_every_selected_layer() {
    let mut h = two_layers_selected();
    let s = steps(&h);
    drag(
        &mut h,
        &[(230.0, 70.0), (230.0, 120.0), (230.0, 170.0)],
        Modifiers::ALT,
    );
    let d = doc(&h);
    let offsets: Vec<(i64, i64)> = d.layers.iter().map(|l| l.offset).collect();
    assert_eq!(d.layers.len(), 5, "{offsets:?}");
    assert_eq!(d.history().1, s + 1);
    // The originals stay, their copies (above each) moved.
    assert_eq!(offsets[1..], [(50, 50), (50, 150), (200, 50), (200, 150)]);
}

#[test]
fn delete_removes_every_selected_layer_in_one_step() {
    let mut h = two_layers_selected();
    let s = steps(&h);
    h.state_mut().run(Action::Delete);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 1);
    assert_eq!(d.history().1, s + 1);
    // The last layer stays, and says so.
    h.state_mut().run(Action::Delete);
    assert_eq!(doc(&h).layers.len(), 1);
    assert_eq!(h.state().status, t!("tools4-status-last-layer"));
}

#[test]
fn ctrl_t_with_a_work_path_selected_leaves_the_layer() {
    use crate::vector::{SubPath, pt};
    let mut h = logo_app();
    {
        let d = h.state_mut().doc.as_mut().unwrap();
        let mut p = crate::vector::Path::default();
        p.subpaths.push(SubPath::polygon([
            pt(20.0, 20.0),
            pt(80.0, 20.0),
            pt(80.0, 60.0),
            pt(20.0, 60.0),
        ]));
        d.work_path = Some(p);
    }
    h.state_mut().vector.selected = Some(crate::document::PathRef::Work);
    h.state_mut().set_tool(Tool::PathSelect);
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert_eq!(h.state().tool, Tool::PathSelect);
    assert!(h.state().transform_rect.is_none());
    assert_eq!(h.state().status, t!("tools4-status-path-transform"));
}

// --------------------------------------------------------------------- Crop

#[test]
fn a_crop_drag_draws_a_crop_box() {
    let mut h = logo_app();
    h.state_mut().set_tool(Tool::Canvas);
    h.run_steps(1);
    drag(
        &mut h,
        &[(50.0, 50.0), (100.0, 85.0), (150.0, 120.0)],
        Modifiers::NONE,
    );
    let r = h.state().canvas_rect.expect("a crop box");
    assert_eq!(r, Rect::from_min_size(pos2(50.0, 50.0), vec2(100.0, 70.0)));
    let d = doc(&h);
    assert_eq!(
        (d.layers[0].offset, d.layers[1].offset),
        ((0, 0), (100, 100))
    );
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!((doc(&h).width(), doc(&h).height()), (100, 70));
}

// ---------------------------------------------------------------- painting

#[test]
fn a_short_alt_right_drag_opens_no_menu() {
    let mut h = app_with(RgbaImage::from_pixel(256, 100, Rgba(WHITE)), Tool::Brush);
    h.state_mut().brush_size = 50.0;
    h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    let p = screen(&h, 100.0, 50.0);
    h.hover_at(p);
    h.run_steps(1);
    let secondary = |h: &mut Harness<'_, AtelierApp>, at: Pos2, pressed: bool| {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::ALT,
        });
        h.run_steps(1);
    };
    secondary(&mut h, p, true);
    let q = p + vec2(4.0, 0.0);
    h.hover_at(q);
    h.run_steps(1);
    secondary(&mut h, q, false);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert_ne!(h.state().brush_size, 50.0);
    assert!(!egui::Popup::is_any_open(&h.ctx));
}

#[test]
fn the_bucket_fills_an_empty_layer_whole_with_anti_alias() {
    let mut h = app_with(RgbaImage::from_pixel(100, 100, Rgba(WHITE)), Tool::Bucket);
    h.state_mut().doc.as_mut().unwrap().add_empty_layer();
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.run_steps(1);
    assert!(h.state().paint.bucket.anti_alias);
    click(&mut h, (50.0, 50.0), Modifiers::NONE);
    for (x, y) in [(50, 50), (0, 50), (0, 0), (99, 99)] {
        assert_eq!(pixel(&h, x, y), [255, 0, 0, 255], "({x}, {y})");
    }
}

/// Gray ramp with a dark 10 px spot at (128, 50).
fn spotted() -> RgbaImage {
    RgbaImage::from_fn(256, 100, |x, y| {
        let d = ((x as f32 - 128.0).powi(2) + (y as f32 - 50.0).powi(2)).sqrt();
        if d < 5.0 {
            Rgba([20, 20, 20, 255])
        } else {
            Rgba([x as u8, x as u8, x as u8, 255])
        }
    })
}

fn near_gray(p: [u8; 4], v: u8) -> bool {
    p[3] > 240 && p[..3].iter().all(|&c| c.abs_diff(v) <= 12)
}

#[test]
fn healing_an_empty_layer_from_the_layers_below_heals_what_shows() {
    for sample in [Sample::CurrentAndBelow, Sample::All] {
        let mut h = app_with(spotted(), Tool::HealingBrush);
        h.state_mut().doc.as_mut().unwrap().add_empty_layer();
        let s = steps(&h);
        let app = h.state_mut();
        app.retouch.heal_sample = sample;
        app.retouch.heal_aligned = false;
        app.brush_size = 20.0;
        h.run_steps(1);
        click(&mut h, (128.0, 20.0), Modifiers::ALT);
        drag(
            &mut h,
            &[(126.0, 50.0), (128.0, 50.0), (130.0, 50.0)],
            Modifiers::NONE,
        );
        assert_eq!(steps(&h), s + 1, "{sample:?}: {}", h.state().status);
        let seen = doc(&h).flatten().get_pixel(128, 50).0;
        assert!(near_gray(seen, 128), "{sample:?}: {seen:?}");
        assert!(pixel(&h, 128, 50)[3] > 240, "painted on the empty layer");
    }
}

#[test]
fn spot_healing_an_empty_layer_sampling_all_layers_fills_it() {
    let mut h = app_with(spotted(), Tool::SpotHealing);
    h.state_mut().doc.as_mut().unwrap().add_empty_layer();
    let s = steps(&h);
    let app = h.state_mut();
    app.retouch.spot_sample_all = true;
    app.retouch.settings.ai = false;
    app.brush_size = 16.0;
    h.run_steps(1);
    drag(&mut h, &[(127.0, 50.0), (129.0, 50.0)], Modifiers::NONE);
    wait_for_fill(&mut h);
    assert_eq!(steps(&h), s + 1);
    assert!(pixel(&h, 128, 50)[3] > 240);
    let seen = doc(&h).flatten().get_pixel(128, 50).0;
    assert!(near_gray(seen, 128), "{seen:?}");
}

#[test]
fn spot_healing_an_empty_layer_from_itself_records_no_step() {
    let mut h = app_with(spotted(), Tool::SpotHealing);
    h.state_mut().doc.as_mut().unwrap().add_empty_layer();
    let s = steps(&h);
    let app = h.state_mut();
    app.retouch.spot_sample_all = false;
    app.retouch.settings.ai = false;
    app.brush_size = 16.0;
    h.run_steps(1);
    drag(&mut h, &[(127.0, 50.0), (129.0, 50.0)], Modifiers::NONE);
    wait_for_fill(&mut h);
    assert_eq!(steps(&h), s);
    assert!(!h.state().status.is_empty());
}

fn add_rectangle(h: &mut Harness<'static, AtelierApp>) {
    use crate::vector::{Bounds, Live, Paint, Shape, ShapeKind, ShapeStroke, pt};
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(20.0, 20.0), pt(120.0, 80.0)),
    );
    let shape = Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default());
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .add_shape_layer("R".into(), "R", shape);
    h.run_steps(1);
}

fn content_aware(h: &mut Harness<'static, AtelierApp>) {
    let app = h.state_mut();
    app.selection = Some(
        PixelRect {
            x: 40,
            y: 40,
            w: 20,
            h: 20,
        }
        .into(),
    );
    let mut settings = app.retouch.settings;
    settings.ai = false;
    app.start_content_aware(FillRequest {
        settings,
        opacity: 1.0,
        keep_alpha: false,
        from_fill: false,
    });
    wait_for_fill(h);
}

#[test]
fn content_aware_fill_asks_to_rasterize_a_shape_and_refuses_a_hidden_layer() {
    let mut h = app_with(spotted(), Tool::Lasso);
    add_rectangle(&mut h);
    let s = steps(&h);
    content_aware(&mut h);
    assert!(doc(&h).active_layer().shape.is_some());
    assert!(h.state().paint.rasterize_confirm.is_some());
    assert_eq!(steps(&h), s);

    let mut h = app_with(spotted(), Tool::Lasso);
    h.state_mut().doc.as_mut().unwrap().layers[0].visible = false;
    let s = steps(&h);
    content_aware(&mut h);
    assert_eq!(steps(&h), s);
    assert!(!h.state().status.is_empty());
}

#[test]
fn a_large_healing_stroke_is_healed_on_another_thread() {
    // A ramp with a dark spot, healed with a brush covering most of it.
    let image = RgbaImage::from_fn(1800, 1400, |x, y| {
        let d = ((x as f32 - 900.0).powi(2) + (y as f32 - 700.0).powi(2)).sqrt();
        let v = (x / 8) as u8;
        if d < 40.0 {
            Rgba([10, 10, 10, 255])
        } else {
            Rgba([v, v, v, 255])
        }
    });
    let mut h = app_with(image, Tool::HealingBrush);
    let s = steps(&h);
    let app = h.state_mut();
    app.retouch.heal_aligned = false;
    app.brush_size = 1400.0;
    app.zoom = 0.25;
    h.run_steps(2);
    click(&mut h, (900.0, 300.0), Modifiers::ALT);
    drag(&mut h, &[(898.0, 700.0), (902.0, 700.0)], Modifiers::NONE);
    wait_for_fill(&mut h);
    assert_eq!(steps(&h), s + 1);
    let p = pixel(&h, 900, 700);
    assert!(p[0] > 60, "the spot is gone: {p:?}");
}
