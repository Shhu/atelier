//! Rotate View tool and View › Flip Horizontal: every tool on a turned view.

use super::*;
use crate::guides::Orientation;
use crate::i18n::t;
use egui_kittest::{Harness, kittest::Queryable};

fn app(tool: Tool) -> Harness<'static, AtelierApp> {
    i18n::set_lang(Lang::En);
    let mut h = Harness::builder()
        .with_size([1400.0, 1000.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]));
            app.doc = Some(Document::new(img, None));
            app.tool = tool;
            app
        });
    h.run_steps(3);
    h
}

fn turned(tool: Tool, angle: f32, flipped: bool) -> Harness<'static, AtelierApp> {
    let mut h = app(tool);
    h.state_mut().view_turn.angle = angle;
    h.state_mut().view_turn.flipped = flipped;
    h.run_steps(2);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

/// Where an image point really is on screen.
fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    let view = app.last_view.expect("canvas shown");
    app.view_turn.to_screen(view.to_screen(pos2(x, y)))
}

/// The image point under a screen position.
fn image_at(h: &Harness<'static, AtelierApp>, p: Pos2) -> Pos2 {
    let app = h.state();
    let view = app.last_view.expect("canvas shown");
    view.to_image(app.view_turn.to_upright(p))
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

fn near(a: Pos2, b: Pos2, tolerance: f32) {
    assert!(a.distance(b) <= tolerance, "{a:?} != {b:?}");
}

#[test]
fn r_picks_the_tool_and_a_drag_turns_the_view_without_history() {
    let mut h = app(Tool::Brush);
    let before = doc(&h).history().1;
    h.key_press(Key::R);
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::RotateView);
    let area = h.state().rulers.canvas_area().expect("canvas");
    let c = area.center();
    // From the right of the center to below it: a quarter turn clockwise.
    drag(&mut h, c + vec2(200.0, 0.0), c + vec2(0.0, 200.0));
    let angle = h.state().view_turn.angle;
    assert!((angle - 90.0).abs() < 1.5, "{angle}");
    assert_eq!(doc(&h).history().1, before, "not a history step");
}

#[test]
fn shift_turns_by_15_degrees_and_esc_resets() {
    let mut h = app(Tool::RotateView);
    let c = h.state().rulers.canvas_area().expect("canvas").center();
    let from = c + vec2(200.0, 0.0);
    let to = c + Vec2::angled(37f32.to_radians()) * 200.0;
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::SHIFT));
    h.event(egui::Event::PointerMoved(to));
    h.run_steps(2);
    assert_eq!(h.state().view_turn.angle, 30.0);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.drop_at(to);
    h.run_steps(2);
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().view_turn.angle, 0.0);
}

