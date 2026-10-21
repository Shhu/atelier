//! Color management, like Photoshop's: Edit › Color Settings (working
//! spaces, intent, black point compensation), Assign Profile and Convert
//! to Profile; View › Proof Setup, Proof Colors (Ctrl+Y) and Gamut Warning
//! (Shift+Ctrl+Y); Image › Mode; the canvas shown through the monitor's
//! profile.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::dialogs::{DialogSpec, block, floating_dialog, gap_between_blocks, help};
use super::*;
use crate::cms::{self, Conversion, Grayscale, Intent, Lut, Profile, Proof, Space, WorkingRgb};
use crate::document::ProofSetup;

/// Proof Colors (Ctrl+Y), Gamut Warning (Shift+Ctrl+Y), Color Settings
/// (Shift+Ctrl+K), like Photoshop.
pub(super) const SHORTCUTS: [(KeyboardShortcut, Command); 3] = [
    (
        KeyboardShortcut::new(Modifiers::COMMAND, Key::Y),
        Command::ProofColors,
    ),
    (
        KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Y),
        Command::GamutWarning,
    ),
    (
        KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::K),
        Command::Settings,
    ),
];

/// Commands of the Edit, View and Image menus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Command {
    /// Edit › Color Settings…
    Settings,
    AssignProfile,
    ConvertToProfile,
    /// View › Proof Setup › Working CMYK.
    ProofWorkingCmyk,
    /// View › Proof Setup › Custom…
    ProofCustom,
    ProofColors,
    GamutWarning,
    /// Image › Mode › Grayscale (and RGB Color, the mode documents are in).
    Grayscale,
    Rgb,
}

/// Edit › Color Settings, kept between sessions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ColorSettings {
    /// Working RGB: the profile of documents without one.
    pub rgb: WorkingRgb,
    /// Working CMYK, a profile file; `None`: the one Atelier comes with.
    pub cmyk: Option<PathBuf>,
    pub intent: Intent,
    pub bpc: bool,
}

impl Default for ColorSettings {
    fn default() -> Self {
        Self {
            rgb: WorkingRgb::Srgb,
            cmyk: None,
            intent: Intent::RelativeColorimetric,
            bpc: true,
        }
    }
}

impl ColorSettings {
    /// Saved settings; tests start from the default ones.
    fn load() -> Self {
        if cfg!(test) {
            return Self::default();
        }
        std::fs::read(settings_file())
            .ok()
            .and_then(|json| serde_json::from_slice(&json).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if cfg!(test) {
            return;
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::create_dir_all(crate::config::dir());
            let _ = std::fs::write(settings_file(), json);
        }
    }

    /// The working CMYK profile: the file chosen if it still is one.
    fn cmyk_profile(&self) -> Profile {
        self.cmyk
            .as_deref()
            .and_then(Profile::load)
            .filter(|p| p.space() == Space::Cmyk)
            .unwrap_or_else(|| cms::default_cmyk().clone())
    }
}

fn settings_file() -> PathBuf {
    crate::config::dir().join("color.json")
}

/// Edit › Assign Profile's choice.
#[derive(Clone, PartialEq)]
pub(super) enum Assigned {
    /// Don't Color Manage This Document: read as the working RGB.
    None,
    Profile(Profile),
}

/// The open dialog, and its settings.
pub(super) enum Dialog {
    Settings(ColorSettings),
    Assign {
        choice: Assigned,
        preview: bool,
    },
    Convert {
        to: Profile,
        intent: Intent,
        bpc: bool,
        preview: bool,
    },
    Proof {
        setup: ProofSetup,
        preview: bool,
    },
    /// "Discard color information?" before Image › Mode › Grayscale.
    Grayscale,
}

/// The color settings, the working and monitor profiles, the open dialog,
/// and what the canvas shows.
pub(super) struct ColorUi {
    pub(super) settings: ColorSettings,
    cmyk: Profile,
    /// The monitor's profile (sRGB when the system gives none).
    monitor: Profile,
    pub(super) dialog: Option<Dialog>,
    /// The canvas's table for what it shows (fingerprint): `None`, the
    /// colors as they are.
    canvas: Option<(u64, Option<Arc<Lut>>)>,
}

impl ColorUi {
    pub(super) fn load() -> Self {
        let settings = ColorSettings::load();
        Self {
            cmyk: settings.cmyk_profile(),
            settings,
            monitor: cms::monitor_profile().unwrap_or_else(|| cms::srgb().clone()),
            dialog: None,
            canvas: None,
        }
    }

