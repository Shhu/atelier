//! User paths driven by real events, that injected calls used to stand in
//! for: opacity digits typed in a row, Alt+click on an eye, Copy then
//! Paste in Place through the keys, Select › Inverse, File › Open Recent.

use super::*;
use egui_kittest::Harness;

type H = Harness<'static, AtelierApp>;

/// A 100 × 80 document, its background and two layers (red, green), the
/// red one active; frames 50 ms apart.
fn app() -> H {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .with_step_dt(0.05)
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let mut doc = Document::new(image::RgbaImage::from_pixel(100, 80, image::Rgba([255; 4])), None);
            doc.add_layer("Red".into(), image::RgbaImage::from_pixel(100, 80, image::Rgba([255, 0, 0, 255])), (0, 0));
            doc.add_layer("Green".into(), image::RgbaImage::from_pixel(100, 80, image::Rgba([0, 255, 0, 255])), (0, 0));
            doc.active = 1;
            app.doc = Some(doc);
            app.tool = Tool::Canvas;
            app
        });
    h.run_steps(3);
    h
}

fn doc(h: &H) -> &Document {
    h.state().doc.as_ref().expect("a document")
}

fn key(h: &mut H, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        });
        h.run_steps(1);
    }
}

fn modifiers(h: &mut H, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.run_steps(1);
}

fn opacity(h: &H) -> u32 {
    (doc(h).active_layer().opacity * 100.0).round() as u32
}

#[test]
fn two_digits_typed_in_a_row_give_the_exact_opacity() {
    let mut h = app();
    // 4 then 5 at once: 45 %.
    key(&mut h, Key::Num4, Modifiers::NONE);
    assert_eq!(opacity(&h), 40);
    key(&mut h, Key::Num5, Modifiers::NONE);
    assert_eq!(opacity(&h), 45);
    // A second later, 5 alone: 50 %.
    h.run_steps(20);
    key(&mut h, Key::Num5, Modifiers::NONE);
    assert_eq!(opacity(&h), 50);
    // 0 then 5: 5 %; 0 alone: 100 %.
    h.run_steps(20);
    key(&mut h, Key::Num0, Modifiers::NONE);
    key(&mut h, Key::Num5, Modifiers::NONE);
    assert_eq!(opacity(&h), 5);
    h.run_steps(20);
    key(&mut h, Key::Num0, Modifiers::NONE);
    assert_eq!(opacity(&h), 100);
}

#[test]
fn alt_click_on_an_eye_shows_that_layer_alone_then_all_again() {
    let mut h = app();
    let id = doc(&h).layers[1].id;
    let eye = h
        .state()
        .ctx
        .read_response(egui::Id::new(("side_eye", id)))
        .expect("the eye of Red shown")
        .rect
        .center();
    let visible = |h: &H| doc(h).layers.iter().map(|l| l.visible).collect::<Vec<_>>();
    let alt_click = |h: &mut H| {
        modifiers(h, Modifiers::ALT);
        h.hover_at(eye);
        h.run_steps(1);
        for pressed in [true, false] {
            h.event(egui::Event::PointerButton {
                pos: eye,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::ALT,
            });
            h.run_steps(1);
        }
        modifiers(h, Modifiers::NONE);
    };
    alt_click(&mut h);
    assert_eq!(visible(&h), [false, true, false], "only Red shown");
    alt_click(&mut h);
    assert_eq!(visible(&h), [true, true, true], "all shown again");
}

#[test]
fn ctrl_c_then_ctrl_shift_v_pastes_where_it_was_copied() {
    let mut h = app();
    h.state_mut().selection = Some(PixelRect { x: 10, y: 20, w: 8, h: 6 }.into());
    h.state_mut().sync_selection();
    h.run_steps(1);
    // Ctrl+C: egui-winit reports a copy, no key press.
    modifiers(&mut h, Modifiers::COMMAND);
    h.event(egui::Event::Copy);
    h.run_steps(1);
    modifiers(&mut h, Modifiers::NONE);
    let copied = super::clipboard::FAKE.with_borrow(|f| f.image.clone()).expect("copied");
    assert_eq!(copied.dimensions(), (8, 6));
    assert_eq!(copied.get_pixel(0, 0).0, [255, 0, 0, 255]);
    // Elsewhere, then Ctrl+Shift+V: a text paste, V let go with both held.
    h.state_mut().selection = None;
    h.state_mut().sync_selection();
    let shift_ctrl = Modifiers::COMMAND.plus(Modifiers::SHIFT);
    modifiers(&mut h, shift_ctrl);
    h.event(egui::Event::Paste(String::new()));
    h.run_steps(1);
    h.event(egui::Event::Key {
        key: Key::V,
        physical_key: Some(Key::V),
        pressed: false,
        repeat: false,
        modifiers: shift_ctrl,
    });
    h.run_steps(1);
    modifiers(&mut h, Modifiers::NONE);
    h.run_steps(2);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 4, "a new layer");
    assert_eq!(d.history().0.last().copied(), Some(t!("files3-menu-paste-in-place").as_str()));
    let pasted = d.active_layer();
    assert_eq!(pasted.pixel_at(10, 20), Some([255, 0, 0, 255]), "in place");
    assert_eq!(pasted.pixel_at(17, 25), Some([255, 0, 0, 255]));
    // Not centered (46..54, 37..43).
    assert!(pasted.pixel_at(50, 40).is_none_or(|p| p[3] == 0));
}

#[test]
fn ctrl_shift_i_inverts_the_selection() {
    let mut h = app();
    h.state_mut().selection = Some(PixelRect { x: 10, y: 20, w: 8, h: 6 }.into());
    h.state_mut().sync_selection();
    h.run_steps(1);
    key(&mut h, Key::I, Modifiers::COMMAND.plus(Modifiers::SHIFT));
    let sel = h.state().selection.clone().expect("a selection");
    assert_eq!((sel.coverage(12, 22), sel.coverage(0, 0), sel.coverage(99, 79)), (0, 255, 255));
    // Again: as it was.
    key(&mut h, Key::I, Modifiers::COMMAND.plus(Modifiers::SHIFT));
    let sel = h.state().selection.clone().expect("a selection");
    assert_eq!((sel.coverage(12, 22), sel.coverage(0, 0)), (255, 0));
}

#[test]
fn a_file_reopened_from_open_recent_goes_to_the_top() {
    let dir = crate::testing::test_dir("events14-recent");
    let (a, b) = (dir.join("a.png"), dir.join("b.png"));
    image::RgbaImage::from_pixel(6, 4, image::Rgba([1, 2, 3, 255])).save(&a).unwrap();
    image::RgbaImage::from_pixel(5, 3, image::Rgba([4, 5, 6, 255])).save(&b).unwrap();
    let mut h = app();
    h.state_mut().recent = crate::recent::Recent::with_paths(vec![b.clone(), a.clone()]);
    h.state_mut().run(Action::Workspace(workspace::Command::OpenRecent(1)));
    h.run_steps(3);
    assert_eq!(doc(&h).path.as_deref(), Some(a.as_path()), "opened");
    assert_eq!(h.state().recent.paths(), [a, b]);
}
