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

// ------------------------------------------------- Photoshop's habits

/// A 400 × 300 red background and "Logo", 100 × 50 blue at (100, 100),
/// the active layer.
fn with_logo() -> Harness<'static, AtelierApp> {
    let red = image::Rgba([200, 0, 0, 255]);
    let blue = image::Rgba([0, 0, 200, 255]);
    let mut doc = Document::new(image::RgbaImage::from_pixel(400, 300, red), None);
    doc.add_layer(
        "Logo".into(),
        image::RgbaImage::from_pixel(100, 50, blue),
        (100, 100),
    );
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

/// Screen point of image point (x, y).
fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn press(h: &Harness<'_, AtelierApp>, pos: Pos2, pressed: bool, modifiers: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers,
    });
}

/// A left drag from `from` to `to` with `modifiers` held.
fn drag(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.run_steps(1);
    press(h, from, true, modifiers);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(from.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
    press(h, to, false, modifiers);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

/// A key pressed and released with `modifiers`, its physical key `place`.
fn type_key(h: &mut Harness<'_, AtelierApp>, key: Key, place: Key, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key,
            physical_key: Some(place),
            pressed,
            repeat: false,
            modifiers,
        });
    }
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

#[test]
fn azerty_ctrl_digits_show_the_channels() {
    let mut h = app();
    let grid = h.state().rulers.settings.grid;
    // The 4 key types "'" (Ctrl+': Grid), the 6 key "-" (Ctrl+−: Zoom Out).
    type_key(&mut h, Key::Quote, Key::Num4, Modifiers::COMMAND);
    let app = h.state();
    assert_eq!(app.channels.color_shown(doc(&h)), Some(1), "green");
    assert_eq!(app.rulers.settings.grid, grid);
    let zoom = h.state().zoom;
    type_key(&mut h, Key::Minus, Key::Num6, Modifiers::COMMAND);
    assert_eq!(h.state().zoom, zoom);
    // Ctrl+3: red; Ctrl+Alt+3 is not a channel shortcut.
    type_key(&mut h, Key::Num3, Key::Num3, Modifiers::COMMAND);
    assert_eq!(h.state().channels.color_shown(doc(&h)), Some(0));
    type_key(
        &mut h,
        Key::Num4,
        Key::Num4,
        Modifiers::COMMAND.plus(Modifiers::ALT),
    );
    assert_eq!(h.state().channels.color_shown(doc(&h)), Some(0));
}

#[test]
fn azerty_shift_ctrl_semicolon_snaps() {
    let mut h = app();
    let snap = h.state().rulers.settings.snap;
    // Shift and the ";" key type "." on an AZERTY keyboard.
    type_key(
        &mut h,
        Key::Period,
        Key::Comma,
        Modifiers::COMMAND.plus(Modifiers::SHIFT),
    );
    assert_ne!(h.state().rulers.settings.snap, snap);
}

#[test]
fn escape_keeps_the_selection() {
    let mut h = selecting();
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().selection.is_some(), "only Ctrl+D deselects");
}

#[test]
fn held_escape_after_a_dialog_keeps_the_selection() {
    let mut h = selecting();
    h.state_mut().run(Action::ImageSize);
    h.run_steps(3);
    for repeat in [false, true] {
        h.event(egui::Event::Key {
            key: Key::Escape,
            physical_key: Some(Key::Escape),
            pressed: true,
            repeat,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(1);
    }
    assert!(h.state().image_size.is_none(), "dialog cancelled");
    assert!(h.state().selection.is_some());
}

#[test]
fn held_enter_after_a_field_keeps_the_crop() {
    let mut h = app();
    h.state_mut().tool = Tool::Canvas;
    let crop = Rect::from_min_size(pos2(20.0, 20.0), vec2(50.0, 50.0));
    h.state_mut().canvas_rect = Some(crop);
    h.run_steps(1);
    focus(&mut h, "layer_name");
    // Enter held: the press leaves the field, the key repeat follows (egui
    // marks it, the key being down).
    for _ in 0..2 {
        key(&h, Key::Enter, Modifiers::NONE);
        h.run_steps(1);
    }
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "field left");
    assert_eq!(h.state().canvas_rect, Some(crop), "not applied");
}

#[test]
fn delete_without_a_selection_deletes_the_layer() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 1);
    h.state_mut().run(Action::Undo);
    h.run_steps(1);
    assert_eq!(doc(&h).layers.len(), 2, "one step");
    // With a selection, its pixels go.
    h.state_mut().selection = Some(some_selection());
    h.run_steps(1);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 2);
}