    pub(super) fn working_rgb(&self) -> &'static Profile {
        self.settings.rgb.profile()
    }

    pub(super) fn working_cmyk(&self) -> &Profile {
        &self.cmyk
    }

    /// The profile of `doc`'s pixels.
    pub(super) fn profile_of(&self, doc: &Document) -> Profile {
        doc.profile_or(self.working_rgb())
    }

    /// The printing condition of a proof setup.
    pub(super) fn proof(&self, setup: &ProofSetup) -> Proof {
        Proof {
            profile: setup.custom.clone().unwrap_or_else(|| self.cmyk.clone()),
            intent: setup.intent,
            bpc: setup.bpc,
            paper: setup.paper,
            ink: setup.ink || setup.paper,
        }
    }

    /// The working conversion of `doc`'s colors to the working CMYK
    /// (Color Picker values).
    pub(super) fn to_cmyk<'a>(&'a self, from: &'a Profile) -> Conversion<'a> {
        Conversion {
            from,
            to: &self.cmyk,
            intent: self.settings.intent,
            bpc: self.settings.bpc,
        }
    }

    /// A new document (File › New) in the working RGB: tagged with it
    /// unless it is sRGB (what documents without a profile are taken for
    /// elsewhere).
    pub(super) fn tag_new(&self, doc: &mut Document) {
        let rgb = self.working_rgb();
        if rgb != cms::srgb() {
            doc.icc_profile = Some(Arc::clone(rgb.icc()));
        }
    }

