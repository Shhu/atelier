mod actions_panel;
#[cfg(test)]
mod artboard_tests;
mod artboard_tool;
mod batch;
mod camera_raw_ui;
mod canvas;
mod channels_panel;
mod color_picker;
mod color_range;
#[cfg(test)]
mod color_tests;
mod color_ui;
mod dialogs;
mod downloads;
#[cfg(test)]
mod exchange_tests;
mod export_as;
mod files;
mod filters_ui;
mod gradient_editor;
mod history_panel;
#[cfg(test)]
mod input_tests;
mod keys;
mod layer_style;
#[cfg(test)]
mod layers_tests;
mod liquify_ui;
mod menus;
mod models_window;
mod new_document;
mod object_select;
mod options_bar;
#[cfg(test)]
mod paint_tests;
mod paint_tools;
mod palette;
mod paths_panel;
#[cfg(test)]
mod picker_tests;
mod quick_mask;
mod render;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod retouch_tests;
mod retouch_tools;
mod right_panel;
mod rulers;
#[cfg(test)]
mod rulers_tests;
mod select_mask;
mod select_menu;
#[cfg(test)]
mod select_menu_tests;
#[cfg(test)]
mod select_tests;
mod select_tools;
#[cfg(test)]
mod smart_tests;
mod smart_ui;
mod status_bar;
mod tabs;
mod titlebar;
mod toolbar;
mod transform;
#[cfg(test)]
mod transform_tests;
#[cfg(test)]
mod type_tests;
mod type_tool;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod vector_tests;
mod vector_tools;
mod welcome;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{
    self, Color32, ColorImage, CursorIcon, Key, KeyboardShortcut, Modifiers, Painter,
    PointerButton, Pos2, Rect, Sense, Stroke as UiStroke, StrokeKind, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use image::imageops::FilterType;

use crate::compose::styles::{Effects, Style};
use crate::document::{
    AlignOp, BlendMode, Document, Fill, Layer, LayerKind, MergeError, NewMask, PixelRect,
    Placement, Selection, intersect,
};
use crate::elements::group_pixels;
use crate::filters::FilterKind;
use crate::handles::{self, Handle, RectDrag, handle_positions, hit_test, round_rect};
use crate::i18n::{self, Lang, t};
use crate::paint::{BrushSettings, DirtyRect, Stroke};
use crate::pixel_ops::{
    AdjustKind, Adjustment, BrightnessContrast, Curve, Histograms, HueSaturation, Levels,
    LevelsChannel,
};
use crate::rotate::rotated_size;
use crate::scissors::{self, EdgeMap};
use crate::selection::SelectMode;
use crate::snap::{Snap, SnapTargets, snap};
use crate::text::TextEngine;
use crate::ui_kit::{self, Kind, icon};
use render::ShownPreview;

const MIN_ZOOM: f32 = 0.02;
const MAX_ZOOM: f32 = 64.0;
use crate::document::MAX_DIMENSION;

/// Distance (screen px) below which a moved element snaps to a guide.
const SNAP_DISTANCE: f32 = 6.0;

/// Distance (screen px) between the top of the frame and the rotation handle.
const ROTATE_HANDLE_DISTANCE: f32 = 24.0;
/// Rotation snapping step with Shift.
const ROTATE_SNAP: f32 = 15.0;

/// Opacity of what overflows the canvas, for the element being manipulated.
const GHOST_ALPHA: f32 = 0.25;

/// Sharp pixels when enlarged, smoothed when reduced.
const IMAGE_TEX_OPTIONS: TextureOptions = TextureOptions {
    magnification: egui::TextureFilter::Nearest,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: None,
};

const SHORTCUT_OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
const SHORTCUT_IMPORT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::O);
const SHORTCUT_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SHORTCUT_SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
// Export As: Alt+Shift+Ctrl+W, like Photoshop.
const SHORTCUT_EXPORT_AS: KeyboardShortcut = KeyboardShortcut::new(
    Modifiers::COMMAND
        .plus(Modifiers::SHIFT)
        .plus(Modifiers::ALT),
    Key::W,
);
const SHORTCUT_UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const SHORTCUT_REDO: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Z);
const SHORTCUT_QUIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);
const SHORTCUT_CLOSE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
// Free Transform: Ctrl+T, like Photoshop.
const SHORTCUT_FREE_TRANSFORM: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::T);
// Display only: egui-winit passes it on as a copy (see `clipboard_action`).
const SHORTCUT_CANVAS_SIZE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::C);
// Fill: Alt+Delete (foreground) and Ctrl+Delete (background), like Photoshop
// on Windows; Backspace works too, for all the fills.
const SHORTCUT_FILL_PRIMARY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::ALT, Key::Delete);
// With Shift: preserving transparency (Photoshop: Shift+Alt / Ctrl+Shift+Backspace).
const SHORTCUT_FILL_PRIMARY_KEEP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::ALT.plus(Modifiers::SHIFT), Key::Backspace);
const SHORTCUT_FILL_SECONDARY_KEEP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Backspace);
const SHORTCUT_FILL_DIALOG: KeyboardShortcut = KeyboardShortcut::new(Modifiers::SHIFT, Key::F5);
const SHORTCUT_FILL_DIALOG_ALT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::SHIFT, Key::Backspace);
const SHORTCUT_INVERT_SELECTION: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::I);
const SHORTCUT_IMAGE_SIZE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::I);
const SHORTCUT_FILL_SECONDARY: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Delete);
const SHORTCUT_COPY: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::C);
const SHORTCUT_PASTE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::V);
const SHORTCUT_CUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::X);
const SHORTCUT_SELECT_ALL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::A);
const SHORTCUT_DESELECT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::D);
const SHORTCUT_RESELECT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::D);
const SHORTCUT_COLOR_BALANCE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::B);
const SHORTCUT_LEVELS: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::L);
const SHORTCUT_CURVES: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::M);
const SHORTCUT_HUE_SATURATION: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::U);
const SHORTCUT_DUPLICATE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::J);
const SHORTCUT_NEW_LAYER: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::N);
const SHORTCUT_MERGE_DOWN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::E);
const SHORTCUT_MERGE_VISIBLE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::E);
// Layer order: Ctrl+[ ] one step, with Shift all the way down / up.
// (Display only: the brackets are read by `bracket_press`.)
const SHORTCUT_RAISE: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::CloseBracket);
const SHORTCUT_LOWER: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::OpenBracket);
const SHORTCUT_TO_FRONT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::CloseBracket);
const SHORTCUT_TO_BACK: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::OpenBracket);
const SHORTCUT_ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Plus);
// Ctrl+=: the + key without Shift on a QWERTY keyboard.
const SHORTCUT_ZOOM_IN_ALT: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::Equals);
const SHORTCUT_ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);
const SHORTCUT_FIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0);
const SHORTCUT_ACTUAL: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Num1);
// Layers, like Photoshop.
const SHORTCUT_GROUP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::G);
const SHORTCUT_UNGROUP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::G);
const SHORTCUT_CLIPPING: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), Key::G);
const SHORTCUT_INVERT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::I);
const SHORTCUT_BLACK_WHITE: KeyboardShortcut = KeyboardShortcut::new(
    Modifiers::COMMAND
        .plus(Modifiers::SHIFT)
        .plus(Modifiers::ALT),
    Key::B,
);

/// Shortcuts that also work while typing in a text field.
const FILE_SHORTCUTS: [(KeyboardShortcut, Action); 7] = [
    (SHORTCUT_EXPORT_AS, Action::ExportAs),
    (SHORTCUT_SAVE_AS, Action::SaveAs),
    (SHORTCUT_SAVE, Action::Save),
    (SHORTCUT_IMPORT, Action::Import),
    (new_document::SHORTCUT_NEW, Action::NewDocument),
    (SHORTCUT_OPEN, Action::Open),
    (SHORTCUT_QUIT, Action::Quit),
];

/// Zoom shortcuts (they also work while a lasso is drawn). "+" is typed
/// with Shift on many keyboards.
const ZOOM_SHORTCUTS: [(KeyboardShortcut, Action); 6] = [
    (SHORTCUT_ZOOM_IN, Action::ZoomIn),
    (
        KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::Plus),
        Action::ZoomIn,
    ),
    (SHORTCUT_ZOOM_IN_ALT, Action::ZoomIn),
    (SHORTCUT_ZOOM_OUT, Action::ZoomOut),
    (SHORTCUT_FIT, Action::Fit),
    (SHORTCUT_ACTUAL, Action::ActualSize),
];

/// Keyboard zoom levels, Photoshop's.
const ZOOM_LEVELS: [f32; 22] = [
    0.0313, 0.0417, 0.0625, 0.0833, 0.125, 0.1667, 0.25, 0.3333, 0.5, 0.6667, 1.0, 2.0, 3.0, 4.0,
    5.0, 6.0, 7.0, 8.0, 12.0, 16.0, 32.0, 64.0,
];
/// Delay (s) to type two opacity digits in a row (4 then 5: 45%).
const DIGIT_DELAY: f64 = 0.6;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    /// Artboard tool (with the Move tool: V).
    Artboard,
    Brush,
    Eraser,
    Gradient,
    Dodge,
    Burn,
    Sponge,
    Blur,
    Sharpen,
    Smudge,
    CloneStamp,
    SpotHealing,
    HealingBrush,
    Patch,
    RedEye,
    Canvas,
    Transform,
    Text,
    Bucket,
    Eyedropper,
    Select,
    Ellipse,
    FreeLasso,
    PolyLasso,
    Lasso,
    ObjectSelect,
    MagicWand,
    Hand,
    Zoom,
    /// Pen, Freeform Pen, Add / Delete Anchor Point, Convert Point (P).
    Pen(vector_tools::PenTool),
    /// Path Selection and Direct Selection (A).
    PathSelect,
    DirectSelect,
    /// Rectangle, Ellipse, Triangle, Polygon, Line, Custom Shape (U).
    Shape(vector_tools::ShapeTool),
}

impl Tool {
    const ALL: [Tool; 42] = [
        Tool::Transform,
        Tool::Artboard,
        Tool::Select,
        Tool::Ellipse,
        Tool::FreeLasso,
        Tool::PolyLasso,
        Tool::Lasso,
        Tool::ObjectSelect,
        Tool::MagicWand,
        Tool::SpotHealing,
        Tool::HealingBrush,
        Tool::Patch,
        Tool::RedEye,
        Tool::Brush,
        Tool::CloneStamp,
        Tool::Eraser,
        Tool::Gradient,
        Tool::Bucket,
        Tool::Blur,
        Tool::Sharpen,
        Tool::Smudge,
        Tool::Dodge,
        Tool::Burn,
        Tool::Sponge,
        Tool::Eyedropper,
        Tool::Pen(vector_tools::PenTool::Pen),
        Tool::Pen(vector_tools::PenTool::Freeform),
        Tool::Pen(vector_tools::PenTool::AddAnchor),
        Tool::Pen(vector_tools::PenTool::DeleteAnchor),
        Tool::Pen(vector_tools::PenTool::Convert),
        Tool::Text,
        Tool::PathSelect,
        Tool::DirectSelect,
        Tool::Shape(vector_tools::ShapeTool::Rectangle),
        Tool::Shape(vector_tools::ShapeTool::Ellipse),
        Tool::Shape(vector_tools::ShapeTool::Triangle),
        Tool::Shape(vector_tools::ShapeTool::Polygon),
        Tool::Shape(vector_tools::ShapeTool::Line),
        Tool::Shape(vector_tools::ShapeTool::Custom),
        Tool::Canvas,
        Tool::Hand,
        Tool::Zoom,
    ];

    /// The tool bar's slots, like Photoshop's: one per group, showing the
    /// group's last used tool; the others are in the slot's menu and
    /// cycle with Shift + the group's key.
    const GROUPS: [&'static [Tool]; 19] = [
        &[Tool::Transform, Tool::Artboard],
        &[Tool::Select, Tool::Ellipse],
        &[Tool::FreeLasso, Tool::PolyLasso, Tool::Lasso],
        &[Tool::ObjectSelect, Tool::MagicWand],
        &[Tool::Canvas],
        &[Tool::Eyedropper],
        &[
            Tool::SpotHealing,
            Tool::HealingBrush,
            Tool::Patch,
            Tool::RedEye,
        ],
        &[Tool::Brush],
        &[Tool::CloneStamp],
        &[Tool::Eraser],
        &[Tool::Gradient, Tool::Bucket],
        &[Tool::Blur, Tool::Sharpen, Tool::Smudge],
        &[Tool::Dodge, Tool::Burn, Tool::Sponge],
        &[
            Tool::Pen(vector_tools::PenTool::Pen),
            Tool::Pen(vector_tools::PenTool::Freeform),
            Tool::Pen(vector_tools::PenTool::AddAnchor),
            Tool::Pen(vector_tools::PenTool::DeleteAnchor),
            Tool::Pen(vector_tools::PenTool::Convert),
        ],
        &[Tool::Text],
        &[Tool::PathSelect, Tool::DirectSelect],
        &[
            Tool::Shape(vector_tools::ShapeTool::Rectangle),
            Tool::Shape(vector_tools::ShapeTool::Ellipse),
            Tool::Shape(vector_tools::ShapeTool::Triangle),
            Tool::Shape(vector_tools::ShapeTool::Polygon),
            Tool::Shape(vector_tools::ShapeTool::Line),
            Tool::Shape(vector_tools::ShapeTool::Custom),
        ],
        &[Tool::Hand],
        &[Tool::Zoom],
    ];

    /// Index of the tool's group in [`Tool::GROUPS`].
    fn group(self) -> usize {
        Tool::GROUPS
            .iter()
            .position(|g| g.contains(&self))
            .expect("every tool is in a group")
    }

