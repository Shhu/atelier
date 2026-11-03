//! Info panel (Window › Info, F8): the values under the pointer, its place,
//! the selection's and the document's sizes, the Color Sampler points, the
//! Ruler's measure.

use super::dialogs::{DialogSpec, floating_dialog};
use super::*;
use crate::i18n::t;

/// Values of one place of the image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Reading {
    pub rgb: [u8; 3],
    /// Ink percentages in the working CMYK.
    pub cmyk: Option<[f32; 4]>,
}

#[derive(Default)]
pub(super) struct InfoPanel {
    pub open: bool,
    /// Under the pointer: what it is computed for (the pixel, the image).
    cursor: Option<(u64, Option<Reading>)>,
    /// The points: what they are computed for.
    samplers: Option<(u64, Vec<Option<Reading>>)>,
    /// When the points were last read (UI time, s).
    samplers_at: f64,
}

#[cfg(test)]
impl InfoPanel {
    pub(super) fn samplers_at(&self) -> f64 {
        self.samplers_at
    }
}

/// How often the points are read again while a press on the canvas edits
/// (s): not at each dab, the stroke would wait for them.
const SAMPLERS_WHILE_HELD: f64 = 0.25;

/// Height of a line of values, of a line between blocks.
const ROW: f32 = 20.0;
const LINE: f32 = 16.0;

