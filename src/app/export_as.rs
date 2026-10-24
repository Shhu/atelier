//! File › Export, like Photoshop's: Export As (Alt+Shift+Ctrl+W) with its
//! preview and the file's size, format and quality, image size (scale,
//! width and height, resampling), metadata (none, or a copyright) and
//! color space (converted to sRGB, profile embedded); Quick Export as PNG
//! (Alt+Shift+Ctrl+'); the Export steps of the actions.

use std::sync::{Arc, mpsc};

use image::RgbaImage;

use super::dialogs::{
    DialogSpec, Track, block, floating_dialog, gap_between_blocks, help, slider_row,
};
use super::*;
use crate::actions::Recorded;
use crate::document::ExportFormat;
use crate::export::ExportOptions;

/// Quick Export as PNG: Alt+Shift+Ctrl+', like Photoshop.
pub(super) const SHORTCUT_QUICK_EXPORT: KeyboardShortcut = KeyboardShortcut::new(
    Modifiers::COMMAND
        .plus(Modifiers::SHIFT)
        .plus(Modifiers::ALT),
    Key::Quote,
);

/// Formats of Export As.
const FORMATS: [ExportFormat; 6] = [
    ExportFormat::Png,
    ExportFormat::Jpeg,
    ExportFormat::Gif,
    ExportFormat::WebP,
    ExportFormat::Tiff,
    ExportFormat::Psd,
];

/// Side of the preview.
const PREVIEW: f32 = 240.0;

/// A preview of the file, and its size in bytes (`None`: not known).
type Preview = (ColorImage, Option<usize>);

/// The Export As window: its settings, the flattened image, the preview
/// of the file and its size, computed in the background.
pub(super) struct ExportDialog {
    pub(super) options: ExportOptions,
    /// Copyright text, kept while Metadata is None.
    copyright: String,
    flat: Option<Arc<RgbaImage>>,
    flat_job: Option<mpsc::Receiver<RgbaImage>>,
    /// Shown: for which settings, the preview, the file's size (`None`:
    /// not known, a PSD).
    shown: Option<(ExportOptions, egui::TextureHandle, Option<usize>)>,
    job: Option<(ExportOptions, mpsc::Receiver<Preview>)>,
}

impl ExportDialog {
    fn new(options: ExportOptions) -> Self {
        Self {
            copyright: options.copyright.clone().unwrap_or_default(),
            options,
            flat: None,
            flat_job: None,
            shown: None,
            job: None,
        }
    }
}

/// The export as it will be written, reduced for the preview, and its
/// size in bytes. `proof`: Use Proof Setup's conversion.
fn preview_of(
    flat: &RgbaImage,
    options: &ExportOptions,
    icc: Option<&[u8]>,
    proof: Option<&crate::cms::Converter>,
    srgb: Option<&crate::cms::Profile>,
) -> Preview {
    // A large export is previewed smaller, its file size estimated from
    // that one's.
    const EXACT: f64 = 4096.0 * 4096.0;
    let area = |o: &ExportOptions| {
        let (w, h) = o.size(flat.dimensions());
        w as f64 * h as f64
    };
    if area(options) > EXACT {
        // A little under, for the rounding of the sides.
        let f = (EXACT / area(options)).sqrt() as f32 * 0.99;
        let reduced = ExportOptions {
            scale: options.scale * f,
            ..options.clone()
        };
        let k = area(options) / area(&reduced);
        let (shown, bytes) = preview_of(flat, &reduced, icc, proof, srgb);
        return (shown, bytes.map(|b| (b as f64 * k) as usize));
    }
    let image = options.render(flat.clone());
    let cmyk = proof.filter(|_| options.use_proof && crate::document::has_cmyk(options.format));
    let (shown, bytes) = match (options.format, cmyk) {
        (_, Some(proof)) => {
            // Shown as printed: through the inks and back.
            let c = proof.conversion();
            let bytes = options.encode_cmyk(&image, &c).ok().map(|b| b.len());
            let back = crate::cms::Conversion {
                from: c.to,
                to: c.from,
                ..c
            };
            let ink = crate::document::to_cmyk(&image, &c).ok();
            let rgb = ink.and_then(|ink| back.cmyk_to_rgb(&ink));
            let (w, h) = image.dimensions();
            let shown = rgb
                .and_then(|rgb| image::RgbImage::from_raw(w, h, rgb))
                .map_or(image, |rgb| {
                    image::DynamicImage::ImageRgb8(rgb).into_rgba8()
                });
            (shown, bytes.filter(|_| options.format != ExportFormat::Psd))
        }
        (ExportFormat::Psd, None) => (image, None),
        _ => {
            let mut image = image;
            let icc = in_srgb(&mut image, srgb).or(icc);
            match options.encode(&image, icc) {
                Ok(bytes) => {
                    let decoded = image::load_from_memory(&bytes).map(|d| d.into_rgba8());
                    (decoded.unwrap_or(image), Some(bytes.len()))
                }
                Err(_) => (image, None),
            }
        }
    };
    // At most twice the preview's side (sharp on high-density screens).
    let side = PREVIEW as u32 * 2;
    let small = if shown.width().max(shown.height()) > side {
        image::DynamicImage::ImageRgba8(shown)
            .thumbnail(side, side)
            .into_rgba8()
    } else {
        shown
    };
    let size = [small.width() as usize, small.height() as usize];
    (
        ColorImage::from_rgba_unmultiplied(size, small.as_raw()),
        bytes,
    )
}

