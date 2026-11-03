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

    // Artboards: the board is cut to the box, what it holds to the board
    // (it shows only there).
    let mut d = Document::with_artboard(400, 300, crate::document::ArtboardBackground::White);
    d.active = 1;
    d.add_layer("Inside".into(), RgbaImage::from_pixel(300, 200, BLUE), (50, 50));
    let inside = d.active;
    let board = d.artboard_of(inside).expect("in the board");
    let want = image::imageops::crop_imm(&d.flatten(), 100, 100, 50, 50).to_image();
    d.crop_area(PixelRect { x: 100, y: 100, w: 50, h: 50 }, true);
    assert_eq!(d.layers[board].artboard.expect("a board").size, (50, 50));
    assert_eq!(d.layers[inside].rect(), PixelRect { x: 0, y: 0, w: 50, h: 50 }, "cut to its board");
    assert_eq!(pixel_diff(&d.flatten(), &want), 0);
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
    println!("turned crop (UI thread, it goes aside): {:?}", t.elapsed());
    settle(&mut h);
    assert_eq!(size(&h), (4000, 2400));
}

// ---------- After the review of the crop options ----------

use crate::document::PixelRect;

fn app_with(doc: Document) -> Harness<'static, AtelierApp> {
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

fn settle(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..4000 {
        h.run_steps(1);
        if !h.state().transform_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the job never landed");
}

/// `n` layers of `w` × `h` with their own buffers.
fn layered(n: usize, w: u32, h: u32) -> Document {
    let image = RgbaImage::from_fn(w, h, |x, y| Rgba([(x % 251) as u8, (y % 241) as u8, 40, 255]));
    let mut d = Document::new(image.clone(), None);
    for i in 1..n {
        let mut own = image.clone();
        own.put_pixel(0, 0, Rgba([i as u8, 0, 0, 255]));
        d.add_layer(format!("L{i}"), own, (0, 0));
    }
    d
}

fn pixel_diff(a: &RgbaImage, b: &RgbaImage) -> u32 {
    assert_eq!(a.dimensions(), b.dimensions());
    let far = |(p, q): (&Rgba<u8>, &Rgba<u8>)| p.0.iter().zip(q.0).any(|(x, y)| x.abs_diff(y) > 2);
    a.pixels().zip(b.pixels()).filter(|&pq| far(pq)).count() as u32
}

fn density(h: &Harness<'static, AtelierApp>) -> Option<(f64, f64)> {
    doc(h).file_metadata.image.as_ref().and_then(|m| m.density)
}

/// A key pressed and released, through the app's input hook.
fn key(h: &mut Harness<'static, AtelierApp>, key: Key, modifiers: Modifiers) {
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        });
        frame(h);
    }
}

#[test]
fn a_ratio_then_w_h_resolution_never_makes_its_values_pixels() {
    let mut h = crop_app();
    let app = h.state_mut();
    app.crop_pick_preset(CropPreset::R16x9, full());
    app.crop_pick_preset(CropPreset::Size, full());
    assert_eq!((app.crop.w, app.crop.h), (400.0, 225.0));
    let plan = app.crop_plan().expect("a box");
    assert_eq!((plan.rect.w, plan.rect.h), (400, 225));
    enter(&mut h);
    assert_eq!(size(&h), (400, 225));
}

#[test]
fn a_drawn_box_stays_through_w_h_resolution_and_crops_one_to_one() {
    let mut h = crop_app();
    draw_box(&mut h);
    let drawn = h.state().canvas_rect.expect("a box");
    let flat = doc(&h).flatten();
    h.state_mut().crop_pick_preset(CropPreset::Size, full());
    assert_eq!(h.state().canvas_rect, Some(drawn), "where it was drawn");
    assert_eq!((h.state().crop.w, h.state().crop.h), (drawn.width(), drawn.height()));
    enter(&mut h);
    let (x, y) = (drawn.min.x as u32, drawn.min.y as u32);
    let want = image::imageops::crop_imm(&flat, x, y, drawn.width() as u32, drawn.height() as u32);
    assert_eq!(pixel_diff(&doc(&h).flatten(), &want.to_image()), 0, "not resampled");
}

#[test]
fn fit_to_visible_pixels_keeps_the_ratio_and_never_stretches() {
    let mut d = Document::new(RgbaImage::new(400, 300), None);
    d.add_layer("Logo".into(), RgbaImage::from_pixel(100, 50, BLUE), (100, 100));
    let mut h = app_with(d);
    let app = h.state_mut();
    app.crop_fit_visible();
    let bounds = Rect::from_min_size(pos2(100.0, 100.0), vec2(100.0, 50.0));
    assert_eq!(app.canvas_rect, Some(bounds));
    app.crop.preset = CropPreset::Size;
    (app.crop.w, app.crop.h) = (200.0, 200.0);
    app.crop_fit_visible();
    let r = app.canvas_rect.expect("a box");
    assert_eq!(r.width(), r.height(), "the shape W and H ask for");
    enter(&mut h);
    assert_eq!(size(&h), (200, 200));
}

