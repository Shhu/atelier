//! Color Sampler and Ruler tools, Info, Histogram and Navigator panels,
//! screen modes.

use super::measure_tools::SampleSize;
use super::*;
use crate::document::{ColorSampler, MAX_SAMPLERS, Measure};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

const RED: [u8; 4] = [200, 20, 10, 255];
const BLUE: [u8; 4] = [10, 20, 200, 255];

/// Red on the left half, blue on the right.
fn image() -> image::RgbaImage {
    image::RgbaImage::from_fn(400, 300, |x, _| {
        image::Rgba(if x < 200 { RED } else { BLUE })
    })
}

fn app_sized(tool: Tool, size: [f32; 2]) -> Harness<'static, AtelierApp> {
    i18n::set_lang(Lang::En);
    let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
        let mut app = AtelierApp::new(cc, None);
        app.doc = Some(Document::new(image(), None));
        app.tool = tool;
        app
    });
    h.run_steps(3);
    h
}

fn app(tool: Tool) -> Harness<'static, AtelierApp> {
    app_sized(tool, [1400.0, 1000.0])
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

fn steps(h: &Harness<'static, AtelierApp>) -> usize {
    doc(h).history().1
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    let app = h.state();
    let view = app.last_view.expect("canvas shown");
    app.view_turn.to_screen(view.to_screen(pos2(x, y)))
}

fn click(h: &mut Harness<'static, AtelierApp>, at: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(at);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers,
        });
        h.run_steps(1);
    }
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
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

/// Until `done`, the workers having time to finish.
fn wait(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in 0..400 {
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.run_steps(1);
    }
    panic!("not done in time");
}

/// The panel's window is wholly on screen.
fn on_screen(h: &Harness<'static, AtelierApp>, id: &str) {
    let window = h.ctx.content_rect();
    let rect = h
        .ctx
        .memory(|m| m.area_rect(egui::Id::new(id)))
        .unwrap_or_else(|| panic!("{id} not shown"));
    assert!(window.contains_rect(rect), "{id}: {rect:?} in {window:?}");
}

// ------------------------------------------------------------ Color Sampler

#[test]
fn i_groups_the_sampler_and_the_ruler_with_the_eyedropper() {
    let mut h = app(Tool::Brush);
    h.key_press(Key::I);
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::Eyedropper);
    h.key_press_modifiers(Modifiers::SHIFT, Key::I);
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::ColorSampler);
    h.key_press_modifiers(Modifiers::SHIFT, Key::I);
    h.run_steps(1);
    assert_eq!(h.state().tool, Tool::Ruler);
}

#[test]
fn a_click_adds_a_point_a_drag_moves_it_alt_click_removes_it_each_one_step() {
    let mut h = app(Tool::ColorSampler);
    let before = steps(&h);
    let at = screen(&h, 50.2, 60.7);
    click(&mut h, at, Modifiers::NONE);
    assert_eq!(doc(&h).samplers, vec![ColorSampler { x: 50.5, y: 60.5 }]);
    assert_eq!(steps(&h), before + 1);
    // Moved.
    let (from, to) = (screen(&h, 50.5, 60.5), screen(&h, 300.5, 100.5));
    drag(&mut h, from, to);
    assert_eq!(doc(&h).samplers.len(), 1);
    let s = doc(&h).samplers[0];
    assert_eq!(s.pixel(), (300, 100));
    assert_eq!(steps(&h), before + 2);
    // Undone: back where it was.
    h.state_mut().doc.as_mut().unwrap().undo();
    assert_eq!(doc(&h).samplers[0].pixel(), (50, 60));
    h.state_mut().doc.as_mut().unwrap().redo();
    // Alt+click: gone, one step.
    let at = screen(&h, 300.5, 100.5);
    click(&mut h, at, Modifiers::ALT);
    assert!(doc(&h).samplers.is_empty());
    assert_eq!(steps(&h), before + 3);
}

#[test]
fn a_point_dragged_off_the_canvas_goes() {
    let mut h = app(Tool::ColorSampler);
    let at = screen(&h, 50.5, 50.5);
    click(&mut h, at, Modifiers::NONE);
    let to = screen(&h, -20.0, 50.0);
    drag(&mut h, at, to);
    assert!(doc(&h).samplers.is_empty());
}

