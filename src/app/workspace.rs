//! Workspace: the side panel's width and the Properties / Layers split
//! (kept with the settings), panels hidden with Tab, Window › Layers and
//! Properties, File › Open Recent, the Help menu (Keyboard Shortcuts,
//! About) and the alert that reports a failed open or save.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::dialogs::{DialogSpec, floating_dialog};
use super::keys;
use super::*;
use crate::i18n::t;

/// Side panel width: by default, narrowest, widest.
pub(super) const SIDE_WIDTH: f32 = 312.0;
pub(super) const SIDE_MIN: f32 = 260.0;
pub(super) const SIDE_MAX: f32 = 480.0;

/// Tab hides the tool bar, the options bar and the side panel; Shift+Tab
/// only the side panel.
pub(super) const SHORTCUT_PANELS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::NONE, Key::Tab);
pub(super) const SHORTCUT_SIDE_PANEL: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::SHIFT, Key::Tab);

const WEBSITE: &str = "https://shu-app.com/atelier/";
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// What the user arranged, kept from one session to the next.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub(super) struct Layout {
    pub side_width: f32,
    /// Height of the Layers part of the side panel, as dragged; `None`:
    /// fitted to the list.
    pub layers_height: Option<f32>,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            side_width: SIDE_WIDTH,
            layers_height: None,
        }
    }
}

impl Layout {
    fn load() -> Self {
        if cfg!(test) {
            return Self::default();
        }
        std::fs::read(settings_file())
            .ok()
            .and_then(|json| serde_json::from_slice::<Self>(&json).ok())
            .map(|l| Self {
                side_width: l.side_width.clamp(SIDE_MIN, SIDE_MAX),
                ..l
            })
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
}

fn settings_file() -> PathBuf {
    crate::config::dir().join("workspace.json")
}

pub(super) struct Workspace {
    pub layout: Layout,
    /// Tab: the tool bar, options bar and side panel are hidden.
    pub hide_all: bool,
    /// Shift+Tab, Window › Layers or Properties: the side panel is hidden.
    pub hide_side: bool,
    /// Help › Keyboard Shortcuts open, and its filter.
    shortcuts: Option<String>,
    about: bool,
    /// A failed open or save, shown until dismissed.
    alert: Option<String>,
    /// The status message that reports an error (shown as one).
    pub error_status: Option<String>,
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            layout: Layout::load(),
            hide_all: false,
            hide_side: false,
            shortcuts: None,
            about: false,
            alert: None,
            error_status: None,
        }
    }
}

impl Workspace {
    /// A window that takes the keyboard is open (About, an alert).
    pub(super) fn modal(&self) -> bool {
        self.about || self.alert.is_some()
    }

    /// The layout changed by a drag: kept for the next sessions.
    pub(super) fn save_layout(&self) {
        self.layout.save();
    }
}

/// Workspace commands (File › Open Recent, Window, Help).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Command {
    /// One of the recent files, by its place in the list.
    OpenRecent(usize),
    ClearRecent,
    /// Tab: every panel.
    TogglePanels,
    /// Shift+Tab, Window › Layers / Properties: the side panel.
    ToggleSidePanel,
    ResetWorkspace,
    Shortcuts,
    Website,
    ReportProblem,
    Diagnostics,
    About,
}

impl AtelierApp {
    pub(super) fn workspace_command(&mut self, command: Command) {
        match command {
            Command::OpenRecent(i) => {
                let Some(path) = self.recent.paths().get(i).cloned() else {
                    return;
                };
                self.open_path(&path);
                if self.tab_of(&path).is_none() {
                    self.recent.remove(&path);
                }
            }
            Command::ClearRecent => self.recent.clear(),
            Command::TogglePanels => self.workspace.hide_all = !self.workspace.hide_all,
            Command::ToggleSidePanel => {
                let ws = &mut self.workspace;
                if ws.hide_all {
                    ws.hide_all = false;
                    ws.hide_side = false;
                } else {
                    ws.hide_side = !ws.hide_side;
                }
            }
            Command::ResetWorkspace => {
                let ws = &mut self.workspace;
                ws.hide_all = false;
                ws.hide_side = false;
                ws.layout = Layout::default();
                ws.save_layout();
                // The floating panels go back to their first place.
                super::dialogs::forget_panel_places(&self.ctx);
            }
            Command::Shortcuts => self.workspace.shortcuts = Some(String::new()),
            Command::Website => self.ctx.open_url(egui::OpenUrl::new_tab(WEBSITE)),
            Command::ReportProblem => self
                .ctx
                .open_url(egui::OpenUrl::new_tab(format!("{REPOSITORY}/issues"))),
            Command::Diagnostics => self.open_diagnostics(),
            Command::About => self.workspace.about = true,
        }
    }

