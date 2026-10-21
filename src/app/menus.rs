//! Menu bar contents, shared by the title bar and the command palette.

use super::keys;
use super::*;
use crate::i18n::{self, Lang, t};

impl AtelierApp {
    /// Photoshop's menus and labels, reduced to what Atelier can do.
    pub(super) fn menus(&self) -> Vec<(String, Vec<MenuEntry>)> {
        use MenuEntry::{Choice, Item, Separator, Submenu};
        let has_doc = self.doc.is_some();
        let has_sel = self.selection.is_some();
        let can_undo = self.doc.as_ref().is_some_and(Document::can_undo);
        let can_redo = self.doc.as_ref().is_some_and(Document::can_redo);
        let can_merge = self
            .doc
            .as_ref()
            .is_some_and(|d| d.active > 0 || d.active_layer().is_group());
        let layer = self.doc.as_ref().map(Document::active_layer);
        let can_reselect = has_doc && self.last_selection.is_some();
        let has_artboards = self.doc.as_ref().is_some_and(Document::has_artboards);
        vec![
            (
                t!("menu-file"),
                vec![
                    Item(
                        t!("artboard-menu-new-document"),
                        true,
                        Some(&new_document::SHORTCUT_NEW),
                        Action::NewDocument,
                    ),
                    Item(
                        t!("menu-file-open"),
                        true,
                        Some(&SHORTCUT_OPEN),
                        Action::Open,
                    ),
                    Item(
                        t!("smart-menu-place-embedded"),
                        true,
                        Some(&SHORTCUT_IMPORT),
                        Action::Import,
                    ),
                    Item(
                        t!("smart-menu-place-linked"),
                        has_doc,
                        None,
                        Action::Smart(smart_ui::Command::PlaceLinked),
                    ),
                    Separator,
                    Item(
                        t!("menu-file-close"),
                        has_doc,
                        Some(&SHORTCUT_CLOSE),
                        Action::CloseDocument,
                    ),
                    Item(
                        t!("menu-file-save"),
                        has_doc,
                        Some(&SHORTCUT_SAVE),
                        Action::Save,
                    ),
                    Item(
                        t!("menu-file-save-as"),
                        has_doc,
                        Some(&SHORTCUT_SAVE_AS),
                        Action::SaveAs,
                    ),
                    Submenu(
                        t!("actions-menu-export"),
                        vec![
                            Item(
                                t!("actions-quick-export"),
                                has_doc,
                                Some(&export_as::SHORTCUT_QUICK_EXPORT),
                                Action::Automate(actions_panel::Command::QuickExport),
                            ),
                            Item(
                                t!("file-menu-export-as"),
                                has_doc,
                                Some(&SHORTCUT_EXPORT_AS),
                                Action::ExportAs,
                            ),
                            Separator,
                            Item(
                                t!("artboard-menu-to-files"),
                                has_artboards,
                                None,
                                Action::Artboard(artboard_tool::Command::ToFiles),
                            ),
                        ],
                    ),
                    Separator,
                    Submenu(
                        t!("actions-menu-automate"),
                        vec![
                            Item(
                                t!("actions-batch-menu"),
                                true,
                                None,
                                Action::Automate(actions_panel::Command::Batch),
                            ),
                            Item(
                                t!("actions-fit-image-menu"),
                                has_doc,
                                None,
                                Action::Automate(actions_panel::Command::FitImage),
                            ),
                        ],
                    ),
                    Submenu(
                        t!("actions-menu-scripts"),
                        vec![Item(
                            t!("actions-processor-menu"),
                            true,
                            None,
                            Action::Automate(actions_panel::Command::ImageProcessor),
                        )],
                    ),
                    Separator,
                    Item(
                        t!("menu-file-quit"),
                        true,
                        Some(&SHORTCUT_QUIT),
                        Action::Quit,
                    ),
                ],
            ),
            (
                t!("menu-edit"),
                vec![
                    Item(
                        t!("menu-edit-undo"),
                        can_undo,
                        Some(&SHORTCUT_UNDO),
                        Action::Undo,
                    ),
                    Item(
                        t!("menu-edit-redo"),
                        can_redo,
                        Some(&SHORTCUT_REDO),
                        Action::Redo,
                    ),
                    Separator,
                    Item(
                        t!("menu-edit-cut"),
                        has_doc && has_sel,
                        Some(&SHORTCUT_CUT),
                        Action::Cut,
                    ),
                    Item(
                        t!("menu-edit-copy"),
                        has_doc,
                        Some(&SHORTCUT_COPY),
                        Action::Copy,
                    ),
                    Item(
                        t!("menu-edit-paste"),
                        true,
                        Some(&SHORTCUT_PASTE),
                        Action::Paste,
                    ),
                    Item(t!("menu-edit-clear"), has_sel, None, Action::ClearSelection),
                    Separator,
                    Item(
                        t!("menu-edit-fill-foreground"),
                        has_doc,
                        Some(&SHORTCUT_FILL_PRIMARY),
                        Action::FillPrimary,
                    ),
                    Item(
                        t!("menu-edit-fill-background"),
                        has_doc,
                        Some(&SHORTCUT_FILL_SECONDARY),
                        Action::FillSecondary,
                    ),
                    Item(
                        t!("menu-edit-fill"),
                        has_doc,
                        Some(&SHORTCUT_FILL_DIALOG),
                        Action::FillDialog,
                    ),
                    Item(
                        t!("retouch-menu-content-aware-fill"),
                        has_doc && has_sel,
                        None,
                        Action::ContentAwareFill,
                    ),
                    Separator,
                    Item(
                        t!("menu-edit-free-transform"),
                        has_doc,
                        Some(&SHORTCUT_FREE_TRANSFORM),
                        Action::Transform(transform::Command::Free),
                    ),
                    Submenu(t!("xform-menu"), self.transform_menu(has_doc)),
                ]
                .into_iter()
                .chain(self.color_edit_entries())
                .chain([
                    Separator,
                    Item(t!("models-menu"), true, None, Action::AiModels),
                    Submenu(
                        t!("uip-menu-preferences"),
                        vec![Item(
                            t!("uip-menu-guides-grid"),
                            true,
                            None,
                            Action::View(rulers::Command::Preferences),
                        )],
                    ),
                    Submenu(
                        t!("menu-edit-language"),
                        Lang::ALL
                            .into_iter()
                            .map(|l| {
                                let name = l.native_name().to_owned();
                                Choice(name, l == i18n::lang(), Action::SetLanguage(l))
                            })
                            .collect(),
                    ),
                ])
                .collect(),
            ),
            (
                t!("menu-image"),
                vec![
                    Submenu(t!("color-menu-mode"), self.mode_menu()),
                    Separator,
                    Submenu(
                        t!("menu-image-adjustments"),
                        ADJUSTMENTS
                            .iter()
                            .flat_map(|a| {
                                // Separator before Vibrance, like Photoshop.
                                let sep = (a.kind == AdjustKind::Vibrance).then_some(Separator);
                                sep.into_iter().chain([Item(
                                    a.label(),
                                    has_doc && !self.smart_filters_apply(),
                                    a.shortcut,
                                    Action::Adjust(a.kind),
                                )])
                            })
                            .collect(),
                    ),
                    Separator,
                    Item(
                        t!("menu-image-image-size"),
                        has_doc,
                        Some(&SHORTCUT_IMAGE_SIZE),
                        Action::ImageSize,
                    ),
                    Item(
                        t!("menu-image-canvas-size"),
                        has_doc,
                        Some(&SHORTCUT_CANVAS_SIZE),
                        Action::CanvasSize,
                    ),
                    Submenu(
                        t!("menu-image-rotation"),
                        vec![
                            Item(
                                t!("menu-image-rotate-180"),
                                has_doc,
                                None,
                                Action::RotateCanvas(180),
                            ),
                            Item(
                                t!("menu-image-rotate-cw"),
                                has_doc,
                                None,
                                Action::RotateCanvas(90),
                            ),
                            Item(
                                t!("menu-image-rotate-ccw"),
                                has_doc,
                                None,
                                Action::RotateCanvas(270),
                            ),
                            Item(
                                t!("menu-image-rotate-arbitrary"),
                                has_doc,
                                None,
                                Action::RotateArbitrary,
                            ),
                            Separator,
                            Item(
                                t!("menu-image-flip-horizontal"),
                                has_doc,
                                None,
                                Action::FlipCanvas(true),
                            ),
                            Item(
                                t!("menu-image-flip-vertical"),
                                has_doc,
                                None,
                                Action::FlipCanvas(false),
                            ),
                        ],
                    ),
                    Separator,
                    Item(
                        t!("menu-image-crop"),
                        self.selection.as_ref().is_some_and(|s| !s.inverted),
                        None,
                        Action::CropToSelection,
                    ),
                    Item(
                        t!("menu-image-crop-to-element"),
                        has_doc,
                        None,
                        Action::CropToElement,
                    ),
                    Item(t!("menu-image-trim"), has_doc, None, Action::CropToContent),
                ],
            ),
            (
                t!("menu-layer"),
                vec![
                    Item(
                        t!("menu-layer-new"),
                        has_doc,
                        Some(&SHORTCUT_NEW_LAYER),
                        Action::NewLayer,
                    ),
                    Item(
                        t!("menu-layer-via-copy"),
                        has_doc,
                        Some(&SHORTCUT_DUPLICATE),
                        Action::LayerFromSelection,
                    ),
                    Item(t!("core-menu-new-group"), has_doc, None, Action::NewGroup),
                    Item(
                        t!("artboard-menu-new"),
                        has_doc,
                        None,
                        Action::Artboard(artboard_tool::Command::New),
                    ),
                    Item(
                        t!("artboard-menu-from-group"),
                        layer.is_some_and(|l| l.is_group() && l.artboard.is_none() && l.depth == 0),
                        None,
                        Action::Artboard(artboard_tool::Command::FromGroup),
                    ),
                    Item(
                        t!("artboard-menu-from-layers"),
                        layer.is_some_and(|l| l.artboard.is_none()),
                        None,
                        Action::Artboard(artboard_tool::Command::FromLayers),
                    ),
                    Item(t!("menu-layer-duplicate"), has_doc, None, Action::Duplicate),
                    Item(t!("menu-layer-delete"), has_doc, None, Action::Delete),
                    Separator,
                    Submenu(t!("core-menu-layer-style"), self.style_menu()),
                    Separator,
                    Submenu(t!("core-menu-new-fill-layer"), self.new_fill_menu()),
                    Submenu(
                        t!("core-menu-new-adjustment-layer"),
                        self.new_adjustment_menu(),
                    ),
                    Submenu(t!("core-menu-layer-mask"), self.mask_menu()),
                    Item(
                        if layer.is_some_and(|l| l.clipped) {
                            t!("core-menu-release-clipping")
                        } else {
                            t!("core-menu-create-clipping")
                        },
                        layer.is_some_and(|l| !l.is_group()),
                        Some(&SHORTCUT_CLIPPING),
                        Action::ToggleClipping,
                    ),
                    Submenu(t!("smart-menu"), self.smart_menu()),
                    Item(
                        t!("vector-menu-rasterize"),
                        layer.is_some_and(Layer::is_live),
                        None,
                        Action::Rasterize,
                    ),
                    Separator,
                    Item(
                        t!("core-menu-group-layers"),
                        has_doc,
                        Some(&SHORTCUT_GROUP),
                        Action::GroupLayers,
                    ),
                    Item(
                        t!("core-menu-ungroup-layers"),
                        layer.is_some_and(Layer::is_group),
                        Some(&SHORTCUT_UNGROUP),
                        Action::Ungroup,
                    ),
                    Item(
                        t!("artboard-menu-ungroup"),
                        layer.is_some_and(|l| l.artboard.is_some()),
                        None,
                        Action::Artboard(artboard_tool::Command::Ungroup),
                    ),
                    Separator,
                    Submenu(
                        t!("menu-layer-arrange"),
                        vec![
                            Item(
                                t!("menu-layer-bring-to-front"),
                                has_doc,
                                Some(&SHORTCUT_TO_FRONT),
                                Action::BringToFront,
                            ),
                            Item(
                                t!("menu-layer-bring-forward"),
                                has_doc,
                                Some(&SHORTCUT_RAISE),
                                Action::Raise,
                            ),
                            Item(
                                t!("menu-layer-send-backward"),
                                has_doc,
                                Some(&SHORTCUT_LOWER),
                                Action::Lower,
                            ),
                            Item(
                                t!("menu-layer-send-to-back"),
                                has_doc,
                                Some(&SHORTCUT_TO_BACK),
                                Action::SendToBack,
                            ),
                        ],
                    ),
                    Item(
                        t!("menu-layer-toggle-lock"),
                        has_doc,
                        None,
                        Action::ToggleLock,
                    ),
                    Separator,
                    Item(
                        if layer.is_some_and(Layer::is_group) {
                            t!("core-menu-merge-group")
                        } else {
                            t!("menu-layer-merge-down")
                        },
                        can_merge,
                        Some(&SHORTCUT_MERGE_DOWN),
                        Action::MergeDown,
                    ),
                    Item(
                        t!("menu-layer-merge-visible"),
                        has_doc,
                        Some(&SHORTCUT_MERGE_VISIBLE),
                        Action::MergeVisible,
                    ),
                    Item(
                        t!("menu-layer-flatten"),
                        has_doc,
                        None,
                        Action::FlattenImage,
                    ),
                    Separator,
                    Item(
                        t!("menu-layer-remove-background"),
                        has_doc && self.subject_job.is_none(),
                        None,
                        Action::RemoveBackground,
                    ),
                    Item(
                        t!("menu-layer-make-transparent"),
                        has_doc,
                        None,
                        Action::MakeTransparent,
                    ),
                    Item(t!("menu-layer-split"), has_doc, None, Action::Split),
                ],
            ),
            (
                t!("menu-select"),
                vec![
                    Item(
                        t!("menu-select-all"),
                        has_doc,
                        Some(&SHORTCUT_SELECT_ALL),
                        Action::SelectAll,
                    ),
                    Item(
                        t!("menu-select-deselect"),
                        has_sel,
                        Some(&SHORTCUT_DESELECT),
                        Action::Deselect,
                    ),
                    Item(
                        t!("menu-select-reselect"),
                        can_reselect,
                        Some(&SHORTCUT_RESELECT),
                        Action::Reselect,
                    ),
                    Item(
                        t!("menu-select-inverse"),
                        has_sel,
                        Some(&SHORTCUT_INVERT_SELECTION),
                        Action::InvertSelection,
                    ),
                    Separator,
                    Item(
                        t!("sel-menu-color-range"),
                        has_doc,
                        None,
                        Action::Select(select_menu::Command::ColorRange),
                    ),
                    Item(
                        t!("menu-select-subject"),
                        has_doc && self.subject_job.is_none(),
                        None,
                        Action::SelectSubject,
                    ),
                ]
                .into_iter()
                .chain(self.select_menu_entries())
                .collect(),
            ),
            self.filter_menu(),
            (
                t!("menu-view"),
                self.color_view_entries()
                    .into_iter()
                    .chain([
                        Item(
                            t!("menu-view-zoom-in"),
                            has_doc,
                            Some(&SHORTCUT_ZOOM_IN),
                            Action::ZoomIn,
                        ),
                        Item(
                            t!("menu-view-zoom-out"),
                            has_doc,
                            Some(&SHORTCUT_ZOOM_OUT),
                            Action::ZoomOut,
                        ),
                        Item(
                            t!("menu-view-fit-on-screen"),
                            has_doc,
                            Some(&SHORTCUT_FIT),
                            Action::Fit,
                        ),
                        Item(
                            t!("zoombar-actual-pixels"),
                            has_doc,
                            Some(&SHORTCUT_ACTUAL),
                            Action::ActualSize,
                        ),
                    ])
                    .chain(self.view_menu_entries())
                    .collect(),
            ),
            (
                t!("uip-menu-window"),
                vec![
                    MenuEntry::Check(
                        t!("actions-title"),
                        true,
                        self.actions.open,
                        Some(&actions_panel::SHORTCUT_ACTIONS),
                        Action::Automate(actions_panel::Command::Panel),
                    ),
                    MenuEntry::Check(
                        t!("xchg-menu-channels"),
                        true,
                        self.channels.open,
                        None,
                        Action::Channels,
                    ),
                    MenuEntry::Check(
                        t!("xchg-menu-history"),
                        true,
                        self.history.open,
                        None,
                        Action::History,
                    ),
                    MenuEntry::Check(
                        t!("vector-menu-paths"),
                        true,
                        self.vector.panel.open,
                        None,
                        Action::Paths,
                    ),
                    MenuEntry::Check(
                        t!("uip-menu-swatches"),
                        true,
                        self.colors.panel_open,
                        None,
                        Action::Swatches,
                    ),
                ],
            ),
        ]
    }