#[test]
fn holding_r_comes_back_to_the_tool_after_a_drag() {
    let mut h = app(Tool::Brush);
    h.event(egui::Event::Key {
        key: Key::R,
        physical_key: Some(Key::R),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::RotateView);
    let c = h.state().rulers.canvas_area().expect("canvas").center();
    drag(&mut h, c + vec2(200.0, 0.0), c + vec2(0.0, 200.0));
    assert_eq!(h.state().tool, Tool::RotateView, "R is still down");
    h.event(egui::Event::Key {
        key: Key::R,
        physical_key: Some(Key::R),
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::Brush);
    assert!(h.state().view_turn.angle.abs() > 80.0, "the turn stays");

    // A tap keeps the tool.
    h.key_press(Key::R);
    h.run_steps(3);
    assert_eq!(h.state().tool, Tool::RotateView);
}

#[test]
fn the_brush_paints_under_the_pointer_on_a_turned_and_flipped_view() {
    for (angle, flipped) in [(90.0, false), (33.0, false), (0.0, true), (-120.0, true)] {
        let mut h = turned(Tool::Brush, angle, flipped);
        h.state_mut().primary = Color32::BLACK;
        let at = screen(&h, 60.0, 40.0);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
        let [r, ..] = doc(&h).active_layer().pixel_at(60, 40).expect("inside");
        assert!(r < 128, "{angle} {flipped}: {r}");
        let [r, ..] = doc(&h).active_layer().pixel_at(340, 260).expect("inside");
        assert_eq!(r, 255, "{angle} {flipped}: elsewhere");
    }
}

#[test]
fn a_marquee_selects_image_coordinates_on_a_flipped_view() {
    let mut h = turned(Tool::Select, 0.0, true);
    let (a, b) = (screen(&h, 50.0, 60.0), screen(&h, 150.0, 200.0));
    drag(&mut h, a, b);
    let canvas = doc(&h).full_rect();
    let r = h
        .state()
        .selection
        .as_ref()
        .and_then(|s| s.bounds(canvas))
        .expect("a selection");
    assert!((r.x - 50).abs() <= 2 && (r.y - 60).abs() <= 2, "{r:?}");
    assert!(
        (r.w as i64 - 100).abs() <= 3 && (r.h as i64 - 140).abs() <= 3,
        "{r:?}"
    );
}

#[test]
fn the_hand_follows_the_pointer_on_a_turned_view() {
    let mut h = turned(Tool::Hand, 60.0, true);
    let at = screen(&h, 200.0, 150.0);
    let to = at + vec2(120.0, -70.0);
    drag(&mut h, at, to);
    near(screen(&h, 200.0, 150.0), to, 1.5);
}

#[test]
fn zooming_keeps_the_point_under_the_pointer() {
    let mut h = turned(Tool::Zoom, 45.0, false);
    let at = screen(&h, 100.0, 100.0);
    h.hover_at(at);
    h.run_steps(1);
    let before = image_at(&h, at);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let zoomed = h.state().zoom;
    assert!(zoomed > 1.0, "{zoomed}");
    near(image_at(&h, at), before, 1.0);
}

#[test]
fn fit_on_screen_fits_the_turned_bounds() {
    let mut h = app(Tool::Hand);
    let img = image::RgbaImage::from_pixel(3000, 800, image::Rgba([255, 255, 255, 255]));
    h.state_mut().doc = Some(Document::new(img, None));
    let area = h.state().rulers.canvas_area().expect("canvas");
    for angle in [0.0, 30.0, 90.0] {
        h.state_mut().view_turn.angle = angle;
        h.state_mut().run(Action::Fit);
        h.run_steps(3);
        for (x, y) in [(0.0, 0.0), (3000.0, 0.0), (0.0, 800.0), (3000.0, 800.0)] {
            let p = screen(&h, x, y);
            assert!(area.contains(p), "{angle}: corner {p:?} out of {area:?}");
        }
    }
}

#[test]
fn a_guide_pulled_from_a_ruler_lands_under_the_pointer() {
    let mut h = turned(Tool::Transform, 90.0, false);
    h.key_press_modifiers(Modifiers::COMMAND, Key::R);
    h.run_steps(3);
    let canvas = h.state().rulers.canvas_area().expect("canvas");
    let to = screen(&h, 200.0, 100.0);
    let from = pos2(to.x, canvas.top() - 8.0);
    drag(&mut h, from, to);
    let guides = &doc(&h).guides;
    assert_eq!(guides.len(), 1);
    assert_eq!(guides[0].orientation, Orientation::Horizontal);
    assert!((guides[0].position - 100.0).abs() <= 8.0, "{guides:?}");
}

#[test]
fn transform_handles_are_where_they_show() {
    let mut h = turned(Tool::Transform, 90.0, false);
    h.state_mut().auto_select = false;
    let img = image::RgbaImage::from_pixel(200, 100, image::Rgba([10, 20, 30, 255]));
    let doc_mut = h.state_mut().doc.as_mut().expect("doc");
    doc_mut.add_layer("small".into(), img, (0, 0));
    h.run_steps(2);
    h.state_mut().free_transform = true;
    h.run_steps(2);
    let corner = screen(&h, 200.0, 100.0);
    let to = screen(&h, 300.0, 150.0);
    drag(&mut h, corner, to);
    let r = h.state().transform_rect.expect("a resized frame");
    assert!((r.max.x - 300.0).abs() <= 3.0, "{r:?}");
    assert!((r.max.y - 150.0).abs() <= 3.0, "{r:?}");
    assert!(r.min.distance(Pos2::ZERO) <= 1.0, "{r:?}");
}

#[test]
fn each_document_has_its_view_and_rotate_all_windows_sets_them_all() {
    let mut h = app(Tool::RotateView);
    h.state_mut().view_turn.angle = 40.0;
    h.state_mut().view_turn.flipped = true;
    let img = image::RgbaImage::from_pixel(50, 50, image::Rgba([0, 0, 0, 255]));
    h.state_mut().add_tab(Document::new(img, None));
    h.run_steps(2);
    let fresh = h.state().view_turn;
    assert_eq!((fresh.angle, fresh.flipped), (0.0, false), "a new tab");
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    let back = h.state().view_turn;
    assert_eq!(
        (back.angle, back.flipped),
        (40.0, true),
        "kept with its tab"
    );

    h.state_mut().rulers.set_rotate_all_windows(true);
    h.state_mut().set_view_angle(-25.0);
    h.state_mut().switch_tab(1);
    h.run_steps(2);
    assert_eq!(h.state().view_turn.angle, -25.0);
    assert!(!h.state().view_turn.flipped, "the angle only");
}

#[test]
fn flip_horizontal_is_a_view_command_with_its_check_mark() {
    let mut h = app(Tool::Hand);
    let before = doc(&h).history().1;
    let flip = Action::View(rulers::Command::FlipHorizontal);
    h.state_mut().run(flip);
    h.run_steps(2);
    assert!(h.state().view_turn.flipped);
    assert_eq!(doc(&h).history().1, before);
    let entries = h.state().view_menu_entries();
    assert!(entries.iter().any(|e| matches!(
        e,
        MenuEntry::Check(
            _,
            true,
            true,
            None,
            Action::View(rulers::Command::FlipHorizontal)
        )
    )));
    h.state_mut().doc = None;
    let entries = h.state().view_menu_entries();
    assert!(entries.iter().any(|e| matches!(
        e,
        MenuEntry::Check(
            _,
            false,
            _,
            None,
            Action::View(rulers::Command::FlipHorizontal)
        )
    )));
}

/// What `apply` leaves on the layer, painted with an upright painter.
fn applied(turn: view_turn::ViewTurn, paint: impl Fn(&Painter)) -> Vec<(Rect, egui::Shape)> {
    let ctx = egui::Context::default();
    let area = Rect::from_center_size(Pos2::new(400.0, 300.0), vec2(800.0, 600.0));
    let layer = egui::LayerId::background();
    let mut out = Vec::new();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = ui.ctx().clone();
        let mut turn = turn;
        turn.center = area.center();
        let upright = turn.upright_bounds(area);
        let first = ctx.graphics_mut(|g| g.entry(layer).next_idx());
        paint(&Painter::new(ctx.clone(), layer, upright));
        turn.apply(&ctx, layer, first, area, upright);
        ctx.graphics(|g| {
            let list = g.get(layer).expect("painted");
            out = list
                .all_entries()
                .skip(first.0)
                .map(|c| (c.clip_rect, c.shape.clone()))
                .collect();
        });
    });
    // No renderer here: the font atlas's upload mustn't trip epaint's drop check.
    output.textures_delta.clear();
    out
}

