//! Marquees, lassos and the Select menu driven through the real app
//! (pointer and keys), with the geometry they select.

use super::select_menu::{Command, ModifyKind};
use super::select_tools::MarqueeStyle;
use super::*;
use egui_kittest::Harness;

const RED: image::Rgba<u8> = image::Rgba([200, 0, 0, 255]);
const BLUE: image::Rgba<u8> = image::Rgba([0, 0, 200, 255]);

/// 200 × 200: red on the left half, blue on the right.
fn halves() -> Document {
    let image = image::RgbaImage::from_fn(200, 200, |x, _| if x < 100 { RED } else { BLUE });
    Document::new(image, None)
}

fn app_with(doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut harness = Harness::builder()
        .with_size([1400.0, 1000.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    harness.run_steps(3);
    harness
}

fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn modifiers(h: &mut Harness<'_, AtelierApp>, m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    h.run_steps(1);
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

fn press(h: &mut Harness<'_, AtelierApp>, at: Pos2, m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    h.hover_at(at);
    h.run_steps(1);
    button(h, at, true, m);
}

/// Pointer moved in a few steps.
fn move_to(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2) {
    for i in 1..=4 {
        h.hover_at(from.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
}

fn click(h: &mut Harness<'_, AtelierApp>, at: Pos2, m: Modifiers) {
    press(h, at, m);
    button(h, at, false, m);
    h.run_steps(1);
}

fn drag(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2, m: Modifiers) {
    press(h, from, m);
    move_to(h, from, to);
    button(h, to, false, m);
    modifiers(h, Modifiers::NONE);
}

fn key(h: &mut Harness<'_, AtelierApp>, key: Key, m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: m,
        });
    }
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn selection(h: &Harness<'_, AtelierApp>) -> Selection {
    h.state().selection.clone().expect("a selection")
}

/// Runs frames until the selection computed in the background is there.
fn settle(h: &mut Harness<'_, AtelierApp>) {
    let start = std::time::Instant::now();
    while h.state().select_busy() {
        assert!(start.elapsed().as_secs() < 30, "selection never computed");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    h.run_steps(1);
}

#[test]
fn m_and_l_cycle_through_their_groups() {
    let mut h = app_with(halves());
    key(&mut h, Key::M, Modifiers::NONE);
    assert!(h.state().tool == Tool::Select);
    key(&mut h, Key::M, Modifiers::SHIFT);
    assert!(h.state().tool == Tool::Ellipse);
    key(&mut h, Key::M, Modifiers::NONE);
    assert!(h.state().tool == Tool::Ellipse, "the last one of the group");
    key(&mut h, Key::L, Modifiers::NONE);
    assert!(h.state().tool == Tool::FreeLasso);
    for next in [Tool::PolyLasso, Tool::Lasso, Tool::FreeLasso] {
        key(&mut h, Key::L, Modifiers::SHIFT);
        assert!(h.state().tool == next);
    }
}

#[test]
fn elliptical_marquee_selects_an_ellipse() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Ellipse;
    h.run_steps(1);
    let (from, to) = (screen(&h, 20.0, 20.0), screen(&h, 120.0, 80.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let sel = selection(&h);
    assert_eq!(sel.rect, rect(20, 20, 100, 60));
    assert_eq!((sel.coverage(70, 50), sel.coverage(21, 21)), (255, 0));
    let edge = sel.coverage(20, 50);
    assert!(edge > 0 && edge < 255, "anti-aliased: {edge}");
    // Without anti-aliasing, Alt and Shift held once drawing: a hard circle
    // from its center.
    h.state_mut().sel.shape.edges(Tool::Ellipse).antialias = false;
    h.state_mut().run(Action::Deselect);
    let center = screen(&h, 150.0, 150.0);
    press(&mut h, center, Modifiers::NONE);
    modifiers(&mut h, Modifiers::ALT | Modifiers::SHIFT);
    let corner = screen(&h, 170.0, 160.0);
    move_to(&mut h, center, corner);
    button(&mut h, corner, false, Modifiers::ALT | Modifiers::SHIFT);
    modifiers(&mut h, Modifiers::NONE);
    let sel = selection(&h);
    assert_eq!(sel.rect, rect(130, 130, 40, 40));
    let mask = sel.to_mask(sel.rect);
    assert!(mask.pixels().all(|p| p.0[0] == 0 || p.0[0] == 255));
}

#[test]
fn marquee_feather_ratio_and_fixed_size() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Select;
    h.state_mut().sel.shape.edges(Tool::Select).feather = 4.0;
    h.run_steps(1);
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 150.0, 150.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let sel = selection(&h);
    assert!(
        (140..=165).contains(&sel.coverage(50, 100)),
        "{}",
        sel.coverage(50, 100)
    );
    assert_eq!(sel.coverage(100, 100), 255);
    assert!(sel.coverage(42, 100) < 20);

    let opts = &mut h.state_mut().sel.shape;
    opts.edges(Tool::Select).feather = 0.0;
    (opts.style, opts.ratio) = (MarqueeStyle::FixedRatio, (2.0, 1.0));
    // The options bar may wrap: the canvas moves.
    h.run_steps(2);
    let (from, to) = (screen(&h, 10.0, 10.0), screen(&h, 40.0, 70.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(selection(&h), rect(10, 10, 120, 60).into());

    let opts = &mut h.state_mut().sel.shape;
    (opts.style, opts.size) = (MarqueeStyle::FixedSize, (30, 20));
    h.run_steps(2);
    let at = screen(&h, 150.0, 120.0);
    click(&mut h, at, Modifiers::NONE);
    assert_eq!(selection(&h), rect(150, 120, 30, 20).into());
}

#[test]
fn lasso_follows_the_drag_and_alt_adds_straight_segments() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::FreeLasso;
    h.run_steps(1);
    let (a, b, c) = (
        screen(&h, 20.0, 20.0),
        screen(&h, 120.0, 20.0),
        screen(&h, 20.0, 120.0),
    );
    press(&mut h, a, Modifiers::NONE);
    move_to(&mut h, a, b);
    move_to(&mut h, b, c);
    button(&mut h, c, false, Modifiers::NONE);
    let sel = selection(&h);
    assert!(sel.contains(40.0, 40.0) && !sel.contains(100.0, 100.0));
    assert!(h.state().sel.free_lasso.is_none());

    // Alt held at the release: straight segments by clicks, closed when
    // Alt is let go.
    let (d, e) = (screen(&h, 150.0, 150.0), screen(&h, 190.0, 150.0));
    let f = screen(&h, 190.0, 190.0);
    press(&mut h, d, Modifiers::NONE);
    move_to(&mut h, d, e);
    modifiers(&mut h, Modifiers::ALT);
    button(&mut h, e, false, Modifiers::ALT);
    assert!(h.state().sel.free_lasso.is_some(), "still open");
    h.hover_at(f);
    h.run_steps(1);
    button(&mut h, f, true, Modifiers::ALT);
    button(&mut h, f, false, Modifiers::ALT);
    modifiers(&mut h, Modifiers::NONE);
    h.run_steps(1);
    assert!(h.state().sel.free_lasso.is_none(), "closed");
    let sel = selection(&h);
    assert!(sel.contains(182.0, 160.0) && !sel.contains(160.0, 180.0));
}

#[test]
fn polygonal_lasso_points_backspace_and_closing() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::PolyLasso;
    h.state_mut().selection = Some(rect(150, 20, 30, 30).into());
    h.run_steps(1);
    // Shift at the first click: added to the selection.
    let first = screen(&h, 20.0, 20.0);
    click(&mut h, first, Modifiers::SHIFT);
    for (x, y) in [(120.0, 20.0), (90.0, 90.0)] {
        let at = screen(&h, x, y);
        click(&mut h, at, Modifiers::NONE);
    }
    // The last point goes away (and the selected pixels stay).
    key(&mut h, Key::Backspace, Modifiers::NONE);
    let last = screen(&h, 20.0, 120.0);
    click(&mut h, last, Modifiers::NONE);
    key(&mut h, Key::Enter, Modifiers::NONE);
    let sel = selection(&h);
    assert!(sel.contains(70.0, 30.0) && sel.contains(40.0, 60.0));
    assert!(!sel.contains(80.0, 80.0) && sel.contains(160.0, 30.0));
    assert_eq!(
        h.state().doc.as_ref().unwrap().layers[0].pixel_at(160, 30),
        Some(BLUE.0)
    );
    // A click on the first point closes it too.
    for (x, y) in [
        (150.0, 150.0),
        (190.0, 150.0),
        (190.0, 190.0),
        (150.0, 150.0),
    ] {
        let at = screen(&h, x, y);
        click(&mut h, at, Modifiers::NONE);
    }
    assert!(h.state().sel.free_lasso.is_none());
    assert!(selection(&h).contains(185.0, 160.0));
}

#[test]
fn modify_windows_change_the_selection() {
    let mut h = app_with(halves());
    h.state_mut().selection = Some(rect(50, 50, 100, 100).into());
    h.state_mut()
        .run(Action::Select(Command::Modify(ModifyKind::Expand)));
    h.run_steps(1);
    h.state_mut()
        .sel
        .modify
        .as_mut()
        .expect("window open")
        .amount = 10.0;
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().sel.modify.is_none());
    let sel = selection(&h);
    assert_eq!(sel.rect, rect(40, 40, 120, 120));
    assert_eq!(sel.coverage(40, 100), 255);

    // Shift+F6: Feather, remembering its amount.
    key(&mut h, Key::F6, Modifiers::SHIFT);
    assert!(h.state().sel.modify.is_some(), "Feather opened");
    h.state_mut().sel.modify.as_mut().unwrap().amount = 6.0;
    key(&mut h, Key::Enter, Modifiers::NONE);
    let sel = selection(&h);
    assert!(sel.mask.is_some() && sel.coverage(100, 100) == 255);
    let edge = sel.coverage(40, 100);
    assert!(edge > 100 && edge < 160, "{edge}");

    // Contracting everything away leaves nothing, and says so.
    h.state_mut().selection = Some(rect(10, 10, 6, 6).into());
    h.state_mut()
        .run(Action::Select(Command::Modify(ModifyKind::Contract)));
    h.run_steps(1);
    h.state_mut().sel.modify.as_mut().unwrap().amount = 5.0;
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().selection.is_none());
    assert_eq!(h.state().status, t!("sel-status-nothing-selected"));
    // Esc cancels.
    h.state_mut().selection = Some(rect(10, 10, 60, 60).into());
    h.state_mut()
        .run(Action::Select(Command::Modify(ModifyKind::Border)));
    h.run_steps(1);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().sel.modify.is_none());
    assert_eq!(selection(&h), rect(10, 10, 60, 60).into());
}

#[test]
fn grow_and_similar_use_the_wand_tolerance() {
    // Red left, blue middle, red again on the right.
    let image = image::RgbaImage::from_fn(300, 100, |x, _| match x {
        0..100 | 200..300 => RED,
        _ => BLUE,
    });
    let mut h = app_with(Document::new(image, None));
    h.state_mut().selection = Some(rect(10, 10, 10, 10).into());
    h.state_mut().run(Action::Select(Command::Grow));
    let sel = selection(&h);
    assert!(sel.contains(90.0, 90.0) && !sel.contains(150.0, 50.0) && !sel.contains(250.0, 50.0));
    h.state_mut().run(Action::Select(Command::Similar));
    let sel = selection(&h);
    assert!(sel.contains(250.0, 50.0) && !sel.contains(150.0, 50.0));
}

#[test]
fn color_range_samples_clicks_within_the_selection() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Brush;
    h.state_mut().run(Action::Select(Command::ColorRange));
    h.run_steps(2);
    // A click on the blue half samples it (and doesn't paint).
    let blue = screen(&h, 150.0, 50.0);
    click(&mut h, blue, Modifiers::NONE);
    assert_eq!(
        h.state().doc.as_ref().unwrap().layers[0].pixel_at(150, 50),
        Some(BLUE.0)
    );
    key(&mut h, Key::Enter, Modifiers::NONE);
    settle(&mut h);
    assert!(h.state().sel.color_range.is_none());
    assert_eq!(selection(&h), rect(100, 0, 100, 200).into());
    // Within a selection, inverted: the red part of it.
    h.state_mut().selection = Some(rect(50, 50, 100, 100).into());
    h.state_mut().run(Action::Select(Command::ColorRange));
    h.run_steps(2);
    let blue = screen(&h, 150.0, 50.0);
    click(&mut h, blue, Modifiers::NONE);
    h.state_mut().sel.color_range.as_mut().unwrap().invert = true;
    key(&mut h, Key::Enter, Modifiers::NONE);
    settle(&mut h);
    assert_eq!(selection(&h), rect(50, 50, 50, 100).into());
}

