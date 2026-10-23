//! Help › Diagnostics: how the program behaves on this computer (frame
//! times, freezes, memory, the slowest operations, warnings and errors,
//! the open documents, the system), and a report of it all to paste into a
//! message or an issue. What is measured, and how, is in `crate::diag`.

use std::time::{Duration, Instant};

use super::dialogs::{DialogSpec, floating_dialog};
use super::models_window::size_text;
use super::*;
use crate::ai::models::Model;
use crate::ai::runtime::Device;
use crate::diag::{self, FrameStats, Freeze, Level, LogEntry, OpStats, TileStats};
use crate::document::{HISTORY_BUDGET, TOTAL_HISTORY_BUDGET};
use ui_kit::color;

/// Width of the window, default height of its body.
const WIDTH: f32 = 780.0;
const BODY_HEIGHT: f32 = 500.0;
/// Narrowest card of the overview.
const CARD_MIN: f32 = 220.0;
const CARD_HEIGHT: f32 = 92.0;
/// The window refreshes what it shows this often.
const REFRESH: Duration = Duration::from_secs(1);
const MINUTE: Duration = Duration::from_secs(60);
const FIVE_MINUTES: Duration = Duration::from_secs(300);
/// Operations listed, at most (in the window and in the report).
const MAX_ROWS: usize = 100;
const REPORT_ROWS: usize = 30;
const REPORT_LOG: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(super) enum Tab {
    #[default]
    Overview,
    Operations,
    Log,
    Documents,
    System,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Sort {
    #[default]
    Slowest,
    Recent,
}

/// The window's state, and the graphics card (read on the first frame).
#[derive(Default)]
pub(super) struct Diagnostics {
    pub open: bool,
    pub tab: Tab,
    sort: Sort,
    snapshot: Option<Snapshot>,
    gpu: Option<Gpu>,
}

/// The graphics card and the API the canvas is drawn with.
#[derive(Clone)]
struct Gpu {
    name: String,
    backend: String,
    kind: String,
    driver: String,
    max_texture: u32,
}

/// What the window shows, gathered at once (every second while open).
pub(super) struct Snapshot {
    taken: Instant,
    since: Instant,
    frames: FrameStats,
    freezes: Vec<Freeze>,
    ops: Vec<(String, OpStats)>,
    tiles: TileStats,
    log: Vec<LogEntry>,
    warnings: u32,
    errors: u32,
    jobs: Vec<String>,
    memory: Option<diag::Memory>,
    threads: Option<u32>,
    docs: Vec<DocInfo>,
    ai_devices: Vec<Device>,
    ai_installed: Vec<Model>,
    color: ColorInfo,
    system: SystemInfo,
}

struct DocInfo {
    /// Tab title (shown in the window, never copied).
    name: String,
    /// Extension of its file; `None`: not saved yet.
    format: Option<String>,
    embedded: bool,
    active: bool,
    modified: bool,
    size: (u32, u32),
    /// Pixel layers, texts, shapes, smart objects, adjustments, fills, groups.
    kinds: [usize; 7],
    layer_bytes: usize,
    steps: usize,
    done: usize,
    history_bytes: usize,
    profile: String,
}

struct ColorInfo {
    document: Option<String>,
    monitor: String,
    monitor_known: bool,
    working_rgb: String,
    working_cmyk: String,
    intent: crate::cms::Intent,
    bpc: bool,
    proof: bool,
    gamut: bool,
}

struct SystemInfo {
    cores: usize,
    scale: f32,
    native_scale: Option<f32>,
    window: Vec2,
    monitor: Option<Vec2>,
    lang: Lang,
    flatpak: bool,
}

impl DocInfo {
    fn layers(&self) -> usize {
        self.kinds.iter().sum()
    }

    fn kinds_text(&self) -> String {
        let labels = [
            t!("diag-kind-pixels", count = self.kinds[0]),
            t!("diag-kind-text", count = self.kinds[1]),
            t!("diag-kind-shape", count = self.kinds[2]),
            t!("diag-kind-smart", count = self.kinds[3]),
            t!("diag-kind-adjustment", count = self.kinds[4]),
            t!("diag-kind-fill", count = self.kinds[5]),
            t!("diag-kind-group", count = self.kinds[6]),
        ];
        let parts: Vec<String> = labels
            .into_iter()
            .zip(self.kinds)
            .filter(|(_, n)| *n > 0)
            .map(|(label, _)| label)
            .collect();
        parts.join(" · ")
    }

    /// For the report: "pixels 3, text 1".
    fn kinds_en(&self) -> String {
        let names = [
            "pixels",
            "text",
            "shape",
            "smart object",
            "adjustment",
            "fill",
            "group",
        ];
        let parts: Vec<String> = names
            .iter()
            .zip(self.kinds)
            .filter(|(_, n)| *n > 0)
            .map(|(name, n)| format!("{name} {n}"))
            .collect();
        parts.join(", ")
    }
}

impl AtelierApp {
    /// Help › Diagnostics.
    pub(super) fn open_diagnostics(&mut self) {
        self.diagnostics.open = true;
        self.diagnostics.snapshot = None;
    }

    /// The graphics card, read once from the renderer.
    pub(super) fn read_gpu(&mut self, frame: &eframe::Frame) {
        if self.diagnostics.gpu.is_some() {
            return;
        }
        let Some(state) = frame.wgpu_render_state() else {
            return;
        };
        let info = state.adapter.get_info();
        let driver = [info.driver.as_str(), info.driver_info.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        self.diagnostics.gpu = Some(Gpu {
            name: info.name,
            backend: format!("{:?}", info.backend),
            kind: format!("{:?}", info.device_type),
            driver,
            max_texture: state.device.limits().max_texture_dimension_2d,
        });
    }

    /// The end of a frame that started at `start`: measured, and if it
    /// froze the window, what was going on.
    pub(super) fn end_frame(&self, start: Instant) {
        let took = start.elapsed();
        if diag::frame(start, took) {
            diag::freeze(took, self.freeze_context(start));
        }
    }

    /// What was going on during a frame that started at `start`: the tool,
    /// the dialog, the operations it ran, the background jobs.
    pub(super) fn freeze_context(&self, start: Instant) -> String {
        let mut parts = vec![format!("tool {:?}", self.tool)];
        if let Some(dialog) = self.dialog_name() {
            parts.push(format!("dialog {dialog}"));
        }
        if self.stroke.is_some() {
            parts.push("stroke in progress".to_owned());
        } else if self.ctx.input(|i| i.pointer.any_down()) {
            parts.push("pointer down".to_owned());
        }
        let ops = diag::ops_since(start);
        if !ops.is_empty() {
            let ran: Vec<String> = ops
                .iter()
                .take(3)
                .map(|(name, took)| format!("{name} {}", en_duration(*took)))
                .collect();
            parts.push(format!("ran {}", ran.join(", ")));
        }
        let jobs = diag::running_jobs();
        if !jobs.is_empty() {
            parts.push(format!("jobs {}", jobs.join(", ")));
        }
        parts.join(" · ")
    }

    /// The dialog or workspace open, if any (English, for developers).
    fn dialog_name(&self) -> Option<&'static str> {
        let open = [
            (self.adjust.is_some(), "Adjustment"),
            (self.layer_style.is_some(), "Layer Style"),
            (self.filters.busy(), "Filter"),
            (self.camera_raw.busy(), "Camera Raw"),
            (self.liquify.busy(), "Liquify"),
            (self.smart.busy(), "Smart Filter"),
            (self.fill_dialog.is_some(), "Fill"),
            (self.retouch.dialog.is_some(), "Content-Aware Fill"),
            (self.image_size.is_some(), "Image Size"),
            (self.canvas_size.is_some(), "Canvas Size"),
            (self.rotate_dialog.is_some(), "Rotate"),
            (self.export.is_some(), "Export As"),
            (self.new_doc.is_open(), "New Document"),
            (self.palette.is_open(), "Command Palette"),
            (self.color.modal_open(), "Color"),
            (self.select_modal(), "Select"),
            (self.automate.modal(), "Automate"),
            (self.models_window.is_some(), "AI Models"),
            (self.diagnostics.open, "Diagnostics"),
        ];
        open.into_iter().find(|(o, _)| *o).map(|(_, name)| name)
    }

    /// Gathers what the window and the report show.
    pub(super) fn diagnostics_snapshot(&self) -> Snapshot {
        let now = Instant::now();
        let (frames, freezes, ops, tiles, since) = diag::measures(|m| {
            let ops = m
                .operations()
                .into_iter()
                .map(|(n, s)| (n.to_owned(), s.clone()))
                .collect();
            let freezes = m.freezes(now, FIVE_MINUTES).into_iter().cloned().collect();
            (
                m.frame_stats(now, MINUTE),
                freezes,
                ops,
                m.tile_stats(now),
                m.since(),
            )
        });
        let (log, warnings, errors) = diag::log_entries();
        let docs = (0..self.tab_count())
            .filter_map(|i| self.tab(i).map(|(doc, _)| (i, doc)))
            .map(|(i, doc)| self.doc_info(doc, i == self.active_tab))
            .collect();
        let (monitor, monitor_known) = self.color.monitor();
        let settings = &self.color.settings;
        let color = ColorInfo {
            document: self
                .doc
                .as_ref()
                .map(|d| color_ui::shown_name(&self.color.profile_of(d))),
            monitor: color_ui::shown_name(monitor),
            monitor_known,
            working_rgb: color_ui::shown_name(self.color.working_rgb()),
            working_cmyk: color_ui::shown_name(self.color.working_cmyk()),
            intent: settings.intent,
            bpc: settings.bpc,
            proof: self.doc.as_ref().is_some_and(|d| d.proof.colors),
            gamut: self.doc.as_ref().is_some_and(|d| d.proof.gamut),
        };
        let mut ai_devices = Vec::new();
        for device in Model::ALL.into_iter().flat_map(Model::devices) {
            if !ai_devices.contains(&device) {
                ai_devices.push(device);
            }
        }
        let system = SystemInfo {
            cores: std::thread::available_parallelism().map_or(1, |n| n.get()),
            scale: self.ctx.pixels_per_point(),
            native_scale: self.ctx.native_pixels_per_point(),
            window: self.ctx.content_rect().size(),
            monitor: self.ctx.input(|i| i.viewport().monitor_size),
            lang: i18n::lang(),
            flatpak: std::env::var_os("FLATPAK_ID").is_some(),
        };
        Snapshot {
            taken: now,
            since,
            frames,
            freezes,
            ops,
            tiles,
            log,
            warnings,
            errors,
            jobs: diag::running_jobs(),
            memory: diag::process_memory(),
            threads: diag::thread_count(),
            docs,
            ai_devices,
            ai_installed: Model::ALL.into_iter().filter(|m| m.ready()).collect(),
            color,
            system,
        }
    }

    fn doc_info(&self, doc: &Document, active: bool) -> DocInfo {
        let mut kinds = [0; 7];
        let mut layer_bytes = 0;
        for layer in &doc.layers {
            let kind = match &layer.kind {
                LayerKind::Group { .. } => 6,
                LayerKind::Fill(_) => 5,
                LayerKind::Adjustment(_) => 4,
                LayerKind::Pixels if layer.smart.is_some() => 3,
                LayerKind::Pixels if layer.shape.is_some() => 2,
                LayerKind::Pixels if layer.text.is_some() => 1,
                LayerKind::Pixels => 0,
            };
            kinds[kind] += 1;
            layer_bytes += layer.image().as_raw().len();
        }
        let (names, done) = doc.history();
        DocInfo {
            name: doc.display_name(),
            format: doc
                .path
                .as_ref()
                .and_then(|p| p.extension())
                .map(|e| e.to_string_lossy().to_lowercase()),
            embedded: doc.embedded_in.is_some(),
            active,
            modified: doc.modified,
            size: (doc.width(), doc.height()),
            kinds,
            layer_bytes,
            steps: names.len(),
            done,
            history_bytes: doc.history_bytes(),
            profile: color_ui::shown_name(&self.color.profile_of(doc)),
        }
    }

    /// Help › Diagnostics: the window, while open.
    pub(super) fn diagnostics_window(&mut self, ctx: &egui::Context) {
        if !self.diagnostics.open {
            return;
        }
        let stale = self
            .diagnostics
            .snapshot
            .as_ref()
            .is_none_or(|s| s.taken.elapsed() >= REFRESH);
        if stale {
            self.diagnostics.snapshot = Some(self.diagnostics_snapshot());
        }
        // Up to date while open, the measures moving on their own.
        ctx.request_repaint_after(REFRESH);
        let Some(snap) = self.diagnostics.snapshot.take() else {
            return;
        };
        let mut copy = false;
        let mut open_logs = false;
        let mut clear = false;
        let subtitle = self.subtitle();
        let (mut tab, mut sort) = (self.diagnostics.tab, self.diagnostics.sort);
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: WIDTH,
                resize: Some(BODY_HEIGHT),
                pivot: egui::Align2::CENTER_CENTER,
                ..DialogSpec::new("diagnostics", t!("diag-title"), ctx.content_rect().center())
            },
            |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&subtitle)
                                .font(ui_kit::mono(12.0))
                                .color(color::MUTED),
                        )
                        .truncate(),
                    );
                });
                ui.horizontal(|ui| {
                    let copy_label = t!("diag-copy");
                    copy = ui_kit::button_with_icon(
                        ui,
                        Some(icon::COPY),
                        &copy_label,
                        Kind::Primary,
                        None,
                        true,
                    )
                    .clicked();
                    open_logs = ui_kit::button_with_icon(
                        ui,
                        Some(icon::FOLDER_OPEN),
                        &t!("diag-open-logs"),
                        Kind::Normal,
                        None,
                        true,
                    )
                    .clicked();
                    clear = ui_kit::button_with_icon(
                        ui,
                        Some(icon::BROOM),
                        &t!("diag-clear"),
                        Kind::Normal,
                        None,
                        true,
                    )
                    .clicked();
                });
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(t!("diag-note"))
                            .size(12.0)
                            .color(color::LABEL),
                    )
                    .wrap(),
                );
                let count = |label: String, n: usize| match n {
                    0 => label,
                    n => format!("{label} {n}"),
                };
                ui_kit::segmented(
                    ui,
                    &mut tab,
                    &[
                        (Tab::Overview, &t!("diag-tab-overview"), ""),
                        (
                            Tab::Operations,
                            &count(t!("diag-tab-operations"), snap.ops.len()),
                            "",
                        ),
                        (Tab::Log, &count(t!("diag-tab-log"), snap.log.len()), ""),
                        (
                            Tab::Documents,
                            &count(t!("diag-tab-documents"), snap.docs.len()),
                            "",
                        ),
                        (Tab::System, &t!("diag-tab-system"), ""),
                    ],
                );
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match tab {
                        Tab::Overview => overview(ui, &snap),
                        Tab::Operations => operations(ui, &snap, &mut sort),
                        Tab::Log => log_tab(ui, &snap),
                        Tab::Documents => documents(ui, &snap),
                        Tab::System => self.system_tab(ui, &snap),
                    });
            },
        );
        self.diagnostics.snapshot = Some(snap);
        (self.diagnostics.tab, self.diagnostics.sort) = (tab, sort);
        if out.cancel {
            self.diagnostics.open = false;
            self.diagnostics.snapshot = None;
        }
        if copy {
            ctx.copy_text(self.diagnostics_report());
            self.status = t!("diag-status-copied");
        }
        if open_logs && let Err(e) = models_window::open_folder(&diag::log_dir()) {
            self.status = t!("diag-open-logs-failed", error = e.to_string());
        }
        if clear {
            diag::clear();
            self.diagnostics.snapshot = None;
        }
    }

    /// "Atelier 0.3.2 · linux/x86_64 · GPU (Vulkan) · up for 5 min".
    fn subtitle(&self) -> String {
        let mut parts = vec![
            format!("Atelier {}", env!("CARGO_PKG_VERSION")),
            format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        ];
        if let Some(gpu) = &self.diagnostics.gpu {
            parts.push(format!("{} ({})", gpu.name, gpu.backend));
        }
        parts.push(t!("diag-uptime", duration = duration(diag::uptime())));
        parts.join(" · ")
    }

    fn system_tab(&self, ui: &mut egui::Ui, snap: &Snapshot) {
        let s = &snap.system;
        let version = env!("CARGO_PKG_VERSION");
        let mut rows = vec![
            (
                t!("diag-sys-version"),
                if cfg!(debug_assertions) {
                    t!("diag-sys-debug", version = version)
                } else {
                    t!("diag-sys-release", version = version)
                },
            ),
            (t!("diag-sys-os"), os_text(s)),
        ];
        match &self.diagnostics.gpu {
            Some(gpu) => {
                let mut text = format!("{} · {} · {}", gpu.name, gpu.backend, gpu.kind);
                if !gpu.driver.is_empty() {
                    text = format!("{text} · {}", gpu.driver);
                }
                rows.push((t!("diag-sys-gpu"), text));
                rows.push((
                    t!("diag-sys-max-texture"),
                    format!("{0} × {0}", gpu.max_texture),
                ));
            }
            None => rows.push((t!("diag-sys-gpu"), t!("diag-sys-gpu-unknown"))),
        }
        rows.push((t!("diag-sys-cores"), s.cores.to_string()));
        rows.push((
            t!("diag-sys-threads"),
            snap.threads
                .map_or_else(|| t!("diag-unknown"), |n| n.to_string()),
        ));
        rows.push((t!("diag-sys-scale"), scale_text(s)));
        rows.push((t!("diag-sys-window"), window_text(s)));
        rows.push((t!("diag-sys-language"), s.lang.native_name().to_owned()));
        let c = &snap.color;
        rows.push((t!("diag-sys-working-rgb"), c.working_rgb.clone()));
        rows.push((t!("diag-sys-working-cmyk"), c.working_cmyk.clone()));
        rows.push((t!("diag-sys-intent"), c.intent.label()));
        let monitor = if c.monitor_known {
            c.monitor.clone()
        } else {
            t!("diag-sys-monitor-assumed", name = c.monitor.as_str())
        };
        rows.push((t!("diag-sys-monitor"), monitor));
        rows.push((
            t!("diag-sys-history-budget"),
            t!(
                "diag-sys-history-budget-value",
                active = size_text(HISTORY_BUDGET as u64),
                total = size_text(TOTAL_HISTORY_BUDGET as u64)
            ),
        ));
        rows.push((
            t!("diag-sys-since"),
            t!("diag-ago", duration = duration(snap.since.elapsed())),
        ));
        key_values(ui, egui::Id::new("diag-system"), &rows);
        ui.add_space(8.0);
        ui_kit::hint(
            ui,
            t!("diag-log-file", path = shown_path(&diag::log_file())),
        );
    }

    /// The report copied by "Copy diagnostics": Markdown, in English (for
    /// whoever reads it), without personal data.
    pub(super) fn diagnostics_report(&self) -> String {
        let snap = self.diagnostics_snapshot();
        let mut r = String::new();
        let now = snap.taken;
        let s = &snap.system;
        let build = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };
        r.push_str("## Atelier diagnostics\n\n");
        r.push_str(&format!(
            "- Version: Atelier {} ({build} build)\n",
            env!("CARGO_PKG_VERSION")
        ));
        r.push_str(&format!(
            "- System: {}/{}{}, {} processor threads, {} program threads\n",
            std::env::consts::OS,
            std::env::consts::ARCH,
            if s.flatpak { " (Flatpak)" } else { "" },
            s.cores,
            snap.threads
                .map_or_else(|| "?".to_owned(), |n| n.to_string()),
        ));
        match &self.diagnostics.gpu {
            Some(g) => r.push_str(&format!(
                "- GPU: {} ({}, {}{}), max texture {}\n",
                g.name,
                g.backend,
                g.kind,
                if g.driver.is_empty() {
                    String::new()
                } else {
                    format!(", driver {}", g.driver)
                },
                g.max_texture
            )),
            None => r.push_str("- GPU: unknown\n"),
        }
        r.push_str(&format!(
            "- Display: scale {:.2}{}, window {:.0} × {:.0} points{}\n",
            s.scale,
            s.native_scale
                .map_or_else(String::new, |n| format!(" (native {n:.2})")),
            s.window.x,
            s.window.y,
            s.monitor.map_or_else(String::new, |m| format!(
                ", monitor {:.0} × {:.0} points",
                m.x, m.y
            )),
        ));
        r.push_str(&format!(
            "- Uptime: {}, language {}, measures since {} ago\n",
            en_duration(diag::uptime()),
            s.lang.code(),
            en_duration(now.saturating_duration_since(snap.since)),
        ));

        r.push_str("\n### Overview\n\n");
        let f = &snap.frames;
        r.push_str(&format!(
            "- Frame time (1 min): average {}, max {}, {} frames\n",
            en_duration(f.average),
            en_duration(f.max),
            f.frames
        ));
        let minute: Vec<&Freeze> = snap
            .freezes
            .iter()
            .filter(|z| now.saturating_duration_since(z.at) <= MINUTE)
            .collect();
        let total: Duration = minute.iter().map(|z| z.took).sum();
        let max = minute.iter().map(|z| z.took).max().unwrap_or_default();
        r.push_str(&format!(
            "- Freezes over {} ms: {} in the last minute ({} in total, max {}), {} in 5 min\n",
            diag::FREEZE.as_millis(),
            minute.len(),
            en_duration(total),
            en_duration(max),
            snap.freezes.len()
        ));
        match snap.memory {
            Some(m) => r.push_str(&format!(
                "- Process memory: {} resident{}\n",
                en_size(m.resident as usize),
                m.private.map_or_else(String::new, |p| format!(
                    ", {} private",
                    en_size(p as usize)
                ))
            )),
            None => r.push_str("- Process memory: unknown\n"),
        }
        let history: usize = snap.docs.iter().map(|d| d.history_bytes).sum();
        r.push_str(&format!(
            "- History memory: {} of {} for all tabs\n",
            en_size(history),
            en_size(TOTAL_HISTORY_BUDGET)
        ));
        let t = &snap.tiles;
        r.push_str(&format!(
            "- Canvas rendering: level {}, last frame {} tiles, max {} per frame, {} frames over the time budget (1 min)\n",
            t.level, t.last, t.max, t.over_budget
        ));
        r.push_str(&format!(
            "- Background jobs: {}\n",
            if snap.jobs.is_empty() {
                "none".to_owned()
            } else {
                snap.jobs.join(", ")
            }
        ));
        let runtime = if cfg!(windows) {
            "ONNX Runtime embedded (DirectML), loaded on first use"
        } else {
            "ONNX Runtime linked in (CPU)"
        };
        let ran: Vec<&str> = snap
            .ai_devices
            .iter()
            .map(|d| match d {
                Device::Gpu => "GPU",
                Device::Cpu => "CPU",
            })
            .collect();
        let installed: Vec<&str> = snap.ai_installed.iter().map(|m| m.name()).collect();
        r.push_str(&format!(
            "- AI: {runtime}; ran on {}; models installed: {}\n",
            if ran.is_empty() {
                "nothing yet".to_owned()
            } else {
                ran.join(", ")
            },
            if installed.is_empty() {
                "none".to_owned()
            } else {
                installed.join(", ")
            }
        ));
        let c = &snap.color;
        r.push_str(&format!(
            "- Color: document {}, monitor {}{}, working {} / {}, intent {:?}{}, proof colors {}, gamut warning {}\n",
            c.document.as_deref().unwrap_or("none"),
            c.monitor,
            if c.monitor_known { "" } else { " (assumed)" },
            c.working_rgb,
            c.working_cmyk,
            c.intent,
            if c.bpc { " with BPC" } else { "" },
            on_off(c.proof),
            on_off(c.gamut),
        ));
        r.push_str(&format!(
            "- Messages: {} warnings, {} errors\n",
            snap.warnings, snap.errors
        ));

        r.push_str("\n### Freezes (5 min)\n\n");
        if snap.freezes.is_empty() {
            r.push_str("None.\n");
        }
        for z in snap.freezes.iter().rev() {
            r.push_str(&format!(
                "- {} ago: {} — {}\n",
                en_duration(now.saturating_duration_since(z.at)),
                en_duration(z.took),
                z.context
            ));
        }

        r.push_str("\n### Slowest operations\n\n");
        if snap.ops.is_empty() {
            r.push_str("None measured.\n");
        } else {
            r.push_str("| Operation | Count | Last | Average | Max | Last run |\n");
            r.push_str("|---|---:|---:|---:|---:|---:|\n");
            for (name, o) in sorted(&snap.ops, Sort::Slowest)
                .into_iter()
                .take(REPORT_ROWS)
            {
                r.push_str(&format!(
                    "| {name}{} | {} | {} | {} | {} | {} ago |\n",
                    if o.background { " (background)" } else { "" },
                    o.count,
                    en_duration(o.last),
                    en_duration(o.total / o.count.max(1)),
                    en_duration(o.max),
                    en_duration(now.saturating_duration_since(o.last_at)),
                ));
            }
        }

        r.push_str("\n### Documents\n\n");
        if snap.docs.is_empty() {
            r.push_str("None open.\n");
        } else {
            r.push_str("| # | Format | Size | Layers | History | Layer pixels | Profile |\n");
            r.push_str("|---|---|---|---|---|---:|---|\n");
            for (i, d) in snap.docs.iter().enumerate() {
                let format = match (&d.format, d.embedded) {
                    (_, true) => "smart object contents".to_owned(),
                    (Some(f), _) => f.clone(),
                    (None, _) => "not saved".to_owned(),
                };
                r.push_str(&format!(
                    "| {}{}{} | {format} | {} × {} | {} ({}) | {} of {} steps, {} | {} | {} |\n",
                    i + 1,
                    if d.active { " (active)" } else { "" },
                    if d.modified { " (modified)" } else { "" },
                    d.size.0,
                    d.size.1,
                    d.layers(),
                    d.kinds_en(),
                    d.done,
                    d.steps,
                    en_size(d.history_bytes),
                    en_size(d.layer_bytes),
                    d.profile,
                ));
            }
        }

        r.push_str("\n### Log\n\n");
        if snap.log.is_empty() {
            r.push_str("No warning or error.\n");
        }
        let skip = snap.log.len().saturating_sub(REPORT_LOG);
        for e in &snap.log[skip..] {
            r.push_str(&format!(
                "- {} {} {}\n",
                diag::timestamp(e.wall),
                e.level.code(),
                e.message.replace('\n', " ")
            ));
        }
        // Whatever slipped through (a profile or GPU name with the home
        // folder in it…).
        diag::scrub(&r)
    }
}

