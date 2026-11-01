//! The partial locks and styles on any layer, driven through the app.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use image::{Rgba, RgbaImage};

use super::layer_locks::{LastLock, LockKind};
use super::*;
use crate::document::Locks;

const RED: [u8; 4] = [255, 0, 0, 128];

/// White 200 × 100; a layer over it, half red at half opacity, half empty
/// (active).
fn app() -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let mut doc = Document::new(RgbaImage::from_pixel(200, 100, Rgba([255; 4])), None);
            let half = RgbaImage::from_fn(200, 100, |x, _| match x < 100 {
                true => Rgba(RED),
                false => Rgba([0; 4]),
            });
            doc.add_layer("half".into(), half, (0, 0));
            app.doc = Some(doc);
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn doc_mut<'a>(h: &'a mut Harness<'static, AtelierApp>) -> &'a mut Document {
    h.state_mut().doc.as_mut().unwrap()
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state().last_view.unwrap().to_screen(pos2(x, y))
}

/// A press at `from`, a move to `to`, a release.
fn drag(h: &mut Harness<'static, AtelierApp>, from: (f32, f32), to: (f32, f32)) {
    let (a, b) = (screen(h, from.0, from.1), screen(h, to.0, to.1));
    h.hover_at(a);
    h.run_steps(1);
    h.drag_at(a);
    h.run_steps(1);
    h.hover_at(b);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: b,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
}

fn pixel(h: &Harness<'static, AtelierApp>, x: u32, y: u32) -> [u8; 4] {
    doc(h).active_layer().image().get_pixel(x, y).0
}

#[test]
fn the_brush_keeps_the_opacity_under_lock_transparent_pixels() {
    let mut h = app();
    doc_mut(&mut h).active_layer_mut().locks.transparent = true;
    let app = h.state_mut();
    app.primary = Color32::from_rgb(0, 255, 0);
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.set_tool(Tool::Brush);
    h.run_steps(1);
    drag(&mut h, (90.0, 50.0), (110.0, 50.0));
    assert_eq!(
        pixel(&h, 95, 50),
        [0, 255, 0, 128],
        "color changes, not alpha"
    );
    assert_eq!(pixel(&h, 105, 50)[3], 0, "the empty part stays so");
    // One step.
    assert!(doc_mut(&mut h).undo());
    assert_eq!(pixel(&h, 95, 50), RED);
}

/// The eraser takes nothing away: it paints the background color.
#[test]
fn the_eraser_paints_the_background_color_under_lock_transparent_pixels() {
    let mut h = app();
    doc_mut(&mut h).active_layer_mut().locks.transparent = true;
    let app = h.state_mut();
    app.secondary = Color32::from_rgb(0, 0, 255);
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.set_tool(Tool::Eraser);
    h.run_steps(1);
    drag(&mut h, (40.0, 50.0), (50.0, 50.0));
    assert_eq!(pixel(&h, 45, 50), [0, 0, 255, 128]);
}

#[test]
fn delete_asks_how_to_fill_under_lock_transparent_pixels() {
    let mut h = app();
    doc_mut(&mut h).active_layer_mut().locks.transparent = true;
    let area = crate::document::PixelRect {
        x: 0,
        y: 0,
        w: 50,
        h: 50,
    };
    h.state_mut().selection = Some(area.into());
    h.state_mut().run(Action::ClearSelection);
    assert!(h.state().fill_dialog.is_some());
    assert_eq!(pixel(&h, 5, 5), RED);
    // Nor do these take pixels away.
    h.state_mut().fill_dialog = None;
    let n = doc(&h).layers.len();
    h.state_mut().run(Action::LayerViaCut);
    assert_eq!(doc(&h).layers.len(), n);
    assert!(h.state().status.contains("half"));
}