#[test]
fn select_and_mask_outputs_a_new_layer_with_a_mask() {
    let mut h = app_with(halves());
    h.state_mut().selection = Some(rect(50, 50, 100, 100).into());
    key(&mut h, Key::R, Modifiers::COMMAND | Modifiers::ALT);
    let sm = h.state_mut().sel.select_mask.as_mut().expect("open");
    sm.refine.feather = 4.0;
    sm.output = select_mask::Output::NewLayerWithMask;
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::NONE);
    settle(&mut h);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.layers.len(), 2);
    assert!(!d.layers[0].visible, "the original is hidden");
    let mask = d.layers[1].mask.as_ref().expect("a layer mask");
    let at = |x, y| mask.image().get_pixel(x, y).0[0];
    assert_eq!((at(100, 100), at(10, 10)), (255, 0));
    assert!(at(50, 100) > 100 && at(50, 100) < 160);
    assert!(h.state().selection.is_none());
    // One undo step.
    h.state_mut().undo();
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.layers.len(), 1);
    assert!(d.layers[0].visible);
}

#[test]
fn saved_selections_load_back_and_undo() {
    let mut h = app_with(halves());
    let soft = rect(20, 20, 60, 40).into();
    h.state_mut().selection = Some(soft);
    h.state_mut().run(Action::Select(Command::Save));
    h.run_steps(1);
    key(&mut h, Key::Enter, Modifiers::NONE);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.channels.len(), 1);
    assert_eq!(d.channels[0].name, t!("sel-channel-default-name", n = 1));
    h.state_mut().run(Action::Deselect);
    h.state_mut().run(Action::Select(Command::Load));
    h.run_steps(1);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(selection(&h), rect(20, 20, 60, 40).into());
    // The channel follows a crop of the canvas.
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.resize_canvas(10, 10, 100, 100);
    assert_eq!(doc.channels[0].mask.dimensions(), (100, 100));
    assert_eq!(doc.channels[0].mask.get_pixel(10, 10).0[0], 255);
    doc.undo();
    doc.undo();
    assert!(doc.channels.is_empty());
}

