//! Patterns through the app: Define Pattern…, Fill…, Pattern Fill layers,
//! Pattern Overlay, files.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use image::{Rgba, RgbaImage};

use super::*;
use crate::compose::pattern::PatternFill;
use crate::document::{Document, Fill, LayerKind, PixelRect};

const WHITE: [u8; 4] = [255; 4];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// 40 × 30 white canvas.
fn app() -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
            app.doc = Some(doc);
            app
        });
    h.run_steps(2);
    h
}

/// 2 × 1: red then blue.
fn tile() -> Arc<Pattern> {
    let mut image = RgbaImage::new(2, 1);
    image.put_pixel(0, 0, Rgba(RED));
    image.put_pixel(1, 0, Rgba(BLUE));
    Pattern::new("Tile", image)
}

fn with_tile(h: &mut Harness<'static, AtelierApp>) -> u64 {
    let p = tile();
    let id = p.id;
    h.state_mut().patterns.library.add(p);
    h.state_mut().patterns.last = id;
    id
}

fn flat(h: &Harness<'static, AtelierApp>) -> RgbaImage {
    h.state().doc.as_ref().unwrap().flatten()
}

fn fill_dialog(pattern: u64) -> FillDialog {
    FillDialog {
        content: FillContent::Pattern,
        color: Color32::BLACK,
        opacity: 1.0,
        preserve_transparency: false,
        adapt: true,
        pattern,
    }
}

/// Runs frames until `done`, the workers having their time.
fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in crate::testing::patience() {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never done");
}

#[test]
fn define_pattern_takes_the_visible_pixels_of_the_selection() {
    let mut h = app();
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    doc.add_layer("red".into(), RgbaImage::from_pixel(4, 3, Rgba(RED)), (2, 1));
    let steps = doc.history().1;
    let count = app.patterns.library.items.len();
    app.selection = Some(
        PixelRect {
            x: 4,
            y: 2,
            w: 6,
            h: 5,
        }
        .into(),
    );
    app.run(Action::DefinePattern);
    settle(&mut h, |a| {
        a.patterns.define.as_ref().is_some_and(|d| d.image_ready())
    });
    let name = t!("pattern8-default-name", number = 1);
    assert_eq!(h.state().patterns.define.as_ref().unwrap().name(), name);
    // The window's OK.
    h.key_press(Key::Enter);
    h.run_steps(3);
    let app = h.state();
    assert!(app.patterns.define.is_none());
    assert_eq!(app.patterns.library.items.len(), count + 1);
    let p = app.patterns.library.items.last().unwrap();
    assert_eq!(p.name, name);
    assert_eq!(p.image.dimensions(), (6, 5));
    assert_eq!(p.image.get_pixel(0, 0).0, RED, "the red layer at (4, 2)");
    assert_eq!(p.image.get_pixel(5, 4).0, WHITE, "the background beyond it");
    assert_eq!(app.patterns.last, p.id);
    // Not a step, nothing changes in the document.
    assert_eq!(app.doc.as_ref().unwrap().history().1, steps);
}

#[test]
fn define_pattern_is_refused_with_a_reason() {
    let mut h = app();
    let app = h.state_mut();
    let canvas = app.doc.as_ref().unwrap().full_rect();
    let ellipse = PixelRect {
        x: 2,
        y: 2,
        w: 9,
        h: 9,
    };
    app.selection = crate::selection::Selection::ellipse(ellipse, canvas, true);
    assert!(app.selection.is_some());
    let why = app.define_pattern_refusal().unwrap();
    assert_eq!(why, t!("pattern8-why-rectangle"));
    assert_eq!(app.why_unavailable(Action::DefinePattern), why);
    app.run(Action::DefinePattern);
    assert!(app.patterns.define.is_none());
    assert_eq!(app.status, why);
    app.selection = None;
    assert!(app.define_pattern_refusal().is_none(), "the canvas will do");
    // Without a document.
    app.doc = None;
    assert!(app.define_pattern_refusal().is_some());
}

#[test]
fn a_fully_transparent_area_defines_nothing() {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(RgbaImage::new(8, 8), None));
            app
        });
    let count = h.state().patterns.library.items.len();
    h.state_mut().run(Action::DefinePattern);
    settle(&mut h, |a| a.patterns.define.is_none());
    assert_eq!(h.state().status, t!("pattern8-status-empty"));
    assert_eq!(h.state().patterns.library.items.len(), count);
}

