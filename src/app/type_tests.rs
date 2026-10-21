//! Type tool on the canvas: point and paragraph texts typed with a caret,
//! selections, styles, commit and cancel.

use super::*;
use crate::text::CharStyle;
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

/// A 400 × 300 white document at 100 %, with the Type tool.
fn app() -> Harness<'static, AtelierApp> {
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let white = RgbaImage::from_pixel(400, 300, Rgba([255, 255, 255, 255]));
            app.doc = Some(Document::new(white, None));
            app
        });
    let app = h.state_mut();
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    app.set_tool(Tool::Text);
    h.run_steps(3);
    h
}

fn has_fonts(h: &mut Harness<'static, AtelierApp>) -> bool {
    !h.state_mut().text_engine().families().is_empty()
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().unwrap()
}

fn text(h: &Harness<'static, AtelierApp>) -> String {
    doc(h)
        .active_layer()
        .text
        .as_ref()
        .expect("a text layer")
        .text
        .clone()
}

fn screen(h: &Harness<'static, AtelierApp>, x: f32, y: f32) -> Pos2 {
    h.state().last_view.unwrap().to_screen(pos2(x, y))
}

fn press(h: &mut Harness<'static, AtelierApp>, at: Pos2, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(at);
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: true,
        modifiers,
    });
    h.run_steps(1);
}

fn release(h: &mut Harness<'static, AtelierApp>, at: Pos2) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn click(h: &mut Harness<'static, AtelierApp>, x: f32, y: f32) {
    let at = screen(h, x, y);
    press(h, at, Modifiers::NONE);
    release(h, at);
}

fn drag(h: &mut Harness<'static, AtelierApp>, from: (f32, f32), to: (f32, f32)) {
    let (a, b) = (screen(h, from.0, from.1), screen(h, to.0, to.1));
    press(h, a, Modifiers::NONE);
    for i in 1..=4 {
        h.hover_at(a.lerp(b, i as f32 / 4.0));
        h.run_steps(1);
    }
    release(h, b);
}

fn type_str(h: &mut Harness<'static, AtelierApp>, s: &str) {
    h.event(egui::Event::Text(s.into()));
    h.run_steps(1);
}

fn key(h: &mut Harness<'static, AtelierApp>, key: Key, modifiers: Modifiers) {
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.event(egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed: true,
        repeat: false,
        modifiers,
    });
    h.run_steps(1);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    h.run_steps(1);
}

fn selection(h: &Harness<'static, AtelierApp>) -> std::ops::Range<usize> {
    h.state().type_tool.edit.as_ref().expect("typing").range()
}

#[test]
fn a_click_types_a_point_text_that_esc_commits() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    assert_eq!(doc(&h).layers.len(), 2, "a text layer");
    assert!(h.state().typing_on_canvas());
    type_str(&mut h, "Hi");
    key(&mut h, Key::Enter, Modifiers::NONE);
    type_str(&mut h, "you");
    key(&mut h, Key::Backspace, Modifiers::NONE);
    // Keys of tools and Space go to the text while it is typed.
    type_str(&mut h, " v");
    assert!(h.state().tool == Tool::Text);
    assert_eq!(text(&h), "Hi\nyo v");
    assert_eq!(doc(&h).active_layer().name, "Hi");
    // The first baseline is on the click.
    let layer = doc(&h).active_layer();
    let t = layer.text.as_ref().unwrap();
    assert_eq!(
        crate::app::type_tool::text_to_canvas(layer.offset, t, (0.0, 0.0)),
        pos2(50.0, 100.0)
    );
    assert!(layer.content_bounds().unwrap().y < 100);

    key(&mut h, Key::Escape, Modifiers::NONE);
    assert!(!h.state().typing_on_canvas());
    assert_eq!(text(&h), "Hi\nyo v");
    // Created and typed: one step.
    h.state_mut().undo();
    assert_eq!(doc(&h).layers.len(), 1);
}

#[test]
fn an_empty_text_is_dropped_on_commit() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    assert_eq!(doc(&h).layers.len(), 2);
    key(&mut h, Key::Enter, Modifiers::COMMAND);
    assert!(!h.state().typing_on_canvas());
    assert_eq!(doc(&h).layers.len(), 1);
    assert!(!doc(&h).can_undo(), "no trace in the history");
}

