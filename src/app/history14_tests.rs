//! History invariants through the app: one step per command that changes
//! something, none for one that doesn't, Undo giving back exactly what was
//! there (gradient fills turned or flipped with the canvas, panel fields
//! that never write back on their own, masks, discarded steps).

use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

use super::*;
use crate::compose::gradient::GradientFill;
use crate::document::{Fill, NewMask};

type H = Harness<'static, AtelierApp>;

/// A 64 × 48 opaque gradient image, `prepare` applied to its document.
fn app_with(prepare: impl FnOnce(&mut Document) + 'static) -> H {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = RgbaImage::from_fn(64, 48, |x, y| {
                Rgba([(x * 4) as u8, (y * 5) as u8, 128, 255])
            });
            let mut doc = Document::new(image, None);
            prepare(&mut doc);
            app.doc = Some(doc);
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn app() -> H {
    app_with(|_| {})
}

fn doc(h: &H) -> &Document {
    h.state().doc.as_ref().unwrap()
}

/// Runs frames until the background work is over.
fn settle(h: &mut H) {
    for _ in crate::testing::patience() {
        h.run_steps(1);
        let a = h.state();
        if !(a.filters.busy() || a.transform_busy() || a.smart.busy() || a.select_busy()) {
            h.run_steps(2);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    panic!("background work not done");
}

/// Steps done, and the name of the last one.
fn steps(h: &H) -> (usize, Option<String>) {
    let (names, done) = doc(h).history();
    (done, done.checked_sub(1).map(|i| names[i].to_owned()))
}

fn angle(h: &H) -> f32 {
    match &doc(h).active_layer().kind {
        LayerKind::Fill(Fill::Gradient(g)) => g.angle,
        _ => panic!("not a gradient fill"),
    }
}

fn gradient_fill(angle: f32) -> impl FnOnce(&mut Document) + 'static {
    move |d| {
        let fill = GradientFill {
            angle,
            ..Default::default()
        };
        d.add_fill_layer(Fill::Gradient(fill), None);
    }
}

/// Image Rotation 180° with a gradient fill selected (shown in the
/// Properties panel): one step, the angle within (-180, 180] where the
/// panel shows it, and Undo goes back past it.
#[test]
fn rotating_a_gradient_fill_is_one_step_that_undoes() {
    let mut h = app_with(gradient_fill(90.0));
    let (before, _) = steps(&h);
    h.state_mut().run(Action::RotateCanvas(180));
    settle(&mut h);
    assert_eq!(angle(&h), -90.0);
    assert_eq!(steps(&h).0, before + 1, "{:?}", doc(&h).history());
    h.state_mut().run(Action::Undo);
    h.run_steps(3);
    assert_eq!((angle(&h), steps(&h).0), (90.0, before));
    // Arbitrary angles too.
    h.state_mut().run(Action::Redo);
    h.run_steps(3);
    let mut g = GradientFill::default();
    for (turn, expected) in [(90.0, 0.0), (-100.0, -170.0), (270.0, 180.0)] {
        g.angle = 90.0;
        g.turned(turn);
        assert_eq!(g.angle, expected, "turned by {turn}");
    }
}

/// A value the panel's field can't take (from a file, an older version)
/// is shown as it is: never written back, so no step is recorded.
#[test]
fn panel_fields_never_write_back_on_their_own() {
    let mut h = app_with(|d| {
        gradient_fill(270.0)(d);
        // A mask density no slider step gives.
        d.add_mask(NewMask::RevealAll, None);
        d.layers[1].mask.as_mut().unwrap().density = 0.555;
    });
    let (before, modified) = (steps(&h), doc(&h).modified);
    h.run_steps(5);
    assert_eq!(angle(&h), 270.0);
    assert_eq!(doc(&h).active_layer().mask.as_ref().unwrap().density, 0.555);
    assert_eq!((steps(&h), doc(&h).modified), (before, modified));
}

/// Flip Canvas mirrors a gradient fill with the rest: the image is the
/// mirror image of what it was.
#[test]
fn flipping_the_canvas_mirrors_gradient_fills() {
    for (horizontal, start) in [(false, 90.0), (true, 30.0)] {
        let mut h = app_with(gradient_fill(start));
        let before = doc(&h).flatten();
        let (n, _) = steps(&h);
        h.state_mut().run(Action::FlipCanvas(horizontal));
        settle(&mut h);
        let after = doc(&h).flatten();
        let mirrored = match horizontal {
            true => image::imageops::flip_horizontal(&before),
            false => image::imageops::flip_vertical(&before),
        };
        let off = after
            .pixels()
            .zip(mirrored.pixels())
            .filter(|(a, b)| a.0.iter().zip(b.0).any(|(&x, y)| x.abs_diff(y) > 2))
            .count();
        assert_eq!(off, 0, "horizontal {horizontal}: angle now {}", angle(&h));
        assert_eq!(steps(&h).0, n + 1);
    }
    // An Angle gradient also sweeps the other way.
    let mut g = GradientFill {
        style: crate::paint::gradient::Shape::Angle,
        angle: 30.0,
        ..Default::default()
    };
    g.mirrored(true);
    assert_eq!((g.angle, g.reverse), (150.0, true));
}

/// An empty group converted to a smart object: Normal, not Pass Through
/// (a mode only groups have), and it reads back as saved.
#[test]
fn an_empty_group_becomes_a_normal_smart_object() {
    let mut h = app();
    h.state_mut().run(Action::NewGroup);
    settle(&mut h);
    h.state_mut().run(Action::Smart(smart_ui::Command::Convert));
    settle(&mut h);
    let layer = doc(&h).active_layer();
    assert!(layer.smart.is_some() && !layer.is_group());
    assert_eq!(layer.blend, crate::compose::BlendMode::Normal);
}

/// A full history and several steps discarded in a row (a rollback): the
/// oldest steps they let go all come back.
#[test]
fn discarded_steps_give_back_every_step_they_let_go() {
    let mut d = Document::new(RgbaImage::from_pixel(4, 4, Rgba([0, 0, 0, 255])), None);
    for k in 0..50u8 {
        d.checkpoint(format!("s{k}"));
        d.layers[0].replace_pixels(RgbaImage::from_pixel(4, 4, Rgba([k, 0, 0, 255])));
    }
    let names = |d: &Document| d.history().0.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let before = names(&d);
    let mark = d.history_mark();
    for k in 0..3 {
        d.checkpoint(format!("nothing {k}"));
    }
    assert!(d.rollback_to(mark));
    assert_eq!(names(&d), before);
    assert!(!d.history_forgotten());
}

/// Edit › Cut is called so in the History panel.
#[test]
fn cut_is_named_cut() {
    let mut h = app();
    let area = crate::document::PixelRect {
        x: 4,
        y: 4,
        w: 10,
        h: 10,
    };
    h.state_mut().selection = Some(area.into());
    h.state_mut().run(Action::Cut);
    settle(&mut h);
    assert_eq!(steps(&h).1.as_deref(), Some(t!("menu-edit-cut").as_str()));
}

/// A pixel command on a targeted mask without an image of its own (Reveal
/// All): Undo gives back the mask as it was, not grown to the canvas.
#[test]
fn undo_gives_back_the_mask_as_it_was() {
    let mut h = app_with(|d| {
        d.add_mask(NewMask::RevealAll, None);
    });
    assert!(doc(&h).mask_targeted());
    let size = |h: &H| doc(h).active_layer().mask.as_ref().unwrap().image().dimensions();
    assert_eq!(size(&h), (0, 0));
    h.state_mut().run(Action::Adjust(crate::pixel_ops::AdjustKind::Invert));
    settle(&mut h);
    assert_eq!(size(&h), (64, 48));
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    assert_eq!(size(&h), (0, 0));
}

/// Commands with nothing to do record nothing (and leave the document
/// unmodified): on an empty layer, the pixel commands say the area is
/// empty; Clear and Disable Smart Filters without filters, Flatten Image
/// of a single background, Assign Profile of the profile it has.
#[test]
fn commands_with_nothing_to_do_record_nothing() {
    use crate::pixel_ops::AdjustKind;
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    settle(&mut h);
    for action in [
        Action::Transform(transform::Command::Rotate180),
        Action::Transform(transform::Command::FlipHorizontal),
        Action::Transform(transform::Command::Free),
        Action::Adjust(AdjustKind::Invert),
        Action::Adjust(AdjustKind::Levels),
        Action::Desaturate,
    ] {
        let before = steps(&h);
        h.state_mut().run(action);
        settle(&mut h);
        assert_eq!(steps(&h), before, "{action:?}");
        assert_eq!(h.state().status.to_string(), t!("xform-status-empty"), "{action:?}");
        assert!(h.state().adjust.is_none() && h.state().transform_rect.is_none());
    }
    h.state_mut().start_filter(crate::filters::Filter::GaussianBlur { radius: 2.0 });
    settle(&mut h);
    assert_eq!(h.state().status.to_string(), t!("xform-status-empty"));

    let mut h = app();
    h.state_mut().doc.as_mut().unwrap().modified = false;
    let before = steps(&h);
    h.state_mut().run(Action::FlattenImage);
    h.state_mut().assign_profile(None, false);
    settle(&mut h);
    assert_eq!(steps(&h), before);
    assert!(!doc(&h).modified);

    h.state_mut().run(Action::Smart(smart_ui::Command::Convert));
    settle(&mut h);
    let before = steps(&h);
    h.state_mut().run(Action::Smart(smart_ui::Command::ClearFilters));
    h.state_mut().run(Action::Smart(smart_ui::Command::ToggleFilters));
    settle(&mut h);
    assert_eq!(steps(&h), before);
}