#[test]
fn fill_with_a_pattern_repeats_from_the_canvas_origin_in_one_step() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    let before = app.doc.as_ref().unwrap().history().1;
    app.selection = Some(
        PixelRect {
            x: 3,
            y: 2,
            w: 3,
            h: 2,
        }
        .into(),
    );
    app.apply_fill(fill_dialog(id));
    let doc = app.doc.as_ref().unwrap();
    assert_eq!(doc.history().1, before + 1);
    let image = doc.flatten();
    assert_eq!(image.get_pixel(3, 2).0, BLUE, "x = 3: second tile pixel");
    assert_eq!(image.get_pixel(4, 2).0, RED);
    assert_eq!(image.get_pixel(5, 3).0, BLUE);
    assert_eq!(image.get_pixel(6, 2).0, WHITE, "outside the selection");
    app.undo();
    assert_eq!(flat(&h).get_pixel(4, 2).0, WHITE);
}

#[test]
fn fill_keeps_transparency_and_opacity_when_asked() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    let mut image = RgbaImage::new(40, 30);
    image.put_pixel(0, 0, Rgba([0, 255, 0, 128]));
    doc.add_layer("part".into(), image, (0, 0));
    app.apply_fill(FillDialog {
        preserve_transparency: true,
        ..fill_dialog(id)
    });
    let l = app.doc.as_ref().unwrap().active_layer();
    let on = l.image().get_pixel(0, 0).0;
    assert_eq!(on, [255, 0, 0, 128], "painted, its alpha kept");
    assert_eq!(l.image().get_pixel(1, 0).0[3], 0, "empty stays empty");
    app.undo();
    app.apply_fill(FillDialog {
        opacity: 0.5,
        ..fill_dialog(id)
    });
    let l = app.doc.as_ref().unwrap().active_layer();
    assert!((l.image().get_pixel(2, 5).0[3] as i32 - 128).abs() <= 1);
}

#[test]
fn fill_goes_to_a_targeted_mask_and_is_refused_on_locked_and_hidden_layers() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    doc.add_layer(
        "top".into(),
        RgbaImage::from_pixel(40, 30, Rgba(BLUE)),
        (0, 0),
    );
    assert!(doc.add_mask(crate::document::NewMask::RevealAll, None));
    let top = doc.active_layer().image().clone();
    app.apply_fill(fill_dialog(id));
    let doc = app.doc.as_ref().unwrap();
    assert_eq!(*doc.active_layer().image(), top, "the mask took it");
    let mask = doc.active_layer().mask.as_ref().unwrap();
    assert_ne!(mask.image().get_pixel(0, 0).0, [255], "the tile's grays");
    let doc = app.doc.as_mut().unwrap();
    doc.target_mask(false);
    doc.active_layer_mut().locked = true;
    let steps = doc.history().1;
    app.apply_fill(fill_dialog(id));
    assert_eq!(app.doc.as_ref().unwrap().history().1, steps, "locked");
    let doc = app.doc.as_mut().unwrap();
    let l = doc.active_layer_mut();
    (l.locked, l.visible) = (false, false);
    app.apply_fill(fill_dialog(id));
    assert_eq!(app.doc.as_ref().unwrap().history().1, steps, "hidden");
}

#[test]
fn a_recorded_pattern_fill_plays_back_or_says_it_is_missing() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    app.apply_fill(fill_dialog(id));
    app.undo();
    let step = crate::actions::Recorded::Fill {
        contents: crate::actions::Contents::Pattern,
        color: [0; 3],
        opacity: 1.0,
        preserve_transparency: false,
        pattern: Some(id),
    };
    let json = serde_json::to_string(&step).unwrap();
    let back: crate::actions::Recorded = serde_json::from_str(&json).unwrap();
    assert_eq!(back, step);
    // Files from before patterns still read.
    let mut old = serde_json::to_value(&step).unwrap();
    let removed = old.as_object_mut().and_then(|o| {
        o.values_mut()
            .find_map(|v| v.as_object_mut().and_then(|f| f.remove("pattern")))
            .or_else(|| o.remove("pattern"))
    });
    assert!(removed.is_some());
    assert!(serde_json::from_value::<crate::actions::Recorded>(old).is_ok());
    // Played: the same fill.
    let app = h.state_mut();
    app.play_step(&step, false).unwrap();
    assert_eq!(flat(&h).get_pixel(0, 0).0, RED);
    assert_eq!(flat(&h).get_pixel(1, 0).0, BLUE);
    // That pattern gone: the step says so.
    let app = h.state_mut();
    app.patterns.library.remove(id);
    assert_eq!(app.play_step(&step, false), Err(t!("pattern8-why-missing")));
}

