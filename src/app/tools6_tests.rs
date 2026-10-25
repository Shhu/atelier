//! Painting, retouching, Move tool, Free Transform and Crop, driven
//! through the real app: the fourth review's findings.

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

// ------------------------------------------------------------- crop

/// `logo_app` with a crop box drawn over (50, 50)–(150, 120).
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

fn size(h: &Harness<'static, AtelierApp>) -> (u32, u32) {
    (doc(h).width(), doc(h).height())
}

#[test]
fn every_command_that_would_apply_a_crop_box_asks_first() {
    for action in [
        Action::Duplicate,
        Action::NewLayer,
        Action::ImageSize,
        Action::Select(super::select_menu::Command::QuickMask),
        Action::ExportAs,
    ] {
        // Asked, nothing done yet.
        let mut h = crop_box();
        let n = steps(&h);
        h.state_mut().run(action);
        h.run_steps(1);
        assert!(h.state().crop_confirm.is_some(), "{action:?} asks");
        assert_eq!(size(&h), (400, 300), "{action:?}");
        assert_eq!(steps(&h), n, "{action:?}");
        // Crop: cropped, then the command.
        h.state_mut().crop_answered(true);
        h.run_steps(1);
        assert_eq!(size(&h), (100, 70), "{action:?}");
        assert!(h.state().crop_confirm.is_none());
    }
    // Ctrl+J then duplicates the cropped layer.
    let mut h = crop_box();
    let layers = doc(&h).layers.len();
    h.state_mut().run(Action::Duplicate);
    h.state_mut().crop_answered(false);
    h.run_steps(1);
    assert_eq!(size(&h), (400, 300), "Don't Crop");
    assert_eq!(doc(&h).layers.len(), layers + 1, "then duplicated");
}

#[test]
fn ctrl_t_after_the_crop_question_transforms() {
    let mut h = crop_box();
    h.state_mut()
        .run(Action::Transform(super::transform::Command::Free));
    h.run_steps(1);
    assert!(h.state().crop_confirm.is_some());
    h.state_mut().crop_answered(true);
    h.run_steps(1);
    assert_eq!(size(&h), (100, 70));
    assert_eq!(h.state().tool, Tool::Transform);
    assert!(h.state().free_transform, "Free Transform started");
}

#[test]
fn opening_or_adding_a_document_with_a_crop_box_asks_first() {
    let mut h = crop_box();
    let image = RgbaImage::from_pixel(30, 20, BLUE);
    h.state_mut().add_tab(Document::new(image, None));
    h.run_steps(1);
    assert_eq!(h.state().tab_count(), 1, "asked first");
    assert_eq!(size(&h), (400, 300));
    h.state_mut().crop_answered(true);
    h.run_steps(1);
    assert_eq!(h.state().tab_count(), 2);
    assert_eq!(size(&h), (30, 20), "the new one shown");
    let first = h.state().tab(0).map(|(d, _)| (d.width(), d.height()));
    assert_eq!(first, Some((100, 70)), "cropped as answered");
    // Cancel: no new tab.
    let mut h = crop_box();
    let image = RgbaImage::from_pixel(30, 20, BLUE);
    h.state_mut().add_tab(Document::new(image, None));
    h.state_mut().crop_confirm = None;
    h.run_steps(1);
    assert_eq!(h.state().tab_count(), 1);
    assert!(h.state().crop_box_pending(), "the box stays");
}

#[test]
fn the_close_button_of_another_tab_closes_it_after_the_crop_question() {
    // A second tab, behind: the crop box is on the active one.
    let image = RgbaImage::from_pixel(30, 20, BLUE);
    let mut h = crop_box();
    let first = h.state().canvas_rect;
    h.state_mut().canvas_rect = None;
    h.state_mut().add_tab(Document::new(image, None));
    h.state_mut().switch_tab(0);
    h.state_mut().canvas_rect = first;
    h.run_steps(1);
    assert!(h.state().crop_box_pending());
    assert_eq!(h.state().active_tab, 0);
    h.state_mut().close_tab(1);
    h.run_steps(1);
    assert!(h.state().crop_confirm.is_some());
    h.state_mut().crop_answered(true);
    h.run_steps(2);
    assert_eq!(h.state().tab_count(), 1, "the other tab closed");
    assert_eq!(size(&h), (100, 70), "the cropped one stays");
}