#[test]
fn outside_the_canvas_no_point_is_added_and_an_eleventh_is_refused() {
    let mut h = app(Tool::ColorSampler);
    let before = steps(&h);
    let off = screen(&h, -30.0, 40.0);
    click(&mut h, off, Modifiers::NONE);
    assert!(doc(&h).samplers.is_empty());
    assert_eq!(steps(&h), before);
    for i in 0..MAX_SAMPLERS {
        let at = screen(&h, 20.0 + i as f32 * 30.0, 40.0);
        click(&mut h, at, Modifiers::NONE);
    }
    assert_eq!(doc(&h).samplers.len(), 10);
    let done = steps(&h);
    let at = screen(&h, 20.0, 200.0);
    click(&mut h, at, Modifiers::NONE);
    assert_eq!(doc(&h).samplers.len(), 10);
    assert_eq!(steps(&h), done);
    assert_eq!(
        h.state().status,
        t!("panels8-status-samplers-full", count = 10)
    );
}

#[test]
fn shift_click_with_the_eyedropper_adds_a_point_and_leaves_the_colors() {
    let mut h = app(Tool::Eyedropper);
    let primary = h.state().primary;
    let at = screen(&h, 10.0, 10.0);
    click(&mut h, at, Modifiers::SHIFT);
    assert_eq!(doc(&h).samplers.len(), 1);
    assert_eq!(h.state().primary, primary);
}

#[test]
fn clear_all_is_one_step_and_undoes() {
    let mut h = app(Tool::ColorSampler);
    for x in [10.0, 40.0] {
        let at = screen(&h, x, 40.0);
        click(&mut h, at, Modifiers::NONE);
    }
    let before = steps(&h);
    h.get_by_label(&t!("panels8-clear-all")).click();
    h.run_steps(2);
    assert!(doc(&h).samplers.is_empty());
    assert_eq!(steps(&h), before + 1);
    h.state_mut().doc.as_mut().unwrap().undo();
    assert_eq!(doc(&h).samplers.len(), 2);
}

#[test]
fn points_read_what_is_shown_averaged_by_the_sample_size() {
    let mut h = app(Tool::ColorSampler);
    let doc_mut = h.state_mut().doc.as_mut().unwrap();
    doc_mut.samplers = vec![
        ColorSampler { x: 10.5, y: 10.5 },
        // On the seam: half red, half blue when averaged.
        ColorSampler { x: 200.5, y: 10.5 },
    ];
    let r = h.state_mut().sampler_readings();
    assert_eq!(r[0].unwrap().rgb, [200, 20, 10]);
    assert_eq!(r[1].unwrap().rgb, [10, 20, 200]);
    assert!(r[0].unwrap().cmyk.is_some());
    h.state_mut().measure.sample = SampleSize::Average3;
    let r = h.state_mut().sampler_readings();
    // Two columns of three are red.
    assert_eq!(r[0].unwrap().rgb, [200, 20, 10]);
    let seam = r[1].unwrap().rgb;
    assert_eq!(seam, [73, 20, 137]);
    // A layer put over them is read at once.
    let doc_mut = h.state_mut().doc.as_mut().unwrap();
    let white = image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]));
    doc_mut.add_layer("over".into(), white, (0, 0));
    let r = h.state_mut().sampler_readings();
    assert_eq!(r[0].unwrap().rgb, [255, 255, 255]);
}

#[test]
fn points_follow_crops_flips_and_turns_and_fall_off_with_them() {
    let mut d = Document::new(image(), None);
    d.samplers = vec![
        ColorSampler { x: 100.5, y: 50.5 },
        ColorSampler { x: 390.5, y: 290.5 },
    ];
    d.ruler = Some(Measure {
        vertex: (0.0, 0.0),
        first: (10.0, 10.0),
        second: None,
    });
    d.flip_canvas(true);
    assert_eq!(d.samplers[0], ColorSampler { x: 299.5, y: 50.5 });
    assert!(d.ruler.is_none(), "for what was there");
    d.undo();
    assert_eq!(d.samplers[0], ColorSampler { x: 100.5, y: 50.5 });
    assert!(d.rotate_canvas(90.0));
    assert_eq!(d.samplers[0], ColorSampler { x: 249.5, y: 100.5 });
    d.undo();
    // A crop leaves the first in, the second out.
    d.fit_canvas_to(PixelRect {
        x: 50,
        y: 0,
        w: 200,
        h: 200,
    });
    assert_eq!(d.samplers, vec![ColorSampler { x: 50.5, y: 50.5 }]);
}