#[test]
fn a_pattern_fill_layer_lays_the_tile_and_follows_the_layer_when_linked() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    let before = app.doc.as_ref().unwrap().history().1;
    app.run(Action::NewPatternFill);
    let doc = app.doc.as_mut().unwrap();
    assert_eq!(doc.history().1, before + 1);
    assert_eq!(doc.layers.len(), 2);
    assert_eq!(
        doc.active_layer().name,
        t!("pattern8-fill-name", number = 1)
    );
    let LayerKind::Fill(Fill::Pattern(f)) = &doc.active_layer().kind else {
        panic!("{:?}", doc.active_layer().kind);
    };
    assert_eq!(f.pattern.id, id);
    assert!(f.linked && f.scale == 1.0, "the defaults");
    let image = doc.flatten();
    assert_eq!(image.get_pixel(0, 0).0, RED);
    assert_eq!(image.get_pixel(1, 9).0, BLUE);
    // Linked: moved by the layer's place; not linked: at the canvas origin.
    doc.active_layer_mut().translate(1, 0);
    assert_eq!(doc.flatten().get_pixel(0, 0).0, BLUE);
    if let LayerKind::Fill(Fill::Pattern(f)) = &mut doc.active_layer_mut().kind {
        f.linked = false;
    }
    assert_eq!(doc.flatten().get_pixel(0, 0).0, RED);
    // Twice the size.
    if let LayerKind::Fill(Fill::Pattern(f)) = &mut doc.active_layer_mut().kind {
        f.scale = 2.0;
    }
    doc.active_layer_mut().name = "double".into();
    let image = doc.flatten();
    let (r, b) = (image.get_pixel(0, 0).0, image.get_pixel(2, 0).0);
    assert!(r[0] > r[2] && b[2] > b[0], "{r:?} {b:?}");
    // Its name in the panel and its look at a reduced view don't fail.
    h.run_steps(2);
    // The Move tool moves it, the tile with it.
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    if let LayerKind::Fill(Fill::Pattern(f)) = &mut doc.active_layer_mut().kind {
        (f.scale, f.linked) = (1.0, true);
    }
    doc.active_layer_mut().offset = (0, 0);
    app.tool = Tool::Transform;
    app.nudge((1, 0));
    h.run_steps(2);
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(doc.active_layer().offset, (1, 0));
    assert_eq!(doc.flatten().get_pixel(0, 0).0, BLUE);
}

#[test]
fn image_size_scales_what_is_laid_with_it() {
    let mut h = app();
    with_tile(&mut h);
    let app = h.state_mut();
    app.run(Action::NewPatternFill);
    let doc = app.doc.as_mut().unwrap();
    doc.active_layer_mut().effects.pattern_overlay.fill.scale = 1.5;
    doc.resize_image(80, 60, image::imageops::FilterType::Triangle);
    let l = doc.active_layer();
    let LayerKind::Fill(Fill::Pattern(f)) = &l.kind else {
        panic!()
    };
    assert_eq!(f.scale, 2.0);
    assert_eq!(l.effects.pattern_overlay.fill.scale, 3.0);
}

#[test]
fn pattern_overlay_covers_the_shape_only_with_its_blend_and_opacity() {
    let mut h = app();
    with_tile(&mut h);
    let tile = h.state().last_pattern();
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    let mut img = RgbaImage::new(40, 30);
    for x in 4..8 {
        img.put_pixel(x, 4, Rgba([0, 255, 0, 255]));
    }
    doc.add_layer("shape".into(), img, (0, 0));
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(tile);
    assert!(fx.any());
    let image = doc.flatten();
    assert_eq!(image.get_pixel(4, 4).0, RED, "from the content's corner");
    assert_eq!(image.get_pixel(5, 4).0, BLUE);
    assert_eq!(image.get_pixel(3, 4).0, WHITE, "outside the shape");
    // Half opaque over the green; not linked: from the canvas origin.
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.opacity = 0.5;
    fx.pattern_overlay.fill.linked = false;
    let [r, g, b, _] = doc.flatten().get_pixel(5, 4).0;
    assert!(r < 5 && g > 120 && b > 120, "x = 5: blue, {r} {g} {b}");
}

