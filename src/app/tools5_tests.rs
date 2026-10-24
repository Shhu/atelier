//! Move tool, Free Transform, Crop, painting and retouching, driven
//! through the real app: the third review's findings.

#![allow(dead_code)]

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

/// 2000 × 2000 of a smooth texture with dark spots.
fn noisy(w: u32, h: u32) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        let v = (100.0 + 50.0 * ((x as f32 / 23.0).sin() + (y as f32 / 31.0).cos())) as u8;
        if (x / 97 + y / 89) % 7 == 0 {
            Rgba([20, 20, 20, 255])
        } else {
            Rgba([v, v, v, 255])
        }
    })
}

// ------------------------------------------------------------- retouching

/// A long diagonal Healing Brush stroke, healed on another thread; with
/// `cancel`, its Cancel is clicked at once.
fn heal_on_a_worker(cancel: bool) -> (Harness<'static, AtelierApp>, RgbaImage, usize) {
    let image = noisy(3000, 1200);
    let mut h = app_with(image.clone(), Tool::HealingBrush);
    let n = steps(&h);
    let app = h.state_mut();
    app.retouch.heal_aligned = true;
    app.brush_size = 120.0;
    (app.zoom, app.pan) = (0.25, Vec2::ZERO);
    h.run_steps(4);
    click(&mut h, (100.0, 850.0), Modifiers::ALT);
    let path: Vec<(f32, f32)> = (0..=40)
        .map(|i| (200.0 + 65.0 * i as f32, 100.0 + 20.0 * i as f32))
        .collect();
    drag(&mut h, &path, Modifiers::NONE);
    assert!(h.state().retouch.busy(), "healed on another thread");
    if cancel {
        h.state().retouch.cancel_fill();
    }
    wait_for_fill(&mut h);
    (h, image, n)
}

#[test]
fn a_cancelled_healing_stroke_is_given_up() {
    let (h, image, n) = heal_on_a_worker(true);
    assert_eq!(steps(&h), n, "no step");
    assert!(
        doc(&h).active_layer().image() == &image,
        "the layer as it was"
    );
    let name = Tool::HealingBrush.name();
    assert_eq!(
        h.state().status,
        t!("retouch-status-cancelled", name = name.as_str())
    );
    let (h, image, n) = heal_on_a_worker(false);
    assert_eq!(steps(&h), n + 1);
    assert!(doc(&h).active_layer().image() != &image);
}

#[test]
fn a_large_patch_is_healed_on_a_worker() {
    let image = noisy(3000, 1200);
    let mut h = app_with(image.clone(), Tool::Patch);
    (h.state_mut().zoom, h.state_mut().pan) = (0.25, Vec2::ZERO);
    h.run_steps(4);
    h.state_mut().retouch.patch_mode = super::retouch_tools::PatchMode::Destination;
    h.state_mut().selection = Some(
        PixelRect {
            x: 100,
            y: 50,
            w: 2000,
            h: 1100,
        }
        .into(),
    );
    h.run_steps(1);
    let n = steps(&h);
    drag(
        &mut h,
        &[(1000.0, 500.0), (1100.0, 500.0), (1200.0, 500.0)],
        Modifiers::NONE,
    );
    assert!(h.state().retouch.busy(), "patched on another thread");
    wait_for_fill(&mut h);
    assert_eq!(steps(&h), n + 1);
    assert!(doc(&h).active_layer().image() != &image);
    // Destination: the selection went with the patch.
    let moved = h.state().selection.as_ref().map(|s| s.rect.x);
    assert_eq!(moved, Some(300));
    // Cancelled: nothing.
    h.state_mut().selection = Some(
        PixelRect {
            x: 100,
            y: 50,
            w: 2000,
            h: 1100,
        }
        .into(),
    );
    h.run_steps(1);
    let before = doc(&h).active_layer().image().clone();
    drag(
        &mut h,
        &[(1000.0, 500.0), (1100.0, 500.0), (1200.0, 500.0)],
        Modifiers::NONE,
    );
    h.state().retouch.cancel_fill();
    wait_for_fill(&mut h);
    assert_eq!(steps(&h), n + 1);
    assert!(doc(&h).active_layer().image() == &before);
}