/// "1.2 MB", "350 KB" ("1,2 Mo" in French).
fn file_size(bytes: usize) -> String {
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        t!("words3-size-kb", size = i18n::decimal(kb.max(0.1), 1))
    } else {
        t!("models-size-mb", size = i18n::decimal(kb / 1024.0, 1))
    }
}

impl AtelierApp {
    /// Export As's window, with `options`.
    pub(super) fn open_export(&mut self, options: ExportOptions) {
        if self.doc.is_some() {
            self.export = Some(ExportDialog::new(options));
        }
    }

    pub(super) fn export_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.export.take() else {
            return;
        };
        let Some(doc) = &self.doc else { return };
        let canvas = (doc.width(), doc.height());
        let icc = doc.icc_profile.clone();
        let proof = self.proof_converter();
        let srgb = self.srgb_source(&d.options);
        // The flattened image once, then the file for each change of the
        // settings (the last one wins).
        if d.flat.is_none() && d.flat_job.is_none() {
            let flatten = doc.flatten_later();
            let (tx, rx) = mpsc::channel();
            let ctx2 = ctx.clone();
            std::thread::spawn(move || {
                let _job = crate::diag::job("Export As: flatten");
                let _ = tx.send(flatten());
                ctx2.request_repaint();
            });
            d.flat_job = Some(rx);
        }
        if let Some(rx) = &d.flat_job
            && let Ok(flat) = rx.try_recv()
        {
            d.flat = Some(Arc::new(flat));
            d.flat_job = None;
        }
        if let Some((options, rx)) = &d.job
            && let Ok((image, bytes)) = rx.try_recv()
        {
            let texture = ctx.load_texture("export_preview", image, egui::TextureOptions::LINEAR);
            d.shown = Some((options.clone(), texture, bytes));
            d.job = None;
        }
        let stale = d.shown.as_ref().is_none_or(|(o, ..)| *o != d.options);
        if stale
            && d.job.is_none()
            && let Some(flat) = &d.flat
        {
            let (flat, options) = (flat.clone(), d.options.clone());
            let (tx, rx) = mpsc::channel();
            let ctx2 = ctx.clone();
            let icc = icc.clone();
            let (proof, srgb) = (proof.clone(), srgb.clone());
            std::thread::spawn(move || {
                let _job = crate::diag::job("Export As: preview");
                let icc = icc.as_deref().map(Vec::as_slice);
                let _ = tx.send(preview_of(
                    &flat,
                    &options,
                    icc,
                    proof.as_ref(),
                    srgb.as_ref(),
                ));
                ctx2.request_repaint();
            });
            d.job = Some((d.options.clone(), rx));
        }

        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok: t!("file-export-ok"),
                width: 600.0,
                ..DialogSpec::new("export", t!("file-export-title"), center)
            },
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(PREVIEW);
                        export_preview(ui, &d, canvas);
                    });
                    ui.add_space(16.0);
                    ui.vertical(|ui| {
                        export_settings(ui, &mut d, canvas, proof.as_ref());
                        let skip = &mut self.artboards.export_skip;
                        AtelierApp::export_as_artboards(ui, self.doc.as_ref(), skip);
                    });
                });
            },
        );
        self.export_settings = d.options.clone();
        if !out.ok && !out.cancel {
            self.export = Some(d);
            return;
        }
        if !out.ok {
            return;
        }
        if self.export_as_artboards_files(&d.options) {
            return;
        }
        let format = d.options.format;
        let title = t!("file-export-title");
        let filters = [(format.name(), format.extensions())];
        let (dir, stem) = (self.current_dir(), self.file_stem());
        if let Some(path) = files::ask_save_path(&title, &filters, dir, &stem) {
            self.export_to(&d.options, &path);
        }
    }

    /// Exports the document to `path` and says so; recorded with its folder.
    pub(super) fn export_to(&mut self, options: &ExportOptions, path: &Path) {
        self.commit_pending();
        // As Save As: the other tab's file would no longer be its own.
        if self.tab_of(path).is_some_and(|i| i != self.active_tab) {
            self.status = t!(
                "file-status-open-in-another-tab",
                name = files::file_name(path)
            );
            return;
        }
        let started = std::time::Instant::now();
        let written = self.write_export(options, path);
        crate::diag::record(format!("Export: {:?}", options.format), started.elapsed());
        match written {
            Ok(notes) => {
                let shown = path.display().to_string();
                self.status = if notes.is_empty() {
                    t!("file-status-exported", path = shown)
                } else {
                    let notes = notes.join(" · ");
                    crate::diag::warn(tm!(
                        "xchg-status-exported-simplified",
                        path = files::file_name(path),
                        notes = notes.clone()
                    ));
                    t!(
                        "xchg-status-exported-simplified",
                        path = shown,
                        notes = notes
                    )
                };
                self.record(Recorded::Export {
                    options: options.clone(),
                    folder: path.parent().map(Path::to_path_buf),
                });
            }
            Err(e) => self.report_error(e),
        }
    }

    /// The document as an export file. The document itself is not saved:
    /// closing it still asks. Returns what a PSD could not keep.
    pub(super) fn write_export(
        &self,
        options: &ExportOptions,
        path: &Path,
    ) -> Result<Vec<String>, String> {
        let Some(doc) = &self.doc else {
            return Ok(Vec::new());
        };
        let failed = |e: crate::document::FileError| {
            t!(
                "file-status-export-failed",
                name = files::file_name(path),
                error = e.to_string()
            )
        };
        if options.use_proof
            && options.format == ExportFormat::Psd
            && crate::document::has_cmyk(options.format)
            && let Some(proof) = self.proof_converter()
        {
            let c = proof.conversion();
            return doc
                .export_cmyk(path, ExportFormat::Psd, options.quality, &c)
                .map_err(failed);
        }
        if options.format == ExportFormat::Psd {
            return doc
                .export(path, ExportFormat::Psd, options.quality)
                .map_err(failed);
        }
        let icc = doc.icc_profile.as_deref().map(Vec::as_slice);
        let (proof, srgb) = (self.proof_converter(), self.srgb_source(options));
        let bytes = export_bytes(doc.flatten(), options, icc, proof.as_ref(), srgb.as_ref());
        crate::export::write(path, &bytes.map_err(failed)?).map_err(failed)?;
        Ok(Vec::new())
    }

    /// The document's profile, when Convert to sRGB has to convert from it.
    pub(super) fn srgb_source(&self, options: &ExportOptions) -> Option<crate::cms::Profile> {
        let doc = self.doc.as_ref()?;
        let from = self.color.profile_of(doc);
        (options.srgb && from != *crate::cms::srgb()).then_some(from)
    }

    /// An Export step played: into `folder` (`None`: the document's),
    /// named after the document, never over an existing file.
    pub(super) fn export_into(
        &mut self,
        options: &ExportOptions,
        folder: Option<&Path>,
    ) -> Result<PathBuf, String> {
        let folder = match folder {
            Some(f) => f.to_path_buf(),
            None => self
                .current_dir()
                .ok_or_else(|| t!("actions-error-no-folder"))?,
        };
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let path = unused_path(&folder, &self.file_stem(), options.format.extensions()[0]);
        self.commit_pending();
        self.write_export(options, &path)?;
        self.status = t!("file-status-exported", path = path.display().to_string());
        Ok(path)
    }

    /// Quick Export as PNG: where to is asked, the PNG is the image as it
    /// is, with its profile.
    pub(super) fn quick_export(&mut self) {
        if self.doc.is_none() {
            return;
        }
        let options = ExportOptions::default();
        let title = t!("actions-quick-export");
        let filters = [("PNG", ExportFormat::Png.extensions())];
        let (dir, stem) = (self.current_dir(), self.file_stem());
        if let Some(path) = files::ask_save_path(&title, &filters, dir, &stem) {
            self.export_to(&options, &path);
        }
    }
}