fn turn_90() -> view_turn::ViewTurn {
    view_turn::ViewTurn {
        angle: 90.0,
        ..Default::default()
    }
}

#[test]
fn tiles_turn_as_meshes_clipped_to_the_canvas() {
    let tile = Rect::from_min_size(pos2(400.0, 300.0), vec2(100.0, 50.0));
    let shapes = applied(turn_90(), |p| {
        p.image(
            egui::TextureId::Managed(7),
            tile,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    });
    let (clip, egui::Shape::Mesh(mesh)) = &shapes[0] else {
        panic!("a mesh: {:?}", shapes[0]);
    };
    assert_eq!(mesh.texture_id, egui::TextureId::Managed(7));
    assert_eq!(clip.width(), 800.0);
    // 100 × 50 to the right of the center: now 50 wide, below and to the left.
    let b = mesh.calc_bounds();
    near(b.min, pos2(350.0, 300.0), 0.01);
    near(b.max, pos2(400.0, 400.0), 0.01);
    // No feathered fringe: neighboring tiles share their edges exactly.
    assert_eq!(mesh.vertices.len(), 4);
}

#[test]
fn a_narrower_clip_cuts_in_upright_space() {
    let shapes = applied(turn_90(), |p| {
        let frame = Rect::from_min_size(pos2(400.0, 300.0), vec2(50.0, 50.0));
        p.with_clip_rect(frame).rect_filled(
            Rect::from_min_size(pos2(400.0, 300.0), vec2(200.0, 200.0)),
            0.0,
            Color32::RED,
        );
    });
    let egui::Shape::Mesh(mesh) = &shapes[0].1 else {
        panic!("a mesh");
    };
    let b = mesh.calc_bounds();
    assert!(b.width() <= 52.0 && b.height() <= 52.0, "{b:?}");
}

#[test]
fn text_stays_readable_and_labels_stay_in_one_piece() {
    let tag = Rect::from_min_size(pos2(500.0, 300.0), vec2(120.0, 30.0));
    let shapes = applied(turn_90(), |p| {
        p.rect_filled(tag, 2.0, Color32::WHITE);
        p.text(
            tag.center(),
            egui::Align2::CENTER_CENTER,
            "label",
            egui::FontId::proportional(12.0),
            Color32::BLACK,
        );
    });
    let egui::Shape::Rect(moved) = &shapes[0].1 else {
        panic!("the box stays a rectangle: {:?}", shapes[0].1);
    };
    assert_eq!(moved.rect.size(), tag.size(), "not turned");
    // Its center has gone around the view's.
    near(moved.rect.center(), pos2(385.0, 460.0), 0.01);
    let egui::Shape::Text(text) = &shapes[1].1 else {
        panic!("still a text");
    };
    assert_eq!(text.angle, 0.0);
    assert!(moved.rect.contains_rect(text.visual_bounding_rect()));
}

#[test]
fn the_options_fit_a_narrow_window_in_french() {
    i18n::set_lang(Lang::Fr);
    let mut h = Harness::builder()
        .with_size([660.0, 700.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]));
            app.doc = Some(Document::new(img, None));
            app.tool = Tool::RotateView;
            app
        });
    h.run_steps(3);
    let check = h.get_by_label(&t!("view8-rotate-all-windows")).rect();
    assert!(check.right() <= 660.0, "{check:?}");
    i18n::set_lang(Lang::En);
}

/// Each angle and flip the tools must hold on.
const TURNS: [(f32, bool); 3] = [(30.0, false), (0.0, true), (30.0, true)];

/// A point of the image, within a tolerance.
fn at_image(p: Pos2, x: f32, y: f32, tolerance: f32) {
    near(p, pos2(x, y), tolerance);
}

