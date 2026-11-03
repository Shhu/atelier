//! Patterns after the review of 0.5.0: group overlays, Free Transform and
//! Flip Canvas of Pattern Fill layers, the offset fields, the library's
//! files, the steps of actions.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable};
use image::{Rgba, RgbaImage};

use super::*;
use crate::actions::Recorded;
use crate::compose::pattern::PatternFill;
use crate::document::{Document, Fill, LayerKind, PixelRect};

const WHITE: [u8; 4] = [255; 4];
const RED: [u8; 4] = [200, 0, 0, 255];

fn harness(doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = doc.take();
            app
        });
    h.run_steps(2);
    h
}

fn asym() -> Arc<Pattern> {
    Pattern::new(
        "asym",
        RgbaImage::from_fn(5, 3, |x, y| {
            Rgba([(x * 50) as u8, (y * 100) as u8, 77, 255])
        }),
    )
}

fn fill_of(doc: &Document, i: usize) -> PatternFill {
    match &doc.layers[i].kind {
        LayerKind::Fill(Fill::Pattern(f)) => f.clone(),
        k => panic!("not a pattern fill: {k:?}"),
    }
}

fn index_of(doc: &Document, name: &str) -> usize {
    doc.layers.iter().position(|l| l.name == name).unwrap()
}

/// 60 × 60, a group holding a 20 × 20 red layer at (20, 20), the group
/// with a linked Pattern Overlay.
fn group_doc() -> (Document, usize) {
    let mut doc = Document::new(RgbaImage::from_pixel(60, 60, Rgba(WHITE)), None);
    doc.add_layer("x".into(), RgbaImage::from_pixel(20, 20, Rgba(RED)), (20, 20));
    let child = doc.active;
    assert!(doc.group_layers(&[child]));
    let group = (0..doc.layers.len()).find(|&i| doc.layers[i].is_group()).unwrap();
    let fx = &mut doc.layers[group].effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill {
        phase: (3.0, 1.0),
        ..PatternFill::of(asym())
    };
    (doc, group)
}

fn same_area(a: &RgbaImage, b: &RgbaImage, area: PixelRect, shift: (i64, i64)) -> usize {
    let mut changed = 0;
    for y in area.y..area.y + area.h as i64 {
        for x in area.x..area.x + area.w as i64 {
            let p = a.get_pixel(x as u32, y as u32);
            let q = b.get_pixel((x + shift.0) as u32, (y + shift.1) as u32);
            changed += usize::from(p != q);
        }
    }
    changed
}

const AREA: PixelRect = PixelRect {
    x: 20,
    y: 20,
    w: 20,
    h: 20,
};

#[test]
fn a_group_overlay_stays_while_a_layer_in_it_is_painted() {
    let (mut doc, _) = group_doc();
    let a = doc.flatten();
    doc.active = index_of(&doc, "x");
    // Enlarged as a brush does, one pixel painted outside what the group held.
    assert!(doc.expand_active_to_canvas());
    let at = doc.active_layer().offset;
    doc.active_layer_mut()
        .pixels_mut()
        .put_pixel((3 - at.0) as u32, (2 - at.1) as u32, Rgba(RED));
    assert_eq!(same_area(&a, &doc.flatten(), AREA, (0, 0)), 0);
}

#[test]
fn a_group_overlay_follows_the_whole_group_and_its_geometry() {
    let (mut doc, group) = group_doc();
    let a = doc.flatten();
    // The Move tool's way: the group with its layers.
    for j in doc.movable_block(group) {
        doc.layers[j].offset.0 += 7;
        doc.layers[j].offset.1 += 4;
    }
    assert_eq!(same_area(&a, &doc.flatten(), AREA, (7, 4)), 0);
    // Turned and mirrored with the image.
    let before = doc.flatten();
    doc.rotate_canvas(90.0);
    let w = worst(&doc.flatten(), &image::imageops::rotate90(&before));
    assert!(w <= 2, "{w}");
    let before = doc.flatten();
    doc.flip_canvas(true);
    let w = worst(&doc.flatten(), &image::imageops::flip_horizontal(&before));
    assert!(w <= 2, "{w}");
}