/// The file of an image export (not a PSD): `flat` at its size, in CMYK
/// with Use Proof Setup (`proof`), else converted to sRGB from `srgb`
/// if given; `icc`: the document's profile.
pub(super) fn export_bytes(
    flat: RgbaImage,
    options: &ExportOptions,
    icc: Option<&[u8]>,
    proof: Option<&crate::cms::Converter>,
    srgb: Option<&crate::cms::Profile>,
) -> Result<Vec<u8>, crate::document::FileError> {
    let mut image = options.render(flat);
    match proof.filter(|_| options.use_proof && crate::document::has_cmyk(options.format)) {
        Some(proof) => options.encode_cmyk(&image, &proof.conversion()),
        None => {
            let icc = in_srgb(&mut image, srgb).or(icc);
            options.encode(&image, icc)
        }
    }
}

/// Convert to sRGB: `image` from the profile `from`; then sRGB's profile,
/// to embed.
fn in_srgb(image: &mut RgbaImage, from: Option<&crate::cms::Profile>) -> Option<&'static [u8]> {
    let srgb = crate::cms::srgb();
    let c = crate::cms::Conversion {
        from: from?,
        to: srgb,
        intent: crate::cms::Intent::RelativeColorimetric,
        bpc: true,
    };
    c.convert_rgba(image).then(|| srgb.icc().as_slice())
}