    /// Layer › Layer Mask.
    pub(super) fn mask_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let layer = self.doc.as_ref().map(Document::active_layer);
        let mask = layer.and_then(|l| l.mask.as_ref());
        let (can_add, has) = (layer.is_some() && mask.is_none(), mask.is_some());
        let sel = self.selection.is_some();
        vec![
            Item(
                t!("core-mask-reveal-all"),
                can_add,
                None,
                Action::AddMask(NewMask::RevealAll),
            ),
            Item(
                t!("core-mask-hide-all"),
                can_add,
                None,
                Action::AddMask(NewMask::HideAll),
            ),
            Item(
                t!("core-mask-reveal-selection"),
                can_add && sel,
                None,
                Action::AddMask(NewMask::RevealSelection),
            ),
            Item(
                t!("core-mask-hide-selection"),
                can_add && sel,
                None,
                Action::AddMask(NewMask::HideSelection),
            ),
            Separator,
            Item(t!("core-mask-delete"), has, None, Action::DeleteMask),
            Item(
                t!("core-mask-apply"),
                has && layer.is_some_and(Layer::has_pixels),
                None,
                Action::ApplyMask,
            ),
            Separator,
            Item(
                if mask.is_some_and(|m| !m.enabled) {
                    t!("core-mask-enable")
                } else {
                    t!("core-mask-disable")
                },
                has,
                None,
                Action::ToggleMaskEnabled,
            ),
            Item(
                if mask.is_some_and(|m| !m.linked()) {
                    t!("core-mask-link")
                } else {
                    t!("core-mask-unlink")
                },
                has,
                None,
                Action::ToggleMaskLink,
            ),
        ]
    }

    /// Right-click menu of a layer in the Layers panel.
    pub(super) fn layer_menu(&self) -> Vec<MenuEntry> {
        use MenuEntry::{Item, Separator};
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let n = doc.layers.len();
        vec![
            Item(t!("menu-layer-duplicate"), true, None, Action::Duplicate),
            Item(t!("menu-layer-delete"), n > 1, None, Action::Delete),
            Item(t!("menu-layer-toggle-lock"), true, None, Action::ToggleLock),
            Separator,
            Item(
                format!("{}…", t!("core-blending-options")),
                doc.active_layer().has_pixels(),
                None,
                Action::LayerStyle(None),
            ),
            Item(
                t!("smart-menu-convert"),
                true,
                None,
                Action::Smart(smart_ui::Command::Convert),
            ),
            Item(
                t!("smart-menu-edit-contents"),
                doc.active_layer().is_smart(),
                None,
                Action::Smart(smart_ui::Command::EditContents),
            ),
            Item(
                t!("vector-menu-rasterize"),
                doc.active_layer().is_live(),
                None,
                Action::Rasterize,
            ),
            Item(
                if doc.active_layer().clipped {
                    t!("core-menu-release-clipping")
                } else {
                    t!("core-menu-create-clipping")
                },
                !doc.active_layer().is_group(),
                Some(&SHORTCUT_CLIPPING),
                Action::ToggleClipping,
            ),
            Separator,
            Item(
                t!("menu-layer-merge-down"),
                doc.active > 0,
                Some(&SHORTCUT_MERGE_DOWN),
                Action::MergeDown,
            ),
            Item(
                t!("menu-layer-merge-visible"),
                n > 1,
                Some(&SHORTCUT_MERGE_VISIBLE),
                Action::MergeVisible,
            ),
            Item(t!("menu-layer-flatten"), true, None, Action::FlattenImage),
        ]
    }
}