fn worst(a: &RgbaImage, b: &RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| (0..4).map(move |i| p.0[i].abs_diff(q.0[i])))
        .max()
        .unwrap_or(0)
}

#[test]
fn a_group_overlay_keeps_its_place_in_both_formats() {
    let dir = std::env::temp_dir().join(format!("atelier-pattern11-group-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut doc, group) = group_doc();
    // Moved, then a layer of it painted beyond: its frame and what it holds
    // part.
    for j in doc.movable_block(group) {
        doc.layers[j].offset.0 += 5;
        doc.layers[j].offset.1 += 3;
    }
    doc.active = index_of(&doc, "x");
    assert!(doc.expand_active_to_canvas());
    let at = doc.active_layer().offset;
    doc.active_layer_mut()
        .pixels_mut()
        .put_pixel((50 - at.0) as u32, (55 - at.1) as u32, Rgba(RED));
    let look = doc.flatten();
    for file in ["group.atelier", "group.psd"] {
        let path = dir.join(file);
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert_eq!(worst(&back.flatten(), &look), 0, "{file}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_offset_fields_count_from_the_visible_corner() {
    let mut doc = Document::new(RgbaImage::from_pixel(120, 100, Rgba(WHITE)), None);
    doc.add_layer("top".into(), RgbaImage::from_pixel(30, 20, Rgba(RED)), (50, 50));
    let fx = &mut doc.active_layer_mut().effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(asym());
    // The first dab enlarges the layer: the tile stays, and so do the
    // numbers shown.
    assert!(doc.expand_active_to_canvas());
    let i = doc.active;
    assert_eq!(doc.tile_corner(i), (50.0, 50.0));
    assert_eq!(doc.active_layer().effects.pattern_overlay.fill.phase, (50.0, 50.0));
    let mut h = harness(doc);
    h.state_mut()
        .run(Action::LayerStyle(Some(Style::PatternOverlay)));
    h.run_steps(3);
    // At the corner already: nothing to snap.
    assert!(h.get_by_label(&t!("pattern8-snap")).accesskit_node().is_disabled());
    // Moved off it, Snap brings it back to the layer's corner, not to its frame's.
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .effects
        .pattern_overlay
        .fill
        .phase = (53.0, 51.0);
    h.state_mut().layer_style = None;
    h.state_mut()
        .run(Action::LayerStyle(Some(Style::PatternOverlay)));
    h.run_steps(3);
    h.get_by_label(&t!("pattern8-snap")).click();
    h.run_steps(3);
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(doc.active_layer().effects.pattern_overlay.fill.phase, (50.0, 50.0));
}

#[test]
fn free_transform_reaches_a_lone_pattern_fill_layer() {
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(asym())), None);
    let fill = doc.active;
    let mut h = harness(doc);
    let steps = h.state().doc.as_ref().unwrap().history().1;
    assert!(h.state_mut().enter_transform(), "{}", h.state().status);
    let r = h.state().transform_rect.unwrap();
    h.state_mut().transform_rect = Some(egui::Rect::from_min_size(r.min, r.size() * 2.0));
    h.state_mut().apply_transform();
    h.run_steps(2);
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(fill_of(doc, fill).scale, 2.0);
    assert_eq!(doc.history().1, steps + 1);
}

fn finish(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..400 {
        h.run_steps(1);
        if !h.state().transform_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn free_transform_of_several_layers_takes_the_tile_too() {
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    doc.add_layer("px".into(), RgbaImage::from_pixel(10, 10, Rgba(RED)), (5, 5));
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(asym())), None);
    let fill = doc.active;
    let ids: Vec<u64> = doc.layers[1..].iter().map(|l| l.id).collect();
    let mut h = harness(doc);
    h.state_mut().layer_set = ids;
    let steps = h.state().doc.as_ref().unwrap().history().1;
    assert!(h.state_mut().enter_transform());
    let r = h.state().transform_rect.unwrap();
    h.state_mut().transform_rect = Some(egui::Rect::from_min_size(r.min, r.size() * 2.0));
    h.state_mut().apply_transform();
    finish(&mut h);
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(fill_of(doc, fill).scale, 2.0);
    assert_eq!(doc.layers[1].rect().w, 20);
    assert_eq!(doc.history().1, steps + 1);
}

#[test]
fn a_tile_follows_an_affine_map_and_refuses_a_perspective() {
    use crate::document::TransformTarget;
    use crate::xform::{Homography, Transform};
    let filter = image::imageops::FilterType::Triangle;
    // A mirror through Free Transform: as Flip Canvas does it.
    let make = || {
        let mut doc = Document::new(RgbaImage::from_pixel(43, 31, Rgba(WHITE)), None);
        let fill = PatternFill {
            phase: (2.0, 1.0),
            angle: 30.0,
            ..PatternFill::of(asym())
        };
        doc.add_fill_layer(Fill::Pattern(fill), None);
        doc
    };
    let mut doc = make();
    let i = doc.active;
    let mut control = make();
    control.flip_canvas(true);
    let src = [0.0, 0.0, 43.0, 31.0];
    let mirror = Transform::projective(src, Homography::affine([-1.0, 0.0, 43.0, 0.0, 1.0, 0.0]));
    assert_eq!(doc.tile_cant_follow(&[i], &mirror), None);
    doc.transform_target(TransformTarget::Layer, &[i], &mirror, filter, "t".into());
    let (got, expected) = (fill_of(&doc, i), fill_of(&control, i));
    assert_eq!(got.pattern.id, expected.pattern.id);
    assert!((got.angle - expected.angle).abs() < 1e-3, "{} {}", got.angle, expected.angle);
    assert!(worst(&doc.flatten(), &control.flatten()) <= 2);
    // Turned a quarter and scaled by 3.
    let turn = Transform::projective(src, Homography::affine([0.0, -3.0, 0.0, 3.0, 0.0, 0.0]));
    doc.transform_target(TransformTarget::Layer, &[i], &turn, filter, "t".into());
    let f = fill_of(&doc, i);
    assert!((f.scale - 3.0).abs() < 1e-4, "{}", f.scale);
    // In perspective: refused.
    let perspective = Transform::projective(
        src,
        Homography([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.001, 0.0, 1.0]),
    );
    assert_eq!(doc.tile_cant_follow(&[i], &perspective), Some(i));
}

#[test]
fn a_flip_mirrors_a_shared_tile_once_and_finds_it_back() {
    let tile = asym();
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    for _ in 0..3 {
        doc.add_fill_layer(Fill::Pattern(PatternFill::of(tile.clone())), None);
    }
    let images = |doc: &Document| {
        let mut ptrs: Vec<usize> = (1..doc.layers.len())
            .map(|i| Arc::as_ptr(&fill_of(doc, i).pattern.image) as usize)
            .collect();
        ptrs.dedup();
        ptrs
    };
    doc.flip_canvas(true);
    assert_eq!(images(&doc).len(), 1);
    assert_ne!(fill_of(&doc, 1).pattern.id, tile.id);
    doc.flip_canvas(true);
    assert_eq!(images(&doc), vec![Arc::as_ptr(&tile.image) as usize]);
}

#[test]
fn flip_canvas_is_one_step_through_the_app() {
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    doc.add_layer("x".into(), RgbaImage::from_pixel(5, 5, Rgba(RED)), (0, 0));
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(asym())), None);
    let mut h = harness(doc);
    let before = h.state().doc.as_ref().unwrap().flatten();
    let steps = h.state().doc.as_ref().unwrap().history().1;
    h.state_mut().run(Action::FlipCanvas(true));
    finish(&mut h);
    let doc = h.state().doc.as_ref().unwrap();
    assert_eq!(doc.history().1, steps + 1);
    assert_eq!(doc.layers[1].offset, (35, 0));
    let w = worst(&doc.flatten(), &image::imageops::flip_horizontal(&before));
    assert!(w <= 2, "{w}");
}

