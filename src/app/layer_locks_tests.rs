//! The partial locks and styles on any layer, driven through the app.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable};
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

// ---------------------------------------------------------- Every path, one rule

fn sel(x: i64, y: i64, w: u32, h: u32) -> Selection {
    PixelRect { x, y, w, h }.into()
}

fn settle(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..1000 {
        h.run_steps(1);
        if !h.state().filters.busy() && !h.state().retouch.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("still busy");
}

fn steps(h: &Harness<'static, AtelierApp>) -> usize {
    doc(h).history().1
}

/// A group of the active layer and a free one over it: (layer, free, group).
fn in_a_group(h: &mut Harness<'static, AtelierApp>) -> (usize, usize, usize) {
    let child = doc(h).active;
    let image = RgbaImage::from_pixel(20, 20, Rgba(RED));
    doc_mut(h).add_layer("free".into(), image, (5, 5));
    let free = doc(h).active;
    assert!(doc_mut(h).group_layers(&[child, free]));
    (child, free, doc(h).active)
}

fn shape() -> crate::vector::Shape {
    use crate::vector::{Bounds, Live, Paint, Shape, ShapeKind, ShapeStroke, pt};
    let bounds = Bounds::of(pt(20.0, 20.0), pt(80.0, 70.0));
    let live = Live::new(ShapeKind::Rectangle, bounds);
    Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default())
}

#[test]
fn free_transform_of_a_selection_keeps_lock_transparent_pixels() {
    let commands = [
        transform::Command::Free,
        transform::Command::FlipHorizontal,
    ];
    for command in commands {
        let mut h = app();
        let a = doc(&h).active;
        doc_mut(&mut h).layers[a].locks.transparent = true;
        h.state_mut().selection = Some(sel(10, 10, 20, 20));
        let (before, n) = (doc(&h).layers[a].image().clone(), steps(&h));
        h.state_mut().run(Action::Transform(command));
        settle(&mut h);
        assert_eq!(*doc(&h).layers[a].image(), before, "{command:?}");
        assert_eq!((doc(&h).layers.len(), steps(&h)), (2, n), "{command:?}");
        assert!(h.state().transform_rect.is_none());
        assert!(h.state().status.contains("half"), "{}", h.state().status);
    }
    // The whole layer lifts nothing: it stays allowed.
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    h.state_mut().run(Action::Transform(transform::Command::Free));
    assert!(h.state().transform_rect.is_some());
}

#[test]
fn apply_mask_is_refused_by_the_pixel_locks_with_the_mask_targeted() {
    let pixels = Locks { pixels: true, ..Default::default() };
    let transparent = Locks { transparent: true, ..Default::default() };
    for locks in [pixels, transparent] {
        let mut h = app();
        let a = doc(&h).active;
        doc_mut(&mut h).add_mask(NewMask::HideAll, None);
        doc_mut(&mut h).layers[a].locks = locks;
        for target in [true, false] {
            if target {
                doc_mut(&mut h).target_mask(true);
            } else {
                doc_mut(&mut h).target_mask(false);
            }
            let n = steps(&h);
            h.state_mut().run(Action::ApplyMask);
            assert!(doc(&h).layers[a].mask.is_some(), "{locks:?}");
            assert_eq!(pixel(&h, 10, 10), RED);
            assert_eq!(steps(&h), n);
        }
    }
    // Without a lock it applies.
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).add_mask(NewMask::HideAll, None);
    h.state_mut().run(Action::ApplyMask);
    assert!(doc(&h).layers[a].mask.is_none());
}

#[test]
fn lock_image_pixels_stops_transforms_with_the_linked_mask_targeted() {
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).add_mask(NewMask::RevealAll, None);
    doc_mut(&mut h).target_mask(true);
    doc_mut(&mut h).layers[a].locks.pixels = true;
    // Painting the mask stays free.
    assert!(!doc(&h).pixels_locked());
    assert!(h.state().transform_frozen());
    let before = doc(&h).layers[a].image().clone();
    for command in [transform::Command::FlipHorizontal, transform::Command::Free] {
        h.state_mut().run(Action::Transform(command));
        settle(&mut h);
        assert_eq!(*doc(&h).layers[a].image(), before);
        assert!(h.state().transform_rect.is_none(), "{command:?}");
    }
}

