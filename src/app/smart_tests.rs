//! Smart objects driven through the app: Convert to Smart Object, Place
//! Embedded (menu, drag and drop), Edit Contents saved back from its tab,
//! smart filters applied, re-edited and rearranged.

use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

use super::smart_ui::Command;
use super::*;
use crate::filters::Filter;

/// White 200 × 100 background, a 40 × 30 pattern layer at (20, 10) over it
/// (active).
fn app() -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let mut doc = Document::new(RgbaImage::from_pixel(200, 100, Rgba([255; 4])), None);
            doc.add_layer("pattern".into(), pattern(), (20, 10));
            app.doc = Some(doc);
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn pattern() -> RgbaImage {
    RgbaImage::from_fn(40, 30, |x, y| {
        Rgba([(x * 6) as u8, (y * 8) as u8, 120, 255])
    })
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn smart<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a crate::document::SmartObject {
    doc(h)
        .active_layer()
        .smart
        .as_deref()
        .expect("a smart object")
}

/// Runs frames until `done`.
fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in 0..500 {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("not done");
}

fn folder(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("atelier-smart-app-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn convert_to_smart_object_from_the_menu() {
    let mut h = app();
    let before = doc(&h).flatten();
    let steps = doc(&h).history_step();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.run_steps(2);
    assert!(doc(&h).active_layer().is_smart());
    assert_eq!(doc(&h).flatten(), before);
    assert_eq!(doc(&h).history_step(), steps + 1);
    // Its menu items now apply; painting doesn't.
    let menu = h.state().smart_menu();
    let enabled = |cmd: Command| {
        menu.iter()
            .any(|e| matches!(e, MenuEntry::Item(_, true, _, Action::Smart(c)) if *c == cmd))
    };
    assert!(enabled(Command::EditContents) && enabled(Command::NewViaCopy));
    assert!(!enabled(Command::UpdateModified), "embedded, not linked");
    assert!(!doc(&h).can_paint());
    // Image › Adjustments: refused, like Photoshop.
    h.state_mut().run(Action::Adjust(AdjustKind::Levels));
    assert!(h.state().adjust.is_none());
    assert_eq!(h.state().status, t!("smart-status-no-adjustments"));
}

#[test]
fn place_embedded_and_drop_place_smart_objects() {
    let dir = folder("place");
    let path = dir.join("square.png");
    RgbaImage::from_pixel(50, 40, Rgba([0, 0, 255, 255]))
        .save(&path)
        .unwrap();
    let mut h = app();
    h.state_mut().place_path(&path, false);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 3, "{}", h.state().status);
    assert_eq!(doc(&h).active_layer().name, "square");
    assert!(smart(&h).link.is_none());
    // Placed through Free Transform, like Photoshop: Enter places it.
    assert!(h.state().transform_pending());
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(!h.state().transform_pending());
    assert!(doc(&h).active_layer().is_smart());
    assert_eq!(doc(&h).flatten().get_pixel(100, 50).0, [0, 0, 255, 255]);
    // A file dropped on the document: placed too.
    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);
    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }
    h.input_mut()
        .dropped_files
        .push(Arc::new(Dropped(path.clone())));
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), 4);
    assert!(doc(&h).active_layer().is_smart());
}

#[test]
fn placing_cancelled_leaves_nothing_and_applied_is_one_step() {
    let dir = folder("cancel");
    let path = dir.join("logo.png");
    RgbaImage::from_pixel(50, 40, Rgba([0, 0, 255, 255]))
        .save(&path)
        .unwrap();
    let mut h = app();
    let (layers, steps) = (doc(&h).layers.len(), doc(&h).history().0.len());
    // Esc: the placed layer goes, no step left.
    h.state_mut().place_path(&path, false);
    h.run_steps(2);
    assert_eq!(doc(&h).layers.len(), layers + 1);
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().transform_pending());
    assert_eq!(doc(&h).layers.len(), layers);
    assert_eq!(doc(&h).history().0.len(), steps);
    // Enter after scaling it: one step, named after the place.
    h.state_mut().place_path(&path, true);
    h.run_steps(2);
    let frame = h.state().transform_rect.unwrap();
    h.state_mut().transform_rect = Some(Rect::from_min_size(frame.min, frame.size() * 2.0));
    h.key_press(egui::Key::Enter);
    settle(&mut h, |app| !app.transform_busy());
    assert!(!h.state().transform_pending());
    assert_eq!(doc(&h).active_layer().rect().w, 100);
    assert!(doc(&h).active_layer().is_smart());
    let (names, done) = doc(&h).history();
    assert_eq!(done, steps + 1);
    assert_eq!(names[done - 1], t!("smart-hist-place-linked"));
}