/// The overview's cards, in a grid as wide as the window.
fn overview(ui: &mut egui::Ui, snap: &Snapshot) {
    let cards = cards(snap);
    let gap = 8.0;
    let width = ui.available_width();
    let columns = (((width + gap) / (CARD_MIN + gap)).floor() as usize).clamp(1, 4);
    let card_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
    ui.spacing_mut().item_spacing = vec2(gap, gap);
    for row in cards.chunks(columns) {
        ui.horizontal_top(|ui| {
            for c in row {
                card(ui, card_w, c);
            }
        });
    }
}

struct Card {
    glyph: &'static str,
    title: String,
    value: String,
    detail: String,
    /// Something to look at (shown in the warning color).
    warn: bool,
}

fn cards(snap: &Snapshot) -> Vec<Card> {
    let now = snap.taken;
    let f = &snap.frames;
    let minute: Vec<&Freeze> = snap
        .freezes
        .iter()
        .filter(|z| now.saturating_duration_since(z.at) <= MINUTE)
        .collect();
    let freeze_total: Duration = minute.iter().map(|z| z.took).sum();
    let freeze_max = minute.iter().map(|z| z.took).max().unwrap_or_default();
    let history: usize = snap.docs.iter().map(|d| d.history_bytes).sum();
    let steps: usize = snap.docs.iter().map(|d| d.steps).sum();
    let largest = snap
        .docs
        .iter()
        .max_by_key(|d| u64::from(d.size.0) * u64::from(d.size.1));
    let layers: usize = snap.docs.iter().map(DocInfo::layers).sum();
    let t = &snap.tiles;
    let devices: Vec<String> = snap
        .ai_devices
        .iter()
        .map(|d| match d {
            Device::Gpu => t!("models-device-gpu"),
            Device::Cpu => t!("models-device-cpu"),
        })
        .collect();
    let c = &snap.color;
    vec![
        Card {
            glyph: icon::GAUGE,
            title: t!("diag-card-frame"),
            value: if f.frames == 0 {
                "–".to_owned()
            } else {
                duration(f.average)
            },
            detail: t!(
                "diag-card-frame-detail",
                max = duration(f.max),
                frames = f.frames
            ),
            warn: f.average > Duration::from_millis(33),
        },
        Card {
            glyph: icon::SNOWFLAKE,
            title: t!("diag-card-freezes"),
            value: minute.len().to_string(),
            detail: t!(
                "diag-card-freezes-detail",
                total = duration(freeze_total),
                max = duration(freeze_max),
                five = snap.freezes.len()
            ),
            warn: !minute.is_empty(),
        },
        Card {
            glyph: icon::MEMORY,
            title: t!("diag-card-memory"),
            value: snap
                .memory
                .map_or_else(|| "–".to_owned(), |m| size_text(m.resident)),
            detail: match snap.memory.and_then(|m| m.private) {
                Some(p) => t!("diag-card-memory-detail", private = size_text(p)),
                None => t!("diag-unknown"),
            },
            warn: false,
        },
        Card {
            glyph: icon::CLOCK_COUNTER_CLOCKWISE,
            title: t!("diag-card-history"),
            value: size_text(history as u64),
            detail: t!(
                "diag-card-history-detail",
                budget = size_text(TOTAL_HISTORY_BUDGET as u64),
                steps = steps
            ),
            warn: history > TOTAL_HISTORY_BUDGET / 10 * 8,
        },
        Card {
            glyph: icon::IMAGES,
            title: t!("diag-card-documents"),
            value: snap.docs.len().to_string(),
            detail: match largest {
                Some(d) => t!(
                    "diag-card-documents-detail",
                    width = d.size.0,
                    height = d.size.1,
                    layers = layers
                ),
                None => t!("diag-card-documents-none"),
            },
            warn: false,
        },
        Card {
            glyph: icon::SQUARES_FOUR,
            title: t!("diag-card-rendering"),
            value: t!("diag-card-rendering-value", tiles = t.last),
            detail: t!(
                "diag-card-rendering-detail",
                max = t.max,
                level = t.level,
                over = t.over_budget
            ),
            warn: false,
        },
        Card {
            glyph: icon::SPINNER,
            title: t!("diag-card-jobs"),
            value: snap.jobs.len().to_string(),
            detail: if snap.jobs.is_empty() {
                t!("diag-card-jobs-none")
            } else {
                snap.jobs.join(" · ")
            },
            warn: false,
        },
        Card {
            glyph: icon::SPARKLE,
            title: t!("diag-card-ai"),
            value: if devices.is_empty() {
                t!("diag-ai-unused")
            } else {
                devices.join(" · ")
            },
            detail: t!(
                "diag-card-ai-detail",
                installed = snap.ai_installed.len(),
                total = Model::ALL.len()
            ),
            warn: false,
        },
        Card {
            glyph: icon::PALETTE,
            title: t!("diag-card-color"),
            value: c.document.clone().unwrap_or_else(|| c.working_rgb.clone()),
            detail: t!(
                "diag-card-color-detail",
                monitor = c.monitor.as_str(),
                proof = if c.proof {
                    t!("diag-on")
                } else {
                    t!("diag-off")
                }
            ),
            warn: false,
        },
        Card {
            glyph: icon::WARNING,
            title: t!("diag-card-log"),
            value: (snap.warnings + snap.errors).to_string(),
            detail: t!(
                "diag-card-log-detail",
                warnings = snap.warnings,
                errors = snap.errors
            ),
            warn: snap.errors > 0,
        },
    ]
}