#[test]
fn the_crop_tool_lands_on_the_image_on_a_turned_or_flipped_view() {
    for (angle, flipped) in TURNS {
        let mut h = turned(Tool::Canvas, angle, flipped);
        h.state_mut().rulers.settings.snap = false;
        h.run_steps(1);
        let zoom = h.state().last_view.expect("canvas").zoom;
        let tolerance = 1.5 / zoom + 1.0;
        let what = format!("{angle} {flipped}");

        // A box drawn.
        let (a, b) = (screen(&h, 100.0, 80.0), screen(&h, 220.0, 170.0));
        drag(&mut h, a, b);
        let r = h.state().canvas_rect.expect(&what);
        at_image(r.min, 100.0, 80.0, tolerance);
        at_image(r.max, 220.0, 170.0, tolerance);
        assert_eq!(h.state().crop.angle, 0.0, "{what}: not turned");

        // Its south-east corner, where it shows.
        let corner = screen(&h, r.max.x, r.max.y);
        let to = screen(&h, 250.0, 200.0);
        drag(&mut h, corner, to);
        let r = h.state().canvas_rect.expect(&what);
        at_image(r.max, 250.0, 200.0, tolerance);
        at_image(r.min, 100.0, 80.0, tolerance);

        // Its inside moves it.
        let (a, b) = (screen(&h, 160.0, 130.0), screen(&h, 170.0, 150.0));
        drag(&mut h, a, b);
        let r = h.state().canvas_rect.expect(&what);
        at_image(r.min, 110.0, 100.0, tolerance + 0.5);

        // Outside: from the east of its center to the south-east of it,
        // +45° on the image, turned clockwise in image terms.
        let c = r.center();
        let (a, b) = (screen(&h, c.x + 130.0, c.y), screen(&h, c.x + 130.0, c.y + 130.0));
        drag(&mut h, a, b);
        let box_angle = h.state().crop.angle;
        assert!((box_angle - 45.0).abs() < 2.0, "{what}: {box_angle}");

        // A corner of the turned box, where it shows: the opposite one stays.
        let r = h.state().canvas_rect.expect(&what);
        let turned_corner = |r: Rect, p: Pos2| canvas::rotate_around(p, r.center(), box_angle);
        let grip = turned_corner(r, r.right_bottom());
        let keep = turned_corner(r, r.left_top());
        let out = r.center() + (grip - r.center()) * 0.8;
        let (a, b) = (screen(&h, grip.x, grip.y), screen(&h, out.x, out.y));
        drag(&mut h, a, b);
        let after = h.state().canvas_rect.expect(&what);
        assert!(after.width() < r.width() - 3.0, "{what}: {r:?} -> {after:?}");
        assert!(after.height() < r.height() - 3.0, "{what}: {r:?} -> {after:?}");
        near(turned_corner(after, after.left_top()), keep, tolerance + 1.0);
    }
}

#[test]
fn a_turned_view_crops_what_the_box_shows() {
    for (angle, flipped) in TURNS {
        let mut h = turned(Tool::Canvas, angle, flipped);
        h.state_mut().rulers.settings.snap = false;
        h.state_mut()
            .doc
            .as_mut()
            .expect("doc")
            .add_layer("Mark".into(), image::RgbaImage::from_pixel(20, 20, image::Rgba([200, 0, 0, 255])), (150, 120));
        h.run_steps(2);
        let (a, b) = (screen(&h, 140.0, 110.0), screen(&h, 180.0, 150.0));
        drag(&mut h, a, b);
        h.key_press(Key::Enter);
        h.run_steps(3);
        let flat = doc(&h).flatten();
        let (w, h_) = flat.dimensions();
        assert!(w.abs_diff(40) <= 2 && h_.abs_diff(40) <= 2, "{w} {h_}");
        assert_eq!(flat.get_pixel(w / 2, h_ / 2).0, [200, 0, 0, 255], "{angle} {flipped}");
        assert_eq!(flat.get_pixel(2, 2).0, [255, 255, 255, 255], "{angle} {flipped}");
    }
}