#[test]
fn edit_contents_saved_from_its_tab_updates_the_smart_object() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    let parent = doc(&h).uid();
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert_eq!(h.state().tab_count(), 2);
    let child = doc(&h);
    assert_eq!(child.embedded_in.as_ref().map(|e| e.parent), Some(parent));
    assert_eq!(child.display_name(), "pattern");
    assert_eq!((child.width(), child.height()), (40, 30));
    // Filled red in its tab, then saved.
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    assert!(doc(&h).modified);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    assert!(!doc(&h).modified);
    assert_eq!(
        h.state().status,
        t!("smart-status-updated", name = "pattern")
    );
    // Back in its document: shown red, one step to undo.
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    assert_eq!(doc(&h).uid(), parent);
    assert_eq!(doc(&h).flatten().get_pixel(30, 20).0, [255, 0, 0, 255]);
    h.state_mut().run(Action::Undo);
    assert_ne!(doc(&h).flatten().get_pixel(30, 20).0, [255, 0, 0, 255]);
    // Edit Contents again shows the tab already open.
    h.state_mut().run(Action::Smart(Command::EditContents));
    assert_eq!(h.state().tab_count(), 2);
}

#[test]
fn smart_filters_apply_then_reedit_and_rearrange() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::ConvertForFilters));
    let original = doc(&h).active_layer().image().clone();
    // The filter dialog on a smart object: a smart filter, previewed.
    h.state_mut().run(Action::Filter(FilterKind::GaussianBlur));
    settle(&mut h, |app| {
        matches!(
            app.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(_))
        )
    });
    assert_eq!(*doc(&h).active_layer().image(), original, "only previewed");
    h.key_press(egui::Key::Enter);
    settle(&mut h, |app| !app.filters.busy() && !app.smart.busy());
    let blur = FilterKind::GaussianBlur.initial();
    assert_eq!(smart(&h).filters.len(), 1);
    assert_eq!(smart(&h).filters[0].filter, blur.into());
    assert!(doc(&h).active_layer().is_smart(), "still smart");
    let blurred = doc(&h).active_layer().image().clone();
    assert_ne!(blurred, original);

    // Re-edited from the Layers panel: the same filter, other settings.
    let doc_mut = h.state_mut().doc.as_mut().unwrap();
    let small = Filter::GaussianBlur { radius: 1.0 };
    doc_mut.edit_smart(doc_mut.active, "Edit".into(), |s| {
        s.filters[0].filter = small.into()
    });
    h.state_mut().run(Action::Smart(Command::EditFilter(0)));
    h.run_steps(2);
    assert!(h.state().filters.busy(), "its dialog");
    h.key_press(egui::Key::Enter);
    settle(&mut h, |app| !app.filters.busy() && !app.smart.busy());
    assert_eq!(smart(&h).filters.len(), 1, "replaced, not added");
    assert_eq!(smart(&h).filters[0].filter, small.into());

    // Last Filter adds another; hidden, moved, deleted.
    h.state_mut().run(Action::Filter(FilterKind::FindEdges));
    h.run_steps(2);
    assert_eq!(smart(&h).filters.len(), 2);
    assert_eq!(smart(&h).filters[1].filter, Filter::FindEdges.into());
    h.state_mut().run(Action::Smart(Command::ToggleFilter(1)));
    assert!(!smart(&h).filters[1].visible);
    h.state_mut().run(Action::Smart(Command::MoveFilter(1, 0)));
    assert_eq!(smart(&h).filters[0].filter, Filter::FindEdges.into());
    h.state_mut().run(Action::Smart(Command::DeleteFilter(0)));
    assert_eq!(smart(&h).filters.len(), 1);
    let mut expected = original.clone();
    small.apply(&mut expected, doc(&h).active_layer().offset);
    assert_eq!(*doc(&h).active_layer().image(), expected);
    h.state_mut().run(Action::Smart(Command::ToggleFilters));
    assert_eq!(*doc(&h).active_layer().image(), original);
    h.run_steps(2);
}

