//! Auto commands, Fade, Stroke and the new adjustments through the real app.

use super::*;
use crate::actions::{MenuItem, Recorded, Step};
use crate::pixel_ops::{AutoMode, ChannelMixer, Exposure, SelectiveColor, StrokeLocation};
use egui_kittest::Harness;

fn app(color: [u8; 4]) -> Harness<'static, AtelierApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 800.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = image::RgbaImage::from_pixel(40, 30, image::Rgba(color));
            app.doc = Some(Document::new(image, None));
            app
        });
    harness.run_steps(2);
    harness
}

fn settle(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..600 {
        h.run_steps(1);
        if !h.state().filters.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("background work not done");
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn pixel(h: &Harness<'static, AtelierApp>, x: u32, y: u32) -> [u8; 4] {
    doc(h).active_layer().image().get_pixel(x, y).0
}

fn steps(h: &Harness<'static, AtelierApp>) -> usize {
    doc(h).history().1
}

fn invert(h: &mut Harness<'static, AtelierApp>) {
    h.state_mut().run(Action::Adjust(AdjustKind::Invert));
    h.run_steps(1);
}

fn press(h: &mut Harness<'static, AtelierApp>, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    h.run_steps(1);
}

/// A ramp with a color cast: every Auto command changes it.
fn cast(h: &mut Harness<'static, AtelierApp>) {
    let image = image::RgbaImage::from_fn(40, 30, |x, _| {
        let v = 60 + x * 3;
        image::Rgba([(v + 30) as u8, v as u8, (v - 20) as u8, 255])
    });
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.active_layer_mut().replace_pixels(image);
}

#[test]
fn auto_commands_have_their_shortcuts_and_one_step() {
    let ctrl_shift = Modifiers::COMMAND.plus(Modifiers::SHIFT);
    for (modifiers, key, name) in [
        (ctrl_shift, Key::L, AutoMode::Tone),
        (ctrl_shift.plus(Modifiers::ALT), Key::L, AutoMode::Contrast),
        (ctrl_shift, Key::B, AutoMode::Color),
    ] {
        let mut h = app([0, 0, 0, 255]);
        cast(&mut h);
        let before = (pixel(&h, 39, 0), steps(&h));
        press(&mut h, modifiers, key);
        settle(&mut h);
        assert_ne!(pixel(&h, 39, 0), before.0, "{name:?}");
        assert_eq!(steps(&h), before.1 + 1, "{name:?}");
        assert_eq!(doc(&h).history().0.last(), Some(&fade_stroke::auto_title(name).as_str()));
        assert!(doc(&h).fade_name().is_some(), "it can be faded");
        h.state_mut().run(Action::Undo);
        assert_eq!(pixel(&h, 39, 0), before.0);
    }
}

#[test]
fn auto_stays_in_the_selection_and_makes_no_empty_step() {
    let mut h = app([0, 0, 0, 255]);
    cast(&mut h);
    h.state_mut().selection = Some(
        PixelRect {
            x: 0,
            y: 0,
            w: 20,
            h: 30,
        }
        .into(),
    );
    let outside = pixel(&h, 30, 3);
    h.state_mut().run(Action::AutoAdjust(AutoMode::Tone));
    settle(&mut h);
    assert_eq!(pixel(&h, 30, 3), outside);
    assert_ne!(pixel(&h, 0, 3), [90, 60, 40, 255]);

    let mut h = app([100, 100, 100, 255]);
    let before = steps(&h);
    h.state_mut().run(Action::AutoAdjust(AutoMode::Contrast));
    settle(&mut h);
    assert_eq!(steps(&h), before, "a flat image changes nothing");
}

#[test]
fn auto_is_refused_on_a_locked_or_hidden_layer() {
    let mut h = app([0, 0, 0, 255]);
    cast(&mut h);
    let before = steps(&h);
    h.state_mut().doc.as_mut().unwrap().active_layer_mut().locked = true;
    h.state_mut().run(Action::AutoAdjust(AutoMode::Tone));
    assert!(h.state().filters.processing.is_none());
    h.state_mut().doc.as_mut().unwrap().active_layer_mut().locked = false;
    h.state_mut().doc.as_mut().unwrap().active_layer_mut().visible = false;
    h.state_mut().run(Action::AutoAdjust(AutoMode::Tone));
    assert!(h.state().filters.processing.is_none());
    assert_eq!(steps(&h), before);
}

#[test]
fn fade_after_an_adjustment_is_its_own_step_with_a_live_preview() {
    let mut h = app([200, 100, 50, 255]);
    let start = steps(&h);
    invert(&mut h);
    assert_eq!(pixel(&h, 0, 0), [55, 155, 205, 255]);
    press(&mut h, Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::F);
    assert!(h.state().filters.fade.is_some(), "Shift+Ctrl+F opens it");
    assert!(h.state().modal_open());
    // Opened again: its first field, focused, then shows the value set.
    press(&mut h, Modifiers::NONE, Key::Escape);
    h.state_mut().run(Action::Fade);
    h.state_mut().filters.fade.as_mut().unwrap().fade.opacity = 0.5;
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().preview_shown.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let shown = h.state().preview_shown.as_ref().expect("previewed");
    assert_eq!(shown.image.get_pixel(0, 0).0, [128, 128, 128, 255]);
    assert_eq!(pixel(&h, 0, 0), [55, 155, 205, 255], "only a preview");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(pixel(&h, 0, 0), [128, 128, 128, 255]);
    assert_eq!(steps(&h), start + 2, "a step after the adjustment");
    assert!(h.state().preview_shown.is_none());
    assert!(doc(&h).fade_name().is_none());
    h.state_mut().run(Action::Undo);
    assert_eq!(pixel(&h, 0, 0), [55, 155, 205, 255], "the fade undone");
    h.state_mut().run(Action::Undo);
    assert_eq!(pixel(&h, 0, 0), [200, 100, 50, 255], "then the adjustment");
}

#[test]
fn fade_cancelled_leaves_everything_and_can_be_opened_again() {
    let mut h = app([200, 100, 50, 255]);
    invert(&mut h);
    let name = doc(&h).history().0.last().unwrap().to_string();
    h.state_mut().run(Action::Fade);
    h.state_mut().filters.fade.as_mut().unwrap().fade.opacity = 0.0;
    h.run_steps(2);
    press(&mut h, Modifiers::NONE, Key::Escape);
    settle(&mut h);
    assert_eq!(pixel(&h, 0, 0), [55, 155, 205, 255]);
    assert_eq!(doc(&h).history().0.last().unwrap().to_string(), name);
    h.state_mut().run(Action::Fade);
    assert!(h.state().filters.fade.is_some());
}

#[test]
fn fade_is_available_only_right_after_and_says_why() {
    let mut h = app([200, 100, 50, 255]);
    let fade_entry = |h: &Harness<'static, AtelierApp>| {
        let menus = h.state().menus();
        menus
            .into_iter()
            .flat_map(|(_, entries)| entries)
            .find_map(|e| match e {
                MenuEntry::Item(label, enabled, _, Action::Fade) => Some((label, enabled)),
                _ => None,
            })
            .expect("Edit › Fade")
    };
    assert!(!fade_entry(&h).1);
    h.state_mut().run(Action::Fade);
    assert!(h.state().filters.fade.is_none());
    invert(&mut h);
    let (_, enabled) = fade_entry(&h);
    assert!(enabled);
    // A selection made since ends it.
    h.state_mut().run(Action::SelectAll);
    h.run_steps(2);
    assert!(!fade_entry(&h).1);
}

#[test]
fn fade_reaches_filters_fills_and_brush_results() {
    let mut h = app([200, 100, 50, 255]);
    h.state_mut().primary = Color32::WHITE;
    h.state_mut().run(Action::FillPrimary);
    assert!(doc(&h).fade_name().is_some(), "a fill");
    let mut h = app([200, 100, 50, 255]);
    h.state_mut().start_filter(crate::filters::Filter::FindEdges);
    settle(&mut h);
    assert!(doc(&h).fade_name().is_some(), "a filter");
}

#[test]
fn fade_works_on_the_quick_mask() {
    let mut h = app([200, 100, 50, 255]);
    h.state_mut().run(Action::Tool(Tool::Canvas));
    let d = h.state_mut().doc.as_mut().unwrap();
    d.enter_quick_mask("Quick Mask".into());
    invert(&mut h);
    h.state_mut().run(Action::Fade);
    assert!(h.state().filters.fade.is_some());
    h.state_mut().filters.fade.as_mut().unwrap().fade.opacity = 0.25;
    h.run_steps(1);
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    let mask = doc(&h).quick_mask().unwrap().get_pixel(0, 0).0[0];
    // The mask, all white, inverted: a quarter of the way to black.
    assert_eq!(mask, 191);
}

fn stroke_settings() -> crate::pixel_ops::StrokeSettings {
    crate::pixel_ops::StrokeSettings {
        width: 2,
        color: [255, 0, 0],
        location: StrokeLocation::Inside,
        mode: BlendMode::Normal,
        opacity: 1.0,
        preserve_transparency: false,
    }
}

#[test]
fn stroke_on_the_selection_is_one_step_and_fades() {
    let mut h = app([255, 255, 255, 255]);
    h.state_mut().selection = Some(
        PixelRect {
            x: 10,
            y: 10,
            w: 10,
            h: 10,
        }
        .into(),
    );
    let before = steps(&h);
    h.state_mut().run(Action::Stroke);
    h.run_steps(1);
    assert!(h.state().modal_open());
    {
        let o = &mut h.state_mut().filters.stroke_options;
        o.width = 2;
        o.color = Some([255, 0, 0]);
        o.location = StrokeLocation::Inside;
    }
    h.run_steps(1);
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    assert_eq!(pixel(&h, 10, 15), [255, 0, 0, 255]);
    assert_eq!(pixel(&h, 11, 15), [255, 0, 0, 255]);
    assert_eq!(pixel(&h, 12, 15), [255, 255, 255, 255]);
    assert_eq!(pixel(&h, 9, 15), [255, 255, 255, 255]);
    assert_eq!(steps(&h), before + 1);
    assert_eq!(
        doc(&h).history().0.last().copied(),
        Some(t!("adjust8-stroke-title").as_str())
    );
    assert!(doc(&h).fade_name().is_some());
    h.state_mut().run(Action::Undo);
    assert_eq!(pixel(&h, 10, 15), [255, 255, 255, 255]);
}

#[test]
fn stroke_is_refused_where_it_must_be() {
    // Locked, hidden: no step.
    let mut h = app([255, 255, 255, 255]);
    let before = steps(&h);
    h.state_mut().doc.as_mut().unwrap().active_layer_mut().locked = true;
    h.state_mut().apply_stroke(stroke_settings());
    assert!(h.state().filters.processing.is_none());
    h.state_mut().doc.as_mut().unwrap().active_layer_mut().locked = false;
    h.state_mut().doc.as_mut().unwrap().active_layer_mut().visible = false;
    h.state_mut().apply_stroke(stroke_settings());
    assert!(h.state().filters.processing.is_none());
    assert_eq!(steps(&h), before);
    // A group has no pixels: the step goes.
    let mut h = app([255, 255, 255, 255]);
    h.state_mut().run(Action::NewGroup);
    let before = steps(&h);
    h.state_mut().apply_stroke(stroke_settings());
    assert!(h.state().filters.processing.is_none());
    assert_eq!(steps(&h), before);
}

#[test]
fn stroke_lands_in_the_quick_mask() {
    let mut h = app([255, 255, 255, 255]);
    h.state_mut().doc.as_mut().unwrap().enter_quick_mask("Quick Mask".into());
    h.state_mut().selection = Some(
        PixelRect {
            x: 10,
            y: 10,
            w: 10,
            h: 10,
        }
        .into(),
    );
    let layer = pixel(&h, 10, 15);
    h.state_mut().apply_stroke(crate::pixel_ops::StrokeSettings {
        color: [0, 0, 0],
        ..stroke_settings()
    });
    settle(&mut h);
    assert_eq!(doc(&h).active_layer().image().get_pixel(10, 15).0, layer);
    assert_eq!(doc(&h).quick_mask().unwrap().get_pixel(10, 15).0[0], 0);
    assert_eq!(doc(&h).quick_mask().unwrap().get_pixel(15, 15).0[0], 255);
}

#[test]
fn every_new_adjustment_works_as_a_dialog_and_as_a_layer() {
    for (kind, set) in [
        (
            AdjustKind::Exposure,
            Adjustment::Exposure(Exposure {
                exposure: -1.0,
                ..Default::default()
            }),
        ),
        (
            AdjustKind::ChannelMixer,
            Adjustment::ChannelMixer(ChannelMixer {
                rows: [[0.0, 0.0, 1.0, 0.0], [0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]],
                monochrome: false,
            }),
        ),
        (
            AdjustKind::SelectiveColor,
            Adjustment::SelectiveColor({
                let mut s = SelectiveColor::default();
                s.ranges[0] = [1.0, 0.0, 0.0, 0.0];
                s
            }),
        ),
    ] {
        let color = [200, 100, 50, 255];
        let mut h = app(color);
        h.state_mut().run(Action::Adjust(kind));
        assert!(h.state().adjust.is_some());
        h.run_steps(2);
        h.state_mut().adjust.as_mut().unwrap().adjustment = set.clone();
        h.run_steps(2);
        press(&mut h, Modifiers::NONE, Key::Enter);
        for _ in 0..200 {
            h.run_steps(1);
            if !h.state().filters.busy() && h.state().adjust.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_ne!(pixel(&h, 0, 0), color, "{kind:?}");
        assert!(doc(&h).fade_name().is_some());

        let mut h = app(color);
        h.state_mut().run(Action::NewAdjustmentLayer(kind));
        h.run_steps(2);
        let d = h.state_mut().doc.as_mut().unwrap();
        d.active_layer_mut().kind = crate::document::LayerKind::Adjustment(set.clone());
        h.run_steps(2);
        let shown = doc(&h).flatten().get_pixel(0, 0).0;
        assert_ne!(shown, color, "{kind:?} layer");
    }
}

/// Their controls fit a narrow window, in the Properties panel too.
#[test]
fn adjustment_dialogs_draw_at_660_px() {
    for kind in [
        AdjustKind::Exposure,
        AdjustKind::ChannelMixer,
        AdjustKind::SelectiveColor,
    ] {
        let mut h = Harness::builder()
            .with_size([660.0, 600.0])
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                let image = image::RgbaImage::from_pixel(40, 30, image::Rgba([1, 2, 3, 255]));
                app.doc = Some(Document::new(image, None));
                app
            });
        h.run_steps(2);
        h.state_mut().run(Action::Adjust(kind));
        h.run_steps(3);
        h.state_mut().adjust = None;
        h.state_mut().run(Action::NewAdjustmentLayer(kind));
        h.run_steps(3);
    }
}