    pub(super) fn modal_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// What the canvas shows `doc` through: its fingerprint and its
    /// table (`None`: the colors as they are, the document's profile being
    /// the monitor's). A dialog's preview counts.
    pub(super) fn canvas_lut(&mut self, doc: &Document) -> (u64, Option<Arc<Lut>>) {
        fn ptr(p: &Profile) -> (usize, usize, &[u8]) {
            fingerprint(p.icc())
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        doc.icc_profile.as_ref().map(fingerprint).hash(&mut h);
        (self.settings.rgb as u8, ptr(&self.cmyk), ptr(&self.monitor)).hash(&mut h);
        let setup = match &self.dialog {
            Some(Dialog::Proof {
                setup,
                preview: true,
            }) => Some(ProofSetup {
                colors: true,
                ..setup.clone()
            }),
            _ => None,
        };
        let shown = setup.as_ref().unwrap_or(&doc.proof);
        let custom = shown.custom.as_ref().map(ptr);
        let proof_key = (
            custom,
            shown.intent as u8,
            shown.bpc,
            shown.paper,
            shown.ink,
        );
        (proof_key, shown.colors, shown.gamut).hash(&mut h);
        match &self.dialog {
            Some(Dialog::Assign {
                choice,
                preview: true,
            }) => {
                1u8.hash(&mut h);
                if let Assigned::Profile(p) = choice {
                    ptr(p).hash(&mut h);
                }
            }
            Some(Dialog::Convert {
                to,
                intent,
                bpc,
                preview: true,
            }) => (2u8, ptr(to), *intent as u8, *bpc).hash(&mut h),
            _ => {}
        }
        let key = h.finish();
        if let Some((k, lut)) = &self.canvas
            && *k == key
        {
            return (key, lut.clone());
        }
        let monitor = &self.monitor;
        let profile = match &self.dialog {
            Some(Dialog::Assign {
                choice,
                preview: true,
            }) => match choice {
                Assigned::None => self.working_rgb().clone(),
                Assigned::Profile(p) => p.clone(),
            },
            _ => self.profile_of(doc),
        };
        let lut = match &self.dialog {
            Some(Dialog::Convert {
                to,
                intent,
                bpc,
                preview: true,
            }) => {
                let c = Conversion {
                    from: &profile,
                    to,
                    intent: *intent,
                    bpc: *bpc,
                };
                Lut::converted(&profile, monitor, &c)
            }
            _ => Lut::proof(
                &profile,
                monitor,
                &self.proof(shown),
                shown.colors,
                shown.gamut,
            ),
        };
        let lut = lut.map(Arc::new);
        self.canvas = Some((key, lut.clone()));
        (key, lut)
    }
}

/// A profile's name as the UI shows it (Adobe RGB's, as "compatible
/// with": a trademark).
pub(super) fn shown_name(p: &Profile) -> String {
    if p == WorkingRgb::AdobeRgb.profile() {
        t!("color-adobe-rgb-compatible")
    } else {
        p.name().to_owned()
    }
}

/// What tells profiles apart, quickly: where their bytes are, how many,
/// and their header (dates, identifier).
fn fingerprint(icc: &Arc<Vec<u8>>) -> (usize, usize, &[u8]) {
    (
        Arc::as_ptr(icc) as usize,
        icc.len(),
        &icc[..icc.len().min(128)],
    )
}

impl ColorUi {
    /// The canvas's last table, and its fingerprint (see
    /// [`Self::canvas_lut`]).
    pub(super) fn shown(&self) -> (u64, Option<Arc<Lut>>) {
        self.canvas.clone().unwrap_or_default()
    }
}

/// An image (a layer lifted to be transformed) as the canvas shows it.
pub(super) fn shown_image<'a>(
    lut: Option<&Lut>,
    image: &'a image::RgbaImage,
) -> std::borrow::Cow<'a, image::RgbaImage> {
    let Some(lut) = lut else {
        return std::borrow::Cow::Borrowed(image);
    };
    let mut out = image.clone();
    let row = out.width() as usize * 4;
    crate::pixel_ops::par_bands(out.as_mut(), row, |band| {
        for p in band.chunks_exact_mut(4) {
            let [r, g, b] = lut.apply([p[0], p[1], p[2]]);
            p[..3].copy_from_slice(&[r, g, b]);
        }
    });
    std::borrow::Cow::Owned(out)
}

impl Intent {
    pub(super) fn label(self) -> String {
        match self {
            Intent::Perceptual => t!("color-intent-perceptual"),
            Intent::Saturation => t!("color-intent-saturation"),
            Intent::RelativeColorimetric => t!("color-intent-relative"),
            Intent::AbsoluteColorimetric => t!("color-intent-absolute"),
        }
    }
}

/// Rendering intent drop-down.
fn intent_combo(ui: &mut egui::Ui, id: &str, intent: &mut Intent) {
    let width = ui.available_width();
    let label = t!("color-intent");
    ui_kit::combo(ui, id, Some(&label), &intent.label(), width, |ui| {
        for i in Intent::ALL {
            ui.selectable_value(intent, i, i.label());
        }
    });
}

/// A profile file the user picks; `None` if cancelled, or not of `space`
/// (said in the status).
fn load_profile(space: Space, status: &mut String) -> Option<(PathBuf, Profile)> {
    let path = rfd::FileDialog::new()
        .set_title(t!("color-load-title"))
        .add_filter(t!("color-filter-profiles"), &["icc", "icm"])
        .pick_file()?;
    match Profile::load(&path).filter(|p| p.space() == space) {
        Some(p) => Some((path, p)),
        None => {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            *status = t!("color-status-not-a-profile", name = name.into_owned());
            None
        }
    }
}