#[test]
fn caret_moves_selects_and_replaces() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    type_str(&mut h, "Hello world");
    key(&mut h, Key::ArrowLeft, Modifiers::COMMAND);
    assert_eq!(selection(&h), 6..6);
    key(
        &mut h,
        Key::ArrowRight,
        Modifiers::COMMAND | Modifiers::SHIFT,
    );
    assert_eq!(selection(&h), 6..11);
    type_str(&mut h, "there");
    assert_eq!(text(&h), "Hello there");
    key(&mut h, Key::Home, Modifiers::SHIFT);
    assert_eq!(selection(&h), 0..11);
    key(&mut h, Key::ArrowRight, Modifiers::NONE);
    assert_eq!(selection(&h), 11..11, "collapsed to its end");
    key(&mut h, Key::A, Modifiers::COMMAND);
    assert_eq!(selection(&h), 0..11);
    key(&mut h, Key::Delete, Modifiers::NONE);
    assert_eq!(text(&h), "");
    // Pasted text, line breaks included.
    h.event(egui::Event::Paste("a\r\nb".into()));
    h.run_steps(1);
    assert_eq!(text(&h), "a\nb");
    key(&mut h, Key::ArrowUp, Modifiers::NONE);
    assert_eq!(selection(&h), 1..1, "the line above, at the same place");
}

#[test]
fn styles_apply_to_the_selection_or_to_what_is_typed_next() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    type_str(&mut h, "ab");
    key(&mut h, Key::ArrowLeft, Modifiers::SHIFT);
    h.state_mut()
        .restyle_selection(|s| s.color = [255, 0, 0, 255]);
    h.run_steps(1);
    let runs = |h: &Harness<'static, AtelierApp>| {
        let t = doc(h).active_layer().text.clone().unwrap();
        t.runs
            .iter()
            .map(|r| (r.len, r.style.color))
            .collect::<Vec<_>>()
    };
    assert_eq!(runs(&h), [(1, [0, 0, 0, 255]), (1, [255, 0, 0, 255])]);
    // Ctrl+Shift+B: faux bold.
    key(&mut h, Key::B, Modifiers::COMMAND | Modifiers::SHIFT);
    assert!(
        doc(&h).active_layer().text.as_ref().unwrap().runs[1]
            .style
            .faux_bold
    );
    // Nothing selected: the next characters take the style.
    key(&mut h, Key::End, Modifiers::NONE);
    h.state_mut().restyle_selection(|s| s.size = 10.0);
    assert_eq!(runs(&h).len(), 2, "nothing changed yet");
    type_str(&mut h, "c");
    let t = doc(&h).active_layer().text.clone().unwrap();
    assert_eq!(t.runs.len(), 3);
    assert_eq!(t.runs[2].style.size, 10.0);
    assert_eq!(
        t.runs[2].style.color,
        [255, 0, 0, 255],
        "the rest as before"
    );
    // Outside an edit: the whole text.
    key(&mut h, Key::Escape, Modifiers::NONE);
    h.state_mut().restyle_selection(|s| s.underline = true);
    let t = doc(&h).active_layer().text.clone().unwrap();
    assert!(t.runs.iter().all(|r| r.style.underline));
    // Paragraphs: Ctrl+Shift+C centers the one being typed.
    click(&mut h, 300.0, 250.0);
    key(&mut h, Key::C, Modifiers::COMMAND | Modifiers::SHIFT);
    let t = doc(&h).active_layer().text.clone().unwrap();
    assert_eq!(t.paragraphs[0].align, crate::text::TextAlign::Center);
}

#[test]
fn a_drag_draws_a_paragraph_box_that_wraps_and_resizes() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    h.state_mut().type_tool.style.size = 20.0;
    drag(&mut h, (20.0, 30.0), (140.0, 230.0));
    let layer = doc(&h).active_layer();
    let t = layer.text.clone().expect("a text layer");
    assert_eq!(t.frame, Some((120.0, 200.0)));
    assert_eq!(
        crate::app::type_tool::text_to_canvas(layer.offset, &t, (0.0, 0.0)),
        pos2(20.0, 30.0),
        "the box where it was drawn"
    );
    type_str(&mut h, "The quick brown fox jumps over the lazy dog");
    let t = doc(&h).active_layer().text.clone().unwrap();
    let layout = h.state_mut().text_engine().layout(&t);
    assert!(layout.lines.len() >= 3);
    let b = doc(&h).active_layer().content_bounds().unwrap();
    assert!(
        b.x >= 19 && b.x + b.w as i64 <= 141,
        "inside the box: {b:?}"
    );

    // The bottom-right handle: a smaller box, the text overflows it.
    drag(&mut h, (140.0, 230.0), (100.0, 60.0));
    let t = doc(&h).active_layer().text.clone().unwrap();
    assert_eq!(t.frame, Some((80.0, 30.0)));
    assert!(h.state_mut().text_engine().layout(&t).overflow);
    // The top-left one moves the box's origin.
    drag(&mut h, (20.0, 30.0), (10.0, 20.0));
    let layer = doc(&h).active_layer();
    let t = layer.text.clone().unwrap();
    assert_eq!(t.frame, Some((90.0, 40.0)));
    assert_eq!(
        crate::app::type_tool::text_to_canvas(layer.offset, &t, (0.0, 0.0)),
        pos2(10.0, 20.0)
    );
}