#[test]
fn picking_another_layer_keeps_the_crop_box() {
    let mut h = crop_box();
    let drawn = h.state().canvas_rect;
    h.state_mut().select_layer(0);
    h.run_steps(1);
    assert_eq!(doc(&h).active, 0);
    assert_eq!(h.state().canvas_rect, drawn);
    assert_eq!(size(&h), (400, 300));
    assert!(h.state().crop_confirm.is_none());
}

// ------------------------------------------------------------- input during jobs

/// A Healing Brush stroke healed on another thread, then at once a press:
/// `logo`-free noisy document, 150 px brush.
fn heal_in_the_background() -> (Harness<'static, AtelierApp>, usize) {
    let mut h = app_with(noisy(2400, 1600), Tool::HealingBrush);
    let app = h.state_mut();
    app.retouch.heal_aligned = true;
    app.brush_size = 150.0;
    (app.zoom, app.pan) = (0.25, Vec2::ZERO);
    h.run_steps(4);
    click(&mut h, (100.0, 1400.0), Modifiers::ALT);
    let n = steps(&h);
    let path: Vec<(f32, f32)> = (0..=6)
        .map(|i| (400.0 + 75.0 * i as f32, 300.0 + 75.0 * i as f32))
        .collect();
    drag(&mut h, &path, Modifiers::NONE);
    (h, n)
}

#[test]
fn a_stroke_made_while_the_last_one_heals_is_not_lost() {
    let (mut h, n) = heal_in_the_background();
    drag(
        &mut h,
        &[(1600.0, 400.0), (1620.0, 400.0), (1640.0, 400.0)],
        Modifiers::NONE,
    );
    // The first one was put in place at the second's press.
    assert!(!h.state().retouch.busy());
    assert_eq!(steps(&h), n + 2, "both strokes");
}

#[test]
fn a_press_held_while_the_last_stroke_heals_paints_once_dragged() {
    let (mut h, n) = heal_in_the_background();
    let start = screen(&h, 1600.0, 400.0);
    button(&mut h, start, true, Modifiers::NONE);
    assert!(!h.state().retouch.busy(), "waited for at the press");
    let before = doc(&h).active_layer().image().clone();
    for i in 1..=10 {
        let p = screen(&h, 1600.0 + 20.0 * i as f32, 400.0);
        h.hover_at(p);
        h.run_steps(1);
    }
    assert!(doc(&h).active_layer().image() != &before, "painted");
    let end = screen(&h, 1800.0, 400.0);
    button(&mut h, end, false, Modifiers::NONE);
    wait_for_fill(&mut h);
    h.run_steps(2);
    assert_eq!(steps(&h), n + 2);
}

#[test]
fn a_tool_key_or_undo_while_a_stroke_heals_applies_after_it() {
    let (mut h, n) = heal_in_the_background();
    key(&mut h, Key::B, Modifiers::NONE);
    assert_eq!(h.state().tool, Tool::Brush);
    assert_eq!(steps(&h), n + 1, "the stroke kept");
    let (mut h, n) = heal_in_the_background();
    key(&mut h, Key::Z, Modifiers::COMMAND);
    h.run_steps(2);
    assert!(!h.state().retouch.busy());
    assert_eq!(steps(&h), n, "the stroke, then undone");
}

