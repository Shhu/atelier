//! Retouching tools driven through the app: their keys, the Clone Stamp's
//! source and alignment, the healing brushes, the Patch and Red Eye tools,
//! Content-Aware Fill.

use super::retouch_tools::{FillRequest, FillSettings, Output, PatchMode, SamplingArea};
use super::*;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

const RED: [u8; 4] = [220, 30, 30, 255];
const BLUE: [u8; 4] = [30, 60, 220, 255];

/// A document of `image` at 100 %, with `tool`.
fn app_with(image: RgbaImage, tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(image.clone(), None));
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    app.set_tool(tool);
    app.brush_size = 10.0;
    app.hardness = 1.0;
    h.run_steps(3);
    h
}

/// 200 × 100: red on the left half, blue on the right.
fn halves() -> RgbaImage {
    RgbaImage::from_fn(200, 100, |x, _| Rgba(if x < 100 { RED } else { BLUE }))
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn pixel(h: &Harness<'static, AtelierApp>, x: i64, y: i64) -> [u8; 4] {
    doc(h).active_layer().pixel_at(x, y).unwrap()
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state().last_view.unwrap().to_screen(pos2(x, y))
}

fn button(h: &mut Harness<'static, AtelierApp>, at: Pos2, pressed: bool, m: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: m,
    });
    h.run_steps(1);
}

/// A click at canvas point (x, y), with `m` held.
fn click(h: &mut Harness<'static, AtelierApp>, (x, y): (f32, f32), m: Modifiers) {
    let at = screen(h, x, y);
    h.event(egui::Event::ModifiersChanged(m));
    h.hover_at(at);
    h.run_steps(1);
    button(h, at, true, m);
    button(h, at, false, m);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

/// A drag through `path` (canvas coordinates).
fn drag(h: &mut Harness<'static, AtelierApp>, path: &[(f32, f32)]) {
    let start = screen(h, path[0].0, path[0].1);
    h.hover_at(start);
    h.run_steps(1);
    button(h, start, true, Modifiers::NONE);
    let mut end = start;
    for &(x, y) in &path[1..] {
        end = screen(h, x, y);
        h.hover_at(end);
        h.run_steps(1);
    }
    button(h, end, false, Modifiers::NONE);
    h.run_steps(1);
}

/// Waits for a fill computed on another thread.
fn wait_for_fill(h: &mut Harness<'static, AtelierApp>) {
    for _ in 0..3000 {
        if !h.state().retouch.busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run_steps(1);
    }
    panic!("the fill never ended");
}

/// Undoes the last step: the image is back to `before`, and there was a
/// single step.
fn undo_once(h: &mut Harness<'static, AtelierApp>, before: &RgbaImage) {
    let doc = h.state_mut().doc.as_mut().unwrap();
    assert!(doc.undo());
    assert_eq!(doc.active_layer().image(), before);
    assert!(!doc.can_undo(), "one step");
}

#[test]
fn keys_s_and_j() {
    let mut h = app_with(halves(), Tool::Brush);
    let press = |h: &mut Harness<'static, AtelierApp>, m: Modifiers, key: Key| {
        h.key_press_modifiers(m, key);
        h.run_steps(1);
        h.state().tool
    };
    assert!(press(&mut h, Modifiers::NONE, Key::S) == Tool::CloneStamp);
    assert!(press(&mut h, Modifiers::NONE, Key::J) == Tool::SpotHealing);
    assert!(press(&mut h, Modifiers::SHIFT, Key::J) == Tool::HealingBrush);
    assert!(press(&mut h, Modifiers::SHIFT, Key::J) == Tool::Patch);
    assert!(press(&mut h, Modifiers::SHIFT, Key::J) == Tool::RedEye);
    assert!(press(&mut h, Modifiers::SHIFT, Key::J) == Tool::SpotHealing);
}

