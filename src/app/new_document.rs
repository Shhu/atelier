//! File › New (Ctrl+N), like Photoshop's New Document: a size (presets for
//! phones, tablets, the web and social media), its orientation, artboards
//! or a plain canvas, the background contents.

use super::artboard_tool::{preset_combo, size_fields};
use super::dialogs::{DialogSpec, block, floating_dialog, gap_between_blocks};
use super::*;
use crate::document::ArtboardBackground;

pub(super) const SHORTCUT_NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);

/// What the new document (or its artboard) is filled with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Contents {
    White,
    Black,
    BackgroundColor,
    Transparent,
}

impl Contents {
    const ALL: [Contents; 4] = [
        Contents::White,
        Contents::Black,
        Contents::BackgroundColor,
        Contents::Transparent,
    ];

    fn label(self) -> String {
        match self {
            Contents::White => t!("app-fill-white"),
            Contents::Black => t!("app-fill-black"),
            Contents::BackgroundColor => t!("app-fill-background"),
            Contents::Transparent => t!("dialog-canvas-size-transparent"),
        }
    }
}

/// Settings of the New Document window, kept for the session.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Settings {
    size: (u32, u32),
    pub(super) artboards: bool,
    contents: Contents,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            size: (1920, 1080),
            artboards: false,
            contents: Contents::White,
        }
    }
}

#[derive(Default)]
pub(super) struct NewDocument {
    /// The window, open.
    open: Option<Settings>,
    pub(super) last: Settings,
}

impl NewDocument {
    pub(super) fn is_open(&self) -> bool {
        self.open.is_some()
    }
}

impl AtelierApp {
    /// File › New: the window, with the last settings.
    pub(super) fn new_document(&mut self) {
        self.new_doc.open = Some(self.new_doc.last);
    }

    pub(super) fn new_document_window(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.new_doc.open else {
            return;
        };
        let center = ctx.content_rect().center();
        let out = floating_dialog(
            ctx,
            DialogSpec {
                pivot: egui::Align2::CENTER_CENTER,
                ok: t!("artboard-new-document-create"),
                ..DialogSpec::new("new_document", t!("artboard-new-document-title"), center)
            },
            |ui| {
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("artboard-preset-label"));
                    let width = ui.available_width();
                    if let Some(size) = preset_combo(ui, "new_document_preset", s.size, width) {
                        s.size = size;
                    }
                    size_fields(ui, &mut s.size);
                    ui_kit::checkbox(ui, &mut s.artboards, &t!("artboard-new-document-artboards"));
                });
                gap_between_blocks(ui);
                block(ui, |ui| {
                    ui_kit::field_label(ui, &t!("artboard-new-document-contents"));
                    let width = ui.available_width();
                    let shown = s.contents.label();
                    ui_kit::combo(ui, "new_document_contents", None, &shown, width, |ui| {
                        for c in Contents::ALL {
                            ui.selectable_value(&mut s.contents, c, c.label());
                        }
                    });
                });
            },
        );
        self.new_doc.open = Some(s);
        self.new_doc.last = s;
        if out.ok || out.cancel {
            self.new_doc.open = None;
        }
        if !out.ok {
            return;
        }
        let [r, g, b, _] = self.secondary.to_srgba_unmultiplied();
        let color = match s.contents {
            Contents::White => Some([255; 3]),
            Contents::Black => Some([0; 3]),
            Contents::BackgroundColor => Some([r, g, b]),
            Contents::Transparent => None,
        };
        let (w, h) = s.size;
        let mut doc = if s.artboards {
            let background = match s.contents {
                Contents::White => ArtboardBackground::White,
                Contents::Black => ArtboardBackground::Black,
                Contents::BackgroundColor => ArtboardBackground::Custom([r, g, b]),
                Contents::Transparent => ArtboardBackground::Transparent,
            };
            Document::with_artboard(w, h, background)
        } else {
            let [r, g, b] = color.unwrap_or_default();
            let alpha = if color.is_some() { 255 } else { 0 };
            let image = image::RgbaImage::from_pixel(w, h, image::Rgba([r, g, b, alpha]));
            let mut doc = Document::new(image, None);
            if color.is_none() {
                // Transparent: an ordinary layer, not a background.
                doc.layers[0].name = t!("doc-layer-default-name", number = 1);
            }
            doc
        };
        self.color.tag_new(&mut doc);
        self.commit_pending();
        self.add_tab(doc);
        self.status = t!("artboard-status-new-document", width = w, height = h);
    }
}