#[test]
fn lock_image_pixels_refuses_painting_and_transforms_but_moves() {
    let mut h = app();
    doc_mut(&mut h).active_layer_mut().locks.pixels = true;
    let rev = doc(&h).active_layer().rev();
    let app = h.state_mut();
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.set_tool(Tool::Brush);
    assert!(app.paints_on_locked_layer());
    h.run_steps(1);
    drag(&mut h, (40.0, 50.0), (50.0, 50.0));
    assert_eq!(doc(&h).active_layer().rev(), rev);
    for action in [
        Action::Desaturate,
        Action::Adjust(AdjustKind::Invert),
        Action::FillPrimary,
    ] {
        h.state_mut().run(action);
        assert_eq!(doc(&h).active_layer().rev(), rev, "{action:?}");
    }
    let refused = t!("layers8-status-pixels-locked", name = "half");
    assert!(h.state().status.contains(&refused), "{}", h.state().status);
    h.state_mut()
        .run(Action::Transform(transform::Command::Free));
    assert!(!h.state().free_transform);
    // It still moves, without handles.
    let app = h.state_mut();
    app.auto_select = false;
    app.set_tool(Tool::Transform);
    assert!(!app.move_controls());
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(doc(&h).active_layer().offset, (1, 0));
    assert_eq!(doc(&h).active_layer().rev(), rev);
}

#[test]
fn lock_position_refuses_moves_and_transforms_but_paints() {
    let mut h = app();
    doc_mut(&mut h).active_layer_mut().locks.position = true;
    let app = h.state_mut();
    app.auto_select = false;
    app.set_tool(Tool::Transform);
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    assert_eq!(doc(&h).active_layer().offset, (0, 0));
    let refused = t!("layers8-status-position-locked", name = "half");
    assert!(h.state().status.contains(&refused), "{}", h.state().status);
    drag(&mut h, (40.0, 50.0), (60.0, 50.0));
    assert_eq!(doc(&h).active_layer().offset, (0, 0), "not dragged");
    h.state_mut().run(Action::CenterElement);
    assert_eq!(doc(&h).active_layer().offset, (0, 0));
    h.state_mut()
        .run(Action::Transform(transform::Command::Free));
    assert!(!h.state().free_transform);
    // Painting goes on.
    let app = h.state_mut();
    app.primary = Color32::from_rgb(0, 255, 0);
    (app.brush_size, app.hardness) = (20.0, 1.0);
    app.set_tool(Tool::Brush);
    h.run_steps(1);
    drag(&mut h, (40.0, 50.0), (50.0, 50.0));
    assert_eq!(pixel(&h, 45, 50)[1], 255);
}

/// In Quick Mask Mode, the mask is what is edited.
#[test]
fn quick_mask_is_under_no_partial_lock() {
    let mut h = app();
    let l = doc_mut(&mut h).active_layer_mut();
    l.locks = Locks::ALL;
    h.state_mut()
        .run(Action::Select(select_menu::Command::QuickMask));
    assert!(h.state().in_quick_mask());
    assert!(!h.state().transform_frozen());
    assert!(!h.state().transform_locked());
}

/// A group's lock holds for its layers; their own toggles are greyed.
#[test]
fn children_inherit_and_cannot_undo_their_group_lock() {
    let mut h = app();
    assert!(doc_mut(&mut h).group_layers(&[1]));
    doc_mut(&mut h).layers[2].locks.position = true;
    doc_mut(&mut h).active = 1;
    let d = doc(&h);
    assert_eq!(LockKind::Position.state(d, 1), (true, false));
    assert_eq!(LockKind::Pixels.state(d, 1), (false, true));
    h.state_mut().run(Action::Lock(LockKind::Position));
    assert!(!doc(&h).layers[1].locks.position, "not its own to change");
    h.key_press(Key::ArrowRight);
    assert_eq!(doc(&h).layers[1].offset, (0, 0));
    doc_mut(&mut h).layers[2].locks.position = false;
    doc_mut(&mut h).layers[2].locked = true;
    assert_eq!(LockKind::Transparent.state(doc(&h), 1), (true, false));
}

#[test]
fn text_and_fills_only_take_position_and_all() {
    let mut h = app();
    doc_mut(&mut h).add_fill_layer(crate::document::Fill::Solid([1, 2, 3]), None);
    let d = doc(&h);
    let active = d.active;
    assert_eq!(LockKind::Transparent.state(d, active), (false, false));
    assert_eq!(LockKind::Pixels.state(d, active), (false, false));
    assert_eq!(LockKind::Position.state(d, active), (false, true));
    assert_eq!(LockKind::All.state(d, active), (false, true));
}