#[test]
fn the_layer_style_page_and_the_picker_show_and_choose() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    let doc = app.doc.as_mut().unwrap();
    doc.add_layer(
        "top".into(),
        RgbaImage::from_pixel(40, 30, Rgba(BLUE)),
        (0, 0),
    );
    app.run(Action::LayerStyle(Some(Style::PatternOverlay)));
    h.run_steps(3);
    assert!(h.state().layer_style.is_some());
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.active_layer_mut().effects.pattern_overlay.enabled = true;
    h.run_steps(2);
    h.get_by_label(&t!("pattern8-pattern")).click();
    h.run_steps(3);
    h.get_by_label("Tile").click();
    h.run_steps(3);
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(
        doc.active_layer().effects.pattern_overlay.fill.pattern.id,
        id
    );
    assert_eq!(doc.flatten().get_pixel(0, 0).0, RED);
}

/// Right click at the center of `rect`, with the pointer's real events.
fn right_click(h: &mut Harness<'static, AtelierApp>, rect: egui::Rect) {
    let pos = rect.center();
    h.event(egui::Event::PointerMoved(pos));
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: Default::default(),
        });
        h.run_steps(2);
    }
    h.run_steps(2);
}

/// The picker of Fill… opened, with real clicks.
fn open_picker(h: &mut Harness<'static, AtelierApp>, id: u64) {
    h.state_mut().fill_dialog = Some(fill_dialog(id));
    h.run_steps(3);
    h.get_by_label(&t!("pattern8-pattern")).click();
    h.run_steps(3);
}

#[test]
fn rename_and_delete_are_reached_by_a_right_click_in_the_picker() {
    let mut h = app();
    let id = with_tile(&mut h);
    let count = h.state().patterns.library.items.len();
    open_picker(&mut h, id);
    let rect = h.get_by_label("Tile").rect();
    right_click(&mut h, rect);
    h.get_by_label(&t!("pattern8-rename")).click();
    h.run_steps(4);
    assert_eq!(h.state().patterns.rename.as_ref().unwrap().name(), "Tile");
    h.state_mut().patterns.rename = None;
    h.run_steps(3);

    // Delete asks first; Cancel keeps it.
    if h.query_by_label("Tile").is_none() {
        h.get_by_label(&t!("pattern8-pattern")).click();
        h.run_steps(3);
    }
    let rect = h.get_by_label("Tile").rect();
    right_click(&mut h, rect);
    h.get_by_label(&t!("pattern8-delete")).click();
    h.run_steps(4);
    assert!(h.state().patterns.delete.is_some());
    assert_eq!(h.state().patterns.library.items.len(), count);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().patterns.delete.is_none());
    assert_eq!(h.state().patterns.library.items.len(), count);

    h.state_mut().patterns.delete = Some(DeleteDialog {
        id,
        name: "Tile".into(),
    });
    h.run_steps(2);
    h.get_by_label(&t!("pattern9-delete-ok")).click();
    h.run_steps(3);
    assert!(h.state().patterns.delete.is_none());
    assert_eq!(h.state().patterns.library.items.len(), count - 1);
}

#[test]
fn a_left_click_on_a_tile_chooses_it_and_closes_the_picker() {
    let mut h = app();
    let id = with_tile(&mut h);
    let first = h.state().patterns.library.items[0].clone();
    open_picker(&mut h, first.id);
    h.get_by_label("Tile").click();
    h.run_steps(4);
    assert_eq!(h.state().fill_dialog.as_ref().unwrap().pattern, id);
    assert!(h.query_by_label("Tile").is_none(), "closed");
}

