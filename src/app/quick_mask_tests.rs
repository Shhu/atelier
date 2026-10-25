//! Moving and transforming the quick mask (Move tool, arrows, Free
//! Transform, Edit › Transform) driven through the real app.

use super::*;
use egui_kittest::Harness;

const RED: image::Rgba<u8> = image::Rgba([200, 0, 0, 255]);
const BLUE: image::Rgba<u8> = image::Rgba([0, 0, 200, 255]);

/// 200 × 200: red on the left half, blue on the right.
fn halves() -> Document {
    let image = image::RgbaImage::from_fn(200, 200, |x, _| if x < 100 { RED } else { BLUE });
    Document::new(image, None)
}

fn app_with(doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut harness = Harness::builder()
        .with_size([1400.0, 1000.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    harness.run_steps(3);
    harness
}

fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
    PixelRect { x, y, w, h }
}

fn screen(h: &Harness<'_, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state()
        .last_view
        .expect("canvas shown")
        .to_screen(pos2(x, y))
}

fn button(h: &mut Harness<'_, AtelierApp>, at: Pos2, pressed: bool, m: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: m,
    });
    h.run_steps(1);
}

fn drag(h: &mut Harness<'_, AtelierApp>, from: Pos2, to: Pos2, m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    h.hover_at(from);
    h.run_steps(1);
    button(h, from, true, m);
    for i in 1..=4 {
        h.hover_at(from.lerp(to, i as f32 / 4.0));
        h.run_steps(1);
    }
    button(h, to, false, m);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn key(h: &mut Harness<'_, AtelierApp>, key: Key, m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    for pressed in [true, false] {
        h.event(egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed,
            repeat: false,
            modifiers: m,
        });
    }
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn doc<'a>(h: &'a Harness<'_, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn mask_at(h: &Harness<'_, AtelierApp>, x: u32, y: u32) -> u8 {
    doc(h)
        .quick_mask()
        .expect("in Quick Mask")
        .get_pixel(x, y)
        .0[0]
}

fn last_step(h: &Harness<'_, AtelierApp>) -> String {
    doc(h)
        .history()
        .0
        .last()
        .copied()
        .unwrap_or_default()
        .to_string()
}

/// Quick Mask Mode from the selection (20, 20)–(80, 80), with `tool`.
fn quick_mask(tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = app_with(halves());
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(doc(&h).quick_mask().is_some());
    h.state_mut().tool = tool;
    h.run_steps(1);
    h
}

/// The Move tool drags the whole mask, the layer staying; one step,
/// undone and redone; leaving Quick Mask gives the moved selection.
#[test]
fn the_move_tool_drags_the_quick_mask() {
    let mut h = quick_mask(Tool::Transform);
    let steps = doc(&h).history().0.len();
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 100.0, 70.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(doc(&h).layers[0].offset, (0, 0), "the layer stays");
    assert_eq!(doc(&h).layers[0].pixel_at(50, 50), Some(RED.0));
    // (20, 20)–(80, 80) went to (70, 40)–(130, 100); what it left is
    // unselected.
    assert_eq!((mask_at(&h, 75, 45), mask_at(&h, 129, 99)), (255, 255));
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 130, 50)), (0, 0));
    assert_eq!(doc(&h).history().0.len(), steps + 1);
    assert_eq!(last_step(&h), t!("xchg-hist-move"));
    h.state_mut().undo();
    h.run_steps(1);
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 100, 90)), (255, 0));
    h.state_mut().redo();
    h.run_steps(1);
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 100, 90)), (0, 255));
    key(&mut h, Key::Q, Modifiers::NONE);
    assert!(doc(&h).quick_mask().is_none());
    let sel = h.state().selection.clone().expect("a selection");
    assert_eq!(sel.bounds(doc(&h).full_rect()), Some(rect(70, 40, 60, 60)));
}