/// No source: the Clone Stamp refuses to paint. Alt + click sets it; with
/// Aligned, the offset of the first stroke holds for the next ones.
#[test]
fn clone_stamp_copies_its_source() {
    let mut h = app_with(halves(), Tool::CloneStamp);
    drag(&mut h, &[(150.0, 50.0), (160.0, 50.0)]);
    assert_eq!(pixel(&h, 155, 50), BLUE);
    assert!(!doc(&h).can_undo());
    assert!(!h.state().status.is_empty(), "says why");

    click(&mut h, (30.0, 50.0), Modifiers::ALT);
    assert!(!doc(&h).can_undo(), "setting the source changes nothing");
    let before = doc(&h).active_layer().image().clone();
    drag(&mut h, &[(130.0, 50.0), (140.0, 50.0)]);
    assert_eq!(pixel(&h, 135, 50), RED);
    assert_eq!(pixel(&h, 135, 20), BLUE);
    // Aligned: (180, 20) copies (80, 20), still red.
    drag(&mut h, &[(180.0, 20.0), (185.0, 20.0)]);
    assert_eq!(pixel(&h, 182, 20), RED);
    // Not aligned: from the source point again; at (195, 80) the copy
    // comes from (30, 80).
    h.state_mut().retouch.clone_aligned = false;
    let image = doc(&h).active_layer().image().clone();
    drag(&mut h, &[(195.0, 80.0), (197.0, 80.0)]);
    assert_eq!(pixel(&h, 196, 80), RED);
    assert_ne!(doc(&h).active_layer().image(), &image);
    // Each stroke is a step.
    let doc = h.state_mut().doc.as_mut().unwrap();
    for _ in 0..3 {
        assert!(doc.undo());
    }
    assert_eq!(doc.active_layer().image(), &before);
}

/// Healing a flat blue area from a striped red one: the stripes come, the
/// blue stays.
#[test]
fn healing_brush_blends_the_texture() {
    let image = RgbaImage::from_fn(200, 100, |x, _| {
        Rgba(if x < 100 {
            if x % 6 < 3 {
                [240, 40, 40, 255]
            } else {
                [160, 20, 20, 255]
            }
        } else {
            BLUE
        })
    });
    let mut h = app_with(image.clone(), Tool::HealingBrush);
    h.state_mut().brush_size = 24.0;
    click(&mut h, (40.0, 50.0), Modifiers::ALT);
    drag(&mut h, &[(140.0, 50.0), (150.0, 50.0), (160.0, 50.0)]);
    let row: Vec<[u8; 4]> = (145..156).map(|x| pixel(&h, x, 50)).collect();
    assert!(
        row.iter().all(|p| p[2] > 150 && p[0] < 120),
        "blue stays: {row:?}"
    );
    let reds: Vec<u8> = row.iter().map(|p| p[0]).collect();
    let spread = reds.iter().max().unwrap() - reds.iter().min().unwrap();
    assert!(spread > 30, "the stripes come: {reds:?}");
    // Around the stroke: untouched.
    assert_eq!(pixel(&h, 150, 20), BLUE);
    undo_once(&mut h, &image);
}

/// A dark spot on a gradient is painted over: it is gone, the area matches
/// its surroundings.
#[test]
fn spot_healing_removes_a_blemish() {
    let image = RgbaImage::from_fn(200, 100, |x, y| {
        let v = 120 + (x / 4) as u8;
        let spot = (x as i32 - 100).pow(2) + (y as i32 - 50).pow(2) < 16;
        Rgba(if spot {
            [10, 10, 10, 255]
        } else {
            [v, v, 200, 255]
        })
    });
    let mut h = app_with(image.clone(), Tool::SpotHealing);
    h.state_mut().brush_size = 14.0;
    drag(&mut h, &[(99.0, 50.0), (101.0, 50.0)]);
    assert!(!h.state().retouch.busy(), "small: at once");
    for (x, y) in [(100, 50), (98, 49), (102, 51)] {
        let p = pixel(&h, x, y);
        let v = 120 + (x / 4) as i32;
        assert!(
            (p[0] as i32 - v).abs() < 20 && p[2] > 170,
            "({x}, {y}): {p:?}"
        );
    }
    undo_once(&mut h, &image);
}

