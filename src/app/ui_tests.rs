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

/// The window at `size`, with a 400 × 300 image and the `tool`.
fn window_with(lang: Lang, size: [f32; 2], tool: Tool) -> Harness<'static, AtelierApp> {
    i18n::set_lang(lang);
    let mut harness = Harness::builder().with_size(size).build_eframe(move |cc| {
        let mut app = AtelierApp::new(cc, None);
        let img = image::RgbaImage::from_pixel(400, 300, image::Rgba([90, 120, 200, 255]));
        app.doc = Some(Document::new(img, None));
        app.set_tool(tool);
        app
    });
    harness.run_steps(4);
    harness
}

/// The options bar's height (without its margins), as last laid out.
fn bar_height(h: &Harness<'static, AtelierApp>) -> f32 {
    h.ctx
        .data(|d| {
            d.get_temp::<((u32, &'static str), f32, f32)>(egui::Id::new("options-bar-height"))
        })
        .expect("options bar shown")
        .2
}

/// Every control of the options bar ends inside the window.
fn bar_fits(h: &Harness<'static, AtelierApp>, what: &str) {
    use egui_kittest::kittest::{NodeT as _, by};
    let width = h.ctx.content_rect().width();
    let bottom = 40.0 + 16.0 + bar_height(h);
    let mut seen = 0;
    for node in h.query_all(by().predicate(|_| true)) {
        let Some(b) = node.accesskit_node().bounding_box() else {
            continue;
        };
        let r = Rect::from_min_max(
            pos2(b.x0 as f32, b.y0 as f32),
            pos2(b.x1 as f32, b.y1 as f32),
        );
        if r.top() >= 40.0 && r.bottom() <= bottom && r.width() < width - 1.0 {
            seen += 1;
            assert!(
                r.right() <= width,
                "{what}: {:?} at {r:?}",
                node.accesskit_node().label()
            );
        }
    }
    assert!(seen >= 2, "{what}: the bar's controls found");
}

/// The Move tool's bar: one row at the minimum window width (660 px) in
/// English and French; a pending transform adds its options (on more rows,
/// all on screen, Scale and ✓ included) without moving the image.
#[test]
fn move_bar_is_compact_and_its_transform_fits() {
    for lang in [Lang::En, Lang::Fr] {
        let mut h = window_with(lang, [660.0, 500.0], Tool::Transform);
        assert!(bar_height(&h) <= 30.0, "{lang:?}: {}", bar_height(&h));
        bar_fits(&h, "idle");
        h.get_by_label(&t!("tools4-show-transform-controls"));
        let before = h.state().last_view.expect("canvas shown").origin;
        h.state_mut()
            .run(Action::Transform(transform::Command::Free));
        h.run_steps(4);
        assert!(
            bar_height(&h) > 30.0,
            "{lang:?}: the transform's options wrap"
        );
        bar_fits(&h, "transforming");
        h.get_by_label(&t!("optbar-scale"));
        let apply = h.get_by_label(&t!("options-apply")).rect();
        assert!(apply.right() <= 660.0, "{lang:?}: {apply:?}");
        let after = h.state().last_view.expect("canvas shown").origin;
        assert!(
            (after - before).length() <= 1.0,
            "{lang:?}: {before:?} → {after:?}"
        );
    }
}

/// Switching tools doesn't change the bar's height (the toolbar under it
/// would move): the tallest bar seen at this width stays.
#[test]
fn tool_switches_keep_the_bar_height() {
    let mut h = window_with(Lang::Fr, [1024.0, 700.0], Tool::Brush);
    h.state_mut().set_tool(Tool::Select);
    h.run_steps(3);
    let marquee = bar_height(&h);
    for tool in [Tool::Brush, Tool::Transform, Tool::Select] {
        h.state_mut().set_tool(tool);
        h.run_steps(3);
        assert_eq!(bar_height(&h), marquee, "{tool:?}");
    }
}

/// With every floating panel open in a window too small for all of them,
/// each one still shows: none opens exactly under another.
#[test]
fn floating_panels_never_open_hidden_under_another() {
    let mut h = window_with(Lang::Fr, [1280.0, 720.0], Tool::Brush);
    for action in [
        Action::Automate(actions_panel::Command::Panel),
        Action::Paths,
        Action::Channels,
        Action::Swatches,
        Action::History,
    ] {
        h.state_mut().run(action);
        h.run_steps(4);
    }
    let rects: Vec<(&str, Rect)> = ["actions", "paths", "channels", "swatches", "history"]
        .into_iter()
        .map(|id| {
            let r = h.ctx.memory(|m| m.area_rect(egui::Id::new(id)));
            (id, r.unwrap_or_else(|| panic!("{id} shown")))
        })
        .collect();
    // The last one opened is in front.
    let (last, r) = rects[rects.len() - 1];
    let on_top = h.ctx.layer_id_at(r.center_top() + vec2(0.0, 10.0));
    assert_eq!(on_top.map(|l| l.id), Some(egui::Id::new(last)));
    // Each one opened later is in front: the title bars of those before
    // still show.
    for (i, (b, rb)) in rects.iter().enumerate() {
        for (a, ra) in &rects[i + 1..] {
            let title = Rect::from_min_size(rb.min, vec2(rb.width(), 20.0));
            assert!(!ra.contains_rect(title), "{b} {rb:?} under {a} {ra:?}");
        }
    }
}

/// During a transform, at every window width, the Move bar's controls stay
/// on screen (a drop-down that wrapped inside its own greyed group was
/// drawn past the window's edge, and pushed ✓ off it).
#[test]
fn transform_bar_fits_at_every_width() {
    for lang in [Lang::En, Lang::Fr] {
        let mut h = window_with(lang, [660.0, 600.0], Tool::Transform);
        h.state_mut()
            .run(Action::Transform(transform::Command::Free));
        h.run_steps(3);
        for width in (660..=1700).step_by(15) {
            h.set_size(vec2(width as f32, 600.0));
            h.run_steps(3);
            bar_fits(&h, &format!("{lang:?} at {width}"));
            let apply = h.get_by_label(&t!("options-apply")).rect();
            assert!(
                apply.right() <= width as f32,
                "{lang:?} at {width}: {apply:?}"
            );
        }
    }
}

/// The active layer's blend mode and opacity sit above the Layers list,
/// named for screen readers; a change there is one step.
#[test]
fn blend_and_opacity_are_above_the_layers_list() {
    let mut h = window_with(Lang::En, [1366.0, 768.0], Tool::Brush);
    let blend = h.get_by_label(&t!("panel-blend-mode")).rect();
    let layers = h.get_by_label(&t!("panel-layers")).rect();
    // (The Brush's options bar has one too.)
    let opacity = h
        .get_all_by_label(&t!("panel-opacity"))
        .map(|n| n.rect())
        .find(|r| r.left() > 1000.0)
        .expect("in the side panel");
    assert!(
        blend.top() > layers.top() && blend.top() < layers.bottom() + 40.0,
        "{blend:?}"
    );
    assert!((opacity.center().y - blend.center().y).abs() < 4.0);
    let steps = steps_of(&h);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .opacity = 1.0;
    // Dragging the opacity scrub to the left.
    let from = opacity.center();
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(from - vec2(40.0, 0.0));
    h.run_steps(1);
    h.drop_at(from - vec2(40.0, 0.0));
    h.run_steps(2);
    assert!(h.state().doc.as_ref().unwrap().active_layer().opacity < 1.0);
    assert_eq!(steps_of(&h), steps + 1);
}

/// Layer › Hide Layers (Ctrl+,): the selected layers hidden, in one step;
/// again, shown.
#[test]
fn hide_layers_toggles_the_selected_layers() {
    let mut h = window_with(Lang::En, [1024.0, 700.0], Tool::Brush);
    h.state_mut().run(Action::NewLayer);
    h.run_steps(2);
    let steps = steps_of(&h);
    h.state_mut().run(Action::ToggleVisibility);
    h.run_steps(1);
    let doc = h.state().doc.as_ref().unwrap();
    assert!(!doc.active_layer().visible);
    assert!(doc.layers[0].visible, "only the selected one");
    assert_eq!(steps_of(&h), steps + 1);
    let shown = t!("ui5-menu-show-layers");
    let listed = h.state().menus().into_iter().any(|(_, entries)| {
        entries
            .iter()
            .any(|e| matches!(e, MenuEntry::Item(label, ..) if *label == shown))
    });
    assert!(listed, "the item says what it does now");
    h.state_mut().run(Action::ToggleVisibility);
    assert!(h.state().doc.as_ref().unwrap().active_layer().visible);
}

/// Select › Modify with no selection: greyed as a whole, like its items.
#[test]
fn a_submenu_with_nothing_available_is_greyed() {
    let mut h = window_with(Lang::En, [1024.0, 700.0], Tool::Brush);
    h.get_by_label(&t!("menu-select")).click();
    h.run_steps(3);
    use egui_kittest::kittest::NodeT as _;
    let label = format!("{} ⏵", t!("sel-menu-modify"));
    let modify = h.get_by_label(&label);
    assert!(modify.accesskit_node().is_disabled());
}

/// A tall side panel lists more layers rather than leaving blank room
/// under the Properties.
#[test]
fn a_tall_panel_lists_more_layers() {
    let mut h = window_with(Lang::En, [1600.0, 1400.0], Tool::Brush);
    for _ in 0..8 {
        h.state_mut().run(Action::NewLayer);
    }
    h.run_steps(4);
    // Its heading, the blend row, the buttons and more than 5 rows of 46.
    let heading = h.get_by_label(&t!("panel-layers")).rect();
    let height = 1400.0 - 28.0 - heading.top();
    assert!(height > 42.0 + 36.0 + 6.0 + 41.0 + 6.0 * 46.0, "{height}");
}

/// Steps in the active document's history.
fn steps_of(h: &Harness<'static, AtelierApp>) -> usize {
    h.state().doc.as_ref().unwrap().history().0.len()
}