#[test]
fn the_resolution_is_a_step_of_its_own_and_saved() {
    let mut h = crop_app();
    h.state_mut().doc.as_mut().unwrap().mark_saved();
    let steps = doc(&h).history().1;
    let app = h.state_mut();
    app.crop_pick_preset(CropPreset::Size, full());
    assert!(!app.crop_box_pending(), "same size, no resolution: nothing asked");
    app.crop.resolution = 300.0;
    assert!(app.crop_box_pending(), "Apply, Enter and the prompt agree");
    enter(&mut h);
    assert_eq!(density(&h), Some((300.0, 300.0)));
    assert_eq!(doc(&h).history().1, steps + 1, "a step");
    assert!(doc(&h).modified, "the close prompt knows");
    let dir = std::env::temp_dir().join(format!("atelier-crop-res-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (psd, native) = (dir.join("a.psd"), dir.join("a.atelier"));
    h.state_mut().doc.as_mut().unwrap().save(&native).unwrap();
    let Ok(back) = Document::open(&native) else { panic!("opens") };
    let back = back.doc;
    assert_eq!(back.file_metadata.image.and_then(|m| m.density), Some((300.0, 300.0)));
    h.state_mut().doc.as_mut().unwrap().save(&psd).unwrap();
    let Ok(back) = Document::open(&psd) else { panic!("opens") };
    let back = back.doc;
    let resources = back.file_metadata.psd_resources.clone();
    let res = &resources.iter().find(|r| r.id == 1005).expect("resolution").data;
    assert_eq!(res[..4], (300u32 << 16).to_be_bytes(), "the PSD says so");
    assert_eq!(res[8..12], (300u32 << 16).to_be_bytes());
    let _ = std::fs::remove_dir_all(dir);
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    assert_eq!(density(&h), None, "undo gives it back");
}

#[test]
fn with_values_and_no_box_enter_and_apply_agree() {
    let mut h = crop_app();
    let app = h.state_mut();
    app.crop.preset = CropPreset::Size;
    (app.crop.w, app.crop.h) = (200.0, 100.0);
    assert_eq!(app.canvas_rect, None);
    assert!(!app.crop_box_pending(), "nothing lit");
    enter(&mut h);
    assert_eq!(size(&h), (400, 300), "nothing applied");
    // Typed, the box comes, and both apply; the prompt asks.
    h.state_mut().crop_values_edited(full());
    assert!(h.state().crop_box_pending());
    enter(&mut h);
    assert_eq!(size(&h), (200, 100));
}

#[test]
fn artboards_are_cut_by_the_box_and_the_crop_lasts() {
    let mut d = Document::with_artboard(400, 300, crate::document::ArtboardBackground::White);
    d.active = 1;
    d.add_layer("Inside".into(), RgbaImage::from_pixel(300, 200, BLUE), (50, 50));
    let board = d.artboard_of(d.active).unwrap();
    let mut h = app_with(d);
    h.state_mut().apply_crop(CropPlan::plain(PixelRect { x: 100, y: 100, w: 50, h: 50 }));
    h.run_steps(2);
    assert_eq!(size(&h), (50, 50));
    let cut = PixelRect { x: 0, y: 0, w: 50, h: 50 };
    assert_eq!(doc(&h).layers[board].artboard_rect(), Some(cut));
    h.state_mut().refit_artboards();
    assert_eq!(size(&h), (50, 50), "a later edit of a board doesn't bring it back");
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    assert_eq!(size(&h), (400, 300));
    let whole = PixelRect { x: 0, y: 0, w: 400, h: 300 };
    assert_eq!(doc(&h).layers[board].artboard_rect(), Some(whole));
}

#[test]
fn an_artboard_the_box_misses_stays_as_it_is() {
    use crate::document::ArtboardBackground as B;
    let mut d = Document::with_artboard(100, 100, B::White);
    let two = PixelRect { x: 200, y: 0, w: 100, h: 100 };
    d.add_artboard("two".into(), "Two".into(), two, B::White);
    let mut h = app_with(d);
    assert_eq!(size(&h), (300, 100));
    h.state_mut().apply_crop(CropPlan::plain(PixelRect { x: 10, y: 10, w: 50, h: 50 }));
    h.run_steps(2);
    let rects: Vec<_> = doc(&h).layers.iter().filter_map(|l| l.artboard_rect()).collect();
    assert!(rects.iter().any(|r| (r.w, r.h) == (50, 50)), "{rects:?}");
    assert!(rects.iter().any(|r| (r.w, r.h) == (100, 100)), "{rects:?}");
    let settled = size(&h);
    h.state_mut().refit_artboards();
    assert_eq!(size(&h), settled, "the canvas is the box of the boards");
}

#[test]
fn delete_cropped_pixels_doesnt_change_what_styles_draw() {
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    d.add_layer("fx".into(), RgbaImage::from_pixel(300, 200, BLUE), (50, 50));
    let l = d.layers.len() - 1;
    d.layers[l].effects.inner_shadow.enabled = true;
    d.layers[l].effects.inner_shadow.blur = 20.0;
    let want = image::imageops::crop_imm(&d.flatten(), 100, 100, 120, 110).to_image();
    d.crop_area(PixelRect { x: 100, y: 100, w: 120, h: 110 }, true);
    assert_eq!(pixel_diff(&d.flatten(), &want), 0);
    // The plain layer under it is still cut.
    assert_eq!(d.layers[0].rect(), PixelRect { x: 0, y: 0, w: 120, h: 110 });
}

/// Styled pixels are cut too, keeping how far their styles reach (their
/// own, or a group's): nothing deleted can come back in a file.
#[test]
fn delete_cropped_pixels_cuts_styled_layers_to_their_reach() {
    type Style = fn(&mut Document, usize);
    let cases: [(&str, Style); 3] = [
        ("shadow", |d, l| {
            let s = &mut d.layers[l].effects.shadow;
            (s.enabled, s.dx, s.dy, s.blur) = (true, 40.0, 30.0, 8.0);
        }),
        ("stroke", |d, l| {
            let s = &mut d.layers[l].effects.stroke;
            (s.enabled, s.width) = (true, 12.0);
        }),
        ("group glow", |d, l| {
            let id = d.layers[l].id;
            assert!(d.group_layers(&[l]));
            let l = d.layers.iter().position(|x| x.id == id).expect("the layer");
            let g = d.parent(l).expect("its group");
            let glow = &mut d.layers[g].effects.outer_glow;
            (glow.enabled, glow.size) = (true, 15.0);
        }),
    ];
    for (name, style) in cases {
        let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
        // Its right and bottom edges inside the box, the rest far outside.
        d.add_layer("fx".into(), RgbaImage::from_pixel(380, 280, BLUE), (-200, -150));
        let l = d.layers.len() - 1;
        style(&mut d, l);
        let id = d.layers[l].id;
        let want = image::imageops::crop_imm(&d.flatten(), 100, 80, 120, 110).to_image();
        d.crop_area(PixelRect { x: 100, y: 80, w: 120, h: 110 }, true);
        assert_eq!(pixel_diff(&d.flatten(), &want), 0, "{name}");
        let l = d.layers.iter().find(|l| l.id == id).expect("the layer");
        let r = l.rect();
        assert!(r.w < 380 && r.h < 280, "{name}: cut, {r:?}");
    }
    // A gradient overlay spans the layer's box: that one stays whole.
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    d.add_layer("fx".into(), RgbaImage::from_pixel(380, 280, BLUE), (-200, -150));
    d.layers[1].effects.gradient_overlay.enabled = true;
    let want = image::imageops::crop_imm(&d.flatten(), 100, 80, 120, 110).to_image();
    d.crop_area(PixelRect { x: 100, y: 80, w: 120, h: 110 }, true);
    assert_eq!(pixel_diff(&d.flatten(), &want), 0);
    assert_eq!((d.layers[1].rect().w, d.layers[1].rect().h), (380, 280));
}

/// With Delete Cropped Pixels, a turned crop turns only what the box
/// reaches, and gives what it gives without.
#[test]
fn a_turned_crop_with_delete_gives_what_it_gives_without() {
    use crate::document::{CropOrder, WholeChange};
    let make = || {
        let mut d = layered(3, 900, 700);
        d.add_layer("fx".into(), RgbaImage::from_pixel(500, 400, BLUE), (100, 100));
        let l = d.layers.len() - 1;
        let s = &mut d.layers[l].effects.shadow;
        (s.enabled, s.dx, s.dy, s.blur) = (true, 20.0, 20.0, 6.0);
        d
    };
    let run = |delete: bool| {
        let mut d = make();
        let order = CropOrder {
            rect: PixelRect { x: 300, y: 250, w: 200, h: 120 },
            angle: 17.0,
            delete,
            size: None,
            resolution: 0.0,
            filter: image::imageops::FilterType::Triangle,
        };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = d.whole_job(WholeChange::Crop(order), cancel).run().expect("done");
        assert!(d.put_whole(done, "Crop".into()));
        d
    };
    let (kept, deleted) = (run(false), run(true));
    assert_eq!(pixel_diff(&kept.flatten(), &deleted.flatten()), 0);
    let largest = |d: &Document| d.layers.iter().map(|l| l.rect().w).max().unwrap_or(0);
    assert!(largest(&deleted) < 300, "only the box's reach turned: {}", largest(&deleted));
    assert!(largest(&kept) > 900, "the whole layers turned");
}

#[test]
fn guides_keep_their_place_on_a_turned_box() {
    use crate::guides::{Guide, Orientation};
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    d.guides.push(Guide::new(Orientation::Vertical, 300.0));
    d.guides.push(Guide::new(Orientation::Horizontal, 150.0));
    let mut h = app_with(d);
    h.state_mut().apply_crop(CropPlan {
        angle: 30.0,
        ..CropPlan::plain(PixelRect { x: 250, y: 100, w: 100, h: 100 })
    });
    h.run_steps(2);
    let g = &doc(&h).guides;
    assert!((g[0].position - 50.0).abs() < 1.0, "{g:?}");
    assert!((g[1].position - 50.0).abs() < 1.0, "{g:?}");
}

#[test]
fn original_ratio_is_the_current_documents() {
    let mut h = crop_app();
    h.state_mut().crop_pick_preset(CropPreset::Original, full());
    assert_eq!(h.state().crop_ratio(), Some(400.0 / 300.0));
    h.state_mut().doc = Some(Document::new(RgbaImage::from_pixel(300, 900, RED), None));
    h.run_steps(2);
    assert_eq!(h.state().crop_ratio(), Some(300.0 / 900.0));
    assert_eq!((h.state().crop.w, h.state().crop.h), (300.0, 900.0), "its fields too");
}

#[test]
fn turns_grow_back_until_the_box_is_changed_by_hand() {
    let mut h = crop_app();
    let app = h.state_mut();
    for degrees in [5.0, 8.0, 12.0, 0.0] {
        app.crop_turn_starts(app.canvas_rect.unwrap_or(full()));
        app.crop_turn_to(degrees, full());
    }
    assert_eq!(app.canvas_rect, Some(full()), "back to where it started");
    // Changed by hand: the turn starts from that.
    let by_hand = Rect::from_min_size(pos2(50.0, 50.0), vec2(100.0, 80.0));
    app.canvas_rect = Some(by_hand);
    app.crop_turn_starts(by_hand);
    app.crop_turn_to(1.0, full());
    assert_eq!(app.canvas_rect, Some(by_hand));
}

#[test]
fn o_and_x_belong_to_the_crop_tool() {
    let mut h = crop_app();
    h.state_mut().primary = Color32::RED;
    let overlay = h.state().crop.overlay;
    key(&mut h, Key::O, Modifiers::NONE);
    assert_eq!(h.state().tool, Tool::Canvas);
    assert_ne!(h.state().crop.overlay, overlay);
    key(&mut h, Key::O, Modifiers::SHIFT);
    assert_eq!(h.state().tool, Tool::Canvas);
    let app = h.state_mut();
    (app.crop.w, app.crop.h) = (16.0, 9.0);
    key(&mut h, Key::X, Modifiers::NONE);
    assert_eq!((h.state().crop.w, h.state().crop.h), (9.0, 16.0));
    assert_eq!(h.state().primary, Color32::RED, "the colors stay");
    // Another tool: as ever.
    h.state_mut().reset_pending();
    h.state_mut().set_tool(Tool::Brush);
    key(&mut h, Key::X, Modifiers::NONE);
    assert_ne!(h.state().primary, Color32::RED);
    key(&mut h, Key::O, Modifiers::NONE);
    assert_eq!(h.state().tool, Tool::Dodge);
}

#[test]
fn the_presets_list_has_a_name_and_empty_shows_empty() {
    let h = crop_app();
    assert!(h.query_by_label(&t!("crop9-preset-name")).is_some(), "named for screen readers");
    let label = t!("optbar-width-short");
    let w = h.query_all_by_label(&label).next().expect("the bar's W");
    assert_eq!(w.value().unwrap_or_default(), "", "not 0");
}

#[test]
fn a_large_turned_crop_goes_aside_as_one_step_and_undoes() {
    let mut h = app_with(layered(3, 3000, 2000));
    let before = doc(&h).flatten();
    let steps = doc(&h).history().1;
    let app = h.state_mut();
    app.canvas_rect = Some(Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0)));
    app.crop.angle = 12.5;
    app.crop.delete_cropped = true;
    app.apply_crop_box();
    assert_eq!(size(&h), (3000, 2000), "not on this thread");
    assert!(h.state().transform_busy());
    settle(&mut h);
    assert_eq!(size(&h), (1000, 800));
    assert_eq!(doc(&h).history().1, steps + 1, "one step");
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    assert_eq!(size(&h), (3000, 2000));
    assert_eq!(pixel_diff(&doc(&h).flatten(), &before), 0);
}