/// Patch tool: a selection around a blemish dragged to a clean area is
/// patched from there (Source); with Destination, the clean area is patched
/// with the selection, which follows.
#[test]
fn patch_tool() {
    let image = RgbaImage::from_fn(200, 100, |x, y| {
        let blemish = (40..50).contains(&x) && (40..50).contains(&y);
        Rgba(if blemish {
            [0, 0, 0, 255]
        } else {
            [90, 160, (x / 2) as u8, 255]
        })
    });
    let mut h = app_with(image.clone(), Tool::Patch);
    // A freehand selection around the blemish.
    drag(
        &mut h,
        &[
            (35.0, 35.0),
            (55.0, 35.0),
            (55.0, 55.0),
            (35.0, 55.0),
            (35.0, 36.0),
        ],
    );
    assert!(h.state().selection.is_some());
    drag(&mut h, &[(45.0, 45.0), (95.0, 45.0), (125.0, 45.0)]);
    let p = pixel(&h, 45, 45);
    assert!(p[0] > 70 && p[1] > 140, "patched: {p:?}");
    assert_eq!(pixel(&h, 10, 10), image.get_pixel(10, 10).0);
    // The patch, then the selection drawn: one step each.
    let doc = h.state_mut().doc.as_mut().unwrap();
    assert!(doc.undo());
    assert_eq!(doc.active_layer().image(), &image);
    assert_eq!(doc.history().1, 1);
    assert!(doc.selection().is_some());

    h.state_mut().retouch.patch_mode = PatchMode::Destination;
    let rect = h.state().selection.as_ref().unwrap().rect;
    drag(&mut h, &[(45.0, 45.0), (95.0, 45.0), (125.0, 45.0)]);
    let p = pixel(&h, 125, 45);
    assert!(p[0] < 60, "the blemish copied: {p:?}");
    assert_eq!(h.state().selection.as_ref().unwrap().rect.x, rect.x + 80);
}

#[test]
fn red_eye_tool() {
    let image = RgbaImage::from_fn(200, 100, |x, y| {
        let d = (x as i32 - 100).pow(2) + (y as i32 - 50).pow(2);
        Rgba(if d < 36 {
            [210, 30, 40, 255]
        } else {
            [225, 180, 150, 255]
        })
    });
    let mut h = app_with(image.clone(), Tool::RedEye);
    click(&mut h, (20.0, 20.0), Modifiers::NONE);
    assert!(!doc(&h).can_undo(), "no red there");
    click(&mut h, (101.0, 50.0), Modifiers::NONE);
    let p = pixel(&h, 100, 50);
    assert!(p[0] < 60, "{p:?}");
    undo_once(&mut h, &image);
}