#[test]
fn the_pen_and_path_selection_land_on_the_image_on_a_turned_or_flipped_view() {
    use super::vector_tools::PenTool;
    for (angle, flipped) in TURNS {
        let mut h = turned(Tool::Pen(PenTool::Pen), angle, flipped);
        h.state_mut().rulers.settings.snap = false;
        h.run_steps(1);
        let what = format!("{angle} {flipped}");
        let tolerance = 1.5 / h.state().last_view.expect("canvas").zoom + 0.5;
        // Two corner anchors, then a smooth one.
        for (x, y) in [(60.0, 50.0), (200.0, 60.0)] {
            let at = screen(&h, x, y);
            drag(&mut h, at, at);
        }
        let (a, b) = (screen(&h, 200.0, 200.0), screen(&h, 240.0, 200.0));
        drag(&mut h, a, b);
        let path = doc(&h).work_path.clone().expect(&what);
        let anchors = &path.subpaths[0].anchors;
        assert_eq!(anchors.len(), 3, "{what}");
        at_image(pos2(anchors[0].p.x, anchors[0].p.y), 60.0, 50.0, tolerance);
        at_image(pos2(anchors[1].p.x, anchors[1].p.y), 200.0, 60.0, tolerance);
        at_image(pos2(anchors[2].p.x, anchors[2].p.y), 200.0, 200.0, tolerance);
        // The handle went where the pointer did, the other one opposite.
        at_image(pos2(anchors[2].after.x, anchors[2].after.y), 240.0, 200.0, tolerance);
        at_image(pos2(anchors[2].before.x, anchors[2].before.y), 160.0, 200.0, tolerance);

        // Direct Selection drags the anchor where it shows.
        h.state_mut().set_tool(Tool::DirectSelect);
        h.run_steps(2);
        let (a, b) = (screen(&h, 200.0, 60.0), screen(&h, 260.0, 110.0));
        drag(&mut h, a, b);
        let path = doc(&h).work_path.clone().expect(&what);
        let moved = path.subpaths[0].anchors[1].p;
        at_image(pos2(moved.x, moved.y), 260.0, 110.0, tolerance);

        // Path Selection takes the whole path.
        h.state_mut().set_tool(Tool::PathSelect);
        h.run_steps(2);
        let (a, b) = (screen(&h, 60.0, 50.0), screen(&h, 70.0, 80.0));
        drag(&mut h, a, b);
        let path = doc(&h).work_path.clone().expect(&what);
        let s = &path.subpaths[0].anchors;
        at_image(pos2(s[0].p.x, s[0].p.y), 70.0, 80.0, tolerance);
        at_image(pos2(s[1].p.x, s[1].p.y), 270.0, 140.0, tolerance + 0.5);
    }
}

#[test]
fn the_type_tool_lands_on_the_image_on_a_turned_or_flipped_view() {
    for (angle, flipped) in TURNS {
        let mut h = turned(Tool::Text, angle, flipped);
        if h.state_mut().text_engine().families().is_empty() {
            return;
        }
        let what = format!("{angle} {flipped}");
        let tolerance = 1.5 / h.state().last_view.expect("canvas").zoom + 0.5;
        let at = screen(&h, 120.0, 150.0);
        drag(&mut h, at, at);
        assert!(h.state().typing_on_canvas(), "{what}");
        h.event(egui::Event::Text("Hello world".into()));
        h.run_steps(2);
        let (offset, data) = {
            let l = doc(&h).active_layer();
            (l.offset, l.text.clone().expect("a text layer"))
        };
        // The first baseline is on the click.
        let origin = type_tool::text_to_canvas(offset, &data, (0.0, 0.0));
        at_image(origin, 120.0, 150.0, tolerance);

        // A click inside it: the caret goes between the letters shown there.
        let layout = h.state_mut().text_engine().layout(&data);
        let (x, top, bottom) = layout.caret(6);
        let p = type_tool::text_to_canvas(offset, &data, (x, (top + bottom) / 2.0));
        let p = screen(&h, p.x, p.y);
        drag(&mut h, p, p);
        let edit = h.state().type_tool.edit.as_ref().expect(&what);
        assert_eq!(edit.range(), 6..6, "{what}");

        // Input methods get the caret where it shows on screen.
        h.run_steps(1);
        let cursor = h.output().platform_output.ime.expect("input methods on").cursor_rect;
        near(cursor.center(), p, 2.0);
    }
}

/// Grey noise: every tool through `brush_input` changes something in it.
fn noise(w: u32, h: u32) -> Document {
    let mut img = image::RgbaImage::new(w, h);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let n = (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)).wrapping_mul(2654435761) >> 24;
        let v = 90 + (n % 80) as u8;
        *p = image::Rgba([v, 200 - v / 2, 255 - v, 255]);
    }
    Document::new(img, None)
}

const BRUSH_TOOLS: [Tool; 11] = [
    Tool::Brush,
    Tool::Eraser,
    Tool::Dodge,
    Tool::Burn,
    Tool::Sponge,
    Tool::Blur,
    Tool::Sharpen,
    Tool::Smudge,
    Tool::CloneStamp,
    Tool::HealingBrush,
    Tool::SpotHealing,
];

