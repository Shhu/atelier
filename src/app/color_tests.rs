//! Color management: Color Settings, Assign Profile, Convert to Profile,
//! Proof Setup, Proof Colors (Ctrl+Y), Gamut Warning (Shift+Ctrl+Y),
//! Image › Mode › Grayscale, the Color Picker's CMYK values, CMYK exports.

use super::color_ui::{Assigned, Dialog};
use super::*;
use crate::cms::{self, WorkingRgb};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

const BLUE: [u8; 4] = [0, 0, 255, 255];

fn app() -> Harness<'static, AtelierApp> {
    i18n::set_lang(Lang::En);
    let mut harness = Harness::builder()
        .with_size([1600.0, 1100.0])
        .build_eframe(|cc| {
            let mut app = AtelierApp::new(cc, None);
            let img = image::RgbaImage::from_pixel(200, 150, image::Rgba(BLUE));
            app.doc = Some(Document::new(img, None));
            app
        });
    harness.run_steps(3);
    harness
}

fn doc<'a>(h: &'a Harness<'static, AtelierApp>) -> &'a Document {
    h.state().doc.as_ref().expect("a document")
}

/// What the canvas shows `rgb` as.
fn shown(h: &mut Harness<'static, AtelierApp>, rgb: [u8; 3]) -> [u8; 3] {
    let app = h.state_mut();
    let doc = app.doc.take().expect("a document");
    let (_, lut) = app.color.canvas_lut(&doc);
    app.doc = Some(doc);
    lut.map_or(rgb, |l| l.apply(rgb))
}

fn shortcut(h: &mut Harness<'static, AtelierApp>, shift: bool, key: Key) {
    let m = if shift {
        Modifiers::COMMAND.plus(Modifiers::SHIFT)
    } else {
        Modifiers::COMMAND
    };
    h.key_press_modifiers(m, key);
    h.run_steps(3);
}

#[test]
fn ctrl_y_proofs_the_canvas_and_shift_ctrl_y_warns() {
    let mut h = app();
    // sRGB on an sRGB monitor: nothing to convert.
    assert_eq!(shown(&mut h, [0, 0, 255]), [0, 0, 255]);
    shortcut(&mut h, false, Key::Y);
    assert!(doc(&h).proof.colors);
    let proofed = shown(&mut h, [0, 0, 255]);
    assert_ne!(proofed, [0, 0, 255], "printing dulls pure blue");
    let gray = shown(&mut h, [128, 128, 128]);
    assert!(gray.iter().all(|&c| c.abs_diff(128) <= 6), "{gray:?}");
    shortcut(&mut h, true, Key::Y);
    assert!(doc(&h).proof.gamut);
    assert_eq!(shown(&mut h, [0, 255, 0]), cms::GAMUT_GRAY);
    assert_ne!(shown(&mut h, [200, 150, 120]), cms::GAMUT_GRAY);
    // Off again: the gamut warning alone.
    shortcut(&mut h, false, Key::Y);
    assert!(!doc(&h).proof.colors && doc(&h).proof.gamut);
    assert_eq!(shown(&mut h, [120, 140, 160]), [120, 140, 160]);
    shortcut(&mut h, true, Key::Y);
    assert_eq!(shown(&mut h, [0, 255, 0]), [0, 255, 0]);
}

#[test]
fn view_menu_has_proof_setup_and_the_shortcuts() {
    let mut h = app();
    h.get_by_label(&t!("menu-view")).click();
    h.run_steps(3);
    h.get_by_label_contains(&t!("color-menu-proof-colors"));
    h.get_by_label_contains("Shift+Ctrl+Y");
    h.get_by_label(&format!("{} ⏵", t!("color-menu-proof-setup")))
        .hover();
    h.run_steps(3);
    h.get_by_label(&t!("color-menu-proof-custom")).click();
    h.run_steps(3);
    assert!(matches!(h.state().color.dialog, Some(Dialog::Proof { .. })));
    // The preview shows the proof while the dialog is open.
    assert_ne!(shown(&mut h, [0, 0, 255]), [0, 0, 255]);
    let Some(Dialog::Proof { setup, .. }) = &mut h.state_mut().color.dialog else {
        panic!("Customize Proof Condition open");
    };
    setup.paper = true;
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert!(h.state().color.dialog.is_none());
    let proof = &doc(&h).proof;
    assert!(proof.paper && proof.colors && proof.custom.is_none());
    // The paper's white, darker than the monitor's.
    assert!(shown(&mut h, [255; 3]).iter().any(|&c| c < 250));
}