/// Edit › Content-Aware Fill on a selection in stripes: filled in the
/// background with stripes, one step; Output To New Layer leaves the layer
/// alone.
#[test]
fn content_aware_fill() {
    let stripes = |y: u32| {
        if y % 8 < 4 {
            [40, 90, 200, 255]
        } else {
            [230, 230, 220, 255]
        }
    };
    let image = RgbaImage::from_fn(200, 100, |x, y| {
        let hole = (80..120).contains(&x) && (30..70).contains(&y);
        Rgba(if hole { RED } else { stripes(y) })
    });
    let mut h = app_with(image.clone(), Tool::Select);
    h.state_mut().run(Action::ContentAwareFill);
    assert!(h.state().retouch.dialog.is_none(), "needs a selection");
    let rect = PixelRect {
        x: 78,
        y: 28,
        w: 44,
        h: 44,
    };
    h.state_mut().selection = Some(rect.into());
    h.state_mut().run(Action::ContentAwareFill);
    assert!(h.state().retouch.dialog.is_some());
    let mut settings = h.state().retouch.dialog.unwrap();
    settings.ai = false;
    let request = |settings| FillRequest {
        settings,
        opacity: 1.0,
        keep_alpha: false,
        from_fill: false,
    };
    h.state_mut().retouch.dialog = None;
    h.state_mut().start_content_aware(request(settings));
    wait_for_fill(&mut h);
    let wrong = (30..70)
        .flat_map(|y| (80..120).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (p, q) = (pixel(&h, x, y), stripes(y as u32));
            (0..3).map(|c| p[c].abs_diff(q[c]) as u32).sum::<u32>() > 90
        })
        .count();
    assert!(wrong < 80, "{wrong} pixels off the stripes");
    undo_once(&mut h, &image);

    let new_layer = FillSettings {
        output: Output::NewLayer,
        area: SamplingArea::Layer,
        ..settings
    };
    h.state_mut().start_content_aware(request(new_layer));
    wait_for_fill(&mut h);
    let doc = doc(&h);
    assert_eq!(doc.layers.len(), 2);
    assert_eq!(doc.layers[0].image(), &image);
    assert_eq!(doc.active_layer().pixel_at(5, 5).unwrap()[3], 0);
    assert_eq!(doc.active_layer().pixel_at(100, 50).unwrap()[3], 255);
}

/// Fill… with Content-Aware: needs a selection; fills it.
#[test]
fn fill_dialog_content_aware() {
    let image = RgbaImage::from_fn(200, 100, |x, _| {
        Rgba(if (90..110).contains(&x) { RED } else { BLUE })
    });
    let mut h = app_with(image, Tool::Select);
    h.state_mut().retouch.settings.ai = false;
    h.state_mut().selection = Some(
        PixelRect {
            x: 88,
            y: 0,
            w: 24,
            h: 100,
        }
        .into(),
    );
    h.state_mut().run(Action::FillDialog);
    h.state_mut().fill_dialog.as_mut().unwrap().content = FillContent::ContentAware;
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(2);
    wait_for_fill(&mut h);
    assert!(h.state().fill_dialog.is_none());
    assert_eq!(pixel(&h, 100, 50), BLUE);
}

/// With the AI model (it must be downloaded): the selection is painted in
/// by it, in one step.
#[test]
#[ignore = "needs the LaMa model"]
fn content_aware_fill_with_the_ai_model() {
    let stripes = |y: u32| {
        if y % 16 < 8 {
            [40, 90, 200, 255]
        } else {
            [230, 230, 220, 255]
        }
    };
    let image = RgbaImage::from_fn(300, 200, |x, y| {
        let hole = (120..180).contains(&x) && (70..130).contains(&y);
        Rgba(if hole { RED } else { stripes(y) })
    });
    let mut h = app_with(image.clone(), Tool::Select);
    h.state_mut().selection = Some(
        PixelRect {
            x: 118,
            y: 68,
            w: 64,
            h: 64,
        }
        .into(),
    );
    let settings = FillSettings {
        area: SamplingArea::Auto,
        adapt: true,
        output: Output::CurrentLayer,
        ai: true,
    };
    h.state_mut().start_content_aware(FillRequest {
        settings,
        opacity: 1.0,
        keep_alpha: false,
        from_fill: false,
    });
    assert!(h.state().retouch.ai_busy());
    wait_for_fill(&mut h);
    // The model ran.
    assert!(!crate::ai::models::Model::Inpaint.devices().is_empty());
    let red = (70..130)
        .flat_map(|y| (120..180).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(&h, x, y)[1] < 60)
        .count();
    assert!(red < 50, "{red} red pixels left");
    undo_once(&mut h, &image);
}