/// With a selection, only its part of the mask moves, with the outline;
/// the place it leaves is unselected.
#[test]
fn a_drag_with_a_selection_moves_its_part_of_the_mask() {
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(20, 20, 30, 60).into());
    h.run_steps(1);
    let (from, to) = (screen(&h, 30.0, 50.0), screen(&h, 130.0, 50.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!((mask_at(&h, 30, 50), mask_at(&h, 60, 50)), (0, 255));
    assert_eq!((mask_at(&h, 130, 50), mask_at(&h, 160, 50)), (255, 0));
    assert_eq!(h.state().selection, Some(rect(120, 20, 30, 60).into()));
    assert_eq!(doc(&h).layers[0].pixel_at(30, 50), Some(RED.0));
    // Undone: the mask and the selection as they were.
    h.state_mut().undo();
    h.run_steps(1);
    assert_eq!((mask_at(&h, 30, 50), mask_at(&h, 130, 50)), (255, 0));
    assert_eq!(h.state().selection, Some(rect(20, 20, 30, 60).into()));
}

/// The arrows move the mask: a series of presses is one step.
#[test]
fn arrows_nudge_the_quick_mask() {
    let mut h = quick_mask(Tool::Transform);
    let steps = doc(&h).history().0.len();
    for _ in 0..3 {
        key(&mut h, Key::ArrowRight, Modifiers::NONE);
    }
    key(&mut h, Key::ArrowDown, Modifiers::SHIFT);
    assert_eq!((mask_at(&h, 22, 50), mask_at(&h, 23, 50)), (0, 255));
    assert_eq!((mask_at(&h, 82, 50), mask_at(&h, 83, 50)), (255, 0));
    assert_eq!((mask_at(&h, 50, 29), mask_at(&h, 50, 30)), (0, 255));
    assert_eq!(doc(&h).history().0.len(), steps + 1);
    assert_eq!(last_step(&h), t!("xchg-hist-move"));
    h.state_mut().undo();
    h.run_steps(1);
    assert_eq!((mask_at(&h, 20, 20), mask_at(&h, 80, 50)), (255, 0));
}

/// Ctrl+T on the mask, a corner dragged, Enter: the mask is scaled, as
/// one step, undone and redone.
#[test]
fn free_transform_scales_the_quick_mask() {
    let mut h = quick_mask(Tool::Brush);
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert_eq!(h.state().tool, Tool::Transform);
    assert_eq!(
        h.state().transform_rect,
        Some(to_rect(rect(20, 20, 60, 60)))
    );
    let (from, to) = (screen(&h, 80.0, 80.0), screen(&h, 140.0, 140.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(mask_at(&h, 100, 100), 0, "not before Enter");
    assert!(h.state().xform.quick.previewed(), "previewed");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none());
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 125, 125)), (255, 255));
    assert_eq!((mask_at(&h, 10, 10), mask_at(&h, 150, 150)), (0, 0));
    assert_eq!(doc(&h).layers[0].rect(), rect(0, 0, 200, 200));
    assert_eq!(last_step(&h), t!("menu-edit-free-transform"));
    h.state_mut().undo();
    h.run_steps(1);
    assert_eq!(mask_at(&h, 100, 100), 0);
    h.state_mut().redo();
    h.run_steps(1);
    assert_eq!(mask_at(&h, 100, 100), 255);
}

/// Esc gives Free Transform up: the mask, the selection and the history
/// stay as they were.
#[test]
fn escape_cancels_a_quick_mask_transform() {
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(20, 20, 30, 60).into());
    h.run_steps(1);
    let steps = doc(&h).history().0.len();
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert_eq!(
        h.state().transform_rect,
        Some(to_rect(rect(20, 20, 30, 60)))
    );
    let (from, to) = (screen(&h, 50.0, 80.0), screen(&h, 90.0, 120.0));
    drag(&mut h, from, to, Modifiers::NONE);
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none());
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 85, 85)), (255, 0));
    assert_eq!(doc(&h).history().0.len(), steps);
    assert_eq!(h.state().selection, Some(rect(20, 20, 30, 60).into()));
}

