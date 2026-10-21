//! Edit › Transform driven through the real app: Free Transform's modifier
//! drags, its context menu and modes, the menu's turns and flips, Transform
//! Again, the selected pixels lifted, a mask transformed alone.

use super::transform::{Command, Mode};
use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

const RED: image::Rgba<u8> = image::Rgba([200, 0, 0, 255]);
const BLUE: image::Rgba<u8> = image::Rgba([0, 0, 200, 255]);

/// Red 400 × 300 background; "Logo", blue 100 × 50 at (100, 100), active,
/// with the Move tool.
fn app() -> Harness<'static, AtelierApp> {
    let mut doc = Document::new(image::RgbaImage::from_pixel(400, 300, RED), None);
    doc.add_layer(
        "Logo".into(),
        image::RgbaImage::from_pixel(100, 50, BLUE),
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

fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn drag(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for i in 1..=4 {
        h.hover_at(from.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(2);
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

/// The frame being prepared (canvas), corners from the top-left, clockwise.
fn quad(h: &Harness<'_, AtelierApp>) -> [Pos2; 4] {
    let app = h.state();
    let src = app.transform_source().expect("an element");
    let t = app.pending_transform(src, false).expect("a transform");
    [
        src.left_top(),
        src.right_top(),
        src.right_bottom(),
        src.left_bottom(),
    ]
    .map(|p| {
        let q = t.forward([p.x as f64, p.y as f64]).expect("in front");
        pos2(q[0] as f32, q[1] as f32)
    })
}

fn near(a: Pos2, b: Pos2) -> bool {
    a.distance(b) < 1.0
}

fn color_at(h: &Harness<'static, AtelierApp>, x: u32, y: u32) -> [u8; 4] {
    doc(h).flatten().get_pixel(x, y).0
}

#[test]
fn ctrl_drag_of_a_corner_distorts_in_one_undo_step() {
    let mut h = app();
    let (from, to) = (screen(&h, 200.0, 150.0), screen(&h, 230.0, 170.0));
    drag(&mut h, from, to, Modifiers::COMMAND);
    let q = quad(&h);
    assert!(near(q[2], pos2(230.0, 170.0)), "{q:?}");
    assert!(near(q[0], pos2(100.0, 100.0)) && near(q[1], pos2(200.0, 100.0)));
    assert!(near(q[3], pos2(100.0, 150.0)));
    // Still pending: the document hasn't changed.
    assert_eq!(color_at(&h, 210, 140), RED.0);
    let steps = doc(&h).history().1;
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none());
    assert_eq!(color_at(&h, 210, 140), BLUE.0);
    assert_eq!(color_at(&h, 150, 125), BLUE.0);
    assert_eq!(doc(&h).history().1, steps + 1);
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    assert_eq!(
        doc(&h).active_layer().rect(),
        PixelRect {
            x: 100,
            y: 100,
            w: 100,
            h: 50
        }
    );
}

#[test]
fn ctrl_shift_drag_of_an_edge_skews() {
    let mut h = app();
    let (from, to) = (screen(&h, 150.0, 100.0), screen(&h, 180.0, 110.0));
    drag(&mut h, from, to, Modifiers::COMMAND | Modifiers::SHIFT);
    let q = quad(&h);
    assert!(
        near(q[0], pos2(130.0, 100.0)) && near(q[1], pos2(230.0, 100.0)),
        "{q:?}"
    );
    assert!(near(q[2], pos2(200.0, 150.0)) && near(q[3], pos2(100.0, 150.0)));
}

#[test]
fn ctrl_alt_shift_drag_of_a_corner_is_a_perspective() {
    let mut h = app();
    let (from, to) = (screen(&h, 200.0, 100.0), screen(&h, 220.0, 103.0));
    drag(
        &mut h,
        from,
        to,
        Modifiers::COMMAND | Modifiers::SHIFT | Modifiers::ALT,
    );
    let q = quad(&h);
    assert!(
        near(q[1], pos2(220.0, 100.0)) && near(q[0], pos2(80.0, 100.0)),
        "{q:?}"
    );
    assert!(near(q[2], pos2(200.0, 150.0)) && near(q[3], pos2(100.0, 150.0)));
    // Escape gives it up.
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none() && !h.state().transform_distorted());
    assert_eq!(
        doc(&h).active_layer().rect(),
        PixelRect {
            x: 100,
            y: 100,
            w: 100,
            h: 50
        }
    );
}

#[test]
fn ctrl_t_then_the_distort_mode_drags_without_modifier() {
    let mut h = app();
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert!(h.state().transform_pending());
    h.state_mut()
        .run(Action::Transform(Command::Mode(Mode::Distort)));
    let (from, to) = (screen(&h, 100.0, 150.0), screen(&h, 90.0, 180.0));
    drag(&mut h, from, to, Modifiers::NONE);
    let q = quad(&h);
    assert!(near(q[3], pos2(90.0, 180.0)), "{q:?}");
    assert!(near(q[2], pos2(200.0, 150.0)));
}

#[test]
fn the_edit_menu_lists_the_transforms_and_flips() {
    let mut h = app();
    h.get_by_label("Edit").click();
    h.run_steps(2);
    h.get_by_label(&format!("{} ⏵", t!("xform-menu"))).hover();
    h.run_steps(3);
    let again = format!(
        "{} {}",
        t!("xform-again"),
        keys::shortcut(&h.ctx, &super::transform::SHORTCUT_AGAIN)
    );
    assert!(h.query_by_label(&again).is_some(), "{again}");
    for label in [
        t!("xform-scale"),
        t!("xform-rotate"),
        t!("xform-skew"),
        t!("xform-distort"),
        t!("xform-perspective"),
        t!("xform-warp"),
        t!("xform-rotate-180"),
        t!("optbar-rotate-cw"),
        t!("optbar-rotate-ccw"),
        t!("xform-flip-vertical"),
    ] {
        assert!(h.query_by_label(&label).is_some(), "{label}");
    }
    h.get_by_label(&t!("xform-flip-horizontal")).click();
    h.run_steps(3);
    // Flipped in place, at once: one step.
    let d = doc(&h);
    assert_eq!(
        d.active_layer().rect(),
        PixelRect {
            x: 100,
            y: 100,
            w: 100,
            h: 50
        }
    );
    assert_eq!(
        d.history().0.last().copied(),
        Some(t!("xform-flip-horizontal").as_str())
    );
    assert!(h.state().transform_rect.is_none());
}

#[test]
fn right_click_during_free_transform_offers_its_modes() {
    let mut h = app();
    key(&mut h, Key::T, Modifiers::COMMAND);
    let p = screen(&h, 150.0, 125.0);
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
    h.run_steps(2);
    for label in [t!("xform-perspective"), t!("xform-flip-vertical")] {
        assert!(h.query_by_label(&label).is_some(), "{label}");
    }
    h.get_by_label(&t!("xform-warp")).click();
    h.run_steps(3);
    assert!(h.state().transform_warping());
    // A drag in the grid bends it; Enter applies the warp.
    let (from, to) = (screen(&h, 150.0, 110.0), screen(&h, 150.0, 80.0));
    drag(&mut h, from, to, Modifiers::NONE);
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(color_at(&h, 150, 95), BLUE.0, "pulled up");
    assert_eq!(color_at(&h, 102, 148), BLUE.0, "the corners stay");
}

#[test]
fn transform_again_repeats_the_last_one() {
    let mut h = app();
    h.state_mut().run(Action::Transform(Command::RotateCw));
    h.run_steps(1);
    // 50 × 100 around the same center.
    let r = doc(&h).active_layer().rect();
    assert_eq!((r.w, r.h), (50, 100));
    key(&mut h, Key::T, Modifiers::COMMAND | Modifiers::SHIFT);
    let r = doc(&h).active_layer().rect();
    assert_eq!((r.w, r.h), (100, 50));
    assert_eq!(
        doc(&h).history().0.last().copied(),
        Some(t!("xform-history-again").as_str())
    );
}

#[test]
fn free_transform_of_a_selection_lifts_its_pixels() {
    let mut h = app();
    h.state_mut().selection = Some(
        PixelRect {
            x: 100,
            y: 100,
            w: 50,
            h: 50,
        }
        .into(),
    );
    let steps = doc(&h).history().1;
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert_eq!(
        h.state().transform_source(),
        Some(Rect::from_min_size(pos2(100.0, 100.0), vec2(50.0, 50.0)))
    );
    // Moved 100 px to the right with the arrows.
    for _ in 0..10 {
        key(&mut h, Key::ArrowRight, Modifiers::SHIFT);
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 2, "dropped back on its layer");
    assert_eq!(d.history().1, steps + 1, "one step");
    let logo = &d.layers[1];
    assert_eq!(logo.alpha_at(120, 120), 0, "a transparent hole");
    assert_eq!(logo.pixel_at(170, 120), Some(BLUE.0), "kept");
    assert_eq!(logo.pixel_at(220, 120), Some(BLUE.0), "moved");
    assert_eq!(
        h.state().selection.as_ref().map(|s| s.rect),
        Some(PixelRect {
            x: 200,
            y: 100,
            w: 50,
            h: 50
        }),
        "the selection follows"
    );
    // Undone at once.
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    assert_eq!(doc(&h).layers[1].pixel_at(120, 120), Some(BLUE.0));
}

#[test]
fn escape_puts_lifted_pixels_back() {
    let mut h = app();
    h.state_mut().selection = Some(
        PixelRect {
            x: 100,
            y: 100,
            w: 50,
            h: 50,
        }
        .into(),
    );
    let steps = doc(&h).history();
    let steps = (steps.0.len(), steps.1);
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert_eq!(doc(&h).layers.len(), 3, "floating");
    key(&mut h, Key::Escape, Modifiers::NONE);
    let d = doc(&h);
    assert_eq!(d.layers.len(), 2);
    assert_eq!(d.layers[1].pixel_at(120, 120), Some(BLUE.0));
    assert_eq!(
        (d.history().0.len(), d.history().1),
        steps,
        "nothing recorded"
    );
    assert!(h.state().selection.is_some());
}

#[test]
fn an_adjustment_layer_transforms_its_mask() {
    let mut h = app();
    let sel: Selection = PixelRect {
        x: 0,
        y: 0,
        w: 100,
        h: 300,
    }
    .into();
    {
        let app = h.state_mut();
        let doc = app.doc.as_mut().unwrap();
        doc.add_adjustment_layer(crate::pixel_ops::Adjustment::Invert, Some(&sel));
    }
    h.run_steps(1);
    assert_eq!(
        color_at(&h, 50, 50),
        [55, 255, 255, 255],
        "inverted where the mask shows"
    );
    assert_eq!(color_at(&h, 350, 50), RED.0);
    // The mask's box, moved by the arrows: the mask alone moves.
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert_eq!(
        h.state().transform_source(),
        Some(Rect::from_min_size(Pos2::ZERO, vec2(100.0, 300.0)))
    );
    for _ in 0..25 {
        key(&mut h, Key::ArrowRight, Modifiers::SHIFT);
    }
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!(color_at(&h, 50, 50), RED.0);
    assert_eq!(color_at(&h, 300, 50), [55, 255, 255, 255]);
}

#[test]
fn selected_layers_transform_together() {
    let mut h = app();
    {
        let app = h.state_mut();
        let doc = app.doc.as_mut().unwrap();
        let green = image::RgbaImage::from_pixel(20, 20, image::Rgba([0, 200, 0, 255]));
        doc.add_layer("Dot".into(), green, (300, 100));
        app.layer_set = doc.layers[1..].iter().map(|l| l.id).collect();
    }
    let steps = doc(&h).history().1;
    h.state_mut()
        .run(Action::Transform(Command::FlipHorizontal));
    h.run_steps(1);
    // Mirrored within their common box, x 100 to 320.
    let d = doc(&h);
    assert_eq!(d.layers[1].content_bounds().map(|r| r.x), Some(220));
    assert_eq!(d.layers[2].content_bounds().map(|r| r.x), Some(100));
    assert_eq!(d.history().1, steps + 1);
}

#[test]
fn large_transforms_are_computed_aside() {
    let mut h = app();
    {
        let doc = h.state_mut().doc.as_mut().unwrap();
        let mut big = image::RgbaImage::from_pixel(2400, 2000, BLUE);
        big.put_pixel(0, 0, RED);
        doc.add_layer("Big".into(), big, (0, 0));
    }
    let steps = doc(&h).history().1;
    h.state_mut()
        .run(Action::Transform(Command::FlipHorizontal));
    assert!(h.state().transform_busy(), "on another thread");
    for _ in 0..200 {
        h.run_steps(1);
        if !h.state().transform_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(!h.state().transform_busy());
    let d = doc(&h);
    assert_eq!(d.history().1, steps + 1);
    assert_eq!(d.active_layer().pixel_at(2399, 0), Some(RED.0));
    assert!(h.state().transform_rect.is_none());
}