    /// The tool bar and options bar are shown.
    pub(super) fn chrome_shown(&self) -> bool {
        !self.workspace.hide_all
    }

    /// The side panel (Properties and Layers) is shown.
    pub(super) fn side_shown(&self) -> bool {
        !self.workspace.hide_all && !self.workspace.hide_side
    }

    /// A painting or retouching tool over a locked layer (not in Quick
    /// Mask Mode, where it paints the mask): its strokes would be refused.
    pub(super) fn paints_on_locked_layer(&self) -> bool {
        let paints = matches!(
            self.tool,
            Tool::Brush
                | Tool::Eraser
                | Tool::CloneStamp
                | Tool::SpotHealing
                | Tool::HealingBrush
                | Tool::Patch
                | Tool::RedEye
                | Tool::Gradient
                | Tool::Bucket
                | Tool::Blur
                | Tool::Sharpen
                | Tool::Smudge
                | Tool::Dodge
                | Tool::Burn
                | Tool::Sponge
        );
        paints
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.quick_mask().is_none() && d.active_layer().locked)
    }

    /// Reports a failed operation: in the status bar, in the warning color,
    /// and in a window that stays until dismissed (`alert`), so that it
    /// can't go unnoticed. Batch runs keep only the status.
    pub(super) fn report_error(&mut self, message: String) {
        crate::diag::error(&message);
        self.status = message.clone();
        self.workspace.error_status = Some(message.clone());
        if !self.automate.running() {
            self.workspace.alert = Some(message);
        }
    }

    /// Reports a failed operation in the status bar only (and the
    /// diagnostics' log), for what can be tried again at once.
    pub(super) fn status_failed(&mut self, message: String) {
        crate::diag::error(&message);
        self.status = message;
    }

    /// File › Open Recent: the recent files (by name, with their folder
    /// when two share a name), then Clear Recent File List.
    pub(super) fn recent_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let paths = self.recent.paths();
        let name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        };
        let mut entries: Vec<MenuEntry> = paths
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let shown = name(path);
                let twins = paths.iter().filter(|p| name(p) == shown).count() > 1;
                let label = match path.parent().and_then(|d| d.file_name()) {
                    Some(dir) if twins => format!("{shown} — {}", dir.to_string_lossy()),
                    _ => shown,
                };
                Item(label, true, None, Action::Workspace(Command::OpenRecent(i)))
            })
            .collect();
        if !entries.is_empty() {
            entries.push(Separator);
        }
        entries.push(Item(
            t!("ui3-menu-clear-recent"),
            !paths.is_empty(),
            None,
            Action::Workspace(Command::ClearRecent),
        ));
        entries
    }

    /// Window menu: every panel, in the alphabetical order of the
    /// language, then hiding and resetting them.
    pub(super) fn window_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Check, Item, Separator};
        let side = self.side_shown();
        let mut panels = vec![
            Check(
                t!("actions-title"),
                true,
                self.actions.open,
                Some(&actions_panel::SHORTCUT_ACTIONS),
                Action::Automate(actions_panel::Command::Panel),
            ),
            Check(
                t!("xchg-menu-channels"),
                true,
                self.channels.open,
                None,
                Action::Channels,
            ),
            Check(
                t!("xchg-menu-history"),
                true,
                self.history.open,
                None,
                Action::History,
            ),
            Check(
                t!("panel-layers"),
                true,
                side,
                None,
                Action::Workspace(Command::ToggleSidePanel),
            ),
            Check(
                t!("vector-menu-paths"),
                true,
                self.vector.panel.open,
                None,
                Action::Paths,
            ),
            Check(
                t!("panel-properties"),
                true,
                side,
                None,
                Action::Workspace(Command::ToggleSidePanel),
            ),
            Check(
                t!("uip-menu-swatches"),
                true,
                self.colors.panel_open,
                None,
                Action::Swatches,
            ),
        ];
        let key = |e: &MenuEntry| match e {
            Check(label, ..) => super::palette::fold(label),
            _ => String::new(),
        };
        panels.sort_by_cached_key(key);
        panels.extend([
            Separator,
            Check(
                t!("ui3-menu-hide-panels"),
                true,
                self.workspace.hide_all,
                Some(&SHORTCUT_PANELS),
                Action::Workspace(Command::TogglePanels),
            ),
            Item(
                t!("ui3-menu-reset-workspace"),
                true,
                None,
                Action::Workspace(Command::ResetWorkspace),
            ),
        ]);
        panels
    }

    /// Help menu.
    pub(super) fn help_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let item = |label, command| Item(label, true, None, Action::Workspace(command));
        vec![
            item(t!("ui3-menu-shortcuts"), Command::Shortcuts),
            Separator,
            item(t!("ui3-menu-website"), Command::Website),
            item(t!("ui3-menu-report-problem"), Command::ReportProblem),
            item(t!("diag-menu"), Command::Diagnostics),
            Separator,
            item(t!("ui3-menu-about"), Command::About),
        ]
    }

    /// Keyboard Shortcuts, Diagnostics, About and the alert.
    pub(super) fn workspace_windows(&mut self, ctx: &egui::Context) {
        self.shortcuts_window(ctx);
        self.diagnostics_window(ctx);
        self.about_window(ctx);
        self.error_alert_window(ctx);
    }

    fn error_alert_window(&mut self, ctx: &egui::Context) {
        let Some(message) = self.workspace.alert.clone() else {
            return;
        };
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 420.0,
                cancel: false,
                ..DialogSpec::new("alert", t!("ui3-alert-title"), ctx.content_rect().center())
            },
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.label(egui::RichText::new(icon::WARNING).size(20.0).color(WARNING));
                    ui.add(egui::Label::new(message.as_str()).wrap());
                });
            },
        );
        if out.ok || out.cancel {
            self.workspace.alert = None;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        if !self.workspace.about {
            return;
        }
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                width: 380.0,
                cancel: false,
                ..DialogSpec::new("about", t!("ui3-menu-about"), ctx.content_rect().center())
            },
            |ui| {
                ui.label(egui::RichText::new("Atelier").font(ui_kit::bold(20.0)));
                ui.label(
                    egui::RichText::new(t!(
                        "ui3-about-version",
                        version = env!("CARGO_PKG_VERSION")
                    ))
                    .font(ui_kit::mono(12.0))
                    .color(ui_kit::color::MUTED),
                );
                ui.label(t!("ui3-about-description"));
                ui.label(
                    egui::RichText::new(t!(
                        "ui3-about-license",
                        license = env!("CARGO_PKG_LICENSE")
                    ))
                    .color(ui_kit::color::LABEL),
                );
                ui.hyperlink_to(t!("ui3-menu-website"), WEBSITE);
                ui.hyperlink_to(t!("ui3-about-source"), REPOSITORY);
            },
        );
        if out.ok || out.cancel {
            self.workspace.about = false;
        }
    }

    /// Help › Keyboard Shortcuts: every menu command with a shortcut, the
    /// tools' keys and the other keys, filtered as you type.
    fn shortcuts_window(&mut self, ctx: &egui::Context) {
        let Some(mut filter) = self.workspace.shortcuts.take() else {
            return;
        };
        let rows = self.shortcut_rows(ctx);
        let folded = super::palette::fold(filter.trim());
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 460.0,
                resize: Some(420.0),
                ..DialogSpec::new(
                    "shortcuts",
                    t!("ui3-shortcuts-title"),
                    super::dialogs::panel_pos(ctx, "shortcuts"),
                )
            },
            |ui| {
                ui_kit::text_field(
                    ui,
                    &t!("ui3-shortcuts-filter"),
                    &mut filter,
                    egui::Id::new("shortcuts-filter"),
                );
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let mut group = None;
                        for (section, label, key) in &rows {
                            let haystack =
                                super::palette::fold(&format!("{section} {label} {key}"));
                            if !folded.is_empty() && !haystack.contains(&folded) {
                                continue;
                            }
                            if group != Some(section) {
                                group = Some(section);
                                ui.add_space(6.0);
                                ui_kit::section(ui, section);
                            }
                            ui.horizontal(|ui| {
                                ui.label(label);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| ui_kit::kbd(ui, key),
                                );
                            });
                        }
                    });
            },
        );
        if !out.cancel {
            self.workspace.shortcuts = Some(filter);
        }
    }

    /// (section, what, keys) of every shortcut.
    fn shortcut_rows(&self, ctx: &egui::Context) -> Vec<(String, String, String)> {
        fn walk(
            ctx: &egui::Context,
            entries: &[MenuEntry],
            path: &str,
            section: &str,
            out: &mut Vec<(String, String, String)>,
        ) {
            for entry in entries {
                match entry {
                    MenuEntry::Submenu(label, sub) => {
                        walk(ctx, sub, &format!("{path} › {label}"), section, out);
                    }
                    MenuEntry::Item(label, _, Some(s), _)
                    | MenuEntry::Check(label, _, _, Some(s), _) => {
                        let what = match path.split_once(" › ") {
                            Some((_, rest)) => format!("{rest} › {label}"),
                            None => label.clone(),
                        };
                        out.push((section.to_owned(), what, keys::shortcut(ctx, s)));
                    }
                    _ => {}
                }
            }
        }
        let mut rows = Vec::new();
        let tools = t!("palette-tools");
        for tool in Tool::ALL {
            if let Some(k) = tool.shortcut() {
                rows.push((tools.clone(), tool.name(), k.name().to_owned()));
            }
        }
        for (title, entries) in self.menus() {
            walk(ctx, &entries, &title, &title, &mut rows);
        }
        let other = t!("ui3-shortcuts-other");
        let more = [
            (
                t!("ui3-shortcuts-palette"),
                keys::shortcut(ctx, &super::palette::SHORTCUT_PALETTE),
            ),
            (
                t!("ui3-menu-hide-panels"),
                keys::shortcut(ctx, &SHORTCUT_PANELS),
            ),
            (
                t!("ui3-shortcuts-hide-side"),
                keys::shortcut(ctx, &SHORTCUT_SIDE_PANEL),
            ),
            (t!("panel-swap-colors"), "X".to_owned()),
            (t!("panel-default-colors"), "D".to_owned()),
            (t!("ui3-shortcuts-brush-size"), "[ ]".to_owned()),
            (t!("ui3-shortcuts-pan"), keys::key(Key::Space)),
        ];
        rows.extend(
            more.into_iter()
                .map(|(what, key)| (other.clone(), what, key)),
        );
        rows
    }

    /// Drag handle on the side panel's left edge: its width, kept for the
    /// next sessions.
    pub(super) fn side_panel_grip(&mut self, ui: &mut egui::Ui, panel: Rect) {
        // Mostly inside the panel: the canvas takes the pointer outside.
        let grip = Rect::from_x_y_ranges(panel.left() - 2.0..=panel.left() + 4.0, panel.y_range());
        let response = ui
            .interact(grip, egui::Id::new("side-panel-grip"), Sense::drag())
            .on_hover_cursor(CursorIcon::ResizeHorizontal);
        if response.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            let layout = &mut self.workspace.layout;
            layout.side_width =
                (layout.side_width - response.drag_delta().x).clamp(SIDE_MIN, SIDE_MAX);
        }
        if response.drag_stopped() {
            self.workspace.save_layout();
        }
    }
}

