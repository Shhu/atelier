//! Navigator panel (Window › Navigator): a small image of the document with
//! the part in view; a click there centers the view, the box can be dragged;
//! the zoom below. Made on another thread when the image changes. What the
//! view shows is asked of the canvas's own mapping, whatever it is.

use std::thread::JoinHandle;

use image::RgbaImage;

use super::dialogs::{DialogSpec, floating_dialog};
use super::*;
use crate::compose;
use crate::i18n::t;

/// Largest side of the thumbnail.
const THUMB: u32 = 256;
/// Height of the picture in the panel at first, of what is below it.
const HEIGHT: f32 = 180.0;
const CONTROLS: f32 = 40.0;
const POLL: std::time::Duration = std::time::Duration::from_millis(50);

#[derive(Default)]
pub(super) struct Navigator {
    pub open: bool,
    /// The thumbnail and what it shows.
    shown: Option<(u64, TextureHandle)>,
    job: Option<(u64, JoinHandle<RgbaImage>)>,
    /// The document of the thumbnail, of the one being made: another tab's
    /// image isn't shown in this one's shape.
    shown_doc: u64,
    job_doc: u64,
    /// The thumbnail whose job failed (a panic): not made again until
    /// the image changes.
    failed: Option<u64>,
    settle: histogram_panel::Settle,
    /// A drag of the view box: from the view's center, in image pixels.
    grab: Option<Vec2>,
    /// Where the picture is on screen (for the tests).
    #[cfg(test)]
    picture: Option<Rect>,
}

impl Navigator {
    #[cfg(test)]
    pub(super) fn ready(&self) -> bool {
        self.shown.is_some()
    }

    #[cfg(test)]
    pub(super) fn busy(&self) -> bool {
        self.job.is_some()
    }

    #[cfg(test)]
    pub(super) fn shows_document_of(&self, uid: u64) -> bool {
        self.shown.is_some() && self.shown_doc == uid
    }

    #[cfg(test)]
    pub(super) fn picture(&self) -> Option<Rect> {
        self.picture
    }
}

/// A zoom's place on the slider (0..1), on a log scale.
pub(super) fn slider_of(zoom: f32) -> f32 {
    (zoom.ln() - MIN_ZOOM.ln()) / (MAX_ZOOM.ln() - MIN_ZOOM.ln())
}

pub(super) fn zoom_of(slider: f32) -> f32 {
    (MIN_ZOOM.ln() + slider * (MAX_ZOOM.ln() - MIN_ZOOM.ln())).exp()
}

/// The document at a reduction, then to [`THUMB`].
fn make_thumbnail(layers: &[Layer], canvas: PixelRect, held: bool) -> RgbaImage {
    let mut level = 0;
    while (canvas.w.max(canvas.h) >> (level + 1)) >= THUMB && level < 12 {
        level += 1;
    }
    let mut cache = compose::LevelCache::new(level);
    let options = compose::Options {
        held,
        later_styles: true,
        ..Default::default()
    };
    let plan = compose::plan(layers, canvas, &mut cache, &options);
    let image = plan.render(PixelRect {
        x: 0,
        y: 0,
        w: canvas.w.div_ceil(1 << level),
        h: canvas.h.div_ceil(1 << level),
    });
    let scale = (THUMB as f32 / image.width().max(image.height()) as f32).min(1.0);
    let w = ((image.width() as f32 * scale).round() as u32).max(1);
    let h = ((image.height() as f32 * scale).round() as u32).max(1);
    image::imageops::thumbnail(&image, w, h)
}

/// The view box when its sides are along the picture's, kept within it: its
/// parts beyond would float around the picture as loose lines.
pub(super) fn upright_box(corners: [Pos2; 4], picture: Rect) -> Option<Rect> {
    let [a, b, _, d] = corners;
    let along = |p: Pos2, q: Pos2| (p.x - q.x).abs() < 0.01 || (p.y - q.y).abs() < 0.01;
    if !along(a, b) || !along(a, d) {
        return None;
    }
    let r = Rect::from_points(&corners).intersect(picture);
    Some(if r.is_positive() {
        r
    } else {
        // Wholly off the picture: a line on the side it left by.
        Rect::from_points(&corners.map(|c| picture.clamp(c)))
    })
}

