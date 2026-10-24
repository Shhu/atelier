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
    // Undone: the crop, Load Selection, Deselect, then the saving.
    for _ in 0..4 {
        doc.undo();
    }
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

fn quick_mask_on(h: &Harness<'_, AtelierApp>) -> bool {
    h.state()
        .doc
        .as_ref()
        .is_some_and(|d| d.quick_mask().is_some())
}

#[test]
fn quick_mask_turns_painting_into_the_selection() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Brush;
    h.state_mut().brush_size = 20.0;
    h.state_mut().hardness = 1.0;
    h.state_mut().selection = Some(rect(20, 20, 160, 160).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(quick_mask_on(&h) && h.state().selection.is_none());
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
    assert!(!quick_mask_on(&h));
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

/// A layer mask with a black square in its middle, targeted.
fn mask_with_hole(h: &mut Harness<'_, AtelierApp>) {
    let doc = h.state_mut().doc.as_mut().unwrap();
    assert!(doc.add_mask(crate::document::NewMask::RevealAll, None));
    let offset = doc.active_layer().offset;
    let m = doc.active_layer_mut().mask.as_mut().unwrap();
    let white = image::GrayImage::from_pixel(200, 200, image::Luma([255]));
    m.replace(white, (0, 0), offset);
    for y in 60..140 {
        for x in 60..140 {
            m.pixels_mut().put_pixel(x, y, image::Luma([0]));
        }
    }
    m.density = 0.5;
    m.feather = 3.0;
    m.enabled = false;
    m.set_linked(false, offset);
    doc.target_mask(true);
}

/// Select and Mask on a layer mask refines it as stored, in place: what
/// lies outside the canvas, its density, feather, link and enabled state
/// stay. OK without a change records nothing.
#[test]
fn select_and_mask_refines_a_layer_mask_in_place() {
    let mut h = app_with(halves());
    mask_with_hole(&mut h);
    let steps = h.state().doc.as_ref().unwrap().history().0.len();
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    assert!(h.state().sel.select_mask.is_some(), "opened on the mask");
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.history().0.len(), steps, "nothing to record");

    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    h.state_mut()
        .sel
        .select_mask
        .as_mut()
        .unwrap()
        .refine
        .feather = 6.0;
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.history().0.len(), steps + 1, "one step");
    let m = d.active_layer().mask.as_ref().unwrap();
    assert_eq!(m.default, 255, "the outside stays shown");
    assert_eq!((m.density, m.feather), (0.5, 3.0));
    assert!(!m.linked() && !m.enabled);
    let at = |x, y| m.image().get_pixel(x, y).0[0];
    assert_eq!((at(100, 100), at(10, 10)), (0, 255), "raw values refined");
    assert!(at(60, 100) > 30 && at(60, 100) < 230, "edge feathered");
    h.state_mut().undo();
    let m = h
        .state()
        .doc
        .as_ref()
        .unwrap()
        .active_layer()
        .mask
        .as_ref()
        .unwrap();
    assert_eq!(m.image().get_pixel(60, 100).0[0], 0);
}

/// Another layer chosen while Select and Mask is open: it closes, and
/// nothing lands on that layer.
#[test]
fn select_and_mask_closes_when_its_layer_changes() {
    let mut h = app_with(halves());
    mask_with_hole(&mut h);
    {
        let doc = h.state_mut().doc.as_mut().unwrap();
        doc.add_layer(
            "B".into(),
            image::RgbaImage::from_pixel(200, 200, RED),
            (0, 0),
        );
        doc.active = 0;
        doc.target_mask(true);
    }
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    assert!(h.state().sel.select_mask.is_some());
    h.state_mut().select_layer(1);
    h.run_steps(2);
    assert!(h.state().sel.select_mask.is_none());
    assert_eq!(h.state().status, t!("sel4-mask-target-changed"));
    assert!(h.state().doc.as_ref().unwrap().layers[1].mask.is_none());
}