/// `stem.ext` in `folder`, or `stem-2.ext`, `stem-3.ext`… if taken.
pub(super) fn unused_path(folder: &Path, stem: &str, ext: &str) -> PathBuf {
    let mut path = folder.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = folder.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    path
}

/// The preview over a checkerboard, the format, size and dimensions below.
fn export_preview(ui: &mut egui::Ui, d: &ExportDialog, canvas: (u32, u32)) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(PREVIEW), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, ui_kit::color::BG1);
    let busy = d.job.is_some() || d.flat.is_none();
    if let Some((_, texture, _)) = &d.shown {
        let size = texture.size_vec2();
        let k = (PREVIEW / size.x).min(PREVIEW / size.y);
        let shown = Rect::from_center_size(rect.center(), size * k);
        draw_checker(painter, shown);
        painter.image(
            texture.id(),
            shown,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    if busy {
        painter.text(
            rect.right_bottom() - vec2(10.0, 10.0),
            egui::Align2::RIGHT_BOTTOM,
            icon::HOURGLASS_MEDIUM,
            egui::FontId::proportional(14.0),
            ui_kit::color::MUTED,
        );
    }
    let (w, h) = d.options.size(canvas);
    let bytes = d
        .shown
        .as_ref()
        .filter(|(o, ..)| *o == d.options)
        .and_then(|(_, _, b)| *b);
    let size = bytes.map_or_else(|| "—".to_owned(), file_size);
    ui.add_space(6.0);
    ui.vertical_centered(|ui| {
        let line = format!("{} · {size} · {w} × {h} px", d.options.format.name());
        ui.label(egui::RichText::new(line).color(ui_kit::color::MUTED));
    });
}

fn export_settings(
    ui: &mut egui::Ui,
    d: &mut ExportDialog,
    canvas: (u32, u32),
    proof: Option<&crate::cms::Converter>,
) {
    let width = ui.available_width();
    image_settings(ui, &mut d.options, canvas, width);
    gap_between_blocks(ui);
    block(ui, |ui| {
        ui_kit::section(ui, &t!("actions-export-metadata"));
        // GIF and PSD files get none.
        let room = !matches!(d.options.format, ExportFormat::Gif | ExportFormat::Psd);
        if !room {
            help(
                ui,
                &t!(
                    "files3-export-no-metadata",
                    format = d.options.format.name()
                ),
            );
            return;
        }
        let mut copyright = d.options.copyright.is_some();
        let label = |on: bool| {
            if on {
                t!("actions-metadata-copyright")
            } else {
                t!("actions-metadata-none")
            }
        };
        let width = ui.available_width();
        ui_kit::combo(
            ui,
            "export_metadata",
            None,
            &label(copyright),
            width,
            |ui| {
                for on in [false, true] {
                    ui.selectable_value(&mut copyright, on, label(on));
                }
            },
        );
        if copyright {
            let id = egui::Id::new("export_copyright");
            ui_kit::text_field(ui, &t!("actions-copyright"), &mut d.copyright, id);
        }
        d.options.copyright = copyright.then(|| d.copyright.clone());
    });
    gap_between_blocks(ui);
    block(ui, |ui| {
        ui_kit::section(ui, &t!("actions-export-color-space"));
        if crate::document::has_cmyk(d.options.format)
            && let Some(proof) = proof
        {
            let label = t!("color-export-use-proof", name = proof.to.name());
            ui_kit::checkbox(ui, &mut d.options.use_proof, &label);
        }
        let cmyk =
            d.options.use_proof && crate::document::has_cmyk(d.options.format) && proof.is_some();
        ui.add_enabled_ui(!cmyk && d.options.format != ExportFormat::Psd, |ui| {
            ui_kit::checkbox(ui, &mut d.options.srgb, &t!("actions-convert-srgb"));
        });
        ui_kit::checkbox(
            ui,
            &mut d.options.embed_profile,
            &t!("actions-embed-profile"),
        );
    });
}

/// File Settings and Image Size.
fn image_settings(ui: &mut egui::Ui, o: &mut ExportOptions, canvas: (u32, u32), width: f32) {
    block(ui, |ui| {
        ui_kit::section(ui, &t!("actions-export-file-settings"));
        ui_kit::combo(ui, "export_format", None, o.format.name(), width, |ui| {
            for format in FORMATS {
                ui.selectable_value(&mut o.format, format, format.name());
            }
        });
    });
    match o.format {
        ExportFormat::Jpeg => {
            let mut quality = o.quality as f32;
            let label = t!("file-export-quality");
            slider_row(
                ui,
                &label,
                None,
                &mut quality,
                1.0..=100.0,
                "",
                Track::Accent,
                90.0,
            );
            o.quality = quality.round().clamp(1.0, 100.0) as u8;
            help(ui, &t!("file-export-jpeg-hint"));
        }
        ExportFormat::Psd => help(ui, &t!("xchg-export-psd-hint")),
        ExportFormat::Gif => help(ui, &t!("actions-export-gif-hint")),
        _ => help(ui, &t!("file-export-lossless-hint")),
    }
    gap_between_blocks(ui);
    let psd = o.format == ExportFormat::Psd;
    block(ui, |ui| {
        ui_kit::section(ui, &t!("actions-export-image-size"));
        ui.add_enabled_ui(!psd, |ui| {
            let (cw, ch) = canvas;
            let (mut w, mut h) = o.size(canvas);
            let half = (width - 8.0) / 2.0;
            // No side beyond what the format holds (Atelier opens).
            let max = o.format.max_side().min(MAX_DIMENSION);
            let size = 1..=max;
            ui.horizontal(|ui| {
                let (wl, hl) = (t!("optbar-width-short"), t!("optbar-height-short"));
                if ui_kit::number_field(ui, &wl, &mut w, size.clone(), " px", half).changed() {
                    o.scale = w as f32 * 100.0 / cw as f32;
                }
                if ui_kit::number_field(ui, &hl, &mut h, size, " px", half).changed() {
                    o.scale = h as f32 * 100.0 / ch as f32;
                }
            });
            let mut scale = o.scale;
            let label = t!("actions-scale");
            let most = (max as f32 * 100.0 / cw.max(ch).max(1) as f32).min(1000.0);
            let suffix = crate::i18n::percent("");
            if ui_kit::number_field(ui, &label, &mut scale, 1.0..=most, &suffix, width).changed() {
                o.scale = scale;
            }
            ui_kit::combo(
                ui,
                "export_resample",
                Some(&t!("dialog-image-size-resample")),
                &actions_panel::resize_filter(o.resample).label(),
                width,
                |ui| {
                    for f in ResizeFilter::ALL {
                        let r = actions_panel::resample(f);
                        ui.selectable_value(&mut o.resample, r, f.label());
                    }
                },
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Library, Step};
    use egui_kittest::Harness;
    use image::Rgba;

    /// A 300 × 200 PNG opened from a folder of its own.
    fn app(name: &str) -> (Harness<'static, AtelierApp>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("atelier-export-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photo.png");
        RgbaImage::from_fn(300, 200, |x, y| Rgba([x as u8, y as u8, 90, 255]))
            .save(&path)
            .unwrap();
        let file = path.clone();
        let mut h = Harness::builder()
            .with_size([1400.0, 900.0])
            .build_eframe(move |cc| {
                let mut app = AtelierApp::new(cc, None);
                app.recent = crate::recent::Recent::with_paths(Vec::new());
                app.open_path(&file);
                app
            });
        h.run_steps(2);
        (h, dir)
    }

    /// Export As over the file of another tab is refused, as Save As is.
    #[test]
    fn export_never_replaces_the_file_of_another_tab() {
        let (mut h, dir) = app("other-tab");
        let other = dir.join("other.png");
        RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]))
            .save(&other)
            .unwrap();
        let before = std::fs::read(&other).unwrap();
        let app = h.state_mut();
        app.open_path(&other);
        app.switch_tab(0);
        app.export_to(&ExportOptions::default(), &other);
        assert_eq!(std::fs::read(&other).unwrap(), before);
        let refused = t!("file-status-open-in-another-tab", name = "other.png");
        assert_eq!(app.status, refused);
        // Over a file of its own tab: as asked.
        let own = dir.join("photo.png");
        app.export_to(&ExportOptions::default(), &own);
        assert_ne!(
            app.status,
            t!("file-status-open-in-another-tab", name = "photo.png")
        );
    }

    #[test]
    fn export_as_previews_the_file_and_its_size() {
        let (mut h, _) = app("preview");
        h.state_mut().run(Action::ExportAs);
        let d = h.state_mut().export.as_mut().unwrap();
        d.options.format = ExportFormat::Jpeg;
        d.options.scale = 50.0;
        for _ in 0..200 {
            h.run_steps(1);
            let shown = h.state().export.as_ref().and_then(|d| d.shown.as_ref());
            if shown.is_some_and(|(o, ..)| o.scale == 50.0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let d = h.state().export.as_ref().unwrap();
        let (options, texture, bytes) = d.shown.as_ref().expect("a preview");
        assert_eq!(options.size((300, 200)), (150, 100));
        assert_eq!(texture.size(), [150, 100]);
        let image = options.render(h.state().doc.as_ref().unwrap().flatten());
        assert_eq!(*bytes, Some(options.encode(&image, None).unwrap().len()));
        h.key_press(egui::Key::Escape);
        h.run_steps(1);
        assert!(h.state().export.is_none());
        // The settings stay for the session.
        assert_eq!(h.state().export_settings.scale, 50.0);
    }

    #[test]
    fn export_steps_go_beside_the_document_and_never_replace() {
        let (mut h, dir) = app("steps");
        let app = h.state_mut();
        app.actions.library = Library::defaults();
        // Export for Web: fitted (not enlarged), then a JPEG beside it.
        app.play_row(actions_panel::Row::Action(0, 3));
        for _ in 0..50 {
            h.run_steps(1);
        }
        assert_eq!(
            image::image_dimensions(dir.join("photo.jpg")).unwrap(),
            (300, 200)
        );
        let app = h.state_mut();
        app.play_row(actions_panel::Row::Action(0, 3));
        for _ in 0..50 {
            h.run_steps(1);
        }
        assert!(dir.join("photo-2.jpg").exists());
        // Recorded with the folder it went to.
        let app = h.state_mut();
        app.actions.library.sets[0].actions[0].steps.clear();
        app.actions.start_recording((0, 0));
        let options = ExportOptions {
            format: ExportFormat::Gif,
            ..ExportOptions::default()
        };
        app.export_to(&options, &dir.join("out.gif"));
        let steps: &[Step] = &app.actions.library.sets[0].actions[0].steps;
        assert!(matches!(
            &steps[0].command,
            Recorded::Export { options, folder: Some(f) }
                if options.format == ExportFormat::Gif && *f == dir
        ));
        assert_eq!(&std::fs::read(dir.join("out.gif")).unwrap()[..3], b"GIF");
    }
}
