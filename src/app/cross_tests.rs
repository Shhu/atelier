//! Features built apart, together: a turned view with the measures, the
//! Navigator after a crop, styles on groups, Lock Transparent Pixels against
//! what fills and distorts, panels in full screen, and files holding it all.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use image::{Rgba, RgbaImage};

use super::*;
use crate::compose::pattern::Pattern;
use crate::compose::pattern::PatternFill;
use crate::document::{Fill, LayerKind};
use crate::filters::{Filter, FilterKind};
use crate::pixel_ops::{Adjustment, ChannelMixer, Exposure, SelectiveColor};

const WHITE: [u8; 4] = [255; 4];
const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn app_with(size: [f32; 2], doc: Document) -> Harness<'static, AtelierApp> {
    let mut doc = Some(doc);
    let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
        let mut app = AtelierApp::new(cc, None);
        app.doc = doc.take();
        app
    });
    h.run_steps(3);
    h
}

fn white(w: u32, h: u32) -> Document {
    Document::new(RgbaImage::from_pixel(w, h, Rgba(WHITE)), None)
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn doc_mut<'a>(h: &'a mut Harness<'static, AtelierApp>) -> &'a mut Document {
    h.state_mut().doc.as_mut().expect("a document")
}

/// Where an image point really is on screen.
fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    let view = app.last_view.expect("canvas shown");
    app.view_turn.to_screen(view.to_screen(pos2(x, y)))
}

fn drag(h: &mut Harness<'static, AtelierApp>, from: Pos2, to: Pos2) {
    h.hover_at(from);
    h.run_steps(1);
    h.drag_at(from);
    h.run_steps(1);
    for i in 1..=6 {
        h.hover_at(from.lerp(to, i as f32 / 6.0));
        h.run_steps(1);
    }
    h.drop_at(to);
    h.run_steps(3);
}

/// Runs frames until `done`, the workers having their time.
fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in 0..800 {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("never done");
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("atelier-cross-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a folder");
    dir
}

fn tile() -> Arc<Pattern> {
    let mut image = RgbaImage::new(2, 1);
    image.put_pixel(0, 0, Rgba(RED));
    image.put_pixel(1, 0, Rgba(BLUE));
    Pattern::new("Tile", image)
}

// ---------------------------------------------------------- A turned view

#[test]
fn ruler_and_color_sampler_land_on_the_image_on_a_turned_or_flipped_view() {
    for (angle, flipped) in [(30.0, false), (0.0, true), (30.0, true)] {
        let what = format!("{angle} {flipped}");
        let mut h = app_with([1400.0, 1000.0], white(400, 300));
        h.state_mut().view_turn.angle = angle;
        h.state_mut().view_turn.flipped = flipped;
        h.state_mut().set_tool(Tool::Ruler);
        h.state_mut().rulers.settings.snap = false;
        h.run_steps(2);
        let tolerance = 1.5 / h.state().last_view.expect("canvas").zoom + 0.6;

        // The line reads on the image, not on the screen: level stays level.
        let (a, b) = (screen(&h, 100.0, 100.0), screen(&h, 250.0, 100.0));
        drag(&mut h, a, b);
        let m = doc(&h).ruler.expect(&what);
        assert!(m.angle().abs() < 0.5, "{what}: {}", m.angle());
        assert!((m.length() - 150.0).abs() < 1.0, "{what}: {}", m.length());
        // Its end, where it shows.
        let (a, b) = (screen(&h, 250.0, 100.0), screen(&h, 250.0, 200.0));
        drag(&mut h, a, b);
        let m = doc(&h).ruler.expect(&what);
        assert!(
            (m.length() - 150f32.hypot(100.0)).abs() < 1.5,
            "{what}: {}",
            m.length()
        );
        // Clear for the next tool's clicks.
        h.state_mut().set_tool(Tool::ColorSampler);
        h.run_steps(2);

        let at = screen(&h, 80.0, 220.0);
        drag(&mut h, at, at);
        let points = doc(&h).samplers.clone();
        assert_eq!(points.len(), 1, "{what}");
        let p = pos2(points[0].x, points[0].y);
        assert!(p.distance(pos2(80.0, 220.0)) <= tolerance, "{what}: {p:?}");
        // Dragged where it shows.
        let (a, b) = (at, screen(&h, 150.0, 40.0));
        drag(&mut h, a, b);
        let points = doc(&h).samplers.clone();
        assert_eq!(points.len(), 1, "{what}");
        let p = pos2(points[0].x, points[0].y);
        assert!(p.distance(pos2(150.0, 40.0)) <= tolerance, "{what}: {p:?}");
    }
}