// ------------------------------------------------------------ Ruler

#[test]
fn a_drag_measures_without_a_step_and_a_click_clears() {
    let mut h = app(Tool::Ruler);
    let before = steps(&h);
    let (a, b) = (screen(&h, 100.0, 100.0), screen(&h, 200.0, 0.0));
    drag(&mut h, a, b);
    let m = doc(&h).ruler.expect("a line");
    assert!((m.vertex.0 - 100.0).abs() < 1.0 && (m.first.0 - 200.0).abs() < 1.0);
    assert!((m.angle() - 45.0).abs() < 1.0, "{}", m.angle());
    assert!((m.length() - 141.4).abs() < 1.5);
    assert_eq!(steps(&h), before);
    // Shown by short names in the options bar.
    let values = h.state().measure_values().unwrap();
    assert!(
        values
            .iter()
            .any(|(n, v)| *n == t!("panels8-short-angle") && v.ends_with('°'))
    );
    h.get_by_label(&t!("panels8-straighten-layer"));
    // A click elsewhere: no more line.
    let off = screen(&h, 300.0, 250.0);
    click(&mut h, off, Modifiers::NONE);
    assert!(doc(&h).ruler.is_none());
    assert_eq!(steps(&h), before);
}

#[test]
fn the_ends_and_the_line_can_be_dragged_and_alt_makes_a_protractor() {
    let mut h = app(Tool::Ruler);
    let (a, b) = (screen(&h, 100.0, 100.0), screen(&h, 200.0, 100.0));
    drag(&mut h, a, b);
    // An end.
    let to = screen(&h, 200.0, 150.0);
    drag(&mut h, b, to);
    let m = doc(&h).ruler.unwrap();
    assert!((m.first.1 - 150.0).abs() < 1.0);
    assert!((m.vertex.1 - 100.0).abs() < 1.0);
    // The whole line: its length stays.
    let (from, by) = (screen(&h, 150.0, 125.0), screen(&h, 150.0, 175.0));
    drag(&mut h, from, by);
    let moved = doc(&h).ruler.unwrap();
    assert!((moved.vertex.1 - 150.0).abs() < 1.0, "{moved:?}");
    assert!((moved.length() - m.length()).abs() < 1.0);
    // Alt from the far end: its vertex, the angle between the arms.
    h.state_mut().doc.as_mut().unwrap().ruler = Some(Measure {
        vertex: (50.0, 100.0),
        first: (150.0, 100.0),
        second: None,
    });
    let (from, to) = (screen(&h, 150.0, 100.0), screen(&h, 150.0, 200.0));
    h.hover_at(from);
    h.run_steps(1);
    let raw_alt = Modifiers::ALT;
    h.event(egui::Event::ModifiersChanged(raw_alt));
    h.event(egui::Event::PointerButton {
        pos: from,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: raw_alt,
    });
    h.run_steps(1);
    h.hover_at(to);
    h.run_steps(2);
    h.event(egui::Event::PointerButton {
        pos: to,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: raw_alt,
    });
    h.run_steps(2);
    let m = doc(&h).ruler.unwrap();
    assert_eq!(m.vertex, (150.0, 100.0));
    let second = m.second.expect("a second arm");
    assert!((second.1 - 200.0).abs() < 1.5, "{second:?}");
    assert!(
        (m.angle() - 90.0).abs() < 1.5,
        "{m:?} {} {from:?} {to:?}",
        m.angle()
    );
    assert!(m.second_length().unwrap() > 90.0);
}

/// A horizontal bar on a layer, turned by the line through it.
fn straighten_setup(h: &mut Harness<'static, AtelierApp>, degrees: f32) {
    let d = h.state_mut().doc.as_mut().unwrap();
    let t = degrees.to_radians();
    d.ruler = Some(Measure {
        vertex: (100.0, 150.0),
        first: (100.0 + 100.0 * t.cos(), 150.0 - 100.0 * t.sin()),
        second: None,
    });
}