/// A Hide All mask is refined too (from nothing); without a selection or a
/// mask, the command says what it needs.
#[test]
fn select_and_mask_opens_on_an_empty_mask_and_explains_otherwise() {
    let mut h = app_with(halves());
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    assert!(h.state().sel.select_mask.is_none());
    assert_eq!(h.state().status, t!("sel4-mask-needs-selection"));
    let doc = h.state_mut().doc.as_mut().unwrap();
    assert!(doc.add_mask(crate::document::NewMask::HideAll, None));
    h.state_mut().selection = Some(rect(10, 10, 20, 20).into());
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    assert!(h.state().sel.select_mask.is_some());
}

/// In Quick Mask Mode, the layer commands keep the layer's lock (the
/// tools and fills edit the mask, whatever the lock).
#[test]
fn quick_mask_keeps_the_lock_for_layer_commands() {
    let mut h = app_with(halves());
    {
        let doc = h.state_mut().doc.as_mut().unwrap();
        doc.add_layer(
            "B".into(),
            image::RgbaImage::from_pixel(200, 200, RED),
            (0, 0),
        );
        doc.active_layer_mut().locked = true;
    }
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(quick_mask_on(&h));
    // (Delete Layer isn't refused: a lock keeps the pixels, not the layer.)
    for action in [Action::MergeDown, Action::ClearLayerStyle] {
        h.state_mut().run(action);
        h.run_steps(1);
    }
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.layers.len(), 2, "the locked layer stays");
    // A fill still edits the mask.
    let before = d.quick_mask().unwrap().get_pixel(150, 150).0[0];
    h.state_mut().primary = egui::Color32::WHITE;
    h.state_mut().run(Action::FillPrimary);
    h.run_steps(1);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(before, 0);
    assert_eq!(d.quick_mask().unwrap().get_pixel(150, 150).0[0], 255);
}

/// A filter's preview in Quick Mask Mode shows on the red overlay, not as
/// a mask of the layer.
#[test]
fn quick_mask_previews_show_on_its_overlay() {
    let mut h = app_with(halves());
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    let key0 = h.state().quick_mask_overlay().unwrap().key;
    h.state_mut()
        .run(Action::Filter(crate::filters::FilterKind::GaussianBlur));
    let start = std::time::Instant::now();
    while h.state().preview_shown.is_none() {
        assert!(start.elapsed().as_secs() < 30, "no preview");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    let app = h.state();
    let p = app.preview_shown.as_ref().unwrap();
    assert!(p.quick_mask);
    assert_ne!(app.quick_mask_overlay().unwrap().key, key0, "previewed");
    let doc = app.doc.as_ref().unwrap();
    let mut opts = crate::compose::Options::default();
    p.options(&mut opts, doc.active_layer().offset);
    assert!(
        opts.mask.is_none() && opts.pixels.is_none(),
        "not on the layer"
    );
}

/// In Quick Mask Mode, the Move tool and Free Transform are refused: they
/// would move the layer under the mask.
#[test]
fn quick_mask_refuses_moving_and_transforming() {
    let mut h = app_with(halves());
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 150.0, 150.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.layers[0].offset, (0, 0));
    assert_eq!(d.quick_mask().unwrap().get_pixel(50, 50).0[0], 255);
    assert_eq!(
        h.state().status,
        t!("sel4-quick-mask-no-move", key = keys::key(Key::Q))
    );
    h.state_mut().status.clear();
    h.state_mut()
        .run(Action::Transform(transform::Command::Free));
    h.run_steps(1);
    assert!(h.state().transform_rect.is_none());
    assert!(!h.state().status.is_empty());
}

/// A background Magic Wand click is named after the wand, even when
/// another tool is chosen before it lands.
#[test]
fn a_background_wand_step_is_named_after_the_wand() {
    let image = image::RgbaImage::from_fn(2000, 1500, |x, _| if x < 1000 { RED } else { BLUE });
    let mut h = app_with(Document::new(image, None));
    h.state_mut().tool = Tool::MagicWand;
    h.run_steps(1);
    let at = screen(&h, 500.0, 500.0);
    click(&mut h, at, Modifiers::NONE);
    let wand = Tool::MagicWand.name();
    h.state_mut().tool = Tool::Brush;
    settle(&mut h);
    let d = h.state().doc.as_ref().unwrap();
    assert!(h.state().selection.is_some());
    assert_eq!(d.history().0.last().copied(), Some(wand.as_str()));
}