fn card(ui: &mut egui::Ui, width: f32, c: &Card) {
    egui::Frame::new()
        .fill(color::BG1)
        .stroke(UiStroke::new(1.0, color::LINE))
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            ui.set_height(CARD_HEIGHT);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                card_body(ui, c);
            });
        });
}

fn card_body(ui: &mut egui::Ui, c: &Card) {
    ui.spacing_mut().item_spacing.y = 4.0;
    let accent = if c.warn {
        workspace::WARNING
    } else {
        color::ACCENT_TEXT
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(egui::RichText::new(c.glyph).size(15.0).color(accent));
        ui.add(
            egui::Label::new(egui::RichText::new(&c.title).size(12.0).color(color::LABEL))
                .truncate(),
        );
    });
    ui.add(
        egui::Label::new(
            egui::RichText::new(&c.value)
                .font(ui_kit::bold(20.0))
                .color(if c.warn { accent } else { color::TEXT }),
        )
        .truncate(),
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new(&c.detail)
                .size(12.0)
                .color(color::MUTED),
        )
        .wrap(),
    );
}

/// The operations, the slowest or the latest first.
fn sorted(ops: &[(String, OpStats)], sort: Sort) -> Vec<(&str, &OpStats)> {
    let mut ops: Vec<(&str, &OpStats)> = ops.iter().map(|(n, s)| (n.as_str(), s)).collect();
    match sort {
        Sort::Slowest => ops.sort_by(|a, b| b.1.max.cmp(&a.1.max).then(a.0.cmp(b.0))),
        Sort::Recent => ops.sort_by(|a, b| b.1.last_at.cmp(&a.1.last_at).then(a.0.cmp(b.0))),
    }
    ops
}