#[test]
fn a_group_command_respects_the_locks_of_its_layers() {
    let position = Locks { position: true, ..Default::default() };
    let pixels = Locks { pixels: true, ..Default::default() };
    for (locks, all, moves) in [(position, false, false), (pixels, false, true), (Locks::default(), true, false)] {
        let setup = || {
            let mut h = app();
            let (child, free, g) = in_a_group(&mut h);
            doc_mut(&mut h).layers[child].locks = locks;
            doc_mut(&mut h).layers[child].locked = all;
            (h, child, free, g)
        };
        // Transforms: refused altogether.
        for command in [transform::Command::Free, transform::Command::FlipHorizontal] {
            let (mut h, child, free, _) = setup();
            let before = (doc(&h).layers[child].image().clone(), doc(&h).layers[free].offset);
            h.state_mut().run(Action::Transform(command));
            settle(&mut h);
            let after = (doc(&h).layers[child].image().clone(), doc(&h).layers[free].offset);
            assert_eq!(after, before, "{locks:?} {all} {command:?}");
            assert!(h.state().transform_rect.is_none());
            assert!(h.state().status.contains("half"), "names the layer: {}", h.state().status);
        }
        // Moves: the free layers go, a locked one only if it may.
        let expected = |d: (i64, i64)| if moves { d } else { (0, 0) };
        let (mut h, child, free, _) = setup();
        h.state_mut().auto_select = false;
        h.state_mut().set_tool(Tool::Transform);
        h.run_steps(1);
        h.key_press(Key::ArrowRight);
        h.run_steps(1);
        assert_eq!(doc(&h).layers[child].offset, expected((1, 0)), "arrow");
        assert_eq!(doc(&h).layers[free].offset, (6, 5), "arrow");

        // Center and Align place the group by the bounds of all it holds:
        // with a layer that would stay, it doesn't move at all.
        let (mut h, child, free, _) = setup();
        let n = steps(&h);
        h.state_mut().run(Action::CenterElement);
        let moved = doc(&h).layers[free].offset;
        assert_eq!(moved != (5, 5), moves, "center");
        let d = (moved.0 - 5, moved.1 - 5);
        assert_eq!(doc(&h).layers[child].offset, expected(d), "center");
        if !moves {
            assert_eq!(steps(&h), n);
            assert!(h.state().status.contains("half"), "names the layer: {}", h.state().status);
        }

        // The stack is narrower than the canvas to the right of half: align
        // on a selection so that something must move.
        let (mut h, child, free, _) = setup();
        h.state_mut().selection = Some(sel(150, 0, 50, 100));
        h.state_mut().run(Action::Align(AlignOp::Right));
        assert_eq!(doc(&h).layers[free].offset != (5, 5), moves, "align");
        let d = (doc(&h).layers[free].offset.0 - 5, doc(&h).layers[free].offset.1 - 5);
        assert_eq!(doc(&h).layers[child].offset, expected(d), "align");
    }
}

#[test]
fn a_group_whose_layers_all_stay_doesnt_pretend_to_move() {
    let mut h = app();
    let (child, free, _) = in_a_group(&mut h);
    doc_mut(&mut h).layers[child].locks.position = true;
    doc_mut(&mut h).layers[free].locks.position = true;
    let n = steps(&h);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(1);
    h.key_press(Key::ArrowRight);
    h.run_steps(1);
    h.state_mut().run(Action::CenterElement);
    h.state_mut().run(Action::Align(AlignOp::Left));
    assert_eq!(steps(&h), n);
    assert!(h.state().status.contains("half"), "{}", h.state().status);
}

#[test]
fn a_selection_of_groups_leaves_the_one_holding_a_locked_layer_out_of_a_transform() {
    let mut h = app();
    let (child, _, g) = in_a_group(&mut h);
    doc_mut(&mut h).layers[child].locks.pixels = true;
    doc_mut(&mut h).add_layer("over".into(), RgbaImage::from_pixel(20, 20, Rgba(RED)), (5, 5));
    let over = doc(&h).active;
    h.state_mut().layer_set = [doc(&h).layers[g].id, doc(&h).layers[over].id].into_iter().collect();
    assert_eq!(h.state().transform_targets(), vec![over]);
}

#[test]
fn lock_position_stops_the_path_tools_on_a_shape() {
    let mut h = app();
    doc_mut(&mut h).add_shape_layer("R".into(), "R", shape());
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.position = true;
    h.state_mut().set_tool(Tool::PathSelect);
    h.run_steps(2);
    let b = doc(&h).layers[a].content_bounds();
    drag(&mut h, (50.0, 45.0), (61.0, 52.0));
    assert_eq!(doc(&h).layers[a].content_bounds(), b);
    assert!(h.state().status.contains("R"), "{}", h.state().status);
    h.key_press(Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(doc(&h).layers[a].content_bounds(), b);
    // Unlocked, the same gesture moves it: the test does reach the tool.
    doc_mut(&mut h).layers[a].locks.position = false;
    drag(&mut h, (50.0, 45.0), (61.0, 52.0));
    assert_ne!(doc(&h).layers[a].content_bounds(), b);
}

#[test]
fn lock_position_stops_the_artboard_tool() {
    use crate::document::ArtboardBackground as B;
    for locked in [false, true] {
        let mut h = app();
        let d = doc_mut(&mut h);
        d.add_artboard("a".into(), "A".into(), PixelRect { x: 0, y: 0, w: 80, h: 60 }, B::Transparent);
        d.add_artboard("b".into(), "B".into(), PixelRect { x: 200, y: 0, w: 80, h: 60 }, B::Transparent);
        h.run_steps(3);
        let i = doc(&h).artboards()[0];
        doc_mut(&mut h).active = i;
        doc_mut(&mut h).layers[i].locks.position = locked;
        h.state_mut().set_tool(Tool::Artboard);
        h.run_steps(3);
        let id = doc(&h).layers[i].id;
        let rect = |h: &Harness<'static, AtelierApp>| {
            let at = doc(h).layers.iter().position(|l| l.id == id).unwrap();
            doc(h).layers[at].artboard_rect().unwrap()
        };
        let r = rect(&h);
        let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        drag(&mut h, (cx, cy), (cx + 20.0, cy + 15.0));
        assert_eq!(rect(&h) == r, locked, "move");
        // Its bottom right handle.
        let r = rect(&h);
        let corner = (r.x as f32 + r.w as f32, r.y as f32 + r.h as f32);
        drag(&mut h, corner, (corner.0 + 10.0, corner.1 + 10.0));
        assert_eq!(rect(&h) == r, locked, "resize");
    }
}

