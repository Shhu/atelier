//! The interface with Inter 14 in narrow windows and in French: title bar,
//! menu rows, fields, panels, and what screen readers are told.

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

/// A press and release of the primary button at `at`, as the pointer does.
fn click_at(h: &mut H, at: Pos2) {
    h.hover_at(at);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(4);
}

/// The menu titles shown in the title bar, left to right.
fn menu_titles(h: &H) -> Vec<(String, Rect)> {
    let titles: Vec<String> = h.state().menus().into_iter().map(|(t, _)| t).collect();
    let mut shown: Vec<(String, Rect)> = titles
        .into_iter()
        .chain([t!("ui11-menu-more")])
        .filter_map(|t| {
            let rect = h
                .query_all_by_label(&t)
                .map(|n| n.rect())
                .find(|r| r.top() < 40.0)?;
            Some((t, rect))
        })
        .collect();
    shown.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    shown
}

/// In every language and width, the menus end clear of the window buttons,
/// with room to drag the window by; the last one opens where it is clicked.
/// What doesn't fit goes into "»", which opens the menus it holds.
#[test]
#[cfg_attr(target_os = "macos", ignore = "macOS draws its own window buttons")]
fn the_menus_stay_clear_of_the_window_buttons() {
    for lang in [Lang::Fr, Lang::En] {
        for width in [660.0, 690.0, 700.0, 1024.0] {
            let mut h = window(width, 500.0, lang);
            let what = format!("{} {width}", lang.code());
            let minimize = h.get_by_label(&t!("title-minimize")).rect();
            let titles = menu_titles(&h);
            let (last, rect) = titles.last().cloned().expect("menus");
            assert!(
                rect.right() + 24.0 <= minimize.left(),
                "{what}: {last} ends at {} and Minimize starts at {}",
                rect.right(),
                minimize.left()
            );
            // Every menu is reached: as a title, or inside "»".
            let more = t!("ui11-menu-more");
            let folded = titles.iter().any(|(t, _)| *t == more);
            if !folded {
                assert_eq!(titles.len(), h.state().menus().len(), "{what}");
            }
            // The visible end of the last title opens its menu.
            click_at(&mut h, pos2(rect.right() - 3.0, rect.center().y));
            let opened = if folded {
                let help = t!("ui3-menu-help");
                h.query_all_by_label_contains(&help)
                    .any(|n| n.rect().top() >= 40.0)
            } else {
                h.query_by_label_contains(&t!("diag-menu")).is_some()
            };
            assert!(opened, "{what}: {last} opened");
            assert!(
                h.ctx.viewport(|v| v.commands.is_empty()),
                "{what}: the window stays as it was"
            );
        }
    }
}

/// Menus that don't fit (here with larger titles) go into "»", which
/// opens them.
#[test]
#[cfg_attr(target_os = "macos", ignore = "macOS draws its own window buttons")]
fn menus_that_dont_fit_go_into_more() {
    let mut h = window(660.0, 500.0, Lang::Fr);
    h.ctx.all_styles_mut(|s| {
        if let Some(font) = s.text_styles.get_mut(&egui::TextStyle::Button) {
            font.size = 22.0;
        }
    });
    h.run_steps(3);
    let titles = menu_titles(&h);
    let more = t!("ui11-menu-more");
    let (last, rect) = titles.last().cloned().expect("menus");
    assert_eq!(last, more, "{titles:?}");
    let minimize = h.get_by_label(&t!("title-minimize")).rect();
    assert!(rect.right() + 24.0 <= minimize.left(), "{titles:?}");
    click_at(&mut h, rect.center());
    let help = t!("ui3-menu-help");
    let item = h
        .query_all_by_label_contains(&help)
        .map(|n| n.rect())
        .find(|r| r.top() >= 40.0)
        .expect("Help inside »");
    // Its submenu opens under the pointer.
    h.hover_at(item.center());
    h.run_steps(8);
    assert!(h.query_by_label_contains(&t!("diag-menu")).is_some());
}

