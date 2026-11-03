//! Info, Histogram and Navigator: what they compute during presses, the
//! Info readings, Straighten Layer refusals, messages said again, where the
//! panels open.

use super::measure_tools::SampleSize;
use super::*;
use crate::document::{ColorSampler, Measure};
use egui_kittest::Harness;

type H = Harness<'static, AtelierApp>;

fn noise(w: u32, h: u32) -> image::RgbaImage {
    image::RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x ^ y) % 256) as u8, 255])
    })
}

fn app_with(image: image::RgbaImage, tool: Tool, size: [f32; 2], dt: f32) -> H {
    i18n::set_lang(Lang::En);
    let mut cell = Some(image);
    let mut h = Harness::builder()
        .with_size(size)
        .with_step_dt(dt)
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            app.doc = Some(Document::new(cell.take().expect("once"), None));
            app.tool = tool;
            app
        });
    h.run_steps(3);
    h
}

fn screen(h: &H, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    app.view_turn
        .to_screen(app.last_view.expect("canvas shown").to_screen(pos2(x, y)))
}

fn button(h: &mut H, at: Pos2, pressed: bool) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
}

fn press(h: &mut H, at: Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    button(h, at, true);
}

/// A document whose whole count takes a while: layers over a large canvas.
fn slow_to_count(tool: Tool) -> H {
    let mut h = app_with(noise(4000, 3000), tool, [1400.0, 1000.0], 0.02);
    let doc = h.state_mut().doc.as_mut().expect("a document");
    for i in 0..12 {
        doc.add_layer(format!("l{i}"), noise(3000, 2000), (i * 40, i * 30));
        doc.active_layer_mut().opacity = 0.5;
    }
    h.state_mut().hist_panel.open = true;
    h.run_steps(2);
    h
}

fn count_done(h: &mut H) {
    for _ in 0..500 {
        h.run_steps(1);
        let p = &h.state().hist_panel;
        if p.counts().is_some() && !p.counting() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the count never ended");
}

#[test]
fn a_pan_keeps_the_histogram_count_a_stroke_stops_it() {
    // The Hand tool pans: what is counted stays right.
    let mut h = slow_to_count(Tool::Hand);
    assert!(h.state().hist_panel.counting(), "a count started");
    let at = screen(&h, 2000.0, 1500.0);
    press(&mut h, at);
    h.hover_at(at + vec2(4.0, 0.0));
    h.run_steps(1);
    assert!(
        h.state().hist_panel.counting() && !h.state().hist_panel.cancelled(),
        "the pan doesn't cancel the count"
    );
    button(&mut h, at + vec2(4.0, 0.0), false);
    count_done(&mut h);
    let counted = h.state().hist_panel.counts().cloned();

    // A brush press may change the layers: the count gives them back.
    let mut h = slow_to_count(Tool::Brush);
    assert!(h.state().hist_panel.counting());
    let at = screen(&h, 100.0, 100.0);
    press(&mut h, at);
    assert!(
        h.state().hist_panel.cancelled(),
        "the stroke stops the count"
    );
    // And none starts while it is held.
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
        assert!(!h.state().hist_panel.counting() || h.state().hist_panel.cancelled());
    }
    button(&mut h, at, false);
    count_done(&mut h);
    assert!(counted.is_some());
}

/// A press held off the canvas (a slider, a menu) doesn't stop the panels.
#[test]
fn histogram_and_navigator_follow_the_image_while_a_button_is_held_elsewhere() {
    let mut h = app_with(noise(400, 300), Tool::Brush, [1400.0, 1000.0], 0.02);
    h.state_mut().hist_panel.open = true;
    h.state_mut().navigator.open = true;
    h.run_steps(2);
    count_done(&mut h);
    let before = h.state().hist_panel.counts().cloned();
    let at = pos2(700.0, 5.0);
    press(&mut h, at);
    let black = image::RgbaImage::from_pixel(400, 300, image::Rgba([0, 0, 0, 255]));
    let doc = h.state_mut().doc.as_mut().expect("a document");
    doc.add_layer("x".into(), black, (0, 0));
    let mut updated = (false, false);
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
        updated.0 |= h.state().hist_panel.counts().cloned() != before;
        updated.1 |= h.state_mut().navigator_current();
        if updated.0 && updated.1 {
            break;
        }
    }
    assert!(h.ctx.input(|i| i.pointer.primary_down()), "still held");
    assert_eq!(updated, (true, true), "Histogram, Navigator");
    button(&mut h, at, false);
}