// ------------------------------------------------------------- fills

/// A gray document with a rectangle shape layer, active.
fn shape_app() -> Harness<'static, AtelierApp> {
    let mut h = app_with(RgbaImage::from_pixel(200, 120, RED), Tool::Lasso);
    use crate::vector::{Bounds, Live, Paint, Shape, ShapeKind, ShapeStroke, pt};
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(20.0, 20.0), pt(120.0, 90.0)),
    );
    let shape = Shape::live(live, Paint::Color([0, 0, 255]), ShapeStroke::default());
    let doc = h.state_mut().doc.as_mut().expect("a document");
    doc.add_shape_layer("R".into(), "R", shape);
    h.run_steps(1);
    assert!(doc_has_shape(&h));
    h
}

fn doc_has_shape(h: &Harness<'static, AtelierApp>) -> bool {
    doc(h).active_layer().shape.is_some()
}

#[test]
fn filling_or_clearing_a_shape_layer_asks_to_rasterize_it() {
    for action in [Action::FillPrimary, Action::ClearSelection] {
        let mut h = shape_app();
        let n = steps(&h);
        h.state_mut().selection = Some(
            PixelRect {
                x: 40,
                y: 40,
                w: 20,
                h: 20,
            }
            .into(),
        );
        h.run_steps(1);
        h.state_mut().run(action);
        h.run_steps(2);
        assert!(doc_has_shape(&h), "{action:?}: not rasterized silently");
        assert!(
            h.state().paint.rasterize_confirm.is_some(),
            "{action:?}: asked"
        );
        assert_eq!(steps(&h), n);
        // OK: rasterized, then filled or cleared.
        key(&mut h, Key::Enter, Modifiers::NONE);
        h.run_steps(1);
        assert!(!doc_has_shape(&h));
        assert_eq!(steps(&h), n + 2, "{action:?}: Rasterize, then the command");
        let px = doc(&h).active_layer().pixel_at(50, 50).unwrap_or([0; 4]);
        match action {
            Action::FillPrimary => assert_eq!(px, [0, 0, 0, 255]),
            _ => assert_eq!(px[3], 0),
        }
    }
    // Cancel keeps the shape, and runs nothing.
    let mut h = shape_app();
    let n = steps(&h);
    h.state_mut().run(Action::FillPrimary);
    h.run_steps(2);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(doc_has_shape(&h));
    assert_eq!(steps(&h), n);
    assert!(h.state().paint.rasterize_then.is_none());
    // Without a selection, Delete asks nothing (it would clear nothing).
    h.state_mut().run(Action::ClearSelection);
    h.run_steps(1);
    assert!(h.state().paint.rasterize_confirm.is_none());
}

// ------------------------------------------------------------- locks

#[test]
fn delete_removes_locked_layers_too() {
    let mut h = two_layers_selected();
    h.state_mut().doc.as_mut().unwrap().layers[2].locked = true;
    h.run_steps(1);
    h.state_mut().run(Action::Delete);
    h.run_steps(1);
    assert_eq!(doc(&h).layers.len(), 1, "both selected layers went");
    let mut h = two_layers_selected();
    h.state_mut().doc.as_mut().unwrap().layers[1].locked = true;
    h.run_steps(1);
    h.state_mut().run(Action::Delete);
    h.run_steps(1);
    assert_eq!(doc(&h).layers.len(), 1);
}