/// Entries of a menu (and of its submenus); the clicked action goes into `action`.
pub(super) fn menu_entries(ui: &mut egui::Ui, entries: &[MenuEntry], action: &mut Option<Action>) {
    for entry in entries {
        match entry {
            MenuEntry::Separator => {
                ui.separator();
            }
            MenuEntry::Submenu(label, sub) => {
                ui.menu_button(label.as_str(), |ui| menu_entries(ui, sub, action));
            }
            MenuEntry::Item(label, enabled, shortcut, act) => {
                let mut button = egui::Button::new(label.as_str());
                if let Some(s) = shortcut {
                    button = button.shortcut_text(keys::shortcut(ui.ctx(), s));
                }
                if ui.add_enabled(*enabled, button).clicked() {
                    *action = Some(*act);
                }
            }
            MenuEntry::Choice(label, selected, act) => {
                if ui.radio(*selected, label.as_str()).clicked() {
                    *action = Some(*act);
                }
            }
            MenuEntry::Unavailable(label, why) => {
                // Aligned with the options around it.
                let mut text = egui::text::LayoutJob::default();
                let font = egui::TextStyle::Button.resolve(ui.style());
                let format = |color| egui::TextFormat::simple(font.clone(), color);
                text.append(icon::CHECK, 0.0, format(Color32::TRANSPARENT));
                text.append(label, 6.0, format(Color32::PLACEHOLDER));
                let button = egui::Button::new(text);
                ui.add_enabled(false, button)
                    .on_disabled_hover_text(why.as_str());
            }
            MenuEntry::Check(label, enabled, checked, shortcut, act) => {
                // A check mark in front, room kept for it when off.
                let mut text = egui::text::LayoutJob::default();
                let font = egui::TextStyle::Button.resolve(ui.style());
                let mark = if *checked {
                    ui.visuals().text_color()
                } else {
                    Color32::TRANSPARENT
                };
                let format = |color| egui::TextFormat::simple(font.clone(), color);
                text.append(icon::CHECK, 0.0, format(mark));
                text.append(label, 6.0, format(Color32::PLACEHOLDER));
                let mut button = egui::Button::new(text);
                if let Some(s) = shortcut {
                    button = button.shortcut_text(keys::shortcut(ui.ctx(), s));
                }
                if ui.add_enabled(*enabled, button).clicked() {
                    *action = Some(*act);
                }
            }
        }
    }
}