#[test]
fn alt_arrows_with_the_move_tool_move_a_copy() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    for _ in 0..2 {
        key(&h, Key::ArrowRight, Modifiers::ALT);
        h.run_steps(1);
    }
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 3, "one copy for the series");
    assert_eq!(d.layers[1].offset, (100, 100), "the original stays");
    assert_eq!(d.active, 2);
    assert_eq!(d.layers[2].offset, (102, 100));
    // Alt again: another copy.
    h.key_press_modifiers(Modifiers::ALT, Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 4);
}

#[test]
fn alt_wheel_zooms_like_ctrl_wheel() {
    let notch = |m: Modifiers| {
        let mut h = app();
        let p = screen(&h, 100.0, 75.0);
        h.hover_at(p);
        h.run_steps(1);
        let before = h.state().zoom;
        h.event(egui::Event::ModifiersChanged(m));
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: m,
        });
        for _ in 0..60 {
            h.step();
        }
        h.state().zoom / before
    };
    let (alt, ctrl) = (notch(Modifiers::ALT), notch(Modifiers::COMMAND));
    assert!(alt > 1.15, "×{alt} per notch");
    assert!((alt - ctrl).abs() < 0.02, "Alt ×{alt}, Ctrl ×{ctrl}");
}

fn space(h: &Harness<'_, AtelierApp>, pressed: bool) {
    h.event(egui::Event::Key {
        key: Key::Space,
        physical_key: Some(Key::Space),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
}

#[test]
fn space_during_a_marquee_moves_it() {
    let mut h = app();
    h.state_mut().tool = Tool::Select;
    h.run_steps(1);
    let (a, b, c) = (
        screen(&h, 20.0, 20.0),
        screen(&h, 60.0, 60.0),
        screen(&h, 100.0, 80.0),
    );
    let zoom = h.state().last_view.unwrap().zoom;
    h.hover_at(a);
    h.run_steps(1);
    press(&h, a, true, Modifiers::NONE);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(1);
    let pan = h.state().pan;
    space(&h, true);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(b.lerp(c, i as f32 / 4.0));
        h.run_steps(1);
    }
    space(&h, false);
    h.run_steps(1);
    press(&h, c, false, Modifiers::NONE);
    h.run_steps(2);
    assert_eq!(h.state().pan, pan, "the view stays");
    let rect = h.state().selection.as_ref().expect("a selection").rect;
    let moved = ((c - b) / zoom).round();
    assert_eq!((rect.w, rect.h), (40, 40), "same size");
    assert_eq!(
        (rect.x, rect.y),
        (20 + moved.x as i64, 20 + moved.y as i64),
        "moved with the pointer"
    );
}

#[test]
fn space_during_a_stroke_does_not_pan() {
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    h.state_mut().tool = Tool::Brush;
    h.run_steps(1);
    let (a, b) = (screen(&h, 20.0, 20.0), screen(&h, 120.0, 80.0));
    h.hover_at(a);
    h.run_steps(1);
    press(&h, a, true, Modifiers::NONE);
    h.run_steps(1);
    let pan = h.state().pan;
    space(&h, true);
    for i in 1..=4 {
        h.hover_at(a.lerp(b, i as f32 / 4.0));
        h.run_steps(1);
    }
    press(&h, b, false, Modifiers::NONE);
    space(&h, false);
    h.run_steps(2);
    assert_eq!(h.state().pan, pan);
}

#[test]
fn ctrl_space_click_zooms_in_alt_space_out() {
    let mut h = app();
    let p = screen(&h, 100.0, 75.0);
    for (m, zoom_in) in [(Modifiers::COMMAND, true), (Modifiers::ALT, false)] {
        let before = h.state().zoom;
        h.event(egui::Event::ModifiersChanged(m));
        space(&h, true);
        h.hover_at(p);
        h.run_steps(1);
        press(&h, p, true, m);
        h.run_steps(1);
        press(&h, p, false, m);
        space(&h, false);
        h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(2);
        let after = h.state().zoom;
        assert_eq!(after > before, zoom_in, "{before} → {after}");
    }
}

#[test]
fn move_tool_drags_the_active_layer_from_anywhere() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Transform);
    h.state_mut().rulers.settings.snap = false;
    h.run_steps(2);
    // Beside Logo, over the background.
    let (from, to) = (screen(&h, 300.0, 250.0), screen(&h, 340.0, 270.0));
    drag(&mut h, from, to, Modifiers::NONE);
    h.key_press(Key::Enter);
    h.run_steps(2);
    let d = doc(&h);
    assert_eq!(d.active, 1, "Logo stays active");
    assert_eq!(d.layers[0].offset, (0, 0), "the background stays");
    assert_eq!(d.layers[1].offset, (140, 120));
}

