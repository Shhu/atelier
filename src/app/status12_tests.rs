//! Status messages: the same message said again, once faded, shows again,
//! in the status bar and in Full Screen.

use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

type H = Harness<'static, AtelierApp>;

fn app(tool: Tool) -> H {
    i18n::set_lang(Lang::En);
    let mut h = Harness::builder()
        .with_size([1400.0, 900.0])
        // Long frames: a message fades in a few dozen of them.
        .with_step_dt(0.25)
        .build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            let image = image::RgbaImage::from_pixel(200, 150, image::Rgba([90, 120, 200, 255]));
            app.doc = Some(Document::new(image, None));
            app.tool = tool;
            app
        });
    h.run_steps(3);
    h
}

/// The message in the status bar (it is a label of its own).
fn in_status_bar(h: &H, text: &str) -> bool {
    h.query_by_label(text).is_some()
}

/// The message over the image in Full Screen.
fn in_full_screen(h: &mut H, text: &str) -> bool {
    let ctx = h.ctx.clone();
    let notices = h.state_mut().full_screen_notices(&ctx);
    notices.iter().any(|n| n.0 == text && n.1 > 0.0)
}

/// Runs `say` (real keys) twice, the second time after the first message
/// faded; `shown` tells whether the message is in sight.
fn said_again(h: &mut H, text: &str, say: impl Fn(&mut H), shown: impl Fn(&mut H, &str) -> bool) {
    say(h);
    h.run_steps(2);
    assert_eq!(h.state().status, text);
    assert!(shown(h, text), "first time: {text}");
    // Faded (5 s, then 1 s to fade out).
    h.run_steps(40);
    assert!(!shown(h, text), "faded: {text}");
    say(h);
    h.run_steps(2);
    assert!(shown(h, text), "said again: {text}");
    // And it lasts its full time again.
    h.run_steps(12);
    assert!(shown(h, text), "for its full time: {text}");
}

fn opacity_5(h: &mut H) {
    h.key_press(Key::Num5);
}

fn merge_down(h: &mut H) {
    h.key_press_modifiers(Modifiers::COMMAND, Key::E);
}

#[test]
fn a_notice_said_again_shows_again_in_the_status_bar() {
    let mut h = app(Tool::Brush);
    let text = t!("status-tool-opacity", percent = 50);
    said_again(&mut h, &text, opacity_5, |h, t| in_status_bar(h, t));
}

#[test]
fn a_refusal_said_again_shows_again_in_the_status_bar() {
    // A single layer: nothing to merge it into.
    let mut h = app(Tool::Transform);
    let text = t!("status-merge-down-failed");
    said_again(&mut h, &text, merge_down, |h, t| in_status_bar(h, t));
}

#[test]
fn messages_said_again_show_again_in_full_screen() {
    let mut h = app(Tool::Brush);
    h.state_mut().set_screen_mode(workspace::ScreenMode::Full);
    h.run_steps(2);
    let text = t!("status-tool-opacity", percent = 50);
    said_again(&mut h, &text, opacity_5, in_full_screen);
    let text = t!("status-merge-down-failed");
    said_again(&mut h, &text, merge_down, in_full_screen);
}

/// A message only read meanwhile (as the action player and Batch read it)
/// still fades: only a new `set` restarts it.
#[test]
fn a_message_left_alone_fades() {
    let mut h = app(Tool::Brush);
    let text = t!("status-tool-opacity", percent = 50);
    opacity_5(&mut h);
    h.run_steps(2);
    assert!(in_status_bar(&h, &text));
    for _ in 0..40 {
        assert_eq!(h.state().status.to_string(), text);
        h.run_steps(1);
    }
    assert!(!in_status_bar(&h, &text), "faded");
}