/// In Quick Mask Mode, the lock still holds for what acts on the layer
/// itself, outside `run()`'s list: Remove Background, smart commands.
#[test]
fn quick_mask_keeps_the_lock_for_remove_background_and_smart_commands() {
    let mut h = app_with(halves());
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locked = true;
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(quick_mask_on(&h));
    let locked = t!(
        "status-layer-is-locked",
        name = h.state().doc.as_ref().unwrap().active_layer().name.as_str()
    );
    h.state_mut().status.clear();
    h.state_mut().run(Action::RemoveBackground);
    h.run_steps(1);
    let app = h.state();
    assert!(app.status.contains(&locked), "{:?}", app.status);
    assert!(app.subject_job.is_none() && app.subject_waiting.is_none());
    assert!(app.model_prompt.is_none());
    h.state_mut().status.clear();
    h.state_mut()
        .run(Action::Smart(super::smart_ui::Command::ReplaceContents));
    h.run_steps(1);
    assert!(h.state().status.contains(&locked), "{:?}", h.state().status);
}

/// Select and Mask keeps a locked layer's mask as it is, and adds none:
/// OK says why and the window stays open for another output.
#[test]
fn select_and_mask_respects_the_lock() {
    let mut h = app_with(halves());
    mask_with_hole(&mut h);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locked = true;
    let steps = h.state().doc.as_ref().unwrap().history().0.len();
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    h.state_mut()
        .sel
        .select_mask
        .as_mut()
        .unwrap()
        .refine
        .feather = 6.0;
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    let app = h.state();
    assert!(app.sel.select_mask.is_some(), "still open");
    let name = app.doc.as_ref().unwrap().active_layer().name.clone();
    assert!(
        app.status
            .contains(&t!("status-layer-is-locked", name = name.as_str()))
    );
    let d = app.doc.as_ref().unwrap();
    assert_eq!(d.history().0.len(), steps);
    let m = d.active_layer().mask.as_ref().unwrap();
    assert_eq!(m.image().get_pixel(60, 100).0[0], 0, "mask unchanged");
    // Another output is fine.
    h.state_mut().sel.select_mask.as_mut().unwrap().output = super::select_mask::Output::Selection;
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    assert!(h.state().sel.select_mask.is_none());
    assert!(h.state().selection.is_some());

    // From a selection, no mask is added to a locked layer.
    let mut h = app_with(halves());
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locked = true;
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    h.state_mut().sel.select_mask.as_mut().unwrap().output = super::select_mask::Output::LayerMask;
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    assert!(
        h.state()
            .doc
            .as_ref()
            .unwrap()
            .active_layer()
            .mask
            .is_none()
    );
    assert!(h.state().sel.select_mask.is_some(), "still open");
}

/// Select and Mask on a layer mask, Output New Layer with Mask: the copy's
/// mask is refined in place, its settings kept.
#[test]
fn select_and_mask_new_layer_with_mask_keeps_the_mask_settings() {
    let mut h = app_with(halves());
    mask_with_hole(&mut h);
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    h.state_mut().sel.select_mask.as_mut().unwrap().output =
        super::select_mask::Output::NewLayerWithMask;
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.layers.len(), 2);
    let m = d.active_layer().mask.as_ref().unwrap();
    assert_eq!(m.default, 255, "the outside stays shown");
    assert_eq!((m.density, m.feather), (0.5, 3.0));
    assert!(!m.enabled);
    let at = |x, y| m.image().get_pixel(x, y).0[0];
    assert_eq!((at(100, 100), at(10, 10)), (0, 255));
}

