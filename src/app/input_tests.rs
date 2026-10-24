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
    // The drop-down itself (its label text carries the same name).
    let r = h
        .query_all_by_label("Resample")
        .max_by(|a, b| a.rect().width().total_cmp(&b.rect().width()))
        .expect("Resample drop-down")
        .rect();
    let pos = r.center();
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
    let fill = clipboard_action(&input, None);
    if cfg!(windows) {
        assert!(matches!(fill, Some(Action::FillDialog)));
    } else {
        assert!(fill.is_none(), "Ctrl+Shift+X");
    }
    input.modifiers = Modifiers::COMMAND;
    assert!(matches!(clipboard_action(&input, None), Some(Action::Cut)));
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
    // The 6 key: the first alpha channel; without one, Zoom Out (a French
    // laptop has no other key for it).
    let zoom = h.state().zoom;
    type_key(&mut h, Key::Minus, Key::Num6, Modifiers::COMMAND);
    assert!(h.state().zoom < zoom, "zoomed out");
    h.state_mut().doc.as_mut().unwrap().new_channel();
    let zoom = h.state().zoom;
    type_key(&mut h, Key::Minus, Key::Num6, Modifiers::COMMAND);
    assert_eq!(h.state().zoom, zoom);
    assert_eq!(
        h.state().channels.color_shown(doc(&h)),
        None,
        "the alpha channel"
    );
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
        let action = clipboard_action(&input, None);
        assert!(
            !matches!(action, Some(Action::Copy | Action::CanvasSize)),
            "{m:?}"
        );
    }
    input.modifiers = ctrl.plus(Modifiers::ALT);
    assert!(matches!(
        clipboard_action(&input, None),
        Some(Action::CanvasSize)
    ));
    input.modifiers = ctrl;
    assert!(matches!(clipboard_action(&input, None), Some(Action::Copy)));
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

// ------------------------------------------------- QA4 regressions

/// A secondary click (press and release) at `pos`.
fn right_click(h: &mut Harness<'_, AtelierApp>, pos: Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(1);
}

/// A left click at `pos`, one frame pressed.
fn click_at(h: &mut Harness<'_, AtelierApp>, pos: Pos2) {
    h.hover_at(pos);
    h.run_steps(1);
    press(h, pos, true, Modifiers::NONE);
    h.run_steps(1);
    press(h, pos, false, Modifiers::NONE);
    h.run_steps(3);
}

fn steps(h: &Harness<'static, AtelierApp>) -> usize {
    doc(h).history().0.len()
}

#[test]
fn a_click_closing_a_menu_does_not_reach_the_canvas() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(2);
    // The Edit menu.
    h.get_by_label("Edit").click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "menu open");
    let before = steps(&h);
    let p = screen(&h, 300.0, 250.0);
    click_at(&mut h, p);
    assert!(!egui::Popup::is_any_open(&h.ctx), "menu closed");
    assert_eq!(steps(&h), before, "nothing painted");
    // The canvas's own menu.
    right_click(&mut h, p);
    assert!(egui::Popup::is_any_open(&h.ctx), "context menu open");
    let p = screen(&h, 50.0, 250.0);
    click_at(&mut h, p);
    assert!(!egui::Popup::is_any_open(&h.ctx), "context menu closed");
    assert_eq!(steps(&h), before, "nothing painted");
    // The next click paints.
    click_at(&mut h, p);
    assert_eq!(steps(&h), before + 1, "painted");
}

#[test]
fn a_click_closing_a_tool_flyout_does_not_paint() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(2);
    let slot = h.get_by_label(&Tool::Select.name()).rect().center();
    right_click(&mut h, slot);
    assert!(egui::Popup::is_any_open(&h.ctx), "flyout open");
    let before = steps(&h);
    let p = screen(&h, 50.0, 50.0);
    click_at(&mut h, p);
    assert!(!egui::Popup::is_any_open(&h.ctx), "flyout closed");
    assert_eq!(steps(&h), before);
}

#[test]
fn a_click_closing_a_menu_keeps_the_marquee_selection() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Select);
    h.state_mut().selection = Some(some_selection());
    h.run_steps(2);
    h.get_by_label("Select").click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "menu open");
    let p = screen(&h, 300.0, 250.0);
    click_at(&mut h, p);
    assert!(!egui::Popup::is_any_open(&h.ctx), "menu closed");
    assert!(h.state().selection.is_some(), "not deselected");
}