fn operations(ui: &mut egui::Ui, snap: &Snapshot, sort: &mut Sort) {
    let now = snap.taken;
    ui_kit::section(ui, &t!("diag-freezes"));
    if snap.freezes.is_empty() {
        ui_kit::hint(ui, t!("diag-freezes-none"));
    }
    for z in snap.freezes.iter().rev() {
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(64.0, 18.0), Sense::hover());
            ui.painter().text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                duration(z.took),
                ui_kit::mono_medium(12.0),
                workspace::WARNING,
            );
            let (rect, _) = ui.allocate_exact_size(vec2(96.0, 18.0), Sense::hover());
            ui.painter().text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                t!(
                    "diag-ago",
                    duration = duration(now.saturating_duration_since(z.at))
                ),
                egui::FontId::proportional(12.0),
                color::LABEL,
            );
            ui.add(
                egui::Label::new(egui::RichText::new(&z.context).font(ui_kit::mono(12.0))).wrap(),
            );
        });
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui_kit::section(ui, &t!("diag-operations"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui_kit::segmented(
                ui,
                sort,
                &[
                    (Sort::Recent, &t!("diag-sort-recent"), ""),
                    (Sort::Slowest, &t!("diag-sort-slowest"), ""),
                ],
            );
        });
    });
    if snap.ops.is_empty() {
        ui_kit::hint(ui, t!("diag-operations-none"));
        return;
    }
    // Numbers right-aligned, except the last column (it would take the
    // rest of the width).
    let cell = |ui: &mut egui::Ui, text: egui::RichText, right: bool| {
        if right {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(text);
            });
        } else {
            ui.label(text);
        }
    };
    let header = |text: String| egui::RichText::new(text).size(12.0).color(color::LABEL);
    let number = |text: String, strong: bool| {
        egui::RichText::new(text).font(if strong {
            ui_kit::mono_medium(12.0)
        } else {
            ui_kit::mono(12.0)
        })
    };
    egui::Grid::new("diag-operations")
        .striped(true)
        .num_columns(6)
        .spacing(vec2(16.0, 4.0))
        .show(ui, |ui| {
            cell(ui, header(t!("diag-col-operation")), false);
            cell(ui, header(t!("diag-col-count")), true);
            cell(ui, header(t!("diag-col-last")), true);
            cell(ui, header(t!("diag-col-average")), true);
            cell(ui, header(t!("diag-col-max")), true);
            cell(ui, header(t!("diag-col-when")), false);
            ui.end_row();
            for (name, o) in sorted(&snap.ops, *sort).into_iter().take(MAX_ROWS) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(egui::RichText::new(name).font(ui_kit::mono(12.0)));
                    if o.background {
                        ui.label(
                            egui::RichText::new(icon::ARROWS_CLOCKWISE)
                                .size(12.0)
                                .color(color::LABEL),
                        )
                        .on_hover_text(t!("diag-background-tip"));
                    }
                });
                let slow = o.max > diag::FREEZE && !o.background;
                cell(ui, number(o.count.to_string(), false), true);
                cell(ui, number(duration(o.last), false), true);
                cell(ui, number(duration(o.total / o.count.max(1)), false), true);
                cell(ui, number(duration(o.max), slow), true);
                let ago = duration(now.saturating_duration_since(o.last_at));
                cell(ui, header(t!("diag-ago", duration = ago)), false);
                ui.end_row();
            }
        });
}