/// A distortion of a 40 × 30 image: pushed right in the middle.
fn pushed() -> crate::liquify::Mesh {
    use crate::liquify::{Brush, Effect, Liquify, Mesh};
    let mut editor = Liquify::new(Mesh::new(40, 30));
    let brush = Brush {
        size: 20.0,
        density: 0.5,
        pressure: 1.0,
        rate: 0.5,
        pin_edges: false,
    };
    editor.stroke(Effect::Warp, &brush, [10.0, 15.0], [25.0, 15.0], 0.1, 1.0);
    assert!(!editor.mesh.is_identity());
    editor.mesh
}

#[test]
fn liquify_as_a_smart_filter() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    let original = doc(&h).active_layer().image().clone();
    // Last Filter after a Liquify: on a smart object, a smart filter.
    let mesh = Arc::new(pushed());
    h.state_mut().liquify.last = Some(Arc::clone(&mesh));
    h.state_mut().run(Action::LastFilter);
    settle(&mut h, |app| !app.smart.busy() && !app.liquify.busy());
    assert!(doc(&h).active_layer().is_smart());
    let kind = crate::document::SmartFilterKind::Liquify(Arc::clone(&mesh));
    assert_eq!(smart(&h).filters[0].filter, kind);
    assert_ne!(*doc(&h).active_layer().image(), original);
    // Edited again in its workspace; cancelled, it stays.
    h.state_mut().run(Action::Smart(Command::EditFilter(0)));
    h.run_steps(2);
    assert!(h.state().liquify.busy(), "the workspace");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().liquify.busy());
    assert_eq!(smart(&h).filters[0].filter, kind);
}

#[test]
fn smart_filter_blending_options() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.state_mut().run(Action::Filter(FilterKind::FindEdges));
    settle(&mut h, |app| !app.smart.busy());
    h.state_mut().run(Action::Smart(Command::FilterBlending(0)));
    h.run_steps(2);
    assert!(h.state().modal_open());
    // The window's settings, as set by its fields.
    if let Some(b) = &mut h.state_mut().smart.blending_mut() {
        b.opacity = 0.5;
        b.blend = BlendMode::Multiply;
    }
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    // Rendered again in the background.
    settle(&mut h, |app| !app.modal_open());
    let f = &smart(&h).filters[0];
    assert_eq!((f.opacity, f.blend), (0.5, BlendMode::Multiply));
}

#[test]
fn painting_a_smart_object_is_refused() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.state_mut().set_tool(Tool::Brush);
    h.run_steps(2);
    let before = doc(&h).active_layer().image().clone();
    let at = h.state().last_view.unwrap().to_screen(pos2(30.0, 20.0));
    h.input_mut().events.push(egui::Event::PointerMoved(at));
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    assert_eq!(*doc(&h).active_layer().image(), before);
    assert!(doc(&h).active_layer().is_smart());
    assert_eq!(h.state().status, t!("smart-status-not-editable"));
}