#[test]
fn straighten_layer_turns_crops_and_clears_in_one_step_and_undoes() {
    let mut h = app(Tool::Ruler);
    straighten_setup(&mut h, 10.0);
    let before = steps(&h);
    let first = doc(&h).active_layer().image().clone();
    h.state_mut().straighten_layer(false);
    wait(&mut h, |a| !a.transform_busy());
    h.run_steps(2);
    assert_eq!(h.state().tool, Tool::Ruler, "stays on its tool");
    assert!(doc(&h).ruler.is_none());
    assert_eq!(steps(&h), before + 1, "{:?}", doc(&h).history().0);
    // The canvas: what stayed filled, smaller, and it is all filled.
    let (w, h_) = (doc(&h).width(), doc(&h).height());
    assert!(w < 400 && h_ < 300 && w > 200 && h_ > 150, "{w}×{h_}");
    for (x, y) in [
        (1, 1),
        (w as i64 - 2, 1),
        (1, h_ as i64 - 2),
        (w as i64 - 2, h_ as i64 - 2),
    ] {
        assert!(doc(&h).color_at(x, y).is_some(), "{x},{y} is empty");
    }
    assert_ne!(
        doc(&h).active_layer().image().dimensions(),
        first.dimensions()
    );
    h.state_mut().doc.as_mut().unwrap().undo();
    assert_eq!((doc(&h).width(), doc(&h).height()), (400, 300));
    assert_eq!(
        doc(&h).active_layer().image().dimensions(),
        first.dimensions()
    );
}

#[test]
fn alt_straighten_keeps_the_canvas_and_a_selection_stays() {
    let mut h = app(Tool::Ruler);
    straighten_setup(&mut h, -8.0);
    let selection = Selection::from(PixelRect {
        x: 10,
        y: 10,
        w: 50,
        h: 50,
    });
    h.state_mut().selection = Some(selection.clone());
    h.state_mut().straighten_layer(true);
    wait(&mut h, |a| !a.transform_busy());
    h.run_steps(2);
    assert_eq!((doc(&h).width(), doc(&h).height()), (400, 300));
    assert_eq!(h.state().selection, Some(selection));
    // The whole layer turned, not the selected part: wider than the canvas.
    assert!(doc(&h).active_layer().image().width() > 400);
    assert_eq!(doc(&h).layers.len(), 1, "nothing lifted");
}

#[test]
fn straighten_is_refused_on_a_locked_layer_and_in_quick_mask_mode() {
    let mut h = app(Tool::Ruler);
    straighten_setup(&mut h, 5.0);
    let before = steps(&h);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locked = true;
    h.state_mut().straighten_layer(false);
    h.run_steps(2);
    assert_eq!(steps(&h), before);
    assert!(doc(&h).ruler.is_some(), "kept");
    assert_eq!(h.state().tool, Tool::Ruler);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locked = false;
    h.state_mut()
        .run(Action::Select(select_menu::Command::QuickMask));
    h.run_steps(2);
    let quick = steps(&h);
    h.state_mut().straighten_layer(false);
    h.run_steps(2);
    assert_eq!(steps(&h), quick);
    assert_eq!(h.state().status, t!("panels8-status-straighten-quick-mask"));
}

#[test]
fn arbitrary_rotation_opens_with_the_lines_angle() {
    let mut h = app(Tool::Ruler);
    straighten_setup(&mut h, -12.0);
    h.state_mut().run(Action::RotateArbitrary);
    let d = h.state().rotate_dialog.expect("the dialog");
    assert!((d.angle - 12.0).abs() < 0.01 && !d.clockwise);
}

#[test]
fn the_line_is_measured_on_a_turned_view() {
    let mut h = app(Tool::Ruler);
    h.state_mut().view_turn.angle = 90.0;
    h.run_steps(2);
    let (a, b) = (screen(&h, 100.0, 100.0), screen(&h, 200.0, 100.0));
    drag(&mut h, a, b);
    let m = doc(&h).ruler.expect("a line");
    assert!(m.angle().abs() < 1.5, "{}", m.angle());
    assert!((m.length() - 100.0).abs() < 2.0);
}

// ------------------------------------------------------------ Info

