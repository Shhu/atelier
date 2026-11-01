//! The Crop tool's options bar choices, turned box and Delete Cropped
//! Pixels, driven through the real app.

use super::crop_tool::{
    CropOverlay, CropPlan, CropPreset, fit_ratio, fit_turned, straighten_angle,
};
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use image::{Rgba, RgbaImage};

const RED: Rgba<u8> = Rgba([200, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 200, 255]);

/// Red 400 × 300, "Logo" blue 100 × 50 at (100, 100), the Crop tool, no snap.
fn crop_app() -> Harness<'static, AtelierApp> {
    let mut doc = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    doc.add_layer(
        "Logo".into(),
        RgbaImage::from_pixel(100, 50, BLUE),
        (100, 100),
    );
    let mut doc = Some(doc);
    let mut h = Harness::builder()
        .with_size([1400.0, 1000.0])
        .with_step_dt(1.0 / 60.0)
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    app.rulers.settings.snap = false;
    app.set_tool(Tool::Canvas);
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn size(h: &Harness<'static, AtelierApp>) -> (u32, u32) {
    (doc(h).width(), doc(h).height())
}

fn full() -> Rect {
    Rect::from_min_size(Pos2::ZERO, vec2(400.0, 300.0))
}

/// Press at the first point (canvas), through the others, release at the
/// last, with `m` held.
fn drag(h: &mut Harness<'static, AtelierApp>, path: &[(f32, f32)], m: Modifiers) {
    let screen = |h: &Harness<'static, AtelierApp>, (x, y): (f32, f32)| {
        h.state().last_view.expect("canvas shown").to_screen(pos2(x, y))
    };
    h.event(egui::Event::ModifiersChanged(m));
    let start = screen(h, path[0]);
    h.hover_at(start);
    h.run_steps(1);
    let press = |h: &mut Harness<'static, AtelierApp>, pos, pressed| {
        h.event(egui::Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: m,
        });
        h.run_steps(1);
    };
    press(h, start, true);
    let mut end = start;
    for &p in &path[1..] {
        end = screen(h, p);
        h.hover_at(end);
        h.run_steps(1);
    }
    press(h, end, false);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn enter(h: &mut Harness<'static, AtelierApp>) {
    h.key_press(Key::Enter);
    h.run_steps(2);
}

fn draw_box(h: &mut Harness<'static, AtelierApp>) {
    drag(h, &[(50.0, 50.0), (100.0, 85.0), (150.0, 120.0)], Modifiers::NONE);
    assert!(h.state().crop_box_pending());
}

#[test]
fn presets_shape_the_box() {
    let mut h = crop_app();
    let app = h.state_mut();
    app.crop_pick_preset(CropPreset::Square, full());
    assert_eq!(
        app.canvas_rect,
        Some(Rect::from_min_max(pos2(50.0, 0.0), pos2(350.0, 300.0)))
    );
    app.crop_pick_preset(CropPreset::R16x9, full());
    assert_eq!(app.canvas_rect.map(|r| r.height().round()), Some(225.0));
    // Original: the whole canvas, nothing to ask about.
    app.crop_pick_preset(CropPreset::Original, full());
    assert_eq!(app.canvas_rect, Some(full()));
    assert!(!app.crop_box_pending());
    assert_eq!((app.crop.w, app.crop.h), (400.0, 300.0));
    // Ratio: free again, its fields empty.
    app.crop_pick_preset(CropPreset::Ratio, full());
    assert_eq!(app.crop.ratio(), None);
}