#[test]
fn a_built_in_pattern_is_shown_in_the_current_language() {
    let mut h = app();
    let id = h.state().patterns.library.items[0].id;
    crate::i18n::set_lang(crate::i18n::Lang::Fr);
    let french = t!("pattern8-builtin-checker");
    h.state_mut().fill_dialog = Some(fill_dialog(id));
    h.run_steps(3);
    let found = h.query_by_value(&french).is_some();
    let name = h.state().patterns.library.items[0].name.clone();
    crate::i18n::set_lang(crate::i18n::Lang::En);
    assert_eq!(name, french);
    assert!(found, "{french}");
}

#[test]
fn define_with_a_selection_off_the_canvas_is_refused() {
    let mut h = app();
    let app = h.state_mut();
    let w = app.doc.as_ref().unwrap().width();
    app.selection = Some(
        PixelRect {
            x: w as i64 + 100,
            y: 0,
            w: 10,
            h: 10,
        }
        .into(),
    );
    assert_eq!(
        app.define_pattern_refusal(),
        Some(t!("pattern8-status-empty"))
    );
    app.run(Action::DefinePattern);
    assert!(app.patterns.define.is_none());
}

#[test]
fn defining_a_tile_already_there_says_so() {
    let mut h = app();
    let id = with_tile(&mut h);
    let count = h.state().patterns.library.items.len();
    h.state_mut().define_tile(&tile(), "Other");
    let app = h.state();
    assert_eq!(app.status, t!("pattern9-status-already", name = "Tile"));
    assert_eq!(app.patterns.library.items.len(), count);
    assert_eq!(app.patterns.library.get(id).unwrap().name, "Tile");
    // A built-in as well.
    let builtin = app.patterns.library.items[0].clone();
    h.state_mut().define_tile(&builtin, "Mine");
    assert_eq!(h.state().patterns.library.items.len(), count);
    assert_eq!(
        h.state().status,
        t!("pattern9-status-already", name = builtin.name.as_str())
    );
}

#[test]
fn define_and_new_pattern_fill_are_recorded_and_played() {
    let mut h = app();
    let id = with_tile(&mut h);
    let app = h.state_mut();
    assert!(matches!(
        app.recorded_action(Action::NewPatternFill),
        Some(crate::actions::Recorded::NewPatternFill { pattern }) if pattern == id
    ));
    let before = app.doc.as_ref().unwrap().layers.len();
    let step = crate::actions::Recorded::NewPatternFill { pattern: id };
    app.play_step(&step, false).unwrap();
    assert_eq!(app.doc.as_ref().unwrap().layers.len(), before + 1);
    app.patterns.library.remove(id);
    assert_eq!(app.play_step(&step, false), Err(t!("pattern8-why-missing")));

    // Without its dialog: under the recorded name, when the tile is ready.
    let count = app.patterns.library.items.len();
    let step = crate::actions::Recorded::DefinePattern {
        name: "Played".into(),
    };
    app.play_step(&step, false).unwrap();
    settle(&mut h, |a| a.patterns.define.is_none());
    let app = h.state();
    assert_eq!(app.patterns.library.items.len(), count + 1);
    assert_eq!(app.patterns.library.items.last().unwrap().name, "Played");
}

#[test]
fn the_offset_fields_set_the_phase_and_snap_brings_it_back() {
    let mut h = app();
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.add_layer(
        "x".into(),
        RgbaImage::from_pixel(40, 30, Rgba(BLUE)),
        (0, 0),
    );
    doc.active_layer_mut().effects.pattern_overlay.enabled = true;
    doc.active_layer_mut().effects.pattern_overlay.fill.phase = (3.0, 4.0);
    h.state_mut()
        .run(Action::LayerStyle(Some(Style::PatternOverlay)));
    h.run_steps(3);
    // Each is named twice: its label and its field.
    assert!(h.query_all_by_label(&t!("pattern9-offset-x")).next().is_some());
    assert!(h.query_all_by_label(&t!("pattern9-offset-y")).next().is_some());
    h.get_by_label(&t!("pattern8-snap")).click();
    h.run_steps(3);
    let fill = &h
        .state()
        .doc
        .as_ref()
        .unwrap()
        .active_layer()
        .effects
        .pattern_overlay
        .fill;
    assert_eq!(fill.phase, (0.0, 0.0));
}