#[test]
fn f8_opens_info_with_what_is_under_the_pointer() {
    let mut h = app(Tool::Brush);
    h.key_press(Key::F8);
    h.run_steps(3);
    assert!(h.state().info.open);
    on_screen(&h, "info");
    let at = screen(&h, 10.0, 10.0);
    h.hover_at(at);
    h.run_steps(2);
    assert_eq!(h.state().hover_pixel, Some((10, 10)));
    let r = h.state_mut().cursor_reading().expect("a color");
    assert_eq!(r.rgb, [200, 20, 10]);
    let cmyk = r.cmyk.expect("in the working CMYK");
    assert!(cmyk[1] > 50.0, "{cmyk:?}");
    // Its values are written out.
    h.get_by_label("200");
    // The Doc line.
    assert!(
        h.get_all_by_label(&t!("panels8-info-size", width = 400, height = 300))
            .next()
            .is_some()
    );
    // Off the image: no value.
    let off = screen(&h, -30.0, 10.0);
    h.hover_at(off);
    h.run_steps(2);
    assert!(h.state_mut().cursor_reading().is_none());
    h.key_press(Key::F8);
    h.run_steps(1);
    assert!(!h.state().info.open);
}

#[test]
fn info_shows_the_selection_points_and_the_measure() {
    let mut h = app(Tool::Ruler);
    h.state_mut().selection = Some(Selection::from(PixelRect {
        x: 3,
        y: 4,
        w: 123,
        h: 45,
    }));
    let d = h.state_mut().doc.as_mut().unwrap();
    d.samplers = vec![ColorSampler { x: 10.5, y: 10.5 }];
    d.ruler = Some(Measure {
        vertex: (0.0, 0.0),
        first: (30.0, 40.0),
        second: None,
    });
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Info));
    h.run_steps(3);
    h.get_by_label("123");
    h.get_by_label("45");
    h.get_by_label("#1");
    h.get_by_label("200  20  10");
    assert!(h.get_all_by_label("50").next().is_some(), "the length");
}

// ------------------------------------------------------------ Histogram

fn histogram_open(tool: Tool) -> Harness<'static, AtelierApp> {
    let mut h = app(tool);
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Histogram));
    h.run_steps(1);
    h
}

fn counted(h: &mut Harness<'static, AtelierApp>) {
    wait(h, |a| {
        !a.hist_panel.counting() && a.hist_panel.counts().is_some()
    });
    h.run_steps(1);
    wait(h, |a| !a.hist_panel.counting());
}

#[test]
fn stats_of_known_levels() {
    let mut levels = [0u64; 256];
    levels[10] = 1;
    levels[20] = 2;
    levels[30] = 1;
    let s = histogram_panel::stats(&levels);
    assert_eq!(s.total, 4);
    assert_eq!(s.mean, 20.0);
    assert_eq!(s.median, 20);
    assert!((s.deviation - 50f64.sqrt()).abs() < 1e-9);
    assert_eq!(histogram_panel::stats(&[0; 256]).total, 0);
}

#[test]
fn it_counts_the_image_off_the_ui_thread_and_again_when_it_changes() {
    let mut h = histogram_open(Tool::Brush);
    counted(&mut h);
    on_screen(&h, "histogram");
    let c = h.state().hist_panel.counts().unwrap();
    assert_eq!(c.pixels, 120_000);
    assert_eq!(c.red[200], 60_000);
    assert_eq!(c.red[10], 60_000);
    assert_eq!(c.green[20], 120_000);
    // Of the luminosity of each side.
    let lum =
        |[r, g, b, _]: [u8; 4]| (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
    assert_eq!(c.luminosity[lum(RED) as usize], 60_000);
    assert_eq!(c.of(histogram_panel::Channel::Rgb)[20], 120_000);
    // The mean is written out.
    h.get_by_label("120000");
    // A change: counted again, the old result shown meanwhile.
    let white = image::RgbaImage::from_pixel(400, 300, image::Rgba([255, 255, 255, 255]));
    let d = h.state_mut().doc.as_mut().unwrap();
    d.checkpoint("over".to_owned());
    d.add_layer("over".into(), white, (0, 0));
    h.run_steps(1);
    assert!(h.state().hist_panel.counts().is_some());
    wait(&mut h, |a| {
        a.hist_panel.counts().is_some_and(|c| c.red[255] == 120_000)
    });
}

#[test]
fn the_selection_and_the_selected_layer_limit_it() {
    let mut h = histogram_open(Tool::Brush);
    counted(&mut h);
    // The red half, 100 columns: 100×300.
    h.state_mut().selection = Some(Selection::from(PixelRect {
        x: 50,
        y: 0,
        w: 100,
        h: 300,
    }));
    wait(&mut h, |a| {
        a.hist_panel.counts().is_some_and(|c| c.pixels == 30_000)
    });
    assert_eq!(h.state().hist_panel.counts().unwrap().red[10], 0);
    h.state_mut().selection = None;
    // The layer on top, smaller, half over the canvas.
    let green = image::RgbaImage::from_pixel(40, 10, image::Rgba([0, 255, 0, 255]));
    let d = h.state_mut().doc.as_mut().unwrap();
    d.add_layer("small".into(), green, (-20, 0));
    h.state_mut().hist_panel.source = histogram_panel::Source::SelectedLayer;
    wait(&mut h, |a| {
        a.hist_panel.counts().is_some_and(|c| c.pixels == 200)
    });
    assert_eq!(h.state().hist_panel.counts().unwrap().green[255], 200);
}

#[test]
fn a_large_image_keeps_the_interface_going_while_counted_and_closing_stops_it() {
    let mut h = app(Tool::Brush);
    let big = image::RgbaImage::from_pixel(6000, 4000, image::Rgba([1, 2, 3, 255]));
    h.state_mut().doc = Some(Document::new(big, None));
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Histogram));
    let t = std::time::Instant::now();
    h.step();
    // Not waited for: the thread is started, the frame goes on.
    assert!(h.state().hist_panel.counting() || h.state().hist_panel.counts().is_some());
    assert!(t.elapsed().as_millis() < 1500, "{:?}", t.elapsed());
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Histogram));
    h.step();
    wait(&mut h, |a| !a.hist_panel.is_running());
}