/// Edit › Transform › Flip Horizontal flips the mask within its box.
#[test]
fn flip_horizontal_flips_the_quick_mask() {
    let mut h = quick_mask(Tool::Brush);
    // An L: the top left of the square cleared.
    h.state_mut().selection = Some(rect(20, 20, 30, 30).into());
    h.state_mut().run(Action::ClearSelection);
    h.state_mut().run(Action::Deselect);
    h.run_steps(1);
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 70, 30)), (0, 255));
    h.state_mut()
        .run(Action::Transform(transform::Command::FlipHorizontal));
    h.run_steps(1);
    assert_eq!((mask_at(&h, 30, 30), mask_at(&h, 70, 30)), (255, 0));
    assert_eq!((mask_at(&h, 30, 70), mask_at(&h, 70, 70)), (255, 255));
    assert_eq!(mask_at(&h, 10, 30), 0);
    assert_eq!(last_step(&h), t!("xform-flip-horizontal"));
    assert_eq!(doc(&h).layers[0].pixel_at(30, 30), Some(RED.0));
}

/// The layer's lock doesn't protect the quick mask: it moves and
/// transforms, the layer staying.
#[test]
fn a_locked_layer_lets_the_quick_mask_move() {
    let mut h = app_with(halves());
    doc_mut(&mut h).active_layer_mut().locked = true;
    h.state_mut().selection = Some(rect(20, 20, 60, 60).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 60.0, 50.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!((mask_at(&h, 25, 50), mask_at(&h, 85, 50)), (0, 255));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!((mask_at(&h, 30, 50), mask_at(&h, 31, 50)), (0, 255));
    key(&mut h, Key::T, Modifiers::COMMAND);
    assert!(h.state().transform_rect.is_some());
    key(&mut h, Key::Escape, Modifiers::NONE);
    let locked = t!(
        "status-layer-is-locked",
        name = doc(&h).active_layer().name.as_str()
    );
    assert!(
        !h.state().status.contains(&locked),
        "{:?}",
        h.state().status
    );
    assert_eq!(doc(&h).layers[0].offset, (0, 0));
}

fn doc_mut<'a>(h: &'a mut Harness<'_, AtelierApp>) -> &'a mut Document {
    h.state_mut().doc.as_mut().unwrap()
}

/// A large mask is transformed on another thread; the result lands.
#[test]
fn a_large_quick_mask_is_transformed_aside() {
    let mut h = app_with(Document::new(image::RgbaImage::new(6000, 4000), None));
    h.state_mut().selection = Some(rect(1000, 1000, 2000, 1000).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    h.state_mut()
        .run(Action::Transform(transform::Command::RotateCw));
    assert!(h.state().transform_busy(), "on another thread");
    let start = std::time::Instant::now();
    while h.state().transform_busy() {
        assert!(start.elapsed().as_secs() < 60, "never done");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    // (1000, 1000)–(3000, 2000) turned around its center.
    assert_eq!((mask_at(&h, 2000, 600), mask_at(&h, 1200, 1500)), (255, 0));
    assert_eq!(last_step(&h), t!("optbar-rotate-cw"));
    // A drag of it.
    h.state_mut().tool = Tool::Transform;
    h.run_steps(1);
    let (from, to) = (screen(&h, 2000.0, 1500.0), screen(&h, 3000.0, 1500.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!((mask_at(&h, 2000, 1500), mask_at(&h, 3000, 1500)), (0, 255));
}

/// Alt+drag with a selection moves a copy of its part of the mask; Ctrl
/// with a brush is the Move tool for a moment, painting nothing.
#[test]
fn alt_copies_and_ctrl_moves_the_quick_mask() {
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(20, 20, 30, 60).into());
    h.run_steps(1);
    let (from, to) = (screen(&h, 30.0, 50.0), screen(&h, 130.0, 50.0));
    drag(&mut h, from, to, Modifiers::ALT);
    assert_eq!((mask_at(&h, 30, 50), mask_at(&h, 130, 50)), (255, 255));
    assert_eq!(mask_at(&h, 160, 50), 0);
    h.state_mut().run(Action::Deselect);
    h.state_mut().tool = Tool::Brush;
    h.run_steps(1);
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 50.0, 150.0));
    drag(&mut h, from, to, Modifiers::COMMAND);
    assert_eq!((mask_at(&h, 30, 50), mask_at(&h, 30, 150)), (0, 255));
    assert_eq!((mask_at(&h, 130, 150), mask_at(&h, 160, 150)), (255, 0));
}

/// Show Transform Controls: a handle of the mask's frame starts Free
/// Transform, a drag inside it moves the mask.
#[test]
fn transform_controls_frame_the_quick_mask() {
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().show_transform_controls = true;
    h.run_steps(1);
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 60.0, 50.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none(), "a move, applied");
    assert_eq!((mask_at(&h, 25, 50), mask_at(&h, 85, 50)), (0, 255));
    let (from, to) = (screen(&h, 90.0, 80.0), screen(&h, 150.0, 140.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert!(h.state().transform_rect.is_some(), "Free Transform");
    key(&mut h, Key::Enter, Modifiers::NONE);
    assert_eq!((mask_at(&h, 140, 130), mask_at(&h, 155, 145)), (255, 0));
}

/// Keys pressed one after the other with `m` held throughout.
fn keys_held(h: &mut Harness<'_, AtelierApp>, keys: &[Key], m: Modifiers) {
    h.event(egui::Event::ModifiersChanged(m));
    h.run_steps(1);
    for &key in keys {
        for pressed in [true, false] {
            h.event(egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed,
                repeat: false,
                modifiers: m,
            });
        }
        h.run_steps(1);
    }
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// Warp, then another mode (the warp applied, pending), then Esc: the
/// mask and the selection outline both come back as they were.
#[test]
fn escape_after_a_warp_and_a_mode_switch_restores_the_selection() {
    use super::transform::{Command, Mode};
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(20, 20, 30, 60).into());
    h.run_steps(1);
    let steps = doc(&h).history().0.len();
    h.state_mut().transform_command(Command::Mode(Mode::Warp));
    h.run_steps(1);
    h.state_mut()
        .set_warp_style((crate::xform::WarpStyle::Arc, 80.0, false));
    h.run_steps(1);
    h.state_mut()
        .transform_command(Command::Mode(Mode::Distort));
    h.run_steps(2);
    while h.state().transform_busy() {
        h.run_steps(1);
    }
    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(h.state().transform_rect.is_none());
    assert_eq!(h.state().selection, Some(rect(20, 20, 30, 60).into()));
    assert_eq!((mask_at(&h, 25, 25), mask_at(&h, 25, 85)), (255, 0));
    assert_eq!(doc(&h).history().0.len(), steps);
}

/// The Type and shape tools would make layers: refused in Quick Mask
/// Mode, with a status; the mask and the layers stay.
#[test]
fn type_and_shapes_are_refused_in_quick_mask() {
    use super::vector_tools::ShapeTool;
    let refused = t!("sel6-status-quick-mask-no-layers", key = keys::key(Key::Q));
    let mut h = quick_mask(Tool::Shape(ShapeTool::Rectangle));
    let steps = doc(&h).history().0.len();
    let (from, to) = (screen(&h, 100.0, 100.0), screen(&h, 160.0, 160.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(doc(&h).layers.len(), 1, "no shape layer");
    assert_eq!(mask_at(&h, 130, 130), 0);
    assert_eq!(doc(&h).history().0.len(), steps);
    assert_eq!(h.state().status, refused);

    let mut h = quick_mask(Tool::Text);
    let at = screen(&h, 120.0, 120.0);
    h.hover_at(at);
    h.run_steps(1);
    button(&mut h, at, true, Modifiers::NONE);
    button(&mut h, at, false, Modifiers::NONE);
    assert_eq!(h.state().status, refused);
    h.event(egui::Event::Text("Hello".into()));
    h.run_steps(2);
    key(&mut h, Key::Enter, Modifiers::COMMAND);
    assert_eq!(doc(&h).layers.len(), 1, "no text layer");

    // Shapes as paths make no layer: still drawn.
    let mut h = quick_mask(Tool::Shape(ShapeTool::Rectangle));
    h.state_mut().vector.shape_mode = super::vector_tools::ToolMode::Path;
    h.run_steps(1);
    drag(&mut h, from, to, Modifiers::NONE);
    assert_ne!(h.state().status, refused);
}

/// Nothing selected in the mask: an arrow or a drag moves nothing and
/// records nothing, and says so.
#[test]
fn moving_an_empty_quick_mask_records_nothing() {
    let mut h = quick_mask(Tool::Transform);
    // All masked.
    h.state_mut().run(Action::SelectAll);
    h.state_mut().run(Action::ClearSelection);
    h.state_mut().run(Action::Deselect);
    h.run_steps(1);
    assert_eq!((mask_at(&h, 50, 50), mask_at(&h, 150, 150)), (0, 0));
    let steps = doc(&h).history().0.len();
    let nothing = t!("xform-status-nothing", name = t!("qm6-quick-mask-name"));
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(doc(&h).history().0.len(), steps);
    assert_eq!(h.state().status, nothing);
    h.state_mut().status.clear();
    let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 100.0, 70.0));
    drag(&mut h, from, to, Modifiers::NONE);
    assert_eq!(doc(&h).history().0.len(), steps);
    assert_eq!(h.state().status, nothing);
    // A selection over a masked area: the same.
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(120, 20, 30, 60).into());
    h.run_steps(1);
    let steps = doc(&h).history().0.len();
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(doc(&h).history().0.len(), steps);
    assert_eq!(h.state().selection, Some(rect(120, 20, 30, 60).into()));
}

/// Alt+arrows with the Move tool and a selection, as its Alt+drag: a copy
/// of the selected part moves, the original stays; the presses of the
/// series move the copy on, in one step.
#[test]
fn alt_arrows_move_a_copy_of_the_quick_mask() {
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(20, 20, 30, 60).into());
    h.run_steps(1);
    let steps = doc(&h).history().0.len();
    let right = [Key::ArrowRight; 3];
    keys_held(&mut h, &right, Modifiers::ALT);
    // The original (20..50) stays whole, the copy went to 23..53.
    assert_eq!((mask_at(&h, 20, 50), mask_at(&h, 21, 50)), (255, 255));
    assert_eq!((mask_at(&h, 52, 50), mask_at(&h, 50, 50)), (255, 255));
    assert_eq!(mask_at(&h, 53, 50), 255, "the rest of the mask (to 80)");
    assert_eq!(h.state().selection, Some(rect(23, 20, 30, 60).into()));
    assert_eq!(doc(&h).history().0.len(), steps + 1, "one step");
    h.state_mut().undo();
    h.run_steps(1);
    assert_eq!(h.state().selection, Some(rect(20, 20, 30, 60).into()));
}