// --------------------------------------------------------------- Navigator

#[test]
fn the_navigator_follows_a_crop() {
    let mut d = Document::new(RgbaImage::from_pixel(800, 400, Rgba(RED)), None);
    d.add_layer(
        "Blue".into(),
        RgbaImage::from_pixel(100, 300, Rgba(BLUE)),
        (100, 50),
    );
    let mut h = app_with([1400.0, 1000.0], d);
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Navigator));
    settle(&mut h, |a| a.navigator.ready());
    let before = h.state().navigator.picture().expect("shown");
    assert!(
        (before.width() / before.height() - 2.0).abs() < 0.05,
        "{before:?}"
    );

    // A thin part of it: the picture takes the new shape at once.
    let rect = PixelRect {
        x: 100,
        y: 50,
        w: 100,
        h: 300,
    };
    h.state_mut().apply_crop(crop_tool::CropPlan::plain(rect));
    h.state_mut().run(Action::Fit);
    h.run_steps(4);
    let r = h.state().navigator.picture().expect("shown");
    assert!((r.width() / r.height() - 1.0 / 3.0).abs() < 0.02, "{r:?}");

    // The view box still maps to the new canvas: a click goes to that point.
    h.state_mut().zoom = 8.0;
    h.run_steps(3);
    let at = r.min + r.size() * vec2(0.25, 0.75);
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
    let c = h.state().view_center().expect("a view");
    assert!(
        (c.x - 25.0).abs() < 3.0 && (c.y - 225.0).abs() < 3.0,
        "{c:?}"
    );

    // And the thumbnail is made again, of what the crop left.
    for _ in 0..800 {
        h.run_steps(1);
        if h.state_mut().navigator_current() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the thumbnail stays as it was");
}

// --------------------------------------------------- Pattern Overlay on a group

fn group_with_overlay(d: &mut Document) -> usize {
    d.add_layer(
        "A".into(),
        RgbaImage::from_pixel(10, 10, Rgba(WHITE)),
        (5, 5),
    );
    let at = d.active;
    assert!(d.group_layers(&[at]));
    let group = d.active;
    assert!(d.layers[group].is_group());
    let fx = &mut d.layers[group].effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(tile());
    group
}

#[test]
fn pattern_overlay_on_a_group_covers_its_content_and_survives_both_formats() {
    let mut d = Document::new(RgbaImage::from_pixel(30, 20, Rgba([0, 255, 0, 255])), None);
    let group = group_with_overlay(&mut d);
    let flat = d.flatten();
    // The tile laid over what the group holds, linked to the group.
    assert_eq!(flat.get_pixel(5, 5).0, RED);
    assert_eq!(flat.get_pixel(6, 5).0, BLUE);
    assert_eq!(
        flat.get_pixel(0, 0).0,
        [0, 255, 0, 255],
        "only over its content"
    );
    let dir = temp("group-overlay");
    for file in ["group.atelier", "group.psd"] {
        let path = dir.join(file);
        let notes = d.save(&path).expect("saved");
        assert!(notes.is_empty(), "{file}: {notes:?}");
        let back = Document::open(&path).expect("read").doc;
        assert!(back.file_notes.is_empty(), "{file}: {:?}", back.file_notes);
        let g = &back.layers[group];
        assert!(g.is_group(), "{file}");
        assert!(g.effects.pattern_overlay.enabled, "{file}");
        assert_eq!(
            g.effects.pattern_overlay, d.layers[group].effects.pattern_overlay,
            "{file}"
        );
        assert_eq!(
            *g.effects.pattern_overlay.fill.pattern.image,
            *tile().image,
            "{file}"
        );
        assert_eq!(back.flatten(), flat, "{file}");
    }
    // In the PSD: the group's section divider record holds the style.
    let raw = std::fs::read(dir.join("group.psd")).expect("read");
    let (lsct, lfx2) = (
        find(&raw, b"lsct").expect("lsct"),
        find(&raw, b"lfx2").expect("lfx2"),
    );
    assert!(lfx2 > lsct, "written with the group, after its divider");
    let _ = std::fs::remove_dir_all(&dir);
}