/// Captures of the retouching tools (offscreen render): `cargo test
/// capture_retouch -- --ignored`, into the folder `ATELIER_CAPTURE_DIR`.
#[test]
#[ignore = "needs a graphics renderer"]
fn capture_retouch() {
    let dir = std::path::PathBuf::from(std::env::var("ATELIER_CAPTURE_DIR").unwrap_or(".".into()));
    if std::env::var("ATELIER_LANG").is_ok_and(|l| l == "fr") {
        i18n::set_lang(Lang::Fr);
    }
    let shot = |name: &str, tool: Tool, setup: &dyn Fn(&mut Harness<'static, AtelierApp>)| {
        let mut h = Harness::builder()
            .with_size([1500.0, 900.0])
            .wgpu()
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                app.doc = Some(Document::new(super::object_select::demo_photo(), None));
                app
            });
        h.state_mut().set_tool(tool);
        h.state_mut().brush_size = 60.0;
        h.run_steps(3);
        setup(&mut h);
        h.run_steps(3);
        let image = h.render().expect("render");
        image
            .save(dir.join(format!("{name}.png")))
            .expect("writing the capture");
    };
    shot("clone-hover", Tool::CloneStamp, &|h| {
        click(h, (150.0, 150.0), Modifiers::ALT);
        h.hover_at(screen(h, 420.0, 260.0));
    });
    shot("clone-painting", Tool::CloneStamp, &|h| {
        click(h, (150.0, 150.0), Modifiers::ALT);
        let start = screen(h, 420.0, 260.0);
        h.hover_at(start);
        h.run_steps(1);
        button(h, start, true, Modifiers::NONE);
        for i in 1..6 {
            h.hover_at(screen(h, 420.0 + 12.0 * i as f32, 260.0));
            h.run_steps(1);
        }
    });
    shot("spot", Tool::SpotHealing, &|h| {
        h.hover_at(screen(h, 300.0, 200.0))
    });
    shot("heal", Tool::HealingBrush, &|_| {});
    shot("patch", Tool::Patch, &|h| {
        h.state_mut().selection = Some(
            PixelRect {
                x: 200,
                y: 150,
                w: 120,
                h: 90,
            }
            .into(),
        );
        h.run_steps(1);
        let start = screen(h, 260.0, 190.0);
        h.hover_at(start);
        h.run_steps(1);
        button(h, start, true, Modifiers::NONE);
        for i in 1..6 {
            h.hover_at(screen(h, 260.0 + 30.0 * i as f32, 190.0));
            h.run_steps(1);
        }
    });
    shot("red-eye", Tool::RedEye, &|_| {});
    shot("caf", Tool::Select, &|h| {
        h.state_mut().selection = Some(
            PixelRect {
                x: 200,
                y: 150,
                w: 120,
                h: 90,
            }
            .into(),
        );
        h.state_mut().run(Action::ContentAwareFill);
    });
    shot("fill", Tool::Select, &|h| {
        h.state_mut().run(Action::FillDialog);
        h.state_mut().fill_dialog.as_mut().unwrap().content = FillContent::ContentAware;
    });
}

/// Timing of the brushes on a 6000 × 4000 image (`--ignored --nocapture`).
#[test]
#[ignore = "timing"]
fn large_image_timing() {
    let image = RgbaImage::from_fn(6000, 4000, |x, y| {
        Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
    });
    for tool in [Tool::CloneStamp, Tool::HealingBrush, Tool::SpotHealing] {
        let mut h = app_with(image.clone(), tool);
        h.state_mut().brush_size = 200.0;
        click(&mut h, (2700.0, 1800.0), Modifiers::ALT);
        let t = std::time::Instant::now();
        let path: Vec<(f32, f32)> = (0..12)
            .map(|i| (2900.0 + 25.0 * i as f32, 2000.0))
            .collect();
        drag(&mut h, &path);
        wait_for_fill(&mut h);
        assert!(doc(&h).can_undo());
        eprintln!(
            "{}: {:?} for {} frames",
            tool.name(),
            t.elapsed(),
            path.len() + 3
        );
    }
}
