//! The Distort filters' dialogs through the app: the preview is the result
//! (shapes, masks, locked pixels, smart objects), and the settings show.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use image::{Rgba, RgbaImage};

use super::*;
use crate::vector::{Bounds, Live, Paint, Shape, ShapeKind, ShapeStroke, pt};

const KINDS: [FilterKind; 8] = [
    FilterKind::Pinch,
    FilterKind::PolarCoordinates,
    FilterKind::Ripple,
    FilterKind::Shear,
    FilterKind::Spherize,
    FilterKind::Twirl,
    FilterKind::Wave,
    FilterKind::ZigZag,
];

fn app(w: u32, h: u32) -> Harness<'static, AtelierApp> {
    let mut harness = Harness::builder()
        .with_size([1200.0, 900.0])
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = RgbaImage::from_fn(w, h, |x, y| {
                Rgba([(x * 255 / w) as u8, (y * 255 / h) as u8, 90, 255])
            });
            app.doc = Some(Document::new(image, None));
            app
        });
    harness.run_steps(2);
    harness
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn doc_mut<'a>(h: &'a mut Harness<'static, AtelierApp>) -> &'a mut Document {
    h.state_mut().doc.as_mut().unwrap()
}

fn settle(h: &mut Harness<'static, AtelierApp>, done: impl Fn(&AtelierApp) -> bool) {
    for _ in crate::testing::patience() {
        h.run_steps(1);
        if done(h.state()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("background work not done");
}

/// The canvas shows the preview of the settings at their full quality.
fn refined(a: &AtelierApp) -> bool {
    a.preview_shown.is_some() && a.filters.dialog.as_ref().is_some_and(|d| d.drafted.is_none())
}

/// Opens the dialog of `filter` and waits for its preview on the canvas,
/// the quick one replaced by the full one.
fn previewed(h: &mut Harness<'static, AtelierApp>, filter: Filter) -> (Arc<RgbaImage>, (i64, i64)) {
    h.state_mut().open_filter_dialog(filter);
    h.run_steps(2);
    settle(h, refined);
    let shown = h.state().preview_shown.as_ref().unwrap();
    let origin = shown.origin.unwrap_or(doc(h).active_layer().offset);
    (shown.image.clone(), origin)
}

fn ok(h: &mut Harness<'static, AtelierApp>) {
    h.get_by_label(&t!("dialog-ok")).click();
    settle(h, |a| {
        a.filters.dialog.is_none() && a.filters.applying.is_none()
    });
}

fn shape_doc(h: &mut Harness<'static, AtelierApp>) {
    let live = Live::new(
        ShapeKind::Rectangle,
        Bounds::of(pt(40.0, 130.0), pt(360.0, 170.0)),
    );
    let shape = Shape::live(live, Paint::Color([200, 30, 30]), ShapeStroke::default());
    doc_mut(h).add_shape_layer("R".into(), "R", shape);
    h.run_steps(2);
}

#[test]
fn the_preview_of_a_shape_is_its_result_and_leaves_it_a_shape_until_ok() {
    for filter in [
        FilterKind::PolarCoordinates.initial(),
        Filter::GaussianBlur { radius: 6.0 },
    ] {
        let mut h = app(400, 300);
        shape_doc(&mut h);
        let (before, rev) = {
            let l = doc(&h).active_layer();
            (l.image().dimensions(), l.rev())
        };
        assert_ne!(before, (400, 300));
        let (preview, origin) = previewed(&mut h, filter);
        let l = doc(&h).active_layer();
        assert!(l.shape.is_some(), "still a shape in the dialog");
        assert_eq!((l.image().dimensions(), l.rev()), (before, rev));
        assert_eq!(preview.dimensions(), (400, 300), "{filter:?}");
        assert_eq!(origin, (0, 0));
        ok(&mut h);
        let l = doc(&h).active_layer();
        assert_eq!(l.offset, origin);
        assert_eq!(
            *l.image(),
            *preview,
            "{filter:?}: the preview is the result"
        );
    }
}

#[test]
fn the_preview_of_a_mask_with_its_own_box_is_its_result() {
    let mut h = app(200, 200);
    let d = doc_mut(&mut h);
    let image = image::GrayImage::from_fn(60, 60, |x, _| image::Luma([(x * 4) as u8]));
    d.active_layer_mut().mask = Some(crate::document::Mask::new(image, (100, 10), 0, (0, 0)));
    d.target_mask(true);
    h.run_steps(2);
    let (preview, _) = previewed(&mut h, FilterKind::Twirl.initial());
    assert_eq!(preview.dimensions(), (200, 200), "as the result will be");
    let m = doc(&h).active_layer().mask.as_ref().unwrap();
    assert_eq!(m.image().dimensions(), (60, 60), "the mask waits for OK");
    let shown = h.state().preview_shown.as_ref().unwrap();
    let shown_mask = shown.mask.as_ref().unwrap().0.clone();
    ok(&mut h);
    let m = doc(&h).active_layer().mask.as_ref().unwrap();
    assert_eq!(m.image().dimensions(), (200, 200));
    assert_eq!(*m.image(), *shown_mask);
}

/// Red on the left, nothing on the right, locked.
fn locked_half(h: &mut Harness<'static, AtelierApp>) -> RgbaImage {
    let half = RgbaImage::from_fn(100, 100, |x, _| {
        if x < 50 {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    });
    let d = doc_mut(h);
    d.active_layer_mut().replace_pixels(half.clone());
    d.active_layer_mut().locks.transparent = true;
    h.run_steps(2);
    half
}

#[test]
fn locked_pixels_stay_their_color_in_the_preview_and_the_result() {
    for kind in KINDS {
        if kind == FilterKind::Shear {
            continue;
        }
        let mut h = app(100, 100);
        let half = locked_half(&mut h);
        let (preview, _) = previewed(&mut h, kind.initial());
        for (a, b) in preview.pixels().zip(half.pixels()) {
            assert_eq!(a[3], b[3], "{kind:?}: the preview keeps the opacity");
            if b[3] == 255 {
                assert!(a[0] > 200, "{kind:?}: black in the preview {a:?}");
            }
        }
        ok(&mut h);
        let done = doc(&h).active_layer().image().clone();
        assert_eq!(done, *preview, "{kind:?}");
    }
}

#[test]
fn a_smart_ripple_keeps_its_pattern_when_the_object_moves() {
    for filter in [
        Filter::Ripple {
            amount: 300.0,
            size: crate::filters::RippleSize::Medium,
        },
        FilterKind::Wave.initial(),
        Filter::Mosaic { cell: 9.0 },
    ] {
        let mut h = app(200, 200);
        let d = doc_mut(&mut h);
        assert!(d.convert_to_smart(&[0]));
        h.run_steps(2);
        h.state_mut().start_filter(filter);
        settle(&mut h, |a| !a.smart.busy());
        let placed = doc(&h).active_layer().image().clone();
        let d = doc_mut(&mut h);
        let at = d.active_layer().offset;
        d.active_layer_mut().offset = (at.0 + 7, at.1 + 3);
        let limit = d.full_rect();
        d.active_layer_mut().render_smart(limit);
        assert_eq!(*doc(&h).active_layer().image(), placed, "{filter:?}");
    }
}

#[test]
fn a_smart_filters_square_and_canvas_previews_show_its_selection() {
    let mut h = app(200, 200);
    assert!(doc_mut(&mut h).convert_to_smart(&[0]));
    h.run_steps(2);
    let original = doc(&h).active_layer().image().clone();
    h.state_mut().selection = Some(Selection {
        rect: PixelRect {
            x: 20,
            y: 20,
            w: 60,
            h: 60,
        },
        mask: None,
        inverted: false,
    });
    let (preview, _) = previewed(&mut h, Filter::Twirl { angle: 300.0 });
    let outside = |image: &RgbaImage| {
        image.enumerate_pixels().all(|(x, y, p)| {
            let inside = (20..80).contains(&x) && (20..80).contains(&y);
            inside || *p == *original.get_pixel(x, y)
        })
    };
    assert!(
        outside(&preview),
        "the canvas preview is cut by the selection"
    );
    assert_ne!(*preview, original);
    let target = h.state_mut().smart_filter_input().unwrap();
    let filter = Filter::Twirl { angle: 300.0 };
    assert_eq!(target.filter_region([0, 0, 200, 200], &filter), *preview);
    ok(&mut h);
    settle(&mut h, |a| !a.smart.busy());
    assert_eq!(*doc(&h).active_layer().image(), *preview);
}

#[test]
fn hidden_pixels_do_not_move_the_center_of_a_filter() {
    // The same picture, one with more of it hidden on the right.
    let picture = |x: u32, y: u32| Rgba([(x * 3) as u8, (y * 3) as u8, 90, 255]);
    let twirled = |w: u32| {
        let mut h = app(80, 80);
        let d = doc_mut(&mut h);
        let image = RgbaImage::from_fn(w, 80, picture);
        d.active_layer_mut().replace_pixels(image);
        h.run_steps(2);
        h.state_mut().start_filter(Filter::Twirl { angle: 200.0 });
        settle(&mut h, |a| !a.filters.busy());
        let out = doc(&h).active_layer().image().clone();
        image::imageops::crop_imm(&out, 0, 0, 80, 80).to_image()
    };
    let (exact, wider) = (twirled(80), twirled(200));
    assert_eq!(exact, wider);
    let mut plain = RgbaImage::from_fn(80, 80, picture);
    Filter::Twirl { angle: 200.0 }.apply(&mut plain, (0, 0));
    assert_eq!(exact, plain);
}

#[test]
fn a_stale_square_is_stopped_and_not_shown() {
    let mut h = app(1000, 1000);
    let huge = crate::filters::Wave {
        generators: 999.0,
        ..match FilterKind::Wave.initial() {
            Filter::Wave(w) => w,
            _ => unreachable!(),
        }
    };
    h.state_mut().open_filter_dialog(Filter::Wave(huge));
    h.run_steps(1);
    let first = h
        .state()
        .filters
        .dialog
        .as_ref()
        .unwrap()
        .square
        .job
        .as_ref()
        .map(Job::key);
    assert!(first.is_some(), "computing");
    h.state_mut().filters.dialog.as_mut().unwrap().filter = Filter::Twirl { angle: 50.0 };
    h.run_steps(1);
    let dialog = h.state().filters.dialog.as_ref().unwrap();
    if let Some(job) = &dialog.square.job
        && Some(job.key) == first
    {
        assert!(job.stopped());
    }
    settle(&mut h, |a| {
        let d = a.filters.dialog.as_ref().unwrap();
        d.square.job.is_none() && d.square.shown.is_some()
    });
    let shown = h
        .state()
        .filters
        .dialog
        .as_ref()
        .unwrap()
        .square
        .shown
        .as_ref()
        .unwrap()
        .0;
    assert_ne!(Some(shown), first);
}

#[test]
fn settings_show_beside_the_preview_on_short_windows() {
    for (w, h_) in [(660.0, 500.0), (1024.0, 700.0)] {
        for kind in KINDS {
            let mut h = app(100, 100);
            h.set_size(egui::vec2(w, h_));
            h.run_steps(3);
            h.state_mut().run(Action::Filter(kind));
            h.run_steps(4);
            let what = format!("{w}x{h_} {kind:?}");
            let window = h.ctx.content_rect();
            let ok = h.get_by_label(&t!("dialog-ok")).rect();
            assert!(window.contains_rect(ok), "{what}");
            let first = match kind {
                FilterKind::Pinch
                | FilterKind::Ripple
                | FilterKind::ZigZag
                | FilterKind::Spherize => {
                    t!("filter-amount")
                }
                FilterKind::Twirl => t!("filter-angle"),
                FilterKind::PolarCoordinates => t!("distort8-conversion"),
                FilterKind::Wave => t!("distort8-generators"),
                _ => t!("distort8-shear-curve"),
            };
            let row = h.get_all_by_label(&first).next().unwrap().rect();
            assert!(
                row.bottom() <= ok.top(),
                "{what}: {first} at {row:?}, buttons at {ok:?}"
            );
            assert!(row.top() >= 0.0, "{what}");
        }
    }
}

#[test]
fn a_tall_window_keeps_a_short_dialog_in_one_column() {
    let mut h = app(100, 100);
    h.state_mut().run(Action::Filter(FilterKind::Twirl));
    h.run_steps(4);
    let area = h
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("filter")))
        .unwrap();
    assert!(area.width() < 340.0, "{area:?}");
}

#[test]
fn a_click_on_the_shear_line_adds_a_point() {
    let mut h = app(100, 100);
    h.state_mut().run(Action::Filter(FilterKind::Shear));
    h.run_steps(4);
    let r = h.get_by_label(&t!("distort8-shear-curve")).rect();
    h.get_by_label(&t!("distort8-shear-curve")).click();
    h.run_steps(2);
    let dialog = h.state().filters.dialog.as_ref().unwrap();
    let Filter::Shear(s) = dialog.filter else {
        panic!()
    };
    assert_eq!(s.used().len(), 3, "{r:?}");
    // On the point just added: nothing more.
    h.get_by_label(&t!("distort8-shear-curve")).click();
    h.run_steps(2);
    let dialog = h.state().filters.dialog.as_ref().unwrap();
    let Filter::Shear(s) = dialog.filter else {
        panic!()
    };
    assert_eq!(s.used().len(), 3);
}

/// A thin vertical line on black, which a strong Twirl compresses.
fn thin_line(h: &mut Harness<'static, AtelierApp>) -> RgbaImage {
    let line = RgbaImage::from_fn(200, 200, |x, y| {
        let v = if x == 100 && y < 100 { 255 } else { 0 };
        Rgba([v, v, v, 255])
    });
    doc_mut(h).active_layer_mut().replace_pixels(line.clone());
    h.run_steps(2);
    line
}

#[test]
fn the_canvas_shows_a_quick_preview_first_then_the_result() {
    let mut h = app(200, 200);
    let line = thin_line(&mut h);
    let filter = Filter::Twirl { angle: 999.0 };
    let mut full = line.clone();
    filter.apply(&mut full, (0, 0));
    let mut quick = line.clone();
    crate::filters::drafted(true, || filter.apply(&mut quick, (0, 0)));
    assert_ne!(quick, full, "the quick preview is cheaper, not the same");
    h.state_mut().open_filter_dialog(filter);
    h.run_steps(2);
    settle(&mut h, |a| a.preview_shown.is_some());
    let first = h.state().preview_shown.as_ref().unwrap().image.clone();
    assert_eq!(*first, quick, "one sample per pixel while the settings move");
    assert!(h.state().filters.dialog.as_ref().unwrap().drafted.is_some());
    settle(&mut h, refined);
    let then = h.state().preview_shown.as_ref().unwrap().image.clone();
    assert_eq!(*then, full, "then the result itself");
    ok(&mut h);
    assert_eq!(*doc(&h).active_layer().image(), full);
}

#[test]
fn ok_on_a_quick_preview_computes_the_result() {
    let mut h = app(200, 200);
    let line = thin_line(&mut h);
    let filter = Filter::Twirl { angle: 999.0 };
    let mut full = line.clone();
    filter.apply(&mut full, (0, 0));
    h.state_mut().open_filter_dialog(filter);
    h.run_steps(2);
    settle(&mut h, |a| a.preview_shown.is_some());
    // OK at once: the quick preview shown is not taken for the result.
    assert!(h.state().filters.dialog.as_ref().unwrap().drafted.is_some());
    ok(&mut h);
    assert_eq!(*doc(&h).active_layer().image(), full);
}

#[test]
fn short_settings_stay_below_a_smaller_square_on_a_short_window() {
    for kind in [FilterKind::Twirl, FilterKind::Pinch, FilterKind::PolarCoordinates] {
        let mut h = app(100, 100);
        h.set_size(egui::vec2(660.0, 500.0));
        h.run_steps(3);
        h.state_mut().run(Action::Filter(kind));
        h.run_steps(4);
        let area = h
            .ctx
            .memory(|m| m.area_rect(egui::Id::new("filter")))
            .unwrap();
        assert!(area.width() < 340.0, "{kind:?}: one column, {area:?}");
        let ok = h.get_by_label(&t!("dialog-ok")).rect();
        assert!(h.ctx.content_rect().contains_rect(ok), "{kind:?}");
    }
    // Taller settings than the square: beside it.
    let mut h = app(100, 100);
    h.set_size(egui::vec2(660.0, 500.0));
    h.run_steps(3);
    h.state_mut().run(Action::Filter(FilterKind::Wave));
    h.run_steps(4);
    let area = h
        .ctx
        .memory(|m| m.area_rect(egui::Id::new("filter")))
        .unwrap();
    assert!(area.width() > 400.0, "Wave: two columns, {area:?}");
}