/// Select and Mask on a mask with a density, Output Selection: what the
/// mask shows is selected (as a Ctrl+click on its thumbnail), not its
/// stored values.
#[test]
fn select_and_mask_selection_from_a_mask_follows_its_density() {
    let mut h = app_with(halves());
    mask_with_hole(&mut h);
    h.state_mut().run(Action::Select(Command::SelectAndMask));
    h.run_steps(1);
    h.state_mut().sel.select_mask.as_mut().unwrap().output = super::select_mask::Output::Selection;
    h.state_mut().close_select_mask(true);
    settle(&mut h);
    let sel = selection(&h);
    let hole = sel.coverage(100, 100);
    assert!((120..=135).contains(&hole), "about half: {hole}");
    assert_eq!(sel.coverage(10, 10), 255);
}

/// A Shift+click with the Magic Wand right after a click computed in the
/// background is chained after it, not waited for: both apply, in order,
/// as two steps.
#[test]
fn a_quick_second_wand_click_is_chained() {
    let image = image::RgbaImage::from_fn(2000, 1500, |x, y| match (x < 1000, y < 750) {
        (true, true) => RED,
        (false, false) => BLUE,
        _ => image::Rgba([0, 200, 0, 255]),
    });
    let mut h = app_with(Document::new(image, None));
    h.state_mut().tool = Tool::MagicWand;
    h.state_mut().wand_contiguous = true;
    h.run_steps(1);
    let steps = h.state().doc.as_ref().unwrap().history().0.len();
    let (a, b) = (screen(&h, 500.0, 300.0), screen(&h, 1500.0, 1200.0));
    click(&mut h, a, Modifiers::NONE);
    click(&mut h, b, Modifiers::SHIFT);
    modifiers(&mut h, Modifiers::NONE);
    settle(&mut h);
    let sel = selection(&h);
    assert_eq!(sel.coverage(500, 300), 255);
    assert_eq!(sel.coverage(1500, 1200), 255, "the second click added");
    assert_eq!(sel.coverage(1500, 300), 0);
    let d = h.state().doc.as_ref().unwrap();
    assert_eq!(d.history().0.len(), steps + 2, "one step per click");
}

/// In Quick Mask Mode, the options bar says so whatever the tool, and a
/// click there leaves it.
#[test]
fn quick_mask_notice_in_the_options_bar() {
    let mut h = app_with(halves());
    h.state_mut().tool = Tool::Brush;
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(quick_mask_on(&h));
    use egui_kittest::kittest::Queryable as _;
    let label = format!("{} {}", icon::CIRCLE_HALF, t!("sel5-options-quick-mask"));
    h.get_by_label(&label).click();
    h.run_steps(2);
    assert!(!quick_mask_on(&h));
    assert!(h.query_by_label(&label).is_none());
}

/// While a selection is computed in the background, one that combines
/// with it starts after it on a worker: the call returns at once, and
/// both apply in order.
#[test]
fn a_selection_combined_with_a_pending_one_does_not_wait() {
    let mut h = app_with(Document::new(image::RgbaImage::new(2000, 1500), None));
    let steps = h.state().doc.as_ref().unwrap().history().0.len();
    let big = 1 << 24;
    let app = h.state_mut();
    app.select_later(
        super::select_menu::Then::Select(SelectMode::Replace),
        big,
        move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            Some(rect(0, 0, 100, 100).into())
        },
    );
    let start = std::time::Instant::now();
    app.select_later(
        super::select_menu::Then::Select(SelectMode::Add),
        big,
        move || Some(rect(500, 500, 100, 100).into()),
    );
    assert!(
        start.elapsed().as_millis() < 300,
        "waited for the first one"
    );
    assert!(app.select_busy());
    settle(&mut h);
    let sel = selection(&h);
    assert_eq!((sel.coverage(50, 50), sel.coverage(550, 550)), (255, 255));
    assert_eq!(sel.coverage(300, 300), 0);
    assert_eq!(h.state().doc.as_ref().unwrap().history().0.len(), steps + 2);
}