#[test]
fn delete_in_quick_mask_keeps_the_layer() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(1);
    h.key_press(Key::Q);
    h.run_steps(2);
    assert!(doc(&h).quick_mask().is_some(), "in Quick Mask");
    assert!(h.state().selection.is_none());
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 2, "layer kept");
    assert!(!h.state().status.is_empty(), "says why");
}

#[test]
fn ctrl_drag_with_a_painting_tool_moves_the_selected_pixels() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Brush);
    h.state_mut().selection = Some(Selection {
        rect: PixelRect {
            x: 110,
            y: 110,
            w: 20,
            h: 20,
        },
        mask: None,
        inverted: false,
    });
    h.run_steps(2);
    let (a, b) = (screen(&h, 120.0, 120.0), screen(&h, 220.0, 220.0));
    drag(&mut h, a, b, Modifiers::COMMAND);
    h.key_press(Key::Enter);
    h.run_steps(2);
    // As with the Move tool: the 20 × 20 pixels went, the layer stayed.
    let d = doc(&h);
    let logo = d.layers.iter().find(|l| l.name == "Logo").expect("Logo");
    let bounds = logo.content_bounds().expect("pixels");
    assert_eq!((bounds.x, bounds.y), (100, 100), "the layer stayed");
    assert_eq!((bounds.w, bounds.h), (130, 130), "its pixels moved");
    assert!(h.state().tool == Tool::Brush);
}

#[test]
fn ctrl_drag_with_the_marquee_and_no_selection_moves_the_layer() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Select);
    h.run_steps(2);
    let (a, b) = (screen(&h, 120.0, 120.0), screen(&h, 150.0, 130.0));
    drag(&mut h, a, b, Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(doc(&h).layers[1].offset, (130, 110), "moved");
    assert!(h.state().selection.is_none(), "no marquee");
}

#[test]
fn paste_keeps_the_modifiers_let_go_before_v() {
    let ctrl = Modifiers::COMMAND;
    let shift_ctrl = ctrl.plus(Modifiers::SHIFT);
    let all = shift_ctrl.plus(Modifiers::ALT);
    let release = |modifiers| egui::Event::Key {
        key: Key::V,
        physical_key: Some(Key::V),
        pressed: false,
        repeat: false,
        modifiers,
    };
    // Frames: the modifiers held, then the events of each.
    let run = |frames: &[(Modifiers, Vec<egui::Event>)]| {
        let mut chord = PasteChord {
            idle: true,
            ..Default::default()
        };
        let mut paste = None;
        for (modifiers, events) in frames {
            let mut input = egui::InputState::default();
            input.modifiers = *modifiers;
            input.events = events.clone();
            paste = chord.update(&input).or(paste);
        }
        let mut input = egui::InputState::default();
        input.events = vec![];
        clipboard_action(&input, paste).map(|a| format!("{a:?}"))
    };
    let some = |s: &str| Some(s.to_owned());
    // Shift let go a few ms before V.
    let frames = [
        (shift_ctrl, vec![]),
        (ctrl, vec![]),
        (ctrl, vec![release(ctrl)]),
    ];
    assert_eq!(run(&frames), some("PasteInPlace"));
    // Alt, then Shift, then Ctrl let go before V.
    let frames = [
        (all, vec![]),
        (shift_ctrl, vec![]),
        (ctrl, vec![]),
        (Modifiers::NONE, vec![release(Modifiers::NONE)]),
    ];
    assert_eq!(run(&frames), some("PasteInto"));
    // A plain Ctrl+V after an earlier Shift+B.
    let b = egui::Event::Key {
        key: Key::B,
        physical_key: Some(Key::B),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::SHIFT,
    };
    let frames = [
        (Modifiers::SHIFT, vec![b]),
        (Modifiers::NONE, vec![]),
        (ctrl, vec![]),
        (ctrl, vec![release(ctrl)]),
    ];
    assert_eq!(run(&frames), some("Paste"));
}

