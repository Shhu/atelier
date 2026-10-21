//! Layout at small window sizes, and labels in the UI's language.

use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

/// The window at its minimum size, with a 512 × 512 image.
fn small_window(lang: Lang) -> Harness<'static, AtelierApp> {
    i18n::set_lang(lang);
    let mut harness = Harness::builder()
        .with_size([600.0, 400.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(512, 512, image::Rgba([90, 120, 200, 255]));
            app.doc = Some(Document::new(img, None));
            app
        });
    harness.run_steps(3);
    harness
}

/// Header (title, close) and footer (OK, Cancel) of a dialog taller than the
/// window stay on screen: its body scrolls.
#[test]
fn dialogs_fit_in_a_small_window() {
    for (action, id) in [
        (Action::CanvasSize, "canvas_size"),
        (Action::Adjust(AdjustKind::Curves), "adjustment"),
        (Action::AiModels, "ai_models"),
    ] {
        let mut h = small_window(Lang::Fr);
        h.state_mut().run(action);
        h.run_steps(4);
        let screen = h.ctx.content_rect();
        let rect = h
            .ctx
            .memory(|m| m.area_rect(egui::Id::new(id)))
            .unwrap_or_else(|| panic!("{id} shown"));
        assert!(screen.contains_rect(rect), "{id}: {rect:?} in {screen:?}");
    }
}

/// A dialog dragged against an edge comes back as soon as it is dragged the
/// other way (the drag beyond the edge is not kept).
#[test]
fn dialog_dragged_off_screen_comes_back() {
    let mut h = small_window(Lang::En);
    h.state_mut().run(Action::RotateArbitrary);
    h.run_steps(4);
    let id = egui::Id::new("rotate_canvas");
    let start = h.ctx.memory(|m| m.area_rect(id)).expect("dialog shown");
    let grab = start.left_top() + vec2(30.0, 20.0);
    let drag = |h: &mut Harness<'static, AtelierApp>, from: Pos2, to: Pos2| {
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for i in 1..=6 {
            h.hover_at(from.lerp(to, i as f32 / 6.0));
            h.run_steps(1);
        }
        h.drop_at(to);
        h.run_steps(2);
    };
    drag(&mut h, grab, grab + vec2(0.0, 600.0));
    let pushed = h.ctx.memory(|m| m.area_rect(id)).expect("dialog shown");
    assert!(h.ctx.content_rect().contains_rect(pushed));
    let grab = pushed.left_top() + vec2(30.0, 20.0);
    drag(&mut h, grab, grab - vec2(0.0, 50.0));
    let back = h.ctx.memory(|m| m.area_rect(id)).expect("dialog shown");
    let moved = pushed.top() - back.top();
    assert!(moved > 40.0, "moved up by {moved}");
}

/// French menu titles are wider: at 600 px, the undo button must not sit on
/// the last menu (a click there undid as well as opening the menu).
#[test]
fn menus_are_not_covered_at_the_minimum_width() {
    let mut h = small_window(Lang::Fr);
    h.state_mut().run(Action::NewLayer);
    h.run_steps(2);
    let view = h.get_by_label("Affichage").rect();
    // Minimize, maximize, close: 3 × 46 px at the right end (macOS keeps
    // its own buttons, on the left).
    let buttons = if cfg!(target_os = "macos") {
        0.0
    } else {
        3.0 * 46.0
    };
    assert!(view.right() <= 600.0 - buttons, "{view:?}");
    let pos = pos2(view.right() - 4.0, view.center().y);
    h.hover_at(pos);
    h.run_steps(1);
    h.drag_at(pos);
    h.run_steps(1);
    h.drop_at(pos);
    h.run_steps(3);
    assert_eq!(h.state().doc.as_ref().unwrap().layers.len(), 2, "undone");
    h.get_by_label_contains(&t!("menu-view-fit-on-screen"));
}

/// Menus, the palette and tooltips show the shortcuts in the UI's language.
#[test]
fn menu_shortcuts_are_translated() {
    let mut h = small_window(Lang::Fr);
    h.get_by_label("Fichier").click();
    h.run_steps(3);
    h.get_by_label_contains("Maj+Ctrl+O");
    let mut h = small_window(Lang::En);
    h.get_by_label("View").click();
    h.run_steps(3);
    h.get_by_label_contains("Ctrl+−");
}
