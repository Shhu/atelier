//! Painting, retouching, transforms, vectors and type driven through the
//! app: the regressions of the tools review (smudge at the edge, the Clone
//! Stamp's source, strokes that change nothing, layers that must be
//! rasterized or are hidden, Remove Background as a mask, group texts and
//! warped shapes kept live, Free Transform's own undo and mode switches,
//! Transform Again, the Move tool with a selection, the Pen's last anchor,
//! Ctrl+Z while typing, per-tool brush sizes).

use super::transform::{Command, Mode};
use super::*;
use crate::xform::WarpStyle;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: Rgba<u8> = Rgba([200, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 200, 255]);

fn app_doc(doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut h = Harness::builder()
        .with_size([1400.0, 1000.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn app_with(image: RgbaImage, tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = app_doc(Document::new(image, None));
    let app = h.state_mut();
    app.set_tool(tool);
    (app.brush_size, app.hardness) = (10.0, 1.0);
    h.run_steps(1);
    h
}

/// Red 400 × 300 background; "Logo", blue 100 × 50 at (100, 100), active.
fn logo_app() -> Harness<'static, AtelierApp> {
    let mut doc = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    doc.add_layer(
        "Logo".into(),
        RgbaImage::from_pixel(100, 50, BLUE),
        (100, 100),
    );
    let mut h = app_doc(doc);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn pixel(h: &Harness<'static, AtelierApp>, x: i64, y: i64) -> [u8; 4] {
    doc(h)
        .active_layer()
        .pixel_at(x, y)
        .expect("inside the layer")
}

fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn steps(h: &Harness<'static, AtelierApp>) -> usize {
    doc(h).history().1
}

fn button(h: &mut Harness<'_, AtelierApp>, at: Pos2, pressed: bool, m: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: m,
    });
    h.run_steps(1);
}

/// Press at the first point (canvas), through the others, release at the
/// last, with `m` held.
fn drag(h: &mut Harness<'_, AtelierApp>, path: &[(f32, f32)], m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    let start = screen(h, path[0].0, path[0].1);
    h.hover_at(start);
    h.run_steps(1);
    button(h, start, true, m);
    let mut end = start;
    for &(x, y) in &path[1..] {
        end = screen(h, x, y);
        h.hover_at(end);
        h.run_steps(1);
    }
    button(h, end, false, m);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn click(h: &mut Harness<'_, AtelierApp>, at: (f32, f32), m: Modifiers) {
    drag(h, &[at], m);
}

fn key(h: &mut Harness<'_, AtelierApp>, key: Key, modifiers: Modifiers) {
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
    h.run_steps(2);
}

fn gray_ramp() -> RgbaImage {
    RgbaImage::from_fn(256, 100, |x, _| Rgba([x as u8, x as u8, x as u8, 255]))
}

// ------------------------------------------------------------ painting

#[test]
fn smudge_from_the_canvas_edge_keeps_an_opaque_layer_opaque() {
    let img = RgbaImage::from_fn(200, 100, |x, _| {
        Rgba(if x % 20 < 10 {
            [200, 30, 30, 255]
        } else {
            [30, 30, 200, 255]
        })
    });
    let mut h = app_with(img, Tool::Smudge);
    let app = h.state_mut();
    (app.brush_size, app.paint.smudge_strength) = (30.0, 0.8);
    app.selection = Some(
        PixelRect {
            x: 50,
            y: 0,
            w: 100,
            h: 100,
        }
        .into(),
    );
    drag(
        &mut h,
        &[(52.0, 50.0), (60.0, 50.0), (80.0, 50.0), (100.0, 50.0)],
        Modifiers::NONE,
    );
    h.state_mut().selection = None;
    drag(
        &mut h,
        &[(2.0, 50.0), (20.0, 50.0), (40.0, 50.0)],
        Modifiers::NONE,
    );
    let min_alpha = (0..150).map(|x| pixel(&h, x, 50)[3]).min().unwrap();
    assert_eq!(min_alpha, 255);
}

#[test]
fn clone_stamp_samples_the_layer_active_when_painting() {
    let mut h = app_with(gray_ramp(), Tool::CloneStamp);
    h.state_mut().doc.as_mut().unwrap().add_empty_layer();
    h.run_steps(1);
    // Alt+click on the empty layer, then paint on the photo.
    click(&mut h, (30.0, 50.0), Modifiers::ALT);
    h.state_mut().select_layer(0);
    h.run_steps(1);
    drag(&mut h, &[(130.0, 50.0), (140.0, 50.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 135, 50), [35, 35, 35, 255]);
}

#[test]
fn clone_stamp_source_can_be_in_another_document() {
    let mut h = app_with(gray_ramp(), Tool::CloneStamp);
    click(&mut h, (30.0, 50.0), Modifiers::ALT);
    // A second, white document: the source stays in the first one.
    h.state_mut().add_tab(Document::new(
        RgbaImage::from_pixel(256, 100, Rgba(WHITE)),
        None,
    ));
    h.run_steps(3);
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    app.set_tool(Tool::CloneStamp);
    h.run_steps(2);
    assert_eq!(pixel(&h, 135, 50), WHITE);
    drag(&mut h, &[(130.0, 50.0), (140.0, 50.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 135, 50), [35, 35, 35, 255]);
}

#[test]
fn a_stroke_that_changes_nothing_records_no_step() {
    let mut h = app_with(gray_ramp(), Tool::CloneStamp);
    click(&mut h, (5.0, 50.0), Modifiers::ALT);
    drag(&mut h, &[(250.0, 50.0), (252.0, 50.0)], Modifiers::NONE);
    let n = steps(&h);
    // The offset is kept (aligned): the source falls left of the image.
    drag(&mut h, &[(100.0, 50.0), (120.0, 50.0)], Modifiers::NONE);
    assert_eq!(steps(&h), n);
    assert!(!h.state().status.is_empty());
}

fn add_rectangle(h: &mut Harness<'static, AtelierApp>) {
    use crate::vector::{Bounds, Live, Paint, Shape, ShapeKind, ShapeStroke, pt};
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(20.0, 20.0), pt(120.0, 80.0)),
    );
    let shape = Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default());
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .add_shape_layer("R".into(), "R", shape);
    h.run_steps(1);
}

#[test]
fn painting_a_shape_layer_asks_to_rasterize_it_first() {
    let mut h = app_with(RgbaImage::from_pixel(200, 100, Rgba(WHITE)), Tool::Brush);
    add_rectangle(&mut h);
    let n = steps(&h);
    drag(&mut h, &[(50.0, 50.0), (60.0, 50.0)], Modifiers::NONE);
    assert!(doc(&h).active_layer().shape.is_some(), "not painted");
    assert_eq!(steps(&h), n);
    assert!(h.state().paint.rasterize_confirm.is_some(), "asked");
    // OK: rasterized; the next stroke paints.
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(doc(&h).active_layer().shape.is_none());
    assert!(h.state().paint.rasterize_confirm.is_none());
    drag(&mut h, &[(50.0, 50.0), (60.0, 50.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 55, 50)[..3], [0, 0, 0]);
}

#[test]
fn painting_a_hidden_layer_is_refused() {
    let mut h = app_with(RgbaImage::from_pixel(200, 100, Rgba(WHITE)), Tool::Brush);
    h.state_mut().doc.as_mut().unwrap().layers[0].visible = false;
    h.run_steps(1);
    let n = steps(&h);
    drag(&mut h, &[(50.0, 50.0), (60.0, 50.0)], Modifiers::NONE);
    assert_eq!(pixel(&h, 55, 50), WHITE);
    assert_eq!(steps(&h), n);
    assert!(!h.state().status.is_empty());
    // The bucket too.
    h.state_mut().set_tool(Tool::Bucket);
    click(&mut h, (50.0, 50.0), Modifiers::NONE);
    assert_eq!(pixel(&h, 50, 50), WHITE);
}

#[test]
fn bucket_on_an_empty_layer_fills_by_what_all_layers_show() {
    // A black frame on the background, an empty layer above: the fill
    // stops at the frame.
    let img = RgbaImage::from_fn(100, 100, |x, y| {
        let edge = (20..=22).contains(&x) || (78..=80).contains(&x);
        let edge = edge || (20..=22).contains(&y) || (78..=80).contains(&y);
        Rgba(if edge { [0, 0, 0, 255] } else { WHITE })
    });
    let mut h = app_with(img, Tool::Bucket);
    h.state_mut().doc.as_mut().unwrap().add_empty_layer();
    let app = h.state_mut();
    app.primary = Color32::from_rgb(255, 0, 0);
    app.paint.bucket.all_layers = true;
    app.paint.bucket.anti_alias = false;
    h.run_steps(1);
    click(&mut h, (50.0, 50.0), Modifiers::NONE);
    assert_eq!(pixel(&h, 50, 50), [255, 0, 0, 255]);
    assert_eq!(pixel(&h, 10, 10), [0, 0, 0, 0], "beyond the frame");
}

#[test]
fn brush_sizes_are_kept_per_tool_up_to_5000() {
    let mut h = app_with(RgbaImage::from_pixel(200, 100, Rgba(WHITE)), Tool::Brush);
    h.state_mut().brush_size = 900.0;
    key(&mut h, Key::CloseBracket, Modifiers::NONE);
    assert_eq!(h.state().brush_size, 1000.0);
    h.state_mut().brush_size = 4950.0;
    key(&mut h, Key::CloseBracket, Modifiers::NONE);
    assert_eq!(h.state().brush_size, 5000.0);
    h.state_mut().set_tool(Tool::Eraser);
    h.state_mut().brush_size = 30.0;
    h.state_mut().set_tool(Tool::Brush);
    assert_eq!(h.state().brush_size, 5000.0);
    h.state_mut().set_tool(Tool::Eraser);
    assert_eq!(h.state().brush_size, 30.0);
}

#[test]
fn patch_from_an_identical_area_records_no_step() {
    let mut h = app_with(RgbaImage::from_pixel(200, 100, Rgba(WHITE)), Tool::Patch);
    h.state_mut().selection = Some(
        PixelRect {
            x: 20,
            y: 20,
            w: 20,
            h: 20,
        }
        .into(),
    );
    h.run_steps(1);
    let n = steps(&h);
    drag(
        &mut h,
        &[(30.0, 30.0), (60.0, 30.0), (90.0, 30.0)],
        Modifiers::NONE,
    );
    assert_eq!(steps(&h), n);
}

#[test]
fn remove_background_adds_a_mask_and_keeps_a_text() {
    let mut h = app_doc(Document::new(
        RgbaImage::from_pixel(200, 100, Rgba(WHITE)),
        None,
    ));
    if h.state_mut().text_engine().families().is_empty() {
        return;
    }
    h.state_mut().set_tool(Tool::Text);
    h.state_mut().create_text(pos2(20.0, 50.0), None);
    h.state_mut().type_text("Hi");
    h.state_mut().commit_text();
    h.run_steps(1);
    let d = h.state_mut().doc.as_mut().unwrap();
    let (w, hh) = d.active_layer().image().dimensions();
    let before = d.active_layer().image().clone();
    let mask =
        image::GrayImage::from_fn(w, hh, |x, _| image::Luma([if x < w / 2 { 255 } else { 0 }]));
    assert!(d.mask_active(&mask));
    let l = d.active_layer();
    assert!(l.text.is_some(), "still a text");
    assert_eq!(*l.image(), before, "pixels untouched");
    assert_eq!(
        l.mask.as_ref().map(|m| m.image().dimensions()),
        Some((w, hh))
    );
    assert!(d.mask_targeted());
}

// ---------------------------------------------------------- transforms

#[test]
fn scaling_a_group_keeps_its_text_editable() {
    let mut h = app_doc(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
    if h.state_mut().text_engine().families().is_empty() {
        return;
    }
    h.state_mut().set_tool(Tool::Text);
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.state_mut().type_text("Hello");
    h.state_mut().commit_text();
    h.run_steps(2);
    let ti = doc(&h).active;
    assert!(h.state_mut().doc.as_mut().unwrap().group_layers(&[ti]));
    h.run_steps(1);
    let d = doc(&h);
    let gi = (0..d.layers.len())
        .find(|&i| d.layers[i].is_group())
        .unwrap();
    h.state_mut().doc.as_mut().unwrap().active = gi;
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    let src = h.state().transform_source().unwrap();
    h.state_mut().transform_rect = Some(Rect::from_min_size(src.min, src.size() * 2.0));
    key(&mut h, Key::Enter, Modifiers::NONE);
    let texts = doc(&h).layers.iter().filter(|l| l.text.is_some()).count();
    assert_eq!(texts, 1);
}

#[test]
fn warping_a_shape_layer_keeps_it_a_shape() {
    let mut h = app_doc(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
    add_rectangle(&mut h);
    let before = doc(&h).active_layer().shape.as_ref().unwrap().path.bounds();
    h.state_mut().set_tool(Tool::Transform);
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.state_mut().set_warp_style((WarpStyle::Arc, 50.0, false));
    h.run_steps(1);
    key(&mut h, Key::Enter, Modifiers::NONE);
    let shape = doc(&h)
        .active_layer()
        .shape
        .as_ref()
        .expect("still a shape");
    // Bent: no longer the rectangle it was.
    assert_ne!(shape.path.bounds(), before);
    assert!(shape.path.subpaths[0].anchors.len() > 4);
}

#[test]
fn ctrl_z_inside_free_transform_undoes_the_last_drag() {
    let mut h = logo_app();
    key(&mut h, Key::T, Modifiers::COMMAND);
    let r0 = h.state().transform_rect;
    drag(
        &mut h,
        &[(150.0, 125.0), (165.0, 125.0), (180.0, 125.0)],
        Modifiers::NONE,
    );
    let r1 = h.state().transform_rect;
    assert_ne!(r1, r0);
    drag(
        &mut h,
        &[(230.0, 150.0), (245.0, 160.0), (260.0, 170.0)],
        Modifiers::NONE,
    );
    assert_ne!(h.state().transform_rect, r1);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().transform_pending());
    assert_eq!(h.state().transform_rect, r1);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert_eq!(h.state().transform_rect, r0);
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(!h.state().transform_pending());
}

#[test]
fn leaving_warp_keeps_everything_pending_until_enter_or_esc() {
    let mut h = logo_app();
    let s0 = steps(&h);
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.state_mut().set_warp_style((WarpStyle::Arc, 50.0, false));
    h.run_steps(1);
    h.state_mut().transform_command(Command::Mode(Mode::Scale));
    h.run_steps(1);
    assert!(h.state().transform_pending());
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert_eq!(steps(&h), s0, "Esc gives the warp up too");
    assert_eq!(doc(&h).active_layer().offset, (100, 100));
    // Enter: one step for both.
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.state_mut().set_warp_style((WarpStyle::Arc, 50.0, false));
    h.run_steps(1);
    h.state_mut().transform_command(Command::Mode(Mode::Scale));
    h.run_steps(1);
    let src = h.state().transform_rect.unwrap();
    h.state_mut().transform_rect = Some(src.expand(10.0));
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(steps(&h), s0 + 1);
}

#[test]
fn leaving_warp_on_a_large_layer_stays_in_free_transform() {
    let mut d = Document::new(RgbaImage::from_pixel(2800, 2600, RED), None);
    d.add_layer(
        "Big".into(),
        RgbaImage::from_pixel(2600, 2400, BLUE),
        (100, 50),
    );
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.state_mut().set_warp_style((WarpStyle::Arc, 50.0, false));
    h.run_steps(1);
    h.state_mut().transform_command(Command::Mode(Mode::Scale));
    for _ in 0..1500 {
        h.run_steps(1);
        if !h.state().transform_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    h.run_steps(2);
    assert!(h.state().transform_pending());
}

#[test]
fn transform_again_turns_another_layer_around_its_own_center() {
    let mut d = Document::new(RgbaImage::from_pixel(400, 300, RED), None);
    d.add_layer("A".into(), RgbaImage::from_pixel(100, 50, BLUE), (100, 100));
    d.add_layer("B".into(), RgbaImage::from_pixel(40, 20, BLUE), (300, 220));
    let mut h = app_doc(d);
    h.state_mut().set_tool(Tool::Transform);
    h.state_mut().doc.as_mut().unwrap().active = 1;
    h.run_steps(1);
    h.state_mut().transform_command(Command::RotateCw);
    h.run_steps(2);
    h.state_mut().doc.as_mut().unwrap().active = 2;
    h.run_steps(1);
    h.state_mut().transform_command(Command::Again);
    h.run_steps(2);
    let b = doc(&h).layers[2].content_bounds().unwrap();
    assert_eq!((b.x, b.y, b.w, b.h), (310, 210, 20, 40));
    // The status says where it is.
    assert!(h.state().status.contains("310"), "{}", h.state().status);
}

#[test]
fn rotate_mode_still_moves_inside_the_frame() {
    let mut h = logo_app();
    h.state_mut().transform_command(Command::Mode(Mode::Rotate));
    h.run_steps(1);
    drag(
        &mut h,
        &[(120.0, 110.0), (145.0, 110.0), (170.0, 110.0)],
        Modifiers::NONE,
    );
    assert_eq!(h.state().transform_angle, 0.0);
    let r = h.state().transform_rect.unwrap();
    assert_eq!(r.min, pos2(150.0, 100.0));
}

#[test]
fn move_tool_moves_the_selected_pixels() {
    let mut h = logo_app();
    h.state_mut().selection = Some(
        PixelRect {
            x: 100,
            y: 100,
            w: 30,
            h: 30,
        }
        .into(),
    );
    h.run_steps(1);
    drag(
        &mut h,
        &[(110.0, 110.0), (110.0, 160.0), (110.0, 210.0)],
        Modifiers::NONE,
    );
    let flat = doc(&h).flatten();
    assert_eq!(flat.get_pixel(150, 110).0, BLUE.0, "the rest stays");
    assert_eq!(flat.get_pixel(110, 210).0, BLUE.0, "moved");
    assert_ne!(flat.get_pixel(110, 110).0, BLUE.0, "taken away");
}

// ------------------------------------------------------- vectors, type

#[test]
fn deleting_the_pens_only_anchor_removes_its_shape_layer() {
    let mut h = logo_app();
    h.state_mut().vector.pen_mode = super::vector_tools::ToolMode::Shape;
    h.state_mut()
        .set_tool(Tool::Pen(super::vector_tools::PenTool::Pen));
    h.run_steps(1);
    let n = doc(&h).layers.len();
    click(&mut h, (50.0, 50.0), Modifiers::NONE);
    assert_eq!(doc(&h).layers.len(), n + 1);
    key(&mut h, Key::Backspace, Modifiers::NONE);
    assert_eq!(doc(&h).layers.len(), n);
}

#[test]
fn ctrl_z_while_typing_undoes_the_last_word() {
    let mut h = app_doc(Document::new(RgbaImage::from_pixel(400, 300, RED), None));
    if h.state_mut().text_engine().families().is_empty() {
        return;
    }
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(1);
    let n = doc(&h).layers.len();
    h.state_mut().create_text(pos2(50.0, 80.0), None);
    h.run_steps(1);
    for w in ["Hello ", "big ", "world"] {
        h.event(egui::Event::Text(w.into()));
        h.run_steps(1);
    }
    let text = |h: &Harness<'static, AtelierApp>| {
        doc(h).active_layer().text.as_ref().map(|t| t.text.clone())
    };
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(h.state().typing_on_canvas());
    assert_eq!(text(&h).as_deref(), Some("Hello big "));
    key(&mut h, Key::Z, Modifiers::SHIFT | Modifiers::COMMAND);
    assert_eq!(text(&h).as_deref(), Some("Hello big world"));
    for _ in 0..3 {
        key(&mut h, Key::Z, Modifiers::COMMAND);
    }
    assert_eq!(text(&h).as_deref(), Some(""));
    // Nothing left in the edit: the new text goes, and nothing more.
    key(&mut h, Key::Z, Modifiers::COMMAND);
    assert!(!h.state().typing_on_canvas());
    assert_eq!(doc(&h).layers.len(), n);
}