/// Capture of a smart object with smart filters in the Layers panel
/// (offscreen render): `cargo test smart_capture -- --ignored`, written to
/// the file given by `ATELIER_CAPTURE`.
#[test]
#[ignore = "needs a graphics renderer"]
fn smart_capture() {
    let out = std::env::var("ATELIER_CAPTURE").unwrap_or_else(|_| "smart.png".into());
    if std::env::var("ATELIER_LANG").is_ok_and(|v| v == "fr") {
        i18n::set_lang(Lang::Fr);
    }
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .wgpu()
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let mut doc = Document::new(RgbaImage::from_pixel(400, 300, Rgba([255; 4])), None);
            let photo = RgbaImage::from_fn(240, 180, |x, y| {
                Rgba([(x + y) as u8, (x * 2) as u8, (255 - y) as u8, 255])
            });
            doc.add_layer("Photo".into(), photo, (80, 60));
            doc.convert_to_smart(&[1]);
            let blur = crate::document::SmartFilter::new(Filter::GaussianBlur { radius: 4.0 });
            let mosaic = crate::document::SmartFilter::new(Filter::Mosaic { cell: 12.0 });
            doc.edit_smart(1, "Filters".into(), |s| s.filters = vec![blur, mosaic]);
            let sel: crate::document::Selection = PixelRect {
                x: 80,
                y: 60,
                w: 120,
                h: 180,
            }
            .into();
            doc.add_filter_mask(1, Some(&sel));
            app.doc = Some(doc);
            app
        });
    h.run_steps(6);
    let image = h.render().expect("render");
    image.save(&out).expect("writing the capture");
}

/// Edit Contents of a linked smart object opens its file; once saved, the
/// smart object shows it when its document is back, its file read on
/// another thread.
#[test]
fn linked_edit_contents_opens_the_file_and_links_follow() {
    let dir = folder("linked-edit");
    let path = dir.join("logo.atelier");
    Document::new(RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255])), None)
        .save(&path)
        .unwrap();
    let mut h = app();
    h.state_mut().place_path(&path, true);
    h.key_press(egui::Key::Enter);
    let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
    settle(&mut h, applied);
    let parent = doc(&h).uid();
    let steps = doc(&h).history().1;
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert_eq!(h.state().tab_count(), 2);
    assert_eq!(doc(&h).path.as_deref(), Some(path.as_path()));
    assert!(doc(&h).embedded_in.is_none());
    // Edited and saved to the file.
    std::thread::sleep(std::time::Duration::from_millis(20));
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    assert!(!doc(&h).modified);
    // Back in the document: the link is read again, as one step.
    h.state_mut().switch_tab(0);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    assert_eq!(doc(&h).uid(), parent);
    let r = doc(&h).active_layer().rect();
    let (x, y) = ((r.x + 10) as u32, (r.y + 10) as u32);
    assert_eq!(doc(&h).flatten().get_pixel(x, y).0, [255, 0, 0, 255]);
    assert_eq!(doc(&h).history().1, steps + 1);
}

/// Edit Contents of a linked PNG opens the PNG; Ctrl+S writes the PNG
/// (no Save As), and the smart object follows once back in its document.
#[test]
fn linked_png_edit_contents_saves_the_png() {
    let dir = folder("linked-png");
    let path = dir.join("logo.png");
    RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
        .save(&path)
        .unwrap();
    let mut h = app();
    h.state_mut().place_path(&path, true);
    h.key_press(egui::Key::Enter);
    let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
    settle(&mut h, applied);
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert_eq!(doc(&h).path.as_deref(), Some(path.as_path()));
    std::thread::sleep(std::time::Duration::from_millis(20));
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    super::files::SAVE_DIALOG.with_borrow_mut(|d| d.asked.clear());
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    let asked = super::files::SAVE_DIALOG.with_borrow_mut(|d| std::mem::take(&mut d.asked));
    assert!(asked.is_empty(), "no Save As: {asked:?}");
    assert!(!doc(&h).modified);
    let on_disk = image::open(&path).unwrap().to_rgba8().get_pixel(5, 5).0;
    assert_eq!(on_disk, [255, 0, 0, 255]);
    h.state_mut().switch_tab(0);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    let r = doc(&h).active_layer().rect();
    let (x, y) = ((r.x + 10) as u32, (r.y + 10) as u32);
    assert_eq!(doc(&h).flatten().get_pixel(x, y).0, [255, 0, 0, 255]);
}