fn find(data: &[u8], tag: &[u8]) -> Option<usize> {
    data.windows(tag.len()).position(|w| w == tag)
}

// --------------------------------------------------- Lock Transparent Pixels

/// Half red at half opacity, half empty; its alpha.
fn half() -> RgbaImage {
    RgbaImage::from_fn(40, 30, |x, _| match x < 20 {
        true => Rgba([255, 0, 0, 128]),
        false => Rgba([0; 4]),
    })
}

fn locked_alpha_app() -> Harness<'static, AtelierApp> {
    let mut d = white(40, 30);
    d.add_layer("half".into(), half(), (0, 0));
    d.active_layer_mut().locks.transparent = true;
    app_with([1400.0, 900.0], d)
}

/// The layer's opacity is as it was.
fn alpha_kept(h: &Harness<'static, AtelierApp>, what: &str) {
    let image = doc(h).active_layer().image().clone();
    let (x0, y0) = (
        0i64 - doc(h).active_layer().offset.0,
        -doc(h).active_layer().offset.1,
    );
    let want = half();
    for (x, y, p) in want.enumerate_pixels() {
        let (lx, ly) = (i64::from(x) + x0, i64::from(y) + y0);
        let a = if lx >= 0
            && ly >= 0
            && lx < i64::from(image.width())
            && ly < i64::from(image.height())
        {
            image.get_pixel(lx as u32, ly as u32).0[3]
        } else {
            0
        };
        assert_eq!(a, p.0[3], "{what}: at {x}, {y}");
    }
}

#[test]
fn lock_transparent_pixels_holds_with_a_pattern_fill() {
    let mut h = locked_alpha_app();
    let p = tile();
    let id = p.id;
    h.state_mut().patterns.library.add(p);
    let steps = doc(&h).history().1;
    h.state_mut().fill_dialog = Some(FillDialog {
        content: FillContent::Pattern,
        color: Color32::BLACK,
        opacity: 1.0,
        preserve_transparency: false,
        adapt: true,
        pattern: id,
    });
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(doc(&h).history().1, steps + 1, "filled");
    alpha_kept(&h, "pattern fill");
    let image = doc(&h).active_layer().image().clone();
    assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 128]);
    assert_eq!(
        image.get_pixel(1, 0).0,
        [0, 0, 255, 128],
        "the color changed"
    );
}

#[test]
fn lock_transparent_pixels_holds_with_a_stroke() {
    let mut h = locked_alpha_app();
    h.state_mut().selection = Some(
        PixelRect {
            x: 5,
            y: 5,
            w: 30,
            h: 20,
        }
        .into(),
    );
    let steps = doc(&h).history().1;
    h.state_mut().run(Action::Stroke);
    h.run_steps(1);
    {
        let o = &mut h.state_mut().filters.stroke_options;
        o.width = 4;
        o.color = Some([0, 255, 0]);
        o.location = crate::pixel_ops::StrokeLocation::Center;
    }
    h.run_steps(1);
    h.key_press(Key::Enter);
    settle(&mut h, |a| !a.filters.busy());
    assert_eq!(doc(&h).history().1, steps + 1, "stroked");
    alpha_kept(&h, "stroke");
    let image = doc(&h).active_layer().image().clone();
    assert_eq!(image.get_pixel(5, 15).0, [0, 255, 0, 128], "on the line");
}