/// With a copy over an unselected area, the original part is still there
/// and the copy lands where it went.
#[test]
fn alt_arrows_copy_the_quick_mask_over_a_masked_area() {
    let mut h = quick_mask(Tool::Transform);
    h.state_mut().selection = Some(rect(60, 20, 20, 60).into());
    h.run_steps(1);
    keys_held(&mut h, &[Key::ArrowRight; 30], Modifiers::ALT);
    assert_eq!((mask_at(&h, 60, 50), mask_at(&h, 79, 50)), (255, 255));
    assert_eq!(
        (mask_at(&h, 61, 50), mask_at(&h, 70, 50)),
        (255, 255),
        "no hole"
    );
    assert_eq!((mask_at(&h, 85, 50), mask_at(&h, 90, 50)), (0, 255));
    assert_eq!((mask_at(&h, 109, 50), mask_at(&h, 110, 50)), (255, 0));
}

/// Ctrl with a marquee and no selection is the Move tool for a moment in
/// Quick Mask Mode too: the drag moves the mask, one step.
#[test]
fn ctrl_drag_with_a_marquee_moves_the_quick_mask() {
    for tool in [Tool::Select, Tool::Ellipse] {
        let mut h = quick_mask(tool);
        assert!(h.state().selection.is_none());
        let steps = doc(&h).history().0.len();
        let (from, to) = (screen(&h, 50.0, 50.0), screen(&h, 100.0, 70.0));
        drag(&mut h, from, to, Modifiers::COMMAND);
        assert_eq!((mask_at(&h, 75, 45), mask_at(&h, 30, 30)), (255, 0));
        assert_eq!(doc(&h).history().0.len(), steps + 1);
    }
}