// ------------------------------------------------------------ Navigator

fn navigator_open(size: [f32; 2]) -> Harness<'static, AtelierApp> {
    let mut h = app_sized(Tool::Brush, size);
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Navigator));
    h.state_mut().zoom = 4.0;
    h.run_steps(3);
    wait(&mut h, |a| a.navigator.ready());
    h.run_steps(1);
    h
}

/// The thumbnail's rectangle in the panel.
fn thumb_rect(h: &Harness<'static, AtelierApp>) -> Rect {
    h.state().navigator.picture().expect("thumbnail shown")
}

#[test]
fn a_click_on_the_navigator_centers_the_view_there() {
    let mut h = navigator_open([1400.0, 1000.0]);
    on_screen(&h, "navigator");
    let r = thumb_rect(&h);
    let at = r.min + r.size() * vec2(0.9, 0.1);
    click(&mut h, at, Modifiers::NONE);
    h.run_steps(2);
    let c = h.state().view_center().unwrap();
    assert!((c.x - 360.0).abs() < 3.0, "{c:?}");
    assert!((c.y - 30.0).abs() < 3.0, "{c:?}");
}

#[test]
fn dragging_the_view_box_follows_from_where_it_is_grabbed() {
    let mut h = navigator_open([1400.0, 1000.0]);
    let r = thumb_rect(&h);
    // Centered at first: grabbed off its center, nothing jumps.
    let center = r.center();
    let before = h.state().view_center().unwrap();
    let grabbed = center + vec2(4.0, 2.0);
    let to = grabbed + vec2(r.width() * 0.2, 0.0);
    drag(&mut h, grabbed, to);
    let after = h.state().view_center().unwrap();
    assert!(
        (after.x - before.x - 80.0).abs() < 3.0,
        "{before:?} {after:?}"
    );
    assert!((after.y - before.y).abs() < 3.0, "{before:?} {after:?}");
    // No history for a view.
    assert_eq!(steps(&h), 0);
}

#[test]
fn it_holds_on_a_turned_view() {
    let mut h = navigator_open([1400.0, 1000.0]);
    h.state_mut().view_turn.angle = 30.0;
    h.state_mut().zoom = 16.0;
    h.run_steps(2);
    let r = thumb_rect(&h);
    let at = r.min + r.size() * vec2(0.25, 0.75);
    click(&mut h, at, Modifiers::NONE);
    h.run_steps(2);
    let c = h.state().view_center().unwrap();
    assert!(
        (c.x - 100.0).abs() < 3.0 && (c.y - 225.0).abs() < 3.0,
        "{c:?}"
    );
}

