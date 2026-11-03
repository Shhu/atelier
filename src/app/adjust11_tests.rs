//! Fade over what each step really covered, and actions that go on after a
//! step that changes nothing.

use super::*;
use crate::actions::{MenuItem, Recorded, Step};
use crate::pixel_ops::Adjustment;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

type H = Harness<'static, AtelierApp>;

fn app_with(image: RgbaImage) -> H {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(image, None));
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn plain(rgb: [u8; 3]) -> H {
    app_with(RgbaImage::from_pixel(40, 30, Rgba([rgb[0], rgb[1], rgb[2], 255])))
}

fn settle(h: &mut H) {
    for _ in 0..1000 {
        h.run_steps(1);
        if !h.state().filters.busy() && !h.state().actions.playing() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("background work not done");
}

fn doc(h: &H) -> &Document {
    h.state().doc.as_ref().unwrap()
}

fn pixel(h: &H, x: i64, y: i64) -> [u8; 4] {
    doc(h).active_layer().pixel_at(x, y).unwrap_or([0; 4])
}

/// Plays `steps` as an action, to the end.
fn play(h: &mut H, steps: Vec<Step>) {
    use crate::actions::{Library, Sequence, Set};
    h.state_mut().actions.library = Library {
        sets: vec![Set {
            name: "S".into(),
            expanded: true,
            actions: vec![Sequence {
                name: "A".into(),
                key: None,
                expanded: false,
                steps,
            }],
        }],
    };
    assert!(h.state_mut().play_for_batch((0, 0)));
    settle(h);
}

fn fade(opacity: f32, mode: &str) -> Step {
    Step::new(Recorded::Fade {
        opacity,
        mode: mode.into(),
    })
}

fn invert() -> Step {
    Step::new(Recorded::Adjust {
        adjustment: Adjustment::Invert,
    })
}

/// Fade at once, as an action plays it.
fn fade_now(h: &mut H, opacity: f32, mode: BlendMode) {
    h.state_mut().fade_with(opacity, mode).expect("something to fade");
    settle(h);
}

#[test]
fn a_stroke_that_changes_nothing_doesnt_stop_the_action() {
    let mut h = plain([200, 100, 50]);
    // Outside the edge of an opaque layer, without a selection: nothing.
    let stroke = Step::new(Recorded::Stroke {
        width: 3,
        color: [255, 0, 0],
        location: "Outside".into(),
        mode: "Normal".into(),
        opacity: 1.0,
        preserve_transparency: false,
    });
    play(&mut h, vec![stroke, fade(0.5, "Multiply"), invert()]);
    assert_eq!(pixel(&h, 0, 0), [55, 155, 205, 255], "the Invert ran, unfaded");
    assert_eq!(doc(&h).history().0.last(), Some(&AdjustKind::Invert.title().as_str()));
}

#[test]
fn a_fade_step_with_its_window_after_a_no_op_lets_the_action_go_on() {
    // Already at full range: Auto Tone changes nothing.
    let image = RgbaImage::from_fn(40, 30, |x, _| {
        let v = if x < 20 { 0 } else { 255 };
        Rgba([v, v, v, 255])
    });
    let mut h = app_with(image);
    let mut faded = fade(0.5, "Normal");
    faded.dialog = true;
    let auto = Step::new(Recorded::Menu {
        item: MenuItem::AutoTone,
    });
    play(&mut h, vec![auto, faded, invert()]);
    assert!(h.state().filters.fade.is_none(), "no window opened");
    assert_eq!(pixel(&h, 0, 0), [255, 255, 255, 255], "the Invert ran");
    assert_ne!(h.state().status, t!("adjust9-why-no-fade"));
}

#[test]
fn a_neutral_adjustment_leaves_nothing_to_fade() {
    let mut h = plain([200, 100, 50]);
    let exposure = Step::new(Recorded::Adjust {
        adjustment: Adjustment::Exposure(Default::default()),
    });
    play(&mut h, vec![invert(), exposure, fade(0.5, "Normal")]);
    assert_eq!(pixel(&h, 0, 0), [55, 155, 205, 255], "the Invert stays whole");

    // From its window too.
    let mut h = plain([200, 100, 50]);
    h.state_mut().run(Action::Adjust(AdjustKind::Invert));
    h.run_steps(1);
    assert!(doc(&h).fade_name().is_some());
    h.state_mut().apply_adjustment(Adjustment::Exposure(Default::default()));
    assert!(doc(&h).fade_name().is_none(), "not the Invert any more");
}

#[test]
fn fade_blends_all_the_paint_bucket_covered() {
    // Columns of 200 and 201: the tolerance covers both, only the 201s change.
    let image = RgbaImage::from_fn(40, 30, |x, _| {
        let v = 200 + (x % 2) as u8;
        Rgba([v, v, v, 255])
    });
    let mut h = app_with(image);
    h.state_mut().primary = Color32::from_rgb(200, 200, 200);
    h.state_mut().bucket_fill(egui::pos2(5.0, 5.0));
    h.run_steps(1);
    assert!(doc(&h).fade_name().is_some());
    fade_now(&mut h, 1.0, BlendMode::Multiply);
    let (a, b) = (pixel(&h, 0, 3)[0], pixel(&h, 1, 3)[0]);
    assert!(a.abs_diff(b) <= 1, "no stripes: {a} {b}");
    assert!(a < 165, "the unchanged column is blended too: {a}");
}

#[test]
fn fade_blends_all_the_stroke_band() {
    // Gray on the left, white on the right; a gray stroke over both.
    let image = RgbaImage::from_fn(40, 30, |x, _| {
        Rgba(if x < 20 { [128, 128, 128, 255] } else { [255; 4] })
    });
    let mut h = app_with(image);
    h.state_mut().selection = Some(
        PixelRect {
            x: 5,
            y: 5,
            w: 30,
            h: 20,
        }
        .into(),
    );
    h.state_mut().apply_stroke(crate::pixel_ops::StrokeSettings {
        width: 4,
        color: [128, 128, 128],
        location: crate::pixel_ops::StrokeLocation::Inside,
        mode: BlendMode::Normal,
        opacity: 1.0,
        preserve_transparency: false,
    });
    settle(&mut h);
    fade_now(&mut h, 1.0, BlendMode::Multiply);
    assert_eq!(pixel(&h, 33, 15)[0], 128, "on white");
    assert!(pixel(&h, 6, 15)[0].abs_diff(64) <= 1, "on gray: {:?}", pixel(&h, 6, 15));
    assert_eq!(pixel(&h, 12, 15)[0], 128, "inside the band stays");
}

#[test]
fn fade_blends_where_the_brush_went() {
    let image = RgbaImage::from_fn(40, 30, |x, _| {
        Rgba(if x < 20 { [128, 128, 128, 255] } else { [255; 4] })
    });
    let mut h = app_with(image);
    {
        let app = h.state_mut();
        app.set_tool(Tool::Brush);
        app.primary = Color32::from_rgb(128, 128, 128);
        (app.brush_size, app.hardness) = (6.0, 1.0);
    }
    h.run_steps(2);
    let view = h.state().last_view.unwrap();
    let (from, to) = (view.to_screen(pos2(4.0, 15.0)), view.to_screen(pos2(36.0, 15.0)));
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(pixel(&h, 30, 15)[0], 128, "painted on white");
    assert!(doc(&h).fade_name().is_some());
    fade_now(&mut h, 1.0, BlendMode::Multiply);
    assert!(pixel(&h, 10, 15)[0].abs_diff(64) <= 2, "on gray: {:?}", pixel(&h, 10, 15));
    assert_eq!(pixel(&h, 10, 3)[0], 128, "away from the dabs");
}

#[test]
fn fade_follows_a_turn_of_the_layer() {
    let mut h = app_with(RgbaImage::from_fn(40, 30, |x, _| Rgba([(x * 6) as u8, 0, 0, 255])));
    h.state_mut().run(Action::Transform(transform::Command::RotateCw));
    settle(&mut h);
    h.run_steps(2);
    let turned = doc(&h).active_layer().rect();
    assert_eq!((turned.w, turned.h), (30, 40));
    assert!(doc(&h).fade_name().is_some(), "a turn can be faded");
    let steps = doc(&h).history().1;
    fade_now(&mut h, 0.5, BlendMode::Normal);
    assert_eq!(doc(&h).history().1, steps + 1, "one step");
    // Outside the turned frame, inside the old one: half of what was there.
    let p = pixel(&h, 0, 0);
    assert!(p[3].abs_diff(128) <= 1, "{p:?}");
    let rect = doc(&h).active_layer().rect();
    assert_eq!(rect.union(turned), rect, "the frame holds both: {rect:?}");
    assert!(h.state_mut().doc.as_mut().unwrap().undo());
    assert_eq!(doc(&h).active_layer().pixel_at(0, 0).map_or(0, |p| p[3]), 0);
}

#[test]
fn a_stroke_step_lists_all_its_settings() {
    let lines = super::actions_panel::step_details(&Recorded::Stroke {
        width: 3,
        color: [255, 0, 16],
        location: "Outside".into(),
        mode: "Multiply".into(),
        opacity: 0.5,
        preserve_transparency: true,
    });
    let all = lines.join("\n");
    assert!(all.contains(&t!("adjust8-stroke-outside")), "{all}");
    assert!(all.contains("#FF0010"), "{all}");
    assert!(all.contains(&t!("dialog-fill-preserve-transparency")), "{all}");
    assert_eq!(lines.len(), 6, "{all}");
}

#[test]
fn stroke_and_auto_on_a_group_dont_speak_of_filters() {
    let mut h = plain([200, 100, 50]);
    h.state_mut().run(Action::NewGroup);
    h.run_steps(1);
    h.state_mut().run(Action::Stroke);
    h.run_steps(1);
    assert_eq!(h.state().status, t!("adjust11-no-pixels"));
    h.state_mut().run(Action::AutoAdjust(crate::pixel_ops::AutoMode::Tone));
    h.run_steps(1);
    assert_eq!(h.state().status, t!("adjust11-no-pixels"));
}