#[test]
fn lock_transparent_pixels_holds_with_every_distort_filter() {
    for kind in [
        FilterKind::Ripple,
        FilterKind::Wave,
        FilterKind::Twirl,
        FilterKind::Spherize,
        FilterKind::PolarCoordinates,
        FilterKind::Pinch,
        FilterKind::ZigZag,
        FilterKind::Shear,
    ] {
        let mut h = locked_alpha_app();
        let filter = match kind.initial() {
            Filter::Shear(mut s) => {
                s.insert(0.3, 0.5);
                Filter::Shear(s)
            }
            other => other,
        };
        let original = doc(&h).active_layer().image().clone();
        // Something to move: a ramp of colors.
        let ramp = RgbaImage::from_fn(40, 30, |x, y| {
            let a = original.get_pixel(x, y).0[3];
            Rgba([(x * 6) as u8, (y * 8) as u8, 40, a])
        });
        doc_mut(&mut h)
            .active_layer_mut()
            .replace_pixels(ramp.clone());
        h.state_mut().start_filter(filter);
        settle(&mut h, |a| !a.filters.busy());
        alpha_kept(&h, &format!("{kind:?}"));
        assert_ne!(
            *doc(&h).active_layer().image(),
            ramp,
            "{kind:?}: it applied"
        );
    }
}

// ----------------------------------------------------------- Full screen

#[test]
fn panels_opened_in_full_screen_show_and_the_mode_comes_back_clean() {
    use workspace::{Command, ScreenMode::*};
    for (command, id) in [
        (Command::Info, "info"),
        (Command::Histogram, "histogram"),
        (Command::Navigator, "navigator"),
    ] {
        // With the menu bar: the Window menu is there.
        let mut h = app_with([1400.0, 900.0], white(400, 300));
        h.state_mut().set_screen_mode(FullWithMenu);
        h.run_steps(3);
        h.state_mut().run(Action::Workspace(command));
        h.run_steps(4);
        assert!(h.state().workspace.screen == FullWithMenu, "{id}");
        let shown = h.ctx.memory(|m| m.area_rect(egui::Id::new(id))).expect(id);
        assert!(h.ctx.content_rect().contains_rect(shown), "{id}: {shown:?}");

        // Full Screen: F8, the palette… bring the panels back, nothing else.
        let mut h = app_with([1400.0, 900.0], white(400, 300));
        h.state_mut().set_screen_mode(Full);
        h.run_steps(3);
        assert!(!h.state().chrome_shown());
        h.state_mut().run(Action::Workspace(command));
        h.run_steps(4);
        assert!(
            h.state().workspace.screen == Full,
            "{id}: still full screen"
        );
        assert!(h.state().chrome_shown(), "{id}: shown, not opened unseen");
        let shown = h.ctx.memory(|m| m.area_rect(egui::Id::new(id))).expect(id);
        assert!(h.ctx.content_rect().contains_rect(shown), "{id}: {shown:?}");
        // Tools under it work: the canvas still takes the pointer.
        h.state_mut().set_tool(Tool::Select);
        h.run_steps(2);
        let (a, b) = (screen(&h, 10.0, 10.0), screen(&h, 60.0, 50.0));
        drag(&mut h, a, b);
        assert!(h.state().selection.is_some(), "{id}: selected");
        h.state_mut().selection = None;
        // Esc leaves it, the panel still there, nothing lost.
        h.key_press(Key::Escape);
        h.run_steps(3);
        h.key_press(Key::Escape);
        h.run_steps(3);
        assert!(h.state().workspace.screen == Standard, "{id}: out");
        assert!(h.state().chrome_shown(), "{id}");
        let shown = h.ctx.memory(|m| m.area_rect(egui::Id::new(id))).expect(id);
        assert!(h.ctx.content_rect().contains_rect(shown), "{id}: {shown:?}");
    }
}

// ------------------------------------------------------------------ Layers

/// All the lock row's pieces, on screen and in the panel.
#[test]
fn the_lock_row_fits_at_660_px() {
    use crate::i18n::Lang;
    for lang in [Lang::En, Lang::Fr] {
        i18n::set_lang(lang);
        let mut h = app_with([660.0, 700.0], white(100, 80));
        h.run_steps(3);
        let mut left = f32::MIN;
        for name in [
            t!("layers8-lock-label"),
            layer_locks::LockKind::Transparent.label(),
            layer_locks::LockKind::Pixels.label(),
            layer_locks::LockKind::Position.label(),
            layer_locks::LockKind::All.label(),
        ] {
            let r = h.get_by_label(&name).rect();
            assert!(
                r.right() <= 660.0 && r.left() >= 0.0,
                "{lang:?} {name}: {r:?}"
            );
            assert!(r.bottom() <= 700.0, "{lang:?} {name}: {r:?}");
            assert!(r.left() >= left, "{lang:?} {name}: side by side");
            left = r.right();
        }
    }
    i18n::set_lang(Lang::En);
}