#[test]
fn transform_selection_moves_scales_and_applies() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Brush;
    h.state_mut().selection = Some(rect(50, 50, 40, 40).into());
    h.state_mut().run(Action::Select(Command::Transform));
    h.run_steps(1);
    // Inside: moves, without painting.
    let (from, to) = (screen(&h, 70.0, 70.0), screen(&h, 90.0, 80.0));
    drag(&mut h, from, to, Modifiers::NONE);
    // The bottom right corner: scales, keeping the proportions.
    let (from, to) = (screen(&h, 110.0, 100.0), screen(&h, 150.0, 120.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(
        h.state().doc.as_ref().unwrap().layers[0].pixel_at(70, 70),
        Some(RED.0)
    );
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().sel.transform.is_none());
    assert_eq!(selection(&h), rect(70, 60, 80, 80).into());
    // Esc gives up.
    h.state_mut().run(Action::Select(Command::Transform));
    h.run_steps(1);
    let (from, to) = (screen(&h, 100.0, 100.0), screen(&h, 120.0, 100.0));
    drag(&mut h, from, to, Modifiers::NONE);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(selection(&h), rect(70, 60, 80, 80).into());
}

#[test]
fn quick_mask_turns_painting_into_the_selection() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Brush;
    h.state_mut().brush_size = 20.0;
    h.state_mut().hardness = 1.0;
    h.state_mut().selection = Some(rect(20, 20, 160, 160).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(h.state().sel.quick_mask.is_some() && h.state().selection.is_none());
    // Black paints the mask: that part leaves the selection.
    let (from, to) = (screen(&h, 100.0, 40.0), screen(&h, 100.0, 160.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(
        d.layers[0].pixel_at(100, 100),
        Some(BLUE.0),
        "the layer isn't painted"
    );
    // Undone, then redone.
    h.state_mut().undo();
    h.state_mut().redo();
    key(&mut h, Key::Q, Modifiers::NONE);
    let sel = selection(&h);
    assert!(h.state().sel.quick_mask.is_none());
    assert_eq!((sel.coverage(100, 100), sel.coverage(50, 100)), (0, 255));
    assert_eq!(sel.coverage(10, 10), 0);
}

/// On a large image, the work is done in the background: the window
/// stays responsive and the selection comes a few frames later.
#[test]
fn large_selections_are_computed_in_the_background() {
    let mut h = app_with(Document::new(image::RgbaImage::new(3000, 2000), None));
    h.state_mut().selection = Some(rect(100, 100, 2800, 1800).into());
    h.state_mut()
        .run(Action::Select(Command::Modify(ModifyKind::Smooth)));
    h.run_steps(1);
    h.state_mut().sel.modify.as_mut().unwrap().amount = 20.0;
    key(&mut h, Key::Enter, Modifiers::NONE);
    settle(&mut h);
    let sel = selection(&h);
    assert_eq!(sel.coverage(1500, 1000), 255);
    assert!(sel.coverage(100, 100) < 128, "corner rounded");
}