#[test]
fn the_slider_and_the_buttons_zoom_around_the_view() {
    let mut h = navigator_open([1400.0, 1000.0]);
    let center = h.state().view_center().unwrap();
    let zoom = h.state().zoom;
    h.get_by_label(&t!("menu-view-zoom-in")).click();
    h.run_steps(3);
    assert!(h.state().zoom > zoom);
    let c = h.state().view_center().unwrap();
    assert!((c - center).length() < 1.5, "{c:?} {center:?}");
    // The slider's scale goes both ways.
    for zoom in [0.25, 1.0, 8.0] {
        let s = navigator::slider_of(zoom);
        assert!((navigator::zoom_of(s) - zoom).abs() < 1e-3);
    }
}

#[test]
fn panels_keep_their_width_and_show_all_they_hold() {
    let mut h = app(Tool::Ruler);
    let d = h.state_mut().doc.as_mut().unwrap();
    d.samplers = vec![ColorSampler { x: 10.5, y: 10.5 }];
    d.ruler = Some(Measure {
        vertex: (60.0, 240.0),
        first: (360.0, 150.0),
        second: Some((300.0, 280.0)),
    });
    for c in [
        workspace::Command::Info,
        workspace::Command::Histogram,
        workspace::Command::Navigator,
    ] {
        h.state_mut().run(Action::Workspace(c));
    }
    wait(&mut h, |a| a.navigator.ready() && !a.hist_panel.counting());
    h.run_steps(6);
    for (id, bottom) in [
        ("info", t!("panels8-short-l2")),
        ("histogram", t!("panels8-histogram-percentile")),
    ] {
        let r = h.ctx.memory(|m| m.area_rect(egui::Id::new(id))).unwrap();
        // (In merged locales: labels are short.)
        assert!(
            r.width() <= 285.0 || bottom.starts_with("panels8"),
            "{id}: {r:?}"
        );
        let inside = h
            .get_all_by_label(&bottom)
            .any(|n| r.contains(n.rect().left_bottom()));
        assert!(inside, "{id}: {r:?} doesn't reach {bottom}");
    }
}

#[test]
fn the_panels_fit_a_narrow_window_and_tab_hides_them() {
    let mut h = navigator_open([660.0, 700.0]);
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Histogram));
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Info));
    h.run_steps(4);
    for id in ["navigator", "histogram", "info"] {
        on_screen(&h, id);
    }
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(h.state().workspace.hide_all);
    assert!(h.state().info.open, "stays open, not shown");
    h.key_press(Key::Tab);
    h.run_steps(3);
    on_screen(&h, "info");
}

#[test]
fn the_window_menu_lists_the_panels_and_the_palette_finds_them() {
    let h = app(Tool::Brush);
    let menu = h.state().window_menu();
    let checks = menu.iter().filter_map(|e| match e {
        MenuEntry::Check(_, _, _, _, a) => Some(*a),
        _ => None,
    });
    let found = |c: workspace::Command| {
        checks
            .clone()
            .any(|a| matches!(a, Action::Workspace(w) if w == c))
    };
    assert!(found(workspace::Command::Info));
    assert!(found(workspace::Command::Histogram));
    assert!(found(workspace::Command::Navigator));
}

#[test]
fn the_options_stay_from_one_session_to_the_next() {
    let mut a = app(Tool::Brush);
    a.state_mut().measure.sample = SampleSize::Average31;
    a.state_mut().hist_panel.channel = histogram_panel::Channel::Colors;
    a.state_mut().hist_panel.source = histogram_panel::Source::SelectedLayer;
    let saved = a.state().session_settings();
    let mut b = app(Tool::Brush);
    b.state_mut().restore_session(&saved);
    assert_eq!(b.state().measure.sample, SampleSize::Average31);
    assert_eq!(
        b.state().hist_panel.channel,
        histogram_panel::Channel::Colors
    );
    assert_eq!(
        b.state().hist_panel.source,
        histogram_panel::Source::SelectedLayer
    );
}

// ------------------------------------------------------------ Screen modes