/// Align in Quick Mask Mode moves the mask (with a selection, its part),
/// not the layer: one step.
#[test]
fn align_in_quick_mask_moves_the_mask() {
    let mut h = quick_mask(Tool::Brush);
    let steps = doc(&h).history().0.len();
    h.state_mut().run(Action::Align(AlignOp::Left));
    h.run_steps(1);
    assert_eq!(doc(&h).layers[0].offset, (0, 0), "the layer stays");
    assert_eq!((mask_at(&h, 0, 50), mask_at(&h, 59, 50)), (255, 255));
    assert_eq!(mask_at(&h, 60, 50), 0);
    assert_eq!(doc(&h).history().0.len(), steps + 1);
    assert_eq!(last_step(&h), t!("xchg-hist-align"));
    // With a selection: its part goes to the bottom, with the outline.
    h.state_mut().selection = Some(rect(0, 20, 30, 60).into());
    h.run_steps(1);
    h.state_mut().run(Action::Align(AlignOp::Bottom));
    h.run_steps(1);
    assert_eq!((mask_at(&h, 10, 199), mask_at(&h, 10, 139)), (255, 0));
    assert_eq!((mask_at(&h, 40, 50), mask_at(&h, 40, 199)), (255, 0));
    assert_eq!(h.state().selection, Some(rect(0, 140, 30, 60).into()));
}

