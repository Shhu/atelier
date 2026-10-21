//! Artboards driven through the real app: drawn, moved by their name,
//! duplicated with their "+" button, a layer moved from one to the other
//! with the Move tool, zoom to an artboard, File › New with artboards.

use super::*;
use crate::document::ArtboardBackground;
use egui_kittest::Harness;

const RED: image::Rgba<u8> = image::Rgba([200, 0, 0, 255]);

fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

/// "A" (200 × 150, at the origin) holding "logo" (red, 40 × 40 at 20, 20),
/// "B" (100 × 100 at 300, 0), empty; "logo" active.
fn app() -> Harness<'static, AtelierApp> {
    let mut doc = Document::with_artboard(200, 150, ArtboardBackground::White);
    doc.layers[1].name = "A".into();
    doc.active = 1;
    doc.add_layer(
        "logo".into(),
        image::RgbaImage::from_pixel(40, 40, RED),
        (20, 20),
    );
    let logo = doc.active;
    doc.add_artboard(
        "new".into(),
        "B".into(),
        rect(300, 0, 100, 100),
        ArtboardBackground::White,
    );
    doc.active = logo;
    let mut doc = Some(doc);
    let mut h = Harness::builder()
        .with_size([1400.0, 1000.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn board(h: &Harness<'static, AtelierApp>, name: &str) -> (usize, PixelRect) {
    let d = doc(h);
    let i = d
        .layers
        .iter()
        .position(|l| l.name == name)
        .expect("the artboard");
    (i, d.layers[i].artboard_rect().expect("an artboard"))
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn drag(h: &mut Harness<'static, AtelierApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(from.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

#[test]
fn the_artboard_tool_draws_an_artboard_in_one_undo_step() {
    let mut h = app();
    h.state_mut().run(Action::Tool(Tool::Artboard));
    h.run_steps(1);
    let (from, to) = (screen(&h, 220.0, 20.0), screen(&h, 290.0, 140.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.artboards().len(), 3);
    assert_eq!(
        d.layers[d.active].artboard_rect(),
        Some(rect(220, 20, 70, 120))
    );
    assert_eq!(d.full_rect(), rect(0, 0, 400, 150));
    h.state_mut().undo();
    assert_eq!(doc(&h).artboards().len(), 2);
}

#[test]
fn dragging_an_artboard_name_moves_it_with_its_layers() {
    let mut h = app();
    h.state_mut().run(Action::Tool(Tool::Transform));
    h.run_steps(1);
    let label = screen(&h, 5.0, 0.0) - vec2(0.0, 10.0);
    let step = screen(&h, 50.0, 20.0) - screen(&h, 0.0, 0.0);
    let logo_before = screen(&h, 20.0, 20.0);
    drag(&mut h, label, label + step, Modifiers::NONE);
    // A at (50, 20): the canvas starts there now (B keeps the top).
    assert_eq!(board(&h, "A").1, rect(0, 20, 200, 150));
    assert_eq!(board(&h, "B").1, rect(250, 0, 100, 100));
    let d = doc(&h);
    let logo = d.layers.iter().find(|l| l.name == "logo").unwrap();
    assert_eq!(logo.offset, (20, 40), "with it");
    // On screen, it went where the pointer took it: the view followed the
    // canvas.
    assert!(screen(&h, 20.0, 40.0).distance(logo_before + step) < 1.0);
    h.state_mut().undo();
    assert_eq!(board(&h, "A").1, rect(0, 0, 200, 150));
}

#[test]
fn alt_click_on_a_plus_button_duplicates_the_artboard() {
    let mut h = app();
    let (a, _) = board(&h, "A");
    h.state_mut().doc.as_mut().unwrap().active = a;
    h.state_mut().run(Action::Tool(Tool::Artboard));
    h.run_steps(2);
    let plus = screen(&h, 100.0, 150.0) + vec2(0.0, 22.0);
    h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    h.hover_at(plus);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: plus,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::ALT,
    });
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: plus,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::ALT,
    });
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    let d = doc(&h);
    assert_eq!(d.artboards().len(), 3);
    let copy = d.active;
    assert_eq!(d.layers[copy].artboard_rect(), Some(rect(0, 250, 200, 150)));
    let content: Vec<&str> = d.block(copy).map(|i| d.layers[i].name.as_str()).collect();
    assert!(content.contains(&"logo"), "{content:?}");
    assert_eq!(d.full_rect(), rect(0, 0, 400, 400));
}

#[test]
fn the_move_tool_moves_a_layer_into_another_artboard() {
    let mut h = app();
    h.state_mut().run(Action::Tool(Tool::Transform));
    h.run_steps(1);
    let (from, to) = (screen(&h, 40.0, 40.0), screen(&h, 340.0, 40.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let app = h.state();
    assert!(app.transform_rect.is_none(), "applied on release");
    let d = doc(&h);
    let logo = d.layers.iter().position(|l| l.name == "logo").unwrap();
    assert_eq!(d.layers[logo].offset, (320, 20));
    assert_eq!(d.artboard_of(logo), Some(board(&h, "B").0));
    // Shown there, over B's background.
    assert_eq!(d.flatten().get_pixel(330, 30).0, RED.0);
    // Off every artboard: onto the pasteboard.
    let (from, to) = (screen(&h, 340.0, 40.0), screen(&h, 250.0, 200.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let d = doc(&h);
    let logo = d.layers.iter().position(|l| l.name == "logo").unwrap();
    assert_eq!((d.artboard_of(logo), d.layers[logo].depth), (None, 0));
}

#[test]
fn zoom_to_an_artboard() {
    let mut h = app();
    let (b, r) = board(&h, "B");
    h.state_mut()
        .run(Action::Artboard(artboard_tool::Command::Fit(b)));
    h.run_steps(2);
    let shown =
        screen(&h, r.right() as f32, r.bottom() as f32) - screen(&h, r.x as f32, r.y as f32);
    assert!(shown.y > 800.0, "{shown:?}");
    let center = screen(&h, 350.0, 50.0);
    let area = h.state().rulers.canvas_area().expect("canvas area");
    assert!(
        center.distance(area.center()) < 2.0,
        "{center:?} {:?}",
        area.center()
    );
}

#[test]
fn file_new_with_artboards() {
    let mut h = app();
    h.state_mut().new_doc.last.artboards = true;
    h.state_mut().run(Action::NewDocument);
    h.run_steps(2);
    assert!(h.state().modal_open());
    h.key_press(Key::Enter);
    h.run_steps(2);
    let d = doc(&h);
    assert_eq!(d.artboards().len(), 1);
    assert_eq!(d.full_rect(), rect(0, 0, 1920, 1080));
    assert_eq!(h.state().tab_count(), 2);
}