#[test]
fn merges_keep_every_lock() {
    let kinds = [
        Locks { transparent: true, ..Default::default() },
        Locks { pixels: true, ..Default::default() },
        Locks { position: true, ..Default::default() },
    ];
    let mk = |upper: Locks, lower: Locks| {
        let mut h = app();
        let lo = doc(&h).active;
        let image = RgbaImage::from_pixel(200, 100, Rgba([0, 0, 255, 255]));
        doc_mut(&mut h).add_layer("top".into(), image, (0, 0));
        let up = doc(&h).active;
        doc_mut(&mut h).layers[up].locks = upper;
        doc_mut(&mut h).layers[lo].locks = lower;
        h
    };
    for locks in kinds {
        for (upper, lower) in [(locks, Locks::default()), (Locks::default(), locks)] {
            for action in [Action::MergeDown, Action::MergeVisible] {
                let mut h = mk(upper, lower);
                let (n, count) = (steps(&h), doc(&h).layers.len());
                h.state_mut().run(action);
                assert_eq!((steps(&h), doc(&h).layers.len()), (n, count), "{locks:?}");
                assert!(!h.state().status.is_empty());
            }
        }
        // A group's merge too, and through what the group says.
        let mut h = app();
        let (child, _, g) = in_a_group(&mut h);
        doc_mut(&mut h).layers[child].locks = locks;
        assert!(doc_mut(&mut h).merge_group().is_err());
        doc_mut(&mut h).layers[child].locks = Locks::default();
        doc_mut(&mut h).layers[g].locks = locks;
        assert!(doc_mut(&mut h).merge_group().is_err());
    }
    let mut h = mk(Locks::default(), Locks::default());
    let count = doc(&h).layers.len();
    h.state_mut().run(Action::MergeDown);
    assert_eq!(doc(&h).layers.len(), count - 1);
}

#[test]
fn the_row_reads_every_group_that_holds_the_layer() {
    let mut h = app();
    let child = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[child]));
    let inner = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[inner]));
    let outer = doc(&h).active;
    let id = doc(&h).layers[child].id;
    let child = doc(&h).layers.iter().position(|l| l.id == id).unwrap();
    let inner = doc(&h).parent(child).unwrap();
    doc_mut(&mut h).layers[inner].locks.position = true;
    doc_mut(&mut h).layers[outer].locks.pixels = true;
    let d = doc(&h);
    assert_eq!(LockKind::Pixels.state(d, child), (true, false));
    assert_eq!(LockKind::Position.state(d, child), (true, false));
    assert_eq!(LockKind::Transparent.state(d, child), (false, true));
    doc_mut(&mut h).layers[outer].locks.pixels = false;
    doc_mut(&mut h).layers[outer].locked = true;
    let d = doc(&h);
    assert_eq!(LockKind::All.state(d, child), (true, false));
    assert_eq!(LockKind::Transparent.state(d, child), (true, false));
    assert!(d.is_locked(child));
}

#[test]
fn ctrl_slash_says_when_it_can_lock_nothing() {
    let mut h = app();
    h.state_mut().last_lock = LastLock::only(LockKind::Transparent);
    doc_mut(&mut h).add_shape_layer("R".into(), "R", shape());
    let n = steps(&h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run_steps(1);
    let l = doc(&h).active_layer();
    assert!(!l.locked && !l.locks.any());
    assert_eq!(steps(&h), n);
    assert!(h.state().status.contains(&t!("layers8-lock-transparent")));

    // Held by its group's Lock All: the group is named, nothing changes.
    let mut h = app();
    let child = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[child]));
    let g = doc(&h).active;
    doc_mut(&mut h).layers[g].name = "Outer".into();
    doc_mut(&mut h).layers[g].locked = true;
    let id = doc(&h).layers[child].id;
    let child = doc(&h).layers.iter().position(|l| l.id == id).unwrap();
    doc_mut(&mut h).active = child;
    let n = steps(&h);
    h.state_mut().run(Action::ToggleLock);
    assert!(h.state().status.contains("Outer"), "{}", h.state().status);
    assert!(!doc(&h).layers[child].locked);
    assert_eq!(steps(&h), n);
}

