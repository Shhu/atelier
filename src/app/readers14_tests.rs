//! What a received file says is never believed: damaged images dropped on
//! the window, and actions files with numbers no dialog allows.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

use super::*;
use crate::actions::Recorded;

type H = Harness<'static, AtelierApp>;

fn app(doc: Option<Document>) -> H {
    let mut doc = doc;
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.recent = crate::recent::Recent::with_paths(Vec::new());
            app.doc = doc.take();
            app
        });
    h.run_steps(3);
    h
}

fn image() -> Document {
    Document::new(RgbaImage::from_pixel(64, 48, Rgba([9, 99, 199, 255])), None)
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

#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// A damaged JPEG on which the decoder panics (fuzzing), dropped on the
/// window without a document, then on a document: said damaged, and the
/// document is still there, as it was.
#[test]
fn a_damaged_image_dropped_takes_nothing_down() {
    let dir = crate::testing::test_dir("readers14-dropped");
    let path = dir.join("broken.jpg");
    let bytes = include_bytes!("../../tests/fixtures/damaged/zune-avx2-246.jpg");
    std::fs::write(&path, bytes).unwrap();
    let damaged = t!("file-error-damaged");
    let mut h = app(None);
    let drop = |h: &mut H| {
        h.hover_at(egui::pos2(600.0, 400.0));
        h.run_steps(1);
        h.input_mut()
            .dropped_files
            .push(Arc::new(Dropped(path.clone())));
        h.run_steps(3);
    };
    drop(&mut h);
    assert_eq!(h.state().tab_count(), 0);
    assert!(h.state().status.to_string().contains(&damaged), "{}", h.state().status);
    let mut h = app(Some(image()));
    drop(&mut h);
    assert_eq!(h.state().doc.as_ref().unwrap().layers.len(), 1);
    assert!(h.state().status.to_string().contains(&damaged), "{}", h.state().status);
    let _ = std::fs::remove_dir_all(dir);
}

/// An actions file's numbers past what the dialogs allow: a feather of
/// 200000 px (which asked for 360 GB) plays as the options bar's largest,
/// a relative canvas size of the largest number as the largest canvas;
/// numbers that aren't, or lie far outside any canvas, stop the action as
/// damaged.
#[test]
fn actions_files_play_within_the_dialogs_limits() {
    let read = |json: &str| -> Recorded { serde_json::from_str(json).expect("a step") };
    let mut h = app(Some(image()));
    let select = read(
        r#"{"command":"select","shape":"ellipse","rect":[4,4,40,30],"mode":"new",
            "feather":200000,"anti_alias":true}"#,
    );
    h.state_mut().play_step(&select, false).expect("played");
    settle(&mut h);
    let wide = read(
        r#"{"command":"canvas-size","width":9223372036854775807,"height":0,
            "relative":true,"anchor":[1,1],"extension":null}"#,
    );
    h.state_mut().play_step(&wide, false).expect("played");
    settle(&mut h);
    let size = h.state().doc.as_ref().map(|d| (d.width(), d.height()));
    assert_eq!(size, Some((crate::document::MAX_DIMENSION, 48)));
    let damaged = t!("file-error-damaged");
    let crop = read(r#"{"command":"crop","rect":[9223372036854775000,0,10,10]}"#);
    assert_eq!(h.state_mut().play_step(&crop, false), Err(damaged.clone()));
    let far = read(
        r#"{"command":"select","shape":"rectangle","rect":[1e300,0,10,10],"mode":"new",
            "feather":0,"anti_alias":false}"#,
    );
    assert_eq!(h.state_mut().play_step(&far, false), Err(damaged.clone()));
    let mut transform = Recorded::Transform {
        corners: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        frame: None,
    };
    if let Recorded::Transform { corners, .. } = &mut transform {
        corners[1][0] = f64::NAN;
    }
    assert_eq!(h.state_mut().play_step(&transform, false), Err(damaged));
}
