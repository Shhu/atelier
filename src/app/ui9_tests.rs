//! Layout at small sizes, accessible names and words of the interface,
//! through real events.

use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::{By, NodeT, Queryable as _};

type H = Harness<'static, AtelierApp>;

fn window(w: f32, h: f32, lang: Lang) -> H {
    i18n::set_lang(lang);
    let mut harness = Harness::builder().with_size([w, h]).build_eframe(|cc| {
        let mut app = AtelierApp::new(cc, None);
        let img = image::RgbaImage::from_pixel(512, 512, image::Rgba([90, 120, 200, 255]));
        app.doc = Some(Document::new(img, Some("photo.png".into())));
        app
    });
    harness.run_steps(3);
    harness
}

fn tab(h: &mut H) {
    h.key_press(egui::Key::Tab);
    // The focus goes from one widget to the next, one a frame.
    h.run_steps(30);
}

/// The widget with the keyboard: role and name.
fn focused(h: &H) -> String {
    h.query_all(By::new().predicate(|_| true))
        .find(|n| n.accesskit_node().is_focused())
        .map(|n| {
            let a = n.accesskit_node();
            format!("{:?} {}", a.role(), a.label().unwrap_or_default())
        })
        .unwrap_or_default()
}

fn is_glyph(c: char) -> bool {
    ('\u{e000}'..='\u{f8ff}').contains(&c)
}

/// No control is named only by a glyph, nor has one as its value.
fn assert_named(h: &H, what: &str) {
    use egui::accesskit::Role;
    let only_glyphs =
        |text: &str| !text.is_empty() && text.chars().all(|c| c.is_whitespace() || is_glyph(c));
    let bad: Vec<String> = h
        .query_all(By::new().predicate(|_| true))
        .filter_map(|n| {
            let a = n.accesskit_node();
            let control = matches!(
                a.role(),
                Role::Button | Role::ComboBox | Role::SpinButton | Role::CheckBox | Role::Slider
            );
            let (label, value) = (a.label().unwrap_or_default(), a.value().unwrap_or_default());
            (control && (only_glyphs(&label) || only_glyphs(&value))).then(|| {
                format!(
                    "{:?} {label:?} = {value:?} at {:?}",
                    a.role(),
                    a.bounding_box()
                )
            })
        })
        .collect();
    assert!(bad.is_empty(), "{what}: {bad:#?}");
}

/// Tab in a dialog goes from text field to text field: Stroke like Fill,
/// which it was checked against (Keyboard focus is for text fields only).
#[test]
fn tab_in_the_stroke_dialog_goes_like_in_older_dialogs() {
    let visits = |action| {
        let mut h = window(1280.0, 800.0, Lang::En);
        h.state_mut().run(action);
        h.run_steps(6);
        let mut seen = vec![focused(&h)];
        for _ in 0..3 {
            tab(&mut h);
            seen.push(focused(&h));
        }
        seen
    };
    let stroke = visits(Action::Stroke);
    let fill = visits(Action::FillDialog);
    let width = "SpinButton Width".to_owned();
    let opacity = "SpinButton Opacity".to_owned();
    assert_eq!(
        stroke,
        [width.clone(), opacity.clone(), width, opacity.clone()]
    );
    assert!(fill.iter().all(|f| f == &opacity), "{fill:?}");
}

/// The Layer menu at 660×500: its last rows are out of the window, an arrow
/// says so, and resting the pointer on it brings them.
#[test]
fn a_clipped_menu_says_it_goes_on_and_scrolls_under_the_pointer() {
    let mut h = window(660.0, 500.0, Lang::En);
    h.get_by_label(&t!("menu-layer")).click();
    h.run_steps(6);
    let screen = h.ctx.content_rect();
    let last = t!("menu-layer-flatten");
    let bottom = |h: &H| h.get_by_label(&last).rect().bottom();
    assert!(bottom(&h) > screen.bottom(), "the menu is clipped");
    let lowest_row = bottom(&h);
    // The pointer on the strip at the bottom of the menu (see `titlebar`).
    let new_layer = t!("menu-layer-new");
    let top = |h: &H| {
        h.query_all_by_label_contains(&new_layer)
            .map(|n| n.rect())
            .min_by(|a, b| a.top().total_cmp(&b.top()))
            .unwrap()
    };
    let first = top(&h);
    let strip = pos2(first.center().x, first.top() + screen.height() - 64.0 - 9.0);
    h.hover_at(strip);
    h.run_steps(120);
    assert!(bottom(&h) < lowest_row, "it scrolled");
    assert!(
        bottom(&h) <= screen.bottom(),
        "Flatten is reached: {}",
        bottom(&h)
    );
    // Up again from the strip at the top, which has appeared.
    h.hover_at(pos2(strip.x, first.top() + 9.0));
    h.run_steps(120);
    assert_eq!(top(&h).top(), first.top());
}

/// At 660×500 the locks go into the heading and the list shows 3 rows; a
/// lock there works through a real click.
#[test]
fn the_layers_list_stays_usable_in_a_low_window() {
    let mut h = window(660.0, 500.0, Lang::Fr);
    let doc = h.state_mut().doc.as_mut().unwrap();
    for _ in 0..5 {
        doc.add_empty_layer();
    }
    h.run_steps(4);
    assert!(
        h.query_by_label(&t!("layers8-lock-label")).is_none(),
        "no row of its own"
    );
    let screen = h.ctx.content_rect();
    let heading = h.get_by_label(&t!("panel-layers")).rect();
    let lock = t!("layers8-lock-position");
    let button = h.get_by_label(&lock).rect();
    assert!(heading.contains_rect(button), "{button:?} in {heading:?}");
    assert!(screen.contains_rect(button));
    h.get_by_label(&lock).click();
    h.run_steps(4);
    let doc = h.state().doc.as_ref().unwrap();
    assert!(doc.active_layer().locks.position);
    // The list: between the blend row (36) and the footer (41), above the
    // status bar (28), 3 rows of 44 and their gaps.
    let list = screen.bottom() - 28.0 - 41.0 - (heading.bottom() + 36.0);
    assert!(list >= 3.0 * 44.0 + 4.0, "{list}");
    // A tall panel keeps the row, with its word.
    let h = window(1280.0, 800.0, Lang::Fr);
    assert!(h.query_by_label(&t!("layers8-lock-label")).is_some());
}

