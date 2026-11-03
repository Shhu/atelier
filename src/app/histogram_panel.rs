//! Histogram panel (Window › Histogram): the levels of the image, or of the
//! active layer, within the selection. Counted on another thread, cancelled
//! as soon as what it counts changes; until then the last result stays shown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use image::RgbaImage;

use super::dialogs::{DialogSpec, floating_dialog};
use super::*;
use crate::compose;
use crate::i18n::t;

/// Height of the graph.
const GRAPH: f32 = 110.0;
/// Height of the panel's body.
const BODY: f32 = 290.0;
/// Rows composited between two looks at the cancel flag.
const BAND: u32 = 256;
const POLL: std::time::Duration = std::time::Duration::from_millis(40);
/// How long what a panel shows must stay the same before it is computed
/// again (seconds).
const SETTLE: f64 = 0.15;

/// When a panel may compute what changed. Not while a press on the canvas
/// may edit (`held`): a job keeps the layers alive, and each dab of a stroke
/// would then copy its whole layer on the UI thread. Other presses (a pan, a
/// dialog's slider) don't wait. Not until it stops changing either, unless
/// the panel has nothing to show.
#[derive(Default)]
pub(super) struct Settle {
    key: u64,
    since: f64,
}

impl Settle {
    pub(super) fn ready(
        &mut self,
        ctx: &egui::Context,
        key: u64,
        nothing_shown: bool,
        held: bool,
    ) -> bool {
        let now = ctx.input(|i| i.time);
        if self.key != key {
            self.key = key;
            self.since = now;
        }
        !held && (nothing_shown || now - self.since >= SETTLE)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Channel {
    /// The three channels together.
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
    Luminosity,
    /// The three channels overlaid, in their colors.
    Colors,
}

impl Channel {
    pub(super) const ALL: [Channel; 6] = [
        Channel::Rgb,
        Channel::Red,
        Channel::Green,
        Channel::Blue,
        Channel::Luminosity,
        Channel::Colors,
    ];

    fn label(self) -> String {
        match self {
            Channel::Rgb => t!("panels8-channel-rgb"),
            Channel::Red => t!("panels8-channel-red"),
            Channel::Green => t!("panels8-channel-green"),
            Channel::Blue => t!("panels8-channel-blue"),
            Channel::Luminosity => t!("panels8-channel-luminosity"),
            Channel::Colors => t!("panels8-channel-colors"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) enum Source {
    #[default]
    EntireImage,
    SelectedLayer,
}

impl Source {
    pub(super) const ALL: [Source; 2] = [Source::EntireImage, Source::SelectedLayer];

    fn label(self) -> String {
        match self {
            Source::EntireImage => t!("panels8-source-image"),
            Source::SelectedLayer => t!("panels8-source-layer"),
        }
    }
}

/// How many pixels have each level.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Counts {
    pub red: [u64; 256],
    pub green: [u64; 256],
    pub blue: [u64; 256],
    pub luminosity: [u64; 256],
    pub pixels: u64,
}

impl Counts {
    fn new() -> Self {
        Self {
            red: [0; 256],
            green: [0; 256],
            blue: [0; 256],
            luminosity: [0; 256],
            pixels: 0,
        }
    }

    /// Adds the pixels of `image`, placed at `at`, that show (not fully
    /// transparent) and are selected.
    fn add(&mut self, image: &RgbaImage, at: (i64, i64), selected: Option<&Selected>) {
        for (x, y, p) in image.enumerate_pixels() {
            let [r, g, b, a] = p.0;
            if a == 0 {
                continue;
            }
            if selected.is_some_and(|s| !s.has(at.0 + i64::from(x), at.1 + i64::from(y))) {
                continue;
            }
            self.red[r as usize] += 1;
            self.green[g as usize] += 1;
            self.blue[b as usize] += 1;
            let l = (299 * r as u32 + 587 * g as u32 + 114 * b as u32 + 500) / 1000;
            self.luminosity[l as usize] += 1;
            self.pixels += 1;
        }
    }

    /// The levels of a channel (Colors: as RGB).
    pub(super) fn of(&self, channel: Channel) -> [u64; 256] {
        match channel {
            Channel::Rgb | Channel::Colors => {
                std::array::from_fn(|i| self.red[i] + self.green[i] + self.blue[i])
            }
            Channel::Red => self.red,
            Channel::Green => self.green,
            Channel::Blue => self.blue,
            Channel::Luminosity => self.luminosity,
        }
    }
}

/// The selection, canvas-wide.
struct Selected {
    mask: image::GrayImage,
}

impl Selected {
    fn has(&self, x: i64, y: i64) -> bool {
        (x >= 0 && y >= 0)
            && self
                .mask
                .get_pixel_checked(x as u32, y as u32)
                .is_some_and(|c| c.0[0] >= 128)
    }
}

/// Mean, standard deviation, median and total of a channel's levels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Stats {
    pub mean: f64,
    pub deviation: f64,
    pub median: u32,
    pub total: u64,
}

pub(super) fn stats(levels: &[u64; 256]) -> Stats {
    let total: u64 = levels.iter().sum();
    if total == 0 {
        return Stats {
            mean: 0.0,
            deviation: 0.0,
            median: 0,
            total,
        };
    }
    let n = total as f64;
    let weighted = |f: &dyn Fn(f64) -> f64| {
        levels
            .iter()
            .enumerate()
            .map(|(l, &c)| c as f64 * f(l as f64))
            .sum::<f64>()
    };
    let mean = weighted(&|l| l) / n;
    let deviation = (weighted(&|l| (l - mean) * (l - mean)) / n).sqrt();
    let mut seen = 0;
    let median = levels
        .iter()
        .position(|&c| {
            seen += c;
            seen * 2 >= total
        })
        .unwrap_or(0) as u32;
    Stats {
        mean,
        deviation,
        median,
        total,
    }
}

struct Job {
    key: u64,
    cancel: Arc<AtomicBool>,
    thread: JoinHandle<Option<Counts>>,
}

#[derive(Default)]
pub(super) struct HistogramPanel {
    pub open: bool,
    pub channel: Channel,
    pub source: Source,
    /// The last result, and what it was counted for.
    shown: Option<(u64, Arc<Counts>)>,
    job: Option<Job>,
    settle: Settle,
    /// The last result is for what is there now.
    fresh: bool,
}

impl HistogramPanel {
    /// The last counts, and whether they are for `key`.
    #[cfg(test)]
    pub(super) fn counts(&self) -> Option<&Counts> {
        self.shown.as_ref().map(|s| &*s.1)
    }

    #[cfg(test)]
    pub(super) fn counting(&self) -> bool {
        self.job.is_some()
    }

    /// The count going on was told to stop.
    #[cfg(test)]
    pub(super) fn cancelled(&self) -> bool {
        self.job
            .as_ref()
            .is_some_and(|j| j.cancel.load(Ordering::Relaxed))
    }

    /// A worker is still at it, even cancelled.
    #[cfg(test)]
    pub(super) fn is_running(&self) -> bool {
        self.job.as_ref().is_some_and(|j| !j.thread.is_finished())
    }

    fn stop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl AtelierApp {
    /// A press on the canvas that may change the layers is held: the
    /// panels' jobs are stopped and wait for its end.
    pub(super) fn edit_held(&self, ctx: &egui::Context) -> bool {
        self.canvas_edit_held && ctx.input(|i| i.pointer.primary_down())
    }

    /// What is to be counted, as a fingerprint.
    fn histogram_key(&self) -> u64 {
        let Some(doc) = &self.doc else { return 0 };
        let selection = self.selection.as_ref().map(|s| {
            (
                (s.rect.x, s.rect.y, s.rect.w, s.rect.h),
                s.inverted,
                s.mask.as_ref().map(|m| Arc::as_ptr(m) as usize),
            )
        });
        let layer = match self.hist_panel.source {
            Source::EntireImage => None,
            Source::SelectedLayer => {
                let l = doc.active_layer();
                Some((l.id, l.offset))
            }
        };
        hash_of((
            self.composite_key(),
            self.hist_panel.source,
            layer,
            selection,
        ))
    }

    /// Lands a count that is over, starts the one needed.
    fn update_histogram(&mut self, ctx: &egui::Context) {
        let key = self.histogram_key();
        // (A count still right goes on through other presses.)
        let held = self.edit_held(ctx);
        let panel = &mut self.hist_panel;
        if let Some(job) = panel.job.take_if(|j| j.thread.is_finished())
            && let Ok(Some(counts)) = job.thread.join()
            && job.key == key
        {
            panel.shown = Some((key, Arc::new(counts)));
        }
        panel.fresh = panel.shown.as_ref().is_some_and(|s| s.0 == key);
        if panel.fresh {
            return;
        }
        ctx.request_repaint_after(POLL);
        if let Some(job) = &panel.job {
            if job.key != key || held {
                job.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        if !panel.settle.ready(ctx, key, panel.shown.is_none(), held) {
            return;
        }
        let Some(doc) = &self.doc else { return };
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        let canvas = doc.full_rect();
        let selection = self.selection.clone();
        let held = doc.effects_held();
        let layers = doc.render_layers().to_vec();
        let source = self.hist_panel.source;
        let active = doc.active;
        let thread = std::thread::Builder::new()
            .name("histogram".into())
            .spawn(move || {
                crate::document::watch_styles_only();
                let selected = selection.map(|s| Selected {
                    mask: s.to_mask(canvas),
                });
                let mut counts = Counts::new();
                let cancelled = || stop.load(Ordering::Relaxed);
                match source {
                    Source::SelectedLayer => {
                        let layer = layers.get(active)?;
                        if layer.has_pixels() {
                            let (ox, oy) = layer.offset;
                            count_layer(
                                &mut counts,
                                layer.image(),
                                (ox, oy),
                                canvas,
                                selected.as_ref(),
                            );
                        }
                    }
                    Source::EntireImage => {
                        let options = compose::Options {
                            held,
                            later_styles: true,
                            ..Default::default()
                        };
                        let plan = compose::plan(&layers, canvas, &mut compose::FullSize, &options);
                        let mut y = canvas.y;
                        while y < canvas.bottom() {
                            if cancelled() {
                                return None;
                            }
                            let h = BAND.min((canvas.bottom() - y) as u32);
                            let band = plan.render(PixelRect {
                                x: canvas.x,
                                y,
                                w: canvas.w,
                                h,
                            });
                            counts.add(&band, (canvas.x, y), selected.as_ref());
                            y += i64::from(h);
                        }
                    }
                }
                (!cancelled()).then_some(counts)
            });
        if let Ok(thread) = thread {
            self.hist_panel.job = Some(Job {
                key,
                cancel,
                thread,
            });
        }
    }

    pub(super) fn histogram_window(&mut self, ctx: &egui::Context) {
        // (Hidden by Tab or Full Screen: nothing to compute for.)
        if !self.hist_panel.open || self.doc.is_none() || !self.chrome_shown() {
            self.hist_panel.stop();
            return;
        }
        self.update_histogram(ctx);
        let fresh = self.hist_panel.fresh;
        let counts = self.hist_panel.shown.as_ref().map(|s| Arc::clone(&s.1));
        let mut channel = self.hist_panel.channel;
        let mut source = self.hist_panel.source;
        let pos = dialogs::panel_pos(ctx, "histogram");
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                resize: Some(BODY),
                ..DialogSpec::new("histogram", t!("panels8-histogram-title"), pos)
            },
            |ui| {
                let scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                scroll.show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui_kit::combo(
                        ui,
                        "histogram-channel",
                        Some(&t!("panels8-histogram-channel")),
                        &channel.label(),
                        190.0,
                        |ui| {
                            for c in Channel::ALL {
                                ui.selectable_value(&mut channel, c, c.label());
                            }
                        },
                    );
                    let levels = counts.as_ref().map(|c| c.of(channel));
                    let hovered = draw_graph(ui, counts.as_deref(), channel);
                    let (name, value) = (t!("panels8-histogram-source"), source.label());
                    ui_kit::combo(ui, "histogram-source", Some(&name), &value, 190.0, |ui| {
                        for s in Source::ALL {
                            ui.selectable_value(&mut source, s, s.label());
                        }
                    });
                    let Some(levels) = levels else {
                        ui_kit::hint(ui, t!("panels8-histogram-computing"));
                        return;
                    };
                    let s = stats(&levels);
                    let number = |v: f64| i18n::decimal(v, 2);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.spacing_mut().interact_size.y = 18.0;
                    ui.columns(2, |c| {
                        readout(&mut c[0], &t!("panels8-histogram-mean"), &number(s.mean));
                        readout(
                            &mut c[0],
                            &t!("panels8-histogram-deviation"),
                            &number(s.deviation),
                        );
                        readout(
                            &mut c[0],
                            &t!("panels8-histogram-median"),
                            &s.median.to_string(),
                        );
                        let pixels = counts.as_ref().map_or(0, |c| c.pixels);
                        readout(
                            &mut c[0],
                            &t!("panels8-histogram-pixels"),
                            &pixels.to_string(),
                        );
                        // Under the pointer on the graph.
                        let dash = || "—".to_owned();
                        let at = |f: &dyn Fn(usize) -> String| hovered.map_or_else(dash, f);
                        let below = |l: usize| levels[..=l].iter().sum::<u64>() as f64;
                        let percentile =
                            |l: usize| i18n::decimal(100.0 * below(l) / (s.total.max(1) as f64), 2);
                        readout(
                            &mut c[1],
                            &t!("panels8-histogram-level"),
                            &at(&|l| l.to_string()),
                        );
                        readout(
                            &mut c[1],
                            &t!("panels8-histogram-count"),
                            &at(&|l| levels[l].to_string()),
                        );
                        readout(
                            &mut c[1],
                            &t!("panels8-histogram-percentile"),
                            &at(&percentile),
                        );
                    });
                    if !fresh {
                        ui_kit::hint(ui, t!("panels8-histogram-computing"));
                    }
                });
            },
        );
        self.hist_panel.channel = channel;
        self.hist_panel.source = source;
        if out.cancel {
            self.hist_panel.open = false;
            self.hist_panel.stop();
        }
    }
}

/// The part of `image`, at `at`, over the canvas.
fn count_layer(
    counts: &mut Counts,
    image: &RgbaImage,
    at: (i64, i64),
    canvas: PixelRect,
    selected: Option<&Selected>,
) {
    let area = PixelRect {
        x: at.0,
        y: at.1,
        w: image.width(),
        h: image.height(),
    };
    let Some(r) = crate::document::intersect(area, canvas) else {
        return;
    };
    let part = image::imageops::crop_imm(image, (r.x - at.0) as u32, (r.y - at.1) as u32, r.w, r.h)
        .to_image();
    counts.add(&part, (r.x, r.y), selected);
}

fn readout(ui: &mut egui::Ui, name: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let name = egui::RichText::new(name).color(ui_kit::color::LABEL);
        ui.add(egui::Label::new(name).truncate());
        let value = egui::RichText::new(value)
            .font(ui_kit::mono(12.0))
            .color(ui_kit::color::TEXT);
        ui.add(egui::Label::new(value).truncate());
    });
}