#[test]
fn a_locked_layer_refuses_a_move_drag_and_says_why() {
    let mut h = logo_app();
    h.state_mut().doc.as_mut().unwrap().layers[1].locked = true;
    h.run_steps(1);
    let n = steps(&h);
    drag(
        &mut h,
        &[(150.0, 125.0), (170.0, 125.0), (190.0, 125.0)],
        Modifiers::NONE,
    );
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
    assert_eq!(steps(&h), n);
    let locked = t!("status-layer-is-locked", name = "Logo");
    assert!(h.state().status.contains(&locked), "{}", h.state().status);
    // Shown like the other refusals.
    assert!(h.state().workspace.error_status.is_some());
}

// ------------------------------------------------------------- Move tool

#[test]
fn alt_arrow_copies_in_one_step() {
    let mut h = logo_app();
    let n = steps(&h);
    key(&mut h, Key::ArrowRight, Modifiers::ALT);
    assert_eq!(steps(&h), n + 1, "the copy and its move");
    assert_eq!(doc(&h).layers.len(), 3);
    assert_eq!(doc(&h).active_layer().offset, (101, 100));
    assert_eq!(
        doc(&h).history().0.last().map(|s| s.to_string()),
        Some(t!("menu-layer-duplicate"))
    );
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(doc(&h).layers.len(), 2, "one undo takes the copy away");
}

#[test]
fn alt_copies_leave_locked_layers_alone() {
    let mut h = two_layers_selected();
    h.state_mut().doc.as_mut().unwrap().layers[1].locked = true;
    h.run_steps(1);
    drag(
        &mut h,
        &[(230.0, 70.0), (230.0, 120.0), (230.0, 170.0)],
        Modifiers::ALT,
    );
    let names: Vec<&str> = doc(&h).layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names.len(), 4, "{names:?}: no copy of the locked A");
    assert!(!names.contains(&"A copy"));
    let mut h = two_layers_selected();
    h.state_mut().doc.as_mut().unwrap().layers[1].locked = true;
    h.run_steps(1);
    key(&mut h, Key::ArrowDown, Modifiers::ALT);
    let names: Vec<&str> = doc(&h).layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names.len(), 4, "{names:?}");
}

#[test]
fn a_move_back_to_its_start_records_nothing() {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    let n = steps(&h);
    drag(
        &mut h,
        &[(150.0, 125.0), (170.0, 125.0), (150.0, 125.0)],
        Modifiers::NONE,
    );
    assert_eq!(steps(&h), n);
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
    // An Alt+drag back: no copy either.
    drag(
        &mut h,
        &[(150.0, 125.0), (170.0, 125.0), (150.0, 125.0)],
        Modifiers::ALT,
    );
    assert_eq!(steps(&h), n);
    assert_eq!(doc(&h).layers.len(), 2);
}

#[test]
fn the_selection_frame_is_the_box_of_the_pixels_lifted() {
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    let circle = RgbaImage::from_fn(200, 200, |x, y| {
        let (dx, dy) = (x as f32 + 0.5 - 100.0, y as f32 + 0.5 - 100.0);
        if dx * dx + dy * dy <= 10000.0 {
            BLUE
        } else {
            Rgba([0, 0, 0, 0])
        }
    });
    d.add_layer("Circle".into(), circle, (100, 50));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.state_mut().show_transform_controls = true;
    h.state_mut().selection = Some(
        PixelRect {
            x: 100,
            y: 50,
            w: 40,
            h: 40,
        }
        .into(),
    );
    h.run_steps(1);
    let frame = h.state().selection_frame().expect("a frame");
    assert!(frame.min.x > 100.0 && frame.min.y > 50.0, "{frame:?}");
    assert_eq!(frame.max, pos2(140.0, 90.0));
    // Grabbed by its corner (just outside, the frame being small), the
    // corner follows the pointer.
    let corner = frame.min;
    let grab = corner - vec2(3.0, 3.0);
    drag(
        &mut h,
        &[
            (grab.x, grab.y),
            (grab.x - 5.0, grab.y - 5.0),
            (grab.x - 10.0, grab.y - 10.0),
        ],
        Modifiers::NONE,
    );
    let r = h.state().transform_rect.expect("transforming");
    assert!((r.min.x - (corner.x - 10.0)).abs() < 1.5, "{r:?}");
    assert!((r.min.y - (corner.y - 10.0)).abs() < 1.5, "{r:?}");
}