/// RGB profiles offered: the working spaces, then the system's (read
/// once).
fn rgb_profiles() -> &'static [Profile] {
    static LIST: std::sync::OnceLock<Vec<Profile>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        let mut list: Vec<Profile> = WorkingRgb::ALL.map(|w| w.profile().clone()).into();
        for found in cms::installed(Space::Rgb) {
            if !list.iter().any(|p| p.name() == found.name)
                && let Some(p) = Profile::load(&found.path)
            {
                list.push(p);
            }
        }
        list
    })
}

/// A list of profiles to choose `chosen` from, with the current one in it
/// if it is none of them.
fn profile_combo(ui: &mut egui::Ui, id: &str, label: &str, chosen: &mut Profile, list: &[Profile]) {
    let width = ui.available_width();
    let shown = shown_name(chosen);
    ui_kit::combo(ui, id, Some(label), &shown, width, |ui| {
        if !list.contains(chosen) {
            let current = chosen.clone();
            ui.selectable_value(chosen, current.clone(), shown_name(&current));
        }
        for p in list {
            ui.selectable_value(chosen, p.clone(), shown_name(p));
        }
    });
}

impl AtelierApp {
    pub(super) fn color_command(&mut self, command: Command) {
        let working = self.color.working_rgb().clone();
        let Some(doc) = &mut self.doc else {
            if command == Command::Settings {
                self.color.dialog = Some(Dialog::Settings(self.color.settings.clone()));
            }
            return;
        };
        match command {
            Command::Settings => {
                self.color.dialog = Some(Dialog::Settings(self.color.settings.clone()));
            }
            Command::AssignProfile => {
                let choice = match &doc.icc_profile {
                    Some(icc) => Profile::from_icc(Arc::clone(icc))
                        .filter(|p| p.space() == Space::Rgb)
                        .map_or(Assigned::None, Assigned::Profile),
                    None => Assigned::None,
                };
                self.color.dialog = Some(Dialog::Assign {
                    choice,
                    preview: true,
                });
            }
            Command::ConvertToProfile => {
                let s = &self.color.settings;
                self.color.dialog = Some(Dialog::Convert {
                    to: working,
                    intent: s.intent,
                    bpc: s.bpc,
                    preview: true,
                });
            }
            Command::ProofWorkingCmyk => {
                doc.proof.custom = None;
                doc.proof.colors = true;
            }
            Command::ProofCustom => {
                self.color.dialog = Some(Dialog::Proof {
                    setup: doc.proof.clone(),
                    preview: true,
                });
            }
            Command::ProofColors => doc.proof.colors = !doc.proof.colors,
            Command::GamutWarning => doc.proof.gamut = !doc.proof.gamut,
            Command::Grayscale => self.color.dialog = Some(Dialog::Grayscale),
            Command::Rgb => {}
        }
    }

    /// Edit › Color Settings, Assign Profile, Convert to Profile.
    pub(super) fn color_edit_entries(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let has_doc = self.doc.is_some();
        vec![
            Separator,
            Item(
                t!("color-menu-color-settings"),
                true,
                Some(&SHORTCUTS[2].0),
                Action::Color(Command::Settings),
            ),
            Item(
                t!("color-menu-assign-profile"),
                has_doc,
                None,
                Action::Color(Command::AssignProfile),
            ),
            Item(
                t!("color-menu-convert-profile"),
                has_doc,
                None,
                Action::Color(Command::ConvertToProfile),
            ),
        ]
    }