#[test]
fn input_during_a_large_crop_waits_for_it_and_applies_to_its_result() {
    let mut h = app_with(layered(3, 3000, 2000));
    let app = h.state_mut();
    app.canvas_rect = Some(Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0)));
    app.crop.angle = 12.5;
    app.apply_crop_box();
    assert!(h.state().transform_busy());
    // Undo by its keys, at once: once it lands, it undoes the crop.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    settle_held(&mut h);
    assert_eq!(size(&h), (3000, 2000));
    assert!(doc(&h).can_redo(), "what was undone is the crop");
}

#[test]
fn a_large_image_rotation_goes_aside_and_cancel_changes_nothing() {
    let mut h = app_with(layered(12, 3000, 2000));
    let (steps, id) = (doc(&h).history().1, doc(&h).state_id());
    h.state_mut().run(Action::RotateCanvas(33));
    assert!(h.state().transform_busy(), "aside");
    h.state_mut().crop.whole.as_mut().unwrap().show_at_once();
    h.run_steps(2);
    assert!(h.state().crop.whole.as_ref().is_some_and(|r| r.showing()), "its indicator");
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!h.state().transform_busy());
    assert_eq!(doc(&h).state_id(), id);
    assert_eq!(doc(&h).history().1, steps);
    assert_eq!(size(&h), (3000, 2000));
    let name = t!("xchg-hist-rotate-canvas");
    assert_eq!(h.state().status, t!("crop9-status-cancelled", name = name.as_str()));
}