#[test]
fn convert_to_smart_object_keeps_lock_position() {
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks = Locks::ALL;
    h.state_mut().run(Action::Smart(smart_ui::Command::Convert));
    let d = doc(&h);
    assert!(d.active_layer().is_smart());
    assert!(d.active_layer().locks.position);
    assert!(!d.can_move(d.active));
    h.key_press(Key::ArrowRight);
    assert_eq!(doc(&h).active_layer().offset, (0, 0));
}

#[test]
fn a_groups_lock_image_pixels_reaches_text_shape_and_smart_content() {
    // Text.
    let mut h = app();
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(2);
    drag(&mut h, (20.0, 20.0), (20.0, 20.0));
    h.run_steps(2);
    h.event(egui::Event::Text("abc".into()));
    h.run_steps(3);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(3);
    let t = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[t]));
    let g = doc(&h).active;
    doc_mut(&mut h).layers[g].locks.pixels = true;
    let t = g - 1;
    h.state_mut().select_layer(t);
    h.state_mut().change_text(|d, _| d.text.push_str("zz"));
    h.run_steps(2);
    assert_eq!(doc(&h).layers[t].text.as_ref().unwrap().text, "abc");

    // Shape's path, by a real drag.
    let mut h = app();
    doc_mut(&mut h).add_shape_layer("R".into(), "R", shape());
    let a = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[a]));
    let g = doc(&h).active;
    doc_mut(&mut h).layers[g].locks.pixels = true;
    h.state_mut().select_layer(a);
    h.state_mut().set_tool(Tool::PathSelect);
    h.run_steps(2);
    let b = doc(&h).layers[a].content_bounds();
    drag(&mut h, (50.0, 45.0), (61.0, 52.0));
    assert_eq!(doc(&h).layers[a].content_bounds(), b);
    assert!(!h.state().status.is_empty());
}

#[test]
fn refusals_name_the_group_that_has_the_lock() {
    let mut h = app();
    let child = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[child]));
    let g = doc(&h).active;
    doc_mut(&mut h).layers[g].name = "Outer".into();
    doc_mut(&mut h).layers[g].locks.pixels = true;
    h.state_mut().select_layer(g - 1);
    h.state_mut().run(Action::Adjust(AdjustKind::Invert));
    let status = &h.state().status;
    let said = t!("locks9-status-group-pixels-locked", name = "half", group = "Outer");
    assert!(status.contains(&said), "{status:?}");
    assert!(!status.contains(&t!("layers8-status-pixels-locked", name = "half")));
}

#[test]
fn place_doesnt_land_in_a_group_that_wont_let_it_move() {
    let dir = std::env::temp_dir().join("atelier-locks-place");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("place.png");
    RgbaImage::from_pixel(30, 30, Rgba([0, 255, 0, 255])).save(&path).unwrap();
    let mut h = app();
    let child = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[child]));
    let g = doc(&h).active;
    doc_mut(&mut h).layers[g].locks.position = true;
    h.state_mut().select_layer(g - 1);
    let (count, n) = (doc(&h).layers.len(), steps(&h));
    h.state_mut().place_path(&path, false);
    assert_eq!((doc(&h).layers.len(), steps(&h)), (count, n));
    assert!(!h.state().transform_pending());
    // Over a layer with Lock Position of its own, it can be placed.
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.position = true;
    h.state_mut().place_path(&path, false);
    assert!(h.state().transform_pending());
}

#[test]
fn the_fill_dialog_shows_the_lock_keeps_the_opacity() {
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    h.state_mut().selection = Some(sel(10, 10, 20, 20));
    h.state_mut().run(Action::ClearSelection);
    h.run_steps(2);
    // Shown checked and disabled; the user's own choice is kept.
    assert!(!h.state().fill_dialog.unwrap().preserve_transparency);
    let label = t!("dialog-fill-preserve-transparency");
    let node = h.get_by_label(&label);
    assert!(node.accesskit_node().is_disabled());
    assert_eq!(node.accesskit_node().toggled(), Some(egui::accesskit::Toggled::True));
}

#[test]
fn a_cut_under_lock_transparent_pixels_is_a_cut_in_the_history() {
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    h.state_mut().selection = Some(sel(10, 10, 20, 20));
    h.state_mut().run(Action::Cut);
    h.run_steps(1);
    assert_eq!(doc(&h).history().0.last().copied(), Some(&*t!("menu-edit-cut")));
}

#[test]
fn stroke_path_with_the_eraser_paints_the_background_under_the_lock() {
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    h.state_mut().selection = Some(sel(10, 10, 60, 60));
    h.state_mut().run(Action::Path(paths_panel::PathCommand::MakeWorkPath, false));
    h.run_steps(1);
    h.state_mut().set_tool(Tool::Eraser);
    h.state_mut().secondary = Color32::from_rgb(0, 0, 255);
    (h.state_mut().brush_size, h.state_mut().hardness) = (10.0, 1.0);
    h.state_mut().vector.panel.stroke_with_eraser();
    let before = doc(&h).layers[a].image().clone();
    h.state_mut().run(Action::Path(paths_panel::PathCommand::StrokePath, false));
    h.run_steps(2);
    let after = doc(&h).layers[a].image();
    let changed = before.pixels().zip(after.pixels()).filter(|(x, y)| x != y).count();
    assert!(changed > 0, "the stroke shows");
    assert!(before.pixels().zip(after.pixels()).all(|(x, y)| x[3] == y[3]));
    assert!(after.pixels().any(|p| p[2] > 200 && p[3] == RED[3]), "the background color");
}

