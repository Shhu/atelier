//! Color Picker and Swatches panel: opened by the color buttons, picking,
//! Cancel, sampling the image, swatches.

use super::*;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

const IMAGE: [u8; 4] = [90, 120, 200, 255];

fn app() -> Harness<'static, AtelierApp> {
    i18n::set_lang(Lang::En);
    let mut harness = Harness::builder()
        .with_size([1600.0, 1100.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(200, 150, image::Rgba(IMAGE));
            app.doc = Some(Document::new(img, None));
            app
        });
    harness.run_steps(3);
    harness
}

fn click(h: &mut Harness<'static, AtelierApp>, at: Pos2) {
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.drop_at(at);
    h.run_steps(3);
}

/// The foreground and background squares at the bottom of the tool bar.
fn color_square(h: &Harness<'static, AtelierApp>, foreground: bool) -> Pos2 {
    // Tool bar: 8 px margin, colors (40 px) then the Quick Mask button
    // (6 + 22 px) above a 12 px margin and the 28 px status bar.
    let bottom = h.ctx.content_rect().bottom() - 28.0 - 12.0 - 68.0;
    if foreground {
        pos2(8.0 + 11.0, bottom + 11.0)
    } else {
        pos2(8.0 + 25.0, bottom + 27.0)
    }
}

fn picker<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a color_picker::ColorPicker {
    h.state().colors.picker.as_ref().expect("Color Picker open")
}

/// Types `text` in place of the hex field's contents.
fn type_hex(h: &mut Harness<'static, AtelierApp>, text: &str) {
    let id = egui::Id::new("picker_hex");
    h.ctx.memory_mut(|m| m.request_focus(id));
    h.run_steps(2);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(egui::Event::Text(text.to_owned()));
    h.run_steps(3);
}

#[test]
fn foreground_square_opens_the_picker_and_ok_keeps_the_color() {
    let mut h = app();
    let square = color_square(&h, true);
    click(&mut h, square);
    assert_eq!(picker(&h).title, t!("uip-picker-foreground-title"));
    type_hex(&mut h, "#F80");
    assert_eq!(picker(&h).picked.rgb8(), [255, 136, 0]);
    // Shown live on the square.
    assert_eq!(h.state().primary, Color32::from_rgb(255, 136, 0));
    h.key_press(Key::Enter);
    h.run_steps(3);
    let app = h.state();
    assert!(app.colors.picker.is_none(), "closed");
    assert_eq!(app.primary, Color32::from_rgb(255, 136, 0));
    assert_eq!(app.colors.swatches.recent.first(), Some(&[255, 136, 0]));
}

#[test]
fn cancel_gives_the_color_back() {
    let mut h = app();
    let square = color_square(&h, false);
    click(&mut h, square);
    assert_eq!(picker(&h).title, t!("uip-picker-background-title"));
    type_hex(&mut h, "123456");
    assert_eq!(h.state().secondary, Color32::from_rgb(0x12, 0x34, 0x56));
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.state().colors.picker.is_none());
    assert_eq!(h.state().secondary, Color32::WHITE);
}

#[test]
fn a_click_on_the_image_samples_it() {
    let mut h = app();
    // Large enough to show around the picker.
    let img = image::RgbaImage::from_pixel(1200, 900, image::Rgba(IMAGE));
    h.state_mut().doc = Some(Document::new(img, None));
    h.state_mut().fit_pending = Some(1.0);
    h.run_steps(2);
    let square = color_square(&h, true);
    click(&mut h, square);
    let view = h.state().last_view.expect("canvas shown");
    let at = view.to_screen(pos2(20.0, 20.0));
    click(&mut h, at);
    let [r, g, b, _] = IMAGE;
    assert_eq!(picker(&h).picked.rgb8(), [r, g, b]);
    assert_eq!(h.state().doc.as_ref().unwrap().layers.len(), 1);
}

#[test]
fn field_and_strip_pick_in_hue_mode() {
    let mut h = app();
    let square = color_square(&h, true);
    click(&mut h, square);
    // Black: the field's bottom. Its top right corner is the pure hue.
    let field = egui::Rect::from_min_size(
        h.ctx
            .memory(|m| m.area_rect(egui::Id::new("color_picker")))
            .expect("picker shown")
            .min
            + vec2(16.0, 44.0 + 8.0 + 16.0),
        Vec2::splat(268.0),
    );
    click(&mut h, field.right_top() + vec2(-1.0, 1.0));
    let hsb = picker(&h).picked;
    assert_eq!(hsb.rgb8()[0], 255, "{hsb:?}");
    // Strip at a third of its height from the bottom: 120°, green.
    let strip = field.right_bottom() + vec2(10.0 + 10.0, -268.0 / 3.0);
    click(&mut h, strip);
    let [r, g, b] = picker(&h).picked.rgb8();
    assert!(g > 250 && r < 10 && b < 10, "{:?}", [r, g, b]);
}

#[test]
fn picker_over_the_fill_dialog_closes_alone() {
    let mut h = app();
    h.state_mut().run(Action::FillDialog);
    h.state_mut().fill_dialog.as_mut().unwrap().content = FillContent::Color;
    h.run_steps(3);
    let request = ui_kit::PickerRequest {
        target: egui::Id::new("test"),
        color: Color32::RED,
        title: t!("uip-picker-title"),
    };
    ui_kit::request_picker(&h.ctx, request);
    h.run_steps(3);
    assert!(h.state().colors.picker.is_some());
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.state().colors.picker.is_none(), "picker closed");
    assert!(h.state().fill_dialog.is_some(), "fill dialog still open");
}

#[test]
fn add_to_swatches_and_use_them() {
    let mut h = app();
    let square = color_square(&h, true);
    click(&mut h, square);
    type_hex(&mut h, "abcdef");
    let before = h.state().colors.swatches.colors.len();
    h.get_by_label(&t!("uip-picker-add-to-swatches")).click();
    h.run_steps(3);
    let swatches = &h.state().colors.swatches.colors;
    assert_eq!(swatches.len(), before + 1);
    assert_eq!(swatches.last(), Some(&[0xab, 0xcd, 0xef]));
    h.key_press(Key::Escape);
    h.run_steps(3);

    // Swatches panel: a click picks the foreground, Ctrl+click the
    // background, Alt+click deletes.
    h.state_mut().run(Action::Swatches);
    h.run_steps(3);
    h.get_by_label("#E8E8E8").click();
    h.run_steps(3);
    assert_eq!(h.state().primary, Color32::from_gray(232));
    assert_eq!(h.state().colors.swatches.recent, [[232; 3]]);
    h.get_by_label("#D1D1D1")
        .click_modifiers(Modifiers::COMMAND);
    h.run_steps(3);
    assert_eq!(h.state().secondary, Color32::from_gray(209));
    assert_eq!(h.state().primary, Color32::from_gray(232));
    let count = h.state().colors.swatches.colors.len();
    // The last one: the swatch, after the recent color.
    let node = h.get_all_by_label("#D1D1D1").last().expect("swatch");
    node.click_modifiers(Modifiers::ALT);
    h.run_steps(3);
    let swatches = &h.state().colors.swatches;
    assert_eq!(swatches.colors.len(), count - 1);
    assert!(!swatches.colors.contains(&[209; 3]));
}