#[test]
fn a_tool_key_while_a_transform_is_applied_aside_applies_after_it() {
    let mut d = Document::new(RgbaImage::from_pixel(2200, 2000, RED), None);
    d.add_layer("Big".into(), RgbaImage::from_pixel(2100, 2000, BLUE), (0, 0));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    let n = steps(&h);
    h.state_mut().transform_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1050.0, 1000.0)));
    // Enter, then B in the next frame.
    h.event(egui::Event::Key {
        key: Key::Enter,
        physical_key: Some(Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    key(&mut h, Key::B, Modifiers::NONE);
    assert!(!h.state().transform_busy());
    assert_eq!(h.state().tool, Tool::Brush, "the key not lost");
    assert_eq!(steps(&h), n + 1);
    assert_eq!(
        doc(&h).active_layer().content_bounds().map(|b| (b.w, b.h)),
        Some((1050, 1000))
    );
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
fn fill_dialog_on_a_shape_layer_asks_to_rasterize_it() {
    let fill = FillDialog {
        content: FillContent::Foreground,
        color: Color32::BLACK,
        opacity: 1.0,
        preserve_transparency: false,
        adapt: true,
    };
    let mut h = shape_app();
    let n = steps(&h);
    h.state_mut().apply_fill(fill);
    h.run_steps(2);
    assert!(doc_has_shape(&h), "not rasterized silently");
    assert!(h.state().paint.rasterize_confirm.is_some(), "asked");
    assert_eq!(steps(&h), n);
    // OK: rasterized, then filled.
    key(&mut h, Key::Enter, Modifiers::NONE);
    h.run_steps(1);
    assert!(!doc_has_shape(&h));
    assert_eq!(steps(&h), n + 2, "Rasterize, then Fill");
    let px = doc(&h).active_layer().pixel_at(50, 50).unwrap_or([0; 4]);
    assert_eq!(px, [0, 0, 0, 255]);
    // Cancel: nothing.
    let mut h = shape_app();
    let n = steps(&h);
    h.state_mut().apply_fill(fill);
    h.run_steps(2);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(doc_has_shape(&h));
    assert_eq!(steps(&h), n);
}

#[test]
fn make_transparent_asks_to_rasterize_only_when_applied() {
    let mut h = shape_app();
    let n = steps(&h);
    h.state_mut().run(Action::MakeTransparent);
    h.run_steps(2);
    assert!(h.state().paint.rasterize_confirm.is_none(), "not at the dialog");
    assert!(h.state().transparency.is_some());
    // Its Cancel: nothing at all.
    h.state_mut().close_transparency(false);
    h.run_steps(2);
    assert!(doc_has_shape(&h));
    assert_eq!(steps(&h), n);
    // Its OK: asked.
    h.state_mut().run(Action::MakeTransparent);
    h.run_steps(1);
    h.state_mut().close_transparency(true);
    h.run_steps(1);
    assert!(h.state().paint.rasterize_confirm.is_some());
    assert!(doc_has_shape(&h));
}

// ------------------------------------------------------------- free transform

/// Ctrl+T, Warp, a bend, then Scale: the warp applied by the switch.
fn warp_then_switch(h: &mut Harness<'static, AtelierApp>, from: (f32, f32)) {
    use super::transform::{Command, Mode};
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.run_steps(1);
    let (x, y) = from;
    drag(h, &[(x, y), (x + 10.0, y + 15.0), (x + 20.0, y + 30.0)], Modifiers::NONE);
    h.state_mut().transform_command(Command::Mode(Mode::Scale));
    h.run_steps(2);
}

#[test]
fn esc_or_ctrl_z_after_a_warp_switch_with_a_selection_gives_everything_up() {
    for undo in [false, true] {
        let mut h = logo_app();
        h.state_mut().rulers.settings.snap = false;
        let sel: Selection = PixelRect {
            x: 100,
            y: 100,
            w: 60,
            h: 50,
        }
        .into();
        h.state_mut().selection = Some(sel.clone());
        h.run_steps(1);
        let (n, before) = (steps(&h), doc(&h).active_layer().image().clone());
        key(&mut h, Key::T, Modifiers::COMMAND);
        warp_then_switch(&mut h, (130.0, 125.0));
        assert!(h.state().transform_pending());
        if undo {
            key(&mut h, Key::Z, Modifiers::COMMAND);
            assert!(h.state().transform_pending(), "still transforming");
            assert!(h.state().transform_warping(), "the grid is back");
            key(&mut h, Key::Escape, Modifiers::NONE);
        } else {
            key(&mut h, Key::Escape, Modifiers::NONE);
        }
        assert!(!h.state().transform_pending());
        assert_eq!(steps(&h), n, "undo {undo}: no step left");
        assert!(doc(&h).active_layer().image() == &before, "undo {undo}");
        assert_eq!(doc(&h).layers.len(), 2, "no floating layer");
        assert_eq!(
            h.state().selection.as_ref().map(|s| s.rect),
            Some(sel.rect),
            "undo {undo}: the selection back"
        );
    }
}

#[test]
fn each_warp_switch_is_a_ctrl_z_point() {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    key(&mut h, Key::T, Modifiers::COMMAND);
    warp_then_switch(&mut h, (150.0, 125.0));
    let d1 = doc(&h).active_layer().image().dimensions();
    let c = h.state().transform_source().expect("a frame").center();
    warp_then_switch(&mut h, (c.x, c.y));
    assert_ne!(doc(&h).active_layer().image().dimensions(), d1);
    // Back to the second grid, the first warp kept.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().transform_pending(), "still transforming");
    assert!(h.state().transform_warping());
    assert_eq!(doc(&h).active_layer().image().dimensions(), d1);
    // Its bend, its Warp, then back before the first warp.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().transform_pending());
    assert!(h.state().transform_warping());
    assert_eq!(doc(&h).active_layer().image().dimensions(), (100, 50));
}

#[test]
fn path_selection_alt_click_leaves_no_copy() {
    use super::vector_tools::ShapeTool;
    let mut h = app_with(
        RgbaImage::from_pixel(400, 300, RED),
        Tool::Shape(ShapeTool::Rectangle),
    );
    drag(
        &mut h,
        &[(50.0, 50.0), (100.0, 80.0), (150.0, 120.0)],
        Modifiers::NONE,
    );
    h.state_mut().set_tool(Tool::PathSelect);
    h.run_steps(1);
    let parts = |h: &Harness<'static, AtelierApp>| {
        doc(h)
            .active_layer()
            .shape
            .as_ref()
            .map(|s| (s.path.subpaths.len(), s.live.is_some()))
    };
    let (before, n) = (parts(&h), steps(&h));
    click(&mut h, (100.0, 85.0), Modifiers::ALT);
    assert_eq!(parts(&h), before, "no copy, still live");
    assert_eq!(steps(&h), n);
    drag(
        &mut h,
        &[(100.0, 85.0), (120.0, 85.0), (100.0, 85.0)],
        Modifiers::ALT,
    );
    assert_eq!(parts(&h), before, "there and back: no copy");
    assert_eq!(steps(&h), n);
    // A real Alt-drag still copies.
    drag(
        &mut h,
        &[(100.0, 85.0), (120.0, 85.0), (140.0, 85.0)],
        Modifiers::ALT,
    );
    assert_eq!(parts(&h).map(|p| p.0), before.map(|p| p.0 + 1));
    assert_eq!(steps(&h), n + 1);
}

#[test]
fn a_drag_back_to_its_start_leaves_a_saved_document_unmodified() {
    for m in [Modifiers::NONE, Modifiers::ALT] {
        let mut h = logo_app();
        h.state_mut().rulers.settings.snap = false;
        h.state_mut().doc.as_mut().expect("a document").mark_saved();
        drag(
            &mut h,
            &[(150.0, 125.0), (170.0, 125.0), (150.0, 125.0)],
            m,
        );
        assert!(!doc(&h).modified, "{m:?}");
        assert_eq!(doc(&h).layers.len(), 2);
    }
}

#[test]
fn the_crop_box_snaps_when_resized_or_moved() {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = true;
    h.state_mut().set_tool(Tool::Canvas);
    h.run_steps(1);
    drag(
        &mut h,
        &[(20.0, 20.0), (150.0, 100.0), (250.0, 200.0)],
        Modifiers::NONE,
    );
    let r = h.state().canvas_rect.expect("a box");
    drag(
        &mut h,
        &[(r.max.x, r.max.y), (r.max.x + 50.0, r.max.y + 50.0), (397.0, 297.0)],
        Modifiers::NONE,
    );
    let r = h.state().canvas_rect.expect("a box");
    assert_eq!((r.max.x, r.max.y), (400.0, 300.0), "the SE corner on the canvas's");
    // Moved: its corner onto the Logo's (100, 100).
    let (dx, dy) = (103.0 - r.min.x, 102.0 - r.min.y);
    drag(
        &mut h,
        &[(200.0, 200.0), (230.0, 230.0), (200.0 + dx, 200.0 + dy)],
        Modifiers::NONE,
    );
    let r = h.state().canvas_rect.expect("a box");
    assert_eq!((r.min.x, r.min.y), (100.0, 100.0));
}

#[test]
fn a_thin_crop_box_can_be_drawn() {
    let mut h = logo_app();
    h.state_mut().rulers.settings.snap = false;
    h.state_mut().set_tool(Tool::Canvas);
    h.run_steps(1);
    drag(
        &mut h,
        &[(50.0, 50.0), (200.0, 53.0), (330.0, 56.0)],
        Modifiers::NONE,
    );
    let r = h.state().canvas_rect.map(|r| (r.width(), r.height()));
    assert_eq!(r, Some((280.0, 6.0)));
}

// ------------------------------------------------------------- move tool

#[test]
fn hidden_layers_of_a_multi_selection_do_not_move() {
    // B (active) dragged, A hidden: B alone moves.
    let mut h = two_layers_selected();
    h.state_mut().rulers.settings.snap = false;
    h.state_mut().doc.as_mut().expect("a document").layers[1].visible = false;
    h.run_steps(1);
    drag(
        &mut h,
        &[(220.0, 70.0), (240.0, 80.0), (260.0, 90.0)],
        Modifiers::NONE,
    );
    let offsets: Vec<(i64, i64)> = doc(&h).layers.iter().map(|l| l.offset).collect();
    assert_eq!(offsets[1], (50, 50), "hidden A stays");
    assert_eq!(offsets[2], (240, 70));
    // The hidden one active: refused, nothing moves.
    let mut h = two_layers_selected();
    h.state_mut().rulers.settings.snap = false;
    let d = h.state_mut().doc.as_mut().expect("a document");
    d.layers[2].visible = false;
    h.run_steps(1);
    drag(
        &mut h,
        &[(220.0, 70.0), (240.0, 80.0), (260.0, 90.0)],
        Modifiers::NONE,
    );
    let offsets: Vec<(i64, i64)> = doc(&h).layers.iter().map(|l| l.offset).collect();
    assert_eq!(offsets[1..], [(50, 50), (200, 50)]);
}

#[test]
fn ctrl_alt_drag_duplicates_whatever_the_tool() {
    let both = Modifiers::COMMAND.plus(Modifiers::ALT);
    for tool in [
        Tool::Select,
        Tool::FreeLasso,
        Tool::Eyedropper,
        Tool::Brush,
        Tool::Bucket,
        Tool::CloneStamp,
        Tool::Eraser,
    ] {
        let mut h = logo_app();
        h.state_mut().rulers.settings.snap = false;
        h.state_mut().set_tool(tool);
        h.run_steps(2);
        let primary = h.state().primary;
        drag(
            &mut h,
            &[(120.0, 120.0), (150.0, 140.0), (180.0, 160.0)],
            both,
        );
        let d = doc(&h);
        assert_eq!(d.layers.len(), 3, "{tool:?}: a copy");
        assert_eq!(d.layers[1].offset, (100, 100), "{tool:?}: the original stays");
        assert_eq!(d.layers[2].offset, (160, 140), "{tool:?}: the copy moved");
        assert_eq!(h.state().primary, primary, "{tool:?}: no color picked");
        assert_eq!(h.state().tool, tool);
    }
}

#[test]
fn align_goes_to_the_selection_when_there_is_one() {
    // Two layers and Select All: centered on the canvas.
    let mut h = two_layers_selected();
    h.state_mut().run(Action::SelectAll);
    h.state_mut().run(Action::Align(AlignOp::CenterH));
    h.run_steps(1);
    let xs: Vec<i64> = doc(&h).layers[1..].iter().map(|l| l.offset.0).collect();
    assert_eq!(xs, [170, 170]);
    // One layer, a selection over 0..100: its right edge.
    let mut h = logo_app();
    h.state_mut().selection = Some(
        PixelRect {
            x: 0,
            y: 0,
            w: 100,
            h: 300,
        }
        .into(),
    );
    h.state_mut().run(Action::Align(AlignOp::Right));
    h.run_steps(1);
    assert_eq!(doc(&h).active_layer().offset, (0, 100));
    // In the Layer menu.
    let menus = h.state().menus();
    let layer = &menus.iter().find(|(name, _)| *name == t!("menu-layer")).expect("Layer").1;
    let found = layer.iter().any(|e| {
        matches!(e, MenuEntry::Submenu(name, _) if *name == t!("tools6-menu-align-to-selection"))
    });
    assert!(found);
}

#[test]
fn arrows_moving_selected_pixels_are_one_step() {
    let mut h = logo_app();
    h.state_mut().selection = Some(
        PixelRect {
            x: 110,
            y: 110,
            w: 20,
            h: 20,
        }
        .into(),
    );
    h.run_steps(1);
    let n = steps(&h);
    for _ in 0..4 {
        key(&mut h, Key::ArrowRight, Modifiers::NONE);
    }
    assert_eq!(steps(&h), n + 1, "one step");
    assert_eq!(h.state().selection.as_ref().map(|s| s.rect.x), Some(114));
    let flat = doc(&h).flatten();
    assert_eq!(flat.get_pixel(111, 115).0, RED.0, "their place cleared");
    assert_eq!(flat.get_pixel(114, 115).0, BLUE.0, "the pixels moved");
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(steps(&h), n);
    let flat = doc(&h).flatten();
    assert_eq!(flat.get_pixel(111, 115).0, BLUE.0, "all undone at once");
}

// ------------------------------------------------------------- retouching

#[test]
fn current_and_below_follows_changes_below() {
    use super::retouch_tools::Sample;
    let mut d = Document::new(RgbaImage::from_pixel(300, 200, RED), None);
    d.add_layer("Top".into(), RgbaImage::new(300, 200), (0, 0));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::CloneStamp);
    let app = h.state_mut();
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.retouch.clone_sample = Sample::CurrentAndBelow;
    app.retouch.clone_aligned = false;
    h.run_steps(2);
    click(&mut h, (50.0, 50.0), Modifiers::ALT);
    drag(&mut h, &[(200.0, 100.0), (201.0, 100.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 200, 100), RED.0);
    // The layer below filled blue (a step): the next stroke clones blue.
    h.state_mut().doc.as_mut().expect("a document").active = 0;
    h.state_mut().primary = Color32::from_rgb(0, 0, 200);
    h.state_mut().run(Action::FillPrimary);
    h.run_steps(1);
    h.state_mut().doc.as_mut().expect("a document").active = 1;
    h.run_steps(1);
    drag(&mut h, &[(250.0, 150.0), (251.0, 150.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 250, 150), BLUE.0);
    // Both undone: red again.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    drag(&mut h, &[(100.0, 150.0), (101.0, 150.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 100, 150), RED.0);
}

// ------------------------------------------------------------- locks, hidden

/// `logo_app` with Logo in a group, Logo active.
fn logo_in_group() -> Harness<'static, AtelierApp> {
    let mut h = logo_app();
    let d = h.state_mut().doc.as_mut().expect("a document");
    assert!(d.group_layers(&[1]));
    d.active = 1;
    h.run_steps(1);
    assert!(doc(&h).layers[2].is_group());
    h
}

#[test]
fn a_locked_group_locks_its_layers() {
    let mut h = logo_in_group();
    h.state_mut().doc.as_mut().expect("a document").layers[2].locked = true;
    h.run_steps(1);
    let n = steps(&h);
    let before = doc(&h).active_layer().image().clone();
    // Fill and a filter: refused, saying so.
    h.state_mut().run(Action::FillPrimary);
    h.run_steps(1);
    assert!(doc(&h).active_layer().image() == &before);
    assert_eq!(steps(&h), n);
    let (group, name) = (doc(&h).layers[2].name.clone(), doc(&h).active_layer().name.clone());
    let says = t!("tools6-status-group-locked", name = name.as_str(), group = group.as_str());
    assert!(h.state().status.contains(&says), "{}", h.state().status);
    assert!(h.state().workspace.error_status.is_some(), "a warning");
    // The Brush: blocked before the click.
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(1);
    assert!(h.state().paint_blocked());
    drag(&mut h, &[(120.0, 120.0), (140.0, 120.0)], Modifiers::NONE);
    assert!(doc(&h).active_layer().image() == &before);
    // A Move drag: nothing moves.
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    drag(&mut h, &[(150.0, 125.0), (170.0, 125.0), (190.0, 125.0)], Modifiers::NONE);
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
    assert_eq!(steps(&h), n);
}

#[test]
fn edit_commands_on_a_layer_in_a_hidden_group_name_the_group() {
    let mut h = logo_in_group();
    h.state_mut().doc.as_mut().expect("a document").layers[2].visible = false;
    h.run_steps(1);
    h.state_mut().run(Action::FillPrimary);
    h.run_steps(1);
    let group = doc(&h).layers[2].name.clone();
    let name = doc(&h).active_layer().name.clone();
    assert_eq!(
        h.state().status,
        t!("tools6-status-group-hidden", name = name.as_str(), group = group.as_str())
    );
    assert!(h.state().workspace.error_status.is_some(), "a warning");
}

#[test]
fn hiding_several_layers_is_named_for_several() {
    let mut h = two_layers_selected();
    h.state_mut().run(Action::ToggleVisibility);
    h.run_steps(1);
    let (names, done) = doc(&h).history();
    assert_eq!(names[done - 1], t!("ui5-menu-hide-layers"));
}

#[test]
fn a_large_scale_says_where_the_element_went() {
    let mut d = Document::new(RgbaImage::from_pixel(2200, 2000, RED), None);
    d.add_layer("Big".into(), RgbaImage::from_pixel(2100, 2000, BLUE), (0, 0));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    h.state_mut().transform_rect = Some(Rect::from_min_size(pos2(10.0, 20.0), vec2(1050.0, 1000.0)));
    key(&mut h, Key::Enter, Modifiers::NONE);
    // Any key waits for it.
    key(&mut h, Key::B, Modifiers::NONE);
    assert!(!h.state().transform_busy());
    let says = t!(
        "status-element-placed",
        width = 1050,
        height = 1000,
        x = 10,
        y = 20,
        rotated = String::new()
    );
    assert_eq!(h.state().status, says);
}