#[test]
fn show_transform_controls_is_disabled_when_it_would_do_nothing() {
    let mut h = app();
    let a = doc(&h).active;
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(2);
    let label = t!("tools4-show-transform-controls");
    assert!(!h.get_by_label(&label).accesskit_node().is_disabled());
    doc_mut(&mut h).layers[a].locks.pixels = true;
    h.run_steps(2);
    assert!(h.get_by_label(&label).accesskit_node().is_disabled());
}

#[test]
fn lock_all_looks_solid_and_a_partial_lock_hollow() {
    // Lit pixels of the glyph at the end of the layer's row.
    let lit = |locked: bool, locks: Locks, tip: String| {
        let mut h = app();
        let a = doc(&h).active;
        doc_mut(&mut h).layers[a].locked = locked;
        doc_mut(&mut h).layers[a].locks = locks;
        h.run_steps(2);
        let r = h.get_by_label(&tip).rect();
        let shot = h.render().unwrap();
        shot.enumerate_pixels()
            .filter(|&(x, y, p)| {
                let (x, y) = (x as f32, y as f32);
                r.contains(pos2(x, y)) && p[0] > 120
            })
            .count()
    };
    let partial = Locks { position: true, ..Default::default() };
    let all = lit(true, Locks::default(), t!("panel-unlock-tip"));
    let part = lit(false, partial, t!("layers8-partial-unlock-tip"));
    assert!(part > 0 && all > part + 10, "{all} {part}");
}

#[test]
fn content_aware_fill_keeps_the_opacity_under_lock_transparent_pixels() {
    use retouch_tools::{FillRequest, Output};
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    h.state_mut().selection = Some(sel(90, 20, 20, 20));
    let before = doc(&h).layers[a].image().clone();
    let mut settings = h.state().retouch.settings;
    (settings.ai, settings.output) = (false, Output::CurrentLayer);
    let n = steps(&h);
    h.state_mut().start_content_aware(FillRequest {
        settings,
        opacity: 1.0,
        keep_alpha: false,
        from_fill: false,
    });
    settle(&mut h);
    assert_eq!(steps(&h), n + 1);
    let after = doc(&h).layers[a].image();
    assert!(before.pixels().zip(after.pixels()).all(|(x, y)| x[3] == y[3]));
    assert_eq!(after.dimensions(), before.dimensions());
}

// ------------------------------------------------------------------ Actions

fn one_action(h: &mut Harness<'static, AtelierApp>, steps: Vec<crate::actions::Step>) {
    use crate::actions::{Sequence, Set};
    h.state_mut().actions.library.sets = vec![Set {
        name: "s".into(),
        actions: vec![Sequence {
            name: "a".into(),
            key: None,
            steps,
            expanded: true,
        }],
        expanded: true,
    }];
}

/// Plays `steps` as an action to its end; the error it stopped at.
fn play(h: &mut Harness<'static, AtelierApp>, commands: Vec<crate::actions::Recorded>) -> Option<String> {
    use crate::actions::Step;
    let steps = commands.into_iter().map(Step::new).collect();
    one_action(h, steps);
    h.state_mut().play_row(actions_panel::Row::Action(0, 0));
    for _ in 0..200 {
        h.run_steps(1);
        if let Some(text) = h.state().actions.error_shown() {
            return Some(text);
        }
        if !h.state().actions.playing() {
            return None;
        }
    }
    panic!("the action never ended (dialog open: {})", h.state().fill_dialog.is_some());
}

#[test]
fn an_action_plays_what_the_locks_allow_like_the_command() {
    use crate::actions::{MenuItem, Recorded};
    let menu = |item| Recorded::Menu { item };
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.pixels = true;
    let r = play(&mut h, vec![menu(MenuItem::RevealAllMask), menu(MenuItem::ClippingMask)]);
    assert_eq!(r, None);
    assert!(doc(&h).layers[a].mask.is_some() && doc(&h).layers[a].clipped);
    // What the command refuses stops the action.
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.pixels = true;
    let image = doc(&h).layers[a].image().clone();
    doc_mut(&mut h).add_mask(NewMask::HideAll, None);
    let r = play(&mut h, vec![menu(MenuItem::ApplyMask)]);
    assert!(r.is_some(), "stopped");
    assert_eq!(*doc(&h).layers[a].image(), image);
}

#[test]
fn clear_in_an_action_under_the_lock_fails_instead_of_asking() {
    use crate::actions::{MenuItem, Recorded};
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    h.state_mut().selection = Some(sel(10, 10, 20, 20));
    let r = play(&mut h, vec![Recorded::Menu { item: MenuItem::Clear }]);
    assert!(r.is_some());
    assert!(h.state().fill_dialog.is_none());
}