#[test]
fn clicking_a_text_edits_it_where_clicked_and_cancel_reverts() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    type_str(&mut h, "Hello");
    key(&mut h, Key::Escape, Modifiers::NONE);
    let steps = doc(&h).history().1;
    let b = doc(&h).active_layer().content_bounds().unwrap();
    // On its last letter's right half: the caret after it.
    let x = (b.x + b.w as i64) as f32 - 2.0;
    click(&mut h, x, 90.0);
    assert!(h.state().typing_on_canvas());
    assert_eq!(selection(&h), 5..5);
    assert_eq!(doc(&h).layers.len(), 2, "no new text");
    type_str(&mut h, "!");
    assert_eq!(text(&h), "Hello!");
    h.state_mut().cancel_text();
    assert_eq!(text(&h), "Hello");
    assert_eq!(doc(&h).history().1, steps, "nothing left in the history");
    assert!(!doc(&h).can_redo());
    // A click elsewhere commits; the next one starts a new text.
    click(&mut h, x, 90.0);
    type_str(&mut h, "?");
    click(&mut h, 300.0, 250.0);
    assert!(!h.state().typing_on_canvas());
    assert_eq!(text(&h), "Hello?");
    click(&mut h, 300.0, 250.0);
    assert_eq!(doc(&h).layers.len(), 3);
}

#[test]
fn another_tool_commits_the_text() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    type_str(&mut h, "Hi");
    h.state_mut().set_tool(Tool::Transform);
    assert!(!h.state().typing_on_canvas());
    assert_eq!(text(&h), "Hi");
    // The styles chosen are those of the next texts.
    let style: CharStyle = h.state().shown_char_style();
    assert_eq!(style.size, 48.0);
}

#[test]
fn texts_stay_editable_through_image_rotation_and_flips() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    click(&mut h, 50.0, 100.0);
    type_str(&mut h, "Hello");
    key(&mut h, Key::Escape, Modifiers::NONE);
    let before = doc(&h).active_layer().content_bounds().unwrap();
    h.state_mut().run(Action::RotateCanvas(90));
    h.run_steps(1);
    let layer = doc(&h).active_layer();
    let t = layer.text.as_ref().expect("still a text");
    assert!(t.transform[0].abs() < 1e-4 && (t.transform[2] - 1.0).abs() < 1e-4);
    let b = layer.content_bounds().unwrap();
    assert!(b.h > b.w);
    // Where the rotated canvas puts it: x ← height − y.
    let canvas_h = 300;
    assert!(
        (b.x - (canvas_h - (before.y + before.h as i64))).abs() <= 2,
        "{b:?}"
    );
    h.state_mut().run(Action::FlipCanvas(true));
    h.run_steps(1);
    let t = doc(&h).active_layer().text.clone().expect("still a text");
    assert!(
        (t.transform[1] - 1.0).abs() < 1e-4,
        "mirrored: {:?}",
        t.transform
    );
    h.state_mut().undo();
    h.state_mut().undo();
    let t = doc(&h).active_layer().text.clone().unwrap();
    assert_eq!(t.transform, crate::text::IDENTITY);
}

#[test]
fn image_size_scales_texts_that_stay_sharp() {
    let mut h = app();
    if !has_fonts(&mut h) {
        return;
    }
    h.state_mut().type_tool.style.size = 20.0;
    drag(&mut h, (20.0, 30.0), (140.0, 130.0));
    type_str(&mut h, "Hello");
    key(&mut h, Key::Escape, Modifiers::NONE);
    h.state_mut().apply_image_size(ImageSizeDialog {
        width: 800,
        height: 600,
        keep_ratio: true,
        filter: ResizeFilter::Bicubic,
    });
    let layer = doc(&h).active_layer();
    let t = layer.text.as_ref().expect("still a text");
    assert_eq!(t.runs[0].style.size, 40.0);
    assert_eq!(t.frame, Some((240.0, 200.0)));
    let origin = crate::app::type_tool::text_to_canvas(layer.offset, t, (0.0, 0.0));
    assert!(origin.distance(pos2(40.0, 60.0)) < 0.01, "{origin:?}");
}