    fn glyph(self) -> &'static str {
        match self {
            Tool::Transform => icon::ARROWS_OUT_CARDINAL,
            Tool::Artboard => icon::FRAME_CORNERS,
            Tool::Brush => icon::PAINT_BRUSH,
            Tool::Eraser => icon::ERASER,
            Tool::Gradient => icon::GRADIENT,
            Tool::Dodge => icon::SUN_DIM,
            Tool::Burn => icon::FIRE,
            Tool::Sponge => icon::DOTS_NINE,
            Tool::Blur => icon::DROP,
            Tool::Sharpen => icon::TRIANGLE,
            Tool::Smudge => icon::HAND_POINTING,
            Tool::CloneStamp => icon::STAMP,
            Tool::SpotHealing => icon::BANDAIDS,
            Tool::HealingBrush => icon::FIRST_AID,
            Tool::Patch => icon::PUZZLE_PIECE,
            Tool::RedEye => icon::EYE,
            Tool::Text => icon::TEXT_T,
            Tool::Canvas => icon::CROP,
            Tool::Bucket => icon::PAINT_BUCKET,
            Tool::Eyedropper => icon::EYEDROPPER,
            Tool::Select => icon::SELECTION,
            Tool::Ellipse => icon::CIRCLE_DASHED,
            Tool::FreeLasso => icon::LASSO,
            Tool::PolyLasso => icon::POLYGON,
            Tool::Lasso => icon::MAGNET,
            Tool::ObjectSelect => icon::CUBE_FOCUS,
            Tool::MagicWand => icon::MAGIC_WAND,
            Tool::Hand => icon::HAND,
            Tool::Zoom => icon::MAGNIFYING_GLASS,
            Tool::Pen(p) => p.glyph(),
            Tool::PathSelect => icon::NAVIGATION_ARROW,
            Tool::DirectSelect => icon::CURSOR,
            Tool::Shape(s) => s.glyph(),
        }
    }

    fn name(self) -> String {
        match self {
            Tool::Transform => t!("tool-transform"),
            Tool::Artboard => t!("artboard-tool"),
            Tool::Brush => t!("tool-brush"),
            Tool::Eraser => t!("tool-eraser"),
            Tool::Gradient => t!("paint-tool-gradient"),
            Tool::Dodge => t!("paint-tool-dodge"),
            Tool::Burn => t!("paint-tool-burn"),
            Tool::Sponge => t!("paint-tool-sponge"),
            Tool::Blur => t!("paint-tool-blur"),
            Tool::Sharpen => t!("paint-tool-sharpen"),
            Tool::Smudge => t!("paint-tool-smudge"),
            Tool::CloneStamp => t!("retouch-tool-clone-stamp"),
            Tool::SpotHealing => t!("retouch-tool-spot-healing"),
            Tool::HealingBrush => t!("retouch-tool-healing"),
            Tool::Patch => t!("retouch-tool-patch"),
            Tool::RedEye => t!("retouch-tool-red-eye"),
            Tool::Text => t!("tool-text"),
            Tool::Canvas => t!("tool-canvas"),
            Tool::Bucket => t!("tool-bucket"),
            Tool::Eyedropper => t!("tool-eyedropper"),
            Tool::Select => t!("tool-select"),
            Tool::Ellipse => t!("sel-tool-ellipse"),
            Tool::FreeLasso => t!("sel-tool-lasso"),
            Tool::PolyLasso => t!("sel-tool-polygonal-lasso"),
            Tool::Lasso => t!("tool-lasso"),
            Tool::ObjectSelect => t!("objsel-tool"),
            Tool::MagicWand => t!("tool-magic-wand"),
            Tool::Hand => t!("tool-hand"),
            Tool::Zoom => t!("tool-zoom"),
            Tool::Pen(p) => p.name(),
            Tool::PathSelect => t!("vector-tool-path-selection"),
            Tool::DirectSelect => t!("vector-tool-direct-selection"),
            Tool::Shape(s) => s.name(),
        }
    }

    /// Keyboard shortcut of the tool (like Photoshop): tools sharing a key
    /// form a group (see `tool_for_key`). Blur, Sharpen and Smudge have none.
    fn shortcut(self) -> Option<Key> {
        Some(match self {
            Tool::Transform | Tool::Artboard => Key::V,
            Tool::Brush => Key::B,
            Tool::CloneStamp => Key::S,
            Tool::SpotHealing | Tool::HealingBrush | Tool::Patch | Tool::RedEye => Key::J,
            Tool::Eraser => Key::E,
            Tool::Text => Key::T,
            Tool::Canvas => Key::C,
            Tool::Gradient | Tool::Bucket => Key::G,
            Tool::Dodge | Tool::Burn | Tool::Sponge => Key::O,
            Tool::Blur | Tool::Sharpen | Tool::Smudge => return None,
            Tool::Eyedropper => Key::I,
            Tool::Select | Tool::Ellipse => Key::M,
            Tool::FreeLasso | Tool::PolyLasso | Tool::Lasso => Key::L,
            Tool::ObjectSelect | Tool::MagicWand => Key::W,
            Tool::Hand => Key::H,
            Tool::Zoom => Key::Z,
            Tool::Pen(vector_tools::PenTool::Pen | vector_tools::PenTool::Freeform) => Key::P,
            Tool::Pen(_) => return None,
            Tool::PathSelect | Tool::DirectSelect => Key::A,
            Tool::Shape(_) => Key::U,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResizeFilter {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos,
}

impl ResizeFilter {
    const ALL: [ResizeFilter; 4] = [
        ResizeFilter::Nearest,
        ResizeFilter::Bilinear,
        ResizeFilter::Bicubic,
        ResizeFilter::Lanczos,
    ];

    fn label(self) -> String {
        match self {
            ResizeFilter::Nearest => t!("app-resample-nearest"),
            ResizeFilter::Bilinear => t!("app-resample-bilinear"),
            ResizeFilter::Bicubic => t!("app-resample-bicubic"),
            ResizeFilter::Lanczos => t!("app-resample-lanczos"),
        }
    }

    fn filter(self) -> FilterType {
        match self {
            ResizeFilter::Nearest => FilterType::Nearest,
            ResizeFilter::Bilinear => FilterType::Triangle,
            ResizeFilter::Bicubic => FilterType::CatmullRom,
            ResizeFilter::Lanczos => FilterType::Lanczos3,
        }
    }
}

/// Direct move of layers with the mouse.
#[derive(Clone)]
struct LayerDrag {
    /// Layers moved (the grabbed one first) and their offsets at the start.
    layers: Vec<(usize, (i64, i64))>,
    start: Pos2,
    /// Box of the visible pixels at the start (for snapping).
    start_bounds: Rect,
    /// History is recorded only on the first real move.
    moved: bool,
}

/// Conversion between screen coordinates and canvas coordinates.
#[derive(Clone, Copy)]
struct View {
    origin: Pos2,
    zoom: f32,
}

impl View {
    fn to_image(self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }

    fn to_screen(self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    fn rect_to_screen(self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }
}

#[derive(Clone, Copy)]
enum Action {
    Open,
    Import,
    /// Close the document (asking to save it first if modified).
    CloseDocument,
    Quit,
    Save,
    SaveAs,
    ExportAs,
    Undo,
    Redo,
    Tool(Tool),
    NewLayer,
    Duplicate,
    MergeDown,
    MergeVisible,
    Delete,
    Raise,
    Lower,
    Split,
    SelectSubject,
    RemoveBackground,
    CropToElement,
    CropToContent,
    CenterElement,
    BringToFront,
    SendToBack,
    ToggleLock,
    MakeTransparent,
    Adjust(AdjustKind),
    Copy,
    Cut,
    Paste,
    SelectAll,
    Deselect,
    Reselect,
    InvertSelection,
    FillDialog,
    /// Edit › Content-Aware Fill…
    ContentAwareFill,
    /// Fill preserving transparency.
    FillPrimaryKeep,
    FillSecondaryKeep,
    ImageSize,
    ClearSelection,
    LayerFromSelection,
    CropToSelection,
    Align(AlignOp),
    FillPrimary,
    FillSecondary,
    Fit,
    ActualSize,
    ZoomIn,
    ZoomOut,
    SetLanguage(Lang),
    /// AI Models window.
    AiModels,
    FlattenImage,
    CanvasSize,
    /// Image Rotation by a quarter or half turn (degrees, clockwise).
    RotateCanvas(u16),
    RotateArbitrary,
    /// Flip Canvas: horizontal (`true`) or vertical.
    FlipCanvas(bool),
    /// A filter of the Filter menu (its dialog, if it has settings).
    Filter(FilterKind),
    /// Last Filter (Alt+Ctrl+F).
    LastFilter,
    /// Filter › Camera Raw Filter…
    CameraRawFilter,
    /// Filter › Liquify… (Shift+Ctrl+X).
    Liquify,
    /// Rulers, guides, grid and snapping (View menu).
    View(rulers::Command),
    /// Window › Swatches.
    Swatches,
    /// Window › History.
    History,
    /// Window › Channels.
    Channels,
    /// Select menu: Color Range, Select and Mask, Modify…
    Select(select_menu::Command),
    /// Layer › New › Group (empty), Group Layers (Ctrl+G), Ungroup Layers.
    NewGroup,
    GroupLayers,
    Ungroup,
    /// Create / Release Clipping Mask (Alt+Ctrl+G).
    ToggleClipping,
    /// Layer › New Adjustment Layer.
    NewAdjustmentLayer(AdjustKind),
    /// Layer › New Fill Layer: solid color (`false`) or gradient.
    NewFillLayer(bool),
    /// Layer › Layer Mask.
    AddMask(NewMask),
    DeleteMask,
    ApplyMask,
    ToggleMaskEnabled,
    ToggleMaskLink,
    InvertMask,
    /// Load the mask as the selection.
    MaskSelection,
    /// Layer Style window, at a style (Blending Options without).
    LayerStyle(Option<Style>),
    ClearLayerStyle,
    /// Link or unlink the selected layers.
    LinkLayers,
    /// Next or previous blend mode (Shift++ / Shift+-), or one by its letter
    /// (Alt+Shift+letter).
    BlendStep(bool),
    SetBlend(BlendMode),
    /// Edit › Free Transform and Edit › Transform.
    Transform(transform::Command),
    /// Window › Paths.
    Paths,
    /// A command of the Paths panel; `true`: its dialog first (Alt).
    Path(paths_panel::PathCommand, bool),
    /// Layer › Rasterize: the active text or shape becomes pixels.
    Rasterize,
    /// File › Automate and Scripts, Window › Actions, Quick Export.
    Automate(actions_panel::Command),
    /// Layer › Smart Objects, Place Linked, the smart filters.
    Smart(smart_ui::Command),
    /// File › New…
    NewDocument,
    /// Artboards: Layer › New › Artboard…, Ungroup Artboards, Artboards to
    /// Files, zoom to one.
    Artboard(artboard_tool::Command),
    /// Color Settings, profiles, Proof Setup, Proof Colors, Image › Mode.
    Color(color_ui::Command),
}

pub struct AtelierApp {
    /// egui context (for window commands outside of drawing).
    ctx: egui::Context,
    /// Active document; the other tabs are set aside in `parked`.
    doc: Option<Document>,
    parked: Vec<tabs::ParkedDoc>,
    /// Position of the active document among the tabs.
    active_tab: usize,
    /// Thumbnails of the Layers panel: (layer id, mask) → fingerprint of
    /// what they show, texture.
    thumbs: HashMap<(u64, bool), (u64, f64, TextureHandle)>,
    /// Area of the active layer changed by the brush, to composite again.
    stroke_dirty: Option<DirtyRect>,

    tool: Tool,
    /// Foreground color (brush, text, fill).
    primary: Color32,
    /// Background color (Ctrl+Delete).
    secondary: Color32,
    /// Color Picker, swatches and recent colors.
    colors: color_picker::Colors,
    /// Rulers, guides and grid.
    rulers: rulers::Rulers,
    /// Selection options, windows and modes of the Select menu.
    sel: select_menu::SelState,
    brush_size: f32,
    /// Layers selected in the list (ids). Ignored if it does not contain
    /// the active layer: the selection is then just that layer.
    layer_set: Vec<u64>,
    /// Rectangular selection (canvas coordinates).
    selection: Option<Selection>,
    /// Starting point of the selection being drawn.
    marquee: Option<select_tools::Marquee>,
    /// Rectangle being drawn when it adds to the selection (or subtracts from it).
    marquee_preview: Option<PixelRect>,
    /// Mode of the selection tools (Shift, Alt and Shift+Alt override it).
    select_mode: SelectMode,
    /// Magic wand: tolerance (0..1), contiguous, sample all layers.
    wand_tolerance: f32,
    wand_contiguous: bool,
    wand_all_layers: bool,
    /// Canvas view at the last frame.
    last_view: Option<View>,
    /// Magnetic lasso in progress, detection width (px), frequency (0..100).
    lasso: Option<select_tools::Lasso>,
    lasso_width: f32,
    lasso_frequency: f32,
    /// Image edges for the lasso, per state of the layers.
    edges: select_tools::LassoEdges,
    /// Model downloads in progress.
    downloads: downloads::Downloads,
    /// Subject search in progress.
    subject_job: Option<select_tools::SubjectJob>,
    /// Question before the first download of the AI model.
    model_prompt: Option<models_window::ModelPrompt>,
    /// What waits for a model being downloaded.
    after_download: Vec<models_window::AfterDownload>,
    /// AI Models window open.
    models_window: Option<models_window::ModelsWindow>,
    /// Object Selection tool: analysis, preview, selections in progress.
    object_select: object_select::ObjectSelect,
    /// Last tool used of each group of tools sharing a shortcut key.
    group_tool: HashMap<usize, Tool>,
    /// Fill… window open (Shift+F5).
    fill_dialog: Option<FillDialog>,
    /// Image Size window open (Ctrl+Alt+I).
    image_size: Option<ImageSizeDialog>,
    /// Canvas Size window open (Ctrl+Alt+C).
    canvas_size: Option<CanvasSizeDialog>,
    /// Image Rotation > Arbitrary window open.
    rotate_dialog: Option<RotateDialog>,
    /// Outline of the displayed selection (selection, canvas, segments).
    selection_outline: Option<render::Outline>,
    /// Selection move in progress (outline, or pixels with Ctrl).
    select_move: Option<SelectMove>,
    /// Last dropped selection (Reselect, Ctrl+Shift+D).
    last_selection: Option<Selection>,
    /// End of the last brush stroke (canvas coordinates): Shift + click draws
    /// a straight line from this point.
    last_paint: Option<Pos2>,
    /// Opacity digit just typed, and when (for 4 then 5: 45%).
    last_digit: Option<(u32, f64)>,
    /// Give focus to the layer Name field (double-click in the list).
    focus_name: bool,
    /// A text field had the keyboard at the end of the last frame (an Esc
    /// that has just taken its focus away is still its own).
    typing: bool,
    /// V is held as a plain key, not as Ctrl+V (see `paste_released`).
    plain_v: bool,
    /// A Shift+Ctrl cut came without its key (see `shift_ctrl_cut`).
    cut_key: bool,
    /// Paint bucket tolerance (0..1).
    bucket_tolerance: f32,
    /// Paint bucket: only the area in one piece ("Contiguous").
    bucket_contiguous: bool,
    hardness: f32,
    opacity: f32,
    stroke: Option<Stroke>,
    /// Brush engine settings, the other painting tools, the Gradient tool.
    paint: paint_tools::PaintTools,
    /// Clone Stamp, healing tools, Red Eye, Content-Aware Fill.
    retouch: retouch_tools::RetouchTools,
    /// Pen, path selection and shape tools, the Paths panel.
    vector: vector_tools::VectorTools,

    /// New canvas being prepared (canvas coordinates, integers).
    canvas_rect: Option<Rect>,
    /// Position and size of the element being prepared.
    transform_rect: Option<Rect>,
    /// Rotation being prepared, in degrees (clockwise), around the element's center.
    transform_angle: f32,
    /// Rotation in progress with the mouse: (pointer angle at the start, start angle).
    rotate_drag: Option<(f32, f32)>,
    keep_ratio: bool,
    fit_canvas: bool,
    resize_filter: ResizeFilter,
    /// Handle being dragged (tools with handles).
    drag: Option<RectDrag>,
    /// Element moved directly with the mouse in the canvas tool.
    layer_drag: Option<LayerDrag>,
    /// Layer dragged in the list of the Layers panel.
    list_drag: Option<usize>,
    /// Snap guides of the move in progress (computed at its start).
    snap_targets: Option<SnapTargets>,
    /// Guides reached in this frame, to draw.
    guides: Snap,
    /// Ctrl held: free move, without snapping (like Photoshop).
    snap_off: bool,

    /// Last series of property edits: (layer, property) and length of the
    /// history right after it was recorded.
    prop_session: Option<((u64, &'static str), u64)>,
    /// "Make a color transparent" window open, and its settings.
    transparency: Option<Transparency>,
    /// Image adjustment window open.
    adjust: Option<AdjustDialog>,
    /// Filter menu: settings, dialog, filter being applied.
    filters: filters_ui::Filters,
    /// Camera Raw: its window, RAW files being read, developments.
    camera_raw: camera_raw_ui::CameraRaw,
    /// Filter › Liquify: its workspace, the result being computed.
    liquify: liquify_ui::LiquifyState,
    /// Histograms of the active layer, for levels and curves: (layer, revision).
    histogram: Option<((u64, u64), Histograms)>,
    /// Preview shown on a layer's texture.
    preview_shown: Option<ShownPreview>,
    /// Confirmation asked before losing changes.
    confirm: Option<Confirm>,
    /// "Discard hidden layers?" before Flatten Image.
    flatten_confirm: bool,
    /// Command palette (Ctrl+K).
    palette: palette::Palette,
    /// Recently opened or saved files (start screen).
    recent: crate::recent::Recent,
    /// Export As window open, and its settings for the session.
    export: Option<export_as::ExportDialog>,
    export_settings: crate::export::ExportOptions,
    /// Dropped files waiting for a modal window to close.
    dropped: Vec<PathBuf>,

    /// System fonts, loaded the first time text is used.
    text_engine: Option<TextEngine>,
    /// Type tool: settings of the next texts, text being typed.
    type_tool: type_tool::TypeTool,
    font_filter: String,

    /// "Split into elements" window open.
    split_open: bool,
    split_gap: u32,
    /// Computed preview: fingerprint (layer, pixels, position, gap) → boxes of
    /// the elements in canvas coordinates.
    split_preview: Option<(u64, Vec<Rect>)>,

    zoom: f32,
    pan: Vec2,
    /// Fit the canvas in the view at the next frame, up to this zoom.
    fit_pending: Option<f32>,
    hover_pixel: Option<(i64, i64)>,
    status: String,
    /// Last message shown and since when (to clear it).
    status_shown: String,
    status_since: f64,
    title: String,

    /// The composited document on the GPU.
    canvas_view: Option<render::CanvasView>,
    /// The layer being transformed, composited on its own.
    lifted: Option<render::Lifted>,
    /// Blend mode the pointer rests on in the list (shown on the canvas).
    blend_preview: Option<(u64, BlendMode)>,
    /// The active layer's mask shown alone, in gray (Alt + click on it).
    mask_view: bool,
    /// Adjustment layer edited in the Properties panel: its id, the tonal
    /// range or channel shown, the selected curve point.
    panel_editor: (u64, usize, Option<usize>),
    /// Layer Style window open.
    layer_style: Option<layer_style::StyleDialog>,
    /// History panel.
    history: history_panel::HistoryPanel,
    /// Channels panel, and what the canvas shows of the channels.
    channels: channels_panel::ChannelsPanel,
    /// Free Transform distorted or warped, its mode, Transform Again.
    xform: transform::Transforming,
    /// Actions panel: the actions, the one recorded, those played.
    actions: actions_panel::ActionsState,
    /// Batch, Image Processor and Fit Image, and the batch running.
    automate: batch::Automate,
    /// Color settings and profiles, their dialogs, the canvas's colors.
    color: color_ui::ColorUi,
    /// Smart objects and smart filters.
    smart: smart_ui::SmartUi,
    /// Artboard tool, artboard dialogs, Artboards to Files.
    artboards: artboard_tool::Artboards,
    /// File › New window.
    new_doc: new_document::NewDocument,
}

/// Processing previewed on a layer while a window is open.
#[allow(clippy::large_enum_variant)] // Only one, like `Adjustment`.
#[derive(Clone, PartialEq)]
enum LayerPreview {
    Transparency(Transparency),
    Adjust(Adjustment),
    /// A filter, computed in the background (see `filters_ui`): the
    /// fingerprint of the layer and settings.
    Filter(u64),
}

impl LayerPreview {
    fn apply(&self, image: &mut image::RgbaImage) {
        match self {
            LayerPreview::Transparency(t) => {
                let [r, g, b, _] = t.color.to_srgba_unmultiplied();
                crate::pixel_ops::color_to_alpha(image, [r, g, b], t.tolerance, t.softness);
            }
            LayerPreview::Adjust(adjustment) => adjustment.apply(image),
            // Shown once computed, never applied here.
            LayerPreview::Filter(_) => {}
        }
    }
}

/// Outline segment (canvas coordinates).
type Edge = [(i64, i64); 2];

/// Contents of the Fill… window, like Photoshop.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FillContent {
    Foreground,
    Background,
    Color,
    ContentAware,
    Black,
    Gray,
    White,
}

impl FillContent {
    const ALL: [FillContent; 7] = [
        FillContent::Foreground,
        FillContent::Background,
        FillContent::Color,
        FillContent::ContentAware,
        FillContent::Black,
        FillContent::Gray,
        FillContent::White,
    ];

    fn label(self) -> String {
        match self {
            FillContent::Foreground => t!("app-fill-foreground"),
            FillContent::Background => t!("app-fill-background"),
            FillContent::Color => t!("app-fill-color"),
            FillContent::ContentAware => t!("retouch-content-aware"),
            FillContent::Black => t!("app-fill-black"),
            FillContent::Gray => t!("app-fill-gray"),
            FillContent::White => t!("app-fill-white"),
        }
    }
}

/// Settings of the Fill… window.
#[derive(Clone, Copy)]
struct FillDialog {
    content: FillContent,
    /// Color chosen for "Color…".
    color: Color32,
    opacity: f32,
    preserve_transparency: bool,
    /// Content-Aware: Color Adaptation.
    adapt: bool,
}

/// Settings of the Image Size window.
#[derive(Clone, Copy)]
struct ImageSizeDialog {
    width: u32,
    height: u32,
    keep_ratio: bool,
    filter: ResizeFilter,
}

/// Settings of the Canvas Size window. With `relative`, `width` and
/// `height` are what is added (or removed).
#[derive(Clone, Copy)]
struct CanvasSizeDialog {
    width: i64,
    height: i64,
    relative: bool,
    /// Where the current image stays: column and row, 0 to 2.
    anchor: (u8, u8),
    /// Color of the added area: `None` = transparent.
    extension: Option<FillContent>,
}

/// Settings of Image Rotation > Arbitrary.
#[derive(Clone, Copy)]
struct RotateDialog {
    angle: f32,
    clockwise: bool,
}

/// Windows that leave the keyboard to the app, except Esc (Cancel) and
/// Enter (OK).
#[derive(Clone, Copy)]
enum ToolWindow {
    Split,
    Transparency,
    Models,
    ColorRange,
    SelectMask,
}

impl ToolWindow {
    /// Id of its dialog (`DialogSpec`).
    fn id(self) -> &'static str {
        match self {
            ToolWindow::Split => "split",
            ToolWindow::Transparency => "transparency",
            ToolWindow::Models => "ai_models",
            ToolWindow::ColorRange => "color_range",
            ToolWindow::SelectMask => "select_mask",
        }
    }
}

/// Drag of a selection: starting point (canvas coordinates) and selection.
#[derive(Clone)]
struct SelectMove {
    from: Pos2,
    selection: Selection,
    /// Ctrl: the pixels move too (Alt: a copy of them). They are lifted at
    /// the first real move, into a floating layer: its id and starting offset.
    pixels: Option<bool>,
    floating: Option<(u64, (i64, i64))>,
    /// The pointer went further than for a click.
    moved: bool,
}

/// Settings of "Make a color transparent".
#[derive(Clone, Copy, PartialEq)]
struct Transparency {
    color: Color32,
    tolerance: f32,
    softness: f32,
    /// A click on the image picks the color.
    picking: bool,
    /// Result shown on the layer while the window is open.
    preview: bool,
}

impl Action {
    /// Actions that modify the active layer: refused if it is locked.
    fn modifies_active(self) -> bool {
        matches!(
            self,
            Action::Cut
                | Action::ClearSelection
                | Action::FillPrimary
                | Action::FillPrimaryKeep
                | Action::ContentAwareFill
                | Action::FillSecondaryKeep
                | Action::FillSecondary
                | Action::MakeTransparent
                | Action::Adjust(_)
                | Action::Filter(_)
                | Action::LastFilter
                | Action::CameraRawFilter
                | Action::Liquify
                | Action::Delete
                | Action::Split
                | Action::CenterElement
                | Action::MergeDown
                | Action::Ungroup
                | Action::ToggleClipping
                | Action::AddMask(_)
                | Action::DeleteMask
                | Action::ApplyMask
                | Action::ToggleMaskEnabled
                | Action::ToggleMaskLink
                | Action::InvertMask
                | Action::LayerStyle(_)
                | Action::ClearLayerStyle
                | Action::BlendStep(_)
                | Action::SetBlend(_)
                | Action::Transform(_)
                | Action::Rasterize
        )
    }

    /// Actions that first apply what is being prepared (transform frame,
    /// canvas…). The others leave it in place or handle it themselves.
    fn commits_pending(self) -> bool {
        !matches!(
            self,
            Action::Copy
                | Action::SelectAll
                | Action::Deselect
                | Action::Reselect
                | Action::InvertSelection
                | Action::SelectSubject
                | Action::Fit
                | Action::ActualSize
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::Undo
                | Action::Redo
                | Action::Tool(_)
                | Action::Open
                | Action::NewDocument
                | Action::Quit
                | Action::Save
                | Action::SaveAs
                | Action::ExportAs
                | Action::SetLanguage(_)
                | Action::AiModels
                | Action::View(_)
                | Action::Swatches
                | Action::History
                | Action::Channels
                | Action::Transform(_)
                | Action::Paths
                | Action::Automate(actions_panel::Command::Panel)
                | Action::Color(
                    color_ui::Command::ProofColors
                        | color_ui::Command::GamutWarning
                        | color_ui::Command::Settings
                )
        )
    }
}

/// Image adjustment in the menus: kind, context menu icon, shortcut.
struct AdjustEntry {
    kind: AdjustKind,
    icon: &'static str,
    shortcut: Option<&'static KeyboardShortcut>,
}

impl AdjustEntry {
    /// Menu label: the window title followed by "…".
    fn label(&self) -> String {
        match self.kind {
            AdjustKind::Invert => self.kind.title(),
            _ => format!("{}…", self.kind.title()),
        }
    }
}

/// The adjustments, in the order of Photoshop's Adjustments menu.
const ADJUSTMENTS: [AdjustEntry; 12] = [
    AdjustEntry {
        kind: AdjustKind::BrightnessContrast,
        icon: icon::SUN,
        shortcut: None,
    },
    AdjustEntry {
        kind: AdjustKind::Levels,
        icon: icon::CHART_BAR,
        shortcut: Some(&SHORTCUT_LEVELS),
    },
    AdjustEntry {
        kind: AdjustKind::Curves,
        icon: icon::WAVE_SINE,
        shortcut: Some(&SHORTCUT_CURVES),
    },
    AdjustEntry {
        kind: AdjustKind::Vibrance,
        icon: icon::DROP,
        shortcut: None,
    },
    AdjustEntry {
        kind: AdjustKind::HueSaturation,
        icon: icon::PALETTE,
        shortcut: Some(&SHORTCUT_HUE_SATURATION),
    },
    AdjustEntry {
        kind: AdjustKind::ColorBalance,
        icon: icon::SLIDERS_HORIZONTAL,
        shortcut: Some(&SHORTCUT_COLOR_BALANCE),
    },
    AdjustEntry {
        kind: AdjustKind::BlackWhite,
        icon: icon::CIRCLE_HALF_TILT,
        shortcut: Some(&SHORTCUT_BLACK_WHITE),
    },
    AdjustEntry {
        kind: AdjustKind::PhotoFilter,
        icon: icon::APERTURE,
        shortcut: None,
    },
    AdjustEntry {
        kind: AdjustKind::Invert,
        icon: icon::SWAP,
        shortcut: Some(&SHORTCUT_INVERT),
    },
    AdjustEntry {
        kind: AdjustKind::Posterize,
        icon: icon::STAIRS,
        shortcut: None,
    },
    AdjustEntry {
        kind: AdjustKind::Threshold,
        icon: icon::CIRCLE_HALF,
        shortcut: None,
    },
    AdjustEntry {
        kind: AdjustKind::GradientMap,
        icon: icon::GRADIENT,
        shortcut: None,
    },
];

/// Menu entry: label, enabled, displayed shortcut, action.
enum MenuEntry {
    Item(String, bool, Option<&'static KeyboardShortcut>, Action),
    Separator,
    /// Submenu.
    Submenu(String, Vec<MenuEntry>),
    /// One of several exclusive options, checked if selected.
    Choice(String, bool, Action),
    /// On / off option: label, enabled, checked, displayed shortcut, action.
    Check(
        String,
        bool,
        bool,
        Option<&'static KeyboardShortcut>,
        Action,
    ),
    /// An option shown but never available: label, why (its tooltip).
    Unavailable(String, String),
}

/// Image adjustment window: the current adjustment and, for color balance,
/// the tonal range shown (0 shadows, 1 midtones, 2 highlights); for levels
/// and curves, the channel (0 RGB, 1 red, 2 green, 3 blue).
#[derive(Clone)]
struct AdjustDialog {
    adjustment: Adjustment,
    range: usize,
    /// Selected curve point.
    point: Option<usize>,
    /// Result shown on the layer while the window is open.
    preview: bool,
}

/// What waits for the answer to "Save changes?".
#[derive(Clone, Copy)]
enum Confirm {
    Close,
    CloseDocument,
}

impl AtelierApp {
    pub fn new(cc: &eframe::CreationContext<'_>, path: Option<PathBuf>) -> Self {
        ui_kit::install(&cc.egui_ctx);
        let mut app = Self {
            ctx: cc.egui_ctx.clone(),
            doc: None,
            parked: Vec::new(),
            downloads: downloads::Downloads::default(),
            active_tab: 0,
            thumbs: HashMap::new(),
            stroke_dirty: None,
            tool: Tool::Transform,
            primary: Color32::BLACK,
            secondary: Color32::WHITE,
            colors: color_picker::Colors::load(),
            rulers: rulers::Rulers::load(),
            sel: Default::default(),
            brush_size: 12.0,
            // 32 out of 255, Photoshop's default.
            bucket_tolerance: 32.0 / 255.0,
            bucket_contiguous: true,
            layer_set: Vec::new(),
            selection: None,
            marquee: None,
            marquee_preview: None,
            select_mode: SelectMode::Replace,
            wand_tolerance: 32.0 / 255.0,
            wand_contiguous: true,
            wand_all_layers: false,
            lasso: None,
            last_view: None,
            lasso_width: 10.0,
            lasso_frequency: 57.0,
            edges: Default::default(),
            subject_job: None,
            model_prompt: None,
            after_download: Vec::new(),
            models_window: None,
            object_select: Default::default(),
            group_tool: HashMap::new(),
            select_move: None,
            selection_outline: None,
            fill_dialog: None,
            image_size: None,
            canvas_size: None,
            rotate_dialog: None,
            last_selection: None,
            last_paint: None,
            last_digit: None,
            focus_name: false,
            typing: false,
            plain_v: false,
            cut_key: false,
            hardness: 0.8,
            opacity: 1.0,
            stroke: None,
            paint: Default::default(),
            retouch: Default::default(),
            vector: Default::default(),
            canvas_rect: None,
            transform_rect: None,
            transform_angle: 0.0,
            rotate_drag: None,
            keep_ratio: true,
            fit_canvas: false,
            resize_filter: ResizeFilter::Lanczos,
            drag: None,
            layer_drag: None,
            list_drag: None,
            snap_targets: None,
            guides: Snap::default(),
            snap_off: false,
            prop_session: None,
            confirm: None,
            flatten_confirm: false,
            palette: Default::default(),
            recent: crate::recent::Recent::load(),
            export: None,
            export_settings: Default::default(),
            dropped: Vec::new(),
            transparency: None,
            adjust: None,
            filters: Default::default(),
            camera_raw: Default::default(),
            liquify: Default::default(),
            preview_shown: None,
            histogram: None,
            text_engine: None,
            type_tool: Default::default(),
            font_filter: String::new(),
            split_open: false,
            split_gap: 12,
            split_preview: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: Some(1.0),
            hover_pixel: None,
            status: String::new(),
            status_shown: String::new(),
            status_since: 0.0,
            title: String::new(),
            canvas_view: None,
            lifted: None,
            blend_preview: None,
            mask_view: false,
            panel_editor: (0, 0, None),
            layer_style: None,
            xform: Default::default(),
            actions: Default::default(),
            automate: Default::default(),
            color: color_ui::ColorUi::load(),
            smart: Default::default(),
            artboards: Default::default(),
            new_doc: Default::default(),
            history: Default::default(),
            channels: Default::default(),
        };
        if let Some(path) = path {
            app.open_path(&path);
        }
        app
    }

    fn set_tool(&mut self, tool: Tool) {
        if tool != self.tool {
            self.commit_pending();
            self.tool = tool;
            // Gestures of the previous tool don't resume when it comes back.
            self.lasso = None;
            self.edges.cancel_click();
            self.marquee = None;
            self.marquee_preview = None;
            self.sel.free_lasso = None;
            self.object_select.cancel_press();
        }
        self.group_tool.insert(tool.group(), tool);
    }

    /// The tool a group's slot shows: the current one if it belongs to the
    /// group, otherwise the group's last used tool.
    fn group_shown(&self, group: usize) -> Tool {
        let tools = Tool::GROUPS[group];
        if tools.contains(&self.tool) {
            return self.tool;
        }
        self.group_tool.get(&group).copied().unwrap_or(tools[0])
    }

    /// Tool for a press of `pressed`'s key, like Photoshop: the key gives
    /// the last used tool of its group, Shift + the key the next one.
    fn tool_for_key(&self, pressed: Tool, shift: bool) -> Tool {
        let index = pressed.group();
        // The tools of the group with that key (not the anchor point tools).
        let group: Vec<Tool> = Tool::GROUPS[index]
            .iter()
            .copied()
            .filter(|t| t.shortcut() == pressed.shortcut())
            .collect();
        let last = self
            .group_tool
            .get(&index)
            .copied()
            .filter(|t| group.contains(t))
            .unwrap_or(group[0]);
        if !shift || group.len() < 2 {
            return last;
        }
        let i = group.iter().position(|&t| t == last).unwrap_or(0);
        group[(i + 1) % group.len()]
    }

    fn select_layer(&mut self, index: usize) {
        if self
            .doc
            .as_ref()
            .is_some_and(|d| d.active != index && index < d.layers.len())
        {
            self.commit_pending();
            self.mask_view = false;
            if let Some(doc) = &mut self.doc {
                doc.active = index;
            }
        }
    }

    /// Applies what is being prepared (canvas, move…).
    /// Esc, on the contrary, gives it up.
    fn commit_pending(&mut self) {
        self.commit_text();
        self.finish_stroke();
        if self.canvas_rect.is_some() {
            self.record_crop();
            self.apply_canvas();
        }
        if self.transform_rect.is_some() {
            self.apply_transform();
        }
        self.finish_select_move();
        self.apply_selection_transform();
        self.reset_pending();
    }

    /// Drops what is being prepared.
    fn reset_pending(&mut self) {
        self.canvas_rect = None;
        self.sel.transform = None;
        self.reset_transform();
        self.layer_drag = None;
    }

    /// Drops the transform being prepared by the Move tool.
    fn reset_transform(&mut self) {
        self.transform_rect = None;
        self.transform_angle = 0.0;
        self.rotate_drag = None;
        self.drag = None;
        self.cancel_transform_extras();
    }

    /// Ends the selection move in progress where it is: lifted pixels are
    /// dropped back onto their layer.
    fn finish_select_move(&mut self) {
        let Some(m) = self.select_move.take() else {
            return;
        };
        if let (Some((id, _)), Some(doc)) = (m.floating, &mut self.doc) {
            doc.drop_floating(id);
        }
        self.clip_selection();
    }

    /// Ctrl+Z during a selection move: everything goes back to how it was
    /// before it. Returns `false` if no move was in progress.
    fn cancel_select_move(&mut self) -> bool {
        let Some(m) = self.select_move.take() else {
            return false;
        };
        if let (Some((id, _)), Some(doc)) = (m.floating, &mut self.doc)
            && doc.drop_floating(id)
        {
            // The lift's step.
            doc.undo();
        }
        self.selection = Some(m.selection);
        true
    }

    /// Records the state before a property edit. With a key (layer,
    /// property), a series of edits of the same property counts as a single
    /// undo step, as long as nothing else has been recorded in between;
    /// without a key, every edit counts.
    fn record_change(&mut self, name: String, key: Option<(u64, &'static str)>) {
        let Some(doc) = &mut self.doc else { return };
        let same_series = key.is_some()
            && self
                .prop_session
                .is_some_and(|(k, step)| Some(k) == key && step == doc.history_step());
        if !same_series {
            doc.checkpoint(name);
            self.prop_session = key.map(|k| (k, doc.history_step()));
        }
        doc.modified = true;
    }

    /// Arrow keys, like Photoshop: with a selection tool, they nudge the
    /// selection outline (Ctrl: the selected pixels); with the Move and Crop
    /// tools, the selected elements (or the frame being prepared by the Move
    /// tool); with the other tools, only with Ctrl (the Move tool's key).
    fn nudge(&mut self, (dx, dy): (i64, i64)) {
        if self.vector_nudge((dx, dy)) {
            return;
        }
        let ctrl = self.ctx.input(|i| i.modifiers.command);
        let selecting = self.tool.selects();
        if selecting && let Some(selection) = self.selection.clone() {
            self.nudge_selection(selection, (dx, dy), ctrl);
            return;
        }
        if !ctrl && !matches!(self.tool, Tool::Transform | Tool::Canvas) {
            return;
        }
        if self.tool == Tool::Transform
            && let Some(r) = self.transform_rect
        {
            self.transform_rect = Some(r.translate(vec2(dx as f32, dy as f32)));
            self.nudge_transform(vec2(dx as f32, dy as f32));
            return;
        }
        if self.doc.is_none() || self.refuse_if_locked() {
            return;
        }
        let id = self
            .doc
            .as_ref()
            .map(|d| d.active_layer().id)
            .unwrap_or_default();
        let indices = self.selected_indices();
        self.record_change(t!("xchg-hist-move"), Some((id, "position")));
        let Some(doc) = &mut self.doc else { return };
        // With their groups' layers and the layers linked to them.
        let mut moved: Vec<usize> = indices.iter().flat_map(|&i| doc.moving_with(i)).collect();
        moved.sort_unstable();
        moved.dedup();
        let ids: Vec<u64> = moved.iter().map(|&i| doc.layers[i].id).collect();
        for i in moved {
            let layer = &mut doc.layers[i];
            if !layer.locked {
                layer.offset.0 += dx;
                layer.offset.1 += dy;
            }
        }
        self.artboards_after_move(&ids);
        let Some(doc) = &self.doc else { return };
        let layer = doc.active_layer();
        let (x, y) = layer.offset;
        self.status = t!(
            "status-layer-moved",
            name = layer.name.as_str(),
            x = signed(x as f32),
            y = signed(y as f32)
        );
    }

    /// Eyedropper: color shown under the pointer → foreground (or background).
    fn pick_color(&mut self, at: Pos2, to_background: bool) {
        let Some(doc) = &self.doc else { return };
        let Some([r, g, b, _]) = doc.color_at(at.x.floor() as i64, at.y.floor() as i64) else {
            self.status = t!("status-no-color");
            return;
        };
        // Like Photoshop: the picked color is always opaque.
        let color = Color32::from_rgb(r, g, b);
        let hex = hex_color([r, g, b]);
        if to_background {
            self.secondary = color;
            self.status = t!("status-background-color", hex = hex);
        } else {
            self.primary = color;
            self.status = t!("status-foreground-color", hex = hex);
        }
    }

    /// Paint bucket at the given point, with the foreground color.
    fn bucket_fill(&mut self, at: Pos2) {
        if self.refuse_if_locked() {
            return;
        }
        let color = self.primary.to_srgba_unmultiplied();
        let tolerance = (self.bucket_tolerance * 255.0).round() as u8;
        let contiguous = self.bucket_contiguous;
        let Some(doc) = &mut self.doc else { return };
        if doc.fill_at(
            at.x.floor() as i64,
            at.y.floor() as i64,
            color,
            tolerance,
            self.selection.as_ref(),
            contiguous,
        ) {
            self.reset_pending();
        }
    }

    /// Image size: everything is scaled, then the texts are rendered again
    /// at their new font size, to stay sharp.
    fn apply_image_size(&mut self, d: ImageSizeDialog) {
        self.commit_pending();
        self.record_image_size(d);
        let Some(doc) = &mut self.doc else { return };
        if !doc.resize_image(d.width, d.height, d.filter.filter()) {
            return;
        }
        self.rerender_texts();
        self.resize_filter = d.filter;
        // The selection's coordinates no longer make sense.
        self.forget_selection();
        self.status = t!("status-image-size", width = d.width, height = d.height);
        self.fit_pending = Some(1.0);
        self.reset_pending();
    }

    /// Fills the active layer (inside the selection if there is one).
    fn fill(&mut self, color: Color32, which: &str, opacity: f32, preserve: bool) {
        let Some(doc) = &mut self.doc else { return };
        let selection = self.selection.as_ref();
        if doc.fill_active(color.to_srgba_unmultiplied(), selection, opacity, preserve) {
            self.status = t!(
                "status-filled",
                name = doc.active_layer().name.as_str(),
                content = which
            );
            self.reset_pending();
        }
    }

    /// Drops the selection, keeping it for Reselect.
    fn deselect(&mut self) {
        if let Some(sel) = self.selection.take() {
            self.last_selection = Some(sel);
        }
    }

    /// The canvas coordinates changed: the selection, and the one kept for
    /// Reselect, no longer apply.
    fn forget_selection(&mut self) {
        self.selection = None;
        self.last_selection = None;
    }

    /// Selects the visible pixels of a layer (Ctrl + click on its thumbnail).
    /// Shift: add to the selection, Alt: subtract.
    fn select_layer_pixels(&mut self, i: usize) {
        let Some(doc) = &self.doc else { return };
        let layer = &doc.layers[i];
        let alpha =
            image::GrayImage::from_fn(layer.image().width(), layer.image().height(), |x, y| {
                image::Luma([layer.image().get_pixel(x, y).0[3]])
            });
        let found =
            Selection::from_mask(&alpha, layer.offset).and_then(|s| s.within(doc.full_rect()));
        if found.is_none() {
            self.status = t!("status-no-visible-pixel");
            return;
        }
        let (shift, alt) = self.ctx.input(|i| (i.modifiers.shift, i.modifiers.alt));
        let mode = self.selection_mode(shift, alt);
        self.apply_selection(found, mode);
    }

    /// Indices of the selected layers (at least the active layer).
    fn selected_indices(&self) -> Vec<usize> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let active_id = doc.active_layer().id;
        if !self.layer_set.contains(&active_id) {
            return vec![doc.active];
        }
        (0..doc.layers.len())
            .filter(|&i| self.layer_set.contains(&doc.layers[i].id))
            .collect()
    }

    /// Click in the list: alone (single selection), Ctrl (add/remove),
    /// Shift (range from the active layer).
    fn click_layer(&mut self, i: usize, modifiers: egui::Modifiers) {
        let current = self.selected_indices();
        let Some(doc) = &self.doc else { return };
        let id = doc.layers[i].id;
        if modifiers.command {
            let mut ids: Vec<u64> = current.iter().map(|&j| doc.layers[j].id).collect();
            if let Some(pos) = ids.iter().position(|&x| x == id) {
                if ids.len() > 1 {
                    ids.remove(pos);
                    if i == doc.active {
                        let next = (0..doc.layers.len())
                            .rev()
                            .find(|&j| ids.contains(&doc.layers[j].id));
                        self.layer_set = ids;
                        if let Some(next) = next {
                            self.select_layer(next);
                        }
                        return;
                    }
                }
            } else {
                ids.push(id);
            }
            self.layer_set = ids;
            self.select_layer(i);
        } else if modifiers.shift {
            let (a, b) = (doc.active.min(i), doc.active.max(i));
            self.layer_set = (a..=b).map(|j| doc.layers[j].id).collect();
        } else {
            self.layer_set = vec![id];
            self.select_layer(i);
        }
    }

    fn active_locked(&self) -> bool {
        self.doc.as_ref().is_some_and(|d| d.active_layer().locked)
    }

    /// If the active layer is locked, says so and returns `true`.
    fn refuse_if_locked(&mut self) -> bool {
        let Some(layer) = self.doc.as_ref().map(Document::active_layer) else {
            return false;
        };
        if layer.locked {
            self.status = format!(
                "🔒 {}",
                t!("status-layer-is-locked", name = layer.name.as_str())
            );
        }
        layer.locked
    }

    fn full_rect(&self) -> Option<Rect> {
        Some(to_rect(self.doc.as_ref()?.full_rect()))
    }

    /// Element handled by the Move tool: visible pixels of the active layer.
    /// (The layer caches its box as long as its pixels don't change.)
    fn transform_source(&self) -> Option<Rect> {
        if let Some(source) = self.transform_source_override() {
            return source;
        }
        let doc = self.doc.as_ref()?;
        let layer = doc.active_layer();
        if !layer.has_pixels() && !layer.is_group() {
            return None;
        }
        let bounds = doc.element_bounds(doc.active);
        Some(to_rect(bounds.unwrap_or(layer.rect())))
    }

    fn undo(&mut self) {
        self.finish_stroke();
        if self.cancel_select_move() || self.quick_mask_undo(false) {
            return;
        }
        // A transform or crop being prepared: only it is undone, like
        // Photoshop (it isn't in the history yet).
        if self.transform_rect.is_some() || self.canvas_rect.is_some() {
            self.reset_pending();
            return;
        }
        self.reset_pending();
        if self.doc.as_mut().is_some_and(Document::undo) {
            self.reset_pending();
            self.clip_selection();
        }
    }

    fn redo(&mut self) {
        self.finish_stroke();
        if self.quick_mask_undo(true) {
            return;
        }
        self.finish_select_move();
        self.reset_pending();
        if self.doc.as_mut().is_some_and(Document::redo) {
            self.reset_pending();
            self.clip_selection();
        }
    }

    /// Keeps the selection within the canvas (after its size changed).
    fn clip_selection(&mut self) {
        if let Some(canvas) = self.doc.as_ref().map(Document::full_rect) {
            self.selection = self.selection.take().and_then(|s| s.within(canvas));
        }
    }

    /// Merge Down, Merge Visible or Flatten Image.
    fn merge_layers(&mut self, action: Action) {
        let Some(doc) = &mut self.doc else { return };
        let merged = match action {
            Action::MergeDown => doc.merge_down(),
            Action::MergeVisible => doc.merge_visible(),
            _ => {
                doc.flatten_image();
                Ok(())
            }
        };
        self.status = match merged {
            Ok(()) => {
                let name = doc.active_layer().name.clone();
                self.reset_pending();
                t!("status-layers-merged", name = name.as_str())
            }
            Err(MergeError::Locked) => t!("doc-merge-locked"),
            Err(MergeError::TooLarge) => t!("doc-merge-too-large"),
            Err(MergeError::Nothing) if matches!(action, Action::MergeDown) => {
                t!("status-merge-down-failed")
            }
            Err(MergeError::Nothing) => t!("status-merge-visible-failed"),
        };
    }

    /// Ends the stroke in progress (the previous state was recorded at its
    /// start; a stroke that painted nothing gives that step up).
    fn finish_stroke(&mut self) {
        let Some(stroke) = self.stroke.take() else {
            return;
        };
        // The healing tools finish their work (the Spot Healing Brush its own way).
        let Some(stroke) = self.finish_retouch(stroke) else {
            return;
        };
        if let Some(doc) = &mut self.doc {
            doc.hold_effects(false);
            // A targeted mask gets its place back.
            doc.end_paint();
            if !stroke.painted() {
                doc.discard_step();
                self.stroke_dirty = None;
            }
        }
    }

    fn run(&mut self, action: Action) {
        if action.modifies_active() && self.refuse_if_locked() {
            return;
        }
        if action.commits_pending() {
            self.commit_pending();
        }
        self.record_action(action);
        match action {
            Action::Transform(cmd) => self.transform_command(cmd),
            Action::Align(op) => {
                let indices = self.selected_indices();
                if let Some(doc) = &mut self.doc
                    && doc.align(&indices, op)
                {
                    self.reset_pending();
                }
            }
            Action::Copy => {
                self.copy_to_clipboard();
            }
            Action::Cut => {
                if self.selection.is_some() && self.copy_to_clipboard() {
                    self.run(Action::ClearSelection);
                }
            }
            Action::SelectAll => {
                self.selection = self.doc.as_ref().map(|d| d.full_rect().into());
            }
            Action::Deselect => self.deselect(),
            Action::Reselect => {
                if let (Some(sel), Some(doc)) = (self.last_selection.clone(), &self.doc) {
                    self.selection = sel.within(doc.full_rect());
                }
            }
            Action::ClearSelection => {
                if let (Some(sel), Some(doc)) = (&self.selection, &mut self.doc)
                    && doc.clear_region(sel)
                {
                    self.reset_pending();
                }
            }
            Action::LayerFromSelection if self.selection.is_none() => self.run(Action::Duplicate),
            Action::LayerFromSelection => {
                if let (Some(sel), Some(doc)) = (&self.selection, &mut self.doc) {
                    if doc.layer_from_region(sel) {
                        self.reset_pending();
                    } else {
                        self.status = t!("status-selection-empty");
                    }
                }
            }
            Action::CropToSelection => {
                if let Some(sel) = self.selection.clone().filter(|s| !s.inverted) {
                    self.canvas_rect = Some(to_rect(sel.rect));
                    self.apply_canvas();
                }
            }
            Action::Paste => self.paste_from_clipboard(),
            // No settings: applied at once, like Photoshop.
            // Not on a smart object, like Photoshop.
            Action::Adjust(_) if self.smart_filters_apply() => {
                self.refuse_on_smart();
            }
            Action::Adjust(AdjustKind::Invert) => {
                let selection = self.selection.as_ref();
                if let Some(doc) = &mut self.doc
                    && doc.adjust_active(&Adjustment::Invert, selection)
                {
                    self.reset_pending();
                }
            }
            Action::Adjust(kind) => {
                if self.doc.is_some() {
                    self.adjust = Some(AdjustDialog {
                        adjustment: kind.initial(),
                        // Balance: open on the midtones.
                        range: usize::from(kind == AdjustKind::ColorBalance),
                        point: None,
                        preview: true,
                    });
                }
            }
            Action::MakeTransparent => {
                if self.doc.is_some() {
                    self.transparency = Some(dialogs::TRANSPARENCY_DEFAULT);
                }
            }
            Action::ToggleLock => {
                // An undo step, like a visibility change.
                let locked = self.doc.as_ref().is_some_and(|d| d.active_layer().locked);
                let step = if locked {
                    t!("xchg-hist-unlock-layer")
                } else {
                    t!("xchg-hist-lock-layer")
                };
                self.record_change(step, None);
                if let Some(doc) = &mut self.doc {
                    let layer = doc.active_layer_mut();
                    layer.locked = !layer.locked;
                    let name = layer.name.as_str();
                    self.status = if layer.locked {
                        format!("🔒 {}", t!("status-layer-locked", name = name))
                    } else {
                        format!("🔓 {}", t!("status-layer-unlocked", name = name))
                    };
                }
            }
            Action::Open => {
                if let Some(path) = self.pick_image(&t!("app-dialog-open-image")) {
                    self.open_path(&path);
                }
            }
            Action::CloseDocument => {
                if self.unsaved() {
                    self.confirm = Some(Confirm::CloseDocument);
                } else {
                    self.close_document();
                }
            }
            Action::Import => {
                if let Some(path) = self.pick_image(&t!("smart-menu-place-embedded")) {
                    self.place_path(&path, false);
                }
            }
            Action::SetLanguage(lang) => i18n::choose(lang),
            Action::AiModels => {
                self.models_window.get_or_insert_default();
            }
            Action::Quit => {
                self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Action::Save => self.save(),
            Action::SaveAs => self.save_as_dialog(),
            Action::ExportAs => self.open_export(self.export_settings.clone()),
            Action::Undo => self.undo(),
            Action::Redo => self.redo(),
            Action::Tool(tool) => self.set_tool(tool),
            Action::NewLayer
            | Action::Duplicate
            | Action::Delete
            | Action::Raise
            | Action::Lower => {
                let Some(doc) = &mut self.doc else { return };
                match action {
                    Action::NewLayer => doc.add_empty_layer(),
                    Action::Duplicate => doc.duplicate_active(),
                    Action::Delete => {
                        doc.delete_active();
                    }
                    Action::Raise => {
                        doc.move_active(true);
                    }
                    _ => {
                        doc.move_active(false);
                    }
                }
                self.reset_pending();
                // A deleted artboard no longer counts in the canvas.
                self.refit_artboards();
            }
            Action::FillPrimary
            | Action::FillSecondary
            | Action::FillPrimaryKeep
            | Action::FillSecondaryKeep => {
                let primary = matches!(action, Action::FillPrimary | Action::FillPrimaryKeep);
                let (color, which) = if primary {
                    (self.primary, t!("status-fill-foreground-color"))
                } else {
                    (self.secondary, t!("status-fill-background-color"))
                };
                let keep = matches!(action, Action::FillPrimaryKeep | Action::FillSecondaryKeep);
                self.fill(color, &which, 1.0, keep);
            }
            Action::FillDialog => {
                if self.doc.is_some() {
                    self.fill_dialog = Some(FillDialog {
                        content: FillContent::Foreground,
                        color: self.primary,
                        opacity: 1.0,
                        preserve_transparency: false,
                        adapt: true,
                    });
                }
            }
            Action::ContentAwareFill => self.open_content_aware(),
            Action::InvertSelection => {
                if let (Some(sel), Some(doc)) = (self.selection.take(), &self.doc) {
                    self.selection = Selection {
                        inverted: !sel.inverted,
                        ..sel
                    }
                    .within(doc.full_rect());
                }
            }
            Action::ImageSize => {
                if let Some(doc) = &self.doc {
                    self.image_size = Some(ImageSizeDialog {
                        width: doc.width(),
                        height: doc.height(),
                        keep_ratio: true,
                        filter: self.resize_filter,
                    });
                }
            }
            Action::CanvasSize => {
                if let Some(doc) = &self.doc {
                    self.canvas_size = Some(CanvasSizeDialog {
                        width: doc.width() as i64,
                        height: doc.height() as i64,
                        relative: false,
                        anchor: (1, 1),
                        extension: None,
                    });
                }
            }
            Action::RotateArbitrary => {
                if self.doc.is_some() {
                    self.rotate_dialog = Some(RotateDialog {
                        angle: 0.0,
                        clockwise: true,
                    });
                }
            }
            Action::RotateCanvas(degrees) => self.rotate_canvas(degrees as f32),
            Action::Filter(kind) => self.open_filter(kind),
            Action::LastFilter => self.last_filter(),
            Action::CameraRawFilter => self.camera_raw_filter(None),
            Action::Liquify => self.open_liquify(),
            Action::FlipCanvas(horizontal) => {
                let texts = self.texts_before();
                if let Some(doc) = &mut self.doc {
                    doc.flip_canvas(horizontal);
                    let map = type_tool::canvas_flip((doc.width(), doc.height()), horizontal);
                    self.texts_after(texts, map);
                    self.forget_selection();
                    self.reset_pending();
                }
            }
            Action::CropToElement | Action::CropToContent => {
                let Some(doc) = &self.doc else { return };
                let bounds = if matches!(action, Action::CropToElement) {
                    doc.element_bounds(doc.active)
                } else {
                    doc.content_bounds()
                };
                match bounds {
                    Some(b) if b == doc.full_rect() => {
                        self.status = if matches!(action, Action::CropToElement) {
                            t!(
                                "status-element-fills-canvas",
                                name = doc.active_layer().name.as_str()
                            )
                        } else {
                            t!("status-content-fills-canvas")
                        };
                    }
                    Some(b) => {
                        self.canvas_rect = Some(to_rect(b));
                        self.apply_canvas();
                    }
                    None => self.status = t!("status-nothing-to-crop"),
                }
            }
            Action::CenterElement | Action::BringToFront | Action::SendToBack => {
                let Some(doc) = &mut self.doc else { return };
                let changed = match action {
                    Action::CenterElement => doc.center_active(),
                    Action::BringToFront => doc.move_active_to_end(true),
                    _ => doc.move_active_to_end(false),
                };
                if changed {
                    self.reset_pending();
                }
            }
            Action::NewGroup
            | Action::GroupLayers
            | Action::Ungroup
            | Action::ToggleClipping
            | Action::NewAdjustmentLayer(_)
            | Action::NewFillLayer(_)
            | Action::AddMask(_)
            | Action::DeleteMask
            | Action::ApplyMask
            | Action::ToggleMaskEnabled
            | Action::ToggleMaskLink
            | Action::InvertMask
            | Action::MaskSelection
            | Action::LayerStyle(_)
            | Action::ClearLayerStyle
            | Action::LinkLayers
            | Action::BlendStep(_)
            | Action::SetBlend(_) => self.layer_action(action),
            Action::SelectSubject => self.find_subject(false),
            Action::RemoveBackground => self.find_subject(true),
            Action::Split => {
                self.split_open = self.doc.is_some();
            }
            // Like Photoshop, flattening asks before discarding hidden layers.
            Action::FlattenImage if self.doc.as_ref().is_some_and(Document::has_hidden_layers) => {
                self.flatten_confirm = true;
            }
            Action::MergeDown | Action::MergeVisible | Action::FlattenImage => {
                self.merge_layers(action);
            }
            // Around the center of the view.
            Action::ZoomIn => self.zoom_step(true, Pos2::ZERO, Pos2::ZERO),
            Action::ZoomOut => self.zoom_step(false, Pos2::ZERO, Pos2::ZERO),
            // Fit on Screen: small images are enlarged too, like Photoshop.
            Action::Fit => self.fit_pending = Some(MAX_ZOOM),
            Action::ActualSize => {
                self.zoom = 1.0;
                self.pan = Vec2::ZERO;
            }
            Action::View(command) => self.view_command(command),
            Action::Swatches => self.colors.panel_open = !self.colors.panel_open,
            Action::History => self.history.open = !self.history.open,
            Action::Channels => self.channels.open = !self.channels.open,
            Action::Paths => self.vector.panel.open = !self.vector.panel.open,
            Action::Path(command, dialog) => self.path_command(command, dialog),
            Action::Smart(command) => self.smart_command(command),
            Action::Rasterize => {
                if let Some(doc) = &mut self.doc
                    && doc.rasterize_active()
                {
                    self.status = t!(
                        "vector-status-rasterized",
                        name = doc.active_layer().name.as_str()
                    );
                }
            }
            Action::Select(command) => self.select_command(command),
            Action::Automate(command) => self.automate_command(command),
            Action::NewDocument => self.new_document(),
            Action::Artboard(command) => self.artboard_command(command),
            Action::Color(command) => self.color_command(command),
        }
    }

    fn apply_canvas(&mut self) {
        let (Some(r), Some(doc)) = (self.canvas_rect.take(), &mut self.doc) else {
            return;
        };
        let r = round_rect(r);
        let (w, h) = (r.width() as u32, r.height() as u32);
        if w == 0 || h == 0 {
            return;
        }
        let old_center = pos2(doc.width() as f32, doc.height() as f32) / 2.0;
        doc.crop(PixelRect {
            x: r.min.x as i64,
            y: r.min.y as i64,
            w,
            h,
        });
        self.status = t!("status-canvas-size", width = w, height = h);
        // The selection's coordinates no longer make sense.
        self.forget_selection();
        self.after_geometry_change(r.center() - old_center);
    }

    fn apply_transform(&mut self) {
        self.record_transform();
        self.commit_placing();
        if self.apply_transform_extras() {
            return;
        }
        let Some(src) = self.transform_source() else {
            return;
        };
        let r = self.transform_rect.take().unwrap_or(src);
        let angle = self.transform_angle;
        // Placed as previewed: the frame's edges rounded to pixels.
        let frame = round_rect(r);
        let (w, h) = (frame.width() as u32, frame.height() as u32);
        let (w, h) = (w.max(1), h.max(1));
        if !fits_max_dimension(vec2(w as f32, h as f32), angle) {
            self.status = t!("select-element-too-large", max = MAX_DIMENSION);
            self.reset_transform();
            return;
        }
        let Some(doc) = &self.doc else { return };
        let old_center = pos2(doc.width() as f32, doc.height() as f32) / 2.0;
        let moved = doc.active_layer().id;
        // A text stays editable: scaled, stretched or rotated, it is
        // rendered again with its new transformation.
        let text = doc.active_layer().text.is_some();
        let at = (frame.min.x as i64, frame.min.y as i64);
        if text {
            let (index, map) = (doc.active, type_tool::text_frame_map(src, frame, angle));
            if let Some(doc) = &mut self.doc {
                doc.checkpoint(t!("menu-edit-free-transform"));
            }
            self.transform_text(index, map);
        }
        let Some(doc) = &mut self.doc else { return };
        if text {
            if self.fit_canvas {
                let corners = [
                    frame.left_top(),
                    frame.right_top(),
                    frame.left_bottom(),
                    frame.right_bottom(),
                ];
                let turned = corners.map(|p| canvas::rotate_around(p, frame.center(), angle));
                doc.fit_canvas_to(to_pixel_rect(round_rect(Rect::from_points(&turned))));
            }
        } else {
            let place = Placement {
                at,
                size: (w, h),
                angle,
            };
            doc.transform_active(
                to_pixel_rect(src),
                place,
                self.resize_filter.filter(),
                self.fit_canvas,
            );
        }
        if frame != src || angle != 0.0 {
            self.remember_transform(src, frame, angle);
        }
        if !self.fit_canvas {
            self.artboards_after_move(&[moved]);
        }
        let turned = if angle != 0.0 {
            t!(
                "status-rotated",
                angle = i18n::short_decimal(f64::from(angle))
            )
        } else {
            String::new()
        };
        if self.fit_canvas {
            self.forget_selection();
            self.status = t!(
                "status-canvas-fit-element",
                width = w,
                height = h,
                rotated = turned
            );
            self.after_geometry_change(frame.center() - old_center);
        } else {
            self.status = t!(
                "status-element-placed",
                width = w,
                height = h,
                x = at.0,
                y = at.1,
                rotated = turned
            );
            // The canvas doesn't change: a crop frame being prepared stays.
            self.reset_transform();
        }
    }

    /// After a geometry change: keeps the content on screen where it was.
    /// `shift` is the move of the canvas center, in pixels.
    fn after_geometry_change(&mut self, shift: Vec2) {
        self.pan += shift * self.screen_zoom();
        self.reset_pending();
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match &self.doc {
            None => "Atelier".to_string(),
            Some(doc) => {
                let name = doc.display_name();
                let star = if doc.modified { "• " } else { "" };
                format!("{star}{name} — Atelier")
            }
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    /// Next (or previous) zoom level, around the point `anchor`.
    fn zoom_step(&mut self, zoom_in: bool, anchor: Pos2, canvas_center: Pos2) {
        let zoom = if zoom_in {
            ZOOM_LEVELS.into_iter().find(|&z| z > self.zoom * 1.001)
        } else {
            ZOOM_LEVELS
                .into_iter()
                .rev()
                .find(|&z| z < self.zoom / 1.001)
        };
        if let Some(zoom) = zoom {
            self.set_zoom_around(zoom, anchor, canvas_center);
        }
    }

    /// Screen points per image pixel (the zoom counts physical pixels).
    fn screen_zoom(&self) -> f32 {
        self.zoom / self.ctx.pixels_per_point()
    }

    fn set_zoom_around(&mut self, new_zoom: f32, anchor: Pos2, canvas_center: Pos2) {
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let rel = anchor - canvas_center - self.pan;
        self.pan += rel - rel * (new_zoom / self.zoom);
        self.zoom = new_zoom;
    }

    /// A modal window is open: it keeps the keyboard to itself (Enter, Esc,
    /// curve arrows…), and nothing must change the image behind it.
    fn modal_open(&self) -> bool {
        self.adjust.is_some()
            || self.layer_style.is_some()
            || self.filters.busy()
            || self.camera_raw.busy()
            || self.liquify.busy()
            || self.smart.busy()
            || self.transform_busy()
            || self.confirm.is_some()
            || self.flatten_confirm
            || self.fill_dialog.is_some()
            || self.retouch.busy()
            || self.retouch.dialog.is_some()
            || self.image_size.is_some()
            || self.canvas_size.is_some()
            || self.rotate_dialog.is_some()
            || self.export.is_some()
            || self.model_prompt.is_some()
            || self.models_modal()
            || self.palette.is_open()
            || self.paint.editor.is_some()
            || self.vector.panel.dialog.is_some()
            || self.colors.picker.is_some()
            || self.rulers.modal_open()
            || self.color.modal_open()
            || self.select_modal()
            || self.artboards.modal()
            || self.new_doc.is_open()
            || self.actions_modal()
            || self.automate.modal()
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let paste = self.paste_released(ctx);
        let cut = self.shift_ctrl_cut(ctx);
        if self.modal_open() || self.automate.running() {
            return;
        }
        // An open menu or drop-down keeps the keyboard too (Esc closes it,
        // and only it), and nothing changes during a mouse gesture (stroke,
        // drag of a selection, of an element, of a layer in the list…).
        if egui::Popup::is_any_open(ctx) || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        // A text field keeps the other keys, Ctrl ones included (its own
        // undo, Ctrl+W…), and the Esc that has just left it: egui drops the
        // focus before this runs.
        if self.typing || ctx.egui_wants_keyboard_input() {
            if let Some(action) = ctx.input_mut(|i| take_shortcut(i, FILE_SHORTCUTS)) {
                self.run(action);
            }
            return;
        }
        // A text typed on the canvas takes the keys.
        if self.typing_on_canvas() {
            self.type_keys(ctx);
            return;
        }
        // A lasso in progress takes Enter, Esc and Backspace; files and zoom
        // still work.
        if self.gesture_keys(ctx) {
            let shortcuts = FILE_SHORTCUTS.into_iter().chain(ZOOM_SHORTCUTS);
            if let Some(action) = ctx.input_mut(|i| take_shortcut(i, shortcuts)) {
                self.run(action);
            }
            return;
        }
        // The Pen's path being drawn, the selected anchors or paths.
        if self.vector_keys(ctx) {
            return;
        }
        if ctx.input_mut(|i| consume_exact(i, palette::SHORTCUT_PALETTE)) {
            self.palette.open();
            return;
        }
        if let Some(n) = ctx.input_mut(channels_panel::channel_shortcut) {
            self.show_channel(n);
            return;
        }
        // Ctrl+Tab / Ctrl+Shift+Tab: next / previous document, like Photoshop.
        let n = self.tab_count();
        if n > 1 {
            let back = Modifiers::COMMAND.plus(Modifiers::SHIFT);
            let step = ctx.input_mut(|i| {
                if i.consume_key(back, Key::Tab) {
                    Some(n - 1)
                } else if i.consume_key(Modifiers::COMMAND, Key::Tab) {
                    Some(1)
                } else {
                    None
                }
            });
            if let Some(step) = step {
                self.switch_tab((self.active_tab + step) % n);
                return;
            }
        }
        let action = ctx.input_mut(|i| {
            let document = [
                (SHORTCUT_CLOSE, Action::CloseDocument),
                (SHORTCUT_REDO, Action::Redo),
                (SHORTCUT_UNDO, Action::Undo),
                (
                    transform::SHORTCUT_AGAIN,
                    Action::Transform(transform::Command::Again),
                ),
                (
                    SHORTCUT_FREE_TRANSFORM,
                    Action::Transform(transform::Command::Free),
                ),
                (SHORTCUT_DUPLICATE, Action::LayerFromSelection),
                (SHORTCUT_NEW_LAYER, Action::NewLayer),
                (SHORTCUT_MERGE_VISIBLE, Action::MergeVisible),
                (SHORTCUT_MERGE_DOWN, Action::MergeDown),
                (SHORTCUT_RESELECT, Action::Reselect),
                (SHORTCUT_INVERT_SELECTION, Action::InvertSelection),
                (SHORTCUT_IMAGE_SIZE, Action::ImageSize),
                (SHORTCUT_FILL_DIALOG, Action::FillDialog),
                (SHORTCUT_DESELECT, Action::Deselect),
                (SHORTCUT_SELECT_ALL, Action::SelectAll),
                (filters_ui::SHORTCUT_LAST_FILTER, Action::LastFilter),
                (camera_raw_ui::SHORTCUT_CAMERA_RAW, Action::CameraRawFilter),
                (liquify_ui::SHORTCUT_LIQUIFY, Action::Liquify),
                (SHORTCUT_GROUP, Action::GroupLayers),
                (SHORTCUT_UNGROUP, Action::Ungroup),
                (SHORTCUT_CLIPPING, Action::ToggleClipping),
                (
                    actions_panel::SHORTCUT_ACTIONS,
                    Action::Automate(actions_panel::Command::Panel),
                ),
                (
                    export_as::SHORTCUT_QUICK_EXPORT,
                    Action::Automate(actions_panel::Command::QuickExport),
                ),
            ];
            let adjustments = ADJUSTMENTS
                .iter()
                .filter_map(|a| Some((*a.shortcut?, Action::Adjust(a.kind))));
            let view = rulers::SHORTCUTS.map(|(s, c)| (s, Action::View(c)));
            let shortcuts = FILE_SHORTCUTS
                .into_iter()
                .chain(ZOOM_SHORTCUTS)
                .chain(document)
                .chain(adjustments)
                .chain(view)
                .chain(select_menu::SHORTCUTS.map(|(s, c)| (s, Action::Select(c))))
                .chain(color_ui::SHORTCUTS.map(|(s, c)| (s, Action::Color(c))));
            take_shortcut(i, shortcuts)
                .or(cut)
                .or_else(|| clipboard_action(i, paste))
        });
        if let Some(action) = action {
            self.run(action);
        }
        if let Some(action) = ctx.input(|i| self.blend_shortcut(i)) {
            self.run(action);
        }
        if let Some(key) = ctx.input_mut(actions_panel::fn_key_press) {
            self.play_key(key);
        }

        // Arrows: move the element by 1 px (10 with Shift).
        let nudge = ctx.input(|i| {
            let step = if i.modifiers.shift { 10 } else { 1 };
            let mut d = (0, 0);
            for (key, dx, dy) in [
                (Key::ArrowLeft, -1, 0),
                (Key::ArrowRight, 1, 0),
                (Key::ArrowUp, 0, -1),
                (Key::ArrowDown, 0, 1),
            ] {
                if i.key_pressed(key) {
                    d.0 += dx * step;
                    d.1 += dy * step;
                }
            }
            d
        });
        if nudge != (0, 0) && self.confirm.is_none() && !self.split_open {
            self.nudge(nudge);
        }
        let delete = ctx.input(|i| {
            key_typed(i, Modifiers::NONE, Key::Delete)
                || key_typed(i, Modifiers::NONE, Key::Backspace)
        });
        if delete && self.selection.is_some() {
            self.run(Action::ClearSelection);
        }
        // Fills, like Photoshop, with Backspace or Delete. (On Windows,
        // egui-winit turns Shift+Delete into a cut: see `clipboard_action`.)
        let fill = ctx.input_mut(|i| {
            let fills = [
                (SHORTCUT_FILL_PRIMARY, Action::FillPrimary),
                (SHORTCUT_FILL_SECONDARY, Action::FillSecondary),
                (SHORTCUT_FILL_PRIMARY_KEEP, Action::FillPrimaryKeep),
                (SHORTCUT_FILL_SECONDARY_KEEP, Action::FillSecondaryKeep),
                (SHORTCUT_FILL_DIALOG_ALT, Action::FillDialog),
            ];
            take_shortcut(
                i,
                fills.into_iter().flat_map(|(s, action)| {
                    [Key::Delete, Key::Backspace]
                        .map(|key| (KeyboardShortcut::new(s.modifiers, key), action))
                }),
            )
        });
        if let Some(fill) = fill {
            self.run(fill);
        }
        let (swap, reset, quick_mask) = ctx.input(|i| {
            (
                key_typed(i, Modifiers::NONE, Key::X),
                key_typed(i, Modifiers::NONE, Key::D),
                key_typed(i, Modifiers::NONE, Key::Q),
            )
        });
        if quick_mask {
            self.run(Action::Select(select_menu::Command::QuickMask));
        }
        if swap {
            std::mem::swap(&mut self.primary, &mut self.secondary);
        }
        if reset {
            (self.primary, self.secondary) = (Color32::BLACK, Color32::WHITE);
        }
        let (tool, bracket, digit, enter, escape) = ctx.input(|i| {
            let tool = Tool::ALL.into_iter().find_map(|t| {
                [Modifiers::NONE, Modifiers::SHIFT]
                    .into_iter()
                    .find(|&m| t.shortcut().is_some_and(|k| key_typed(i, m, k)))
                    .map(|m| (t, m.shift))
            });
            (
                tool,
                bracket_press(i),
                digit_press(i, Modifiers::NONE),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        if let Some((tool, shift)) = tool {
            self.set_tool(self.tool_for_key(tool, shift));
        }
        if let Some((up, modifiers)) = bracket {
            self.bracket(up, modifiers);
        }
        if let Some(digit) = digit {
            let now = ctx.input(|i| i.time);
            self.opacity_digit(digit, now);
        }
        if let Some(digit) = ctx.input(|i| digit_press(i, Modifiers::SHIFT)) {
            let now = ctx.input(|i| i.time);
            self.flow_digit(digit, now);
        }
        if !enter && !escape {
            return;
        }
        // Esc / Enter: Cancel / OK of the topmost tool window, else they
        // give up / apply what is being prepared.
        match self.top_tool_window(ctx) {
            Some(ToolWindow::Split) if enter => self.apply_split(),
            Some(ToolWindow::Split) => self.split_open = false,
            Some(ToolWindow::Transparency) => self.close_transparency(enter),
            Some(ToolWindow::Models) => self.models_window = None,
            Some(ToolWindow::ColorRange) => self.close_color_range(enter),
            Some(ToolWindow::SelectMask) => self.close_select_mask(enter),
            None if enter => self.commit_pending(),
            None => {
                self.reset_pending();
                if self.tool.selects() {
                    self.deselect();
                }
            }
        }
    }

    /// Keyboard focus is for text fields only: Tab or the arrows must not
    /// leave it on a button (the shortcuts would stop, Space would press
    /// it). A clicked number field gets it a frame before its text field
    /// shows. Then remembers whether a text field has the keyboard.
    fn keep_focus_on_text(&mut self, ctx: &egui::Context) {
        if let Some(id) = ctx.memory(|m| m.focused())
            && !ctx.text_edit_focused()
            && !ctx.input(|i| i.pointer.any_click())
        {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        self.typing = ctx.egui_wants_keyboard_input();
    }

    /// The topmost of the open tool windows: they leave the keyboard to the
    /// app, except Esc and Enter.
    fn top_tool_window(&self, ctx: &egui::Context) -> Option<ToolWindow> {
        let open: Vec<ToolWindow> = [
            (ToolWindow::Split, self.split_open),
            (ToolWindow::Transparency, self.transparency.is_some()),
            (ToolWindow::Models, self.models_window.is_some()),
            (ToolWindow::ColorRange, self.sel.color_range.is_some()),
            (ToolWindow::SelectMask, self.sel.select_mask.is_some()),
        ]
        .into_iter()
        .filter_map(|(w, open)| open.then_some(w))
        .collect();
        let shown = ctx.memory(|m| {
            m.layer_ids()
                .filter_map(|l| open.iter().find(|w| l.id == egui::Id::new(w.id())))
                .last()
                .copied()
        });
        // A window opened in this frame isn't shown yet.
        shown.or(open.first().copied())
    }

    /// Shift+Ctrl+X (Liquify) and, on Windows, Shift+Ctrl+Delete (fill with
    /// the background color, preserving transparency) both come from
    /// egui-winit as a cut, without their key: its release tells them apart.
    fn shift_ctrl_cut(&mut self, ctx: &egui::Context) -> Option<Action> {
        let mut action = None;
        ctx.input(|i| {
            for e in &i.events {
                match e {
                    egui::Event::Cut => {
                        let m = i.modifiers;
                        self.cut_key = m.shift && m.command && !m.alt;
                    }
                    egui::Event::Key {
                        key,
                        pressed: false,
                        ..
                    } if self.cut_key => {
                        action = match key {
                            Key::X => Some(Action::Liquify),
                            Key::Delete => Some(Action::FillSecondaryKeep),
                            _ => continue,
                        };
                        self.cut_key = false;
                    }
                    _ => {}
                }
            }
        });
        action
    }

    /// Paste on the release of V: egui-winit swallows the press of Ctrl+V
    /// when the clipboard holds an image (it only reports text). A release
    /// without its plain press is a paste, even if Ctrl was let go first.
    fn paste_released(&mut self, ctx: &egui::Context) -> bool {
        let mut paste = false;
        ctx.input(|i| {
            for e in &i.events {
                if let egui::Event::Key {
                    key: Key::V,
                    pressed,
                    modifiers,
                    ..
                } = e
                {
                    if *pressed {
                        self.plain_v = !modifiers.command;
                    } else {
                        paste |= !self.plain_v;
                        self.plain_v = false;
                    }
                }
            }
        });
        paste
    }

    /// Brackets, like Photoshop: [ ] tool size, Shift hardness, Ctrl layer
    /// order (Shift: all the way down / up), Alt layer below / above.
    fn bracket(&mut self, up: bool, m: Modifiers) {
        if m.command {
            self.run(match (up, m.shift) {
                (true, false) => Action::Raise,
                (false, false) => Action::Lower,
                (true, true) => Action::BringToFront,
                (false, true) => Action::SendToBack,
            });
        } else if m.alt {
            let Some(doc) = &self.doc else { return };
            let target = if up {
                doc.active + 1
            } else {
                doc.active.wrapping_sub(1)
            };
            if target < doc.layers.len() {
                let id = doc.layers[target].id;
                self.layer_set = vec![id];
                self.select_layer(target);
            }
        } else if m.shift {
            let step = if up { 0.25 } else { -0.25 };
            self.hardness = (self.hardness + step).clamp(0.0, 1.0);
            self.status = t!(
                "status-hardness",
                value = format!("{:.0}", self.hardness * 100.0)
            );
        } else {
            self.brush_size = brush_size_step(self.brush_size, up);
            self.status = t!(
                "status-brush-size",
                value = format!("{:.0}", self.brush_size)
            );
        }
    }

    /// Digits, like Photoshop: opacity of the tool (brush, eraser) or of the
    /// active layer (other tools). 1 = 10%, 0 = 100%; two digits typed
    /// quickly give the exact value (4 then 5: 45%, 0 then 5: 5%).
    fn opacity_digit(&mut self, digit: u32, now: f64) {
        let percent = self.typed_percent(digit, now);
        if self.tool_percent(percent, false) {
            return;
        }
        let Some(id) = self.doc.as_ref().map(|d| d.active_layer().id) else {
            return;
        };
        self.record_change(t!("xchg-hist-opacity"), Some((id, "opacity")));
        if let Some(doc) = &mut self.doc {
            let layer = doc.active_layer_mut();
            layer.opacity = percent as f32 / 100.0;
            self.status = t!(
                "status-layer-opacity",
                name = layer.name.as_str(),
                percent = percent
            );
        }
    }

    /// Shift + digits: the flow of the painting tools, like Photoshop.
    fn flow_digit(&mut self, digit: u32, now: f64) {
        let percent = self.typed_percent(digit, now);
        self.tool_percent(percent, true);
    }

    /// Percentage typed with a digit, given the one before (see `opacity_digit`).
    fn typed_percent(&mut self, digit: u32, now: f64) -> u32 {
        match self.last_digit.take() {
            Some((first, at)) if now - at < DIGIT_DELAY => first * 10 + digit,
            _ => {
                self.last_digit = Some((digit, now));
                if digit == 0 { 100 } else { digit * 10 }
            }
        }
    }
}

/// Bracket pressed in this frame: `true` for ], and the modifiers.
/// Shift+[ gives "{" on a QWERTY keyboard: both are accepted.
fn bracket_press(input: &egui::InputState) -> Option<(bool, Modifiers)> {
    input.events.iter().find_map(|e| match e {
        egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => match key {
            Key::OpenBracket | Key::OpenCurlyBracket => Some((false, *modifiers)),
            Key::CloseBracket | Key::CloseCurlyBracket => Some((true, *modifiers)),
            _ => None,
        },
        _ => None,
    })
}

/// Digit typed with exactly `modifiers`. The physical key is read: on an
/// AZERTY keyboard, the digit row gives & é " ' … without Shift.
fn digit_press(input: &egui::InputState, modifiers: Modifiers) -> Option<u32> {
    const DIGITS: [Key; 10] = [
        Key::Num0,
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    input.events.iter().find_map(|e| match e {
        egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            repeat: false,
            modifiers: m,
        } if m.matches_exact(modifiers) => {
            let key = physical_key.unwrap_or(*key);
            DIGITS.iter().position(|&d| d == key).map(|n| n as u32)
        }
        _ => None,
    })
}

/// Next tool size with [ or ], in steps like Photoshop.
fn brush_size_step(size: f32, up: bool) -> f32 {
    let step = |s: f32| match s {
        s if s < 10.0 => 1.0,
        s if s < 100.0 => 10.0,
        s if s < 200.0 => 25.0,
        s if s < 300.0 => 50.0,
        _ => 100.0,
    };
    let next = if up {
        // Rounded to the step: 12 → 20, not 22.
        let st = step(size);
        (size / st).floor() * st + st
    } else {
        let st = step(size - 0.5);
        (size / st).ceil() * st - st
    };
    next.clamp(1.0, 500.0)
}

/// Snap guides, as View › Snap To says: edges and center of the canvas and
/// of the other visible elements (not the ones being moved), ruler guides
/// and grid.
fn snap_targets(doc: &Document, moving: &[usize], view: &rulers::ViewSettings) -> SnapTargets {
    let mut targets = SnapTargets::default();
    if !view.snap {
        return targets;
    }
    if view.snap_bounds {
        targets.add_rect(to_rect(doc.full_rect()));
    }
    for (i, layer) in doc.layers.iter().enumerate() {
        if view.snap_layers
            && !moving.contains(&i)
            && layer.visible
            && let Some(b) = layer.content_bounds()
        {
            targets.add_rect(to_rect(b));
        }
    }
    view.add_targets(&mut targets, doc);
    targets
}

/// Clipboard shortcut of this frame: Ctrl+C, Ctrl+X, or Ctrl+V (`paste`,
/// see `paste_released`). egui-winit passes on other keys as a copy or a
/// cut, with their modifiers still held.
fn clipboard_action(input: &egui::InputState, paste: bool) -> Option<Action> {
    let m = input.modifiers;
    let clipboard = input.events.iter().find_map(|e| match e {
        // Ctrl+Alt+C.
        egui::Event::Copy if m.alt => Some(Action::CanvasSize),
        egui::Event::Copy => Some(Action::Copy),
        // Shift+Ctrl+X, or Shift+Ctrl+Delete on Windows: told apart when the
        // key is released (see `shift_ctrl_cut`).
        egui::Event::Cut if m.shift && m.command => None,
        // On Windows, Shift+Delete (Photoshop: Fill) and Alt+Shift+Delete
        // (fill, preserving transparency).
        egui::Event::Cut if m.shift => cfg!(windows).then_some(if m.alt {
            Action::FillPrimaryKeep
        } else {
            Action::FillDialog
        }),
        egui::Event::Cut => Some(Action::Cut),
        _ => None,
    });
    clipboard.or(paste.then_some(Action::Paste))
}

/// The first of `shortcuts` pressed in this frame (consumed). Modifiers must
/// match exactly: Ctrl+Shift+U (Photoshop: Desaturate) is not Ctrl+U.
fn take_shortcut(
    input: &mut egui::InputState,
    shortcuts: impl IntoIterator<Item = (KeyboardShortcut, Action)>,
) -> Option<Action> {
    shortcuts
        .into_iter()
        .find_map(|(shortcut, action)| consume_exact(input, shortcut).then_some(action))
}

/// Consumes the presses of `shortcut` with exactly its modifiers.
fn consume_exact(input: &mut egui::InputState, shortcut: KeyboardShortcut) -> bool {
    let mut found = false;
    input.events.retain(|e| {
        let hit = matches!(e, egui::Event::Key { key, pressed: true, modifiers, .. }
            if *key == shortcut.logical_key && modifiers.matches_exact(shortcut.modifiers));
        found |= hit;
        !hit
    });
    found
}

/// `key` pressed in this frame with exactly `modifiers`, key repeat aside
/// (holding a tool letter doesn't cycle through its group).
fn key_typed(input: &egui::InputState, modifiers: Modifiers, key: Key) -> bool {
    input.events.iter().any(|e| {
        matches!(e, egui::Event::Key { key: k, pressed: true, repeat: false, modifiers: m, .. }
            if *k == key && m.matches_exact(modifiers))
    })
}

/// Fingerprint of a value (cache keys).
fn hash_of(value: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    h.finish()
}

/// Color in #RRGGBB format.
fn hex_color([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// Integer with a real minus sign (−).
fn signed(v: f32) -> String {
    let v = v.round() as i64;
    if v < 0 {
        format!("−{}", -v)
    } else {
        v.to_string()
    }
}

fn to_rect(r: PixelRect) -> Rect {
    Rect::from_min_size(pos2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32))
}

fn to_pixel_rect(r: Rect) -> PixelRect {
    let r = round_rect(r);
    PixelRect {
        x: r.min.x as i64,
        y: r.min.y as i64,
        w: r.width() as u32,
        h: r.height() as u32,
    }
}

/// Transparency checkerboard (8 screen px tiles), limited to the visible part.
fn draw_checker(painter: &Painter, rect: Rect) {
    const TILE: f32 = 8.0;
    let visible = rect.intersect(painter.clip_rect());
    if !visible.is_positive() {
        return;
    }
    let mut mesh = egui::epaint::Mesh::default();
    mesh.add_colored_rect(visible, ui_kit::color::CHECKER_A);
    // Tiles aligned on the rectangle's corner, only the dark ones are added.
    let first = ((visible.min - rect.min) / TILE).floor();
    let last = ((visible.max - rect.min) / TILE).ceil();
    for j in first.y as i64..last.y as i64 {
        for i in first.x as i64..last.x as i64 {
            if (i + j) % 2 == 1 {
                let min = rect.min + vec2(i as f32, j as f32) * TILE;
                let cell = Rect::from_min_size(min, Vec2::splat(TILE)).intersect(visible);
                mesh.add_colored_rect(cell, ui_kit::color::CHECKER_B);
            }
        }
    }
    painter.add(mesh);
}

/// Brings an angle into ]-180°, 180°].
fn normalize_angle(a: f32) -> f32 {
    let a = (a + 180.0).rem_euclid(360.0) - 180.0;
    if a <= -180.0 { 180.0 } else { a }
}

/// Does an element of `size`, turned by `angle` degrees, fit in a layer?
fn fits_max_dimension(size: Vec2, angle: f32) -> bool {
    let (w, h) = rotated_size(size.x, size.y, angle);
    w.round() <= MAX_DIMENSION as f32 && h.round() <= MAX_DIMENSION as f32
}

impl eframe::App for AtelierApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_dropped_files(&ctx);
        self.poll_subject(&ctx);
        self.poll_select_job();
        self.poll_downloads(&ctx);
        self.poll_object_select();
        self.play_actions();
        self.run_batch();
        self.poll_smart(&ctx);
        self.handle_shortcuts(&ctx);
        self.update_title(&ctx);

        use ui_kit::color;
        // Panels draw their own separator line on their inner edge.
        egui::Panel::top("title_bar")
            .exact_size(40.0)
            .frame(
                egui::Frame::new()
                    .fill(color::TITLE)
                    .inner_margin(egui::Margin {
                        left: 10,
                        right: 0,
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::top("options_bar")
            .frame(
                egui::Frame::new()
                    .fill(color::OPTIONS)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                // 46 px with the margins; wraps when the window is too narrow.
                ui.set_min_height(28.0);
                ui.horizontal_wrapped(|ui| self.options_bar(ui));
            });
        egui::Panel::bottom("status_bar")
            .exact_size(28.0)
            .frame(
                egui::Frame::new()
                    .fill(color::TITLE)
                    .inner_margin(egui::Margin::symmetric(12, 0)),
            )
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("toolbar")
            .resizable(false)
            .exact_size(52.0)
            .frame(
                egui::Frame::new()
                    .fill(color::BG1)
                    .inner_margin(egui::Margin {
                        left: 8,
                        right: 8,
                        top: 8,
                        bottom: 12,
                    }),
            )
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::right("side_panel")
            .resizable(false)
            .exact_size(312.0)
            .frame(egui::Frame::new().fill(color::BG2))
            .show(ui, |ui| self.right_panel(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui));

        // Closing the window with unsaved changes: asks for each document,
        // one after the other (see `confirm_window`).
        if ctx.input(|i| i.viewport().close_requested())
            && let Some(i) = self.first_modified_tab()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.switch_tab(i);
            self.confirm = Some(Confirm::Close);
        }
        self.transparency_window(&ctx);
        self.adjust_window(&ctx);
        self.layer_style_window(&ctx);
        self.filter_window(&ctx);
        self.camera_raw_window(&ctx);
        self.smart_windows(&ctx);
        self.liquify_window(&ctx);
        self.transform_window(&ctx);
        self.fill_window(&ctx);
        self.retouch_windows(&ctx);
        self.image_size_window(&ctx);
        self.canvas_size_window(&ctx);
        self.rotate_window(&ctx);
        self.models_window(&ctx);
        self.model_prompt_window(&ctx);
        self.export_window(&ctx);
        self.confirm_window(&ctx);
        self.flatten_confirm_window(&ctx);
        self.split_window(&ctx);
        self.select_windows(&ctx);
        self.palette_window(&ctx);
        self.gradient_editor_window(&ctx);
        self.swatches_window(&ctx);
        self.history_window(&ctx);
        self.actions_windows(&ctx);
        self.automate_windows(&ctx);
        self.channels_window(&ctx);
        self.vector_windows(&ctx);
        self.new_guide_window(&ctx);
        self.grid_preferences_window(&ctx);
        self.color_windows(&ctx);
        self.artboard_windows(&ctx);
        self.new_document_window(&ctx);
        // Last: it may open over another dialog.
        self.color_picker_window(&ctx);
        titlebar::resize_grips(&ctx);
        self.keep_focus_on_text(&ctx);
    }
}

#[cfg(test)]
mod doc_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Capture of the interface (offscreen render): `cargo test capture -- --ignored`.
    /// The image is written to the file given by `ATELIER_CAPTURE`, in the
    /// language given by `ATELIER_LANG` (English by default).
    #[test]
    #[ignore = "needs a graphics renderer"]
    fn capture() {
        let out = std::env::var("ATELIER_CAPTURE").unwrap_or_else(|_| "capture.png".into());
        if let Some(lang) = Lang::ALL
            .into_iter()
            .find(|l| std::env::var("ATELIER_LANG").is_ok_and(|v| v == l.code()))
        {
            i18n::set_lang(lang);
        }
        let tool = match std::env::var("ATELIER_TOOL").as_deref() {
            Ok("transform") => Tool::Transform,
            Ok("brush") => Tool::Brush,
            Ok("text") => Tool::Text,
            Ok("select") => Tool::Select,
            Ok("ellipse") => Tool::Ellipse,
            Ok("free-lasso") => Tool::FreeLasso,
            Ok("poly-lasso") => Tool::PolyLasso,
            Ok("zoom") => Tool::Zoom,
            Ok("lasso") => Tool::Lasso,
            Ok("wand") => Tool::MagicWand,
            Ok("objsel") => Tool::ObjectSelect,
            Ok("eraser") => Tool::Eraser,
            Ok("gradient") => Tool::Gradient,
            Ok("dodge") => Tool::Dodge,
            Ok("sponge") => Tool::Sponge,
            Ok("blur") => Tool::Blur,
            Ok("smudge") => Tool::Smudge,
            Ok("bucket") => Tool::Bucket,
            Ok("eyedropper") => Tool::Eyedropper,
            Ok("hand") => Tool::Hand,
            _ => Tool::Canvas,
        };
        // Window size (`ATELIER_WIDTH` × `ATELIER_HEIGHT`, 1600 × 1250 by default).
        let size = |var, default| {
            std::env::var(var)
                .ok()
                .and_then(|w| w.parse().ok())
                .unwrap_or(default)
        };
        let mut harness = egui_kittest::Harness::builder()
            .with_size([
                size("ATELIER_WIDTH", 1600.0),
                size("ATELIER_HEIGHT", 1250.0),
            ])
            .wgpu()
            .build_eframe(|cc| {
                let mut app = AtelierApp::new(cc, None);
                // A 90 × 95 logo in the middle of a 512 × 512 canvas.
                let mut logo =
                    image::RgbaImage::from_pixel(90, 95, image::Rgba([70, 110, 160, 255]));
                for (x, y, p) in logo.enumerate_pixels_mut() {
                    if (x / 10 + y / 10) % 3 == 0 {
                        *p = image::Rgba([230, 230, 230, 255]);
                    }
                }
                let mut doc = Document::new(logo, Some("logo.webp".into()));
                doc.resize_canvas(-211, -208, 512, 512);
                doc.modified = true;
                if std::env::var("ATELIER_FX").is_ok() {
                    let fx = &mut doc.active_layer_mut().effects;
                    fx.stroke.enabled = true;
                    fx.shadow.enabled = true;
                    fx.shadow.dx = 12.0;
                    fx.shadow.dy = 12.0;
                }
                app.doc = Some(doc);
                // Locked layer, then a text layer above it (active).
                if std::env::var("ATELIER_LOCKED").is_ok()
                    && let Some(doc) = &mut app.doc
                {
                    doc.active_layer_mut().locked = true;
                }
                // Being typed; `paragraph`: in a box, with styled words.
                match std::env::var("ATELIER_TEXT").as_deref() {
                    Ok("paragraph") => {
                        app.create_text(pos2(40.0, 40.0), Some(vec2(330.0, 150.0)));
                        app.restyle_selection(|s| s.size = 30.0);
                        app.type_text("Atelier edits images like a pro: type on the canvas.");
                        if let Some(e) = &mut app.type_tool.edit {
                            (e.anchor, e.caret) = (8, 13);
                        }
                        app.restyle_selection(|s| {
                            (s.color, s.underline, s.weight) = ([200, 40, 40, 255], true, 700)
                        });
                    }
                    Ok(_) => {
                        app.create_text(pos2(120.0, 360.0), None);
                        app.type_text("Atelier");
                    }
                    Err(_) => {}
                }
                // A photo-like image instead of the logo (Object Selection).
                if std::env::var("ATELIER_PHOTO").is_ok() {
                    let photo = object_select::demo_photo();
                    app.doc = Some(Document::new(photo, Some("C:/img/photo.jpg".into())));
                }
                if std::env::var("ATELIER_NODOC").is_ok() {
                    app.doc = None;
                }
                // Start screen: no document, demo recent files.
                if std::env::var("ATELIER_EMPTY").is_ok() {
                    app.doc = None;
                    app.recent = demo_recent_files();
                }
                app.tool = tool;
                // Two more documents in tabs, the second one active.
                if std::env::var("ATELIER_TABS").is_ok() {
                    for (name, color) in [
                        ("vacances-2024-plage.jpg", [200, 140, 80, 255]),
                        (
                            "affiche-concert-tres-long-nom-de-fichier.png",
                            [40, 160, 120, 255],
                        ),
                    ] {
                        let image = image::RgbaImage::from_pixel(300, 200, image::Rgba(color));
                        app.add_tab(Document::new(image, Some(format!("C:/img/{name}").into())));
                    }
                    app.switch_tab(1);
                }
                // Foreground the same grey as the bar: it must still stand out.
                if std::env::var("ATELIER_GREY_COLOR").is_ok() {
                    app.primary = ui_kit::color::BG1;
                }
                // Adjustment open, with demo values.
                let kind = match std::env::var("ATELIER_ADJ").as_deref() {
                    Ok("bc") => Some(AdjustKind::BrightnessContrast),
                    Ok("levels") => Some(AdjustKind::Levels),
                    Ok("curves") => Some(AdjustKind::Curves),
                    Ok("vibrance") => Some(AdjustKind::Vibrance),
                    Ok("hue") => Some(AdjustKind::HueSaturation),
                    Ok("balance") => Some(AdjustKind::ColorBalance),
                    _ => None,
                };
                // BiRefNet downloading, at that fraction (e.g. "0.42").
                if let Some(f) = std::env::var("ATELIER_DOWNLOADING")
                    .ok()
                    .and_then(|f| f.parse().ok())
                {
                    app.downloads.simulate(crate::ai::models::Model::Subject, f);
                }
                match std::env::var("ATELIER_DIALOG").as_deref() {
                    Ok("fill") => app.run(Action::FillDialog),
                    Ok("size") => app.run(Action::ImageSize),
                    Ok("transparent") => app.run(Action::MakeTransparent),
                    Ok("split") => app.run(Action::Split),
                    Ok("canvas") => app.run(Action::CanvasSize),
                    Ok("rotate") => app.run(Action::RotateArbitrary),
                    Ok("models") => app.run(Action::AiModels),
                    Ok("models-delete") => {
                        app.run(Action::AiModels);
                        app.models_window = Some(models_window::ModelsWindow::confirming(
                            crate::ai::models::Model::Objects,
                        ));
                    }
                    Ok("model-prompt") => {
                        app.model_prompt = Some(models_window::ModelPrompt {
                            then: models_window::AfterDownload::ObjectSelection,
                        });
                    }
                    Ok("model-prompt-subject") => {
                        app.model_prompt = Some(models_window::ModelPrompt {
                            then: models_window::AfterDownload::SelectSubject,
                        });
                    }
                    Ok("confirm") => app.confirm = Some(Confirm::CloseDocument),
                    Ok("gradient") => {
                        let mut g = crate::paint::gradient::Preset::Spectrum.gradient();
                        g.opacities[1].opacity = 0.3;
                        app.paint.editor = Some(gradient_editor::GradientEditor::new(g));
                    }
                    Ok("picker") => ui_kit::request_picker(
                        &cc.egui_ctx,
                        ui_kit::PickerRequest {
                            target: egui::Id::new("capture"),
                            color: Color32::from_rgb(70, 110, 160),
                            title: t!("uip-picker-foreground-title"),
                        },
                    ),
                    Ok("new-guide") => app.run(Action::View(rulers::Command::NewGuide)),
                    Ok("guides-grid") => app.run(Action::View(rulers::Command::Preferences)),
                    Ok("swatches") => app.run(Action::Swatches),
                    Ok("history-channels") => {
                        app.run(Action::NewLayer);
                        app.run(Action::Duplicate);
                        app.run(Action::History);
                        app.run(Action::Channels);
                        if let Some(doc) = &mut app.doc {
                            doc.new_channel();
                        }
                    }
                    Ok(
                        what @ ("color-range"
                        | "select-mask"
                        | "feather"
                        | "save-selection"
                        | "transform-selection"
                        | "quick-mask"),
                    ) => app.capture_select(what),
                    Ok(
                        what @ ("actions" | "actions-buttons" | "batch" | "image-processor"
                        | "export-as" | "fit-image"),
                    ) => app.capture_automate(what),
                    Ok(
                        what @ ("color-settings" | "assign-profile" | "convert-profile"
                        | "proof-setup" | "proof-colors"),
                    ) => app.capture_color(what),
                    _ => {}
                }
                // Groups, masks, clipping, adjustment and fill layers
                // (`ATELIER_LAYERS`; `style`: the Layer Style window).
                if let Ok(what) = std::env::var("ATELIER_LAYERS") {
                    layer_style::capture_layers(&mut app, &what);
                }
                // Rulers, two guides and the grid.
                if std::env::var("ATELIER_RULERS").is_ok()
                    && let Some(doc) = &mut app.doc
                {
                    use crate::guides::{Guide, Orientation};
                    doc.guides = vec![
                        Guide::new(Orientation::Vertical, 256.0),
                        Guide::new(Orientation::Horizontal, 160.0),
                    ];
                    let view = &mut app.rulers.settings;
                    (view.rulers, view.grid) = (true, true);
                }
                // Command palette open, with the query given (may be empty).
                if let Ok(query) = std::env::var("ATELIER_PALETTE") {
                    app.palette.open();
                    app.palette.query = query;
                }
                if std::env::var("ATELIER_INVERTED").is_ok() {
                    app.selection = Some(Selection {
                        rect: PixelRect {
                            x: 150,
                            y: 150,
                            w: 200,
                            h: 180,
                        },
                        mask: None,
                        inverted: true,
                    });
                }
                if let Some(kind) = kind {
                    app.run(Action::Adjust(kind));
                    let dialog = app.adjust.as_mut().expect("adjustment open");
                    match &mut dialog.adjustment {
                        Adjustment::BrightnessContrast(bc) => {
                            (bc.brightness, bc.contrast) = (0.4, 0.6);
                        }
                        Adjustment::Levels(lv) => {
                            (lv.rgb.in_black, lv.rgb.in_white) = (40, 220);
                            lv.rgb.gamma = 1.4;
                        }
                        Adjustment::Curves(cv) => {
                            cv.rgb.insert(64.0, 40.0);
                            dialog.point = cv.rgb.insert(192.0, 215.0);
                        }
                        Adjustment::Vibrance(v) => v.vibrance = 0.6,
                        Adjustment::HueSaturation(hs) => hs.hue = 120.0,
                        Adjustment::ColorBalance(cb) => cb.midtones = [0.6, 0.0, -0.4],
                        _ => {}
                    }
                }
                app
            });
        harness.run_steps(4);
        object_select::settle(&mut harness);
        // A menu of the title bar open ("Edit", in the capture's language).
        if let Ok(menu) = std::env::var("ATELIER_MENU") {
            use egui_kittest::kittest::Queryable as _;
            harness.get_by_label(&menu).click();
            harness.run_steps(4);
        }
        // A click at `ATELIER_CLICK` ("x,y"), e.g. to open a drop-down list.
        if let Some((x, y)) = std::env::var("ATELIER_CLICK").ok().and_then(|p| {
            let (x, y) = p.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        }) {
            let pos = pos2(x, y);
            harness.hover_at(pos);
            harness.run_steps(1);
            harness.drag_at(pos);
            harness.run_steps(1);
            harness.event(egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            harness.run_steps(4);
        }
        // The pointer resting at `ATELIER_HOVER` ("x,y").
        if let Some((x, y)) = std::env::var("ATELIER_HOVER").ok().and_then(|p| {
            let (x, y) = p.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        }) {
            harness.hover_at(pos2(x, y));
            harness.run_steps(4);
        }
        // A drag from "x1,y1" to "x2,y2" (`ATELIER_DRAG`), e.g. to move a dialog.
        if let Some([x1, y1, x2, y2]) = std::env::var("ATELIER_DRAG").ok().and_then(|p| {
            let v: Vec<f32> = p.split(',').filter_map(|n| n.parse().ok()).collect();
            <[f32; 4]>::try_from(v).ok()
        }) {
            let (from, to) = (pos2(x1, y1), pos2(x2, y2));
            harness.hover_at(from);
            harness.run_steps(1);
            harness.drag_at(from);
            harness.run_steps(1);
            for i in 1..=8 {
                harness.hover_at(from.lerp(to, i as f32 / 8.0));
                harness.run_steps(1);
            }
            harness.event(egui::Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            });
            harness.run_steps(4);
        }
        object_select::settle(&mut harness);
        let image = harness.render().expect("render");
        image.save(&out).expect("writing the capture");
    }

    /// Two small images written to the temporary folder, as recent files.
    fn demo_recent_files() -> crate::recent::Recent {
        let dir = std::env::temp_dir().join("atelier-capture");
        std::fs::create_dir_all(&dir).expect("temporary folder");
        let mut paths = Vec::new();
        for (name, (w, h), color) in [
            ("LOGO-VTL.png", (792, 261), [4, 49, 59]),
            ("vacances-2024-plage.jpg", (640, 480), [200, 140, 80]),
        ] {
            let path = dir.join(name);
            image::RgbImage::from_pixel(w, h, image::Rgb(color))
                .save(&path)
                .expect("demo image");
            paths.push(path);
        }
        crate::recent::Recent::with_paths(paths)
    }

    /// The adjustments menu lists each adjustment once, under its window title.
    #[test]
    fn adjustment_menu_matches_titles() {
        for (i, a) in ADJUSTMENTS.iter().enumerate() {
            assert_eq!(a.kind.initial().kind(), a.kind);
            // "…" when a window opens.
            let title = a.kind.initial().title();
            let label = a.label();
            assert!(label == title || label == format!("{title}…"));
            assert!(ADJUSTMENTS[..i].iter().all(|b| b.kind != a.kind));
        }
    }

    /// Every non-ASCII character of the interface must exist in egui's
    /// proportional fonts, otherwise it shows as an empty box.
    #[test]
    fn ui_symbols_exist_in_egui_fonts() {
        let fonts: Vec<ttf_parser::Face> = [
            epaint_default_fonts::UBUNTU_LIGHT,
            epaint_default_fonts::NOTO_EMOJI_REGULAR,
            epaint_default_fonts::EMOJI_ICON,
            egui_phosphor::Variant::Regular.font_bytes(),
        ]
        .into_iter()
        .map(|data| ttf_parser::Face::parse(data, 0).expect("valid embedded font"))
        .collect();
        let sources = [
            include_str!("mod.rs"),
            include_str!("menus.rs"),
            include_str!("options_bar.rs"),
            include_str!("right_panel.rs"),
            include_str!("status_bar.rs"),
            include_str!("titlebar.rs"),
            include_str!("palette.rs"),
            include_str!("toolbar.rs"),
            include_str!("canvas.rs"),
            include_str!("dialogs.rs"),
            include_str!("filters_ui.rs"),
            include_str!("camera_raw_ui.rs"),
            include_str!("select_tools.rs"),
            include_str!("welcome.rs"),
        ];
        // Test code is not displayed: only what precedes it is looked at.
        let code = sources.iter().flat_map(|source| {
            let end = source
                .find("#[cfg(test)]\nmod tests")
                .unwrap_or(source.len());
            source[..end]
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
        });
        // Interface text lives in the translation files (`#`: comments).
        let translations = [
            include_str!("../../locales/en.ftl"),
            include_str!("../../locales/fr.ftl"),
        ];
        let texts = translations
            .iter()
            .flat_map(|ftl| ftl.lines())
            .filter(|l| !l.trim_start().starts_with('#'));
        let missing: std::collections::BTreeSet<char> = code
            .chain(texts)
            .flat_map(str::chars)
            .filter(|c| !c.is_ascii() && fonts.iter().all(|f| f.glyph_index(*c).is_none()))
            .collect();
        assert!(
            missing.is_empty(),
            "characters missing from egui's fonts: {missing:?}"
        );
    }
}