#[test]
fn a_cancelled_crop_gives_its_box_back() {
    let mut h = app_with(layered(12, 3000, 2000));
    let drawn = Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0));
    let app = h.state_mut();
    app.canvas_rect = Some(drawn);
    app.crop.angle = 12.5;
    app.apply_crop_box();
    app.crop.whole.as_mut().unwrap().show_at_once();
    h.run_steps(1);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(h.state().canvas_rect, Some(drawn));
    assert_eq!(h.state().crop.angle, 12.5);
}

#[test]
fn a_size_keeps_a_text_as_text_scaled_with_the_image() {
    let mut h = crop_app();
    assert!(typed_text(&mut h, (50.0, 50.0), "Bonjour"));
    h.state_mut().commit_text();
    h.state_mut().set_tool(Tool::Canvas);
    h.run_steps(2);
    let l = doc(&h).layers.iter().find(|l| l.text.is_some()).expect("a text");
    let (id, old_w) = (l.id, l.rect().w);
    let app = h.state_mut();
    app.apply_crop(CropPlan {
        size: Some((200, 200)),
        ..CropPlan::plain(PixelRect { x: 0, y: 0, w: 300, h: 300 })
    });
    h.run_steps(2);
    let l = doc(&h).layers.iter().find(|l| l.id == id).expect("still there");
    assert!(l.text.is_some(), "still a text");
    assert!(l.rect().w < old_w, "sized with the image");
}

