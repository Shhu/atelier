//! Camera Raw's window: Filter › Camera Raw Filter… and opening RAW files.

use egui_kittest::Harness;

use super::*;
use crate::camera_raw::WhiteBalance;

/// A 96 × 64 document: a warm gray on the left, white on the right.
fn app() -> Harness<'static, AtelierApp> {
    let mut harness = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = image::RgbaImage::from_fn(96, 64, |x, _| {
                if x < 48 {
                    image::Rgba([150, 120, 90, 255])
                } else {
                    image::Rgba([255, 255, 255, 255])
                }
            });
            app.doc = Some(Document::new(image, None));
            app
        });
    harness.run_steps(2);
    harness
}

fn pixels(h: &Harness<'static, AtelierApp>) -> image::RgbaImage {
    h.state()
        .doc
        .as_ref()
        .unwrap()
        .active_layer()
        .image()
        .clone()
}

/// Runs frames until `done`.
fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in 0..600 {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("background work not done");
}

fn window<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a RawWindow {
    h.state().camera_raw.window.as_deref().expect("window open")
}

fn previewed(app: &AtelierApp) -> bool {
    app.camera_raw
        .window
        .as_ref()
        .is_some_and(|w| w.preview.panes[0].shown.is_some() && w.preview.histogram.is_some())
}

#[test]
fn the_filter_menu_and_shortcut_open_the_window() {
    let mut h = app();
    let menus = h.state().menus();
    let (_, entries) = menus
        .iter()
        .find(|(t, _)| *t == t!("filter-menu"))
        .expect("Filter menu");
    assert!(entries.iter().any(|e| matches!(
        e,
        MenuEntry::Item(_, true, Some(s), Action::CameraRawFilter) if **s == camera_raw_ui::SHORTCUT_CAMERA_RAW
    )));
    h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::A);
    h.run_steps(2);
    assert!(h.state().modal_open(), "the window keeps the keyboard");
    settle(&mut h, previewed);
    // \ shows the image before, U and O the clipping warnings.
    h.key_press(Key::Backslash);
    h.key_press(Key::O);
    h.run_steps(2);
    let view = &window(&h).view;
    assert!(view.before && view.clip_highlights && !view.clip_shadows);
    // The white side clips: its warning is lit.
    let histogram = window(&h).preview.histogram.as_ref().unwrap();
    assert!(histogram[0][255] > 0);
}

#[test]
fn a_setting_then_ok_develops_the_layer_in_one_undo_step() {
    let mut h = app();
    let original = pixels(&h);
    let steps = h.state().doc.as_ref().unwrap().history_step();
    h.state_mut().run(Action::CameraRawFilter);
    settle(&mut h, previewed);
    h.state_mut()
        .camera_raw
        .window
        .as_mut()
        .unwrap()
        .settings
        .exposure = -1.0;
    // The preview follows.
    settle(&mut h, |app| {
        let w = app.camera_raw.window.as_ref().unwrap();
        w.preview.panes[0]
            .shown
            .as_ref()
            .is_some_and(|s| s.image.get_pixel(0, 0).0[0] < 120)
    });
    assert_eq!(pixels(&h), original, "only previewed");
    h.key_press(Key::Enter);
    settle(&mut h, |app| !app.camera_raw.busy());
    let developed = pixels(&h);
    assert!(developed.get_pixel(80, 10).0[0] < 200, "darker");
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(doc.history_step(), steps + 1, "one undo step");
    h.state_mut().undo();
    assert_eq!(pixels(&h), original);
}

#[test]
fn cancel_changes_nothing() {
    let mut h = app();
    let original = pixels(&h);
    let steps = h.state().doc.as_ref().unwrap().history_step();
    h.state_mut().run(Action::CameraRawFilter);
    settle(&mut h, previewed);
    h.state_mut()
        .camera_raw
        .window
        .as_mut()
        .unwrap()
        .settings
        .contrast = 80.0;
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert!(!h.state().camera_raw.busy());
    assert_eq!(pixels(&h), original);
    assert_eq!(h.state().doc.as_ref().unwrap().history_step(), steps);
}

#[test]
fn the_white_balance_tool_neutralizes_what_it_clicks() {
    let mut h = app();
    h.state_mut().run(Action::CameraRawFilter);
    settle(&mut h, previewed);
    h.key_press(Key::I);
    h.run_steps(1);
    assert!(window(&h).view.sampling);
    // The left of the preview: the warm gray.
    let area = h
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("camera_raw")))
        .unwrap();
    let at = pos2(area.left() + 120.0, area.center().y);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    let s = &window(&h).settings;
    assert_eq!(s.white_balance, WhiteBalance::Custom);
    assert!(s.temperature < -5.0, "cooler: {}", s.temperature);
    settle(&mut h, |app| {
        let w = app.camera_raw.window.as_ref().unwrap();
        w.preview.panes[0].shown.as_ref().is_some_and(|s| {
            let p = s.image.get_pixel(2, s.image.height() / 2).0;
            p[0].abs_diff(p[2]) <= 3
        })
    });
}