// ------------------------------------------------------------- Free Transform

#[test]
fn redo_in_free_transform_redoes_its_edit() {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    key(&mut h, Key::T, Modifiers::COMMAND);
    drag(
        &mut h,
        &[(150.0, 125.0), (165.0, 125.0), (180.0, 125.0)],
        Modifiers::NONE,
    );
    drag(
        &mut h,
        &[(180.0, 125.0), (180.0, 140.0), (180.0, 155.0)],
        Modifiers::NONE,
    );
    let two = h.state().transform_rect;
    key(&mut h, Key::Z, Modifiers::COMMAND);
    let one = h.state().transform_rect;
    assert_ne!(one, two);
    key(&mut h, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(h.state().transform_pending());
    assert_eq!(h.state().transform_rect, two);
    // Nothing more to redo: nothing happens.
    key(&mut h, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.state().transform_rect, two);
    // A document step undone before stays undone.
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    drag(
        &mut h,
        &[(150.0, 125.0), (160.0, 125.0), (170.0, 125.0)],
        Modifiers::NONE,
    );
    key(&mut h, Key::Z, Modifiers::COMMAND);
    key(&mut h, Key::T, Modifiers::COMMAND);
    drag(
        &mut h,
        &[(150.0, 125.0), (160.0, 135.0), (170.0, 145.0)],
        Modifiers::NONE,
    );
    let moved = h.state().transform_rect;
    key(&mut h, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(h.state().transform_pending());
    assert_eq!(h.state().transform_rect, moved);
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
}

/// Logo with a work path selected, with Path Selection.
fn path_selected() -> Harness<'static, AtelierApp> {
    use crate::vector::{SubPath, pt};
    let mut h = logo_app();
    let d = h.state_mut().doc.as_mut().unwrap();
    let mut p = crate::vector::Path::default();
    p.subpaths.push(SubPath::polygon([
        pt(20.0, 20.0),
        pt(80.0, 20.0),
        pt(80.0, 60.0),
        pt(20.0, 60.0),
    ]));
    d.work_path = Some(p);
    h.state_mut().vector.selected = Some(crate::document::PathRef::Work);
    h.state_mut().set_tool(Tool::PathSelect);
    h.run_steps(1);
    h
}

#[test]
fn edit_transform_with_a_path_selected_leaves_the_layer() {
    use super::transform::{Command, Mode};
    for cmd in [Command::FlipHorizontal, Command::Mode(Mode::Scale)] {
        let mut h = path_selected();
        let n = steps(&h);
        h.state_mut().transform_command(cmd);
        h.run_steps(2);
        assert_eq!(steps(&h), n);
        assert!(!h.state().transform_pending());
        assert_eq!(h.state().status, t!("tools5-status-path-transform"));
    }
}

// ------------------------------------------------------------- Crop

/// Logo with the Crop tool and a box drawn.
fn crop_box() -> Harness<'static, AtelierApp> {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    h.state_mut().set_tool(Tool::Canvas);
    h.run_steps(1);
    drag(
        &mut h,
        &[(50.0, 50.0), (100.0, 85.0), (150.0, 120.0)],
        Modifiers::NONE,
    );
    assert!(h.state().crop_box_pending());
    h
}