/// Past the budget, the crop's undo still has the original pixels: 1.1 GiB
/// of layers (run with `--ignored`).
#[test]
#[ignore]
fn a_turned_crop_undoes_past_the_history_budget() {
    let mut h = app_with(layered(12, 6000, 4000));
    let before = doc(&h).flatten();
    h.state_mut().apply_crop(CropPlan {
        angle: 12.5,
        ..CropPlan::plain(PixelRect { x: 1000, y: 800, w: 4000, h: 2400 })
    });
    settle(&mut h);
    assert_eq!(size(&h), (4000, 2400));
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    assert_eq!(size(&h), (6000, 4000), "not the turned intermediate");
    assert_eq!(pixel_diff(&doc(&h).flatten(), &before), 0);
}

#[test]
fn a_turned_gradient_fill_keeps_its_direction() {
    use crate::compose::gradient::GradientFill;
    use crate::document::{Fill, LayerKind};
    let mut d = Document::new(RgbaImage::from_pixel(100, 100, RED), None);
    d.add_fill_layer(Fill::Gradient(GradientFill::default()), None);
    assert!(d.rotate_canvas(90.0));
    let LayerKind::Fill(Fill::Gradient(g)) = &d.layers[1].kind else {
        panic!("a gradient fill");
    };
    assert_eq!(g.angle, 0.0);
}

#[test]
fn a_quarter_turned_crop_turns_pattern_fills_and_overlays() {
    use crate::compose::pattern::{Pattern, PatternFill};
    use crate::document::Fill;
    let tile = RgbaImage::from_fn(17, 13, |x, y| {
        Rgba(if (x + y) % 5 == 0 { [255, 255, 0, 255] } else { [0, 0, 0, 255] })
    });
    let fill = || PatternFill::of(Pattern::new("t", tile.clone()));
    let base = || Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    let mut kinds = Vec::new();
    let mut d = base();
    d.add_fill_layer(Fill::Pattern(fill()), None);
    kinds.push(("linked", d));
    let mut d = base();
    let mut f = fill();
    f.linked = false;
    d.add_fill_layer(Fill::Pattern(f), None);
    kinds.push(("unlinked", d));
    let mut d = base();
    d.add_layer("o".into(), RgbaImage::from_pixel(300, 200, BLUE), (50, 50));
    d.layers[1].effects.pattern_overlay.enabled = true;
    d.layers[1].effects.pattern_overlay.fill = fill();
    kinds.push(("overlay", d));
    for (name, d) in kinds {
        let flat = d.flatten();
        let turned = image::imageops::rotate90(&flat);
        let mut h = app_with(d);
        // Turned the other way, the box covers the same pixels at 90°.
        h.state_mut().apply_crop(CropPlan {
            angle: -90.0,
            ..CropPlan::plain(PixelRect { x: 100, y: 50, w: 200, h: 100 })
        });
        h.run_steps(2);
        let (w, tall) = size(&h);
        assert_eq!((w, tall), (200, 100), "{name}");
        // The canvas's turn takes (x, y) to (299 - y, x); the box stays centered on (200, 100).
        let want = image::imageops::crop_imm(&turned, 300 - 100 - 100, 200 - 50, 200, 100).to_image();
        assert_eq!(pixel_diff(&doc(&h).flatten(), &want), 0, "{name}");
    }
}

