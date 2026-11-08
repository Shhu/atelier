//! Recording and playing actions: Cut recorded as a Cut, a held function
//! key playing once, a recorded turn keeping its shape on a layer of
//! another shape, opacity and blend mode, Color Range, what isn't
//! recorded told, function keys of loaded sets.

use super::*;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

type H = Harness<'static, AtelierApp>;

/// A background `size` and, over it, an active layer `top` at (10, 10).
fn doc(size: (u32, u32), top: (u32, u32)) -> Document {
    let image = RgbaImage::from_fn(size.0, size.1, |x, y| {
        Rgba([(x * 2) as u8, (y * 3) as u8, 200, 255])
    });
    let mut d = Document::new(image, None);
    let layer = RgbaImage::from_fn(top.0, top.1, |x, y| {
        Rgba([255 - (x * 3) as u8, (y * 4) as u8, 40, 255])
    });
    d.add_layer("Top".into(), layer, (10, 10));
    d
}

fn app_with(doc: Document) -> H {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(doc);
            app
        });
    h.run_steps(2);
    h
}

fn settle(h: &mut H, done: impl Fn(&AtelierApp) -> bool) {
    for _ in crate::testing::patience() {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("not done ({})", h.state().status);
}

fn idle(app: &AtelierApp) -> bool {
    !app.actions.playing()
        && !app.actions_wait()
        && !app.transform_pending()
        && !app.transform_busy()
}

fn with_action(h: &mut H, steps: Vec<Step>, key: Option<FnKey>) {
    let a = &mut h.state_mut().actions;
    a.library.sets = vec![Set {
        name: "Test".into(),
        actions: vec![Sequence {
            name: "A".into(),
            key,
            steps,
            expanded: true,
        }],
        expanded: true,
    }];
    a.selected = Some(Row::Action(0, 0));
}

/// What `f` records in a new action.
fn record(h: &mut H, f: impl FnOnce(&mut H)) -> Vec<Step> {
    with_action(h, Vec::new(), None);
    h.state_mut().actions.start_recording((0, 0));
    h.run_steps(1);
    f(h);
    settle(h, idle);
    h.state_mut().actions.stop_recording();
    h.state().actions.library.sets[0].actions[0].steps.clone()
}

fn play(h: &mut H, steps: Vec<Step>) {
    with_action(h, steps, None);
    h.state_mut().panel_click(PanelClick::Play);
    settle(h, idle);
    assert!(h.state().actions.message.is_none(), "played to the end");
}

/// Size of the opaque part of the active layer.
fn opaque_size(h: &H) -> (u32, u32) {
    let d = h.state().doc.as_ref().unwrap();
    let image = d.active_layer().image();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in image.enumerate_pixels() {
        if p.0[3] > 0 {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    (x1 + 1 - x0, y1 + 1 - y0)
}

#[test]
fn a_cut_is_recorded_as_a_cut_and_played_clears() {
    let selection = || {
        Some(
            PixelRect {
                x: 20,
                y: 20,
                w: 30,
                h: 20,
            }
            .into(),
        )
    };
    let mut a = app_with(doc((120, 80), (60, 40)));
    a.state_mut().selection = selection();
    let steps = record(&mut a, |h| h.state_mut().run(Action::Cut));
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0].command,
        Recorded::Menu {
            item: MenuItem::Cut
        }
    );
    let expected = a.state().doc.as_ref().unwrap().flatten();
    let mut b = app_with(doc((120, 80), (60, 40)));
    b.state_mut().selection = selection();
    play(&mut b, steps);
    assert_eq!(b.state().doc.as_ref().unwrap().flatten(), expected, "the pixels cut");
}

/// A function key held down plays its action once, not on each repeat.
#[test]
fn a_held_function_key_plays_its_action_once() {
    let mut h = app_with(doc((40, 30), (20, 10)));
    let f2 = FnKey {
        number: 2,
        shift: false,
        ctrl: false,
    };
    with_action(
        &mut h,
        vec![Step::new(Recorded::Adjust {
            adjustment: Adjustment::Invert,
        })],
        Some(f2),
    );
    let before = h.state().doc.as_ref().unwrap().history().1;
    let key = |repeat| egui::Event::Key {
        key: egui::Key::F2,
        physical_key: None,
        pressed: true,
        repeat,
        modifiers: egui::Modifiers::NONE,
    };
    h.event(key(false));
    h.run_steps(1);
    settle(&mut h, idle);
    for _ in 0..3 {
        h.event(key(true));
        h.run_steps(1);
        settle(&mut h, idle);
    }
    let (_, done) = h.state().doc.as_ref().unwrap().history();
    assert_eq!(done, before + 1, "played once");
}

/// Rotate 90° recorded on a 60 × 40 layer turns a 90 × 70 one into
/// 70 × 90, as the command itself does (the corners alone gave 60 × 105).
#[test]
fn a_recorded_turn_keeps_its_shape_on_a_layer_of_another_shape() {
    let mut a = app_with(doc((120, 80), (60, 40)));
    let steps = record(&mut a, |h| {
        h.state_mut()
            .run(Action::Transform(transform::Command::RotateCw))
    });
    let [Step {
        command: Recorded::Transform {
            frame: Some(frame), ..
        },
        ..
    }] = steps.as_slice()
    else {
        panic!("a transform with its frame: {steps:?}");
    };
    assert!((frame.angle - 90.0).abs() < 1e-6, "{frame:?}");
    assert!((frame.scale[0] - 1.0).abs() < 1e-6 && (frame.scale[1] - 1.0).abs() < 1e-6);
    let mut direct = app_with(doc((200, 150), (90, 70)));
    direct
        .state_mut()
        .run(Action::Transform(transform::Command::RotateCw));
    settle(&mut direct, idle);
    assert_eq!(opaque_size(&direct), (70, 90));
    let mut played = app_with(doc((200, 150), (90, 70)));
    play(&mut played, steps);
    assert_eq!(opaque_size(&played), (70, 90));
}

/// The frames of corners: plain moves, scales, turns, flips and skews come
/// back as they were, on any box; a distortion has none.
#[test]
fn transform_frames_round_trip() {
    let cases: [[[f64; 2]; 4]; 4] = [
        // Half as wide, from the left.
        [[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]],
        // Flipped horizontally.
        [[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
        // Skewed.
        [[0.2, 0.0], [1.2, 0.0], [1.0, 1.0], [0.0, 1.0]],
        // Turned 90° about the center of a 60 × 40 box (in its units).
        [[5.0 / 6.0, -0.25], [5.0 / 6.0, 1.25], [1.0 / 6.0, 1.25], [1.0 / 6.0, -0.25]],
    ];
    for corners in cases {
        let frame = act::Frame::of(corners, 60.0, 40.0).expect("a frame");
        let back = frame.corners(60.0, 40.0);
        for (p, q) in back.iter().zip(corners) {
            assert!((p[0] - q[0]).abs() < 1e-9 && (p[1] - q[1]).abs() < 1e-9, "{back:?} {corners:?}");
        }
    }
    let turned = act::Frame::of(cases[3], 60.0, 40.0).unwrap();
    assert!((turned.angle - 90.0).abs() < 1e-9);
    let skewed = act::Frame::of(cases[2], 60.0, 40.0).unwrap();
    assert!(skewed.skew.abs() > 1.0 && skewed.angle.abs() < 1e-9);
    // In perspective: no frame, the corners are played.
    let perspective = [[0.1, 0.0], [0.9, 0.0], [1.0, 1.0], [0.0, 1.0]];
    assert!(act::Frame::of(perspective, 60.0, 40.0).is_none());
}

/// Opacity and blend mode are recorded, a series of changes as one step,
/// and played on the selected layer of another document.
#[test]
fn layer_opacity_and_blend_mode_are_recorded() {
    let mut a = app_with(doc((60, 40), (20, 20)));
    let steps = record(&mut a, |h| {
        h.state_mut().set_layers_look(Some(0.3), None);
        h.state_mut().set_layers_look(Some(0.5), None);
        h.state_mut().run(Action::SetBlend(BlendMode::Multiply));
    });
    assert_eq!(
        steps.iter().map(|s| s.command.clone()).collect::<Vec<_>>(),
        [
            Recorded::LayerLook {
                opacity: Some(0.5),
                mode: None
            },
            Recorded::LayerLook {
                opacity: None,
                mode: Some("Multiply".into())
            },
        ]
    );
    let mut b = app_with(doc((90, 70), (30, 30)));
    play(&mut b, steps);
    let layer = b.state().doc.as_ref().unwrap().active_layer();
    assert_eq!((layer.opacity, layer.blend), (0.5, BlendMode::Multiply));
}

/// Color Range is recorded with its colors and played on another document.
#[test]
fn color_range_is_recorded_and_played() {
    let red_square = || {
        let mut d = Document::new(RgbaImage::from_pixel(60, 40, Rgba([255; 4])), None);
        let red = RgbaImage::from_pixel(20, 10, Rgba([255, 0, 0, 255]));
        d.add_layer("Red".into(), red, (5, 7));
        d
    };
    let mut a = app_with(red_square());
    a.state_mut().primary = Color32::from_rgb(255, 0, 0);
    let steps = record(&mut a, |h| {
        h.state_mut()
            .run(Action::Select(select_menu::Command::ColorRange));
        settle(h, |app| {
            app.sel
                .color_range
                .as_ref()
                .is_some_and(|cr| cr.ready())
        });
        h.key_press(egui::Key::Enter);
    });
    assert!(
        matches!(steps.as_slice(), [Step { command: Recorded::ColorRange { samples, .. }, .. }] if samples.len() == 1),
        "{steps:?}"
    );
    let expected = a.state().selection_bounds();
    assert_eq!(
        expected,
        Some(PixelRect {
            x: 5,
            y: 7,
            w: 20,
            h: 10
        })
    );
    let mut b = app_with(red_square());
    play(&mut b, steps);
    assert_eq!(b.state().selection_bounds(), expected);
}

/// While recording, a change that adds no step (a tool's work, a window
/// that doesn't record) says so; a recorded command, Undo and Redo don't.
#[test]
fn what_is_not_recorded_is_told() {
    let mut h = app_with(doc((40, 30), (20, 10)));
    with_action(&mut h, Vec::new(), None);
    h.state_mut().actions.start_recording((0, 0));
    h.run_steps(2);
    let not_recorded = t!("smart3-actions-not-recorded");
    // Recorded.
    h.state_mut().run(Action::Adjust(AdjustKind::Invert));
    h.run_steps(2);
    assert_ne!(h.state().status.to_string(), not_recorded);
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    h.state_mut().run(Action::Redo);
    h.run_steps(2);
    assert_ne!(h.state().status.to_string(), not_recorded);
    // A tool's step, made outside any command.
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.checkpoint("Brush".into());
    doc.active_layer_mut().opacity = 0.5;
    h.run_steps(2);
    assert_eq!(h.state().status.to_string(), not_recorded);
}

/// Load Actions: a loaded action whose function key is already used (or
/// used twice in the file) loses it.
#[test]
fn loaded_sets_never_take_a_function_key_already_used() {
    let key = |number| {
        Some(FnKey {
            number,
            shift: false,
            ctrl: false,
        })
    };
    let action = |name: &str, key| Sequence {
        name: name.into(),
        key,
        steps: Vec::new(),
        expanded: false,
    };
    let set = |actions| Set {
        name: "S".into(),
        actions,
        expanded: false,
    };
    let mut library = Library {
        sets: vec![set(vec![action("Mine", key(2))])],
    };
    let dropped = library.add_sets(vec![set(vec![
        action("Loaded F2", key(2)),
        action("Loaded F3", key(3)),
        action("Loaded F3 again", key(3)),
    ])]);
    assert_eq!(dropped, 2);
    let keys: Vec<_> = library.sets[1].actions.iter().map(|a| a.key).collect();
    assert_eq!(keys, [None, key(3), None]);
    assert_eq!(library.with_key(key(2).unwrap()), Some((0, 0)));
}

/// A step whose command is known but whose settings can't be read says
/// so, rather than asking for a newer version.
#[test]
fn a_damaged_step_is_told_apart_from_a_newer_command() {
    let damaged = Recorded::Unknown(serde_json::json!({
        "command": "image-size", "width": -5, "height": 10
    }));
    let newer = Recorded::Unknown(serde_json::json!({"command": "liquify-3d"}));
    assert!(damaged.damaged());
    assert!(!newer.damaged());
    let mut h = app_with(doc((40, 30), (20, 10)));
    assert_eq!(h.state_mut().play_step(&damaged, false), Err(t!("batch14-error-damaged")));
    assert_eq!(h.state_mut().play_step(&newer, false), Err(t!("actions-error-unknown")));
}