/// No menu row wraps onto a second line, in a narrow window, in French
/// too: menus grow to their widest row.
#[test]
fn menu_rows_never_wrap() {
    for (lang, width) in [(Lang::Fr, 660.0), (Lang::Fr, 1024.0), (Lang::En, 1024.0)] {
        let mut h = window(width, 700.0, lang);
        let menus = h.state().menus();
        for (title, entries) in &menus {
            let labels: Vec<&str> = entries
                .iter()
                .filter_map(|e| match e {
                    MenuEntry::Item(l, ..) | MenuEntry::Check(l, ..) => Some(l.as_str()),
                    _ => None,
                })
                .collect();
            let shown = menu_titles(&h);
            let Some((_, rect)) = shown.iter().find(|(t, _)| t == title) else {
                continue;
            };
            // The menu's rows: what wasn't there before it opened.
            let rect_of = |n: &egui_kittest::Node<'_>| {
                let b = n.accesskit_node().bounding_box()?;
                Some(Rect::from_min_max(
                    pos2(b.x0 as f32, b.y0 as f32),
                    pos2(b.x1 as f32, b.y1 as f32),
                ))
            };
            let before: Vec<Rect> = h
                .query_all(By::new().predicate(|_| true))
                .filter_map(|n| rect_of(&n))
                .collect();
            click_at(&mut h, rect.center());
            for label in labels {
                for row in h.query_all_by_label_contains(label) {
                    let Some(r) = rect_of(&row) else { continue };
                    if r.top() < 40.0 || r.height() < 1.0 || before.contains(&r) {
                        continue;
                    }
                    assert!(
                        r.height() < 34.0,
                        "{} {title} › {label}: {} px high",
                        lang.code(),
                        r.height()
                    );
                }
            }
            // Closed again.
            h.key_press(egui::Key::Escape);
            h.run_steps(4);
        }
    }
}

/// An option of a menu is a check box for screen readers: named by its
/// words (no mark glyph), on or off.
#[test]
fn menu_options_are_named_and_say_whether_they_are_on() {
    use egui::accesskit::{Role, Toggled};
    let mut h = window(1280.0, 800.0, Lang::En);
    h.state_mut().hist_panel.open = true;
    h.run_steps(2);
    let window_menu = menu_titles(&h)
        .into_iter()
        .find(|(t, _)| *t == t!("uip-menu-window"))
        .expect("Window menu")
        .1;
    click_at(&mut h, window_menu.center());
    let rows: Vec<(String, Role, Option<Toggled>)> = h
        .query_all(By::new().predicate(|_| true))
        .map(|n| {
            let a = n.accesskit_node();
            (a.label().unwrap_or_default(), a.role(), a.toggled())
        })
        .filter(|(_, role, _)| *role == Role::CheckBox)
        .collect();
    let named = |label: &str| {
        rows.iter()
            .find(|(l, ..)| l.starts_with(label))
            .unwrap_or_else(|| panic!("{label} in {rows:?}"))
            .clone()
    };
    assert_eq!(named(&t!("panels8-menu-histogram")).2, Some(Toggled::True));
    assert_eq!(named(&t!("panels8-menu-navigator")).2, Some(Toggled::False));
    let glyph = |c: char| ('\u{e000}'..='\u{f8ff}').contains(&c);
    let labels: Vec<String> = h
        .query_all(By::new().predicate(|_| true))
        .filter_map(|n| n.accesskit_node().label())
        .filter(|l| l.chars().any(glyph) && l.chars().any(char::is_alphabetic))
        .collect();
    assert!(labels.is_empty(), "glyphs in names: {labels:?}");
}

/// A long value widens its number field: it never runs into the label.
#[test]
fn a_long_value_stays_clear_of_its_label() {
    for lang in [Lang::En, Lang::Fr] {
        i18n::set_lang(lang);
        let mut value = 1600.0_f32;
        let (mut label_end, mut shown) = (0.0, Rect::NOTHING);
        let mut installed = false;
        let mut h = Harness::new_ui(|ui| {
            // The app's fonts, from the next frame on.
            if !installed {
                ui_kit::install(ui.ctx());
                installed = true;
                return;
            }
            let left = ui.cursor().left();
            let label =
                ui.painter()
                    .layout_no_wrap("W".into(), ui_kit::medium(11.0), Color32::WHITE);
            label_end = left + 8.0 + label.size().x;
            shown = ui_kit::number_field(ui, "W", &mut value, 0.0..=30000.0, " px", 0.0).rect;
        });
        h.run_steps(3);
        drop(h);
        assert!(
            shown.left() >= label_end + 2.0,
            "{}: value from {} over a label ending at {label_end}",
            lang.code(),
            shown.left()
        );
    }
    i18n::set_lang(Lang::En);
}