#[test]
fn locks_are_recorded_as_what_they_turn_to() {
    use crate::actions::Recorded;
    let mut h = app();
    one_action(&mut h, Vec::new());
    h.state_mut().actions.start_recording((0, 0));
    h.state_mut().run(Action::Lock(LockKind::Pixels));
    h.state_mut().run(Action::Lock(LockKind::Pixels));
    h.state_mut().run(Action::ToggleLock);
    let steps: Vec<Recorded> = h.state().actions.library.sets[0].actions[0]
        .steps
        .iter()
        .map(|s| s.command.clone())
        .collect();
    assert_eq!(steps.len(), 3, "{steps:?}");
    // The file reads back.
    let json = serde_json::to_string(&steps).unwrap();
    let back: Vec<Recorded> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, steps);

    // Played on a layer already in that state: the same end, no toggling.
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.pixels = true;
    let on = |pixels: bool| Recorded::Locks {
        transparent: false,
        pixels,
        position: false,
        all: false,
        on: true,
    };
    let n = doc(&h).history().1;
    assert_eq!(play(&mut h, vec![on(true)]), None);
    assert!(doc(&h).layers[a].locks.pixels);
    assert_eq!(doc(&h).history().1, n);
    // Unavailable on the layer: the step fails.
    let mut h = app();
    doc_mut(&mut h).add_shape_layer("R".into(), "R", shape());
    assert!(play(&mut h, vec![on(true)]).is_some());
}

#[test]
fn unlocking_from_the_row_is_recorded_for_the_active_layer() {
    use crate::actions::Recorded;
    let mut h = app();
    one_action(&mut h, Vec::new());
    h.state_mut().actions.start_recording((0, 0));
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.position = true;
    h.state_mut().unlock_layer(a);
    let steps = &h.state().actions.library.sets[0].actions[0].steps;
    assert!(matches!(steps[0].command, Recorded::Locks { on: false, .. }));
}

