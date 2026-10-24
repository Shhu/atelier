//! Selection and Move tools: gestures driven through the real app
//! (pointer and keys), interrupted, released off the canvas, and the
//! geometry they apply.

use super::*;
use egui_kittest::Harness;

const RED: image::Rgba<u8> = image::Rgba([200, 0, 0, 255]);
const BLUE: image::Rgba<u8> = image::Rgba([0, 0, 200, 255]);
/// Over the right panel, far from the canvas.
const OFF_CANVAS: Pos2 = pos2(1395.0, 500.0);

fn opaque(w: u32, h: u32, c: image::Rgba<u8>) -> image::RgbaImage {
    image::RgbaImage::from_pixel(w, h, c)
}

fn app_with(doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut harness = Harness::builder()
        .with_size([1400.0, 1000.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    harness.run_steps(3);
    harness
}

/// Background (red) and "Top" (blue, half opacity), both 200 × 200.
fn two_layers() -> Document {
    let mut doc = Document::new(opaque(200, 200, RED), None);
    doc.add_layer("Top".into(), opaque(200, 200, BLUE), (0, 0));
    doc.active_layer_mut().opacity = 0.5;
    doc
}

fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn press(h: &mut Harness<'_, AtelierApp>, at: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
}

/// Pointer moved in a few steps, button held.
fn move_to(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2) {
    for i in 1..=4 {
        h.hover_at(from.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
}

fn release(h: &mut Harness<'_, AtelierApp>, at: Pos2) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

/// A key pressed while `modifiers` are held.
fn key_held(h: &mut Harness<'_, AtelierApp>, key: Key, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers,
        });
    }
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn drag(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    press(h, from, modifiers);
    move_to(h, from, to);
    release(h, to);
}

/// Ctrl+drag of a selection on "Top", interrupted before the release.
fn start_ctrl_drag(h: &mut Harness<'static, AtelierApp>) -> Pos2 {
    h.state_mut().tool = Tool::Select;
    h.state_mut().selection = Some(rect(50, 50, 50, 50).into());
    h.run_steps(1);
    let from = screen(h, 70.0, 70.0);
    let to = screen(h, 90.0, 70.0);
    press(h, from, Modifiers::COMMAND);
    move_to(h, from, to);
    assert_eq!(doc(h).layers.len(), 3, "pixels lifted");
    to
}

fn names(doc: &Document) -> Vec<&str> {
    doc.layers.iter().map(|l| l.name.as_str()).collect()
}

#[test]
fn undo_during_ctrl_drag_puts_everything_back() {
    let mut h = app_with(two_layers());
    let to = start_ctrl_drag(&mut h);
    h.state_mut().undo();
    release(&mut h, to);
    let app = h.state();
    let d = doc(&h);
    assert_eq!(names(d), ["Background", "Top"]);
    assert_eq!(d.active, 1);
    let top = &d.layers[1];
    assert_eq!((top.offset, top.opacity), ((0, 0), 0.5));
    assert_eq!(top.pixel_at(60, 60), Some(BLUE.0), "pixels back in place");
    assert_eq!(d.layers[0].rect(), rect(0, 0, 200, 200));
    assert!(app.select_move.is_none());
    assert_eq!(app.selection, Some(rect(50, 50, 50, 50).into()));
}

/// Whatever Ctrl+Z does in the middle of the drag, no layer is merged
/// into another, and the move can still be undone.
#[test]
fn ctrl_z_key_during_ctrl_drag_loses_nothing() {
    let mut h = app_with(two_layers());
    let to = start_ctrl_drag(&mut h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(1);
    release(&mut h, to);
    let d = doc(&h);
    assert_eq!(names(d), ["Background", "Top"]);
    assert_eq!(d.layers[1].opacity, 0.5);
    assert_eq!(d.layers[0].rect(), rect(0, 0, 200, 200));
    assert_eq!(d.layers[0].pixel_at(60, 60), Some(RED.0));
    while doc(&h).layers[1].pixel_at(60, 60) != Some(BLUE.0) {
        assert!(h.state_mut().doc.as_mut().unwrap().undo(), "undoable");
    }
    assert_eq!(names(doc(&h)), ["Background", "Top"]);
}

#[test]
fn tool_change_during_ctrl_drag_drops_the_pixels() {
    let mut h = app_with(two_layers());
    let to = start_ctrl_drag(&mut h);
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(1);
    release(&mut h, to);
    assert!(h.state().tool == Tool::Brush);
    assert!(h.state().select_move.is_none());
    let d = doc(&h);
    assert_eq!(names(d), ["Background", "Top"]);
    let top = &d.layers[1];
    // Moved by 20 px and dropped onto "Top".
    assert_eq!(top.alpha_at(55, 60), 0);
    assert_eq!(top.pixel_at(115, 60), Some(BLUE.0));
    // Back to the marquee: nothing resumes.
    h.key_press(Key::M);
    h.run_steps(1);
    h.hover_at(screen(&h, 150.0, 150.0));
    h.run_steps(2);
    assert_eq!(names(doc(&h)), ["Background", "Top"]);
    assert_eq!(doc(&h).layers[1].pixel_at(115, 60), Some(BLUE.0));
}

#[test]
fn tab_change_during_ctrl_drag_drops_the_pixels() {
    let mut h = app_with(two_layers());
    h.state_mut()
        .add_tab(Document::new(opaque(64, 64, RED), None));
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    let to = start_ctrl_drag(&mut h);
    h.state_mut().switch_tab(1);
    h.run_steps(1);
    release(&mut h, to);
    assert_eq!(h.state().active_tab, 1);
    h.state_mut().switch_tab(0);
    h.run_steps(1);
    let d = doc(&h);
    assert_eq!(names(d), ["Background", "Top"]);
    assert_eq!(d.layers[1].pixel_at(115, 60), Some(BLUE.0));
}

#[test]
fn marquee_released_off_canvas_ends() {
    let mut h = app_with(Document::new(opaque(200, 200, RED), None));
    h.state_mut().tool = Tool::Select;
    h.run_steps(1);
    let (from, to) = (screen(&h, 20.0, 20.0), OFF_CANVAS);
    drag(&mut h, from, to, Modifiers::NONE);
    let app = h.state();
    assert!(app.marquee.is_none());
    let sel = app.selection.clone().expect("selection");
    assert_eq!((sel.rect.x, sel.rect.y, sel.rect.right()), (20, 20, 200));
    // Coming back over the canvas changes nothing.
    h.hover_at(screen(&h, 60.0, 50.0));
    h.run_steps(2);
    assert_eq!(h.state().selection, Some(sel));

    // Add mode: the rectangle is added on release.
    h.state_mut().selection = Some(rect(0, 0, 5, 5).into());
    h.state_mut().select_mode = SelectMode::Add;
    let (from, to) = (screen(&h, 20.0, 20.0), OFF_CANVAS);
    drag(&mut h, from, to, Modifiers::NONE);
    let app = h.state();
    assert!(app.marquee.is_none() && app.marquee_preview.is_none());
    let sel = app.selection.as_ref().expect("selection");
    assert!(sel.contains(2.0, 2.0) && sel.contains(150.0, 30.0));
}

#[test]
fn layer_drag_released_off_canvas_ends() {
    let mut d = Document::new(opaque(400, 300, RED), None);
    d.add_layer("logo".into(), opaque(50, 50, BLUE), (20, 20));
    let mut h = app_with(d);
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let (from, to) = (screen(&h, 45.0, 45.0), OFF_CANVAS);
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().layer_drag.is_none());
    assert_ne!(doc(&h).layers[1].offset, (20, 20));
}

#[test]
fn click_inside_selection_deselects_and_drag_moves_it() {
    let mut h = app_with(Document::new(opaque(200, 200, RED), None));
    h.state_mut().tool = Tool::Select;
    h.state_mut().selection = Some(rect(50, 50, 50, 50).into());
    h.run_steps(1);
    let inside = screen(&h, 70.0, 70.0);
    let (from, to) = (inside, screen(&h, 80.0, 75.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(h.state().selection, Some(rect(60, 55, 50, 50).into()));
    drag(&mut h, inside, inside, Modifiers::NONE);
    assert!(h.state().selection.is_none());
    assert_eq!(h.state().last_selection, Some(rect(60, 55, 50, 50).into()));
}

/// Ctrl+drag on a hidden layer is refused: nothing hidden shows up.
#[test]
fn ctrl_drag_on_hidden_layer_is_refused() {
    let mut d = two_layers();
    d.layers[1].visible = false;
    let mut h = app_with(d);
    h.state_mut().tool = Tool::Select;
    h.state_mut().selection = Some(rect(50, 50, 50, 50).into());
    h.run_steps(1);
    let (from, to) = (screen(&h, 70.0, 70.0), screen(&h, 90.0, 70.0));
    drag(&mut h, from, to, Modifiers::COMMAND);
    assert_eq!(doc(&h).layers.len(), 2);
    assert_eq!(h.state().selection, Some(rect(50, 50, 50, 50).into()));
}

#[test]
fn arrows_follow_the_tool() {
    let mut h = app_with(two_layers());
    h.state_mut().tool = Tool::Select;
    h.state_mut().selection = Some(rect(50, 50, 50, 50).into());
    h.run_steps(1);
    // Selection tool: the outline moves, not the layer.
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(h.state().selection, Some(rect(51, 50, 50, 50).into()));
    assert_eq!(doc(&h).layers[1].offset, (0, 0));
    // Ctrl: the selected pixels move with it.
    key_held(&mut h, Key::ArrowDown, Modifiers::COMMAND);
    assert_eq!(h.state().selection, Some(rect(51, 51, 50, 50).into()));
    let d = doc(&h);
    assert_eq!(names(d), ["Background", "Top"]);
    assert_eq!(d.layers[1].alpha_at(60, 50), 0);
    assert_eq!(d.layers[1].pixel_at(60, 100), Some(BLUE.0));
    // Brush: nothing moves.
    h.state_mut().tool = Tool::Brush;
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(doc(&h).layers[1].offset, (0, 0));
    // Move tool: the selected pixels, like its drag…
    h.state_mut().tool = Tool::Transform;
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(h.state().selection, Some(rect(52, 51, 50, 50).into()));
    assert_eq!(doc(&h).layers[1].offset, (0, 0));
    assert_eq!(doc(&h).layers[1].alpha_at(51, 60), 0);
    assert_eq!(doc(&h).layers[1].pixel_at(101, 60), Some(BLUE.0));
    // …and without a selection, the layer.
    h.state_mut().deselect();
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(doc(&h).layers[1].offset, (1, 0));
}

#[test]
fn tool_change_cancels_the_marquee() {
    let mut h = app_with(Document::new(opaque(200, 200, RED), None));
    h.state_mut().tool = Tool::Select;
    h.state_mut().selection = Some(rect(0, 0, 5, 5).into());
    h.state_mut().select_mode = SelectMode::Add;
    h.run_steps(1);
    let from = screen(&h, 20.0, 20.0);
    press(&mut h, from, Modifiers::NONE);
    let to = screen(&h, 120.0, 120.0);
    move_to(&mut h, from, to);
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(1);
    release(&mut h, to);
    h.key_press(Key::M);
    h.run_steps(1);
    h.hover_at(screen(&h, 150.0, 150.0));
    h.run_steps(2);
    let app = h.state();
    assert!(app.marquee.is_none() && app.marquee_preview.is_none());
    assert_eq!(app.selection, Some(rect(0, 0, 5, 5).into()));
}

#[test]
fn reselect_forgets_a_selection_from_before_a_crop() {
    let mut h = app_with(Document::new(opaque(200, 200, RED), None));
    let app = h.state_mut();
    app.selection = Some(rect(10, 10, 20, 20).into());
    app.deselect();
    app.canvas_rect = Some(Rect::from_min_size(pos2(50.0, 50.0), vec2(100.0, 100.0)));
    app.apply_canvas();
    app.run(Action::Reselect);
    assert!(app.selection.is_none() && app.last_selection.is_none());
}

/// Photo + logo, Move tool: each drag is applied on release, a "Move"
/// step, and the logo is grabbed again where it is.
#[test]
fn each_move_drag_is_a_step_applied_on_release() {
    let mut d = Document::new(opaque(400, 300, RED), None);
    d.add_layer("logo".into(), opaque(50, 50, BLUE), (20, 20));
    let mut h = app_with(d);
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let steps = doc(&h).history().1;
    let (from, to) = (screen(&h, 45.0, 45.0), screen(&h, 245.0, 45.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none(), "nothing pending");
    assert_eq!(doc(&h).layers[1].offset, (220, 20));
    // Esc doesn't take it back.
    h.key_press(Key::Escape);
    h.run_steps(1);
    assert_eq!(doc(&h).layers[1].offset, (220, 20));
    let (from, to) = (screen(&h, 245.0, 45.0), screen(&h, 245.0, 105.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.active, 1);
    assert_eq!(d.layers[1].offset, (220, 80));
    assert_eq!(d.history().1, steps + 2);
    assert_eq!(d.history().0[steps + 1], t!("xchg-hist-move"));
}

/// "Fit canvas": grabbing B applies A's move (the canvas origin moves),
/// and B follows the pointer from where it is.
#[test]
fn grabbing_another_element_with_fit_canvas_does_not_jump() {
    let mut d = Document::new(image::RgbaImage::new(400, 300), None);
    d.add_layer("A".into(), opaque(50, 50, RED), (100, 100));
    d.add_layer("B".into(), opaque(50, 50, BLUE), (300, 200));
    d.active = 1;
    let mut h = app_with(d);
    h.state_mut().tool = Tool::Transform;
    h.state_mut().fit_canvas = true;
    h.state_mut().auto_select = true;
    h.run_steps(1);
    let (from, to) = (screen(&h, 125.0, 125.0), screen(&h, 135.0, 125.0));
    drag(&mut h, from, to, Modifiers::NONE);
    // A's box became the canvas: B is now at (190, 100).
    let d = doc(&h);
    assert_eq!((d.width(), d.height()), (50, 50));
    assert_eq!(d.layers[2].offset, (190, 100));
    // B grabbed where it shows, then the canvas takes its box.
    let (from, to) = (screen(&h, 215.0, 125.0), screen(&h, 225.0, 125.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.active, 2);
    assert_eq!(d.layers[2].offset, (0, 0));
    assert_eq!(d.layers[1].offset, (-200, -100));
}

/// Shift+drag along a guide's axis: snapping doesn't push it off the axis.
#[test]
fn snapping_keeps_a_shift_constrained_move_on_its_axis() {
    let mut d = Document::new(image::RgbaImage::new(400, 300), None);
    d.add_layer("A".into(), opaque(50, 50, RED), (250, 100));
    d.add_layer("B".into(), opaque(50, 50, BLUE), (100, 103));
    let mut h = app_with(d);
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let (from, to) = (screen(&h, 125.0, 128.0), screen(&h, 155.0, 128.0));
    drag(&mut h, from, to, Modifiers::SHIFT);
    assert_eq!(doc(&h).layers[2].offset.1, 103);
    assert_ne!(doc(&h).layers[2].offset.0, 100);
}

#[test]
fn multi_selected_layers_move_together_with_the_mouse() {
    let mut d = Document::new(image::RgbaImage::new(400, 300), None);
    d.add_layer("A".into(), opaque(50, 50, RED), (20, 20));
    d.add_layer("B".into(), opaque(50, 50, BLUE), (200, 100));
    let mut h = app_with(d);
    let ids: Vec<u64> = doc(&h).layers[1..].iter().map(|l| l.id).collect();
    h.state_mut().layer_set = ids;
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let (from, to) = (screen(&h, 45.0, 45.0), screen(&h, 75.0, 45.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().layer_drag.is_none());
    let d = doc(&h);
    assert_eq!(d.layers[1].offset, (50, 20));
    assert_eq!(d.layers[2].offset, (230, 100));
}

#[test]
fn quarter_turns_do_not_drift() {
    let mut d = Document::new(image::RgbaImage::new(400, 300), None);
    d.add_layer("E".into(), opaque(101, 50, RED), (100, 100));
    let mut h = app_with(d);
    let app = h.state_mut();
    app.tool = Tool::Transform;
    for turn in 1..=4 {
        let src = app.transform_source().unwrap();
        app.transform_rect = Some(src);
        app.transform_angle = 90.0;
        app.apply_transform();
        if turn % 2 == 0 {
            let r = app.doc.as_ref().unwrap().layers[1].rect();
            assert_eq!(r, rect(100, 100, 101, 50), "after {turn} turns");
        }
    }
}

#[test]
fn applied_size_and_place_are_the_previewed_ones() {
    let mut d = Document::new(image::RgbaImage::new(100, 100), None);
    d.add_layer("E".into(), opaque(10, 10, RED), (0, 0));
    let mut h = app_with(d);
    let app = h.state_mut();
    app.tool = Tool::Transform;
    app.transform_rect = Some(Rect::from_min_max(pos2(0.6, 0.0), pos2(11.1, 10.0)));
    app.apply_transform();
    let r = app.doc.as_ref().unwrap().layers[1].rect();
    assert_eq!(r, rect(1, 0, 10, 10));
}

#[test]
fn elements_larger_than_the_limit_are_refused() {
    let mut d = Document::new(image::RgbaImage::new(100, 100), None);
    d.add_layer("E".into(), opaque(10, 10, RED), (0, 0));
    let mut h = app_with(d);
    let app = h.state_mut();
    app.tool = Tool::Transform;
    let huge = MAX_DIMENSION as f32 + 10.0;
    app.transform_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(huge, 10.0)));
    app.apply_transform();
    assert_eq!(
        app.doc.as_ref().unwrap().layers[1].rect(),
        rect(0, 0, 10, 10)
    );
    assert!(app.transform_rect.is_none());
    // Turned, its box counts.
    let big = Vec2::splat(25_000.0);
    assert!(fits_max_dimension(big, 0.0));
    assert!(!fits_max_dimension(big, 45.0));
}

#[test]
fn stretched_or_rotated_text_stays_editable() {
    let mut h = app_with(Document::new(image::RgbaImage::new(600, 300), None));
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.state_mut().type_text("Hello");
    h.run_steps(1);
    let app = h.state_mut();
    app.tool = Tool::Transform;
    let src = app.transform_source().unwrap();
    let wide = Rect::from_min_size(src.min, vec2(src.width() * 2.0, src.height()));
    app.transform_rect = Some(wide);
    app.apply_transform();
    let layer = app.doc.as_ref().unwrap().active_layer();
    let t = layer.text.as_ref().expect("still a text");
    assert!((t.transform[0] - 2.0).abs() < 0.05 && (t.transform[3] - 1.0).abs() < 0.05);
    // Rendered again in the stretched frame.
    let b = layer.content_bounds().unwrap();
    assert!((to_rect(b).width() - wide.width()).abs() <= 3.0);
    assert!((to_rect(b).center().x - wide.center().x).abs() <= 2.0);

    let src = app.transform_source().unwrap();
    app.transform_rect = Some(src);
    app.transform_angle = 90.0;
    app.apply_transform();
    let layer = app.doc.as_ref().unwrap().active_layer();
    let t = layer.text.as_ref().expect("still a text");
    assert!(t.transform[2] > 1.9, "turned: {:?}", t.transform);
    let b = layer.content_bounds().unwrap();
    assert!(b.h > b.w);
}

#[test]
fn scaled_text_stays_editable_and_fits_the_canvas() {
    let mut h = app_with(Document::new(image::RgbaImage::new(600, 300), None));
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.state_mut().type_text("Hello");
    h.run_steps(1);
    let app = h.state_mut();
    app.tool = Tool::Transform;
    app.fit_canvas = true;
    let src = app.transform_source().unwrap();
    let frame = round_rect(Rect::from_min_size(src.min, src.size() * 1.5));
    app.transform_rect = Some(frame);
    app.apply_transform();
    let d = app.doc.as_ref().unwrap();
    let t = d.active_layer().text.as_ref().expect("still a text");
    // Scaled evenly: its size changed, not its transformation.
    assert_eq!(t.transform, crate::text::IDENTITY);
    assert!((t.runs[0].style.size - 72.0).abs() < 2.0);
    assert_eq!(
        (d.width() as f32, d.height() as f32),
        (frame.width(), frame.height())
    );
}

/// Side panel W/H in the Crop tool: the element is resampled, the crop
/// frame being prepared stays.
#[test]
fn applying_an_element_keeps_the_crop_frame() {
    let mut d = Document::new(image::RgbaImage::new(200, 200), None);
    d.add_layer("E".into(), opaque(10, 10, RED), (0, 0));
    let mut h = app_with(d);
    let app = h.state_mut();
    app.tool = Tool::Canvas;
    let crop = Rect::from_min_size(pos2(10.0, 10.0), vec2(100.0, 100.0));
    app.canvas_rect = Some(crop);
    app.transform_rect = Some(Rect::from_min_size(Pos2::ZERO, vec2(20.0, 20.0)));
    app.apply_transform();
    assert_eq!(
        app.doc.as_ref().unwrap().layers[1].rect(),
        rect(0, 0, 20, 20)
    );
    assert_eq!(app.canvas_rect, Some(crop));
}

#[test]
fn fit_on_screen_enlarges_small_images() {
    let mut h = app_with(Document::new(opaque(64, 64, RED), None));
    assert_eq!(h.state().zoom, 1.0, "opened at 100 % at most");
    h.state_mut().run(Action::Fit);
    h.run_steps(2);
    assert!(h.state().zoom > 4.0, "zoom {}", h.state().zoom);
}