#[test]
fn swap_trades_the_values_and_the_box_sides() {
    let mut h = crop_app();
    let app = h.state_mut();
    app.crop_pick_preset(CropPreset::R4x5, full());
    app.crop_swap(full());
    assert_eq!((app.crop.w, app.crop.h), (5.0, 4.0));
    assert_eq!(app.crop.preset, CropPreset::Ratio, "no longer the preset");
    assert_eq!(
        app.canvas_rect,
        Some(Rect::from_min_max(pos2(13.0, 0.0), pos2(388.0, 300.0)))
    );
    // Free: the box itself turns.
    app.crop_clear();
    app.canvas_rect = Some(Rect::from_min_size(pos2(100.0, 100.0), vec2(80.0, 40.0)));
    app.crop_swap(full());
    let r = app.canvas_rect.expect("a box");
    assert_eq!((r.width(), r.height()), (40.0, 80.0));
    assert_eq!(r.center(), pos2(140.0, 120.0));
}

#[test]
fn a_ratio_constrains_drawing_and_resizing() {
    let mut h = crop_app();
    h.state_mut().crop_pick_preset(CropPreset::Square, full());
    h.state_mut().reset_pending();
    h.run_steps(1);
    drag(&mut h, &[(50.0, 50.0), (100.0, 80.0), (150.0, 90.0)], Modifiers::NONE);
    let r = h.state().canvas_rect.expect("a box");
    assert_eq!((r.width(), r.height()), (100.0, 100.0));
    // Its corner dragged: still square, the opposite one in place.
    drag(&mut h, &[(150.0, 150.0), (170.0, 170.0), (200.0, 180.0)], Modifiers::NONE);
    let r = h.state().canvas_rect.expect("a box");
    assert_eq!(r.width(), r.height());
    assert!(r.width() > 100.0);
    assert_eq!(r.min, pos2(50.0, 50.0));
}

#[test]
fn a_drag_outside_turns_the_box_and_applies_as_one_step() {
    let mut h = crop_app();
    draw_box(&mut h);
    let steps = doc(&h).history().1;
    // From the east of its center (100, 85) to the south-east: 45°.
    drag(&mut h, &[(200.0, 85.0), (210.0, 120.0), (200.0, 185.0)], Modifiers::NONE);
    assert!((h.state().crop.angle - 45.0).abs() < 0.01, "{}", h.state().crop.angle);
    let r = h.state().canvas_rect.expect("a box");
    assert_eq!((r.width(), r.height()), (100.0, 70.0), "room on the image");
    assert_eq!(r.center(), pos2(100.0, 85.0));
    assert!(h.state().crop_box_pending());
    // Leaving the tool still asks.
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::Canvas);
    assert!(h.state().crop_confirm.is_some());
    h.state_mut().crop_answered(true);
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::Brush);
    assert_eq!(size(&h), (100, 70));
    assert_eq!(h.state().crop.angle, 0.0);
    assert_eq!(doc(&h).history().1, steps + 1, "one step");
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    assert_eq!(size(&h), (400, 300));
}

#[test]
fn turning_the_whole_canvas_shrinks_the_box_onto_the_image() {
    let mut h = crop_app();
    // Outside the canvas, no box yet.
    drag(&mut h, &[(450.0, 150.0), (452.0, 170.0), (449.0, 180.0)], Modifiers::NONE);
    let angle = h.state().crop.angle;
    assert!(angle > 5.0 && angle < 20.0, "{angle}");
    let r = h.state().canvas_rect.expect("a box");
    assert!(r.width() < 400.0 && r.height() < 300.0);
    assert!(r.center().distance(full().center()) <= 1.0);
    let (w, h_) = rotated_size(r.width(), r.height(), angle);
    assert!(w <= 400.0 && h_ <= 300.0, "the corners stay on the image");
}

#[test]
fn a_box_is_moved_inside_and_never_redrawn_outside() {
    let mut h = crop_app();
    draw_box(&mut h);
    drag(&mut h, &[(100.0, 85.0), (110.0, 90.0), (130.0, 105.0)], Modifiers::NONE);
    let r = h.state().canvas_rect.expect("a box");
    assert_eq!(r.min, pos2(80.0, 70.0));
    assert_eq!(h.state().crop.angle, 0.0);
    // Esc, and a new one can be drawn.
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(h.state().canvas_rect.is_none());
    assert_eq!(h.state().crop.angle, 0.0);
    draw_box(&mut h);
}