impl AtelierApp {
    /// The thumbnail shows the document as it is now (for the tests).
    #[cfg(test)]
    pub(super) fn navigator_current(&mut self) -> bool {
        let (lut_key, _) = self.color.shown();
        let key = hash_of((self.composite_key(), lut_key));
        self.navigator.shown.as_ref().is_some_and(|s| s.0 == key)
    }

    /// Moves the view so that `point` (image) is at the center of the canvas
    /// area: asked of the mapping, so that it holds for any view.
    pub(super) fn center_view_on(&mut self, point: Pos2) {
        let (Some(view), Some(area)) = (self.last_view, self.rulers.canvas_area()) else {
            return;
        };
        self.pan += self.view_turn.to_upright(area.center()) - view.to_screen(point);
    }

    /// The image point at the center of the canvas area.
    pub(super) fn view_center(&self) -> Option<Pos2> {
        let view = self.last_view?;
        let center = self.rulers.canvas_area()?.center();
        Some(view.to_image(self.view_turn.to_upright(center)))
    }

    fn update_thumbnail(&mut self, ctx: &egui::Context) {
        let Some(doc) = &self.doc else { return };
        let (lut_key, lut) = self.color.shown();
        let key = hash_of((self.composite_key(), lut_key));
        let held = self.edit_held(ctx);
        let nav = &mut self.navigator;
        if let Some((made_for, job)) = nav.job.take_if(|(_, j)| j.is_finished()) {
            match job.join() {
                Ok(mut image) => {
                    color_ui::show_through(lut.as_deref(), image.as_mut(), false);
                    let size = [image.width() as usize, image.height() as usize];
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                    let texture = ctx.load_texture("navigator", color, TextureOptions::LINEAR);
                    nav.shown = Some((made_for, texture));
                    nav.shown_doc = nav.job_doc;
                }
                Err(_) => nav.failed = Some(made_for),
            }
        }
        if nav.shown.as_ref().is_some_and(|s| s.0 == key) || nav.failed == Some(key) {
            return;
        }
        ctx.request_repaint_after(POLL);
        if nav.job.is_some() {
            return;
        }
        let other_doc = nav.shown_doc != doc.uid();
        if !nav.settle.ready(ctx, key, nav.shown.is_none() || other_doc, held) {
            return;
        }
        nav.job_doc = doc.uid();
        let layers = doc.render_layers().to_vec();
        let (canvas, held) = (doc.full_rect(), doc.effects_held());
        let job = std::thread::Builder::new()
            .name("navigator".into())
            .spawn(move || {
                crate::document::watch_styles_only();
                make_thumbnail(&layers, canvas, held)
            });
        if let Ok(job) = job {
            nav.job = Some((key, job));
        }
    }

