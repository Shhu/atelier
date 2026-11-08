//! Batch: a step's dialog cancelled or a Stop answered stops it (nothing
//! saved), Export steps write to its destination, Save and Close writes
//! BMP, TGA and QOI back.

use super::*;
use crate::actions::{Sequence, Set, Step};
use crate::pixel_ops::Adjustment;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use image::{Rgba, RgbaImage};

type H = Harness<'static, AtelierApp>;

fn app(steps: Vec<Step>) -> H {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.recent = crate::recent::Recent::with_paths(Vec::new());
            app.actions.library.sets = vec![Set {
                name: "S".into(),
                actions: vec![Sequence {
                    name: "A".into(),
                    key: None,
                    steps: steps.clone(),
                    expanded: false,
                }],
                expanded: true,
            }];
            app
        });
    h.run_steps(2);
    h
}

fn png(path: &Path, v: u8) {
    RgbaImage::from_pixel(40, 30, Rgba([v, v, v, 255]))
        .save(path)
        .unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn to_folder(source: &Path, dest: &Path) -> BatchDialog {
    BatchDialog {
        folder: Some(source.to_path_buf()),
        destination: Destination::Folder,
        dest_folder: Some(dest.to_path_buf()),
        format: ExportFormat::Png,
        ..BatchDialog::default()
    }
}

fn invert() -> Step {
    Step::new(Recorded::Adjust {
        adjustment: Adjustment::Invert,
    })
}

/// Frames until the batch ends, `each` run on every one.
fn run(h: &mut H, mut each: impl FnMut(&mut H)) {
    for _ in crate::testing::patience() {
        h.run_steps(1);
        each(h);
        if !h.state().automate.running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("batch not done");
}

fn three_images(name: &str) -> (PathBuf, PathBuf) {
    let (src, dest) = (
        crate::testing::test_dir(&format!("batch14-{name}-src")),
        crate::testing::test_dir(&format!("batch14-{name}-dest")),
    );
    for n in ["a.png", "b.png", "c.png"] {
        png(&src.join(n), 60);
    }
    (src, dest)
}

/// The Cancel of a step's own window (Image Size here) stops the batch:
/// no file saved unprocessed, the next one not opened.
#[test]
fn a_step_dialog_cancelled_stops_the_batch() {
    let (src, dest) = three_images("dialog");
    let mut h = app(vec![
        Step {
            dialog: true,
            ..Step::new(Recorded::ImageSize {
                width: 20,
                height: 15,
                resample: crate::actions::Resample::Bicubic,
            })
        },
        invert(),
    ]);
    h.state_mut().start_batch(&to_folder(&src, &dest));
    let mut cancels = 0;
    run(&mut h, |h| {
        if h.state().image_size.is_some() {
            cancels += 1;
            // What the window's Cancel button does (the progress window
            // has a Cancel of its own, which would stop it anyway).
            h.state_mut().image_size = None;
        }
    });
    assert_eq!(cancels, 1);
    assert!(names(&dest).is_empty(), "nothing saved: {:?}", names(&dest));
    assert_eq!(h.state().status.to_string(), t!("actions-batch-cancelled", count = 0));
    assert!(h.state().automate.alert.is_none());
}

/// "Stop" on a Stop step stops the batch, the document left as it is.
#[test]
fn stop_on_a_stop_step_stops_the_batch() {
    let (src, dest) = three_images("stop");
    let mut h = app(vec![
        Step::new(Recorded::Stop {
            message: "Check".into(),
            allow_continue: true,
        }),
        invert(),
    ]);
    h.state_mut().start_batch(&to_folder(&src, &dest));
    let mut stops = 0;
    let stop = t!("actions-stop");
    run(&mut h, |h| {
        if h.state().actions_modal() {
            stops += 1;
            h.get_by_label(&stop).click();
        }
    });
    // Asked for the first document only (a click takes a frame or two).
    assert!((1..3).contains(&stops), "{stops}");
    assert!(names(&dest).is_empty(), "nothing saved: {:?}", names(&dest));
    assert!(!h.state().actions.playing());
    assert_eq!(h.state().status.to_string(), t!("actions-batch-cancelled", count = 0));
}

/// An Export step without a folder of its own writes into the batch's
/// destination, never next to the images processed.
#[test]
fn export_steps_write_to_the_batch_destination() {
    let src = crate::testing::test_dir("batch14-export-src");
    let dest = crate::testing::test_dir("batch14-export-dest");
    png(&src.join("a.png"), 60);
    let mut h = app(vec![
        invert(),
        Step::new(Recorded::Export {
            options: crate::export::ExportOptions::web(),
            folder: None,
        }),
    ]);
    h.state_mut().start_batch(&to_folder(&src, &dest));
    run(&mut h, |_| {});
    assert_eq!(names(&src), ["a.png"]);
    let written = names(&dest);
    assert!(written.iter().any(|n| n.ends_with(".jpg")), "{written:?}");
    assert!(written.iter().any(|n| n.ends_with(".png")), "{written:?}");
}

/// Save and Close writes BMP, TGA and QOI files back in their format.
#[test]
fn save_and_close_writes_bmp_tga_and_qoi_back() {
    let src = crate::testing::test_dir("batch14-plain");
    let image = RgbaImage::from_pixel(40, 30, Rgba([60, 60, 60, 255]));
    for ext in ["bmp", "tga", "qoi"] {
        image.save(src.join(format!("f.{ext}"))).unwrap();
    }
    let mut h = app(vec![invert()]);
    let d = BatchDialog {
        folder: Some(src.clone()),
        destination: Destination::SaveAndClose,
        ..BatchDialog::default()
    };
    h.state_mut().start_batch(&d);
    run(&mut h, |_| {});
    assert!(h.state().automate.alert.is_none(), "{:?}", h.state().automate.alert);
    for ext in ["bmp", "tga", "qoi"] {
        let path = src.join(format!("f.{ext}"));
        let format = image::ImageFormat::from_path(&path).unwrap();
        let read = image::ImageReader::open(&path)
            .unwrap()
            .with_guessed_format()
            .unwrap();
        assert_eq!(read.format(), Some(format), "{ext} kept its format");
        let pixel = read.decode().unwrap().to_rgba8().get_pixel(0, 0).0;
        assert_eq!(pixel, [195, 195, 195, 255], "{ext} inverted");
    }
    assert_eq!(h.state().tab_count(), 0);
}
