//! Undo, redo and the History panel through the real app: selections and
//! Quick Mask as history steps, steps that change nothing, text edits, the
//! saved state.

use super::select_menu::ModifyKind;
use super::*;
use egui_kittest::Harness;

fn app(w: u32, h: u32) -> Harness<'static, AtelierApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 800.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 255, 255]));
            app.doc = Some(Document::new(image, None));
            app
        });
    harness.run_steps(2);
    harness
}

fn rect(x: i64, y: i64, w: u32, h: u32) -> Selection {
    PixelRect { x, y, w, h }.into()
}

fn doc<'a>(h: &'a Harness<'_, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

/// Runs frames until the selection computed in the background is there.
fn settle(h: &mut Harness<'_, AtelierApp>) {
    let start = std::time::Instant::now();
    while h.state().select_busy() {
        assert!(start.elapsed().as_secs() < 30, "selection never computed");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    h.run_steps(1);
}

#[test]
fn quick_mask_survives_undoing_a_crop() {
    let mut h = app(200, 100);
    let app = h.state_mut();
    app.doc.as_mut().unwrap().crop(PixelRect {
        x: 0,
        y: 0,
        w: 100,
        h: 50,
    });
    app.toggle_quick_mask();
    // Undoes entering Quick Mask Mode, then the crop.
    app.undo();
    assert!(app.doc.as_ref().unwrap().quick_mask().is_none());
    app.undo();
    assert_eq!(app.doc.as_ref().unwrap().width(), 200);
    // Back in the cropped state, in Quick Mask Mode, then out of it.
    app.redo();
    app.redo();
    app.toggle_quick_mask();
    let sel = app.selection.clone().expect("everything selected");
    assert_eq!(
        sel.rect,
        PixelRect {
            x: 0,
            y: 0,
            w: 100,
            h: 50
        }
    );
}

#[test]
fn quick_mask_follows_a_rotation_of_the_canvas() {
    let mut h = app(200, 100);
    h.state_mut().selection = Some(rect(0, 0, 50, 100));
    h.state_mut().toggle_quick_mask();
    h.state_mut().run(Action::RotateCanvas(90));
    h.run_steps(2);
    assert_eq!((doc(&h).width(), doc(&h).height()), (100, 200));
    h.state_mut().toggle_quick_mask();
    // The left quarter is now the top quarter.
    let sel = h.state().selection.clone().expect("a selection");
    assert_eq!(
        sel.rect,
        PixelRect {
            x: 0,
            y: 0,
            w: 100,
            h: 50
        }
    );
}

#[test]
fn every_tool_edits_the_quick_mask() {
    let mut h = app(100, 100);
    h.state_mut().toggle_quick_mask();
    // Edit › Fill with black: all of the mask, none of the layer.
    h.state_mut().primary = Color32::BLACK;
    h.state_mut().run(Action::FillPrimary);
    let d = doc(&h);
    assert_eq!(d.layers[0].pixel_at(50, 50), Some([0, 0, 255, 255]));
    assert_eq!(d.quick_mask().unwrap().get_pixel(50, 50).0[0], 0);
    // One step, undone like any other, before leaving the mode.
    h.state_mut().undo();
    assert_eq!(doc(&h).quick_mask().unwrap().get_pixel(50, 50).0[0], 255);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().toggle_quick_mask();
    assert!(h.state().selection.is_none());
    // Undo goes back into Quick Mask Mode, with the mask.
    h.state_mut().undo();
    assert!(doc(&h).quick_mask().is_some());
}

#[test]
fn a_brush_click_that_paints_nothing_keeps_redo() {
    let mut h = app(8, 8);
    let app = h.state_mut();
    app.primary = Color32::RED;
    app.run(Action::FillPrimary);
    app.undo();
    assert!(app.doc.as_ref().unwrap().can_redo());
    app.tool = Tool::Brush;
    app.brush_size = 2.0;
    app.selection = Some(rect(0, 0, 2, 2));
    app.settle_selection();
    let view = View {
        origin: Pos2::ZERO,
        zoom: 1.0,
    };
    let at = Some(pos2(6.0, 6.0));
    app.brush_input(view, at, true, true, false, false);
    app.brush_input(view, at, false, false, true, false);
    assert!(app.doc.as_ref().unwrap().can_redo());
}

#[test]
fn cancelling_a_text_with_a_full_history_removes_it() {
    let mut h = app(64, 64);
    let app = h.state_mut();
    for _ in 0..60 {
        app.doc.as_mut().unwrap().checkpoint("step".into());
    }
    let before = app.doc.as_ref().unwrap().layers.len();
    app.create_text(pos2(10.0, 30.0), None);
    app.type_text("abc");
    app.cancel_text();
    assert_eq!(app.doc.as_ref().unwrap().layers.len(), before);
    // An empty text committed is removed too.
    app.create_text(pos2(10.0, 30.0), None);
    app.commit_text();
    assert_eq!(app.doc.as_ref().unwrap().layers.len(), before);
}

#[test]
fn a_history_click_while_typing_commits_the_text() {
    let mut h = app(64, 64);
    let app = h.state_mut();
    app.create_text(pos2(10.0, 30.0), None);
    app.type_text("abc");
    app.commit_text();
    for _ in 0..5 {
        app.doc.as_mut().unwrap().checkpoint("step".into());
    }
    let total = app.doc.as_ref().unwrap().history().0.len();
    app.go_to_history(1);
    let i = app.doc.as_ref().unwrap().active;
    app.edit_text_layer(i, pos2(12.0, 30.0));
    app.go_to_history(total);
    app.cancel_text();
    assert_eq!(app.doc.as_ref().unwrap().history().1, total);
}

#[test]
fn selection_changes_are_history_steps() {
    let mut h = app(8, 8);
    let app = h.state_mut();
    app.primary = Color32::RED;
    app.run(Action::FillPrimary);
    app.run(Action::SelectAll);
    app.run(Action::Deselect);
    let names = app.doc.as_ref().unwrap().history().0.len();
    assert_eq!(names, 3);
    // Ctrl+Z brings the selection back and keeps the fill.
    app.undo();
    assert!(app.selection.is_some());
    let d = app.doc.as_ref().unwrap();
    assert_eq!(d.active_layer().pixel_at(0, 0), Some([255, 0, 0, 255]));
    app.redo();
    assert!(app.selection.is_none());
}

#[test]
fn undoing_a_select_command_restores_the_selection_not_the_pixels() {
    let mut h = app(100, 100);
    h.state_mut().selection = Some(rect(20, 20, 60, 60));
    h.state_mut().run(Action::ClearSelection);
    h.run_steps(1);
    h.state_mut()
        .modify_selection_step(ModifyKind::Contract, 5.0, false, false);
    settle(&mut h);
    assert_eq!(h.state().selection.as_ref().unwrap().rect.w, 50);
    h.state_mut().undo();
    assert_eq!(
        h.state().selection.as_ref().unwrap().rect,
        rect(20, 20, 60, 60).rect
    );
    assert_eq!(doc(&h).layers[0].pixel_at(50, 50).map(|p| p[3]), Some(0));
    // Undoing the Delete brings the pixels back.
    h.state_mut().undo();
    assert_eq!(doc(&h).layers[0].pixel_at(50, 50).map(|p| p[3]), Some(255));
}

#[test]
fn a_deselect_waits_for_the_selection_being_computed() {
    let mut h = app(3000, 2000);
    h.state_mut().selection = Some(rect(100, 100, 2800, 1800));
    h.state_mut()
        .modify_selection_step(ModifyKind::Feather, 40.0, false, false);
    assert!(h.state().select_busy());
    h.state_mut().run(Action::Deselect);
    settle(&mut h);
    assert!(h.state().selection.is_none());
    // Both are steps: the feather, then the deselection.
    h.state_mut().undo();
    assert!(
        h.state()
            .selection
            .as_ref()
            .is_some_and(|s| s.mask.is_some())
    );
}

#[test]
fn two_shapes_added_in_the_background_are_both_kept() {
    let mut h = app(4000, 3000);
    h.state_mut().selection = Some(rect(0, 0, 10, 10));
    let (a, b) = (rect(100, 100, 1800, 1800), rect(2000, 1000, 1800, 1800));
    h.state_mut()
        .select_shape(a.rect, true, SelectMode::Add, 0.0, true);
    h.state_mut()
        .select_shape(b.rect, true, SelectMode::Add, 0.0, true);
    settle(&mut h);
    let s = h.state().selection.clone().unwrap();
    assert_eq!(
        (
            s.coverage(1000, 1000),
            s.coverage(2900, 1900),
            s.coverage(5, 5)
        ),
        (255, 255, 255)
    );
}

#[test]
fn undo_back_to_the_saved_state_is_not_modified() {
    let mut h = app(8, 8);
    let app = h.state_mut();
    app.primary = Color32::RED;
    app.run(Action::FillPrimary);
    app.doc.as_mut().unwrap().mark_saved();
    app.run(Action::FillPrimary);
    assert!(app.doc.as_ref().unwrap().modified);
    app.undo();
    assert!(!app.doc.as_ref().unwrap().modified);
    app.undo();
    assert!(app.doc.as_ref().unwrap().modified);
}

#[test]
fn undo_and_redo_restore_the_targeted_mask() {
    let mut h = app(8, 8);
    let app = h.state_mut();
    let d = app.doc.as_mut().unwrap();
    d.add_empty_layer();
    d.add_mask(crate::document::NewMask::RevealAll, None);
    d.target_mask(false);
    d.checkpoint("step".into());
    d.target_mask(true);
    app.undo();
    assert!(!app.doc.as_ref().unwrap().mask_targeted());
    app.redo();
    assert!(app.doc.as_ref().unwrap().mask_targeted());
}

#[test]
fn a_history_click_waits_for_the_selection_being_computed() {
    let mut h = app(3000, 2000);
    {
        let app = h.state_mut();
        app.selection = Some(rect(0, 0, 100, 100));
        app.commit_selection(Some("select".into()));
        let d = app.doc.as_mut().unwrap();
        d.checkpoint("later 1".into());
        d.checkpoint("later 2".into());
    }
    h.state_mut()
        .run(Action::Select(select_menu::Command::Similar));
    assert!(h.state().select_busy());
    h.state_mut().go_to_history(1);
    settle(&mut h);
    // Applied before the click, then undone with the steps after it:
    // nothing lost.
    let (names, done) = doc(&h).history();
    assert_eq!(done, 1);
    assert_eq!(&names[..3], ["select", "later 1", "later 2"]);
    assert_eq!(names.len(), 4);
}

#[test]
fn a_selection_computed_from_an_undone_state_is_dropped() {
    let mut h = app(3000, 2000);
    {
        let app = h.state_mut();
        app.selection = Some(rect(100, 100, 2800, 1800));
        app.commit_selection(Some("select".into()));
        app.doc.as_mut().unwrap().checkpoint("later".into());
        app.modify_selection_step(ModifyKind::Feather, 40.0, false, false);
        assert!(app.select_busy());
        // An undo that does not wait (the history moved meanwhile).
        app.doc.as_mut().unwrap().undo();
    }
    settle(&mut h);
    assert!(doc(&h).can_redo(), "the redo list is kept");
    assert_eq!(doc(&h).history().0, ["select", "later"]);
}

#[test]
fn select_subject_found_after_an_undo_is_dropped() {
    let mut h = app(400, 300);
    {
        let app = h.state_mut();
        let mut img = image::RgbaImage::from_pixel(400, 300, image::Rgba([20, 40, 200, 255]));
        for y in 100..200 {
            for x in 150..250 {
                img.put_pixel(x, y, image::Rgba([240, 30, 20, 255]));
            }
        }
        app.doc = Some(Document::new(img, None));
        let d = app.doc.as_mut().unwrap();
        d.checkpoint("a".into());
        d.checkpoint("b".into());
        app.start_subject(false, false);
        app.run(Action::Undo);
    }
    let start = std::time::Instant::now();
    while h.state().subject_job.is_some() {
        assert!(start.elapsed().as_secs() < 60, "subject never found");
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run_steps(1);
    }
    assert!(doc(&h).can_redo());
    assert_eq!(doc(&h).history(), (vec!["a", "b"], 1));
    assert!(h.state().selection.is_none());
}

#[test]
fn a_modify_queued_behind_another_keeps_its_name() {
    let mut h = app(3000, 2000);
    h.state_mut().selection = Some(rect(100, 100, 2800, 1800));
    h.state_mut().settle_selection();
    h.state_mut()
        .modify_selection_step(ModifyKind::Feather, 30.0, false, false);
    h.state_mut()
        .modify_selection_step(ModifyKind::Contract, 10.0, false, false);
    settle(&mut h);
    let names: Vec<String> = doc(&h).history().0.iter().map(|s| s.to_string()).collect();
    let name = |k: ModifyKind| select_menu::step_label(k.menu_label());
    assert_eq!(
        names,
        [name(ModifyKind::Feather), name(ModifyKind::Contract)]
    );
}

#[test]
fn esc_on_a_text_keeps_a_layers_panel_change() {
    let mut h = app(64, 64);
    let app = h.state_mut();
    app.doc.as_mut().unwrap().add_empty_layer();
    app.create_text(pos2(10.0, 30.0), None);
    app.type_text("abc");
    // The Layers panel's eye on the background (as `layers_list` does).
    app.record_change("Hide Layer".into(), None);
    app.doc.as_mut().unwrap().layers[0].visible = false;
    app.type_text("d");
    app.cancel_text();
    let d = app.doc.as_ref().unwrap();
    assert!(!d.layers[0].visible);
    assert!(
        d.layers.iter().any(|l| l.text.is_some()),
        "the text was committed"
    );
}

#[test]
fn clicking_the_current_row_with_an_empty_text_redoes_nothing() {
    let mut h = app(64, 64);
    let app = h.state_mut();
    for i in 0..4 {
        app.doc.as_mut().unwrap().checkpoint(format!("s{i}"));
    }
    app.undo();
    app.undo();
    app.create_text(pos2(10.0, 30.0), None);
    let (_, done) = app.doc.as_ref().unwrap().history();
    app.go_to_history(done);
    // The empty text goes away, the undone steps stay undone.
    assert_eq!(
        app.doc.as_ref().unwrap().history(),
        (vec!["s0", "s1", "s2", "s3"], 2)
    );
}