/// A linked smart object whose file is missing: Edit Contents edits the
/// embedded copy, and saving it embeds the smart object, so the file
/// coming back doesn't replace the edits.
#[test]
fn missing_link_edit_contents_kept_when_file_returns() {
    let dir = folder("missing-link");
    let path = dir.join("logo.png");
    let away = dir.join("away.png");
    RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
        .save(&path)
        .unwrap();
    let mut h = app();
    h.state_mut().place_path(&path, true);
    h.key_press(egui::Key::Enter);
    let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
    settle(&mut h, applied);
    std::fs::rename(&path, &away).unwrap();
    // Focus away and back: the link is found missing.
    h.state_mut().smart.focused = false;
    h.run_steps(1);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert!(doc(&h).embedded_in.is_some(), "the embedded copy is edited");
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    assert!(smart(&h).link.is_none(), "embedded");
    let r = doc(&h).active_layer().rect();
    let (x, y) = ((r.x + 10) as u32, (r.y + 10) as u32);
    let edited = doc(&h).flatten().get_pixel(x, y).0;
    assert_eq!(edited, [255, 0, 0, 255]);
    // The file comes back (drive reconnected): focus regained.
    std::fs::rename(&away, &path).unwrap();
    h.state_mut().smart.focused = false;
    h.run_steps(2);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    assert_eq!(
        doc(&h).flatten().get_pixel(x, y).0,
        edited,
        "the edits kept"
    );
}

/// Replace Contents on one instance (a Duplicate Layer copy): every
/// instance shows the file, still sharing their contents, as one step.
#[test]
fn replace_contents_updates_every_instance() {
    let dir = folder("replace-instances");
    let path = dir.join("other.png");
    RgbaImage::from_pixel(40, 30, Rgba([0, 255, 0, 255]))
        .save(&path)
        .unwrap();
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.run_steps(2);
    h.state_mut().run(Action::Duplicate);
    h.run_steps(2);
    let d = h.state_mut().doc.as_mut().unwrap();
    let ids = |d: &Document| -> Vec<u64> {
        d.layers
            .iter()
            .filter_map(|l| l.smart.as_ref().map(|s| s.contents.id))
            .collect()
    };
    let before = ids(d);
    let steps = d.history().1;
    d.replace_contents(&path).unwrap();
    let after = ids(d);
    assert!(after.iter().all(|&id| id == after[0]), "still shared");
    assert_ne!(after[0], before[0], "new contents, a new id");
    assert_eq!(d.history().1, steps + 1);
    for l in d.layers.iter().filter(|l| l.is_smart()) {
        assert_eq!(l.pixel_at(30, 20), Some([0, 255, 0, 255]), "{}", l.name);
    }
}

#[test]
fn a_long_smart_object_rendering_is_cancelled_and_undone() {
    let mut h = app();
    // Large enough to be rendered on another thread, slow to filter.
    let mut d = Document::new(
        RgbaImage::from_fn(2100, 2100, |x, y| {
            Rgba([(x % 251) as u8, (y % 241) as u8, 90, 255])
        }),
        None,
    );
    assert!(d.convert_to_smart(&[0]));
    let mut median = crate::document::SmartFilter::new(Filter::Median { radius: 80.0 });
    median.visible = false;
    let i = d.active;
    d.edit_smart(i, "Add".into(), |s| s.filters.push(median));
    let before = d.active_layer().image().clone();
    h.state_mut().doc = Some(d);
    h.state_mut().run(Action::Smart(Command::ToggleFilter(0)));
    assert!(h.state().smart.busy(), "rendering in the background");
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(!h.state().smart.busy());
    assert!(!smart(&h).filters[0].visible, "the change undone");
    assert_eq!(*doc(&h).active_layer().image(), before);
}

/// A linked file, written by `write`, placed then opened by Edit Contents.
fn linked_tab(
    name: &str,
    file: &str,
    write: impl FnOnce(&std::path::Path),
) -> (Harness<'static, AtelierApp>, std::path::PathBuf) {
    let path = folder(name).join(file);
    write(&path);
    let mut h = app();
    h.state_mut().place_path(&path, true);
    h.key_press(egui::Key::Enter);
    let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
    settle(&mut h, applied);
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert_eq!(doc(&h).path.as_deref(), Some(path.as_path()));
    (h, path)
}