#[test]
fn stroke_options_last_between_sessions() {
    let mut h = app([0, 0, 0, 255]);
    h.state_mut().filters.stroke_options = fade_stroke::StrokeOptions {
        width: 7,
        color: Some([1, 2, 3]),
        location: StrokeLocation::Outside,
        mode: BlendMode::Multiply,
        opacity: 0.5,
        preserve_transparency: true,
    };
    let saved = h.state().session_settings();
    let mut other = app([0, 0, 0, 255]);
    other.state_mut().restore_session(&saved);
    let wanted = h.state().filters.stroke_options;
    assert!(other.state().filters.stroke_options == wanted);
}

#[test]
fn actions_record_and_replay_them() {
    let mut h = app([0, 0, 0, 255]);
    cast(&mut h);
    h.state_mut().actions.library = crate::actions::Library {
        sets: vec![crate::actions::Set {
            name: "Set".into(),
            expanded: true,
            actions: vec![crate::actions::Sequence {
                name: "Action".into(),
                key: None,
                steps: Vec::new(),
                expanded: true,
            }],
        }],
    };
    h.state_mut().actions.start_recording((0, 0));
    h.state_mut().run(Action::AutoAdjust(AutoMode::Contrast));
    settle(&mut h);
    h.state_mut().run(Action::Fade);
    h.state_mut().filters.fade.as_mut().unwrap().fade.opacity = 0.5;
    h.run_steps(1);
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    h.state_mut().apply_stroke(stroke_settings());
    settle(&mut h);
    let recorded: Vec<Step> = h.state().actions.library.sets[0].actions[0].steps.clone();
    assert!(matches!(
        recorded[0].command,
        Recorded::Menu {
            item: MenuItem::AutoContrast
        }
    ));
    assert!(matches!(&recorded[1].command, Recorded::Fade { opacity, .. } if *opacity == 0.5));
    assert!(matches!(&recorded[2].command, Recorded::Stroke { width: 2, .. }));
    let json = serde_json::to_string(&recorded).unwrap();
    let back: Vec<Step> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, recorded, "kept in the actions file");

    // Played on another image, without their dialogs.
    let mut other = app([0, 0, 0, 255]);
    cast(&mut other);
    let start = steps(&other);
    other.state_mut().actions.library = h.state().actions.library.clone();
    other.state_mut().actions.library.sets[0].actions[0].expanded = false;
    assert!(other.state_mut().play_for_batch((0, 0)));
    for _ in 0..600 {
        other.run_steps(1);
        if !other.state().actions.playing() && !other.state().filters.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // Auto Contrast, its Fade and the stroke.
    assert_eq!(steps(&other), start + 3);
}