#[test]
fn convert_to_profile_converts_every_layer_in_one_step() {
    let mut h = app();
    h.state_mut().run(Action::NewLayer);
    h.run_steps(2);
    let steps = doc(&h).history_step();
    h.get_by_label(&t!("menu-edit")).click();
    h.run_steps(3);
    h.get_by_label(&t!("color-menu-convert-profile")).click();
    h.run_steps(3);
    let Some(Dialog::Convert { to, .. }) = &mut h.state_mut().color.dialog else {
        panic!("Convert to Profile open");
    };
    assert_eq!(to.name(), cms::srgb().name(), "the working RGB first");
    *to = WorkingRgb::AdobeRgb.profile().clone();
    h.run_steps(2);
    // Previewed: the same look on the canvas.
    let blue = shown(&mut h, [0, 0, 255]);
    assert!(blue[2] >= 250 && blue[0] <= 3, "{blue:?}");
    h.key_press(Key::Enter);
    h.run_steps(3);
    let d = doc(&h);
    let adobe = WorkingRgb::AdobeRgb.profile();
    assert_eq!(d.icc_profile.as_deref(), Some(adobe.icc().as_ref()));
    let px = d.layers[0].pixel_at(10, 10).unwrap();
    assert!(
        px[0] <= 2 && px[1] <= 2 && (245..=252).contains(&px[2]),
        "{px:?}"
    );
    assert_eq!(d.history_step(), steps + 1);
    // Shown as before, through the new profile.
    let back = shown(&mut h, [px[0], px[1], px[2]]);
    assert!(back[2] >= 252 && back[0] <= 3, "{back:?}");
    h.state_mut().run(Action::Undo);
    h.run_steps(2);
    assert_eq!(doc(&h).layers[0].pixel_at(10, 10), Some(BLUE));
    assert_eq!(doc(&h).icc_profile, None);
}

#[test]
fn assign_profile_changes_the_look_not_the_numbers() {
    let mut h = app();
    h.state_mut()
        .run(Action::Color(color_ui::Command::AssignProfile));
    h.run_steps(2);
    let Some(Dialog::Assign { choice, .. }) = &mut h.state_mut().color.dialog else {
        panic!("Assign Profile open");
    };
    assert!(*choice == Assigned::None);
    *choice = Assigned::Profile(WorkingRgb::AdobeRgb.profile().clone());
    h.run_steps(2);
    // Previewed: Adobe RGB's blue is beyond sRGB's.
    assert_ne!(shown(&mut h, [0, 128, 255]), [0, 128, 255]);
    h.key_press(Key::Enter);
    h.run_steps(3);
    let d = doc(&h);
    assert_eq!(d.layers[0].pixel_at(0, 0), Some(BLUE));
    let adobe = WorkingRgb::AdobeRgb.profile();
    assert_eq!(&d.profile_or(cms::srgb()), adobe);
    let name = t!("color-adobe-rgb-compatible");
    let said = t!("color-status-assigned", name = name);
    assert_eq!(h.state().status, said);
}

#[test]
fn color_settings_change_the_working_spaces() {
    let mut h = app();
    shortcut(&mut h, true, Key::K);
    let Some(Dialog::Settings(s)) = &mut h.state_mut().color.dialog else {
        panic!("Color Settings open");
    };
    assert!(s.bpc, "on by default");
    s.bpc = false;
    s.rgb = WorkingRgb::ProPhoto;
    h.run_steps(2);
    h.key_press(Key::Enter);
    h.run_steps(3);
    let color = &h.state().color;
    assert_eq!(color.settings.rgb, WorkingRgb::ProPhoto);
    assert!(!color.settings.bpc);
    assert_eq!(color.working_cmyk().name(), cms::default_cmyk().name());
    // The document has no profile: it is now read as ProPhoto RGB (its
    // colors more saturated than sRGB's).
    let [r, g, b] = shown(&mut h, [100, 150, 100]);
    assert!(g > 165 && r < 80 && b > 100, "{:?}", [r, g, b]);
}