#[test]
fn paste_forgets_the_modifiers_of_an_earlier_chord_with_ctrl_held() {
    let ctrl = Modifiers::COMMAND;
    let shift_ctrl = ctrl.plus(Modifiers::SHIFT);
    let all = shift_ctrl.plus(Modifiers::ALT);
    let key = |key, pressed, modifiers| egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    };
    // Frames: their time, the modifiers held, then their events.
    let run = |frames: Vec<(f64, Modifiers, Vec<egui::Event>)>| {
        let mut chord = PasteChord {
            idle: true,
            ..Default::default()
        };
        let mut paste = None;
        for (time, modifiers, events) in frames {
            let mut input = egui::InputState::default();
            input.time = time;
            input.modifiers = modifiers;
            input.events = events;
            paste = chord.update(&input).or(paste);
        }
        let input = egui::InputState::default();
        clipboard_action(&input, paste).map(|a| format!("{a:?}"))
    };
    let some = |s: &str| Some(s.to_owned());
    let paste = || egui::Event::Paste(String::new());
    // Shift+Ctrl+C (a copy event, no key press), Shift let go, Ctrl+V.
    let frames = vec![
        (0.0, shift_ctrl, vec![egui::Event::Copy]),
        (0.1, shift_ctrl, vec![key(Key::C, false, shift_ctrl)]),
        (0.3, ctrl, vec![]),
        (0.6, ctrl, vec![paste()]),
        (0.7, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("Paste"));
    // The same, the clipboard holding an image: no sign of the V press.
    let frames = vec![
        (0.0, shift_ctrl, vec![egui::Event::Copy]),
        (0.3, ctrl, vec![]),
        (0.7, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("Paste"));
    // Shift+Ctrl+V, then Ctrl+V again.
    let frames = vec![
        (0.0, shift_ctrl, vec![paste()]),
        (0.1, shift_ctrl, vec![key(Key::V, false, shift_ctrl)]),
        (0.3, ctrl, vec![]),
        (0.6, ctrl, vec![paste()]),
        (0.7, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("Paste"));
    // Shift+Ctrl+Z (a key press), then Ctrl+V.
    let frames = vec![
        (0.0, shift_ctrl, vec![key(Key::Z, true, shift_ctrl)]),
        (0.1, shift_ctrl, vec![key(Key::Z, false, shift_ctrl)]),
        (0.3, ctrl, vec![]),
        (0.7, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("Paste"));
    // Alt+Shift+Ctrl+E, Alt and Shift let go, then Ctrl+V.
    let frames = vec![
        (0.0, all, vec![key(Key::E, true, all)]),
        (0.1, all, vec![key(Key::E, false, all)]),
        (0.3, ctrl, vec![]),
        (0.7, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("Paste"));
    // But Shift let go a moment before V still counts, and Shift held
    // while V is down too, however long V is held.
    let frames = vec![
        (0.0, shift_ctrl, vec![]),
        (0.05, ctrl, vec![paste()]),
        (0.6, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("PasteInPlace"));
    let frames = vec![
        (0.0, shift_ctrl, vec![paste()]),
        (0.5, ctrl, vec![]),
        (0.9, ctrl, vec![key(Key::V, false, ctrl)]),
    ];
    assert_eq!(run(frames), some("PasteInPlace"));
}

#[test]
fn held_toggles_do_not_repeat() {
    let mut h = with_logo();
    for repeat in [false, true, true, true] {
        h.event(egui::Event::Key {
            key: Key::Tab,
            physical_key: Some(Key::Tab),
            pressed: true,
            repeat,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(1);
        assert!(h.state().workspace.hide_all, "repeat {repeat}");
    }
    assert!(h.ctx.memory(|m| m.focused()).is_none(), "focus untouched");
    let rulers = h.state().rulers.settings.rulers;
    for repeat in [false, true, true] {
        h.event(egui::Event::Key {
            key: Key::R,
            physical_key: Some(Key::R),
            pressed: true,
            repeat,
            modifiers: Modifiers::COMMAND,
        });
        h.run_steps(1);
    }
    assert_ne!(h.state().rulers.settings.rulers, rulers, "toggled once");
    // Undo repeats.
    for _ in 0..2 {
        h.state_mut().run(Action::NewLayer);
    }
    h.run_steps(1);
    let layers = doc(&h).layers.len();
    for repeat in [false, true] {
        h.event(egui::Event::Key {
            key: Key::Z,
            physical_key: Some(Key::Z),
            pressed: true,
            repeat,
            modifiers: Modifiers::COMMAND,
        });
        h.run_steps(1);
    }
    assert_eq!(doc(&h).layers.len(), layers - 2, "undone twice");
}

#[test]
fn tab_hides_the_floating_panels_too() {
    let mut h = with_logo();
    h.state_mut().run(Action::History);
    h.run_steps(3);
    let history = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("history"));
    let shown =
        |h: &Harness<'_, AtelierApp>| h.ctx.memory(|m| m.areas().visible_last_frame(&history));
    assert!(shown(&h), "History shown");
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(h.state().workspace.hide_all);
    assert!(!shown(&h), "History hidden");
    assert!(h.state().history.open, "and still open");
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(shown(&h), "History back");
}

#[test]
fn the_side_panel_edge_follows_the_pointer_back() {
    let mut h = with_logo();
    let start = h.state().workspace.layout.side_width;
    let edge = h.ctx.content_rect().right() - start + 2.0;
    let at = pos2(edge, 400.0);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(at - vec2(100.0 * i as f32, 0.0));
        h.run_steps(1);
    }
    assert_eq!(h.state().workspace.layout.side_width, workspace::SIDE_MAX);
    for i in (0..4).rev() {
        h.hover_at(at - vec2(100.0 * i as f32, 0.0));
        h.run_steps(1);
    }
    h.drop_at(at);
    h.run_steps(2);
    assert_eq!(h.state().workspace.layout.side_width, start);
}

#[test]
fn escape_closes_keyboard_shortcuts_and_diagnostics() {
    let mut h = with_logo();
    for command in [
        workspace::Command::Shortcuts,
        workspace::Command::Diagnostics,
    ] {
        h.state_mut().run(Action::Workspace(command));
        h.run_steps(3);
        h.key_press(Key::Enter);
        h.run_steps(2);
        let open = |h: &Harness<'_, AtelierApp>| {
            h.state().workspace.shortcuts.is_some() || h.state().diagnostics.open
        };
        assert!(open(&h), "{command:?}: Enter isn't theirs");
        h.key_press(Key::Escape);
        h.run_steps(2);
        assert!(!open(&h), "{command:?} closed");
    }
}

#[test]
fn tab_goes_from_field_to_field_in_a_dialog() {
    let mut h = with_logo();
    h.state_mut().run(Action::ImageSize);
    h.run_steps(8);
    // Open, its first field has the keyboard.
    assert!(h.ctx.text_edit_focused(), "first field focused");
    let first = h.ctx.memory(|m| m.focused());
    let mut seen = vec![first];
    // Tab goes on to the other fields, past the checkboxes and drop-downs,
    // and comes back round.
    for _ in 0..6 {
        h.key_press(Key::Tab);
        h.run_steps(FOCUS_HUNT_TRIES as usize);
        assert!(h.ctx.text_edit_focused(), "a field has the keyboard");
        seen.push(h.ctx.memory(|m| m.focused()));
    }
    assert_ne!(seen[1], first, "moved on");
    assert!(seen[1..].contains(&first), "came back round: {seen:?}");
    // Shift+Tab goes back.
    let now = h.ctx.memory(|m| m.focused());
    h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    h.run_steps(FOCUS_HUNT_TRIES as usize);
    assert!(h.ctx.text_edit_focused());
    assert_ne!(h.ctx.memory(|m| m.focused()), now);
}

#[test]
fn shift_pressed_again_constrains_an_added_marquee() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Select);
    h.state_mut().selection = Some(some_selection());
    h.run_steps(2);
    let (a, b) = (screen(&h, 200.0, 50.0), screen(&h, 300.0, 90.0));
    // Shift at the press: Add. Let go, then pressed again: a square.
    h.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(a);
    h.run_steps(1);
    press(&h, a, true, Modifiers::SHIFT);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.hover_at(a.lerp(b, 0.5));
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    h.hover_at(b);
    h.run_steps(1);
    press(&h, b, false, Modifiers::SHIFT);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
    let sel = h.state().selection.clone().expect("a selection");
    assert!(sel.contains(290.0, 140.0), "a 100 × 100 square added");
    assert!(sel.contains(20.0, 20.0), "the first one kept");
}

#[test]
fn ime_composition_is_shown_until_committed() {
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(1);
    let p = screen(&h, 50.0, 50.0);
    click_at(&mut h, p);
    assert!(h.state().type_tool.edit.is_some(), "typing");
    h.event(egui::Event::Ime(egui::ImeEvent::Preedit {
        text: "かん".into(),
        active_range_chars: None,
    }));
    h.run_steps(1);
    let edit = h.state().type_tool.edit.as_ref().unwrap();
    assert_eq!(edit.preedit, "かん");
    h.event(egui::Event::Ime(egui::ImeEvent::Commit("漢".into())));
    h.run_steps(1);
    let edit = h.state().type_tool.edit.as_ref().unwrap();
    assert!(edit.preedit.is_empty());
    let text = doc(&h).active_layer().text.as_ref().expect("a text");
    assert_eq!(text.text, "漢");
}

#[test]
fn alt_click_clips_only_on_the_line_between_two_rows() {
    let mut h = with_logo();
    h.state_mut()
        .click_row(1, right_panel::RowPart::Name, Modifiers::ALT);
    assert!(!doc(&h).layers[1].clipped, "not on the name");
    h.state_mut()
        .click_row(0, right_panel::RowPart::ClipLine(1), Modifiers::ALT);
    assert!(doc(&h).layers[1].clipped, "on the line");
}

// ------------------------------------------------- Third review

/// Frames, a little time between them (a background preview runs).
fn frames(h: &mut Harness<'static, AtelierApp>, n: usize) {
    for _ in 0..n {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn a_filter_dialog_keeps_the_keyboard_while_its_preview_computes() {
    let mut h = with_logo();
    h.state_mut()
        .run(Action::Filter(crate::filters::FilterKind::GaussianBlur));
    frames(&mut h, 20);
    // Its first field has the keyboard, with no click.
    assert!(h.ctx.text_edit_focused(), "first field focused");
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    // Each digit starts a preview: the field keeps the keyboard.
    for c in ["2", "5"] {
        h.event(egui::Event::Text(c.into()));
        frames(&mut h, 4);
        assert!(h.ctx.text_edit_focused(), "still focused after {c}");
    }
    assert!(h.query_all_by_value("25").next().is_some(), "25 typed");
}

#[test]
fn dialogs_give_the_keyboard_to_their_first_field() {
    use crate::filters::FilterKind;
    for (action, focused) in [
        (Action::Filter(FilterKind::UnsharpMask), true),
        (Action::Filter(FilterKind::AddNoise), true),
        (Action::Adjust(AdjustKind::BrightnessContrast), true),
        (Action::Adjust(AdjustKind::HueSaturation), true),
        (Action::FillDialog, true),
        // Its keys move the points.
        (Action::Adjust(AdjustKind::Curves), false),
    ] {
        let mut h = with_logo();
        h.state_mut().run(action);
        frames(&mut h, 45);
        assert!(h.state().modal_open(), "{action:?} open");
        assert_eq!(h.ctx.text_edit_focused(), focused, "{action:?}");
    }
}

#[test]
fn enter_while_the_focus_goes_to_the_first_field_is_not_a_buttons() {
    let mut h = with_logo();
    h.state_mut()
        .run(Action::Filter(crate::filters::FilterKind::GaussianBlur));
    // The focus on its way, over the preview and its zoom buttons.
    h.run_steps(2);
    assert!(focus_hunt(&h.ctx).is_some(), "on its way");
    // Enter then: the focus stops and is let go, so no button takes it
    // (the dialog's OK does).
    h.key_press(Key::Enter);
    h.run_steps(1);
    assert!(focus_hunt(&h.ctx).is_none(), "stopped");
    assert!(
        !h.state().modal_open() || h.ctx.memory(|m| m.focused()).is_none(),
        "no button focused"
    );
}

#[test]
fn window_menu_shows_a_panel_hidden_with_tab() {
    let mut h = with_logo();
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(h.state().workspace.hide_all);
    let shown = |h: &Harness<'static, AtelierApp>, id: &str| {
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new(id));
        h.ctx.memory(|m| m.areas().visible_last_frame(&layer))
    };
    h.state_mut().run(Action::History);
    h.run_steps(3);
    assert!(h.state().history.open && !h.state().workspace.hide_all);
    assert!(shown(&h, "history"), "History shown");
    // Hidden again, then Alt+F9: Actions, History still open.
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(h.state().workspace.hide_all);
    h.key_press_modifiers(Modifiers::ALT, Key::F9);
    h.run_steps(3);
    assert!(shown(&h, "actions") && shown(&h, "history"));
    // Shown: Window › History hides it again.
    h.state_mut().run(Action::History);
    h.run_steps(3);
    assert!(!h.state().history.open);
}

#[test]
fn ctrl_drag_with_the_selection_tools_and_the_eyedropper_moves() {
    for tool in [
        Tool::FreeLasso,
        Tool::PolyLasso,
        Tool::Lasso,
        Tool::MagicWand,
        Tool::ObjectSelect,
        Tool::Eyedropper,
    ] {
        let mut h = with_logo();
        h.state_mut().set_tool(tool);
        h.state_mut().rulers.settings.snap = false;
        h.run_steps(2);
        let primary = h.state().primary;
        let (a, b) = (screen(&h, 120.0, 120.0), screen(&h, 150.0, 130.0));
        drag(&mut h, a, b, Modifiers::COMMAND);
        h.run_steps(2);
        assert_eq!(doc(&h).layers[1].offset, (130, 110), "{tool:?} moved the layer");
        assert!(h.state().selection.is_none(), "{tool:?}: no selection");
        assert_eq!(h.state().primary, primary, "{tool:?}: no color picked");
        assert_eq!(doc(&h).history().0.len(), 2, "{tool:?}: one step");
    }
    // With a selection: its pixels, the layer stays.
    let mut h = with_logo();
    h.state_mut().set_tool(Tool::FreeLasso);
    h.state_mut().rulers.settings.snap = false;
    h.state_mut().selection = Some(Selection {
        rect: PixelRect {
            x: 110,
            y: 110,
            w: 20,
            h: 20,
        },
        mask: None,
        inverted: false,
    });
    h.run_steps(2);
    let (a, b) = (screen(&h, 120.0, 120.0), screen(&h, 150.0, 130.0));
    drag(&mut h, a, b, Modifiers::COMMAND);
    h.run_steps(2);
    assert_eq!(doc(&h).layers[1].offset, (100, 100), "layer kept");
    let moved = h.state().selection.as_ref().map(|s| (s.rect.x, s.rect.y));
    assert_eq!(moved, Some((140, 120)), "selection moved with its pixels");
}

#[test]
fn a_press_that_closes_a_menu_reaches_no_panel() {
    let mut h = with_logo();
    let brush = h.get_by_label(&Tool::Brush.name()).rect().center();
    h.get_by_label("Edit").click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx));
    let event = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    let mut swallowed = false;
    let mut raw = egui::RawInput {
        events: vec![event(brush, true)],
        ..Default::default()
    };
    swallow_popup_closing_press(&h.ctx, &mut raw, &mut swallowed);
    assert!(raw.events.is_empty() && swallowed, "press dropped");
    assert!(!egui::Popup::is_any_open(&h.ctx), "menu closed");
    raw.events = vec![event(brush, false)];
    swallow_popup_closing_press(&h.ctx, &mut raw, &mut swallowed);
    assert!(raw.events.is_empty() && !swallowed, "its release too");
    // The next press goes through.
    raw.events = vec![event(brush, true)];
    swallow_popup_closing_press(&h.ctx, &mut raw, &mut swallowed);
    assert_eq!(raw.events.len(), 1);
    // A press in the menu itself, or on another menu title, too.
    h.run_steps(2);
    h.get_by_label("Edit").click();
    h.run_steps(3);
    let window = h.get_by_label("Window").rect().center();
    let menu = h
        .ctx
        .memory(|m| {
            m.layer_ids()
                .filter(|l| l.order == egui::Order::Foreground)
                .filter(|l| m.areas().visible_last_frame(l))
                .filter_map(|l| m.area_rect(l.id))
                // Under the title bar.
                .find(|r| (30.0..=60.0).contains(&r.top()))
        })
        .expect("the Edit menu");
    for pos in [menu.center(), window] {
        raw.events = vec![event(pos, true)];
        swallow_popup_closing_press(&h.ctx, &mut raw, &mut swallowed);
        assert_eq!(raw.events.len(), 1, "{pos:?} kept");
        assert!(!swallowed);
    }
}

#[test]
fn a_resized_panel_corner_follows_the_pointer_back() {
    let mut h = with_logo();
    h.state_mut().run(Action::History);
    h.run_steps(4);
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("history"));
    let rect = || h.ctx.memory(|m| m.area_rect(layer.id)).expect("History shown");
    let before = rect();
    let corner = before.right_bottom() - vec2(6.0, 6.0);
    h.hover_at(corner);
    h.run_steps(1);
    press(&h, corner, true, Modifiers::NONE);
    h.run_steps(1);
    // Far past the minimum size, then back where it started.
    let far = corner - vec2(600.0, 600.0);
    for i in 1..=6 {
        h.hover_at(corner.lerp(far, i as f32 / 6.0));
        h.run_steps(1);
    }
    for i in 1..=6 {
        h.hover_at(far.lerp(corner, i as f32 / 6.0));
        h.run_steps(1);
    }
    press(&h, corner, false, Modifiers::NONE);
    h.run_steps(2);
    let after = h.ctx.memory(|m| m.area_rect(layer.id)).expect("History shown");
    assert!(
        (after.size() - before.size()).length() < 1.5,
        "{before:?} -> {after:?}"
    );
}