fn asym() -> Arc<Pattern> {
    Pattern::new(
        "asym",
        RgbaImage::from_fn(5, 3, |x, y| {
            Rgba([(x * 50) as u8, (y * 100) as u8, 77, 255])
        }),
    )
}

/// A document whose size isn't a multiple of the tile's: nothing is right by
/// luck.
fn doc_of(linked: bool, overlay: bool) -> Document {
    let mut doc = Document::new(RgbaImage::from_pixel(43, 31, Rgba(WHITE)), None);
    let fill = PatternFill {
        linked,
        phase: (2.0, 1.0),
        angle: 30.0,
        ..PatternFill::of(asym())
    };
    if overlay {
        doc.add_layer("x".into(), RgbaImage::from_pixel(30, 20, Rgba(RED)), (6, 5));
        let fx = &mut doc.active_layer_mut().effects;
        fx.pattern_overlay.enabled = true;
        fx.pattern_overlay.fill = fill;
    } else {
        doc.add_fill_layer(Fill::Pattern(fill), None);
    }
    doc
}

/// The largest difference, per channel, between two images, away from the
/// edges of a layer (`margin`), which are resampled.
fn worst(a: &RgbaImage, b: &RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| (0..4).map(move |i| p.0[i].abs_diff(q.0[i])))
        .max()
        .unwrap_or(0)
}

#[test]
fn image_rotation_turns_pattern_fills_and_overlays_exactly() {
    for (degrees, turn) in [
        (
            90.0,
            image::imageops::rotate90 as fn(&RgbaImage) -> RgbaImage,
        ),
        (180.0, image::imageops::rotate180),
        (270.0, image::imageops::rotate270),
    ] {
        for overlay in [false, true] {
            for linked in [false, true] {
                let mut doc = doc_of(linked, overlay);
                let expected = turn(&doc.flatten());
                doc.rotate_canvas(degrees);
                let got = doc.flatten();
                let w = worst(&got, &expected);
                assert!(w <= 2, "{degrees} overlay {overlay} linked {linked}: {w}");
            }
        }
    }
}

#[test]
fn a_flip_mirrors_pattern_fills_and_overlays_exactly() {
    for horizontal in [true, false] {
        for overlay in [false, true] {
            for linked in [false, true] {
                let mut doc = doc_of(linked, overlay);
                let before = doc.flatten();
                let expected = if horizontal {
                    image::imageops::flip_horizontal(&before)
                } else {
                    image::imageops::flip_vertical(&before)
                };
                doc.flip_canvas(horizontal);
                let w = worst(&doc.flatten(), &expected);
                assert!(
                    w <= 2,
                    "{horizontal} overlay {overlay} linked {linked}: {w}"
                );
                // Undone: as it was.
                doc.undo();
                assert_eq!(doc.flatten(), before);
            }
        }
    }
}

#[test]
fn a_linked_overlay_stays_while_painting_and_follows_a_move() {
    let mut doc = Document::new(RgbaImage::from_pixel(60, 60, Rgba(WHITE)), None);
    doc.add_layer(
        "x".into(),
        RgbaImage::from_pixel(20, 20, Rgba(RED)),
        (20, 20),
    );
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(asym());
    let a = doc.flatten();
    // The layer enlarged, as a brush does, one pixel painted far away.
    assert!(doc.expand_active_to_canvas());
    let at = doc.active_layer().offset;
    doc.active_layer_mut()
        .pixels_mut()
        .put_pixel((3 - at.0) as u32, (2 - at.1) as u32, Rgba(RED));
    let b = doc.flatten();
    for y in 20..40 {
        for x in 20..40 {
            assert_eq!(a.get_pixel(x, y), b.get_pixel(x, y), "{x},{y}");
        }
    }
    // Moved: the tile comes along.
    doc.active_layer_mut().translate(7, 4);
    let c = doc.flatten();
    for y in 20..40 {
        for x in 20..40 {
            assert_eq!(b.get_pixel(x, y), c.get_pixel(x + 7, y + 4), "{x},{y}");
        }
    }
}

#[test]
fn a_linked_overlay_on_a_layer_without_pixels_follows_a_move() {
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    doc.add_fill_layer(Fill::Solid([1, 2, 3]), None);
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(asym());
    let a = doc.flatten();
    doc.active_layer_mut().translate(1, 0);
    let b = doc.flatten();
    for y in 0..30 {
        for x in 0..39 {
            assert_eq!(a.get_pixel(x, y), b.get_pixel(x + 1, y), "{x},{y}");
        }
    }
}