/// Warning color: errors in the status bar and the alert.
pub(super) const WARNING: Color32 = Color32::from_rgb(0xf0, 0xb4, 0x5a);

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    /// A window of `size` with a 512 × 512 image, or none.
    fn app(size: [f32; 2], with_doc: bool) -> Harness<'static, AtelierApp> {
        i18n::set_lang(Lang::En);
        let mut h = Harness::builder().with_size(size).build_eframe(move |cc| {
            let mut app = AtelierApp::new(cc, None);
            if with_doc {
                let img = image::RgbaImage::from_pixel(512, 512, image::Rgba([90, 120, 200, 255]));
                app.doc = Some(Document::new(img, None));
            }
            app
        });
        h.run_steps(3);
        h
    }

    fn click(h: &mut Harness<'static, AtelierApp>, at: Pos2) {
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(2);
    }

    /// A small laptop (1366 × 768 less the taskbar): every tool is on
    /// screen, in two columns.
    #[test]
    fn every_tool_is_visible_on_a_low_window() {
        let mut h = app([1366.0, 728.0], true);
        h.run_steps(2);
        let window = h.ctx.content_rect();
        for (index, _) in Tool::GROUPS.iter().enumerate() {
            let tool = h.state().group_shown(index);
            // The toolbar's slot (the options bar names the active tool too).
            let slot = h
                .get_all_by_label(&tool.name())
                .map(|n| n.rect())
                .find(|r| r.left() < 100.0)
                .expect("in the toolbar");
            assert!(window.contains_rect(slot), "{} at {slot:?}", tool.name());
        }
        // A tall window keeps one column.
        assert_eq!(toolbar::columns(1100.0), 1);
        assert_eq!(toolbar::columns(600.0), 2);
    }

    /// The Layer menu, taller than a small laptop's window, scrolls instead
    /// of covering the menu bar.
    #[test]
    fn a_tall_menu_stays_below_the_menu_bar() {
        let mut h = app([1280.0, 620.0], true);
        let menu_bar = |h: &Harness<'static, AtelierApp>, label: &str| {
            h.get_all_by_label(label)
                .map(|n| n.rect())
                .find(|r| r.top() < 40.0)
                .expect("in the menu bar")
        };
        let layer = menu_bar(&h, &t!("menu-layer")).center();
        click(&mut h, layer);
        h.run_steps(2);
        let first = h
            .get_all_by_label_contains(&t!("menu-layer-new"))
            .map(|n| n.rect())
            .min_by(|a, b| a.top().total_cmp(&b.top()))
            .expect("Layer › New Layer");
        assert!(first.top() >= 40.0, "{first:?}");
        let window = h.ctx.content_rect();
        let last = t!("menu-layer-split");
        // The last item is reachable: listed, inside the window once scrolled.
        let item = h.get_by_label_contains(&last);
        item.scroll_to_me();
        h.run_steps(3);
        let rect = h.get_by_label_contains(&last).rect();
        assert!(rect.bottom() <= window.bottom(), "{rect:?}");
        // And it runs.
        click(&mut h, rect.center());
        assert!(h.state().split_open);
    }

    #[test]
    fn tab_hides_the_panels_and_shift_tab_the_side_panel() {
        let mut h = app([1280.0, 800.0], true);
        let layers = t!("panel-layers");
        assert!(h.query_by_label(&layers).is_some());
        h.key_press(Key::Tab);
        h.run_steps(2);
        assert!(h.state().workspace.hide_all);
        assert!(h.query_by_label(&layers).is_none());
        assert!(h.query_by_label(&Tool::Brush.name()).is_none());
        h.key_press(Key::Tab);
        h.run_steps(2);
        assert!(h.query_by_label(&Tool::Brush.name()).is_some());
        h.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
        h.run_steps(2);
        assert!(h.query_by_label(&layers).is_none());
        assert!(h.query_by_label(&Tool::Brush.name()).is_some());
        assert!(h.ctx.memory(|m| m.focused()).is_none());
        // Window › Layers brings it back.
        h.state_mut()
            .run(Action::Workspace(Command::ToggleSidePanel));
        h.run_steps(2);
        assert!(h.query_by_label(&layers).is_some());
    }

    #[test]
    fn the_window_menu_lists_every_panel_in_the_languages_order() {
        for lang in [Lang::En, Lang::Fr] {
            let h = app([1280.0, 800.0], true);
            i18n::set_lang(lang);
            let labels: Vec<String> = h
                .state()
                .window_menu()
                .iter()
                .take_while(|e| !matches!(e, MenuEntry::Separator))
                .filter_map(|e| match e {
                    MenuEntry::Check(label, ..) => Some(label.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(labels.len(), 7);
            assert!(labels.contains(&t!("panel-layers")));
            assert!(labels.contains(&t!("panel-properties")));
            let mut sorted = labels.clone();
            sorted.sort_by_key(|l| super::super::palette::fold(l));
            assert_eq!(labels, sorted, "{}", lang.code());
        }
        i18n::set_lang(Lang::En);
    }

    #[test]
    fn open_recent_opens_and_clears() {
        let dir = std::env::temp_dir().join(format!("atelier-ui3-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recent.png");
        image::RgbaImage::from_pixel(8, 6, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let mut h = app([1280.0, 800.0], false);
        h.state_mut().recent = crate::recent::Recent::with_paths(vec![path.clone()]);
        let menu = h.state().recent_menu();
        assert!(matches!(&menu[0], MenuEntry::Item(label, true, _, _) if label == "recent.png"));
        h.state_mut().run(Action::Workspace(Command::OpenRecent(0)));
        h.run_steps(2);
        assert_eq!(h.state().doc.as_ref().map(Document::width), Some(8));
        h.state_mut().run(Action::Workspace(Command::ClearRecent));
        assert!(h.state().recent.paths().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_open_is_reported_in_an_alert() {
        let mut h = app([1280.0, 800.0], false);
        let path = std::env::temp_dir().join("atelier-ui3-missing.png");
        h.state_mut().open_path(&path);
        h.run_steps(3);
        assert!(h.state().workspace.alert.is_some());
        assert!(h.state().modal_open());
        // Enter is OK.
        h.key_press(Key::Enter);
        h.run_steps(2);
        assert!(h.state().workspace.alert.is_none());
        // The status keeps it, in the warning color, with the name only.
        assert!(h.state().status.contains("atelier-ui3-missing.png"));
        assert!(!h.state().status.contains(&path.display().to_string()) || cfg!(windows));
    }

    #[test]
    fn help_shows_the_shortcuts_and_the_version() {
        let mut h = app([1280.0, 800.0], true);
        let rows = h.state().shortcut_rows(&h.ctx.clone());
        let save = keys::shortcut(&h.ctx, &SHORTCUT_SAVE);
        assert!(
            rows.iter()
                .any(|(_, what, key)| *what == t!("menu-file-save") && *key == save)
        );
        assert!(
            rows.iter()
                .any(|(_, what, key)| *what == Tool::Brush.name() && key == "B")
        );
        h.state_mut().run(Action::Workspace(Command::About));
        h.run_steps(3);
        let version = t!("ui3-about-version", version = env!("CARGO_PKG_VERSION"));
        assert!(h.query_by_label(&version).is_some());
    }

    /// History and Channels, opened together, don't cover each other, and
    /// stay above the status bar.
    #[test]
    fn floating_panels_open_side_by_side() {
        let mut h = app([1366.0, 728.0], true);
        h.state_mut().run(Action::History);
        h.run_steps(3);
        h.state_mut().run(Action::Channels);
        h.run_steps(3);
        h.state_mut()
            .run(Action::Automate(actions_panel::Command::Panel));
        h.run_steps(3);
        let rect = |id: &str| {
            h.ctx
                .memory(|m| m.area_rect(egui::Id::new(id)))
                .unwrap_or_else(|| panic!("{id} shown"))
        };
        let (history, channels, actions) = (rect("history"), rect("channels"), rect("actions"));
        assert!(!history.intersects(channels), "{history:?} {channels:?}");
        assert!(!actions.intersects(channels) && !actions.intersects(history));
        let status_top = h.ctx.content_rect().bottom() - 28.0;
        for r in [history, channels, actions] {
            assert!(r.bottom() <= status_top, "{r:?}");
            assert!(r.top() >= 40.0 + 46.0, "{r:?}");
        }
    }

    /// Dragging the border between Properties and Layers gives the Layers
    /// more room; a double click fits them again.
    #[test]
    fn the_layers_split_is_dragged() {
        let mut h = app([1280.0, 800.0], true);
        let heading = h.get_by_label(&t!("panel-layers")).rect();
        // The border is the top of the Layers heading.
        let at = pos2(heading.center().x, heading.top() + 1.0);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.hover_at(at - vec2(0.0, 120.0));
        h.run_steps(1);
        h.drop_at(at - vec2(0.0, 120.0));
        h.run_steps(2);
        let dragged = h.state().workspace.layout.layers_height;
        assert!(dragged.is_some(), "the split moved");
        let moved = h.get_by_label(&t!("panel-layers")).rect();
        assert!(moved.top() < heading.top() - 100.0, "{moved:?}");
    }

    /// The side panel is wider when its edge is dragged, within limits.
    #[test]
    fn the_side_panel_is_resized() {
        let mut h = app([1280.0, 800.0], true);
        let edge = h.ctx.content_rect().right() - SIDE_WIDTH + 2.0;
        let at = pos2(edge, 400.0);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.hover_at(at - vec2(400.0, 0.0));
        h.run_steps(1);
        h.drop_at(at - vec2(400.0, 0.0));
        h.run_steps(2);
        assert_eq!(h.state().workspace.layout.side_width, SIDE_MAX);
        h.state_mut()
            .run(Action::Workspace(Command::ResetWorkspace));
        assert_eq!(h.state().workspace.layout.side_width, SIDE_WIDTH);
    }

    #[test]
    fn the_start_screen_offers_new() {
        let mut h = app([1280.0, 800.0], false);
        let new = h
            .get_by_label(&t!("artboard-menu-new-document"))
            .rect()
            .center();
        click(&mut h, new);
        assert!(h.state().new_doc.is_open());
    }
}
