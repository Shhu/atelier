//! Keyboard and mouse dispatch: who gets the keys (text fields, menus,
//! dialogs, gestures) and what the shortcuts do.

use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

fn app() -> Harness<'static, AtelierApp> {
    let mut harness = Harness::builder()
        .with_size([1600.0, 1100.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(200, 150, image::Rgba([90, 120, 200, 255]));
            app.doc = Some(Document::new(img, None));
            app
        });
    harness.run_steps(3);
    harness
}

fn layers(h: &Harness<'static, AtelierApp>) -> usize {
    h.state().doc.as_ref().unwrap().layers.len()
}

fn some_selection() -> Selection {
    Selection {
        rect: PixelRect {
            x: 10,
            y: 10,
            w: 50,
            h: 40,
        },
        mask: None,
        inverted: false,
    }
}

/// Selection tool with a selection.
fn selecting() -> Harness<'static, AtelierApp> {
    let mut h = app();
    h.state_mut().tool = Tool::Select;
    h.state_mut().selection = Some(some_selection());
    h.run_steps(1);
    h
}

fn focus(h: &mut Harness<'static, AtelierApp>, id: &str) {
    let id = egui::Id::new(id);
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.run_steps(2);
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(id), "field focused");
}

fn key(h: &Harness<'static, AtelierApp>, key: Key, modifiers: Modifiers) {
    h.event(egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    });
}

#[test]
fn escape_leaving_a_text_field_keeps_the_selection() {
    let mut h = selecting();
    focus(&mut h, "layer_name");
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "field left");
    assert!(h.state().selection.is_some());
}

#[test]
fn ctrl_shortcuts_go_to_the_text_field() {
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    h.run_steps(2);
    focus(&mut h, "layer_name");
    for k in [Key::Z, Key::E, Key::J] {
        h.key_press_modifiers(Modifiers::COMMAND, k);
        h.run_steps(2);
    }
    assert_eq!(layers(&h), 2, "no undo, merge or duplicate");
}

#[test]
fn tab_does_not_take_the_keyboard() {
    let mut h = app();
    h.key_press(Key::Tab);
    h.run_steps(2);
    assert!(h.ctx.memory(|m| m.focused()).is_none());
    h.key_press(Key::B);
    h.run_steps(2);
    assert!(h.state().tool == Tool::Brush);
}

#[test]
fn escape_closing_a_menu_keeps_the_selection() {
    let mut h = selecting();
    h.get_by_label("Edit").click();
    h.run_steps(3);
    assert!(
        h.query_by_label_contains("Free Transform").is_some(),
        "menu open"
    );
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(
        h.query_by_label_contains("Free Transform").is_none(),
        "menu closed"
    );
    assert!(h.state().selection.is_some());
}

#[test]
fn escape_closing_a_drop_down_keeps_the_dialog() {
    let mut h = app();
    h.state_mut().run(Action::ImageSize);
    h.run_steps(3);
    let r = h.get_by_label("Resample").rect();
    let pos = pos2(r.left() + 40.0, r.bottom() + 20.0);
    h.hover_at(pos);
    h.run_steps(1);
    h.drag_at(pos);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "drop-down open");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(!egui::Popup::is_any_open(&h.ctx), "drop-down closed");
    assert!(h.state().image_size.is_some(), "dialog still open");
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.state().image_size.is_none());
}

#[test]
fn escape_closes_the_tool_window_first() {
    let mut h = selecting();
    h.state_mut().run(Action::MakeTransparent);
    h.run_steps(2);
    assert!(h.state().transparency.is_some());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().transparency.is_none());
    assert!(h.state().selection.is_some(), "Esc only closed the window");
}

#[test]
fn ctrl_alt_c_opens_canvas_size() {
    let mut h = app();
    h.state_mut().tool = Tool::Brush;
    let m = Modifiers::COMMAND.plus(Modifiers::ALT);
    // egui-winit turns it into a copy.
    h.event(egui::Event::ModifiersChanged(m));
    h.event(egui::Event::Copy);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    assert!(h.state().canvas_size.is_some());
    assert!(h.state().tool == Tool::Brush);
}

#[test]
fn ctrl_q_quits() {
    let mut h = app();
    // One event: one frame, whose output is checked.
    key(&h, Key::Q, Modifiers::COMMAND);
    h.step();
    let closes = h.output().viewport_output.values().any(|v| {
        v.commands
            .iter()
            .any(|c| matches!(c, egui::ViewportCommand::Close))
    });
    assert!(closes);
}