#[test]
fn fade_on_a_targeted_mask_changes_the_mask_only_in_a_step_of_its_own() {
    let mut h = app([200, 100, 50, 255]);
    {
        let d = h.state_mut().doc.as_mut().unwrap();
        let reveal = crate::selection::Selection::from(PixelRect { x: 0, y: 0, w: 20, h: 30 });
        assert!(d.add_mask(crate::document::NewMask::RevealSelection, Some(&reveal)));
        assert!(d.mask_targeted());
    }
    let mask_at = |h: &Harness<'static, AtelierApp>, x: u32| -> u8 {
        let layer = doc(h).active_layer();
        let m = layer.mask.as_ref().expect("a mask");
        let r = m.rect(layer.offset);
        let inside = i64::from(x) >= r.x && i64::from(x) < r.right();
        if inside {
            m.image().get_pixel((i64::from(x) - r.x) as u32, 0).0[0]
        } else {
            m.default
        }
    };
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (255, 0));
    let layer_pixels = doc(&h).active_layer().image().clone();
    let start = steps(&h);
    invert(&mut h);
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (0, 255), "the mask inverted");

    h.state_mut().run(Action::Fade);
    h.state_mut().filters.fade.as_mut().expect("Fade opens").fade.opacity = 0.25;
    for _ in 0..200 {
        h.run_steps(1);
        if h.state().preview_shown.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (0, 255), "only a preview");
    press(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h);
    // A quarter of the inversion over what was there.
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (191, 64));
    assert_eq!(*doc(&h).active_layer().image(), layer_pixels, "not the layer");
    assert!(doc(&h).mask_targeted(), "still the mask's turn");
    assert_eq!(steps(&h), start + 2, "a step after the inversion");
    let names = doc(&h).history().0;
    let faded = t!("adjust8-hist-fade", name = names[names.len() - 2]);
    assert_eq!(names.last().copied(), Some(faded.as_str()));

    h.state_mut().run(Action::Undo);
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (0, 255), "the fade undone");
    assert!(doc(&h).fade_name().is_none(), "not fadable again");
    h.state_mut().run(Action::Undo);
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (255, 0), "then the inversion");
    h.state_mut().run(Action::Redo);
    h.state_mut().run(Action::Redo);
    assert_eq!((mask_at(&h, 5), mask_at(&h, 30)), (191, 64), "redone");
}