#[test]
fn grayscale_mode_discards_the_colors() {
    let mut h = app();
    h.get_by_label(&t!("menu-image")).click();
    h.run_steps(3);
    h.get_by_label(&format!("{} ⏵", t!("color-menu-mode")))
        .hover();
    h.run_steps(3);
    h.get_by_label_contains(&t!("color-menu-mode-grayscale"))
        .click();
    h.run_steps(3);
    assert!(matches!(h.state().color.dialog, Some(Dialog::Grayscale)));
    h.key_press(Key::Enter);
    h.run_steps(3);
    let [r, g, b, a] = doc(&h).layers[0].pixel_at(5, 5).unwrap();
    assert!(r == g && g == b && a == 255, "{:?}", [r, g, b]);
    // Blue is dark: its lightness, not its brightness.
    assert!(r < 90, "{r}");
}

#[test]
fn picker_shows_cmyk_and_offers_the_closest_printable_color() {
    let mut h = app();
    ui_kit::request_picker(
        &h.ctx,
        ui_kit::PickerRequest {
            target: egui::Id::new("test-target"),
            title: "Color".into(),
            color: Color32::from_rgb(0, 0, 255),
        },
    );
    h.run_steps(4);
    let ink = h
        .state()
        .colors
        .picker
        .as_ref()
        .and_then(|p| p.ink)
        .expect("CMYK known");
    assert!(!ink.printable);
    assert!(ink.cmyk[0] > 80.0 && ink.cmyk[1] > 60.0, "{:?}", ink.cmyk);
    h.get_by_label(&t!("color-picker-gamut")).click();
    h.run_steps(4);
    let picker = h.state().colors.picker.as_ref().expect("still open");
    assert_eq!(picker.picked.rgb8(), ink.closest);
    let now = picker.ink.expect("known");
    assert!(now.printable, "{now:?}");
    assert!(h.query_by_label(&t!("color-picker-gamut")).is_none());
}

#[test]
fn export_with_use_proof_setup_writes_cmyk() {
    let h = app();
    let dir = std::env::temp_dir().join(format!("atelier-cmyk-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("print.jpg");
    let options = crate::export::ExportOptions {
        format: crate::document::ExportFormat::Jpeg,
        use_proof: true,
        ..Default::default()
    };
    h.state().write_export(&options, &path).unwrap();
    let mut decoder = jpeg_decoder::Decoder::new(std::fs::File::open(&path).unwrap());
    let pixels = decoder.decode().unwrap();
    let info = decoder.info().unwrap();
    assert_eq!(info.pixel_format, jpeg_decoder::PixelFormat::CMYK32);
    assert_eq!(
        decoder.icc_profile().as_deref(),
        Some(cms::default_cmyk().icc().as_slice())
    );
    // Blue prints with cyan and magenta, no yellow.
    assert!(
        pixels[0] > 150 && pixels[1] > 120 && pixels[2] < 40,
        "{:?}",
        &pixels[..4]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_new_is_in_the_working_rgb() {
    let mut h = app();
    h.state_mut().color.settings.rgb = WorkingRgb::DisplayP3;
    h.state_mut().run(Action::NewDocument);
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    let p3 = WorkingRgb::DisplayP3.profile();
    assert_eq!(doc(&h).icc_profile.as_deref(), Some(p3.icc().as_ref()));
    // sRGB documents go without a profile, as before.
    h.state_mut().color.settings.rgb = WorkingRgb::Srgb;
    h.state_mut().run(Action::NewDocument);
    h.run_steps(3);
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(doc(&h).icc_profile, None);
}