/// One name and value of a block.
fn row(ui: &mut egui::Ui, name: &str, value: &str) {
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

/// A level, or a dash where the image shows nothing.
fn level(value: Option<u8>) -> String {
    value.map_or("—".to_owned(), |v| v.to_string())
}

fn ink(value: Option<f32>) -> String {
    value.map_or("—".to_owned(), |v| format!("{}", v.round() as i32))
}

impl AtelierApp {
    /// What the image shows, as a fingerprint (no pixel compared): layers
    /// and their versions, as the canvas composites them.
    pub(super) fn composite_key(&self) -> u64 {
        let Some(doc) = &self.doc else { return 0 };
        let layers = doc.render_layers();
        let opts = crate::compose::Options {
            held: doc.effects_held(),
            later_styles: true,
            ..Default::default()
        };
        let sigs = crate::compose::signatures(&layers, &opts);
        hash_of((doc.uid(), format!("{sigs:?}"), doc.width(), doc.height()))
    }

    fn reading_at(&self, pixel: (i64, i64), radius: i64) -> Option<Reading> {
        let rgb = self.doc.as_ref()?.sampled(pixel, radius)?;
        // (Inks only: what the Color Picker adds about the gamut costs ms.)
        Some(Reading {
            rgb,
            cmyk: self.inks_of(rgb),
        })
    }

    /// The values under the pointer and of the points, computed again only
    /// when the image, the pixel or the settings change. Both average over
    /// the Sample Size.
    fn update_readings(&mut self) {
        let composite = self.composite_key();
        let image = hash_of((composite, self.inks_key()));
        let hover = self.hover_pixel;
        let radius = self.measure.sample.radius();
        let key = hash_of((image, hover, radius));
        if self.info.cursor.as_ref().is_none_or(|c| c.0 != key) {
            let reading = hover.and_then(|p| self.reading_at(p, radius));
            self.info.cursor = Some((key, reading));
        }
        let Some(doc) = &self.doc else { return };
        let spots: Vec<(i64, i64)> = doc.samplers.iter().map(|s| s.pixel()).collect();
        // (Their values are levels only: no inks.)
        let key = hash_of((composite, &spots, radius));
        let now = self.ctx.input(|i| i.time);
        let edited = self.edit_held(&self.ctx);
        let current = self.info.samplers.as_ref().filter(|c| {
            c.0 == key
                // While a stroke goes on: from time to time.
                || (edited
                    && c.1.len() == spots.len()
                    && now - self.info.samplers_at < SAMPLERS_WHILE_HELD)
        });
        if current.is_none() {
            let readings = doc
                .sampled_all(&spots, radius)
                .into_iter()
                .map(|rgb| rgb.map(|rgb| Reading { rgb, cmyk: None }))
                .collect();
            self.info.samplers = Some((key, readings));
            self.info.samplers_at = now;
        }
    }

    /// The readings of the points (panel closed: computed for this call).
    #[cfg(test)]
    pub(super) fn sampler_readings(&mut self) -> Vec<Option<Reading>> {
        self.update_readings();
        self.info
            .samplers
            .as_ref()
            .map_or_else(Vec::new, |s| s.1.clone())
    }

    /// The reading under the pointer.
    #[cfg(test)]
    pub(super) fn cursor_reading(&mut self) -> Option<Reading> {
        self.update_readings();
        self.info.cursor.as_ref().and_then(|c| c.1)
    }

    /// Width and height of the marquee being drawn, else of the selection.
    fn selection_size(&self) -> Option<(u32, u32)> {
        if let Some(r) = self.marquee_preview {
            return Some((r.w, r.h));
        }
        let canvas = self.doc.as_ref()?.full_rect();
        let b = self.selection.as_ref()?.bounds(canvas)?;
        Some((b.w, b.h))
    }

    pub(super) fn info_window(&mut self, ctx: &egui::Context) {
        // (Hidden by Tab or Full Screen: nothing to compute for.)
        if !self.info.open || self.doc.is_none() || !self.chrome_shown() {
            return;
        }
        self.update_readings();
        let reading = self.info.cursor.as_ref().and_then(|c| c.1);
        let samplers = self.info.samplers.as_ref().map_or(&[][..], |s| &s.1);
        let channel = |i: usize| reading.map(|r| r.rgb[i]);
        let cyan = |i: usize| reading.and_then(|r| r.cmyk).map(|c| c[i]);
        let selection = self.selection_size();
        // The line shows with its tool only: its values too.
        let measure = self.measure_values().filter(|_| self.tool == Tool::Ruler);
        let hover = self.hover_pixel;
        let Some(doc) = &self.doc else { return };
        let size = (doc.width(), doc.height());
        // Sized for what it holds now, which comes and goes.
        let measure_rows = measure.as_ref().map_or(0, |v| v.len().div_ceil(2));
        let rows = 4 + 2 + 1 + samplers.len() + measure_rows;
        let lines = 2 + usize::from(!samplers.is_empty()) + usize::from(measure.is_some());
        let height = rows as f32 * ROW + lines as f32 * LINE;
        let pos = dialogs::panel_pos(ctx, "info");
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                resize: Some(height),
                ..DialogSpec::new("info", t!("panels8-info-title"), pos)
            },
            |ui| {
                let scroll = egui::ScrollArea::vertical().auto_shrink([false, false]);
                scroll.show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.spacing_mut().interact_size.y = 18.0;
                    let dash = || "—".to_owned();
                    ui.columns(2, |c| {
                        row(&mut c[0], &t!("panels8-short-r"), &level(channel(0)));
                        row(&mut c[0], &t!("panels8-short-g"), &level(channel(1)));
                        row(&mut c[0], &t!("panels8-short-b"), &level(channel(2)));
                        row(&mut c[1], &t!("panels8-short-c"), &ink(cyan(0)));
                        row(&mut c[1], &t!("panels8-short-m"), &ink(cyan(1)));
                        row(&mut c[1], &t!("panels8-short-y-ink"), &ink(cyan(2)));
                        row(&mut c[1], &t!("panels8-short-k"), &ink(cyan(3)));
                    });
                    ui.separator();
                    ui.columns(2, |c| {
                        let x = hover.map_or_else(dash, |p| p.0.to_string());
                        let y = hover.map_or_else(dash, |p| p.1.to_string());
                        row(&mut c[0], &t!("panels8-short-x"), &x);
                        row(&mut c[0], &t!("panels8-short-y"), &y);
                        let w = selection.map_or_else(dash, |s| s.0.to_string());
                        let h = selection.map_or_else(dash, |s| s.1.to_string());
                        row(&mut c[1], &t!("panels8-short-w"), &w);
                        row(&mut c[1], &t!("panels8-short-h"), &h);
                    });
                    ui.separator();
                    row(
                        ui,
                        &t!("panels8-info-document"),
                        &t!("panels8-info-size", width = size.0, height = size.1),
                    );
                    if !samplers.is_empty() {
                        ui.separator();
                    }
                    for (i, reading) in samplers.iter().enumerate() {
                        let values = match reading {
                            Some(r) => format!("{:>3} {:>3} {:>3}", r.rgb[0], r.rgb[1], r.rgb[2]),
                            None => "—".to_owned(),
                        };
                        row(ui, &format!("#{}", i + 1), &values);
                    }
                    if let Some(values) = &measure {
                        ui.separator();
                        ui.columns(2, |c| {
                            for (i, (name, value)) in values.iter().enumerate() {
                                row(&mut c[i % 2], name, value);
                            }
                        });
                    }
                });
            },
        );
        if out.cancel {
            self.info.open = false;
        }
    }
}