#[test]
fn f_cycles_the_screen_modes_and_shift_f_goes_back() {
    use workspace::ScreenMode::*;
    let mut h = app(Tool::Brush);
    let mode = |h: &Harness<'static, AtelierApp>| h.state().workspace.screen;
    assert_eq!(mode(&h), Standard);
    h.key_press(Key::F);
    h.run_steps(2);
    assert_eq!(mode(&h), FullWithMenu);
    assert!(h.state().chrome_shown(), "the panels stay");
    h.key_press(Key::F);
    h.run_steps(2);
    assert_eq!(mode(&h), Full);
    assert!(!h.state().chrome_shown());
    h.key_press(Key::F);
    h.run_steps(2);
    assert_eq!(mode(&h), Standard);
    assert!(h.state().chrome_shown(), "as they were");
    h.key_press_modifiers(Modifiers::SHIFT, Key::F);
    h.run_steps(2);
    assert_eq!(mode(&h), Full);
    h.key_press_modifiers(Modifiers::SHIFT, Key::F);
    h.run_steps(2);
    assert_eq!(mode(&h), FullWithMenu);
}

#[test]
fn the_canvas_takes_the_screen_and_tab_still_shows_the_panels() {
    use workspace::ScreenMode::*;
    let mut h = app(Tool::Brush);
    let standard = h.state().rulers.canvas_area().unwrap();
    h.state_mut().set_screen_mode(FullWithMenu);
    h.run_steps(3);
    let with_menu = h.state().rulers.canvas_area().unwrap();
    assert!(with_menu.top() < standard.top(), "no tabs");
    assert!(with_menu.top() >= 40.0, "the menus");
    h.state_mut().set_screen_mode(Full);
    h.run_steps(3);
    let full = h.state().rulers.canvas_area().unwrap();
    assert_eq!(full, h.ctx.content_rect(), "all of it");
    // Tab: the panels, the canvas still under no title bar.
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert!(h.state().chrome_shown());
    assert!(h.get_all_by_label(&Tool::Brush.name()).next().is_some());
    let beside = h.state().rulers.canvas_area().unwrap();
    assert!(beside.top() < with_menu.top(), "still no title bar");
    // Tools still work there.
    h.key_press(Key::Tab);
    h.run_steps(3);
    assert_eq!(h.state().rulers.canvas_area().unwrap(), full);
}

#[test]
fn esc_leaves_full_screen_after_what_is_being_prepared() {
    use workspace::ScreenMode::*;
    let mut h = app(Tool::Transform);
    h.state_mut().set_screen_mode(Full);
    h.run_steps(2);
    // A transform being prepared goes first.
    h.state_mut()
        .run(Action::Transform(transform::Command::Free));
    h.run_steps(2);
    assert!(h.state().transform_pending());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().workspace.screen, Full);
    assert!(!h.state().transform_pending());
    h.key_press(Key::Escape);
    h.run_steps(2);
    assert_eq!(h.state().workspace.screen, Standard);
    assert!(h.state().chrome_shown());
}

#[test]
fn tab_before_full_screen_is_given_back_and_reset_workspace_leaves_it() {
    use workspace::ScreenMode::*;
    let mut h = app(Tool::Brush);
    h.key_press(Key::Tab);
    h.run_steps(2);
    assert!(h.state().workspace.hide_all);
    h.state_mut().set_screen_mode(Full);
    h.state_mut().set_screen_mode(Standard);
    assert!(h.state().workspace.hide_all, "as it was");
    h.state_mut().set_screen_mode(Full);
    h.state_mut()
        .run(Action::Workspace(workspace::Command::ResetWorkspace));
    assert_eq!(h.state().workspace.screen, Standard);
    assert!(!h.state().workspace.hide_all);
}

#[test]
fn f_typed_in_a_text_goes_to_the_text() {
    let mut h = app(Tool::Text);
    let at = screen(&h, 100.0, 100.0);
    click(&mut h, at, Modifiers::NONE);
    assert!(h.state().typing_on_canvas());
    h.key_press(Key::F);
    h.run_steps(2);
    assert_eq!(h.state().workspace.screen, workspace::ScreenMode::Standard);
}

#[test]
fn the_view_menu_says_the_screen_mode() {
    let mut h = app(Tool::Brush);
    h.state_mut()
        .set_screen_mode(workspace::ScreenMode::FullWithMenu);
    let entries = h.state().screen_mode_entries();
    let MenuEntry::Submenu(_, modes) = &entries[1] else {
        panic!("a submenu");
    };
    let chosen: Vec<bool> = modes
        .iter()
        .map(|e| matches!(e, MenuEntry::Choice(_, true, _)))
        .collect();
    assert_eq!(chosen, [false, true, false]);
}
