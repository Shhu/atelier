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
    h.run_steps(2);
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
    h.run_steps(2);
    assert!(!h.state().modal_open());
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