// ---------- After the review of 0.5.0: the job path and its indicator ----------

/// One frame as the app gets it: through its input hook first (what it
/// gets is put in `input_mut`, not queued with `event`, which the hook
/// would never see).
fn frame(h: &mut Harness<'static, AtelierApp>) {
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    *h.input_mut() = raw;
    h.run_steps(1);
}

/// Frames through the hook until the job has landed and what was held is
/// given back.
fn settle_held(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..4000 {
        frame(h);
        if !h.state().transform_busy() {
            for _ in 0..8 {
                frame(h);
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the job never landed");
}

fn pointer(h: &mut Harness<'static, AtelierApp>, pos: Pos2, pressed: bool) {
    h.input_mut().events.push(egui::Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
}

/// The platform's side of closing the window: a close asked goes through
/// unless the frame cancels it, and one the app asks for (Quit) comes back
/// as a request. Whether the window closed.
fn window_closes(h: &mut Harness<'static, AtelierApp>) -> bool {
    let commands = |h: &Harness<'static, AtelierApp>| {
        h.output()
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    };
    let (mut asked, mut idle) = (true, 0);
    for _ in 0..4000 {
        if asked {
            h.input_mut()
                .viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .events
                .push(egui::ViewportEvent::Close);
        }
        frame(h);
        let commands = commands(h);
        if asked && !commands.contains(&egui::ViewportCommand::CancelClose) {
            return true;
        }
        asked = commands.contains(&egui::ViewportCommand::Close);
        idle = if asked || h.state().transform_busy() { 0 } else { idle + 1 };
        if idle > 5 {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("never settled");
}

#[test]
fn closing_the_window_during_a_job_asks_once_it_lands() {
    for rotate in [false, true] {
        let mut h = app_with(layered(3, 3000, 2000));
        h.state_mut().doc.as_mut().unwrap().mark_saved();
        let app = h.state_mut();
        if rotate {
            app.run(Action::RotateCanvas(33));
        } else {
            app.canvas_rect = Some(Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0)));
            app.crop.angle = 12.5;
            app.apply_crop_box();
        }
        assert!(h.state().transform_busy(), "aside");
        assert!(!window_closes(&mut h), "rotate {rotate}: never closed without asking");
        assert!(doc(&h).modified, "rotate {rotate}");
        assert!(matches!(h.state().confirm, Some(Confirm::Close)), "rotate {rotate}: it asks");
    }
}

#[test]
fn closing_the_window_during_a_cancelled_job_keeps_it_open() {
    let mut h = app_with(layered(12, 3000, 2000));
    h.state_mut().doc.as_mut().unwrap().mark_saved();
    h.state_mut().run(Action::RotateCanvas(33));
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    frame(&mut h);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!h.state().transform_busy(), "cancelled");
    assert!(!doc(&h).modified);
    // Asked again, it closes: nothing is pending.
    assert!(window_closes(&mut h));
}

/// A 2600 × 2400 layer distorted with Free Transform: applied aside.
fn transform_aside(h: &mut Harness<'static, AtelierApp>) {
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    key(h, Key::T, Modifiers::COMMAND);
    let narrowed = [[0.0, 0.0], [1.0, 0.0], [0.9, 1.0], [0.1, 1.0]];
    assert!(h.state_mut().set_transform_frame(narrowed));
    h.run_steps(1);
    key(h, Key::Enter, Modifiers::NONE);
    assert!(h.state().transform_busy(), "applied aside");
}

fn big_layer() -> Document {
    let mut d = Document::new(RgbaImage::from_pixel(2800, 2600, RED), None);
    d.add_layer("Big".into(), RgbaImage::from_pixel(2600, 2400, BLUE), (100, 50));
    d
}

#[test]
fn closing_the_window_during_a_transform_applied_aside_asks() {
    let mut h = app_with(big_layer());
    h.state_mut().doc.as_mut().unwrap().mark_saved();
    transform_aside(&mut h);
    assert!(!window_closes(&mut h), "never closed without asking");
    assert!(doc(&h).modified);
    assert!(matches!(h.state().confirm, Some(Confirm::Close)));
}

#[test]
fn esc_cancels_a_job_before_its_indicator_shows() {
    let mut h = app_with(layered(12, 3000, 2000));
    let drawn = Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0));
    let (steps, id) = (doc(&h).history().1, doc(&h).state_id());
    let app = h.state_mut();
    app.canvas_rect = Some(drawn);
    app.crop.angle = 12.5;
    app.apply_crop_box();
    assert!(!h.state().crop.whole.as_ref().unwrap().showing());
    let t = std::time::Instant::now();
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(t.elapsed().as_millis() < 250, "no wait: {:?}", t.elapsed());
    assert!(!h.state().transform_busy(), "cancelled");
    assert_eq!((size(&h), doc(&h).state_id(), doc(&h).history().1), ((3000, 2000), id, steps));
    assert_eq!(h.state().canvas_rect, Some(drawn), "the box back");
    assert_eq!(h.state().crop.angle, 12.5);
}