/// Diagnostics names the dialogs in the UI's language; Fade and Stroke are
/// not "Filter".
#[test]
fn diagnostics_names_every_new_dialog() {
    let mut h = window(1024.0, 700.0, Lang::Fr);
    h.state_mut().run(Action::Stroke);
    h.run_steps(3);
    assert_eq!(h.state().dialog_name(), Some("Stroke"));
    assert_eq!(
        diagnostics::dialog_label("Stroke"),
        t!("adjust8-stroke-title")
    );
    assert_eq!(diagnostics::dialog_label("Fade"), t!("adjust8-fade-title"));
    assert_eq!(
        diagnostics::dialog_label("Rename Pattern"),
        t!("pattern8-rename-title")
    );
    assert_eq!(
        diagnostics::dialog_label("Define Pattern"),
        t!("pattern8-menu-define").trim_end_matches('…')
    );
}

/// A lock's refusal is a warning in the log, in words only: nothing failed.
#[test]
fn a_lock_refusal_is_no_error() {
    let mut h = window(1024.0, 700.0, Lang::En);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locks
        .position = true;
    let (_, _, errors) = crate::diag::log_entries();
    let name = h.state().doc.as_ref().unwrap().active_layer().name.clone();
    assert!(h.state_mut().refuse_position_locked());
    let (log, _, after) = crate::diag::log_entries();
    assert_eq!(errors, after);
    let line = log
        .iter()
        .rev()
        .find(|l| l.english.contains(&name))
        .unwrap();
    assert!(!line.english.chars().any(is_glyph), "{}", line.english);
    assert!(
        h.state().workspace.error_status.is_some(),
        "still in the warning color"
    );
}

/// With only Lock Position on, the Move tool's bar doesn't say the layer is
/// locked as a whole.
#[test]
fn the_move_bar_names_a_partial_lock() {
    let mut h = window(1024.0, 700.0, Lang::En);
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locks
        .position = true;
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains(&t!("ui9-options-position-locked"))
            .next()
            .is_some()
    );
    assert!(
        h.query_all_by_label_contains(&t!("options-layer-locked"))
            .next()
            .is_none()
    );
    h.state_mut()
        .doc
        .as_mut()
        .unwrap()
        .active_layer_mut()
        .locked = true;
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains(&t!("options-layer-locked"))
            .next()
            .is_some()
    );
}

/// At 660 px the Color Sampler's hint takes a row of its own rather than
/// going: nothing else says how to remove a point.
#[test]
fn the_sampler_hint_stays_in_a_narrow_window() {
    let mut h = window(660.0, 500.0, Lang::En);
    h.state_mut().run(Action::Tool(Tool::ColorSampler));
    h.run_steps(4);
    let alt = keys::click(&h.ctx, Modifiers::ALT);
    let hint = t!("panels8-sampler-hint", alt_click = alt);
    let rect = h.get_by_label(&hint).rect();
    assert!(h.ctx.content_rect().contains_rect(rect), "{rect:?}");
}

/// An action on F8 takes the key: the menu and Help › Shortcuts stop giving
/// it to Info. Shift+F and holding R are listed.
#[test]
fn shortcuts_tell_what_the_keys_really_do() {
    let mut h = window(1024.0, 700.0, Lang::En);
    let f8 = |h: &H| {
        h.state()
            .shortcut_rows(&h.ctx)
            .iter()
            .any(|(_, what, key)| what == &t!("panels8-menu-info") && key == "F8")
    };
    assert!(f8(&h));
    let rows = h.state().shortcut_rows(&h.ctx);
    let has = |what: String| rows.iter().any(|(_, w, _)| *w == what);
    assert!(has(t!("ui9-shortcut-screen-mode-back")));
    assert!(has(t!("ui9-shortcut-rotate-view-hold")));
    let library = &mut h.state_mut().actions.library;
    library.sets[0].actions[0].key = Some(crate::actions::FnKey {
        number: 8,
        shift: false,
        ctrl: false,
    });
    assert!(!f8(&h));
}

/// The controls labelled by a glyph have a name for screen readers.
#[test]
fn glyph_controls_have_names() {
    let mut h = window(1600.0, 1000.0, Lang::En);
    h.state_mut().run(Action::Tool(Tool::Canvas));
    h.run_steps(4);
    assert_named(&h, "crop bar");
    let name = t!("crop8-overlay");
    let value = h
        .get_by_label(&name)
        .accesskit_node()
        .value()
        .unwrap_or_default();
    assert_eq!(value, h.state().crop.overlay.label());
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Navigator));
    h.run_steps(6);
    assert_named(&h, "Navigator");
    assert!(h.query_by_label(&t!("ui9-navigator-image")).is_some());
    h.state_mut().run(Action::NewPatternFill);
    h.run_steps(6);
    assert_named(&h, "Pattern Fill");
    assert!(h.query_by_label(&t!("core-gradient-angle-tip")).is_some());
}