/// The Layer Style list is as wide as its widest style name, in French too.
#[test]
fn the_style_list_fits_its_longest_name() {
    for lang in [Lang::En, Lang::Fr] {
        let mut h = window(1024.0, 700.0, lang);
        h.state_mut().doc.as_mut().unwrap().add_empty_layer();
        h.state_mut().run(Action::LayerStyle(None));
        h.run_steps(6);
        let blend = t!("panel-blend-mode");
        let combo = h
            .query_all_by_label(&blend)
            .map(|n| n.rect())
            .min_by(|a, b| a.left().total_cmp(&b.left()))
            .expect("blend mode");
        let font = ui_kit::medium(ui_kit::TEXT_SIZE);
        let names: Vec<String> = crate::compose::styles::Style::ALL
            .iter()
            .map(|s| s.label())
            .collect();
        let widest = h.ctx.fonts_mut(|f| {
            names
                .iter()
                .map(|n| {
                    f.layout_no_wrap(n.clone(), font.clone(), Color32::WHITE)
                        .size()
                        .x
                })
                .fold(0.0, f32::max)
        });
        // A row of the list, left of the settings.
        let row = h
            .query_all_by_label(&names[0])
            .map(|n| n.rect())
            .find(|r| r.right() < combo.left() && r.height() >= 28.0)
            .expect("the list's first row");
        let what = lang.code();
        assert!(
            row.width() >= 36.0 + widest + 4.0,
            "{what}: names {widest} px wide in a {} px row",
            row.width()
        );
        // The settings column: its 110 px label, then the combo.
        assert!(
            row.right() + 8.0 <= combo.left() - 110.0,
            "{what}: the list ends at {}, the settings start at {}",
            row.right(),
            combo.left() - 110.0
        );
    }
}

/// French statistics write their decimals with a comma.
#[test]
fn histogram_statistics_use_the_decimal_comma() {
    let mut h = window(1280.0, 800.0, Lang::Fr);
    h.state_mut()
        .run(Action::Workspace(workspace::Command::Histogram));
    let mean = t!("panels8-histogram-mean");
    let computing = t!("panels8-histogram-computing");
    let mut text = String::new();
    for _ in crate::testing::patience() {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
        text = h
            .query_all(By::new().predicate(|_| true))
            .filter_map(|n| {
                let a = n.accesskit_node();
                a.value().or(a.label())
            })
            .collect::<Vec<_>>()
            .join(" | ");
        if text.contains(&mean) && !text.contains(&computing) {
            break;
        }
    }
    assert!(text.contains(&mean), "{text}");
    // The image is one colour: its RGB mean is (90 + 120 + 200) / 3.
    assert!(text.contains("136,67"), "a mean with a comma: {text}");
    let point = regex_free_decimal_point(&text);
    assert!(point.is_none(), "a decimal point: {point:?} in {text}");
}

/// The first "12.34" (digits, a point, two digits) in `text`.
fn regex_free_decimal_point(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(4).find_map(|w| {
        (w[0].is_ascii_digit() && w[1] == '.' && w[2].is_ascii_digit() && w[3].is_ascii_digit())
            .then(|| w.iter().collect())
    })
}

/// A freeze in the Delete Pattern confirmation or in Camera Raw names it
/// in the interface's language.
#[test]
fn diagnostics_names_the_delete_pattern_dialog() {
    i18n::set_lang(Lang::Fr);
    assert_eq!(
        diagnostics::dialog_label("Delete Pattern"),
        t!("pattern9-delete-title")
    );
    assert_eq!(
        diagnostics::dialog_label("Camera Raw"),
        t!("raw-window-title")
    );
}