#[test]
fn straighten_levels_the_line_drawn() {
    let mut h = crop_app();
    h.state_mut().crop.straighten = true;
    h.run_steps(1);
    drag(&mut h, &[(50.0, 50.0), (100.0, 55.0), (150.0, 60.0)], Modifiers::NONE);
    let want = (10.0f32 / 100.0).atan().to_degrees();
    assert!((h.state().crop.angle - want).abs() < 0.01);
    assert!(!h.state().crop.straighten, "one line");
    let r = h.state().canvas_rect.expect("a box");
    let (w, h_) = rotated_size(r.width(), r.height(), want);
    assert!(w <= 400.0 && h_ <= 300.0);
    // A slip of the hand does nothing but leave it.
    let mut h = crop_app();
    h.state_mut().crop.straighten = true;
    h.run_steps(1);
    drag(&mut h, &[(50.0, 50.0), (51.0, 51.0), (52.0, 52.0)], Modifiers::NONE);
    assert_eq!(h.state().crop.angle, 0.0);
    assert!(h.state().canvas_rect.is_none());
}

#[test]
fn straighten_angle_goes_to_the_nearest_axis() {
    let a = |x, y| straighten_angle(Pos2::ZERO, pos2(x, y));
    assert!((a(100.0, 0.0)).abs() < 1e-3);
    assert!((a(-100.0, 0.0)).abs() < 1e-3);
    assert!((a(0.0, 100.0)).abs() < 1e-3, "a vertical line is level too");
    assert!((a(100.0, 10.0) - 5.71).abs() < 0.01);
    assert!((a(-10.0, 100.0) - 5.71).abs() < 0.01);
    assert!((a(10.0, 100.0) + 5.71).abs() < 0.01);
}