/// A real moving drag: the events reach `frame_samples` as the system sends them.
#[test]
fn every_brush_tool_strokes_only_along_the_pointer_on_a_turned_view() {
    let turns = [
        (0.0, false),
        (90.0, false),
        (33.0, false),
        (180.0, false),
        (0.0, true),
        (-120.0, true),
    ];
    for tool in BRUSH_TOOLS {
        for (angle, flipped) in turns {
            let what = format!("{tool:?} {angle} {flipped}");
            let mut h = turned(tool, angle, flipped);
            h.state_mut().doc = Some(noise(400, 300));
            h.run_steps(2);
            let before = (*doc(&h).active_layer().shared_image()).clone();
            let app = h.state_mut();
            app.primary = Color32::BLACK;
            app.secondary = Color32::WHITE;
            app.brush_size = 10.0;
            app.hardness = 1.0;
            app.paint.smoothing = 0.0;
            if tool.clones() {
                app.set_clone_source(pos2(40.0, 260.0));
            }
            let (a, b) = (pos2(100.0, 100.0), pos2(300.0, 140.0));
            let (sa, sb) = (screen(&h, a.x, a.y), screen(&h, b.x, b.y));
            drag(&mut h, sa, sb);
            // Healing lands from its job.
            for _ in 0..400 {
                if !h.state().retouch_running() {
                    break;
                }
                h.run_steps(1);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            h.run_steps(2);
            let layer = doc(&h).active_layer();
            let (mut near_line, mut stray) = (0, 0);
            let ab = b - a;
            for (x, y, p) in before.enumerate_pixels() {
                if layer.pixel_at(i64::from(x), i64::from(y)) == Some(p.0) {
                    continue;
                }
                let at = pos2(x as f32, y as f32);
                let t = ((at - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0);
                if (a + ab * t).distance(at) > 14.0 {
                    stray += 1;
                } else {
                    near_line += 1;
                }
            }
            assert_eq!(stray, 0, "{what}: pixels away from the stroke");
            assert!(near_line > 300, "{what}: the stroke painted {near_line}");
        }
    }
}

#[test]
fn alt_right_drag_resizes_the_brush_in_screen_directions() {
    for (angle, flipped) in [(0.0, false), (90.0, false), (180.0, false), (0.0, true), (33.0, true)] {
        let what = format!("{angle} {flipped}");
        let mut h = turned(Tool::Brush, angle, flipped);
        h.state_mut().brush_size = 100.0;
        h.state_mut().hardness = 0.5;
        let at = screen(&h, 200.0, 150.0);
        h.hover_at(at);
        h.run_steps(1);
        h.event(egui::Event::ModifiersChanged(Modifiers::ALT));
        h.event(egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: true,
            modifiers: Modifiers::ALT,
        });
        h.run_steps(2);
        let zoom = h.state().last_view.expect("canvas").zoom;
        // To the right on screen: the size only.
        h.event(egui::Event::PointerMoved(at + vec2(20.0, 0.0)));
        h.run_steps(2);
        let expected = (100.0 + 2.0 * 20.0 / zoom).round();
        assert_eq!(h.state().brush_size, expected, "{what}");
        assert_eq!(h.state().hardness, 0.5, "{what}");
        // Down on screen: the hardness only.
        h.event(egui::Event::PointerMoved(at + vec2(20.0, 40.0)));
        h.run_steps(2);
        assert_eq!(h.state().brush_size, expected, "{what}");
        assert!((h.state().hardness - 0.7).abs() < 1e-4, "{what}");
        h.event(egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Secondary,
            pressed: false,
            modifiers: Modifiers::ALT,
        });
        h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
        h.run_steps(2);
    }
}

/// At quarter turns and flips, what each ruler reads at a screen position is
/// the coordinate of the image under it.
#[test]
fn the_rulers_read_the_image_under_them_at_quarter_turns() {
    for flipped in [false, true] {
        for angle in [0.0, 90.0, 180.0, -90.0] {
            let h = turned(Tool::Hand, angle, flipped);
            let (view, turn) = (h.state().last_view.expect("canvas"), h.state().view_turn);
            let c = turn.center;
            let [top, left] = turn.ruler_axes();
            assert_ne!(top.image_x, left.image_x);
            let (origin_top, origin_left) = (
                top.place(view.origin, c, c.x),
                left.place(view.origin, c, c.y),
            );
            for ip in [pos2(0.0, 0.0), pos2(400.0, 0.0), pos2(120.0, 77.0)] {
                let s = screen(&h, ip.x, ip.y);
                let value = |image_x: bool| if image_x { ip.x } else { ip.y };
                let tick_top = origin_top + top.sign * value(top.image_x) * view.zoom;
                let tick_left = origin_left + left.sign * value(left.image_x) * view.zoom;
                let what = format!("{angle} {flipped} {ip:?}");
                assert!((tick_top - s.x).abs() < 0.01, "{what}: {tick_top} {}", s.x);
                assert!((tick_left - s.y).abs() < 0.01, "{what}: {tick_left} {}", s.y);
                // The pointer's mark is right on it.
                let u = turn.to_upright(s);
                assert!((top.place(u, c, c.x) - s.x).abs() < 0.01, "{what}");
                assert!((left.place(u, c, c.y) - s.y).abs() < 0.01, "{what}");
            }
        }
    }
}

#[test]
fn each_ruler_keeps_its_own_axis_between_quarter_turns() {
    for angle in [45.0, -45.0, 135.0, -135.0, 30.0, 60.0] {
        for flipped in [false, true] {
            let t = view_turn::ViewTurn { angle, flipped, center: Pos2::ZERO };
            let [top, left] = t.ruler_axes();
            assert_ne!(top.image_x, left.image_x, "{angle} {flipped}");
        }
    }
}