/// The points are read from time to time during a stroke, not at each dab,
/// and at once after it.
#[test]
fn info_points_are_not_read_at_each_dab() {
    let mut h = app_with(noise(800, 600), Tool::Brush, [1400.0, 1000.0], 0.02);
    {
        let app = h.state_mut();
        app.info.open = true;
        app.measure.sample = SampleSize::Average11;
        let doc = app.doc.as_mut().expect("a document");
        doc.samplers = (0..10)
            .map(|i| ColorSampler {
                x: 40.5 + i as f32 * 70.0,
                y: 300.5,
            })
            .collect();
    }
    h.run_steps(2);
    let mut reads = 0;
    let mut last = h.state().info.samplers_at();
    let start = screen(&h, 20.0, 300.0);
    press(&mut h, start);
    for i in 1..=40 {
        h.hover_at(screen(&h, 20.0 + i as f32 * 18.0, 300.0));
        h.run_steps(1);
        let at = h.state().info.samplers_at();
        if at != last {
            reads += 1;
            last = at;
        }
    }
    // 40 frames of 20 ms: some reads, far fewer than dabs.
    assert!((1..=6).contains(&reads), "{reads} reads");
    let end = screen(&h, 740.0, 300.0);
    button(&mut h, end, false);
    h.run_steps(1);
    let shown = h.state_mut().sampler_readings();
    let doc = h.state().doc.as_ref().expect("a document");
    let fresh: Vec<_> = doc
        .samplers
        .iter()
        .map(|s| doc.sampled(s.pixel(), 5))
        .collect();
    assert_eq!(
        shown.iter().map(|r| r.map(|r| r.rgb)).collect::<Vec<_>>(),
        fresh
    );
}

/// The pointer's reading averages over the Sample Size, as the points do.
#[test]
fn the_pointer_reading_follows_the_sample_size() {
    // Red on the left half, blue on the right.
    let image = image::RgbaImage::from_fn(400, 300, |x, _| {
        image::Rgba(if x < 200 {
            [200, 20, 10, 255]
        } else {
            [10, 20, 200, 255]
        })
    });
    let mut h = app_with(image, Tool::Brush, [1400.0, 1000.0], 0.25);
    h.state_mut().info.open = true;
    h.run_steps(1);
    h.hover_at(screen(&h, 200.5, 10.5));
    h.run_steps(2);
    assert_eq!(h.state().hover_pixel, Some((200, 10)));
    let point = h.state_mut().cursor_reading().expect("a color").rgb;
    assert_eq!(point, [10, 20, 200]);
    h.state_mut().measure.sample = SampleSize::Average3;
    h.run_steps(1);
    let averaged = h.state_mut().cursor_reading().expect("a color").rgb;
    assert_eq!(averaged, [73, 20, 137], "one red column of three");
}

/// A hidden layer isn't turned, nor the canvas cropped: said, no step.
#[test]
fn straighten_layer_refuses_a_hidden_layer() {
    let mut h = app_with(noise(400, 300), Tool::Ruler, [1400.0, 1000.0], 0.25);
    {
        let doc = h.state_mut().doc.as_mut().expect("a document");
        doc.add_layer("c".into(), noise(200, 100), (50, 50));
        let i = doc.active;
        doc.layers[i].visible = false;
        doc.ruler = Some(Measure {
            vertex: (10.0, 10.0),
            first: (200.0, 40.0),
            second: None,
        });
    }
    h.run_steps(1);
    let steps = h.state().doc.as_ref().expect("a document").history().1;
    h.state_mut().straighten_layer(false);
    h.run_steps(5);
    let app = h.state();
    let doc = app.doc.as_ref().expect("a document");
    assert_eq!((doc.width(), doc.height()), (400, 300));
    assert_eq!(doc.history().1, steps);
    assert!(doc.ruler.is_some(), "the line stays");
    assert_eq!(app.status, t!("fx4-layer-hidden", name = "c"));
}