fn log_tab(ui: &mut egui::Ui, snap: &Snapshot) {
    if snap.log.is_empty() {
        ui_kit::hint(ui, t!("diag-log-none"));
    }
    for e in snap.log.iter().rev() {
        let (label, tint) = match e.level {
            Level::Warning => (t!("diag-level-warning"), workspace::WARNING),
            Level::Error => (t!("diag-level-error"), color::CLOSE_HOVER),
            Level::Panic => (t!("diag-level-panic"), color::CLOSE_HOVER),
        };
        ui.horizontal_top(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(110.0, 18.0), Sense::hover());
            ui.painter().text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                label,
                ui_kit::bold(12.0),
                tint,
            );
            let (rect, _) = ui.allocate_exact_size(vec2(96.0, 18.0), Sense::hover());
            ui.painter().text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                t!(
                    "diag-ago",
                    duration = duration(snap.taken.saturating_duration_since(e.at))
                ),
                egui::FontId::proportional(12.0),
                color::LABEL,
            );
            ui.add(egui::Label::new(e.message.as_str()).wrap());
        });
    }
    ui.add_space(8.0);
    ui_kit::hint(
        ui,
        t!("diag-log-file", path = shown_path(&diag::log_file())),
    );
}

fn documents(ui: &mut egui::Ui, snap: &Snapshot) {
    if snap.docs.is_empty() {
        ui_kit::hint(ui, t!("diag-card-documents-none"));
    }
    for (i, d) in snap.docs.iter().enumerate() {
        let mut title = d.name.clone();
        if d.active {
            title = format!("{title} · {}", t!("diag-doc-active"));
        }
        ui_kit::section(ui, &title);
        let format = match (&d.format, d.embedded) {
            (_, true) => t!("diag-doc-embedded"),
            (Some(f), _) => f.to_uppercase(),
            (None, _) => t!("diag-doc-unsaved"),
        };
        let rows = [
            (t!("diag-doc-size"), format!("{} × {}", d.size.0, d.size.1)),
            (
                t!("diag-doc-layers"),
                format!("{} · {}", d.layers(), d.kinds_text()),
            ),
            (t!("diag-doc-pixels"), size_text(d.layer_bytes as u64)),
            (
                t!("diag-doc-history"),
                t!(
                    "diag-doc-history-value",
                    done = d.done,
                    steps = d.steps,
                    memory = size_text(d.history_bytes as u64)
                ),
            ),
            (t!("diag-doc-profile"), d.profile.clone()),
            (t!("diag-doc-format"), format),
        ];
        key_values(ui, egui::Id::new("diag-doc").with(i), &rows);
        ui.add_space(10.0);
    }
}