/// Timings of the quick mask's moves at 6000 × 4000 (arrows, a drag with
/// and without a selection): `cargo test qm_move_timings -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn qm_move_timings() {
    use std::time::Instant;
    let mut h = app_with(Document::new(image::RgbaImage::new(6000, 4000), None));
    h.state_mut().selection = Some(rect(1000, 1000, 2000, 1000).into());
    key(&mut h, Key::Q, Modifiers::NONE);
    h.state_mut().tool = Tool::Transform;
    h.run_steps(2);
    for with_sel in [false, true] {
        if with_sel {
            h.state_mut().selection = Some(rect(500, 500, 3000, 2000).into());
            h.run_steps(2);
        }
        for _ in 0..3 {
            let t = Instant::now();
            key(&mut h, Key::ArrowRight, Modifiers::NONE);
            println!("selection {with_sel}: arrow (2 frames) {:?}", t.elapsed());
        }
        let (from, to) = (screen(&h, 2000.0, 1500.0), screen(&h, 2600.0, 1500.0));
        h.hover_at(from);
        h.run_steps(1);
        let t = Instant::now();
        button(&mut h, from, true, Modifiers::NONE);
        println!("selection {with_sel}: press {:?}", t.elapsed());
        for i in 1..=6 {
            let t = Instant::now();
            h.hover_at(from.lerp(to, i as f32 / 6.0));
            h.run_steps(1);
            println!("selection {with_sel}: drag frame {:?}", t.elapsed());
        }
        let t = Instant::now();
        button(&mut h, to, false, Modifiers::NONE);
        println!("selection {with_sel}: release {:?}", t.elapsed());
    }
}