#[test]
fn free_transform_of_a_pattern_fill_scales_and_turns_its_tile() {
    use crate::document::Placement;
    let filter = image::imageops::FilterType::Triangle;
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(asym())), None);
    let a = doc.flatten();
    let src = doc.full_rect();
    let steps = doc.history().1;
    let place = Placement {
        at: (0, 0),
        size: (80, 60),
        angle: 0.0,
    };
    doc.transform_active(src, place, filter, false);
    let LayerKind::Fill(Fill::Pattern(f)) = &doc.active_layer().kind else {
        panic!("a fill");
    };
    assert_eq!(f.scale, 2.0);
    assert_ne!(a, doc.flatten());
    assert_eq!(doc.history().1, steps + 1);
    let place = Placement {
        at: (0, 0),
        size: (40, 30),
        angle: 90.0,
    };
    doc.transform_active(src, place, filter, false);
    let LayerKind::Fill(Fill::Pattern(f)) = &doc.active_layer().kind else {
        panic!("a fill");
    };
    assert_eq!(f.angle, -90.0);
    // Only moved, a tile staying at the document's origin stays.
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    let fill = PatternFill {
        linked: false,
        ..PatternFill::of(asym())
    };
    doc.add_fill_layer(Fill::Pattern(fill), None);
    let a = doc.flatten();
    let place = Placement {
        at: (3, 2),
        size: (40, 30),
        angle: 0.0,
    };
    doc.transform_active(src, place, filter, false);
    assert_eq!(a, doc.flatten());
}

#[test]
fn a_plain_crop_leaves_every_pattern_where_it_was() {
    for overlay in [false, true] {
        for linked in [false, true] {
            let mut doc = doc_of(linked, overlay);
            let before = doc.flatten();
            doc.resize_canvas(3, 2, 30, 20);
            let after = doc.flatten();
            for y in 0..20 {
                for x in 0..30 {
                    let (p, q) = (after.get_pixel(x, y).0, before.get_pixel(x + 3, y + 2).0);
                    let d = (0..4).map(|i| p[i].abs_diff(q[i])).max().unwrap();
                    assert!(d <= 1, "overlay {overlay} linked {linked}: {x},{y}");
                }
            }
        }
    }
}

#[test]
fn pattern_tiles_count_in_the_history_budget() {
    let big = Pattern::new("big", RgbaImage::from_pixel(1024, 1024, Rgba(RED)));
    let mut doc = Document::new(RgbaImage::from_pixel(8, 8, Rgba(WHITE)), None);
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(big.clone())), None);
    let before = doc.history_bytes();
    // A state holds it, the document no longer does.
    doc.checkpoint("test".to_owned());
    doc.layers.pop();
    doc.active = 0;
    let held = doc.history_bytes() - before;
    assert!(held >= 4 * 1024 * 1024, "{held}");
    // Its second use: counted once.
    drop(big);
    assert!(held < 8 * 1024 * 1024, "{held}");
}