/// Ctrl+/: Lock All at first, then the last one used; one step each.
#[test]
fn ctrl_slash_toggles_the_last_lock_used() {
    let mut h = app();
    h.state_mut().run(Action::ToggleLock);
    assert!(doc(&h).active_layer().locked);
    h.state_mut().run(Action::ToggleLock);
    assert!(!doc(&h).active_layer().locked);
    h.state_mut().run(Action::Lock(LockKind::Position));
    assert!(doc(&h).active_layer().locks.position);
    h.state_mut().run(Action::ToggleLock);
    assert!(!doc(&h).active_layer().locks.any());
    h.state_mut().run(Action::ToggleLock);
    let l = doc(&h).active_layer();
    assert!(l.locks.position && !l.locked && !l.locks.pixels);
    assert!(doc_mut(&mut h).undo());
    assert!(!doc(&h).active_layer().locks.any());
}

#[test]
fn last_lock_survives_the_session() {
    let last = LastLock {
        locks: Locks {
            transparent: true,
            pixels: false,
            position: true,
        },
        all: false,
    };
    assert_eq!(LastLock::from_bits(last.to_bits()), last);
    assert_eq!(LastLock::from_bits(0), LastLock::default());
}

/// The row: its toggles have names, and act on the layer.
#[test]
fn the_panel_row_toggles_the_locks() {
    let mut h = app();
    let label = t!("layers8-lock-pixels");
    h.get_by_label(&label).click();
    h.run_steps(2);
    assert!(doc(&h).active_layer().locks.pixels);
    h.get_by_label(&label).click();
    h.run_steps(2);
    assert!(!doc(&h).active_layer().locks.pixels);
    h.get_by_label(&t!("layers8-lock-all")).click();
    h.run_steps(2);
    assert!(doc(&h).active_layer().locked);
    // Lock All covers the others: greyed and shown on.
    assert_eq!(LockKind::Position.state(doc(&h), 1), (true, false));
}

/// Layer › Lock Layers: entries in the menu, found by the palette.
#[test]
fn the_locks_are_in_the_layer_menu_and_greyed_when_unavailable() {
    let mut h = app();
    doc_mut(&mut h).add_fill_layer(crate::document::Fill::Solid([1, 2, 3]), None);
    let entries = h.state().lock_menu();
    assert_eq!(entries.len(), 4);
    let enabled: Vec<bool> = entries
        .iter()
        .map(|e| matches!(e, MenuEntry::Check(_, true, ..)))
        .collect();
    assert_eq!(enabled, [false, false, true, true]);
}

/// The Layer Style window opens on a group, a fill and an adjustment; one
/// step on OK, and the menu's entries are on.
#[test]
fn layer_style_opens_on_groups_fills_and_adjustments() {
    let mut h = app();
    doc_mut(&mut h).add_group();
    doc_mut(&mut h).add_fill_layer(crate::document::Fill::Solid([1, 2, 3]), None);
    doc_mut(&mut h).add_adjustment_layer(crate::pixel_ops::Adjustment::Invert, None);
    for i in 0..doc(&h).layers.len() {
        if doc(&h).layers[i].has_pixels() {
            continue;
        }
        doc_mut(&mut h).active = i;
        h.state_mut()
            .run(Action::LayerStyle(Some(Style::DropShadow)));
        h.run_steps(1);
        assert!(
            h.state().layer_style.is_some(),
            "{}",
            doc(&h).layers[i].name
        );
        doc_mut(&mut h).active_layer_mut().effects.shadow.enabled = true;
        h.run_steps(1);
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert!(h.state().layer_style.is_none());
        assert!(doc(&h).active_layer().effects.shadow.enabled);
        assert!(doc_mut(&mut h).undo());
        assert!(!doc(&h).active_layer().effects.shadow.enabled);
    }
}

/// A style on a group shows on the display (tiles recomposited, dirtied
/// by its extent).
#[test]
fn group_style_reaches_the_display_composite() {
    let mut h = app();
    assert!(doc_mut(&mut h).group_layers(&[1]));
    let group = doc_mut(&mut h).active_layer_mut();
    assert!(group.is_group());
    group.effects.stroke.enabled = true;
    group.effects.stroke.width = 4.0;
    group.effects.stroke.opacity = 1.0;
    group.effects.stroke.color = [0, 255, 0, 255];
    h.run_steps(3);
    let flat = doc(&h).flatten();
    assert_eq!(
        flat.get_pixel(102, 50).0[1],
        255,
        "the stroke beyond the content"
    );
}