/// Edit Contents of a linked PNG, a layer added: Close › Save asks
/// whether to merge the layers into the file; Save writes it and the
/// tab closes (quitting goes on the same way).
#[test]
fn layered_link_tab_closes_once_saved() {
    let (mut h, path) = linked_tab("layered-close", "logo.png", |p| {
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
            .save(p)
            .unwrap()
    });
    let tabs = h.state().tab_count();
    h.state_mut().run(Action::NewLayer);
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::CloseDocument);
    h.run_steps(2);
    assert!(h.state().confirm.is_some(), "Save changes?");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().smart.link_save.is_some(), "merging asked first");
    assert_eq!(h.state().tab_count(), tabs, "not closed yet");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().tab_count(), tabs - 1, "closed once saved");
    let saved = image::open(&path).unwrap().to_rgba8();
    assert_eq!(saved.get_pixel(5, 5).0, [255, 0, 0, 255], "merged");
}

/// Same, Cancel at the merge question: nothing written, the tab stays.
#[test]
fn layered_link_save_cancelled() {
    let (mut h, path) = linked_tab("layered-cancel", "logo.png", |p| {
        RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
            .save(p)
            .unwrap()
    });
    let tabs = h.state().tab_count();
    h.state_mut().run(Action::NewLayer);
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    assert!(h.state().smart.link_save.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    assert!(h.state().smart.link_save.is_none());
    assert_eq!(h.state().tab_count(), tabs);
    assert!(doc(&h).modified);
    let file = image::open(&path).unwrap().to_rgba8();
    assert_eq!(file.get_pixel(5, 5).0, [0, 0, 255, 255], "left as it is");
}

/// Edit Contents of a linked 16-bit PNG: Ctrl+S says what the file will
/// lose and, agreed to, writes it; the smart object follows.
#[test]
fn linked_16bit_png_saved_back_once_agreed() {
    let (mut h, path) = linked_tab("deep", "deep.png", |p| {
        image::ImageBuffer::<Rgba<u16>, _>::from_pixel(20, 20, Rgba([0, 0, 65535, 65535]))
            .save(p)
            .unwrap()
    });
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    assert!(h.state().smart.link_save.is_some(), "the loss said first");
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(!doc(&h).modified, "saved");
    let file = image::open(&path).unwrap().to_rgba8();
    assert_eq!(file.get_pixel(5, 5).0, [255, 0, 0, 255]);
    // Saved again: nothing more to lose, no question.
    h.state_mut().primary = Color32::from_rgb(0, 255, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    assert!(h.state().smart.link_save.is_none());
    assert!(!doc(&h).modified);
}

/// Embed Linked on one instance embeds every instance of its contents
/// (they share them): one step, none left linked.
#[test]
fn embed_linked_embeds_every_instance() {
    let path = folder("embed-all").join("logo.png");
    RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255]))
        .save(&path)
        .unwrap();
    let mut h = app();
    h.state_mut().place_path(&path, true);
    h.key_press(egui::Key::Enter);
    let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
    settle(&mut h, applied);
    h.state_mut().run(Action::Duplicate);
    h.run_steps(1);
    let steps = doc(&h).history_step();
    h.state_mut().run(Action::Smart(Command::EmbedLinked));
    h.run_steps(1);
    let linked = doc(&h)
        .layers
        .iter()
        .filter_map(|l| l.smart.as_deref())
        .filter(|s| s.link.is_some())
        .count();
    assert_eq!(linked, 0);
    assert_eq!(doc(&h).history_step(), steps + 1, "one step");
}