#[test]
fn ctrl_or_auto_select_picks_the_layer_under_the_pointer() {
    for (auto_select, modifiers) in [(false, Modifiers::COMMAND), (true, Modifiers::NONE)] {
        let mut h = with_logo();
        h.state_mut().set_tool(Tool::Transform);
        h.state_mut().auto_select = auto_select;
        h.run_steps(2);
        let (from, to) = (screen(&h, 300.0, 250.0), screen(&h, 340.0, 270.0));
        drag(&mut h, from, to, modifiers);
        assert_eq!(doc(&h).active, 0, "the background is picked");
    }
}

#[test]
fn right_click_keeps_the_active_layer() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(2);
    let p = screen(&h, 300.0, 250.0);
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
    assert_eq!(doc(&h).active, 1);
    // The Move tool's menu lists the layers under the pointer.
    h.key_press(Key::Escape);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(2);
    let p = screen(&h, 150.0, 120.0);
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
    h.run_steps(1);
    assert_eq!(h.state().menu_layers, vec![1, 0]);
    let background = doc(&h).layers[0].name.clone();
    h.get_by_label(&background).click();
    h.run_steps(2);
    assert_eq!(doc(&h).active, 0);
}

#[test]
fn free_transform_turns_when_dragged_outside_its_frame() {
    let mut h = with_logo();
    h.key_press_modifiers(Modifiers::COMMAND, Key::T);
    h.run_steps(2);
    let corner = screen(&h, 200.0, 150.0);
    drag(
        &mut h,
        corner + vec2(18.0, 18.0),
        corner + vec2(-10.0, 50.0),
        Modifiers::NONE,
    );
    let app = h.state();
    assert_eq!(doc(&h).active, 1, "no other layer picked");
    assert!(
        app.transform_angle.abs() > 1.0,
        "turned: {}",
        app.transform_angle
    );
    assert_eq!(doc(&h).layers[0].offset, (0, 0));
}

#[test]
fn alt_drag_with_the_move_tool_moves_a_copy() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    // A click alone copies nothing.
    let a = screen(&h, 150.0, 125.0);
    h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
    h.hover_at(a);
    h.run_steps(1);
    press(&h, a, true, Modifiers::ALT);
    h.run_steps(1);
    press(&h, a, false, Modifiers::ALT);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 2);
    let b = screen(&h, 250.0, 125.0);
    drag(&mut h, a, b, Modifiers::ALT);
    h.key_press(Key::Enter);
    h.run_steps(2);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 3);
    assert_eq!(d.layers[1].offset, (100, 100), "the original stays");
    assert_eq!(d.layers[2].offset, (200, 100), "the copy moved");
}

#[test]
fn copy_variants_are_not_a_plain_copy() {
    let mut input = egui::InputState::default();
    input.events = vec![egui::Event::Copy];
    let ctrl = Modifiers::COMMAND;
    for m in [
        ctrl.plus(Modifiers::SHIFT),
        ctrl.plus(Modifiers::SHIFT).plus(Modifiers::ALT),
    ] {
        input.modifiers = m;
        let action = clipboard_action(&input, false);
        assert!(
            !matches!(action, Some(Action::Copy | Action::CanvasSize)),
            "{m:?}"
        );
    }
    input.modifiers = ctrl.plus(Modifiers::ALT);
    assert!(matches!(
        clipboard_action(&input, false),
        Some(Action::CanvasSize)
    ));
    input.modifiers = ctrl;
    assert!(matches!(
        clipboard_action(&input, false),
        Some(Action::Copy)
    ));
}

#[test]
fn space_during_a_shape_drag_moves_its_box() {
    let mut h = app();
    h.state_mut()
        .set_tool(Tool::Shape(crate::app::vector_tools::ShapeTool::Rectangle));
    h.run_steps(1);
    let (a, b) = (screen(&h, 20.0, 10.0), screen(&h, 60.0, 40.0));
    let c = screen(&h, 100.0, 60.0);
    h.hover_at(a);
    h.run_steps(1);
    press(&h, a, true, Modifiers::NONE);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(a.lerp(b, i as f32 / 4.0));
        h.run_steps(1);
    }
    space(&h, true);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(b.lerp(c, i as f32 / 4.0));
        h.run_steps(1);
    }
    space(&h, false);
    h.run_steps(1);
    press(&h, c, false, Modifiers::NONE);
    h.run_steps(2);
    let bounds = doc(&h).active_layer().content_bounds().expect("a shape");
    assert_eq!(
        bounds,
        PixelRect {
            x: 60,
            y: 30,
            w: 40,
            h: 30
        }
    );
}
