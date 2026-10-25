//! Pen, path selection and shape tools driven through the app: drawing a
//! path anchor by anchor, editing it, shape layers drawn and moved as
//! vectors, the Paths panel's commands.

use super::paths_panel::PathCommand;
use super::vector_tools::{PenTool, ShapeTool, ToolMode};
use super::*;
use crate::document::PathRef;
use crate::vector::{Paint, PathOp};
use egui_kittest::Harness;

const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A 200 × 100 white document at 100 %, with `tool`.
fn app(tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(
                image::RgbaImage::from_pixel(200, 100, image::Rgba(WHITE)),
                None,
            ));
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    app.set_tool(tool);
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

/// Press at canvas point `from` (with `modifiers`), move through `path`,
/// release at its end (a click when it is empty).
fn gesture(
    h: &mut Harness<'static, AtelierApp>,
    from: (f32, f32),
    path: &[(f32, f32)],
    m: Modifiers,
) {
    h.event(egui::Event::ModifiersChanged(m));
    let start = screen(h, from.0, from.1);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    let mut end = start;
    for &(x, y) in path {
        let to = screen(h, x, y);
        for i in 1..=4 {
            h.hover_at(end.lerp(to, i as f32 / 4.0));
            h.run_steps(1);
        }
        end = to;
    }
    h.event(egui::Event::PointerButton {
        pos: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: m,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
}

fn click(h: &mut Harness<'static, AtelierApp>, at: (f32, f32)) {
    gesture(h, at, &[], Modifiers::NONE);
}

fn work_path(h: &Harness<'static, AtelierApp>) -> crate::vector::Path {
    doc(h).work_path.clone().expect("a work path")
}

fn near(a: crate::vector::Pt, x: f32, y: f32) -> bool {
    (a.x - x).abs() < 0.6 && (a.y - y).abs() < 0.6
}

#[test]
fn the_pen_draws_corners_smooth_points_and_closes_the_path() {
    let mut h = app(Tool::Pen(PenTool::Pen));
    click(&mut h, (20.0, 20.0));
    click(&mut h, (80.0, 20.0));
    // A drag: a smooth point, its handles symmetric.
    gesture(&mut h, (80.0, 70.0), &[(100.0, 70.0)], Modifiers::NONE);
    let p = work_path(&h);
    let s = &p.subpaths[0];
    assert_eq!((s.anchors.len(), s.closed), (3, false));
    assert!(s.anchors[0].before == s.anchors[0].p && s.anchors[1].after == s.anchors[1].p);
    let a = s.anchors[2];
    assert!(
        a.smooth && near(a.after, 100.0, 70.0) && near(a.before, 60.0, 70.0),
        "{a:?}"
    );
    // Shift: 45° from the last anchor.
    gesture(&mut h, (30.0, 75.0), &[], Modifiers::SHIFT);
    let last = *work_path(&h).subpaths[0].anchors.last().unwrap();
    assert!((last.p.y - 70.0).abs() < 0.6, "{:?}", last.p);
    // On the first anchor: closed, the drawing ends.
    click(&mut h, (20.5, 20.5));
    let s = &work_path(&h).subpaths[0];
    assert!(s.closed && s.anchors.len() == 4);
    assert!(h.state().vector.drawing.is_none());
    // One step per anchor: undo opens it again.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run_steps(2);
    assert!(!work_path(&h).subpaths[0].closed);
    h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
    h.run_steps(2);
    // Ctrl+Enter: the path loaded as a selection.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    h.run_steps(2);
    let sel = h.state().selection.clone().expect("a selection");
    assert!(sel.contains(40.0, 40.0) && !sel.contains(150.0, 20.0));
    // The image itself never changed.
    assert_eq!(doc(&h).layers.len(), 1);
    assert_eq!(doc(&h).flatten().get_pixel(40, 40).0, WHITE);
}

#[test]
fn ctrl_moves_anchors_and_the_anchor_tools_edit_the_path() {
    let mut h = app(Tool::Pen(PenTool::Pen));
    for p in [(20.0, 20.0), (120.0, 20.0), (120.0, 80.0)] {
        click(&mut h, p);
    }
    h.key_press(Key::Enter);
    h.run_steps(2);
    assert!(h.state().vector.drawing.is_none());
    // Ctrl: Direct Selection for a moment, the anchor follows.
    gesture(&mut h, (120.0, 80.0), &[(140.0, 90.0)], Modifiers::COMMAND);
    assert!(near(work_path(&h).subpaths[0].anchors[2].p, 140.0, 90.0));
    assert!(h.state().tool == Tool::Pen(PenTool::Pen));
    // Auto Add: a click on a segment adds an anchor there, on an anchor
    // deletes it.
    click(&mut h, (70.0, 20.0));
    assert_eq!(work_path(&h).subpaths[0].anchors.len(), 4);
    assert!(near(work_path(&h).subpaths[0].anchors[1].p, 70.0, 20.0));
    click(&mut h, (70.0, 20.0));
    assert_eq!(work_path(&h).subpaths[0].anchors.len(), 3);
    // Convert Point: a drag pulls out handles.
    h.state_mut().set_tool(Tool::Pen(PenTool::Convert));
    gesture(&mut h, (120.0, 20.0), &[(140.0, 20.0)], Modifiers::NONE);
    let a = work_path(&h).subpaths[0].anchors[1];
    assert!(a.smooth && near(a.after, 140.0, 20.0) && near(a.before, 100.0, 20.0));
    // Path Selection, then Delete: the path goes.
    h.state_mut().set_tool(Tool::PathSelect);
    click(&mut h, (70.0, 20.0));
    assert_eq!(h.state().vector.components, [0]);
    h.key_press(Key::Delete);
    h.run_steps(2);
    assert!(doc(&h).work_path.is_none());
}

#[test]
fn shape_tools_draw_live_shape_layers() {
    let mut h = app(Tool::Shape(ShapeTool::Rectangle));
    h.state_mut().primary = Color32::from_rgb(200, 0, 0);
    gesture(&mut h, (20.0, 10.0), &[(60.0, 40.0)], Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 2);
    let layer = d.active_layer();
    let shape = layer.shape.as_ref().expect("a shape layer");
    assert!(shape.live.is_some());
    assert_eq!(shape.fill, Paint::Color([200, 0, 0]));
    assert_eq!(
        layer.content_bounds(),
        Some(PixelRect {
            x: 20,
            y: 10,
            w: 40,
            h: 30
        })
    );
    assert_eq!(d.flatten().get_pixel(30, 20).0, [200, 0, 0, 255]);
    // Shift: a square, Alt: from the center; another layer each time.
    gesture(
        &mut h,
        (100.0, 50.0),
        &[(120.0, 60.0)],
        Modifiers::SHIFT | Modifiers::ALT,
    );
    let d = doc(&h);
    assert_eq!(d.layers.len(), 3);
    assert_eq!(
        d.active_layer().content_bounds(),
        Some(PixelRect {
            x: 80,
            y: 30,
            w: 40,
            h: 40
        })
    );
    // An ellipse with Combine goes into the active shape layer.
    {
        let app = h.state_mut();
        app.set_tool(Tool::Shape(ShapeTool::Ellipse));
        app.vector.op = Some(PathOp::Combine);
    }
    gesture(&mut h, (150.0, 20.0), &[(190.0, 60.0)], Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 3);
    let shape = d.active_layer().shape.as_ref().unwrap();
    assert_eq!(shape.path.subpaths.len(), 2);
    assert!(shape.live.is_none());
    // Path mode: a component of the Work Path, no layer.
    {
        let app = h.state_mut();
        app.vector.shape_mode = ToolMode::Path;
        app.vector.op = None;
        app.set_tool(Tool::Shape(ShapeTool::Polygon));
    }
    gesture(&mut h, (10.0, 60.0), &[(40.0, 90.0)], Modifiers::NONE);
    assert_eq!(doc(&h).layers.len(), 3);
    let b = work_path(&h).bounds().unwrap();
    assert!(b.min.y >= 59.0 && b.max.y <= 91.0);
}

#[test]
fn the_pen_in_shape_mode_and_path_selection_move_shapes_as_vectors() {
    let mut h = app(Tool::Pen(PenTool::Pen));
    h.state_mut().vector.pen_mode = ToolMode::Shape;
    h.state_mut().primary = Color32::from_rgb(0, 0, 255);
    for p in [
        (20.0, 20.0),
        (80.0, 20.0),
        (80.0, 80.0),
        (20.0, 80.0),
        (20.0, 20.0),
    ] {
        click(&mut h, p);
    }
    let d = doc(&h);
    assert_eq!(d.layers.len(), 2);
    let shape = d.active_layer().shape.as_ref().expect("a shape layer");
    assert!(shape.path.subpaths[0].closed);
    assert_eq!(d.flatten().get_pixel(50, 50).0, [0, 0, 255, 255]);
    // Path Selection: dragged, the shape moves (by whole pixels at 100 %:
    // its image as it is, never resampled).
    h.state_mut().set_tool(Tool::PathSelect);
    gesture(&mut h, (50.0, 50.0), &[(90.0, 60.0)], Modifiers::NONE);
    let d = doc(&h);
    let id = d.active_layer().id;
    let b = d.path(PathRef::Layer(id)).unwrap().bounds().unwrap();
    assert!(near(b.min, 60.0, 30.0), "{b:?}");
    assert_eq!(d.flatten().get_pixel(100, 80).0, [0, 0, 255, 255]);
    assert_eq!(d.flatten().get_pixel(30, 30).0, WHITE);
    // Rasterized: plain pixels.
    h.state_mut().run(Action::Rasterize);
    assert!(doc(&h).active_layer().shape.is_none());
}

#[test]
fn paths_panel_commands() {
    let mut h = app(Tool::Select);
    let canvas = doc(&h).full_rect();
    // A selection made into a Work Path, then filled and stroked.
    h.state_mut().selection = Some(
        PixelRect {
            x: 20,
            y: 20,
            w: 60,
            h: 40,
        }
        .into(),
    );
    h.state_mut()
        .run(Action::Path(PathCommand::MakeWorkPath, false));
    assert!(h.state().selection.is_none());
    let b = work_path(&h).bounds().unwrap();
    assert!(near(b.min, 20.0, 20.0) && near(b.max, 80.0, 60.0), "{b:?}");
    assert_eq!(h.state().vector.selected, Some(PathRef::Work));
    h.state_mut().primary = Color32::from_rgb(0, 128, 0);
    h.state_mut()
        .run(Action::Path(PathCommand::FillPath, false));
    let flat = doc(&h).flatten();
    assert_eq!(flat.get_pixel(50, 40).0, [0, 128, 0, 255]);
    assert_eq!(flat.get_pixel(10, 10).0, WHITE);
    h.state_mut().primary = Color32::BLACK;
    h.state_mut().brush_size = 4.0;
    h.state_mut()
        .run(Action::Path(PathCommand::StrokePath, false));
    assert!(doc(&h).flatten().get_pixel(50, 20).0[1] < 100);
    // Saved, then a new empty path; deleted.
    h.state_mut()
        .run(Action::Path(PathCommand::SavePath, false));
    let d = doc(&h);
    assert!(d.work_path.is_none() && d.paths.len() == 1);
    h.state_mut().run(Action::Path(PathCommand::NewPath, false));
    assert_eq!(doc(&h).paths.len(), 2);
    assert_eq!(h.state().vector.selected, Some(PathRef::Saved(1)));
    h.state_mut()
        .run(Action::Path(PathCommand::DeletePath, false));
    assert_eq!(doc(&h).paths.len(), 1);
    // The saved path, selected, loads as a selection.
    h.state_mut().vector.selected = Some(PathRef::Saved(0));
    h.state_mut()
        .run(Action::Path(PathCommand::MakeSelection, false));
    let sel = h.state().selection.clone().expect("a selection");
    assert_eq!(
        sel.bounds(canvas),
        Some(PixelRect {
            x: 20,
            y: 20,
            w: 60,
            h: 40
        })
    );
    // The panel opens from the Window menu and shows the paths.
    h.state_mut().run(Action::Paths);
    h.run_steps(2);
    assert!(h.state().vector.panel.open);
}

#[test]
fn pen_and_shape_keys() {
    let mut h = app(Tool::Transform);
    let press = |h: &mut Harness<'static, AtelierApp>, m: Modifiers, key: Key| {
        h.key_press_modifiers(m, key);
        h.run_steps(1);
        h.state().tool
    };
    assert!(press(&mut h, Modifiers::NONE, Key::P) == Tool::Pen(PenTool::Pen));
    assert!(press(&mut h, Modifiers::SHIFT, Key::P) == Tool::Pen(PenTool::Freeform));
    // Shift+P cycles the pens only, not the anchor point tools.
    assert!(press(&mut h, Modifiers::SHIFT, Key::P) == Tool::Pen(PenTool::Pen));
    h.state_mut().set_tool(Tool::Pen(PenTool::Convert));
    assert!(press(&mut h, Modifiers::NONE, Key::P) == Tool::Pen(PenTool::Pen));
    assert!(press(&mut h, Modifiers::NONE, Key::A) == Tool::PathSelect);
    assert!(press(&mut h, Modifiers::SHIFT, Key::A) == Tool::DirectSelect);
    assert!(press(&mut h, Modifiers::NONE, Key::U) == Tool::Shape(ShapeTool::Rectangle));
    assert!(press(&mut h, Modifiers::SHIFT, Key::U) == Tool::Shape(ShapeTool::Ellipse));
}

/// Capture of the vector tools (offscreen render):
/// `ATELIER_CAPTURE=out.png cargo test vector_capture -- --ignored`.
#[test]
#[ignore = "needs a graphics renderer"]
fn vector_capture() {
    let out = std::env::var("ATELIER_CAPTURE").unwrap_or_else(|_| "vector.png".into());
    if std::env::var("ATELIER_LANG").is_ok_and(|v| v == "fr") {
        i18n::set_lang(i18n::Lang::Fr);
    }
    let mut h = Harness::builder()
        .with_size([1400.0, 1300.0])
        .wgpu()
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(
                image::RgbaImage::from_pixel(400, 300, image::Rgba([235, 235, 240, 255])),
                None,
            ));
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.5, Vec2::ZERO, None);
    app.set_tool(Tool::Shape(ShapeTool::Rectangle));
    app.vector.radius = 16.0;
    app.vector.stroke.paint = Paint::Color([30, 30, 30]);
    app.vector.stroke.width = 3.0;
    app.primary = Color32::from_rgb(70, 110, 200);
    h.run_steps(3);
    gesture(&mut h, (30.0, 30.0), &[(170.0, 130.0)], Modifiers::NONE);
    h.state_mut().set_tool(Tool::Shape(ShapeTool::Custom));
    h.state_mut().vector.custom = crate::vector::Custom::Heart;
    h.state_mut().primary = Color32::from_rgb(220, 60, 80);
    gesture(&mut h, (220.0, 40.0), &[(330.0, 140.0)], Modifiers::NONE);
    h.state_mut().set_tool(Tool::Pen(PenTool::Pen));
    click(&mut h, (40.0, 200.0));
    gesture(&mut h, (160.0, 260.0), &[(220.0, 260.0)], Modifiers::NONE);
    click(&mut h, (300.0, 190.0));
    let tool = match std::env::var("ATELIER_TOOL").as_deref() {
        Ok("direct") => Tool::DirectSelect,
        Ok("path") => Tool::PathSelect,
        Ok("polygon") => Tool::Shape(ShapeTool::Polygon),
        _ => Tool::Pen(PenTool::Pen),
    };
    h.state_mut().set_tool(tool);
    h.state_mut().run(Action::Paths);
    h.hover_at(screen(&h, 360.0, 260.0));
    h.run_steps(4);
    let image = h.render().expect("render");
    image.save(&out).expect("writing the capture");
}