#[test]
fn a_drag_anywhere_on_an_artboard_name_takes_that_artboard() {
    use crate::document::ArtboardBackground;
    for (angle, end) in [
        (0.0f32, -1.0f32),
        (90.0, -1.0),
        (90.0, 0.0),
        (90.0, 1.0),
        (45.0, -1.0),
        (45.0, 1.0),
    ] {
        let mut h = app(Tool::Transform);
        let mut board = Document::with_artboard(300, 200, ArtboardBackground::White);
        board.layers[1].name = "A rather long artboard name".into();
        board.add_artboard(
            "new".into(),
            "B".into(),
            PixelRect { x: 500, y: 0, w: 100, h: 100 },
            ArtboardBackground::White,
        );
        board.active = board.layers.iter().position(|l| l.name == "B").expect("B");
        h.state_mut().doc = Some(board);
        h.state_mut().view_turn.angle = angle;
        h.run_steps(3);
        let (_, r) = h.state().artboards.labels[0];
        let c = h.state().view_turn.to_screen(r.center());
        let from = c + vec2(end * (r.width() / 2.0 - 4.0), 0.0);
        drag(&mut h, from, from + vec2(60.0, 40.0));
        let name = &doc(&h).active_layer().name;
        assert!(name.starts_with("A rather"), "{angle} {end}: {name}");
    }
}

/// A Free Transform with a dragged corner.
fn free_transform() -> Harness<'static, AtelierApp> {
    let mut h = app(Tool::Transform);
    h.state_mut().auto_select = false;
    let img = image::RgbaImage::from_pixel(100, 100, image::Rgba([10, 20, 30, 255]));
    h.state_mut().doc.as_mut().expect("doc").add_layer("s".into(), img, (0, 0));
    h.run_steps(2);
    h.state_mut().free_transform = true;
    h.run_steps(2);
    let (a, b) = (screen(&h, 100.0, 100.0), screen(&h, 150.0, 150.0));
    drag(&mut h, a, b);
    assert!(h.state().transform_rect.is_some());
    h
}