#[test]
fn leaving_a_crop_box_asks_first() {
    // Crop: cropped, then the tool.
    let mut h = crop_box();
    key(&mut h, Key::B, Modifiers::NONE);
    assert_eq!(h.state().tool, Tool::Canvas, "asked first");
    assert_eq!(doc(&h).width(), 400);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(h.state().tool, Tool::Brush);
    assert_eq!((doc(&h).width(), doc(&h).height()), (100, 70));
    // Don't Crop: the box goes, the tool changes.
    let mut h = crop_box();
    key(&mut h, Key::B, Modifiers::NONE);
    // (Its button, as the window's would.)
    assert!(h.state().crop_confirm.is_some());
    h.state_mut().crop_answered(false);
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::Brush);
    assert_eq!(doc(&h).width(), 400);
    assert!(h.state().canvas_rect.is_none());
    // Cancel: nothing changes.
    let mut h = crop_box();
    let drawn = h.state().canvas_rect;
    key(&mut h, Key::B, Modifiers::NONE);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(h.state().tool, Tool::Canvas);
    assert_eq!(h.state().canvas_rect, drawn);
    // Saving asks too.
    let mut h = crop_box();
    h.state_mut().run(Action::Save);
    h.run_steps(1);
    assert!(h.state().crop_confirm.is_some());
    assert_eq!(doc(&h).width(), 400);
}

#[test]
fn crop_box_gestures() {
    // A slip of a few pixels draws no box.
    let mut h = logo_app();
    h.state_mut().set_tool(Tool::Canvas);
    h.run_steps(1);
    drag(
        &mut h,
        &[(200.0, 150.0), (202.0, 152.0), (205.0, 155.0)],
        Modifiers::NONE,
    );
    assert!(!h.state().crop_box_pending());
    // Alt: from the center.
    drag(
        &mut h,
        &[(200.0, 150.0), (220.0, 160.0), (250.0, 180.0)],
        Modifiers::ALT,
    );
    assert_eq!(
        h.state().canvas_rect,
        Some(Rect::from_min_max(pos2(150.0, 120.0), pos2(250.0, 180.0)))
    );
    // Snapped to the canvas edge.
    h.state_mut().rulers.settings.snap = true;
    drag(
        &mut h,
        &[(20.0, 20.0), (50.0, 50.0), (396.0, 150.0)],
        Modifiers::NONE,
    );
    assert_eq!(h.state().canvas_rect.map(|r| r.max.x), Some(400.0));
    // A double click inside applies it.
    let p = screen(&h, 200.0, 100.0);
    h.hover_at(p);
    h.run_steps(1);
    for _ in 0..2 {
        button(&mut h, p, true, Modifiers::NONE);
        button(&mut h, p, false, Modifiers::NONE);
    }
    h.run_steps(2);
    assert!(h.state().canvas_rect.is_none());
    assert_eq!((doc(&h).width(), doc(&h).height()), (380, 130));
}

#[test]
fn redo_with_a_crop_box_keeps_it() {
    let mut h = crop_box();
    let drawn = h.state().canvas_rect;
    key(&mut h, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert_eq!(h.state().canvas_rect, drawn);
}

#[test]
fn copies_are_numbered_not_copy_copy() {
    let mut h = logo_app();
    h.state_mut().run(Action::Duplicate);
    h.run_steps(1);
    assert_eq!(
        doc(&h).active_layer().name,
        t!("doc-layer-copy-name", name = "Logo")
    );
    h.state_mut().run(Action::Duplicate);
    h.run_steps(1);
    assert_eq!(
        doc(&h).active_layer().name,
        t!("tools5-layer-copy-numbered", name = "Logo", number = 2)
    );
}

#[test]
fn several_copies_are_one_duplicate_layers_step() {
    let mut h = two_layers_selected();
    let n = steps(&h);
    drag(
        &mut h,
        &[(230.0, 70.0), (230.0, 120.0), (230.0, 170.0)],
        Modifiers::ALT,
    );
    assert_eq!(steps(&h), n + 1);
    assert_eq!(
        doc(&h).history().0.last().map(|s| s.to_string()),
        Some(t!("tools5-hist-duplicate-layers"))
    );
}

#[test]
fn a_hidden_layer_is_not_moved() {
    let mut h = logo_app();
    h.state_mut().doc.as_mut().unwrap().layers[1].visible = false;
    h.run_steps(1);
    drag(
        &mut h,
        &[(150.0, 125.0), (170.0, 125.0), (190.0, 125.0)],
        Modifiers::NONE,
    );
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
    assert_eq!(h.state().status, t!("fx4-layer-hidden", name = "Logo"));
}

#[test]
fn undo_after_a_warp_applied_by_a_mode_switch_brings_the_grid_back() {
    use super::transform::{Command, Mode};
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    key(&mut h, Key::T, Modifiers::COMMAND);
    drag(
        &mut h,
        &[(150.0, 125.0), (165.0, 125.0), (180.0, 125.0)],
        Modifiers::NONE,
    );
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.run_steps(1);
    drag(
        &mut h,
        &[(180.0, 125.0), (190.0, 140.0), (200.0, 155.0)],
        Modifiers::NONE,
    );
    let n = steps(&h);
    h.state_mut().transform_command(Command::Mode(Mode::Scale));
    h.run_steps(2);
    assert!(h.state().transform_pending());
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().transform_pending(), "still transforming");
    assert!(h.state().transform_warping(), "the grid is back");
    assert_eq!(steps(&h), n, "the warp given up");
    assert_eq!(doc(&h).active_layer().image().dimensions(), (100, 50));
    // Then the bend, then Warp itself.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().transform_warping());
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().transform_pending());
    assert!(!h.state().transform_warping());
}

