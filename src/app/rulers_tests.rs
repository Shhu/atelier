//! Rulers, guides, grid and snapping (View menu).

use super::*;
use crate::guides::{Guide, Orientation};
use egui_kittest::Harness;

fn app() -> Harness<'static, AtelierApp> {
    i18n::set_lang(Lang::En);
    let mut harness = Harness::builder()
        .with_size([1600.0, 1100.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(400, 300, image::Rgba([90, 120, 200, 255]));
            app.doc = Some(Document::new(img, None));
            app
        });
    harness.run_steps(3);
    harness
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn drag(h: &mut Harness<'static, AtelierApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for i in 1..=6 {
        h.hover_at(from.lerp(to, i as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

fn rulers_on(h: &mut Harness<'static, AtelierApp>) -> Rect {
    h.key_press_modifiers(Modifiers::COMMAND, Key::R);
    h.run_steps(3);
    assert!(h.state().rulers.settings.rulers, "Ctrl+R");
    h.state().rulers.canvas_area().expect("canvas area")
}

#[test]
fn ctrl_r_shows_the_rulers_beside_the_canvas() {
    let mut h = app();
    let before = h.state().rulers.canvas_area().expect("canvas area");
    let after = rulers_on(&mut h);
    assert_eq!(after.min, before.min + Vec2::splat(18.0));
    assert_eq!(after.max, before.max);
    h.key_press_modifiers(Modifiers::COMMAND, Key::R);
    h.run_steps(2);
    assert!(!h.state().rulers.settings.rulers);
}

#[test]
fn guides_are_dragged_from_the_rulers_and_back() {
    let mut h = app();
    let area = rulers_on(&mut h);
    // From the top ruler: a horizontal guide, one undo step.
    let ruler = pos2(area.center().x, area.top() - 9.0);
    let steps = doc(&h).history_step();
    let to = screen(&h, 100.0, 120.3);
    drag(&mut h, ruler, to);
    assert_eq!(doc(&h).guides, [Guide::new(Orientation::Horizontal, 120.0)]);
    assert_ne!(doc(&h).history_step(), steps);
    // From the left ruler with Alt: horizontal too.
    let left = pos2(area.left() - 9.0, area.center().y);
    h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    let to = screen(&h, 50.0, 70.0);
    drag(&mut h, left, to);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    assert_eq!(doc(&h).guides[1], Guide::new(Orientation::Horizontal, 70.0));
    h.state_mut().undo();
    assert_eq!(doc(&h).guides.len(), 1);

    // The Move tool moves it; back on the ruler it goes.
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let from = screen(&h, 300.0, 120.0);
    let to = screen(&h, 300.0, 200.0);
    drag(&mut h, from, to);
    assert_eq!(doc(&h).guides, [Guide::new(Orientation::Horizontal, 200.0)]);
    let from = screen(&h, 300.0, 200.0);
    drag(&mut h, from, ruler);
    assert!(doc(&h).guides.is_empty());
    h.state_mut().undo();
    assert_eq!(doc(&h).guides.len(), 1, "deleting is undone");
}

#[test]
fn locked_guides_stay_and_the_move_tool_moves_the_layer() {
    let mut h = app();
    h.state_mut().doc.as_mut().unwrap().guides = vec![Guide::new(Orientation::Vertical, 100.0)];
    h.state_mut().tool = Tool::Transform;
    h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::ALT), Key::Semicolon);
    h.run_steps(2);
    assert!(h.state().rulers.settings.lock_guides);
    let from = screen(&h, 100.0, 50.0);
    let to = screen(&h, 150.0, 50.0);
    drag(&mut h, from, to);
    assert_eq!(doc(&h).guides, [Guide::new(Orientation::Vertical, 100.0)]);
}

#[test]
fn layers_snap_to_guides_and_grid_as_the_view_menu_says() {
    let mut h = app();
    let d = h.state_mut().doc.as_mut().unwrap();
    d.guides = vec![Guide::new(Orientation::Vertical, 123.0)];
    let app = h.state_mut();
    let targets =
        |app: &AtelierApp| snap_targets(app.doc.as_ref().unwrap(), &[], &app.rulers.settings);
    assert!(targets(app).xs.contains(&123.0));
    // Hidden guides, or Snap To › Guides off: no snapping to them.
    app.run(Action::View(rulers::Command::Guides));
    assert!(!targets(app).xs.contains(&123.0));
    app.run(Action::View(rulers::Command::Guides));
    app.run(Action::View(rulers::Command::SnapTo(
        rulers::SnapTo::Guides,
    )));
    assert!(!targets(app).xs.contains(&123.0));
    // Grid shown: its lines, subdivisions included.
    app.run(Action::View(rulers::Command::Grid));
    assert!(targets(app).xs.contains(&75.0));
    // View › Snap off: nothing.
    app.run(Action::View(rulers::Command::Snap));
    let t = targets(app);
    assert!(t.xs.is_empty() && t.ys.is_empty());
}

#[test]
fn new_guide_and_clear_guides() {
    let mut h = app();
    h.state_mut().run(Action::View(rulers::Command::NewGuide));
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert_eq!(doc(&h).guides, [Guide::new(Orientation::Horizontal, 0.0)]);
    h.state_mut()
        .run(Action::View(rulers::Command::ClearGuides));
    assert!(doc(&h).guides.is_empty());
    h.state_mut().undo();
    assert_eq!(doc(&h).guides.len(), 1);
}

#[test]
fn guides_follow_the_canvas() {
    let mut d = Document::new(image::RgbaImage::new(100, 50), None);
    d.guides = vec![Guide::new(Orientation::Vertical, 10.0)];
    // 20 px added on the left.
    d.resize_canvas(-20, 0, 120, 50);
    assert_eq!(d.guides[0].position, 30.0);
    d.flip_canvas(true);
    assert_eq!(d.guides[0].position, 90.0);
    d.undo();
    d.undo();
    assert_eq!(d.guides[0].position, 10.0);
}