/// A click on the indicator's Cancel: it cancels, and the tool under it
/// sees nothing.
fn click_cancel(h: &mut Harness<'static, AtelierApp>) {
    h.state_mut().crop.whole.as_mut().unwrap().show_at_once();
    // Its first frame only measures it, out of sight.
    frame(h);
    assert!(h.state().crop.whole.as_ref().is_some_and(|r| r.shown_rect().is_none()));
    frame(h);
    let label = t!("dialog-cancel");
    let shown = h.state().crop.whole.as_ref().and_then(|r| r.shown_rect()).expect("shown");
    let at = (h.query_all_by_label(&label))
        .map(|n| n.rect().center())
        .map(|c| pos2(c.x, c.y))
        .find(|&c| shown.contains(c))
        .expect("the indicator's Cancel");
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    frame(h);
    pointer(h, at, true);
    frame(h);
    pointer(h, at, false);
    frame(h);
    frame(h);
}

#[test]
fn the_indicators_cancel_cancels_and_never_reaches_the_tool() {
    for tool in [Tool::Brush, Tool::Canvas] {
        let mut h = app_with(layered(12, 3000, 2000));
        h.state_mut().set_tool(tool);
        h.run_steps(2);
        let (steps, id) = (doc(&h).history().1, doc(&h).state_id());
        let drawn = Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0));
        let app = h.state_mut();
        if tool == Tool::Canvas {
            app.canvas_rect = Some(drawn);
            app.crop.angle = 12.5;
            app.apply_crop_box();
        } else {
            app.run(Action::RotateCanvas(33));
        }
        click_cancel(&mut h);
        assert!(!h.state().transform_busy(), "{tool:?}: cancelled at once");
        settle_held(&mut h);
        let d = doc(&h);
        assert_eq!((d.width(), d.height()), (3000, 2000), "{tool:?}");
        assert_eq!((d.history().1, d.state_id()), (steps, id), "{tool:?}: nothing done");
        if tool == Tool::Canvas {
            assert_eq!(h.state().canvas_rect, Some(drawn), "the box back");
        }
    }
}

#[test]
fn input_during_a_job_is_held_and_applies_once_it_lands() {
    let mut h = app_with(layered(12, 3000, 2000));
    let steps = doc(&h).history().1;
    h.state_mut().run(Action::RotateCanvas(33));
    assert!(h.state().transform_busy());
    // Undo by its keys, at once: the window doesn't wait.
    let t = std::time::Instant::now();
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(t.elapsed().as_millis() < 250, "no wait: {:?}", t.elapsed());
    assert!(h.state().transform_busy(), "still computed");
    frame(&mut h);
    assert!(h.state().crop.whole.as_ref().unwrap().showing(), "the indicator says why");
    settle_held(&mut h);
    assert_eq!(size(&h), (3000, 2000), "undone");
    assert!(doc(&h).can_redo(), "what was undone is the rotation");
    assert_eq!(doc(&h).history().1, steps);
}

#[test]
fn crops_and_rotations_clear_the_ruler() {
    let m = crate::document::Measure {
        vertex: (10.0, 10.0),
        first: (390.0, 290.0),
        second: None,
    };
    let plans = [
        Some(CropPlan {
            angle: 20.0,
            ..CropPlan::plain(PixelRect { x: 100, y: 50, w: 200, h: 150 })
        }),
        None,
        Some(CropPlan::plain(PixelRect { x: 100, y: 50, w: 100, h: 100 })),
    ];
    for plan in plans {
        let mut h = crop_app();
        h.state_mut().doc.as_mut().unwrap().ruler = Some(m);
        match plan {
            Some(plan) => h.state_mut().apply_crop(plan),
            None => h.state_mut().run(Action::RotateCanvas(33)),
        }
        settle(&mut h);
        assert_eq!(doc(&h).ruler, None, "{plan:?}");
    }
}