/// Waits for the filters, the retouch and the transforms in the background.
fn settle_all(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..1000 {
        h.run_steps(1);
        let app = h.state();
        if !app.filters.busy() && !app.retouch.busy() && !app.transform_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("still busy");
}

/// A text "abc" typed at (20, 20), put in a group under Lock Image Pixels:
/// (text, group).
fn text_in_a_pixel_locked_group(h: &mut Harness<'static, AtelierApp>) -> (usize, usize) {
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(2);
    drag(h, (20.0, 20.0), (20.0, 20.0));
    h.run_steps(2);
    h.event(egui::Event::Text("abc".into()));
    h.run_steps(3);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(3);
    let t = doc(h).active;
    assert!(doc_mut(h).group_layers(&[t]));
    let g = doc(h).active;
    doc_mut(h).layers[g].name = "Locked".into();
    doc_mut(h).layers[g].locks.pixels = true;
    h.state_mut().select_layer(g - 1);
    (g - 1, g)
}

fn texts(h: &Harness<'static, AtelierApp>) -> Vec<String> {
    let d = doc(h);
    d.layers.iter().filter_map(|l| l.text.as_ref().map(|t| t.text.clone())).collect()
}

#[test]
fn reset_transform_follows_lock_position() {
    let mut h = app();
    h.state_mut().run(Action::Smart(smart_ui::Command::Convert));
    settle_all(&mut h);
    let half = [[0.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]];
    assert!(h.state_mut().play_step(&crate::actions::Recorded::Transform { corners: half }, false).is_ok());
    settle_all(&mut h);
    let a = doc(&h).active;
    let b = doc(&h).layers[a].content_bounds();
    doc_mut(&mut h).layers[a].locks.position = true;
    let n = steps(&h);
    h.state_mut().run(Action::Smart(smart_ui::Command::ResetTransform));
    settle_all(&mut h);
    assert_eq!((doc(&h).layers[a].content_bounds(), steps(&h)), (b, n));
    assert!(h.state().status.contains("half"), "{}", h.state().status);
    // Unlocked, it resets.
    doc_mut(&mut h).layers[a].locks.position = false;
    h.state_mut().run(Action::Smart(smart_ui::Command::ResetTransform));
    settle_all(&mut h);
    assert_ne!(doc(&h).layers[a].content_bounds(), b);
}

#[test]
fn new_smart_object_via_copy_works_on_a_locked_one_as_duplicate_does() {
    let mut h = app();
    h.state_mut().run(Action::Smart(smart_ui::Command::Convert));
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locked = true;
    let n = doc(&h).layers.len();
    h.state_mut().run(Action::Smart(smart_ui::Command::NewViaCopy));
    assert_eq!(doc(&h).layers.len(), n + 1);
    assert!(doc(&h).layers[a].locked, "the original stays locked");
}

#[test]
fn both_converts_keep_the_pixel_locks_inside_and_rasterize_brings_them_back() {
    for cmd in [smart_ui::Command::Convert, smart_ui::Command::ConvertForFilters] {
        let mut h = app();
        let a = doc(&h).active;
        let locks = Locks { pixels: true, transparent: true, position: false };
        doc_mut(&mut h).layers[a].locks = locks;
        let n = steps(&h);
        h.state_mut().run(Action::Smart(cmd));
        settle_all(&mut h);
        assert!(doc(&h).active_layer().is_smart(), "{cmd:?}: {}", h.state().status);
        assert_eq!(steps(&h), n + 1);
        h.state_mut().run(Action::Rasterize);
        let l = doc(&h).active_layer();
        assert!(!l.is_smart(), "{cmd:?}");
        assert_eq!(l.locks, locks, "{cmd:?}");
        assert!(doc(&h).pixels_locked());
    }
    // Under Lock All: refused, the layer named.
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locked = true;
    h.state_mut().run(Action::Smart(smart_ui::Command::ConvertForFilters));
    assert!(!doc(&h).active_layer().is_smart());
    assert!(h.state().status.contains("half"), "{}", h.state().status);
}

#[test]
fn the_type_tool_starts_nothing_on_a_locked_text_or_in_a_locked_group() {
    let mut h = app();
    let (t, _) = text_in_a_pixel_locked_group(&mut h);
    let (n, count) = (steps(&h), doc(&h).layers.len());
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(2);
    // On the text: not typed in, no other text over it.
    let r = doc(&h).layers[t].rect();
    let (x, y) = (r.x as f32 + 3.0, r.y as f32 + r.h as f32 / 2.0);
    drag(&mut h, (x, y), (x, y));
    h.event(egui::Event::Text("zz".into()));
    h.run_steps(3);
    assert_eq!((steps(&h), doc(&h).layers.len()), (n, count));
    assert_eq!(texts(&h), ["abc"]);
    assert!(h.state().status.contains("Locked"), "{}", h.state().status);
    // Elsewhere, the new text would land in the group: refused too.
    h.state_mut().status.clear();
    drag(&mut h, (150.0, 60.0), (150.0, 60.0));
    h.event(egui::Event::Text("zz".into()));
    h.run_steps(3);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(3);
    assert_eq!((steps(&h), doc(&h).layers.len()), (n, count));
    assert!(h.state().status.contains("Locked"), "{}", h.state().status);

    // Unlocked, the same clicks type.
    let mut h = app();
    let (t, g) = text_in_a_pixel_locked_group(&mut h);
    doc_mut(&mut h).layers[g].locks.pixels = false;
    h.state_mut().set_tool(Tool::Text);
    h.run_steps(2);
    let r = doc(&h).layers[t].rect();
    let (x, y) = (r.x as f32 + 3.0, r.y as f32 + r.h as f32 / 2.0);
    drag(&mut h, (x, y), (x, y));
    h.event(egui::Event::Text("zz".into()));
    h.run_steps(3);
    h.state_mut().set_tool(Tool::Transform);
    h.run_steps(3);
    assert_eq!(texts(&h).len(), 1);
    assert_ne!(texts(&h), ["abc"]);
}

#[test]
fn text_options_on_a_locked_text_say_why_nothing_changes() {
    let mut h = app();
    let (t, _) = text_in_a_pixel_locked_group(&mut h);
    h.state_mut().status.clear();
    h.state_mut().change_text(|d, _| d.text.push_str("zz"));
    assert_eq!(doc(&h).layers[t].text.as_ref().unwrap().text, "abc");
    assert!(h.state().status.contains("Locked"), "{}", h.state().status);
}

#[test]
fn the_shape_tool_draws_no_layer_in_a_pixel_locked_group() {
    for locked in [true, false] {
        let mut h = app();
        let a = doc(&h).active;
        assert!(doc_mut(&mut h).group_layers(&[a]));
        let g = doc(&h).active;
        doc_mut(&mut h).layers[g].name = "Locked".into();
        doc_mut(&mut h).layers[g].locks.pixels = locked;
        h.state_mut().select_layer(a);
        let count = doc(&h).layers.len();
        let tool = Tool::Shape(vector_tools::ShapeTool::Rectangle);
        h.state_mut().set_tool(tool);
        h.run_steps(2);
        drag(&mut h, (20.0, 20.0), (60.0, 60.0));
        assert_eq!(doc(&h).layers.len() == count, locked);
        if locked {
            assert!(h.state().status.contains("Locked"), "{}", h.state().status);
        }
    }
}

#[test]
fn an_artboard_with_a_lock_position_layer_doesnt_move() {
    use crate::document::ArtboardBackground as B;
    for with_free in [false, true] {
        let mut h = app();
        let d = doc_mut(&mut h);
        d.add_artboard("a".into(), "A".into(), PixelRect { x: 0, y: 0, w: 80, h: 60 }, B::Transparent);
        let b = d.artboards()[0];
        d.active = b;
        let board = d.layers[b].id;
        d.add_layer("kid".into(), RgbaImage::from_pixel(20, 20, Rgba(RED)), (10, 10));
        let kid = d.active;
        d.layers[kid].locks.position = true;
        let kid = d.layers[kid].id;
        if with_free {
            d.add_layer("free".into(), RgbaImage::from_pixel(10, 10, Rgba(RED)), (50, 40));
        }
        d.active = d.layers.iter().position(|l| l.id == board).unwrap();
        h.run_steps(3);
        h.state_mut().set_tool(Tool::Artboard);
        h.run_steps(3);
        let find = |h: &Harness<'static, AtelierApp>, id: u64| {
            let d = doc(h);
            d.layers.iter().position(|l| l.id == id).unwrap()
        };
        let r = doc(&h).layers[find(&h, board)].artboard_rect().unwrap();
        let k = doc(&h).layers[find(&h, kid)].offset;
        let n = steps(&h);
        let (cx, cy) = (r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0);
        drag(&mut h, (cx, cy), (cx + 100.0, cy));
        assert_eq!(doc(&h).layers[find(&h, board)].artboard_rect(), Some(r), "{with_free}");
        assert_eq!(doc(&h).layers[find(&h, kid)].offset, k, "{with_free}");
        assert_eq!(steps(&h), n);
        assert!(h.state().status.contains("kid"), "{}", h.state().status);
        // Unlocked, the same drag moves it.
        let i = find(&h, kid);
        doc_mut(&mut h).layers[i].locks.position = false;
        drag(&mut h, (cx, cy), (cx + 100.0, cy));
        assert_ne!(steps(&h), n, "{with_free}");
    }
}