    /// View › Proof Setup, Proof Colors, Gamut Warning (first in the View
    /// menu, like Photoshop's).
    pub(super) fn color_view_entries(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Check, Item, Separator, Submenu};
        let proof = self.doc.as_ref().map(|d| &d.proof);
        let has_doc = proof.is_some();
        let working = proof.is_some_and(|p| p.custom.is_none());
        let custom = proof.is_some_and(|p| p.custom.is_some());
        vec![
            Submenu(
                t!("color-menu-proof-setup"),
                vec![
                    Item(
                        t!("color-menu-proof-custom"),
                        has_doc,
                        None,
                        Action::Color(Command::ProofCustom),
                    ),
                    Separator,
                    Check(
                        t!("color-menu-proof-working-cmyk"),
                        has_doc,
                        working,
                        None,
                        Action::Color(Command::ProofWorkingCmyk),
                    ),
                ]
                .into_iter()
                .chain(custom.then(|| {
                    let name = proof
                        .and_then(|p| p.custom.as_ref())
                        .map_or_else(String::new, |p| p.name().to_owned());
                    Check(name, true, true, None, Action::Color(Command::ProofCustom))
                }))
                .collect(),
            ),
            Check(
                t!("color-menu-proof-colors"),
                has_doc,
                proof.is_some_and(|p| p.colors),
                Some(&SHORTCUTS[0].0),
                Action::Color(Command::ProofColors),
            ),
            Check(
                t!("color-menu-gamut-warning"),
                has_doc,
                proof.is_some_and(|p| p.gamut),
                Some(&SHORTCUTS[1].0),
                Action::Color(Command::GamutWarning),
            ),
            Separator,
        ]
    }

    /// Image › Mode: documents are RGB; Grayscale turns their colors into
    /// grays; CMYK is for the files written (Export As, Use Proof Setup).
    pub(super) fn mode_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Check, Unavailable};
        let has_doc = self.doc.is_some();
        vec![
            Check(
                t!("color-menu-mode-grayscale"),
                has_doc,
                false,
                None,
                Action::Color(Command::Grayscale),
            ),
            Check(
                t!("color-menu-mode-rgb"),
                has_doc,
                has_doc,
                None,
                Action::Color(Command::Rgb),
            ),
            Unavailable(t!("color-menu-mode-cmyk"), t!("color-mode-cmyk-tip")),
        ]
    }

    /// The color management dialogs, while one is open.
    pub(super) fn color_windows(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.color.dialog.take() else {
            return;
        };
        self.color.dialog = match dialog {
            Dialog::Settings(s) => self.settings_window(ctx, s),
            Dialog::Assign { choice, preview } => self.assign_window(ctx, choice, preview),
            Dialog::Convert {
                to,
                intent,
                bpc,
                preview,
            } => self.convert_window(ctx, to, intent, bpc, preview),
            Dialog::Proof { setup, preview } => self.proof_window(ctx, setup, preview),
            Dialog::Grayscale => self.grayscale_window(ctx),
        };
    }

    fn settings_window(&mut self, ctx: &egui::Context, mut s: ColorSettings) -> Option<Dialog> {
        let center = ctx.content_rect().center();
        let mut load = false;
        let cmyk = if s.cmyk == self.color.settings.cmyk {
            self.color.cmyk.clone()
        } else {
            s.cmyk_profile()
        };
        let monitor = shown_name(&self.color.monitor).to_owned();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 380.0,
                ..DialogSpec::new("color_settings", t!("color-settings-title"), center)
            },
            |ui| {
                let width = ui.available_width();
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("color-settings-working-spaces"));
                    let rgb = t!("color-settings-rgb");
                    let name = shown_name(s.rgb.profile());
                    ui_kit::combo(ui, "color_rgb", Some(&rgb), &name, width, |ui| {
                        for w in WorkingRgb::ALL {
                            ui.selectable_value(&mut s.rgb, w, shown_name(w.profile()));
                        }
                    });
                    let label = t!("color-settings-cmyk");
                    ui_kit::combo(ui, "color_cmyk", Some(&label), cmyk.name(), width, |ui| {
                        let default = cms::default_cmyk().name();
                        ui.selectable_value(&mut s.cmyk, None, default);
                        for p in cms::installed(Space::Cmyk) {
                            let name = p.name.clone();
                            ui.selectable_value(&mut s.cmyk, Some(p.path), name);
                        }
                        ui.separator();
                        load = ui.button(t!("color-settings-load-cmyk")).clicked();
                    });
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("color-conversion-options"));
                    intent_combo(ui, "color_intent", &mut s.intent);
                    ui_kit::checkbox(ui, &mut s.bpc, &t!("color-bpc"));
                });
                gap_between_blocks(ui);
                help(ui, &t!("color-settings-hint", monitor = monitor.clone()));
            },
        );
        if load && let Some((path, _)) = load_profile(Space::Cmyk, &mut self.status) {
            s.cmyk = Some(path);
        }
        if out.ok {
            self.color.cmyk = s.cmyk_profile();
            s.save();
            self.color.settings = s;
            return None;
        }
        (!out.cancel).then_some(Dialog::Settings(s))
    }

    fn assign_window(
        &mut self,
        ctx: &egui::Context,
        mut choice: Assigned,
        mut preview: bool,
    ) -> Option<Dialog> {
        let center = ctx.content_rect().center();
        let working = self.color.working_rgb();
        let list = rgb_profiles();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 380.0,
                preview: Some(&mut preview),
                ..DialogSpec::new("assign_profile", t!("color-assign-title"), center)
            },
            |ui| {
                let width = ui.available_width();
                ui_kit::field_label(ui, &t!("color-assign-label"));
                let none = t!("color-assign-none");
                let shown = match &choice {
                    Assigned::None => none.clone(),
                    Assigned::Profile(p) => shown_name(p),
                };
                ui_kit::combo(ui, "assign_profile", None, &shown, width, |ui| {
                    ui.selectable_value(&mut choice, Assigned::None, none.as_str());
                    let label = t!("color-assign-working", name = shown_name(working));
                    let w = Assigned::Profile(working.clone());
                    ui.selectable_value(&mut choice, w, label);
                    ui.separator();
                    if let Assigned::Profile(p) = &choice
                        && !list.contains(p)
                    {
                        let p = p.clone();
                        ui.selectable_value(
                            &mut choice,
                            Assigned::Profile(p.clone()),
                            shown_name(&p),
                        );
                    }
                    for p in list {
                        let a = Assigned::Profile(p.clone());
                        ui.selectable_value(&mut choice, a, shown_name(p));
                    }
                });
                gap_between_blocks(ui);
                help(ui, &t!("color-assign-hint"));
            },
        );
        if out.ok {
            let profile = match choice {
                Assigned::None => None,
                Assigned::Profile(p) => Some(p),
            };
            self.assign_profile(profile, false);
            return None;
        }
        (!out.cancel).then_some(Dialog::Assign { choice, preview })
    }

    fn convert_window(
        &mut self,
        ctx: &egui::Context,
        mut to: Profile,
        mut intent: Intent,
        mut bpc: bool,
        mut preview: bool,
    ) -> Option<Dialog> {
        let Some(doc) = &self.doc else { return None };
        let from = self.color.profile_of(doc);
        let center = ctx.content_rect().center();
        let list = rgb_profiles();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 380.0,
                preview: Some(&mut preview),
                ..DialogSpec::new("convert_profile", t!("color-convert-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("color-convert-source"));
                    ui_kit::key_value(ui, &t!("color-profile"), &shown_name(&from));
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("color-convert-destination"));
                    profile_combo(ui, "convert_to", &t!("color-profile"), &mut to, list);
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("color-conversion-options"));
                    intent_combo(ui, "convert_intent", &mut intent);
                    ui_kit::checkbox(ui, &mut bpc, &t!("color-bpc"));
                });
            },
        );
        if out.ok {
            self.convert_to_profile(to, intent, bpc, false);
            return None;
        }
        (!out.cancel).then_some(Dialog::Convert {
            to,
            intent,
            bpc,
            preview,
        })
    }

    fn proof_window(
        &mut self,
        ctx: &egui::Context,
        mut setup: ProofSetup,
        mut preview: bool,
    ) -> Option<Dialog> {
        let center = ctx.content_rect().center();
        let working = t!("color-proof-working", name = self.color.cmyk.name());
        let mut load = false;
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 380.0,
                preview: Some(&mut preview),
                ..DialogSpec::new("proof_setup", t!("color-proof-title"), center)
            },
            |ui| {
                let width = ui.available_width();
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("color-proof-device"));
                    let shown = setup
                        .custom
                        .as_ref()
                        .map_or_else(|| working.clone(), |p| p.name().to_owned());
                    ui_kit::combo(ui, "proof_device", None, &shown, width, |ui| {
                        ui.selectable_value(&mut setup.custom, None, working.as_str());
                        if let Some(p) = setup.custom.clone() {
                            ui.selectable_value(&mut setup.custom, Some(p.clone()), p.name());
                        }
                        ui.separator();
                        for found in cms::installed(Space::Cmyk) {
                            let selected = setup
                                .custom
                                .as_ref()
                                .is_some_and(|p| p.name() == found.name);
                            if ui.selectable_label(selected, &found.name).clicked()
                                && let Some(p) = Profile::load(&found.path)
                            {
                                setup.custom = Some(p);
                            }
                        }
                        load = ui.button(t!("color-proof-load")).clicked();
                    });
                    intent_combo(ui, "proof_intent", &mut setup.intent);
                    ui_kit::checkbox(ui, &mut setup.bpc, &t!("color-bpc"));
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::section(ui, &t!("color-proof-display"));
                    ui_kit::checkbox(ui, &mut setup.paper, &t!("color-proof-paper"));
                    // The paper's color comes with its black.
                    let mut ink = setup.ink || setup.paper;
                    ui.add_enabled_ui(!setup.paper, |ui| {
                        ui_kit::checkbox(ui, &mut ink, &t!("color-proof-ink"));
                    });
                    if !setup.paper {
                        setup.ink = ink;
                    }
                });
            },
        );
        if load && let Some((_, p)) = load_profile(Space::Cmyk, &mut self.status) {
            setup.custom = Some(p);
        }
        if out.ok {
            if let Some(doc) = &mut self.doc {
                doc.proof = ProofSetup {
                    colors: true,
                    ..setup
                };
            }
            return None;
        }
        (!out.cancel).then_some(Dialog::Proof { setup, preview })
    }

    fn grayscale_window(&mut self, ctx: &egui::Context) -> Option<Dialog> {
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok: t!("color-grayscale-discard-ok"),
                ..DialogSpec::new("grayscale", t!("color-menu-mode-grayscale"), center)
            },
            |ui| {
                ui.label(t!("color-grayscale-discard"));
            },
        );
        if out.ok {
            self.grayscale_mode(false);
            return None;
        }
        (!out.cancel).then_some(Dialog::Grayscale)
    }

    /// Assign Profile (`None`: Don't Color Manage): at once (its window's
    /// OK, an action's step), or its window filled with it.
    pub(super) fn assign_profile(&mut self, profile: Option<Profile>, dialog: bool) {
        if dialog {
            let choice = profile.map_or(Assigned::None, Assigned::Profile);
            self.color.dialog = Some(Dialog::Assign {
                choice,
                preview: true,
            });
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        doc.assign_profile(profile.as_ref());
        self.status = match &profile {
            Some(p) => t!("color-status-assigned", name = shown_name(p)),
            None => t!("color-status-unmanaged"),
        };
        let profile = profile.map(|p| p.icc().to_vec());
        self.record(crate::actions::Recorded::AssignProfile { profile });
    }

    /// Convert to Profile: at once (its window's OK, an action's step), or
    /// its window filled with these settings.
    pub(super) fn convert_to_profile(
        &mut self,
        to: Profile,
        intent: Intent,
        bpc: bool,
        dialog: bool,
    ) {
        if dialog {
            self.color.dialog = Some(Dialog::Convert {
                to,
                intent,
                bpc,
                preview: true,
            });
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let from = self.color.profile_of(doc);
        let c = Conversion {
            from: &from,
            to: &to,
            intent,
            bpc,
        };
        doc.convert_to_profile(&c);
        self.status = t!("color-status-converted", name = shown_name(&to));
        let profile = to.icc().to_vec();
        self.record(crate::actions::Recorded::ConvertToProfile {
            profile,
            intent,
            bpc,
        });
    }

    /// Image › Mode › Grayscale: at once (its window's OK, an action's
    /// step), or its window first.
    pub(super) fn grayscale_mode(&mut self, dialog: bool) {
        if dialog {
            self.color.dialog = Some(Dialog::Grayscale);
            return;
        }
        let Some(doc) = &mut self.doc else { return };
        let of = self.color.profile_of(doc);
        doc.grayscale(&Grayscale { of: &of });
        self.record(crate::actions::Recorded::Grayscale);
    }
}

/// A composited pixel (premultiplied) as the canvas shows it through `lut`.
#[inline]
pub(super) fn shown(lut: &Lut, p: crate::compose::blend::Px) -> Color32 {
    let a = p[3].clamp(0.0, 1.0);
    if a <= 0.0 {
        return Color32::TRANSPARENT;
    }
    let straight = |v: f32| ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8;
    let rgb = lut.apply([straight(p[0]), straight(p[1]), straight(p[2])]);
    let [r, g, b] = rgb.map(|v| (v as f32 * a).round() as u8);
    Color32::from_rgba_premultiplied(r, g, b, (a * 255.0).round() as u8)
}

impl AtelierApp {
    /// The profile of the colors picked: the document's.
    fn picked_profile(&self) -> Profile {
        match &self.doc {
            Some(doc) => self.color.profile_of(doc),
            None => self.color.working_rgb().clone(),
        }
    }

    /// `rgb` in the working CMYK (Color Picker).
    pub(super) fn ink_of(&self, rgb: [u8; 3]) -> Option<color_picker::Ink> {
        let from = self.picked_profile();
        let cmyk = self.color.to_cmyk(&from).color_to_cmyk(rgb)?;
        let (printable, closest) = cms::printable(rgb, &from, self.color.working_cmyk());
        Some(color_picker::Ink {
            rgb,
            cmyk,
            printable,
            closest,
        })
    }

    /// The color of CMYK inks (%) typed in the Color Picker.
    pub(super) fn rgb_of_ink(&self, cmyk: [f32; 4]) -> Option<[u8; 3]> {
        let to = self.picked_profile();
        let c = self.color.to_cmyk(&to);
        let back = Conversion {
            from: c.to,
            to: &to,
            ..c
        };
        back.cmyk_to_color(cmyk)
    }
}

impl AtelierApp {
    /// Use Proof Setup's conversion: the document's colors to its proof
    /// profile, with its intent and compensation.
    pub(super) fn proof_converter(&self) -> Option<cms::Converter> {
        let doc = self.doc.as_ref()?;
        let proof = self.color.proof(&doc.proof);
        Some(cms::Converter {
            from: self.color.profile_of(doc),
            to: proof.profile,
            intent: proof.intent,
            bpc: proof.bpc,
        })
    }
}

#[cfg(test)]
impl AtelierApp {
    /// A color management dialog, or the canvas proofed with its gamut
    /// warning, for the capture.
    pub(super) fn capture_color(&mut self, what: &str) {
        match what {
            "color-settings" => self.color_command(Command::Settings),
            "assign-profile" => self.color_command(Command::AssignProfile),
            "convert-profile" => self.color_command(Command::ConvertToProfile),
            "proof-setup" => self.color_command(Command::ProofCustom),
            _ => {
                self.color_command(Command::ProofColors);
                self.color_command(Command::GamutWarning);
            }
        }
    }
}