fn r_key(h: &mut Harness<'static, AtelierApp>, pressed: bool) {
    h.event(egui::Event::Key {
        key: Key::R,
        physical_key: Some(Key::R),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
}

#[test]
fn holding_r_turns_the_view_during_a_free_transform_and_keeps_it() {
    let mut h = free_transform();
    let steps = doc(&h).history().1;
    let frame = h.state().transform_rect;
    r_key(&mut h, true);
    assert_eq!(h.state().tool, Tool::Transform);
    assert_eq!(h.state().transform_rect, frame, "not applied");
    let c = h.state().rulers.canvas_area().expect("canvas").center();
    drag(&mut h, c + vec2(200.0, 0.0), c + vec2(0.0, 200.0));
    assert!((h.state().view_turn.angle - 90.0).abs() < 1.5);
    assert_eq!(h.state().transform_rect, frame, "the drag only turned");
    r_key(&mut h, false);
    assert_eq!(h.state().shown_tool(), Tool::Transform);
    assert_eq!(doc(&h).history().1, steps, "no step");

    // The handles still answer, where they now show.
    let (a, b) = (screen(&h, 150.0, 150.0), screen(&h, 180.0, 180.0));
    drag(&mut h, a, b);
    let r = h.state().transform_rect.expect("a frame");
    assert!((r.max.x - 180.0).abs() <= 3.0, "{r:?}");
    assert!((r.max.y - 180.0).abs() <= 3.0, "{r:?}");

    // Enter applies it, once.
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(h.state().transform_rect.is_none());
    assert_eq!(doc(&h).active_layer().rect().w, 180);
}

#[test]
fn a_view_tool_tapped_over_a_transform_stays_until_another_is_chosen() {
    let mut h = free_transform();
    let frame = h.state().transform_rect;
    h.key_press(Key::R);
    h.run_steps(2);
    assert_eq!(h.state().shown_tool(), Tool::RotateView);
    let c = h.state().rulers.canvas_area().expect("canvas").center();
    drag(&mut h, c + vec2(200.0, 0.0), c + vec2(0.0, 200.0));
    assert!(h.state().view_turn.angle.abs() > 80.0);
    assert_eq!(h.state().transform_rect, frame);

    // The Zoom tool clicks in; the frame doesn't care.
    let zoom = h.state().zoom;
    h.key_press(Key::Z);
    h.run_steps(2);
    let at = c + vec2(150.0, 150.0);
    drag(&mut h, at, at);
    assert!(h.state().zoom > zoom);
    assert_eq!(h.state().transform_rect, frame);

    // The Hand pans.
    h.key_press(Key::H);
    h.run_steps(2);
    let before = screen(&h, 150.0, 150.0);
    drag(&mut h, at, at + vec2(120.0, 40.0));
    near(screen(&h, 150.0, 150.0), before + vec2(120.0, 40.0), 1.5);
    assert_eq!(h.state().transform_rect, frame);

    // Its own key gives the Move tool back; another tool applies it.
    h.key_press(Key::V);
    h.run_steps(2);
    assert_eq!(h.state().shown_tool(), Tool::Transform);
    assert_eq!(h.state().transform_rect, frame);
    h.key_press(Key::B);
    h.run_steps(3);
    assert_eq!(h.state().tool, Tool::Brush);
    assert!(h.state().transform_rect.is_none());
    assert_eq!(doc(&h).active_layer().rect().w, 150);
}

#[test]
fn view_tools_leave_a_crop_box_without_asking() {
    let mut h = app(Tool::Canvas);
    h.state_mut().rulers.settings.snap = false;
    h.run_steps(1);
    let (a, b) = (screen(&h, 100.0, 80.0), screen(&h, 220.0, 170.0));
    drag(&mut h, a, b);
    let frame = h.state().canvas_rect.expect("a box");
    r_key(&mut h, true);
    assert!(h.state().crop_confirm.is_none(), "no question");
    let c = h.state().rulers.canvas_area().expect("canvas").center();
    drag(&mut h, c + vec2(300.0, 0.0), c + vec2(0.0, 300.0));
    r_key(&mut h, false);
    assert!(h.state().view_turn.angle.abs() > 80.0);
    assert_eq!(h.state().tool, Tool::Canvas);
    assert_eq!(h.state().canvas_rect, Some(frame));
    assert_eq!(h.state().crop.angle, 0.0, "the box wasn't turned");
}

#[test]
fn a_turned_view_only_keeps_the_tiles_on_screen() {
    let mut h = app(Tool::Hand);
    let img = image::RgbaImage::from_pixel(6000, 4000, image::Rgba([255, 255, 255, 255]));
    h.state_mut().doc = Some(Document::new(img, None));
    h.state_mut().zoom = 1.0;
    h.state_mut().fit_pending = None;
    h.state_mut().pan = Vec2::ZERO;
    h.state_mut().view_turn.angle = 45.0;
    for _ in 0..60 {
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let area = h.state().rulers.canvas_area().expect("canvas");
    let (view, turn) = (h.state().last_view.expect("canvas"), h.state().view_turn);
    let (tiles, side) = h.state().tiles_held();
    let rect = |t: (i64, i64)| {
        let at = pos2((t.0 * side) as f32, (t.1 * side) as f32);
        view.rect_to_screen(Rect::from_min_size(at, Vec2::splat(side as f32)))
    };
    for &t in &tiles {
        assert!(turn.shows(area.expand(1.0), rect(t)), "{t:?} is off screen");
    }
    // None missing: every pixel on screen, over the canvas, has its tile.
    for sy in (area.top() as i32..area.bottom() as i32).step_by(40) {
        for sx in (area.left() as i32..area.right() as i32).step_by(40) {
            let ip = image_at(&h, pos2(sx as f32, sy as f32));
            if (0.0..6000.0).contains(&ip.x) && (0.0..4000.0).contains(&ip.y) {
                let t = (ip.x as i64 / side, ip.y as i64 / side);
                assert!(tiles.contains(&t), "{t:?} missing at {sx},{sy}");
            }
        }
    }
}

#[test]
fn a_paste_lands_in_the_part_of_the_canvas_on_screen() {
    let mut h = app(Tool::Hand);
    let img = image::RgbaImage::from_pixel(4000, 4000, image::Rgba([255, 255, 255, 255]));
    h.state_mut().doc = Some(Document::new(img, None));
    h.state_mut().zoom = 1.0;
    h.state_mut().fit_pending = None;
    h.state_mut().view_turn.angle = 45.0;
    h.run_steps(3);
    let area = h.state().rulers.canvas_area().expect("canvas");
    // Only a little of the canvas is shown, against an edge of the window.
    let view = h.state().last_view.expect("canvas");
    let corner = h.state().view_turn.to_screen(view.to_screen(pos2(4000.0, 4000.0)));
    let by = h.state().view_turn.vec_to_upright(area.center() + vec2(300.0, 0.0) - corner);
    h.state_mut().pan += by;
    h.run_steps(3);
    let (x, y) = h.state().paste_center().expect("in view");
    assert!(area.contains(screen(&h, x, y)), "{x} {y}");

    // The canvas out of the window: nothing to aim at.
    h.state_mut().pan += vec2(20000.0, 20000.0);
    h.run_steps(3);
    assert_eq!(h.state().paste_center(), None);
}

#[test]
fn the_center_within_a_rectangle_is_the_polygons_part() {
    let r = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
    let square = |c: Pos2, s: f32| {
        [
            c + vec2(-s, -s),
            c + vec2(s, -s),
            c + vec2(s, s),
            c + vec2(-s, s),
        ]
    };
    let inner = view_turn::center_within(&square(pos2(30.0, 40.0), 5.0), r).expect("inside");
    near(inner, pos2(30.0, 40.0), 0.001);
    let cut = view_turn::center_within(&square(pos2(100.0, 50.0), 20.0), r).expect("half");
    near(cut, pos2(90.0, 50.0), 0.001);
    assert!(view_turn::center_within(&square(pos2(300.0, 50.0), 20.0), r).is_none());
}