#[test]
fn extra_modifiers_are_other_shortcuts() {
    let mut h = app();
    // Photoshop: Desaturate, Auto Tone, Auto Color (not Hue/Saturation,
    // Levels, Color Balance).
    for k in [Key::U, Key::L, Key::B] {
        h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::SHIFT), k);
        h.run_steps(2);
        assert!(h.state().adjust.is_none(), "{k:?}");
    }
    h.key_press_modifiers(Modifiers::COMMAND, Key::U);
    h.run_steps(2);
    assert!(h.state().adjust.is_some());
}

#[test]
fn alt_digit_picks_the_channel_by_physical_key() {
    let mut h = app();
    h.state_mut().run(Action::Adjust(AdjustKind::Levels));
    h.run_steps(2);
    // AZERTY: the 4 key gives "'".
    h.event(egui::Event::Key {
        key: Key::Quote,
        physical_key: Some(Key::Num4),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::ALT,
    });
    h.run_steps(2);
    assert_eq!(h.state().adjust.as_ref().unwrap().range, 2);
}

#[test]
fn paste_when_ctrl_is_released_before_v() {
    let mut h = app();
    h.state_mut().status = "marker".into();
    // The Ctrl+V press is swallowed by egui-winit (image clipboard); only
    // the release of V arrives, after Ctrl's.
    h.event(egui::Event::Key {
        key: Key::V,
        physical_key: Some(Key::V),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_ne!(h.state().status, "marker", "paste attempted");

    // A plain V (Move tool) is not a paste.
    h.state_mut().status = "marker".into();
    h.key_press(Key::V);
    h.run_steps(2);
    assert_eq!(h.state().status, "marker");
    assert!(h.state().tool == Tool::Transform);
}

#[test]
fn windows_shift_delete_is_not_a_cut() {
    let mut input = egui::InputState::default();
    input.modifiers = Modifiers::SHIFT;
    input.events = vec![egui::Event::Cut];
    let fill = clipboard_action(&input, false);
    if cfg!(windows) {
        assert!(matches!(fill, Some(Action::FillDialog)));
    } else {
        assert!(fill.is_none(), "Ctrl+Shift+X");
    }
    input.modifiers = Modifiers::COMMAND;
    assert!(matches!(clipboard_action(&input, false), Some(Action::Cut)));
}

#[test]
fn no_shortcut_during_a_mouse_gesture() {
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    h.state_mut().tool = Tool::Brush;
    h.run_steps(2);
    let view = h.state().last_view.expect("canvas shown");
    let p = view.to_screen(pos2(50.0, 50.0));
    h.hover_at(p);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::COMMAND));
    key(&h, Key::Z, Modifiers::COMMAND);
    key(&h, Key::E, Modifiers::COMMAND);
    h.run_steps(1);
    key(&h, Key::M, Modifiers::NONE);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert_eq!(layers(&h), 2);
    assert!(h.state().tool == Tool::Brush);
}

#[test]
fn undo_with_a_pending_transform_only_drops_it() {
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    h.state_mut().tool = Tool::Transform;
    h.state_mut().transform_rect = Some(Rect::from_min_size(pos2(5.0, 5.0), vec2(40.0, 30.0)));
    h.run_steps(1);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(h.state().transform_rect.is_none());
    assert_eq!(layers(&h), 2, "the new layer stays");
}

#[test]
fn right_click_keeps_a_pending_transform() {
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    h.state_mut().tool = Tool::Transform;
    let pending = Rect::from_min_size(pos2(5.0, 5.0), vec2(40.0, 30.0));
    h.state_mut().transform_rect = Some(pending);
    h.run_steps(1);
    let view = h.state().last_view.expect("canvas shown");
    // Over the background (the new layer is empty).
    let p = view.to_screen(pos2(150.0, 100.0));
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(1);
    }
    let app = h.state();
    assert_eq!(app.doc.as_ref().unwrap().active, 1);
    assert_eq!(app.transform_rect, Some(pending));
}

#[test]
fn held_keys_do_not_repeat_tool_shortcuts() {
    let mut h = app();
    // E held: the second press is a key repeat (Shift+W held would cycle
    // through its group).
    key(&h, Key::E, Modifiers::NONE);
    h.run_steps(1);
    assert!(h.state().tool == Tool::Eraser);
    h.state_mut().tool = Tool::Brush;
    key(&h, Key::E, Modifiers::NONE);
    h.run_steps(1);
    assert!(h.state().tool == Tool::Brush, "a repeat is not a new press");
}

#[test]
fn shift_digit_is_not_opacity() {
    let mut h = app();
    h.state_mut().tool = Tool::Brush;
    h.key_press_modifiers(Modifiers::SHIFT, Key::Num5);
    h.run_steps(2);
    assert_eq!(h.state().opacity, 1.0);
    h.key_press(Key::Num5);
    h.run_steps(2);
    assert_eq!(h.state().opacity, 0.5);
}