#[test]
fn transform_again_with_fit_canvas_fits_what_shows() {
    use super::transform::Command;
    let mut h = logo_app();
    h.state_mut().fit_canvas = true;
    key(&mut h, Key::T, Modifiers::COMMAND);
    h.state_mut().transform_angle = 30.0;
    h.state_mut().transform_rect = h.state().transform_source();
    key(&mut h, Key::Enter, Modifiers::NONE);
    h.state_mut().transform_command(Command::Again);
    h.run_steps(2);
    let d = doc(&h);
    let b = d.active_layer().content_bounds().expect("pixels");
    assert_eq!((d.width(), d.height()), (b.w, b.h));
}

#[test]
fn a_large_scale_is_applied_on_a_worker() {
    let mut d = Document::new(RgbaImage::from_pixel(2200, 2000, RED), None);
    d.add_layer(
        "Big".into(),
        RgbaImage::from_pixel(2100, 2000, BLUE),
        (0, 0),
    );
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    let n = steps(&h);
    h.state_mut().transform_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1050.0, 1000.0)));
    key(&mut h, Key::Enter, Modifiers::NONE);
    // Under load the worker can finish within the frames `key` runs, so
    // only the result is checked, not that it is still busy.
    for _ in 0..1000 {
        if !h.state().transform_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run_steps(1);
    }
    assert!(!h.state().transform_busy());
    assert_eq!(steps(&h), n + 1);
    assert_eq!(
        doc(&h).active_layer().content_bounds().map(|b| (b.w, b.h)),
        Some((1050, 1000))
    );
}

#[test]
fn redo_while_typing_with_nothing_typed_to_redo_goes_on_typing() {
    let mut h = logo_app();
    if !has_fonts(&mut h) {
        return;
    }
    h.state_mut().rulers.settings.snap = false;
    drag(
        &mut h,
        &[(150.0, 125.0), (160.0, 125.0), (170.0, 125.0)],
        Modifiers::NONE,
    );
    key(&mut h, Key::Z, Modifiers::COMMAND);
    let logo = doc(&h).layers[1].offset;
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(1);
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.run_steps(1);
    h.event(egui::Event::Text("Hi".into()));
    h.run_steps(1);
    key(&mut h, Key::Z, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(h.state().typing_on_canvas());
    assert_eq!(doc(&h).layers[1].offset, logo, "the move stays undone");
}

fn has_fonts(h: &mut Harness<'static, AtelierApp>) -> bool {
    !h.state_mut().text_engine().families().is_empty()
}