#[test]
fn a_psds_resolution_reaches_the_document() {
    let mut h = crop_app();
    let app = h.state_mut();
    let meta = app.doc.as_mut().unwrap().file_metadata.image.get_or_insert_default();
    std::sync::Arc::make_mut(meta).density = Some((300.0, 300.0));
    let dir = std::env::temp_dir().join(format!("atelier-crop-psd-res-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let psd = dir.join("a.psd");
    app.doc.as_mut().unwrap().save(&psd).unwrap();
    let Ok(back) = Document::open(&psd) else { panic!("opens") };
    let _ = std::fs::remove_dir_all(dir);
    let back = back.doc;
    assert_eq!(back.file_metadata.image.as_ref().and_then(|m| m.density), Some((300.0, 300.0)));
    // W × H × Resolution at its own size and resolution: nothing to apply.
    let mut h = app_with(back);
    let app = h.state_mut();
    app.crop_pick_preset(CropPreset::Size, full());
    app.crop.resolution = 300.0;
    assert!(!app.crop_box_pending(), "nothing asked");
}

#[test]
fn fit_to_visible_pixels_with_a_ratio_holds_them_all() {
    let mut d = Document::new(RgbaImage::new(400, 300), None);
    d.add_layer("c".into(), RgbaImage::from_pixel(200, 50, BLUE), (100, 100));
    let mut h = app_with(d);
    let app = h.state_mut();
    app.crop_pick_preset(CropPreset::Square, full());
    app.crop_fit_visible();
    let r = app.canvas_rect.expect("a box");
    let content = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 50.0));
    assert!(r.contains_rect(content), "{r:?}");
    assert_eq!((r.width(), r.height()), (200.0, 200.0), "the smallest square around it");
    assert_eq!(r.center(), content.center());
}

#[test]
fn a_crops_resolution_is_bounded() {
    use crate::document::{CropOrder, MAX_RESOLUTION, WholeChange};
    let mut d = Document::new(RgbaImage::from_pixel(40, 30, RED), None);
    let order = CropOrder {
        rect: PixelRect { x: 0, y: 0, w: 40, h: 30 },
        angle: 0.0,
        delete: false,
        size: Some((40, 30)),
        resolution: 1e30,
        filter: image::imageops::FilterType::Triangle,
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = d.whole_job(WholeChange::Crop(order), cancel).run().expect("done");
    assert!(d.put_whole(done, "Crop".into()));
    let ppi = f64::from(MAX_RESOLUTION);
    assert_eq!(d.file_metadata.image.and_then(|m| m.density), Some((ppi, ppi)));
}

#[test]
fn a_cancelled_resample_stops() {
    use crate::document::{CropOrder, WholeChange};
    let d = layered(4, 1000, 800);
    let order = CropOrder {
        rect: PixelRect { x: 100, y: 100, w: 600, h: 400 },
        angle: 0.0,
        delete: false,
        size: Some((1200, 800)),
        resolution: 0.0,
        filter: image::imageops::FilterType::Lanczos3,
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    assert!(d.whole_job(WholeChange::Crop(order), cancel).run().is_none());
    // Not cancelled: every layer resampled, the same as one by one.
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done = d.whole_job(WholeChange::Crop(order), cancel).run().expect("done");
    let mut one = layered(4, 1000, 800);
    one.crop_area(order.rect, false);
    assert!(one.resize_image(1200, 800, order.filter));
    let mut d = d;
    assert!(d.put_whole(done, "Crop".into()));
    for (a, b) in d.layers.iter().zip(&one.layers) {
        assert_eq!(a.rect(), b.rect());
        assert_eq!(pixel_diff(a.image(), b.image()), 0);
    }
}

#[test]
fn background_jobs_have_english_names_in_diagnostics() {
    let mut h = app_with(layered(12, 3000, 2000));
    h.state_mut().run(Action::RotateCanvas(33));
    assert!(h.state().transform_busy());
    let mut seen = false;
    for _ in 0..200 {
        seen |= crate::diag::running_jobs().iter().any(|j| j == "Image Rotation");
        if seen {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    settle(&mut h);
    assert!(seen, "measured under its English name");
    let shown = super::diagnostics::static_operation("Image Rotation");
    assert_eq!(shown, Some(t!("menu-image-rotation")), "shown in the interface's");
    assert_eq!(super::diagnostics::static_operation("Crop"), Some(t!("menu-image-crop")));
}

#[test]
fn the_box_stays_drawn_while_its_crop_is_computed() {
    let mut h = app_with(layered(12, 3000, 2000));
    let drawn = Rect::from_min_size(pos2(500.0, 500.0), vec2(1000.0, 800.0));
    let app = h.state_mut();
    app.canvas_rect = Some(drawn);
    app.crop.angle = 12.5;
    app.apply_crop_box();
    assert!(h.state().transform_busy());
    assert!(!h.state().crop_box_pending(), "nothing left to ask about");
    assert_eq!(h.state().crop_box_shown(), Some((drawn, 12.5)), "the box still drawn");
    key(&mut h, Key::Escape, Modifiers::NONE);
}