#[test]
fn a_disabled_overlay_keeps_its_settings_and_tile_in_a_file() {
    let dir = crate::testing::test_dir("overlay");
    let path = dir.join("x.atelier");
    let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba(WHITE)), None);
    doc.add_layer("x".into(), RgbaImage::from_pixel(4, 4, Rgba(RED)), (1, 1));
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.enabled = false;
    fx.pattern_overlay.opacity = 0.3;
    fx.pattern_overlay.fill = PatternFill {
        scale: 2.0,
        ..PatternFill::of(asym())
    };
    let kept = fx.clone();
    doc.save(&path).unwrap();
    let back = Document::open(&path).unwrap().doc;
    assert_eq!(back.layers.last().unwrap().effects, kept);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_names_of_a_tile_survive_a_file() {
    let dir = crate::testing::test_dir("names");
    let path = dir.join("x.atelier");
    let t = asym();
    let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba(WHITE)), None);
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(t.clone())), None);
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(t.renamed("other"))), None);
    doc.save(&path).unwrap();
    let back = Document::open(&path).unwrap().doc;
    let names: Vec<&str> = back.layers[1..]
        .iter()
        .filter_map(|l| match &l.kind {
            LayerKind::Fill(Fill::Pattern(f)) => Some(f.pattern.name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["asym", "other"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn values_that_are_not_numbers_still_give_a_file_that_opens() {
    let dir = crate::testing::test_dir("nan");
    let path = dir.join("x.atelier");
    let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba(WHITE)), None);
    let fill = PatternFill {
        scale: f32::NAN,
        angle: f32::INFINITY,
        phase: (f32::NAN, f32::NEG_INFINITY),
        ..PatternFill::of(asym())
    };
    doc.add_fill_layer(Fill::Pattern(fill), None);
    doc.save(&path).unwrap();
    Document::open(&path).expect("opens");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_last_pattern_comes_back_in_the_next_session() {
    let mut h = app();
    let id = with_tile(&mut h);
    let saved = h.state().session_settings();
    let mut next = self::app();
    next.state_mut().restore_session(&saved);
    assert_eq!(next.state().patterns.last, id);
    // Unknown: the first of the library is offered.
    assert_eq!(
        next.state().last_pattern().id,
        next.state().patterns.library.items[0].id
    );
}

#[test]
fn documents_keep_their_tiles_in_both_formats() {
    let dir = crate::testing::test_dir("pattern-doc");
    let mut doc = Document::new(RgbaImage::from_pixel(20, 20, Rgba(WHITE)), None);
    let fill = PatternFill {
        scale: 2.0,
        angle: 30.0,
        linked: false,
        phase: (1.0, 2.0),
        ..PatternFill::of(tile())
    };
    doc.add_fill_layer(Fill::Pattern(fill.clone()), None);
    let mut other = RgbaImage::from_pixel(3, 3, Rgba([9, 9, 9, 255]));
    other.put_pixel(1, 1, Rgba([1, 2, 3, 77]));
    let second = Pattern::new("Second", other);
    doc.add_layer("fx".into(), RgbaImage::from_pixel(5, 5, Rgba(RED)), (0, 0));
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(second.clone());
    fx.pattern_overlay.opacity = 0.5;
    fx.pattern_overlay.blend = crate::document::BlendMode::Multiply;
    let look = doc.flatten();
    for file in ["doc.atelier", "doc.psd"] {
        let path = dir.join(file);
        let notes = doc.save(&path).unwrap();
        assert!(notes.is_empty(), "{file}: {notes:?}");
        let back = Document::open(&path).unwrap().doc;
        assert!(back.file_notes.is_empty(), "{file}: {:?}", back.file_notes);
        let LayerKind::Fill(Fill::Pattern(f)) = &back.layers[1].kind else {
            panic!("{file}: {:?}", back.layers[1].kind);
        };
        assert_eq!(f, &fill, "{file}");
        assert_eq!(*f.pattern.image, *fill.pattern.image);
        let o = &back.layers[2].effects.pattern_overlay;
        assert_eq!(o, &doc.layers[2].effects.pattern_overlay, "{file}");
        assert_eq!(*o.fill.pattern.image, *second.image);
        assert_eq!(back.flatten(), look, "{file}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_fields_fit_a_narrow_window() {
    let mut h = app();
    with_tile(&mut h);
    h.set_size(egui::vec2(660.0, 700.0));
    let id = h.state().last_pattern().id;
    h.state_mut().fill_dialog = Some(fill_dialog(id));
    h.run_steps(3);
    let picker = h.get_by_label(&t!("pattern8-pattern")).rect();
    assert!(
        picker.right() <= 660.0 && picker.left() >= 0.0,
        "{picker:?}"
    );
    h.state_mut().fill_dialog = None;
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.add_layer(
        "top".into(),
        RgbaImage::from_pixel(40, 30, Rgba(BLUE)),
        (0, 0),
    );
    doc.active_layer_mut().effects.pattern_overlay.enabled = true;
    h.state_mut()
        .run(Action::LayerStyle(Some(Style::PatternOverlay)));
    h.run_steps(3);
    for name in [
        t!("pattern8-pattern"),
        t!("pattern8-link"),
        t!("pattern8-snap"),
    ] {
        let r = h.get_by_label(&name).rect();
        assert!(r.right() <= 660.0 && r.left() >= 0.0, "{name}: {r:?}");
    }
}