/// The same message asked for again, once faded, is shown again (status
/// bar and Full Screen).
#[test]
fn a_message_said_again_is_shown_again() {
    let mut h = app_with(noise(400, 300), Tool::Ruler, [1400.0, 1000.0], 0.25);
    h.state_mut().doc.as_mut().expect("a document").ruler = Some(Measure {
        vertex: (10.0, 10.0),
        first: (200.0, 10.0),
        second: None,
    });
    h.state_mut().set_screen_mode(workspace::ScreenMode::Full);
    h.run_steps(2);
    let level = t!("panels9-status-already-level");
    let shown = |h: &mut H| {
        let ctx = h.ctx.clone();
        let notices = h.state_mut().full_screen_notices(&ctx);
        notices.iter().any(|n| n.0 == level && n.1 > 0.0)
    };
    h.state_mut().straighten_layer(false);
    h.run_steps(1);
    assert!(shown(&mut h), "first time");
    // Faded (kittest time).
    h.run_steps(40);
    assert!(!shown(&mut h), "faded");
    h.state_mut().doc.as_mut().expect("a document").ruler = Some(Measure {
        vertex: (10.0, 10.0),
        first: (200.0, 10.0),
        second: None,
    });
    h.state_mut().straighten_layer(false);
    h.run_steps(1);
    assert!(shown(&mut h), "said again");
}

/// Info (with points and a measure), Histogram and Navigator opened
/// together: every title bar stays in sight, and where the room allows
/// (1024 px wide, two columns) they cover little of each other: a panel
/// placed low is shortened rather than laid over another.
#[test]
fn info_histogram_and_navigator_open_without_covering_each_other() {
    // The most height of a panel covered by another.
    for (size, most) in [([1024.0, 700.0], 100.0), ([660.0, 500.0], f32::INFINITY)] {
        let mut h = app_with(noise(512, 512), Tool::Ruler, size, 0.25);
        {
            let app = h.state_mut();
            app.run(Action::Workspace(workspace::Command::Info));
            app.run(Action::Workspace(workspace::Command::Histogram));
            app.run(Action::Workspace(workspace::Command::Navigator));
            let doc = app.doc.as_mut().expect("a document");
            doc.samplers = (0..4)
                .map(|i| ColorSampler {
                    x: 50.5 + 80.0 * i as f32,
                    y: 80.5,
                })
                .collect();
            doc.ruler = Some(Measure {
                vertex: (60.0, 240.0),
                first: (360.0, 150.0),
                second: Some((300.0, 280.0)),
            });
        }
        for _ in 0..30 {
            h.run_steps(2);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let ids = ["info", "histogram", "navigator"];
        let rect = |id: &str| {
            h.ctx
                .memory(|m| m.area_rect(egui::Id::new(id)))
                .unwrap_or_else(|| panic!("{id} shown"))
        };
        let order = |id: &str| {
            h.ctx
                .memory(|m| m.layer_ids().position(|l| l.id == egui::Id::new(id)))
        };
        for (i, j) in [(0, 1), (0, 2), (1, 2)] {
            let (a, b) = (rect(ids[i]), rect(ids[j]));
            let (behind, front) = if order(ids[i]) < order(ids[j]) {
                (a, b)
            } else {
                (b, a)
            };
            // Its title, as a cascade leaves it.
            let title = Rect::from_min_max(behind.min, pos2(behind.max.x, behind.min.y + 27.0));
            assert!(
                !front.intersects(title.shrink(0.5)),
                "{size:?}: a title bar covered: {behind:?} under {front:?}"
            );
            let both = a.intersect(b);
            let covered = if both.is_positive() {
                both.height()
            } else {
                0.0
            };
            assert!(
                covered <= most,
                "{size:?}: {a:?} and {b:?} overlap by {covered}"
            );
        }
    }
}