/// OK in a smart filter's dialog puts in what its preview rendered: no
/// rendering again.
#[test]
fn smart_filter_ok_puts_in_its_preview() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::ConvertForFilters));
    let original = doc(&h).active_layer().image().clone();
    h.state_mut().run(Action::Filter(FilterKind::GaussianBlur));
    settle(&mut h, |app| {
        matches!(
            app.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(_))
        )
    });
    let previewed = h.state().preview_shown.as_ref().unwrap().image.clone();
    h.key_press(egui::Key::Enter);
    h.run_steps(1);
    assert!(!h.state().filters.busy(), "applied");
    assert!(!h.state().smart.busy(), "not rendered again");
    assert_eq!(smart(&h).filters.len(), 1);
    let image = doc(&h).active_layer().image().clone();
    assert_ne!(image, original);
    assert_eq!(image, *previewed);
}

/// A smart object with filters renders on another thread whatever its
/// size (a filter's cost can't be told), cancellable.
#[test]
fn small_smart_objects_with_filters_render_in_the_background() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.state_mut().start_filter(Filter::Median { radius: 3.0 });
    assert!(h.state().smart.busy(), "in the background");
    settle(&mut h, |app| !app.smart.busy());
    assert_eq!(smart(&h).filters.len(), 1);
    // Without filters: at once.
    h.state_mut().run(Action::Smart(Command::ClearFilters));
    assert!(!h.state().smart.busy());
}

/// A linked file missing, Edit Contents edits the embedded copy; the file
/// comes back (focus regained) before the tab is saved: Save asks whether
/// to embed the edits (the file would replace them), the tab still the
/// smart object's contents; Embed puts them in, unlinked.
#[test]
fn link_back_before_contents_saved_asks_to_embed() {
    let dir = folder("link-back");
    let path = dir.join("logo.png");
    let away = dir.join("away.png");
    RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255])).save(&path).unwrap();
    let mut h = app();
    h.state_mut().place_path(&path, true);
    h.key_press(egui::Key::Enter);
    let applied = |app: &AtelierApp| !app.transform_pending() && !app.transform_busy();
    settle(&mut h, applied);
    std::fs::rename(&path, &away).unwrap();
    h.state_mut().smart.focused = false;
    h.run_steps(1);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert!(doc(&h).embedded_in.is_some());
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    std::fs::rename(&away, &path).unwrap();
    h.state_mut().smart.focused = false;
    h.run_steps(1);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    super::files::SAVE_DIALOG.with_borrow_mut(|d| d.asked.clear());
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    let asked = super::files::SAVE_DIALOG.with_borrow_mut(|d| std::mem::take(&mut d.asked));
    assert!(asked.is_empty(), "no Save As for a smart object still there");
    assert!(h.state().smart.link_save.is_some(), "embed or not, asked");
    assert!(doc(&h).embedded_in.is_some() && doc(&h).modified);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(h.state().smart.link_save.is_none());
    assert!(!doc(&h).modified, "put in the smart object");
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    assert!(smart(&h).link.is_none(), "embedded");
    let r = doc(&h).active_layer().rect();
    let (x, y) = ((r.x + 10) as u32, (r.y + 10) as u32);
    assert_eq!(doc(&h).flatten().get_pixel(x, y).0, [255, 0, 0, 255]);
}

/// Edit Contents, then the smart object deleted: Save says so and offers
/// Save As; cancelled, the tab stays its contents, and once the delete is
/// undone its Save updates the smart object again.
#[test]
fn contents_tab_kept_when_its_smart_object_is_gone() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.run_steps(2);
    h.state_mut().run(Action::Smart(Command::EditContents));
    h.run_steps(2);
    assert!(doc(&h).embedded_in.is_some());
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    assert!(h.state_mut().doc.as_mut().unwrap().delete_active());
    h.run_steps(2);
    h.state_mut().switch_tab(1);
    h.run_steps(2);
    super::files::SAVE_DIALOG.with_borrow_mut(|d| d.asked.clear());
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    let asked = super::files::SAVE_DIALOG.with_borrow_mut(|d| std::mem::take(&mut d.asked));
    assert_eq!(asked.len(), 1, "Save As offered");
    assert!(doc(&h).embedded_in.is_some(), "still its contents");
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    h.state_mut().switch_tab(1);
    h.run_steps(2);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    assert!(!doc(&h).modified, "the smart object updated");
}