/// Moving a whole styled shape with Path Selection only moves its image
/// (not rendered again), and while a gesture edits a shape its styles are
/// held (drawn again once, at the end).
#[test]
fn path_selection_moves_a_styled_shape_without_rendering_it_again() {
    use crate::vector::{Bounds, Live, Shape, ShapeKind, ShapeStroke, pt};
    let mut h = app(Tool::PathSelect);
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(20.0, 20.0), pt(80.0, 70.0)),
    );
    let shape = Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default());
    let d = h.state_mut().doc.as_mut().unwrap();
    d.add_shape_layer("R".into(), "R", shape);
    let fx = &mut d.active_layer_mut().effects;
    fx.stroke.enabled = true;
    fx.stroke.width = 4.0;
    h.run_steps(2);
    let (rev, offset) = (doc(&h).active_layer().rev(), doc(&h).active_layer().offset);
    let start = screen(&h, 50.0, 45.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    let to = screen(&h, 61.0, 52.0);
    for i in 1..=4 {
        h.hover_at(start.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
        assert!(doc(&h).effects_held(), "held during the gesture");
    }
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    let layer = doc(&h).active_layer();
    assert!(!doc(&h).effects_held(), "released with the gesture");
    assert_eq!(layer.rev(), rev, "the same image");
    assert_eq!(layer.offset, (offset.0 + 11, offset.1 + 7));
    assert_eq!(doc(&h).flatten().get_pixel(60, 50).0, [255, 0, 0, 255]);
}

/// A locked shape layer's path stays put in Quick Mask Mode too (the mode
/// lifts the lock for edits of the mask only).
#[test]
fn quick_mask_keeps_a_locked_shape_in_place() {
    use crate::vector::{Bounds, Live, Shape, ShapeKind, ShapeStroke, pt};
    let mut h = app(Tool::PathSelect);
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(20.0, 20.0), pt(80.0, 70.0)),
    );
    let shape = Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default());
    let d = h.state_mut().doc.as_mut().unwrap();
    d.add_shape_layer("R".into(), "R", shape);
    d.active_layer_mut().locked = true;
    h.run_steps(2);
    h.state_mut()
        .run(Action::Select(super::select_menu::Command::QuickMask));
    h.run_steps(2);
    assert!(doc(&h).quick_mask().is_some());
    let offset = doc(&h).active_layer().offset;
    let start = screen(&h, 50.0, 45.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    let to = screen(&h, 61.0, 52.0);
    for i in 1..=4 {
        h.hover_at(start.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(doc(&h).active_layer().offset, offset);
    let name = doc(&h).active_layer().name.clone();
    assert!(
        h.state()
            .status
            .contains(&t!("status-layer-is-locked", name = name.as_str())),
        "{:?}",
        h.state().status
    );
}

/// A shape layer dragged with Path Selection zoomed in: its image moves by
/// whole pixels during the drag (not drawn again at each step, its styles
/// following), and takes the fraction left at the end.
#[test]
fn a_shape_dragged_zoomed_in_moves_by_whole_pixels_until_released() {
    use crate::vector::{Bounds, Live, Shape, ShapeKind, ShapeStroke, pt};
    let mut h = app(Tool::PathSelect);
    h.state_mut().zoom = 4.0;
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(20.0, 20.0), pt(80.0, 70.0)),
    );
    let shape = Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default());
    let d = h.state_mut().doc.as_mut().unwrap();
    d.add_shape_layer("R".into(), "R", shape);
    let id = d.active_layer().id;
    h.run_steps(3);
    let bounds = |h: &Harness<'static, AtelierApp>| {
        let b = doc(h).path(PathRef::Layer(id)).unwrap().bounds().unwrap();
        (b.min.x, b.min.y)
    };
    let start = screen(&h, 50.0, 45.0);
    h.hover_at(start);
    h.run_steps(1);
    h.drag_at(start);
    h.run_steps(1);
    let to = screen(&h, 60.25, 48.5);
    let mut revs = std::collections::HashSet::new();
    for i in 1..=12 {
        h.hover_at(start.lerp(to, i as f32 / 12.0));
        h.run_steps(1);
        let l = doc(&h).active_layer();
        revs.insert(l.rev());
        let (x, y) = bounds(&h);
        assert!(x.fract() == 0.0 && y.fract() == 0.0, "whole pixels: {x}, {y}");
    }
    assert!(revs.len() <= 2, "drawn again during the drag: {}", revs.len());
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    let (x, y) = bounds(&h);
    assert!((x - 30.25).abs() < 0.01 && (y - 23.5).abs() < 0.01, "{x}, {y}");
    assert!(doc(&h).active_layer().shape.as_ref().unwrap().live.is_some());
}