/// Two columns: what, its value.
fn key_values(ui: &mut egui::Ui, id: egui::Id, rows: &[(String, String)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing(vec2(24.0, 6.0))
        .min_col_width(160.0)
        .show(ui, |ui| {
            for (key, value) in rows {
                ui.label(egui::RichText::new(key).size(12.0).color(color::LABEL));
                ui.add(
                    egui::Label::new(egui::RichText::new(value).font(ui_kit::mono(12.0))).wrap(),
                );
                ui.end_row();
            }
        });
}

fn os_text(s: &SystemInfo) -> String {
    let mut text = format!("{} · {}", std::env::consts::OS, std::env::consts::ARCH);
    if s.flatpak {
        text.push_str(" · Flatpak");
    }
    text
}

fn scale_text(s: &SystemInfo) -> String {
    match s.native_scale {
        Some(n) if (n - s.scale).abs() > 0.001 => format!(
            "{} ({})",
            i18n::decimal(f64::from(s.scale), 2),
            i18n::decimal(f64::from(n), 2)
        ),
        _ => i18n::decimal(f64::from(s.scale), 2),
    }
}

fn window_text(s: &SystemInfo) -> String {
    let mut text = format!("{:.0} × {:.0}", s.window.x, s.window.y);
    if let Some(m) = s.monitor {
        text = format!("{text} · {:.0} × {:.0}", m.x, m.y);
    }
    text
}

/// A path as shown: the home folder as `~`.
fn shown_path(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        Some(home) if !home.is_empty() => text.replace(&*home.to_string_lossy(), "~"),
        _ => text,
    }
}