#[test]
fn partial_locks_in_a_group_survive_duplicate_and_undo() {
    use crate::document::Locks;
    let mut d = white(40, 30);
    d.add_layer("alpha".into(), half(), (0, 0));
    let alpha = d.active;
    d.layers[alpha].locks = Locks {
        transparent: true,
        ..Default::default()
    };
    d.add_layer("pixels".into(), half(), (0, 0));
    let pixels = d.active;
    d.layers[pixels].locks = Locks {
        pixels: true,
        ..Default::default()
    };
    d.add_layer("free".into(), half(), (0, 0));
    let free = d.active;
    assert!(d.group_layers(&[alpha, pixels, free]));
    let group = d.active;
    d.layers[group].locks = Locks {
        position: true,
        ..Default::default()
    };
    let names = |d: &Document| -> Vec<(String, Locks, u32)> {
        d.layers
            .iter()
            .map(|l| (l.name.clone(), l.locks, l.depth))
            .collect()
    };
    let before = names(&d);
    let block = d.block(group);
    let count = d.layers.len();
    let ids: Vec<u64> = d.layers.iter().map(|l| l.id).collect();

    d.duplicate_active();
    assert_eq!(d.layers.len(), count + block.len());
    let copy = d.active;
    assert!(d.layers[copy].is_group() && d.layers[copy].locks.position);
    let copies: Vec<usize> = d.block(copy).filter(|&i| i != copy).collect();
    assert_eq!(copies.len(), 3);
    let (a, p, f) = (copies[0], copies[1], copies[2]);
    // Theirs and, within the copied group, what it passes down.
    assert!(d.layers[a].locks.transparent && !d.layers[a].locks.pixels);
    assert!(d.layers[p].locks.pixels && !d.layers[p].locks.transparent);
    assert!(!d.layers[f].locks.any());
    for &c in &copies {
        assert!(d.locks(c).position, "from its group");
        assert_eq!(d.lock_group(c), Some(copy), "the copy, not the first");
    }
    assert!(d.locks(a).transparent && !d.locks(a).pixels);
    assert!(d.locks(p).pixels);

    // The copy's lock works like the original's: alpha kept by a fill.
    d.active = a;
    assert!(d.alpha_locked());
    assert!(d.fill_active([0, 255, 0, 255], None, 1.0, false));
    let image = d.layers[a].image();
    assert_eq!(image.get_pixel(0, 0).0, [0, 255, 0, 128]);
    assert_eq!(image.get_pixel(30, 0).0[3], 0);
    assert!(d.undo(), "the fill");

    // The copy gone: everything as it was, and the locks still bite.
    assert!(d.undo(), "the duplicate");
    assert_eq!(names(&d), before);
    assert_eq!(d.layers.iter().map(|l| l.id).collect::<Vec<_>>(), ids);
    d.active = alpha;
    assert!(d.alpha_locked());
    assert!(d.fill_active([0, 255, 0, 255], None, 1.0, false));
    assert_eq!(
        d.layers[alpha].image().get_pixel(30, 0).0[3],
        0,
        "the lock holds"
    );
    assert!(d.undo());
    d.active = pixels;
    assert!(d.pixels_locked());
    // Redone: the copy's locks are back with it.
    d.active = group;
    d.duplicate_active();
    let again = d.active;
    assert!(d.block(again).any(|i| d.layers[i].locks.transparent));
}

// ---------------------------------------------------- Smart distort filters