    pub(super) fn navigator_window(&mut self, ctx: &egui::Context) {
        // (Hidden by Tab or Full Screen: nothing to compute for.)
        if !self.navigator.open || self.doc.is_none() || !self.chrome_shown() {
            self.navigator.grab = None;
            return;
        }
        self.update_thumbnail(ctx);
        let Some(doc) = &self.doc else { return };
        let (w, h) = (doc.width() as f32, doc.height() as f32);
        let nav = &self.navigator;
        let texture = (nav.shown_doc == doc.uid())
            .then(|| nav.shown.as_ref().map(|s| s.1.id()))
            .flatten();
        let (view, area) = (self.last_view, self.rulers.canvas_area());
        let turn = self.view_turn;
        let mut zoom = self.zoom;
        let mut percent = self.zoom * 100.0;
        let mut step = None;
        let mut target = None;
        let mut grab = self.navigator.grab;
        #[cfg(test)]
        let mut picture = None;
        let center = self.view_center();
        let pos = dialogs::panel_pos(ctx, "navigator");
        let out = floating_dialog(
            ctx,
            DialogSpec {
                modal: false,
                footer: false,
                width: 282.0,
                resize: Some(HEIGHT + CONTROLS),
                ..DialogSpec::new("navigator", t!("panels8-navigator-title"), pos)
            },
            |ui| {
                let room = (ui.available_height() - CONTROLS).max(40.0);
                let scale = (ui.available_width() / w).min(room / h);
                let size = vec2(w, h) * scale;
                let (band, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), size.y), Sense::hover());
                let rect = Rect::from_center_size(band.center(), size);
                #[cfg(test)]
                {
                    picture = Some(rect);
                }
                let response = ui.interact(
                    rect,
                    ui.id().with("navigator-image"),
                    Sense::click_and_drag(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Image,
                        true,
                        t!("ui9-navigator-image"),
                    )
                });
                let painter = ui.painter_at(band);
                painter.rect_filled(rect, 0.0, ui_kit::color::BG0);
                if let Some(texture) = texture {
                    let uv = Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0));
                    painter.image(texture, rect, uv, Color32::WHITE);
                }
                let to_thumb = |p: Pos2| rect.min + p.to_vec2() * scale;
                let box_corners = view.zip(area).map(|(view, area)| {
                    [
                        area.left_top(),
                        area.right_top(),
                        area.right_bottom(),
                        area.left_bottom(),
                    ]
                    .map(|c| view.to_image(turn.to_upright(c)))
                });
                if let Some(corners) = box_corners {
                    let stroke = UiStroke::new(1.5, Color32::from_rgb(240, 60, 60));
                    let thumb = corners.map(to_thumb);
                    match upright_box(thumb, rect) {
                        Some(part) => {
                            painter.rect_stroke(part, 0.0, stroke, egui::StrokeKind::Middle);
                        }
                        // Turned: what of its sides is over the picture.
                        None => {
                            ui.painter_at(rect.expand(1.0))
                                .add(egui::Shape::closed_line(thumb.to_vec(), stroke));
                        }
                    }
                }
                let pointer = response.interact_pointer_pos();
                if let Some(p) = pointer {
                    let at = ((p - rect.min) / scale).to_pos2();
                    if grab.is_none() && response.is_pointer_button_down_on() {
                        // Grabbed in the box: it follows from there; else
                        // the view goes to the point.
                        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
                        let pressed = ((origin - rect.min) / scale).to_pos2();
                        let inside = view.zip(area).is_some_and(|(view, area)| {
                            area.contains(turn.to_screen(view.to_screen(pressed)))
                        });
                        grab = Some(match (inside, center) {
                            (true, Some(center)) => pressed - center,
                            _ => Vec2::ZERO,
                        });
                    }
                    // The view's center stays over the image.
                    if let Some(g) = grab {
                        let c = at - g;
                        target = Some(pos2(c.x.clamp(0.0, w), c.y.clamp(0.0, h)));
                    }
                }
                if !response.is_pointer_button_down_on() {
                    grab = None;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let name = t!("panels8-navigator-zoom");
                    let mut slider = slider_of(zoom);
                    if ui_kit::icon_button(ui, icon::MINUS, &t!("menu-view-zoom-out"), true)
                        .clicked()
                    {
                        step = Some(false);
                    }
                    let room = (ui.available_width() - 100.0).max(40.0);
                    ui.spacing_mut().slider_width = room;
                    let moved = ui
                        .add(egui::Slider::new(&mut slider, 0.0..=1.0).show_value(false))
                        .on_hover_text(&name);
                    ui_kit::name_control(
                        ui,
                        moved.id,
                        &name,
                        &i18n::percent((zoom * 100.0).round() as u32),
                    );
                    if moved.changed() {
                        zoom = zoom_of(slider);
                    }
                    if ui_kit::icon_button(ui, icon::PLUS, &t!("menu-view-zoom-in"), true).clicked()
                    {
                        step = Some(true);
                    }
                    let field = ui_kit::number_field(
                        ui,
                        "",
                        &mut percent,
                        MIN_ZOOM * 100.0..=MAX_ZOOM * 100.0,
                        &i18n::percent(""),
                        62.0,
                    );
                    ui_kit::name_control(
                        ui,
                        field.id,
                        &name,
                        &i18n::percent(percent.round() as u32),
                    );
                    if field.changed() {
                        zoom = percent / 100.0;
                    }
                });
            },
        );
        self.navigator.grab = grab;
        #[cfg(test)]
        {
            self.navigator.picture = picture;
        }
        if zoom != self.zoom {
            // Around the center of the view.
            self.set_zoom_around(zoom, Pos2::ZERO, Pos2::ZERO);
        }
        if let Some(zoom_in) = step {
            self.zoom_step(zoom_in, Pos2::ZERO, Pos2::ZERO);
        }
        if let Some(target) = target {
            self.center_view_on(target);
        }
        if out.cancel {
            self.navigator.open = false;
        }
    }
}