/// "4,2 ms", "1,4 s", "3 min", "1 h 5 min", in the interface's language.
fn duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        let ms = secs * 1000.0;
        let value = if ms < 1.0 {
            i18n::decimal(ms, 2)
        } else if ms < 10.0 {
            i18n::short_decimal(ms)
        } else {
            format!("{ms:.0}")
        };
        t!("diag-ms", value = value)
    } else if secs < 60.0 {
        t!("diag-s", value = i18n::short_decimal(secs))
    } else if secs < 3600.0 {
        t!("diag-min", value = (secs / 60.0).floor() as u64)
    } else {
        let minutes = (secs / 60.0).floor() as u64;
        t!("diag-hours", hours = minutes / 60, minutes = minutes % 60)
    }
}

/// The same for the report, always in English.
fn en_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        let ms = secs * 1000.0;
        if ms < 1.0 {
            format!("{ms:.2} ms")
        } else if ms < 10.0 {
            format!("{ms:.1} ms")
        } else {
            format!("{ms:.0} ms")
        }
    } else if secs < 60.0 {
        format!("{secs:.1} s")
    } else if secs < 3600.0 {
        format!("{:.0} min", (secs / 60.0).floor())
    } else {
        let minutes = (secs / 60.0).floor() as u64;
        format!("{} h {} min", minutes / 60, minutes % 60)
    }
}

