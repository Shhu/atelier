//! What keeps the user's work: crash recovery files, a close held while a
//! workspace is open or work is under way, a PSD file's losses asked before
//! it is written.

use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use image::{Rgba, RgbaImage};

type H = Harness<'static, AtelierApp>;

fn app(doc: Option<Document>, recovery: Option<&Path>) -> H {
    let root = recovery.map(Path::to_path_buf);
    let mut doc = doc;
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.recent = crate::recent::Recent::with_paths(Vec::new());
            if let Some(root) = &root {
                app.recover_into(root);
            }
            if let Some(doc) = doc.take() {
                app.add_tab(doc);
            }
            app
        });
    h.run_steps(2);
    h
}

fn picture() -> Document {
    let image = RgbaImage::from_fn(40, 30, |x, y| Rgba([x as u8 * 6, y as u8 * 8, 90, 255]));
    let mut doc = Document::new(image, None);
    doc.mark_saved();
    doc
}

fn commands(h: &H) -> Vec<egui::ViewportCommand> {
    h.output()
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|v| v.commands.clone())
        .unwrap_or_default()
}

/// The window's ×: whether the close was held back.
fn close_held(h: &mut H) -> bool {
    h.input_mut()
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    h.step();
    commands(h).contains(&egui::ViewportCommand::CancelClose)
}

/// Liquify open, its strokes nowhere else: the close is refused, and says
/// why; Liquify stays open.
#[test]
fn closing_with_liquify_open_is_refused() {
    let mut h = app(Some(picture()), None);
    h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::X);
    h.run_steps(2);
    assert!(h.state().liquify.open());
    assert!(close_held(&mut h));
    assert_eq!(
        h.state().workspace.error_status.as_deref(),
        Some(t!("safety14-close-liquify-open").as_str())
    );
    h.run_steps(2);
    assert!(h.state().liquify.open());
    assert!(!commands(&h).contains(&egui::ViewportCommand::Close));
}

/// Artboards to Files writing (the document isn't modified): the close
/// waits for the files, then goes on.
#[test]
fn closing_waits_for_artboards_to_files() {
    let mut h = app(Some(picture()), None);
    let (open, gate) = std::sync::mpsc::channel::<()>();
    h.state_mut().run_artboards_job(1, move || {
        let _ = gate.recv();
        Ok(Vec::new())
    });
    assert!(close_held(&mut h));
    assert_eq!(&*h.state().status, t!("safety14-status-close-waits"));
    h.run_steps(3);
    assert!(!commands(&h).contains(&egui::ViewportCommand::Close));
    open.send(()).unwrap();
    let mut closed = false;
    for _ in crate::testing::patience() {
        h.step();
        if commands(&h).contains(&egui::ViewportCommand::Close) {
            closed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(closed, "the close goes on once the files are written");
}

/// A crash: the modified documents written aside come back at the next
/// start, as they were and still modified; saved, their copy goes; the
/// window closing as asked leaves nothing behind.
#[test]
fn modified_documents_come_back_after_a_crash() {
    let root = crate::testing::test_dir("safety14-recovery");
    let mut h = app(Some(picture()), Some(&root));
    let file = root.join("photo.atelier");
    {
        let app = h.state_mut();
        let doc = app.doc.as_mut().unwrap();
        doc.path = Some(file.clone());
        doc.active_layer_mut().pixels_mut().put_pixel(3, 3, Rgba([1, 2, 3, 255]));
        doc.mark_modified();
        app.write_recovery_now();
    }
    let before = h.state().doc.as_ref().unwrap().flatten();
    // The process ends without closing anything: its folder stays.
    h.state_mut().crash_recovery();
    drop(h);

    let mut h = app(None, Some(&root));
    assert!(h.state().recovery.asking());
    h.get_by_label(&t!("safety14-recovery-open")).click();
    h.run_steps(2);
    let app = h.state();
    let doc = app.doc.as_ref().unwrap();
    assert!(doc.modified);
    assert_eq!(doc.path.as_deref(), Some(file.as_path()));
    assert_eq!(doc.flatten(), before);
    // Kept in the new window's folder until saved.
    let folders = || std::fs::read_dir(&root).unwrap().count();
    assert_eq!(folders(), 1);
    h.state_mut().doc.as_mut().unwrap().mark_saved();
    h.run_steps(2);
    let own = std::fs::read_dir(&root).unwrap().flatten().next().unwrap().path();
    let documents = std::fs::read_dir(&own)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "atelier"))
        .count();
    assert_eq!(documents, 0, "saved: its copy goes");
    drop(h);
    assert_eq!(folders(), 0, "closed as asked: nothing left");
}

/// Save As a PSD file that can't hold everything: asked before anything is
/// written; saved anyway, the document stays modified and the close that
/// asked goes on.
#[test]
fn a_lossy_psd_is_asked_before_writing() {
    let dir = crate::testing::test_dir("safety14-psd");
    let path = dir.join("photo.psd");
    let mut doc = picture();
    doc.path = Some(path.clone());
    doc.file_notes = vec!["what opening left out".into()];
    doc.mark_modified();
    let mut h = app(Some(doc), None);
    h.state_mut().save_to(&path);
    h.run_steps(2);
    assert!(h.state().psd_ask.is_some());
    assert!(!path.exists(), "nothing written before the answer");
    // Cancelled: nothing written.
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().psd_ask.is_none() && !path.exists());
    h.state_mut().save_to(&path);
    h.state_mut().psd_ask.as_mut().unwrap().then = Some(Confirm::CloseDocument);
    h.run_steps(2);
    h.get_by_label(&t!("safety14-psd-save-anyway")).click();
    h.run_steps(2);
    assert!(path.exists());
    assert!(h.state().doc.is_none(), "the close went on");
}