#[test]
fn the_turn_keeps_what_the_box_covered() {
    // A block 30 px east of the box's center.
    let mut doc = Document::new(RgbaImage::from_pixel(200, 200, RED), None);
    doc.add_layer("Dot".into(), RgbaImage::from_pixel(6, 6, BLUE), (127, 97));
    let mut doc = Some(doc);
    let mut h = Harness::builder()
        .with_size([1000.0, 800.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    let steps = self::doc(&h).history().1;
    let rect = PixelRect { x: 60, y: 60, w: 80, h: 80 };
    h.state_mut().apply_crop(CropPlan { angle: 30.0, ..CropPlan::plain(rect) });
    let flat = self::doc(&h).flatten();
    assert_eq!(flat.dimensions(), (80, 80));
    // The box turned clockwise: its contents turn the other way.
    let (dx, dy) = (30.0 * 30f32.to_radians().cos(), -30.0 * 30f32.to_radians().sin());
    let at = flat.get_pixel((40.0 + dx) as u32, (40.0 + dy) as u32);
    assert_eq!(at.0[2], 200, "the block, moved by the turn");
    assert_eq!(flat.get_pixel(70, 40).0[2], 0, "not where it was");
    assert_eq!(self::doc(&h).history().1, steps + 1, "one step");
}

#[test]
fn delete_cropped_pixels_is_followed_only_by_the_tool() {
    let mut h = crop_app();
    h.state_mut().crop.delete_cropped = true;
    draw_box(&mut h);
    enter(&mut h);
    assert_eq!(size(&h), (100, 70));
    let d = doc(&h);
    assert_eq!(d.layers[0].rect(), PixelRect { x: 0, y: 0, w: 100, h: 70 }, "cut");
    assert_eq!(d.layers[1].rect(), PixelRect { x: 50, y: 50, w: 50, h: 20 }, "cut");
    // Off: the pixels stay.
    let mut h = crop_app();
    draw_box(&mut h);
    enter(&mut h);
    let d = doc(&h);
    assert_eq!(d.layers[0].rect(), PixelRect { x: -50, y: -50, w: 400, h: 300 });
    // Image › Crop has no such option.
    let mut h = crop_app();
    h.state_mut().crop.delete_cropped = true;
    h.state_mut().selection = Some(PixelRect { x: 10, y: 10, w: 50, h: 50 }.into());
    h.state_mut().run(Action::CropToSelection);
    h.run_steps(1);
    assert_eq!(size(&h), (50, 50));
    assert_eq!(doc(&h).layers[0].rect(), PixelRect { x: -10, y: -10, w: 400, h: 300 });
}

#[test]
fn delete_cropped_pixels_leaves_smart_objects_and_texts_whole() {
    let mut d = Document::new(RgbaImage::from_pixel(100, 100, RED), None);
    assert!(d.convert_to_smart(&[0]));
    let before = d.layers[0].image().dimensions();
    d.crop_area(PixelRect { x: 10, y: 10, w: 20, h: 20 }, true);
    assert!(d.layers[0].is_smart());
    assert_eq!(d.layers[0].image().dimensions(), before);
}

#[test]
fn size_preset_resamples_and_stays_one_step() {
    let mut h = crop_app();
    let steps = doc(&h).history().1;
    let app = h.state_mut();
    app.crop.preset = CropPreset::Size;
    (app.crop.w, app.crop.h, app.crop.resolution) = (200.0, 100.0, 300.0);
    app.crop_values_edited(full());
    let r = app.canvas_rect.expect("a box");
    assert_eq!(r.width() / r.height(), 2.0);
    enter(&mut h);
    assert_eq!(size(&h), (200, 100));
    assert_eq!(doc(&h).history().1, steps + 1);
    let kept = doc(&h).file_metadata.image.as_ref().and_then(|m| m.density);
    assert_eq!(kept, Some((300.0, 300.0)));
}

#[test]
fn undo_drops_the_box_and_its_turn() {
    let mut h = crop_app();
    draw_box(&mut h);
    drag(&mut h, &[(200.0, 85.0), (210.0, 120.0), (200.0, 185.0)], Modifiers::NONE);
    h.state_mut().run(Action::Undo);
    h.run_steps(1);
    assert!(h.state().canvas_rect.is_none());
    assert_eq!(h.state().crop.angle, 0.0);
    assert_eq!(size(&h), (400, 300));
}

#[test]
fn the_crop_is_recorded_with_its_options_and_old_steps_still_read() {
    use crate::actions::Recorded;
    let old: Recorded =
        serde_json::from_str(r#"{"command":"crop","rect":[1,2,3,4]}"#).expect("an old recording");
    let Recorded::Crop { angle, delete, size: resampled, .. } = old else {
        panic!("a crop");
    };
    assert_eq!((angle, delete, resampled), (0.0, false, None));
    // A turned, deleting crop goes through the file and plays back alike.
    let new = Recorded::Crop {
        rect: [60, 60, 80, 80],
        angle: 30.0,
        delete: true,
        size: None,
        resolution: 0.0,
    };
    let text = serde_json::to_string(&new).expect("written");
    assert_eq!(serde_json::from_str::<Recorded>(&text).expect("read"), new);
    let mut h = crop_app();
    h.state_mut().play_step(&new, false).expect("played");
    h.run_steps(1);
    assert_eq!(size(&h), (80, 80));
    assert_eq!(doc(&h).layers[0].rect(), PixelRect { x: 0, y: 0, w: 80, h: 80 });
}

/// A turned box over the image, to look at: `ATELIER_CAPTURE=out.png`, its
/// turn in `ATELIER_CROP_ANGLE` (by default what a drag gives).
#[test]
#[ignore = "needs a graphics renderer"]
fn capture_turned_box() {
    let mut h = crop_app();
    draw_box(&mut h);
    drag(&mut h, &[(200.0, 85.0), (210.0, 100.0), (200.0, 125.0)], Modifiers::NONE);
    if let Some(angle) = std::env::var("ATELIER_CROP_ANGLE").ok().and_then(|a| a.parse().ok()) {
        h.state_mut().crop.angle = angle;
        h.run_steps(2);
    }
    let image = h.render().expect("a renderer");
    let out = std::env::var("ATELIER_CAPTURE").unwrap_or_else(|_| "capture.png".into());
    image.save(out).expect("written");
}

#[test]
fn the_options_come_back_next_session() {
    let mut h = crop_app();
    let a = h.state_mut();
    a.crop.preset = CropPreset::R16x9;
    (a.crop.w, a.crop.h) = (16.0, 9.0);
    a.crop.overlay = CropOverlay::Grid;
    a.crop.shield = 0.4;
    a.crop.delete_cropped = true;
    a.crop.angle = 12.0;
    let saved = a.session_settings();
    let mut h = crop_app();
    let b = h.state_mut();
    b.restore_session(&saved);
    assert_eq!(b.crop.preset, CropPreset::R16x9);
    assert_eq!(b.crop.ratio(), Some(16.0 / 9.0));
    assert_eq!(b.crop.overlay, CropOverlay::Grid);
    assert_eq!(b.crop.shield, 0.4);
    assert!(b.crop.delete_cropped);
    assert_eq!(b.crop.angle, 0.0, "the box's turn isn't an option");
}

#[test]
fn geometry_helpers() {
    assert_eq!(
        fit_ratio(full(), 1.0),
        Rect::from_min_max(pos2(50.0, 0.0), pos2(350.0, 300.0))
    );
    // Not turned, or room around it: untouched.
    let r = Rect::from_min_size(pos2(50.0, 50.0), vec2(100.0, 70.0));
    assert_eq!(fit_turned(r, 0.0, full()), r);
    assert_eq!(fit_turned(r, 45.0, full()), r);
    // Past the image already: the canvas is being enlarged.
    let past = Rect::from_min_max(pos2(-10.0, 0.0), pos2(100.0, 100.0));
    assert_eq!(fit_turned(past, 10.0, full()), past);
}

#[test]
fn crop_options_fit_in_a_narrow_window() {
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Fr] {
        crate::i18n::set_lang(lang);
        let mut doc = Some(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
        let mut h = Harness::builder()
            .with_size([660.0, 700.0])
            .build_eframe(move |cc| {
                let mut app = AtelierApp::new(cc, None);
                app.doc = doc.take();
                app
            });
        h.state_mut().crop.preset = CropPreset::Size;
        h.state_mut().crop.overlay = CropOverlay::Grid;
        h.state_mut().set_tool(Tool::Canvas);
        h.run_steps(3);
        let check = t!("crop8-delete-cropped");
        let r = h.get_by_label(&check).rect();
        assert!(r.right() <= 660.0, "{lang:?}: {r:?}");
    }
    crate::i18n::set_lang(crate::i18n::Lang::En);
}

fn typed_text(h: &mut Harness<'static, AtelierApp>, at: (f32, f32), s: &str) -> bool {
    if h.state_mut().text_engine().families().is_empty() {
        return false;
    }
    h.state_mut().tool = Tool::Text;
    h.run_steps(1);
    drag(h, &[at, at], Modifiers::NONE);
    h.event(egui::Event::Text(s.into()));
    h.run_steps(1);
    h.key_press(Key::Escape);
    h.run_steps(1);
    h.state_mut().tool = Tool::Canvas;
    h.run_steps(1);
    true
}

#[test]
fn a_text_layer_stays_a_text_through_a_turned_crop() {
    let mut h = crop_app();
    if !typed_text(&mut h, (180.0, 150.0), "Turned") {
        return;
    }
    let before = doc(&h).active_layer().id;
    let steps = doc(&h).history().1;
    let keep = doc(&h).active_layer().text.clone().expect("a text layer");
    let rect = PixelRect { x: 100, y: 75, w: 200, h: 150 };
    let plan = CropPlan { angle: 30.0, delete: true, ..CropPlan::plain(rect) };
    h.state_mut().apply_crop(plan);
    h.run_steps(2);
    assert_eq!(doc(&h).history().1, steps + 1, "one step");
    let layer = doc(&h).layers.iter().find(|l| l.id == before).expect("the layer");
    let t = layer.text.as_ref().expect("still a text");
    assert_eq!(t.text, "Turned");
    // The box turned clockwise: the text turned the other way, around it.
    let (s, c) = (-30f32).to_radians().sin_cos();
    let [a, b, cc, d] = t.transform;
    assert!((a - c).abs() < 1e-3 && (b + s).abs() < 1e-3, "{:?}", t.transform);
    assert!((cc - s).abs() < 1e-3 && (d - c).abs() < 1e-3, "{:?}", t.transform);
    // (180, 150) is 20 px left of the box's center (200, 150).
    let origin = type_tool::text_to_canvas(layer.offset, t, (0.0, 0.0));
    let want = pos2(100.0 - 20.0 * c, 75.0 - 20.0 * s);
    assert!(origin.distance(want) < 1.5, "{origin:?} != {want:?}");
    // Still shown there.
    assert!(layer.content_bounds().is_some_and(|b| b.w > 0 && b.h > 0));
    // Undone: the text as it was.
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    let layer = doc(&h).layers.iter().find(|l| l.id == before).expect("the layer");
    assert_eq!(layer.text.as_ref().expect("a text").transform, keep.transform);
    assert_eq!(size(&h), (400, 300));
}

#[test]
fn delete_cropped_pixels_leaves_texts_shapes_and_artboards_whole() {
    use crate::vector::{Bounds, Live, Paint, Shape, ShapeKind, ShapeStroke, pt};
    // A shape overhanging the crop on all sides.
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    let live = Live::new(ShapeKind::Rectangle, Bounds::of(pt(10.0, 10.0), pt(390.0, 290.0)));
    let shape = Shape::live(live, Paint::Color([0, 255, 0]), ShapeStroke::default());
    d.add_shape_layer("Shape".into(), "Shape", shape);
    let shape_at = d.active;
    let bounds = d.layers[shape_at].content_bounds().expect("a shape");
    d.add_layer("Plain".into(), RgbaImage::from_pixel(300, 200, BLUE), (50, 50));
    let plain_at = d.active;
    d.crop_area(PixelRect { x: 100, y: 100, w: 50, h: 50 }, true);
    let l = &d.layers[shape_at];
    assert!(l.shape.is_some(), "still a shape");
    let moved = l.content_bounds().expect("whole");
    assert_eq!((moved.w, moved.h, moved.x, moved.y), (bounds.w, bounds.h, bounds.x - 100, bounds.y - 100));
    assert_eq!(d.layers[plain_at].rect(), PixelRect { x: 0, y: 0, w: 50, h: 50 }, "pixels are cut");

    // Artboards: the board and what it holds.
    let mut d = Document::with_artboard(400, 300, crate::document::ArtboardBackground::White);
    d.active = 1;
    d.add_layer("Inside".into(), RgbaImage::from_pixel(300, 200, BLUE), (50, 50));
    let inside = d.active;
    let board = d.artboard_of(inside).expect("in the board");
    d.crop_area(PixelRect { x: 100, y: 100, w: 50, h: 50 }, true);
    assert_eq!(d.layers[board].artboard.expect("a board").size, (400, 300));
    assert_eq!(
        d.layers[inside].rect(),
        PixelRect { x: -50, y: -50, w: 300, h: 200 },
        "what a board holds stays"
    );
}

#[test]
fn delete_cropped_pixels_on_a_text_keeps_all_its_letters() {
    let mut h = crop_app();
    if !typed_text(&mut h, (20.0, 150.0), "A long text over the crop") {
        return;
    }
    let (id, image) = {
        let l = doc(&h).active_layer();
        (l.id, l.image().clone())
    };
    let r = PixelRect { x: 40, y: 100, w: 30, h: 60 };
    h.state_mut().apply_crop(CropPlan { delete: true, ..CropPlan::plain(r) });
    h.run_steps(2);
    let l = doc(&h).layers.iter().find(|l| l.id == id).expect("the layer");
    assert!(l.text.is_some());
    assert_eq!(*l.image(), image, "not cut");
}

#[test]
fn trim_and_crop_to_element_ignore_the_tools_options() {
    for action in [Action::CropToContent, Action::CropToElement] {
        let mut h = crop_app();
        let app = h.state_mut();
        // A transparent canvas around the blue block.
        let d = app.doc.as_mut().unwrap();
        d.layers[0].replace_pixels(RgbaImage::new(400, 300));
        d.active = 1;
        app.crop.delete_cropped = true;
        app.crop.preset = CropPreset::Size;
        (app.crop.w, app.crop.h, app.crop.resolution) = (30.0, 30.0, 300.0);
        app.crop.angle = 25.0;
        let steps = doc(&h).history().1;
        h.state_mut().run(action);
        h.run_steps(2);
        assert_eq!(size(&h), (100, 50), "{action:?}: not resampled, not turned");
        assert_eq!(
            doc(&h).layers[0].rect(),
            PixelRect { x: -100, y: -100, w: 400, h: 300 },
            "{action:?}: nothing deleted"
        );
        assert_eq!(doc(&h).history().1, steps + 1, "{action:?}");
        assert!(doc(&h).file_metadata.image.as_ref().is_none_or(|m| m.density.is_none()), "{action:?}");
    }
}

/// How many pieces of a mesh cover `p`.
fn covered(mesh: &egui::epaint::Mesh, p: Pos2) -> usize {
    let cross = |a: Pos2, b: Pos2| (b - a).x * (p - a).y - (b - a).y * (p - a).x;
    mesh.indices
        .chunks_exact(3)
        .filter(|t| {
            let [a, b, c] = [0, 1, 2].map(|i| mesh.vertices[t[i] as usize].pos);
            let (x, y, z) = (cross(a, b), cross(b, c), cross(c, a));
            (x > 0.0 && y > 0.0 && z > 0.0) || (x < 0.0 && y < 0.0 && z < 0.0)
        })
        .count()
}

#[test]
fn the_shield_covers_all_but_the_box_at_any_turn() {
    let frame = Rect::from_min_size(pos2(10.0, 20.0), vec2(400.0, 300.0));
    let boxes = [
        Rect::from_min_size(pos2(60.0, 70.0), vec2(100.0, 70.0)),
        Rect::from_min_size(pos2(100.0, 30.0), vec2(250.0, 20.0)),
        Rect::from_min_size(pos2(160.0, 120.0), vec2(30.0, 180.0)),
        // Past the frame on a side.
        Rect::from_min_size(pos2(300.0, 200.0), vec2(160.0, 100.0)),
    ];
    let mut angle = -180.0f32;
    while angle <= 180.0 {
        for r in boxes {
            let hole = super::crop_tool::turned_corners(r, angle);
            let mesh = super::crop_tool::shield_mesh(frame, hole, Color32::BLACK);
            let inside = |p: Pos2| {
                let q = canvas::rotate_around(p, r.center(), -angle);
                r.contains(q)
            };
            let near_edge = |p: Pos2| {
                let q = canvas::rotate_around(p, r.center(), -angle);
                r.expand(0.6).contains(q) && !r.shrink(0.6).contains(q)
            };
            for ix in 0..=60 {
                for iy in 0..=45 {
                    let p = frame.min + vec2(ix as f32 * 400.0 / 60.0 + 0.13, iy as f32 * 300.0 / 45.0 + 0.17);
                    if near_edge(p) || !frame.shrink(0.5).contains(p) {
                        continue;
                    }
                    let want = usize::from(!inside(p));
                    assert_eq!(covered(&mesh, p), want, "{angle}° {r:?} at {p:?}");
                }
            }
        }
        angle += 5.0;
    }
}

/// Every piece of the bar: fully on screen.
fn crop_bar_on_screen(h: &Harness<'static, AtelierApp>, what: &str) {
    use egui_kittest::kittest::NodeT as _;
    let canvas = h.state().rulers.canvas_area().expect("canvas");
    let mut seen = 0;
    for node in h.query_all(egui_kittest::kittest::by().predicate(|_| true)) {
        let Some(b) = node.accesskit_node().bounding_box() else {
            continue;
        };
        let r = Rect::from_min_max(pos2(b.x0 as f32, b.y0 as f32), pos2(b.x1 as f32, b.y1 as f32));
        // Between the menus and the canvas.
        if r.top() < 30.0 || r.bottom() > canvas.top() + 1.0 || r.width() >= 640.0 {
            continue;
        }
        seen += 1;
        assert!(r.left() >= -0.5 && r.right() <= 660.5, "{what}: {:?} {r:?}", node.accesskit_node().label());
    }
    assert!(seen >= 5, "{what}: {seen} pieces");
}

#[test]
fn the_whole_bar_stays_on_screen_at_660_for_every_preset() {
    for lang in [crate::i18n::Lang::En, crate::i18n::Lang::Fr] {
        crate::i18n::set_lang(lang);
        for preset in CropPreset::ALL {
            let mut doc = Some(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
            let mut h = Harness::builder()
                .with_size([660.0, 700.0])
                .build_eframe(move |cc| {
                    let mut app = AtelierApp::new(cc, None);
                    app.doc = doc.take();
                    app
                });
            h.state_mut().set_tool(Tool::Canvas);
            h.run_steps(2);
            h.state_mut().crop_pick_preset(preset, full());
            // The longest it gets: turned, a value in each field.
            h.state_mut().crop.angle = -123.4;
            if preset == CropPreset::Size {
                (h.state_mut().crop.w, h.state_mut().crop.h) = (12000.0, 12000.0);
                h.state_mut().crop.resolution = 1200.0;
            }
            h.run_steps(4);
            let what = format!("{lang:?} {preset:?}");
            crop_bar_on_screen(&h, &what);
            for name in [
                t!("crop8-delete-cropped"),
                t!("crop8-clear"),
                t!("crop8-straighten-tip"),
                t!("options-apply"),
                t!("dialog-cancel"),
            ] {
                let r = h.get_by_label(&name).rect();
                let canvas = h.state().rulers.canvas_area().expect("canvas");
                assert!(r.right() <= 660.0 && r.bottom() <= canvas.top() + 1.0, "{what} {name}: {r:?}");
            }
            if preset == CropPreset::Size {
                let r = h.get_by_label(&t!("crop8-resolution")).rect();
                assert!(r.right() <= 660.0, "{what}: {r:?}");
            }
        }
    }
    crate::i18n::set_lang(crate::i18n::Lang::En);
}

/// Applying a turned crop at 6000 × 4000: `cargo test --release crop_speed
/// -- --ignored --nocapture`.
#[test]
#[ignore = "timing"]
fn turned_crop_speed() {
    let image = RgbaImage::from_fn(6000, 4000, |x, y| Rgba([(x % 256) as u8, (y % 256) as u8, 40, 255]));
    let mut d = Document::new(image, None);
    let top = RgbaImage::from_pixel(3000, 2000, Rgba([10, 200, 10, 180]));
    d.add_layer("Top".into(), top, (1500, 1000));
    let mut d = Some(d);
    let mut h = Harness::builder().with_size([1400.0, 1000.0]).build_eframe(move |cc| {
        let mut app = AtelierApp::new(cc, None);
        app.doc = d.take();
        app
    });
    h.run_steps(2);
    let plan = CropPlan {
        angle: 12.5,
        ..CropPlan::plain(PixelRect { x: 1000, y: 800, w: 4000, h: 2400 })
    };
    let t = std::time::Instant::now();
    h.state_mut().apply_crop(plan);
    println!("turned crop (UI thread): {:?}", t.elapsed());
    assert_eq!(size(&h), (4000, 2400));
}