/// "820 MB", "1.2 GB" (decimal units, like the rest of the interface).
fn en_size(bytes: usize) -> String {
    let mb = bytes as f64 / 1e6;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else if mb >= 10.0 || bytes == 0 {
        format!("{mb:.0} MB")
    } else {
        format!("{mb:.1} MB")
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

#[cfg(test)]
impl AtelierApp {
    /// The window open on a tab, with a few measures and messages (see
    /// `ATELIER_DIALOG` in the capture test).
    pub(super) fn capture_diagnostics(&mut self, what: &str) {
        self.run(Action::NewLayer);
        self.run(Action::Duplicate);
        diag::record("Open: psd", Duration::from_millis(1240));
        diag::record("Filter: GaussianBlur", Duration::from_millis(380));
        diag::frame(Instant::now(), Duration::from_millis(112));
        diag::freeze(
            Duration::from_millis(112),
            "tool Brush · stroke in progress · ran Stroke end: Brush 104 ms".to_owned(),
        );
        diag::warn("Opened poster.psd simplified: 2 adjustment layers rasterized");
        diag::error("Can't save poster.png: no space left on device");
        self.open_diagnostics();
        self.diagnostics.tab = match what {
            "diagnostics-operations" => Tab::Operations,
            "diagnostics-log" => Tab::Log,
            "diagnostics-documents" => Tab::Documents,
            "diagnostics-system" => Tab::System,
            _ => Tab::Overview,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;

    fn app(with_doc: Option<PathBuf>) -> Harness<'static, AtelierApp> {
        i18n::set_lang(Lang::En);
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_eframe(move |cc| {
                let mut app = AtelierApp::new(cc, None);
                if let Some(path) = with_doc {
                    let img =
                        image::RgbaImage::from_pixel(64, 48, image::Rgba([90, 120, 200, 255]));
                    app.doc = Some(Document::new(img, Some(path)));
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

    /// Help › Diagnostics… opens the window, which shows its tabs.
    #[test]
    fn the_help_menu_opens_the_window() {
        let mut h = app(None);
        let help = h
            .get_all_by_label(&t!("ui3-menu-help"))
            .map(|n| n.rect())
            .find(|r| r.top() < 40.0)
            .expect("in the menu bar")
            .center();
        click(&mut h, help);
        let item = h.get_by_label(&t!("diag-menu")).rect().center();
        click(&mut h, item);
        h.run_steps(2);
        assert!(h.state().diagnostics.open);
        assert!(h.query_by_label(&t!("diag-copy")).is_some());
        assert!(h.query_by_label(&t!("diag-card-memory")).is_some());
        // The tabs show what they list.
        let log = t!("diag-tab-log");
        let tab = h.get_by_label_contains(&log).rect().center();
        click(&mut h, tab);
        assert_eq!(h.state().diagnostics.tab, Tab::Log);
        // Its close button closes it.
        let close = h
            .get_all_by_label(&t!("dialog-close-tip"))
            .last()
            .expect("close button")
            .rect()
            .center();
        click(&mut h, close);
        assert!(!h.state().diagnostics.open);
    }

    /// The command palette finds it too.
    #[test]
    fn the_palette_lists_the_window() {
        let h = app(None);
        let found = h.state().menus().iter().any(|(_, entries)| {
            entries.iter().any(|e| {
                matches!(e, MenuEntry::Item(label, true, _, Action::Workspace(workspace::Command::Diagnostics)) if *label == t!("diag-menu"))
            })
        });
        assert!(found);
    }

    /// The report keeps the home folder and the documents' folders out.
    #[test]
    fn the_report_has_no_home_path() {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/home/tester".to_owned());
        let doc_path = PathBuf::from(&home)
            .join("Pictures")
            .join("secret-trip")
            .join("beach.png");
        let mut h = app(Some(doc_path.clone()));
        let shown = doc_path.display().to_string();
        diag::error(format!("Can't save {shown}: disk full"));
        h.state_mut()
            .run(Action::Workspace(workspace::Command::Diagnostics));
        h.run_steps(2);
        let report = h.state().diagnostics_report();
        assert!(!report.contains(&home), "{report}");
        assert!(!report.contains("secret-trip"), "{report}");
        assert!(!report.contains("Pictures"), "{report}");
        // The message stays, with the file's name only.
        assert!(
            report.contains("Can't save beach.png: disk full"),
            "{report}"
        );
        // Its sections.
        for heading in [
            "## Atelier diagnostics",
            "### Overview",
            "### Documents",
            "### Log",
        ] {
            assert!(report.contains(heading), "{heading}");
        }
        assert!(report.contains(env!("CARGO_PKG_VERSION")));
        assert!(
            report.contains("| 1 (active) | png | 64 × 48 |"),
            "{report}"
        );
    }

    /// A frame over the freeze threshold is kept with what was going on:
    /// the tool, the dialog and the operation that ran.
    #[test]
    fn freezes_are_recorded_with_their_context() {
        let mut h = app(Some(PathBuf::from("logo.png")));
        h.state_mut().tool = Tool::Brush;
        h.state_mut().run(Action::ImageSize);
        assert!(h.state().image_size.is_some());
        // A frame that started 150 ms ago, and ran a slow operation.
        let start = Instant::now() - Duration::from_millis(150);
        diag::record("Test operation 5c2", Duration::from_millis(140));
        h.state().end_frame(start);
        let freeze = diag::measures(|m| {
            m.freezes(Instant::now(), MINUTE)
                .into_iter()
                .find(|z| z.context.contains("Test operation 5c2"))
                .cloned()
        })
        .expect("recorded");
        assert!(freeze.took > diag::FREEZE);
        assert!(freeze.context.contains("tool Brush"), "{}", freeze.context);
        assert!(
            freeze.context.contains("dialog Image Size"),
            "{}",
            freeze.context
        );
    }

    /// Commands are measured by `run`, under their name.
    #[test]
    fn commands_are_measured() {
        let mut h = app(Some(PathBuf::from("logo.png")));
        h.state_mut().run(Action::FlipCanvas(true));
        let measured = diag::measures(|m| {
            m.operations()
                .iter()
                .any(|(name, s)| *name == "FlipCanvas(true)" && s.count >= 1)
        });
        assert!(measured);
        let snap = h.state().diagnostics_snapshot();
        assert_eq!(snap.docs.len(), 1);
        assert_eq!(snap.docs[0].kinds[0], 1);
        assert!(snap.docs[0].steps >= 1);
    }

    #[test]
    fn durations_read_well() {
        i18n::set_lang(Lang::Fr);
        assert_eq!(duration(Duration::from_micros(4_250)), "4,3 ms");
        assert_eq!(duration(Duration::from_millis(112)), "112 ms");
        assert_eq!(duration(Duration::from_millis(1_400)), "1,4 s");
        assert_eq!(duration(Duration::from_secs(3_900)), "1 h 5 min");
        i18n::set_lang(Lang::En);
        assert_eq!(en_duration(Duration::from_millis(1_400)), "1.4 s");
        assert_eq!(en_size(1_234_000_000), "1.2 GB");
    }
}