/// The tallest level drawn at full height: spikes (often at the very ends)
/// don't flatten the rest.
fn graph_top(levels: &[u64; 256]) -> u64 {
    let mut sorted = *levels;
    sorted.sort_unstable();
    // The fourth tallest, times a margin, or the tallest if less.
    let cap = sorted[252].saturating_mul(2).max(1);
    sorted[255].min(cap).max(1)
}

/// The graph; returns the level under the pointer.
fn draw_graph(ui: &mut egui::Ui, counts: Option<&Counts>, channel: Channel) -> Option<usize> {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, GRAPH), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, ui_kit::color::BG0);
    let counts = counts?;
    let column =
        |i: usize, levels: &[u64; 256], top: u64| (levels[i].min(top) as f32 / top as f32) * GRAPH;
    let step = rect.width() / 256.0;
    let line = |i: usize, from: f32, to: f32, color: Color32| {
        let x = rect.left() + (i as f32 + 0.5) * step;
        painter.line_segment(
            [pos2(x, rect.bottom() - from), pos2(x, rect.bottom() - to)],
            UiStroke::new(step.max(1.0), color),
        );
    };
    if channel == Channel::Colors {
        // Where channels overlap: their mix.
        let top = graph_top(&counts.red)
            .max(graph_top(&counts.green))
            .max(graph_top(&counts.blue));
        for i in 0..256 {
            let (r, g, b) = (
                column(i, &counts.red, top),
                column(i, &counts.green, top),
                column(i, &counts.blue, top),
            );
            let mut cuts = [0.0, r, g, b];
            cuts.sort_by(f32::total_cmp);
            for pair in cuts.windows(2) {
                let (from, to) = (pair[0], pair[1]);
                if to > from {
                    let on = |h: f32| if h >= to { 255 } else { 0 };
                    line(i, from, to, Color32::from_rgb(on(r), on(g), on(b)));
                }
            }
        }
    } else {
        let color = match channel {
            Channel::Red => Color32::from_rgb(240, 70, 70),
            Channel::Green => Color32::from_rgb(70, 220, 90),
            Channel::Blue => Color32::from_rgb(90, 130, 255),
            _ => ui_kit::color::TEXT,
        };
        let levels = counts.of(channel);
        let top = graph_top(&levels);
        for i in 0..256 {
            line(i, 0.0, column(i, &levels, top), color);
        }
    }
    let level = response
        .hover_pos()
        .map(|p| (((p.x - rect.left()) / step) as usize).min(255))?;
    let x = rect.left() + (level as f32 + 0.5) * step;
    painter.vline(
        x,
        rect.y_range(),
        UiStroke::new(1.0, ui_kit::color::ACCENT_HOVER),
    );
    Some(level)
}