/// A document saved with a linked smart object, its file then gone:
/// reopened, the missing link is said in a window, once.
#[test]
fn reopened_missing_link_is_said() {
    let dir = folder("reopen-missing");
    let path = dir.join("logo.png");
    RgbaImage::from_pixel(20, 20, Rgba([0, 0, 255, 255])).save(&path).unwrap();
    let mut d = Document::new(RgbaImage::from_pixel(100, 100, Rgba([255; 4])), None);
    d.place_file(&path, true).unwrap();
    let saved = dir.join("doc.atelier");
    d.save(&saved).unwrap();
    std::fs::remove_file(&path).unwrap();
    let mut h = app();
    h.state_mut().status = String::new();
    h.state_mut().open_path(&saved);
    h.run_steps(2);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    let said = |h: &Harness<'static, AtelierApp>| {
        let s = &h.state().status;
        (s.contains("logo") || s.contains("smart6-alert-links-not-found"))
            && h.state().workspace.error_status.as_ref() == Some(s)
    };
    assert!(said(&h), "said: {:?}", h.state().status);
    assert!(smart(&h).link.as_ref().unwrap().missing, "with its badge");
    // Looked at again (focus back): not said again.
    h.state_mut().status = String::new();
    h.state_mut().smart.focused = false;
    h.run_steps(1);
    settle(&mut h, |app| !app.links_reading());
    h.run_steps(2);
    assert!(h.state().status.is_empty(), "once: {:?}", h.state().status);
}

/// A smart filter's dialog cancelled after its preview was computed: the
/// preview's rendering (its base and filtered images) is let go.
#[test]
fn cancelled_smart_filter_dialog_lets_its_rendering_go() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::ConvertForFilters));
    h.state_mut().run(Action::Filter(FilterKind::GaussianBlur));
    settle(&mut h, |app| {
        matches!(
            app.preview_shown.as_ref().map(|p| &p.preview),
            Some(LayerPreview::Filter(_))
        )
    });
    assert!(h.state().smart.ready_held(), "kept for OK");
    h.key_press(egui::Key::Escape);
    h.run_steps(5);
    assert!(!h.state().filters.busy());
    assert!(!h.state().smart.ready_held(), "nothing held once closed");
}

/// Edit Contents of a smart object linked to a network file: the file is
/// looked at on another thread first (the window waits), then, missing,
/// the copy opens; its Save embeds it as for any missing file.
#[test]
fn network_link_looked_at_before_edit_contents() {
    let mut h = app();
    h.state_mut().run(Action::Smart(Command::Convert));
    h.run_steps(2);
    let unc = std::path::PathBuf::from(r"\\nas\share\logo.png");
    let s = h.state_mut().doc.as_mut().unwrap().active_layer_mut().smart.as_deref_mut().unwrap();
    s.link = Some(crate::document::Link {
        unchecked: true,
        ..crate::document::Link::new(&unc)
    });
    h.state_mut().run(Action::Smart(Command::EditContents));
    assert!(h.state().smart.checking(), "looked at first");
    settle(&mut h, |app| !app.smart.checking());
    h.run_steps(2);
    assert!(doc(&h).embedded_in.is_some(), "missing: the copy");
    h.state_mut().switch_tab(0);
    let link = smart(&h).link.clone().unwrap();
    assert!(link.missing && !link.unchecked);
}

/// Update Modified Content while a focus read of the document goes on:
/// its network files are read once that read lands, the missing files
/// said again.
#[test]
fn update_modified_content_during_a_focus_read() {
    let mut h = app();
    let uid = doc(&h).uid();
    let held = h.state_mut().smart.hold_links_read(uid);
    h.state_mut().run(Action::Smart(Command::UpdateModified));
    assert!(h.state().smart.network_read_waits(uid));
    drop(held);
    h.run_steps(2);
    assert!(!h.state().smart.network_read_waits(uid));
}