#[test]
fn a_refused_merge_names_the_locked_layer() {
    let mut h = app();
    let lower = doc(&h).active;
    doc_mut(&mut h).add_layer("top".into(), RgbaImage::from_pixel(20, 20, Rgba(RED)), (5, 5));
    doc_mut(&mut h).layers[lower].locks.position = true;
    h.state_mut().run(Action::MergeDown);
    let status = h.state().status.to_string();
    assert!(status.contains("half") && status.contains(icon::LOCK_SIMPLE), "{status}");
    h.state_mut().run(Action::MergeVisible);
    assert!(h.state().status.contains("half"), "{}", h.state().status);
}

#[test]
fn edit_contents_cant_update_a_locked_smart_object() {
    let mut h = app();
    h.state_mut().run(Action::Smart(smart_ui::Command::Convert));
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locked = true;
    let before = doc(&h).flatten().get_pixel(150, 50).0;
    h.state_mut().run(Action::Smart(smart_ui::Command::EditContents));
    h.run_steps(2);
    assert_eq!(h.state().tab_count(), 2);
    h.state_mut().primary = Color32::from_rgb(255, 0, 0);
    h.state_mut().run(Action::FillPrimary);
    h.state_mut().run(Action::Save);
    h.run_steps(2);
    // The edits stay in their tab, unsaved; the reason given.
    assert!(doc(&h).modified);
    assert!(h.state().status.contains("half"), "{}", h.state().status);
    h.state_mut().switch_tab(0);
    h.run_steps(2);
    assert_eq!(doc(&h).flatten().get_pixel(150, 50).0, before);
}

#[test]
fn a_fill_under_lock_transparent_records_the_users_choice() {
    use crate::actions::{Recorded, Sequence, Set};
    let mut h = app();
    let a = doc(&h).active;
    doc_mut(&mut h).layers[a].locks.transparent = true;
    let action = Sequence { name: "a".into(), key: None, steps: Vec::new(), expanded: true };
    h.state_mut().actions.library.sets = vec![Set { name: "s".into(), actions: vec![action], expanded: true }];
    h.state_mut().actions.start_recording((0, 0));
    h.state_mut().apply_fill(FillDialog {
        content: FillContent::Black,
        color: Color32::BLACK,
        opacity: 1.0,
        preserve_transparency: false,
        adapt: true,
        pattern: 0,
    });
    // The opacity kept all the same.
    assert_eq!(pixel(&h, 150, 50)[3], 0);
    assert_eq!(pixel(&h, 50, 50), [0, 0, 0, 128]);
    let steps = &h.state().actions.library.sets[0].actions[0].steps;
    assert!(
        matches!(steps[..], [ref s] if matches!(s.command, Recorded::Fill { preserve_transparency: false, .. })),
        "{steps:?}"
    );
}

#[test]
fn ctrl_slash_names_the_group_that_already_has_the_lock() {
    let mut h = app();
    let child = doc(&h).active;
    assert!(doc_mut(&mut h).group_layers(&[child]));
    let g = doc(&h).active;
    doc_mut(&mut h).layers[g].name = "Outer".into();
    doc_mut(&mut h).layers[g].locks.position = true;
    h.state_mut().select_layer(g - 1);
    h.state_mut().last_lock = LastLock::only(LockKind::Position);
    let n = steps(&h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run_steps(1);
    assert_eq!(steps(&h), n);
    let status = h.state().status.to_string();
    assert!(status.contains("Outer") && status.contains(&t!("layers8-lock-position")), "{status}");
}