#[test]
fn image_size_keeps_the_tile_scale_beyond_the_fields() {
    let mut doc = Document::new(RgbaImage::from_pixel(10, 10, Rgba(WHITE)), None);
    let fill = PatternFill {
        scale: 0.5,
        ..PatternFill::of(asym())
    };
    doc.add_fill_layer(Fill::Pattern(fill), None);
    let i = doc.active;
    doc.resize_image(300, 300, image::imageops::FilterType::Triangle);
    assert!((fill_of(&doc, i).scale - 15.0).abs() < 1e-3);
    // And back: as it was.
    doc.resize_image(10, 10, image::imageops::FilterType::Triangle);
    assert!((fill_of(&doc, i).scale - 0.5).abs() < 1e-4);
    // Kept by the files.
    doc.resize_image(300, 300, image::imageops::FilterType::Triangle);
    let dir = std::env::temp_dir().join(format!("atelier-pattern11-scale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for file in ["s.atelier", "s.psd"] {
        let path = dir.join(file);
        doc.save(&path).unwrap();
        let back = Document::open(&path).unwrap().doc;
        assert!((fill_of(&back, 1).scale - 15.0).abs() < 1e-3, "{file}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The record of the layer above the background of a written PSD (its
/// rectangle first: top, left, bottom, right).
fn first_record(data: &[u8]) -> usize {
    let be = |at: usize| u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
    let mut at = 26;
    at += 4 + be(at); // color mode data
    at += 4 + be(at); // resources
    at + 4 + 4 + 2 // layer and mask info, layer info, count
}

#[test]
fn a_pattern_fill_record_with_a_box_starts_its_tile_there() {
    let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None);
    doc.add_fill_layer(Fill::Pattern(PatternFill::of(asym())), None);
    doc.layers.remove(0);
    doc.active = 0;
    let dir = std::env::temp_dir().join(format!("atelier-pattern11-rect-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rect.psd");
    doc.save(&path).unwrap();
    let mut data = std::fs::read(&path).unwrap();
    let at = first_record(&data);
    assert_eq!(&data[at..at + 16], &[0; 16], "an empty box as written");
    // As a vector-masked fill comes: the box of its shape.
    for (k, v) in [4i32, 7, 0, 0].iter().enumerate() {
        data[at + 4 * k..at + 4 * k + 4].copy_from_slice(&v.to_be_bytes());
    }
    std::fs::write(&path, &data).unwrap();
    let back = Document::open(&path).unwrap().doc;
    let f = fill_of(&back, 0);
    assert_eq!(f.phase, (7.0, 4.0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_long_pattern_name_is_cut_and_leaves_the_dialog_as_it_was() {
    let mut h = harness(Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None));
    let short = asym();
    let long = Pattern::new(
        "A big pattern defined on the whole canvas, with a long name",
        RgbaImage::from_pixel(3, 3, Rgba(RED)),
    );
    h.state_mut().patterns.library.add(short.clone());
    h.state_mut().patterns.library.add(long.clone());
    let mut ok = Vec::new();
    for id in [short.id, long.id] {
        h.state_mut().fill_dialog = Some(FillDialog {
            content: FillContent::Pattern,
            color: Color32::BLACK,
            opacity: 1.0,
            preserve_transparency: false,
            adapt: true,
            pattern: id,
        });
        h.run_steps(4);
        ok.push(h.get_by_label(&t!("dialog-ok")).rect());
        h.state_mut().fill_dialog = None;
        h.run_steps(2);
    }
    assert_eq!(ok[0], ok[1]);
    let cut = elided(&long.name, 100.0, |s| s.chars().count() as f32 * 10.0);
    assert!(cut.ends_with('…') && cut.chars().count() <= 10, "{cut}");
    assert_eq!(elided("short", 100.0, |s| s.len() as f32), "short");
}

#[test]
fn a_tile_that_cannot_be_read_for_now_is_kept() {
    use crate::patterns::{Change, Library};
    let dir = std::env::temp_dir().join(format!("atelier-pattern11-lib-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = Pattern::new("t", RgbaImage::from_fn(64, 64, |x, y| Rgba([x as u8, y as u8, 1, 255])));
    let file = format!("{:016x}.png", p.id);
    let add = || Change::Add {
        file: file.clone(),
        pattern: p.clone(),
    };
    let files = || {
        let mut f: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".png"))
            .collect();
        f.sort();
        f
    };
    Library::apply(&dir, &[add()]).unwrap();
    Library::apply(&dir, &[add()]).unwrap();
    assert_eq!(files(), vec![file.clone()]);
    // Memory short: the good file stays, nothing set aside.
    crate::document::set_test_free_memory(Some(1000));
    Library::apply(&dir, &[add()]).unwrap();
    crate::document::set_test_free_memory(None);
    assert_eq!(files(), vec![file.clone()]);
    // Damaged: set aside and written again.
    std::fs::write(dir.join(&file), b"\x89PNG\r\n\x1a\nnot a png").unwrap();
    Library::apply(&dir, &[add()]).unwrap();
    assert_eq!(files().len(), 2, "{:?}", files());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn taken_in_tiles_keep_their_names_or_get_new_ones() {
    use crate::patterns::Library;
    let dir = std::env::temp_dir().join(format!("atelier-pattern11-names-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let png = |name: &str, v: u8| {
        RgbaImage::from_pixel(2, 2, Rgba([v, v, v, 255]))
            .save_with_format(dir.join(name), image::ImageFormat::Png)
            .unwrap();
    };
    png("a.png", 1);
    png("b.png", 2);
    // Damaged at its end: the first entry still reads.
    std::fs::write(
        dir.join("index.json"),
        br#"[{"name": "Bricks", "file": "a.png"}, {"name": "Pattern 2", "file": "b.p"#,
    )
    .unwrap();
    let read = Library::read(&dir);
    assert!(read.set_aside.is_some());
    let mut library = Library::default();
    library.merge(read);
    let mut names: Vec<String> = library.user().iter().map(|p| p.name.clone()).collect();
    names.sort();
    assert_eq!(names, vec!["Bricks".to_owned(), t!("pattern8-default-name", number = 2)]);
    // A name already given isn't given twice.
    let taken = t!("pattern8-default-name", number = 1);
    assert_eq!(
        crate::patterns::default_name(&[&taken], 0),
        t!("pattern8-default-name", number = 2)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fill_keeps_the_remembered_pattern_while_the_library_is_read() {
    let mut h = harness(Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None));
    // Still being read: a choice the list doesn't have yet stays.
    let (_send, recv) = std::sync::mpsc::channel();
    h.state_mut().patterns.loading = Some(recv);
    h.state_mut().fill_dialog = Some(FillDialog {
        content: FillContent::Pattern,
        color: Color32::BLACK,
        opacity: 1.0,
        preserve_transparency: false,
        adapt: true,
        pattern: 12345,
    });
    h.run_steps(3);
    assert_eq!(h.state().fill_dialog.unwrap().pattern, 12345);
}

#[test]
fn fill_layers_and_path_commands_are_recorded_and_played() {
    use crate::vector::{SubPath, pt};
    let mut h = harness(Document::new(RgbaImage::from_pixel(40, 30, Rgba(WHITE)), None));
    let app = h.state_mut();
    app.primary = Color32::from_rgb(10, 20, 30);
    app.secondary = Color32::from_rgb(40, 50, 60);
    let solid = app.recorded_action(Action::NewFillLayer(false)).unwrap();
    assert_eq!(solid, Recorded::NewSolidFill { color: [10, 20, 30] });
    let gradient = app.recorded_action(Action::NewFillLayer(true)).unwrap();
    assert_eq!(
        gradient,
        Recorded::NewGradientFill {
            from: [10, 20, 30],
            to: [40, 50, 60]
        }
    );
    app.primary = Color32::BLACK;
    app.play_step(&solid, false).unwrap();
    let doc = app.doc.as_ref().unwrap();
    assert!(matches!(doc.active_layer().kind, LayerKind::Fill(Fill::Solid([10, 20, 30]))));
    app.play_step(&gradient, false).unwrap();
    assert!(matches!(app.doc.as_ref().unwrap().active_layer().kind, LayerKind::Fill(Fill::Gradient(_))));
    assert!(!actions_panel::step_title(&solid).is_empty());
    assert!(!actions_panel::step_details(&gradient).is_empty());

    // Fill Path and Stroke Path, on a pixel layer, while recording.
    let doc = app.doc.as_mut().unwrap();
    doc.add_layer("px".into(), RgbaImage::new(40, 30), (0, 0));
    let mut p = crate::vector::Path::default();
    p.subpaths.push(SubPath::polygon([
        pt(5.0, 5.0),
        pt(20.0, 5.0),
        pt(20.0, 20.0),
        pt(5.0, 20.0),
    ]));
    doc.work_path = Some(p);
    app.vector.selected = Some(crate::document::PathRef::Work);
    let recorded = |app: &mut AtelierApp, command: paths_panel::PathCommand| {
        use crate::actions::{Sequence, Set};
        let action = Sequence { name: "a".into(), key: None, steps: Vec::new(), expanded: true };
        app.actions.library.sets = vec![Set { name: "s".into(), actions: vec![action], expanded: true }];
        app.actions.start_recording((0, 0));
        app.path_command(command, false);
        app.actions.stop_recording();
        app.actions.library.sets[0].actions[0].steps.clone()
    };
    let steps = recorded(app, paths_panel::PathCommand::FillPath);
    assert!(
        matches!(steps[..], [ref s] if matches!(s.command, Recorded::FillPath { .. })),
        "{steps:?}"
    );
    let fill = steps[0].command.clone();
    let steps = recorded(app, paths_panel::PathCommand::StrokePath);
    assert!(
        matches!(steps[..], [ref s] if matches!(s.command, Recorded::StrokePath { .. })),
        "{steps:?}"
    );
    // Played on an empty layer: it fills the path again.
    let doc = app.doc.as_mut().unwrap();
    doc.add_layer("again".into(), RgbaImage::new(40, 30), (0, 0));
    let n = doc.history().1;
    app.play_step(&fill, false).unwrap();
    let doc = app.doc.as_ref().unwrap();
    assert_eq!(doc.history().1, n + 1);
    assert_eq!(doc.active_layer().image().get_pixel(10, 10).0[3], 255);
    assert_eq!(doc.active_layer().image().get_pixel(30, 25).0[3], 0);
}

#[test]
fn shared_mirrors_follow_renamed_tiles() {
    let tile = asym();
    let renamed = tile.renamed("other");
    let m = tile.mirrored(true);
    let n = renamed.mirrored(true);
    assert!(Arc::ptr_eq(&m.image, &n.image));
    assert_eq!(n.name, "other");
    assert_eq!(m.name, "asym");
    assert!(Arc::ptr_eq(&m.mirrored(true).image, &tile.image));
    assert!(!Arc::ptr_eq(&tile.mirrored(false).image, &m.image));
}