#[test]
fn a_smart_filter_of_each_distort_survives_the_file() {
    let dir = temp("smart-distort");
    for kind in [
        FilterKind::Ripple,
        FilterKind::Wave,
        FilterKind::Twirl,
        FilterKind::Spherize,
        FilterKind::PolarCoordinates,
        FilterKind::Pinch,
        FilterKind::ZigZag,
        FilterKind::Shear,
    ] {
        let filter = match kind.initial() {
            // Non-default, with a part that isn't a round number.
            Filter::Shear(mut s) => {
                s.insert(0.333, 0.25);
                Filter::Shear(s)
            }
            other => other,
        };
        let image =
            RgbaImage::from_fn(48, 36, |x, y| Rgba([(x * 5) as u8, (y * 7) as u8, 90, 255]));
        let mut d = Document::new(image.clone(), None);
        d.add_layer("smart".into(), image, (0, 0));
        let at = d.active;
        assert!(d.convert_to_smart(&[at]));
        let at = d.active;
        let set = vec![crate::document::SmartFilter::new(filter)];
        assert!(d.edit_smart(at, "Filter".into(), |s| s.filters = set.clone()));
        let flat = d.flatten();
        let path = dir.join(format!("{kind:?}.atelier"));
        d.save(&path).expect("saved");
        let back = Document::open(&path).expect("read").doc;
        let s = back.layers[at].smart.as_ref().expect("still smart");
        assert_eq!(s.filters.len(), 1, "{kind:?}");
        assert_eq!(s.filters[0].filter, set[0].filter, "{kind:?}");
        assert_eq!(back.flatten(), flat, "{kind:?}");
        assert_eq!(
            s.filters[0].filter,
            crate::document::SmartFilterKind::from(filter),
            "{kind:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// -------------------------------------------------------- Whole documents

#[test]
fn pattern_and_new_adjustments_survive_both_formats_with_the_same_look() {
    let image = RgbaImage::from_fn(60, 40, |x, y| {
        Rgba([(x * 4) as u8, (y * 6) as u8, 120, 255])
    });
    let mut d = Document::new(image, None);
    // The Pattern Fill, half covering.
    let fill = PatternFill {
        scale: 2.0,
        ..PatternFill::of(tile())
    };
    d.add_fill_layer(Fill::Pattern(fill.clone()), None);
    d.active_layer_mut().opacity = 0.5;
    // Pattern Overlay.
    d.add_layer(
        "fx".into(),
        RgbaImage::from_pixel(20, 20, Rgba(WHITE)),
        (10, 10),
    );
    let fx = &mut d.active_layer_mut().effects;
    fx.pattern_overlay.enabled = true;
    fx.pattern_overlay.fill = PatternFill::of(tile());
    fx.pattern_overlay.opacity = 0.75;
    // Adjustments.
    let exposure = Adjustment::Exposure(Exposure {
        exposure: 0.5,
        offset: 0.01,
        gamma: 0.9,
    });
    let mixer = Adjustment::ChannelMixer(ChannelMixer {
        rows: [
            [0.5, 0.3, 0.2, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.1, 0.0, 0.9, 0.02],
        ],
        monochrome: false,
    });
    let selective = Adjustment::SelectiveColor({
        let mut s = SelectiveColor::default();
        s.ranges[0] = [0.2, 0.0, -0.1, 0.0];
        s.ranges[8] = [0.0, 0.1, 0.0, 0.05];
        s
    });
    for a in [&exposure, &mixer, &selective] {
        d.add_adjustment_layer(a.clone(), None);
    }
    let look = d.flatten();
    let dir = temp("round-trip");
    for file in ["all.atelier", "all.psd"] {
        let path = dir.join(file);
        let notes = d.save(&path).expect("saved");
        assert!(notes.is_empty(), "{file}: {notes:?}");
        let back = Document::open(&path).expect("read").doc;
        assert!(back.file_notes.is_empty(), "{file}: {:?}", back.file_notes);
        assert_eq!(back.layers.len(), d.layers.len(), "{file}");
        assert!(
            matches!(&back.layers[1].kind, LayerKind::Fill(Fill::Pattern(f)) if *f == fill),
            "{file}: {:?}",
            back.layers[1].kind
        );
        assert_eq!(
            back.layers[2].effects.pattern_overlay, d.layers[2].effects.pattern_overlay,
            "{file}"
        );
        for (i, a) in [&exposure, &mixer, &selective].into_iter().enumerate() {
            let LayerKind::Adjustment(got) = &back.layers[3 + i].kind else {
                panic!("{file}: {:?}", back.layers[3 + i].kind);
            };
            assert_eq!(got, a, "{file}");
        }
        assert_eq!(back.flatten(), look, "{file}");
        // A second generation doesn't drift.
        let mut back = back;
        let again = dir.join(format!("again-{file}"));
        back.save(&again).expect("saved again");
        assert_eq!(
            Document::open(&again).expect("read").doc.flatten(),
            look,
            "{file}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