#[test]
fn a_raw_file_opens_through_the_window_as_a_new_document() {
    let mut h = app();
    // A tiny DNG: a mosaic of a gray card.
    let (w, ht) = (32u32, 24u32);
    let pattern = [0u8, 1, 1, 2];
    let as_shot = [0.5, 1.0, 0.8];
    let samples = (0..w * ht)
        .map(|i| {
            let c = pattern[(((i / w) % 2) * 2 + (i % w) % 2) as usize] as usize;
            (as_shot[c] * 0.2 * 4000.0) as u16
        })
        .collect();
    let dng = crate::camera_raw::raw::TestDng {
        width: w,
        height: ht,
        samples,
        linear: false,
        lossless_jpeg: false,
        black: 0,
        white: 4000,
        pattern,
        color_matrix: crate::camera_raw::raw::SRGB_CAMERA,
        as_shot,
        orientation: 1,
    };
    let dir = std::env::temp_dir().join("atelier-raw-open");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("card.DNG");
    std::fs::write(&path, dng.bytes()).unwrap();
    let tabs = h.state().parked.len();
    h.state_mut().open_path(&path);
    settle(&mut h, previewed);
    assert!(window(&h).source.is_raw());
    assert!(
        window(&h).settings.temperature > 2000.0,
        "kelvins for a RAW file"
    );
    h.key_press(Key::Enter);
    settle(&mut h, |app| !app.camera_raw.busy());
    assert_eq!(h.state().parked.len(), tabs + 1, "a new tab");
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(doc.path.as_deref(), Some(path.as_path()));
    let p = doc.active_layer().image().get_pixel(10, 10).0;
    assert!(
        p[0].abs_diff(p[1]) <= 3 && p[1].abs_diff(p[2]) <= 3,
        "neutral: {p:?}"
    );
    // Placed in a document instead, it develops with the defaults.
    assert!(Document::open(&path).is_ok());
    let _ = std::fs::remove_dir_all(dir);
}

fn exposure_of(app: &AtelierApp) -> Option<f32> {
    let doc = app.doc.as_ref()?;
    let smart = doc.active_layer().smart.as_deref()?;
    match &smart.filters.first()?.filter {
        crate::document::SmartFilterKind::CameraRaw(s) => Some(s.exposure),
        _ => None,
    }
}

#[test]
fn on_a_smart_object_it_is_a_smart_filter_edited_again_from_the_layer() {
    let mut h = app();
    h.state_mut()
        .run(Action::Smart(smart_ui::Command::ConvertForFilters));
    let original = pixels(&h);
    h.state_mut().run(Action::CameraRawFilter);
    settle(&mut h, previewed);
    window_mut(&mut h).settings.exposure = -1.0;
    h.key_press(Key::Enter);
    settle(&mut h, |app| !app.camera_raw.busy() && !app.smart.busy());
    assert_eq!(exposure_of(h.state()), Some(-1.0));
    assert!(h.state().doc.as_ref().unwrap().active_layer().is_smart());
    let darker = pixels(&h);
    assert!(darker.get_pixel(80, 10).0[0] < original.get_pixel(80, 10).0[0]);

    // Double-click on it: the window, with its settings.
    h.state_mut()
        .run(Action::Smart(smart_ui::Command::EditFilter(0)));
    settle(&mut h, previewed);
    assert_eq!(window(&h).settings.exposure, -1.0);
    window_mut(&mut h).settings.exposure = 0.5;
    h.key_press(Key::Enter);
    settle(&mut h, |app| !app.camera_raw.busy() && !app.smart.busy());
    let smart = h.state().doc.as_ref().unwrap().active_layer().smart.clone();
    assert_eq!(smart.unwrap().filters.len(), 1, "replaced, not added");
    assert_eq!(exposure_of(h.state()), Some(0.5));

    // Saved and opened again: the same smart filter.
    let dir = std::env::temp_dir().join("atelier-raw-smart");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("smart.atelier");
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.save(&path).unwrap();
    let back = Document::open(&path).unwrap().doc;
    let filters = |d: &Document| d.active_layer().smart.as_ref().unwrap().filters.clone();
    assert_eq!(filters(&back), filters(h.state().doc.as_ref().unwrap()));
    assert_eq!(back.flatten(), h.state().doc.as_ref().unwrap().flatten());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn last_filter_repeats_camera_raw() {
    let mut h = app();
    h.state_mut().run(Action::CameraRawFilter);
    settle(&mut h, previewed);
    window_mut(&mut h).settings.exposure = -1.0;
    h.key_press(Key::Enter);
    settle(&mut h, |app| !app.camera_raw.busy());
    let once = pixels(&h);
    let steps = h.state().doc.as_ref().unwrap().history_step();
    h.key_press_modifiers(Modifiers::COMMAND.plus(Modifiers::ALT), Key::F);
    h.run_steps(1);
    settle(&mut h, |app| !app.camera_raw.busy());
    assert!(window_opt(&h).is_none(), "without its window");
    assert_eq!(h.state().doc.as_ref().unwrap().history_step(), steps + 1);
    assert!(pixels(&h).get_pixel(80, 10).0[0] < once.get_pixel(80, 10).0[0]);
    // Liquify applied since forgets it.
    h.state_mut().filters.forget_last();
    h.state_mut().run(Action::LastFilter);
    h.run_steps(2);
    assert!(!h.state().camera_raw.busy());
}

fn window_opt<'a>(h: &'a Harness<'static, AtelierApp>) -> Option<&'a RawWindow> {
    h.state().camera_raw.window.as_deref()
}

fn window_mut<'a>(h: &'a mut Harness<'static, AtelierApp>) -> &'a mut RawWindow {
    h.state_mut()
        .camera_raw
        .window
        .as_deref_mut()
        .expect("window open")
}
