//! Actions, like Photoshop's: commands recorded with their final settings
//! (not the mouse gestures: brush strokes aren't recorded), grouped in
//! sets and played back on other documents by the Actions panel, File ›
//! Automate › Batch and the Image Processor. Kept in the settings folder
//! (`actions.json`), in Atelier's own JSON format.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::export::ExportOptions;
use crate::filters::Filter;
use crate::i18n::t;
use crate::pixel_ops::{Adjustment, HueSaturation};

/// Version of the actions files.
const VERSION: u32 = 1;

/// The sets of the Actions panel.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    pub sets: Vec<Set>,
}

/// A set of actions (a folder of the panel).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Set {
    pub name: String,
    pub actions: Vec<Sequence>,
    /// Shown open in the panel.
    #[serde(skip)]
    pub expanded: bool,
}

/// An action: its steps, played in order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub name: String,
    /// Function key that plays it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<FnKey>,
    pub steps: Vec<Step>,
    #[serde(skip)]
    pub expanded: bool,
}

/// F2 to F12, with Shift and/or Ctrl.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FnKey {
    /// 2 to 12.
    pub number: u8,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
}

/// A step of an action: a command and its settings; skipped when not
/// `enabled`; with `dialog` (modal control), its dialog opens, filled with
/// the settings, for them to be changed before it applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    #[serde(rename = "do", with = "lenient")]
    pub command: Recorded,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub dialog: bool,
    /// Its settings listed in the panel.
    #[serde(skip)]
    pub expanded: bool,
}

fn yes() -> bool {
    true
}

impl Step {
    pub fn new(command: Recorded) -> Self {
        Self {
            command,
            enabled: true,
            dialog: false,
            expanded: false,
        }
    }

    fn with_dialog(command: Recorded) -> Self {
        Self {
            dialog: true,
            ..Self::new(command)
        }
    }
}

/// A command as recorded, with its final settings. Pixel sizes are those
/// of the document it was recorded on.
// An action has a few steps: an adjustment's curves cost little.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Recorded {
    ImageSize {
        width: u32,
        height: u32,
        resample: Resample,
    },
    /// File › Automate › Fit Image: the image scaled to fit in the box.
    FitImage {
        width: u32,
        height: u32,
        dont_enlarge: bool,
    },
    /// With `relative`, the size added (or removed). `anchor`: column and
    /// row (0 to 2) where the image stays; `extension`: color of the added
    /// area (`None`: transparent).
    CanvasSize {
        width: i64,
        height: i64,
        relative: bool,
        anchor: [u8; 2],
        extension: Option<Contents>,
    },
    /// Image Rotation, in degrees clockwise.
    RotateCanvas {
        angle: f32,
    },
    FlipCanvas {
        horizontal: bool,
    },
    /// The Crop tool: x, y, width, height (canvas pixels); its turn
    /// (degrees), Delete Cropped Pixels, W × H × Resolution.
    Crop {
        rect: [i64; 4],
        #[serde(default)]
        angle: f32,
        #[serde(default)]
        delete: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<(u32, u32)>,
        #[serde(default)]
        resolution: f32,
    },
    /// Image › Adjustments, on the active layer.
    Adjust {
        #[serde(with = "adjustment")]
        adjustment: Adjustment,
    },
    /// The Filter menu, on the active layer.
    Filter {
        filter: Filter,
    },
    /// Filter › Camera Raw Filter, on the active layer.
    CameraRaw {
        settings: crate::camera_raw::Settings,
    },
    /// Edit › Fill.
    Fill {
        contents: Contents,
        /// For `Contents::Color`.
        color: [u8; 3],
        /// 0..1.
        opacity: f32,
        preserve_transparency: bool,
        /// For `Contents::Pattern`: which one of the library.
        #[serde(default)]
        pattern: Option<u64>,
    },
    /// Edit › Define Pattern, from the canvas or the selection.
    DefinePattern {
        name: String,
    },
    /// Layer › New Fill Layer › Pattern: which one of the library.
    NewPatternFill {
        pattern: u64,
    },
    /// Layer › New Fill Layer › Solid Color.
    NewSolidFill {
        color: [u8; 3],
    },
    /// Layer › New Fill Layer › Gradient, from one color to the other.
    NewGradientFill {
        from: [u8; 3],
        to: [u8; 3],
    },
    /// Paths panel › Fill Path, on the path the panel works on then; the
    /// contents as Edit › Fill's.
    FillPath {
        contents: Contents,
        /// For `Contents::Color`.
        color: [u8; 3],
        /// 0..1.
        opacity: f32,
        preserve_transparency: bool,
        #[serde(default)]
        pattern: Option<u64>,
        /// Pixels.
        feather: f32,
        antialias: bool,
    },
    /// Paths panel › Stroke Path, with the Brush (or the Eraser) as it is
    /// set when played.
    StrokePath {
        eraser: bool,
        pressure: bool,
    },
    /// Edit › Fade, of the step before it.
    Fade {
        /// 0..1.
        opacity: f32,
        /// A blend mode's name.
        mode: String,
    },
    /// Opacity and/or blend mode (a blend mode's name) of the selected
    /// layers, as the Layers panel, the digits or Shift++ set them.
    LayerLook {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opacity: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
    },
    /// Edit › Stroke.
    Stroke {
        width: u32,
        color: [u8; 3],
        /// "Inside", "Center" or "Outside".
        location: String,
        /// A blend mode's name.
        mode: String,
        /// 0..1.
        opacity: f32,
        preserve_transparency: bool,
    },
    /// A marquee: x, y, width, height, in pixels or in % of the canvas.
    Select {
        shape: Shape,
        rect: [f64; 4],
        #[serde(default)]
        percent: bool,
        mode: Combine,
        feather: f32,
        anti_alias: bool,
    },
    SelectAll,
    Deselect,
    Reselect,
    InvertSelection,
    /// Select › Color Range: the colors sampled (CIE Lab; `add` false: taken
    /// out of the range), Fuzziness (0 to 200), Invert; within the
    /// selection when there is one.
    ColorRange {
        samples: Vec<RangeColor>,
        fuzziness: f32,
        invert: bool,
    },
    /// Select › Modify, by `amount` px.
    ModifySelection {
        modify: Modify,
        amount: f32,
        at_bounds: bool,
    },
    /// Free Transform: where the corners of the transformed element's box
    /// go (top-left first, clockwise), in units of that box (0,0 its
    /// top-left corner, 1,1 its bottom-right one): a scale, a turn, a skew
    /// or a distortion, of whatever is transformed when played. A frame
    /// without distortion is also kept as its move, scale, turn and skew,
    /// which are played instead: a turn stays a turn on a box of another
    /// shape (the corners alone would stretch it).
    Transform {
        corners: [[f64; 2]; 4],
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frame: Option<Frame>,
    },
    /// File › Place Embedded or Place Linked: the file, fitted and
    /// centered as when placed, then its Free Transform (as `Transform`).
    Place {
        path: PathBuf,
        #[serde(default)]
        linked: bool,
        corners: [[f64; 2]; 4],
    },
    /// Layer › Lock Layers: these locks on or off, whichever the layer had
    /// (the command toggles; the step says what it ended as).
    Locks {
        #[serde(default)]
        transparent: bool,
        #[serde(default)]
        pixels: bool,
        #[serde(default)]
        position: bool,
        #[serde(default)]
        all: bool,
        on: bool,
    },
    /// A menu command without settings.
    Menu {
        item: MenuItem,
    },
    /// File › Export › Export As, into `folder` (`None`: the document's),
    /// named after the document.
    Export {
        options: ExportOptions,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder: Option<PathBuf>,
    },
    /// Filter › Liquify: its mesh (`liquify::Mesh::to_bytes`), stretched to
    /// the layer when played.
    Liquify {
        #[serde(with = "hex")]
        mesh: Vec<u8>,
    },
    /// Edit › Convert to Profile: the profile (its ICC bytes), the intent,
    /// black point compensation.
    ConvertToProfile {
        #[serde(with = "hex")]
        profile: Vec<u8>,
        intent: crate::cms::Intent,
        bpc: bool,
    },
    /// Edit › Assign Profile (`None`: Don't Color Manage).
    AssignProfile {
        #[serde(with = "hex_option")]
        profile: Option<Vec<u8>>,
    },
    /// Image › Mode › Grayscale.
    Grayscale,
    /// Insert Stop: a message, and whether playing can go on from it.
    Stop {
        message: String,
        allow_continue: bool,
    },
    /// A command this version doesn't know (from a newer Atelier), kept as
    /// it was read.
    #[serde(skip)]
    Unknown(serde_json::Value),
}

/// Edit › Fill contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Contents {
    Foreground,
    Background,
    Color,
    ContentAware,
    Pattern,
    Black,
    Gray,
    White,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resample {
    Nearest,
    Bilinear,
    Bicubic,
    Lanczos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shape {
    Rectangle,
    Ellipse,
}

/// How a selection combines with the current one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Combine {
    New,
    Add,
    Subtract,
    Intersect,
}

/// Select › Modify.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Modify {
    Border,
    Smooth,
    Expand,
    Contract,
    Feather,
}

/// Menu commands without settings that actions record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuItem {
    AutoTone,
    AutoContrast,
    AutoColor,
    NewLayer,
    DuplicateLayer,
    DeleteLayer,
    LayerViaCopy,
    MergeDown,
    MergeVisible,
    FlattenImage,
    BringForward,
    SendBackward,
    BringToFront,
    SendToBack,
    NewGroup,
    GroupLayers,
    UngroupLayers,
    ClippingMask,
    RevealAllMask,
    HideAllMask,
    RevealSelectionMask,
    HideSelectionMask,
    DeleteMask,
    ApplyMask,
    Rasterize,
    Copy,
    Cut,
    Paste,
    Clear,
    CropToSelection,
    CropToContent,
    ConvertToSmartObject,
    ConvertForSmartFilters,
    LayerViaCut,
    StampVisible,
    Desaturate,
    CopyMerged,
    SelectSubject,
}

/// A color of Color Range's (see `Recorded::ColorRange`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RangeColor {
    pub lab: [f32; 3],
    pub add: bool,
}

/// A Free Transform without distortion, rebuilt on the box of what it
/// plays on: its center moved by `offset`, sized by `scale` (both in
/// fractions of the box's width and height; a negative scale flips),
/// skewed by `skew` then turned by `angle` (degrees, clockwise on screen).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub offset: [f64; 2],
    pub scale: [f64; 2],
    pub angle: f64,
    pub skew: f64,
}

impl Frame {
    /// The frame of `corners` (in units of a `w` × `h` box); `None` for a
    /// distortion or a perspective (not a parallelogram), or a flat one.
    pub fn of(corners: [[f64; 2]; 4], w: f64, h: f64) -> Option<Frame> {
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        let p = corners.map(|[u, v]| [u * w, v * h]);
        let tolerance = 1e-6 * w.max(h);
        let parallelogram = (0..2).all(|i| (p[0][i] + p[2][i] - p[1][i] - p[3][i]).abs() <= tolerance);
        if !parallelogram {
            return None;
        }
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1]];
        let e2 = [p[3][0] - p[0][0], p[3][1] - p[0][1]];
        let angle = e1[1].atan2(e1[0]);
        let (sin, cos) = angle.sin_cos();
        // The left edge turned back: its lean (the skew) and its height.
        let (along, across) = (e2[0] * cos + e2[1] * sin, e2[1] * cos - e2[0] * sin);
        if across.abs() <= tolerance || e1[0].hypot(e1[1]) <= tolerance {
            return None;
        }
        let center = [(p[0][0] + p[2][0]) / 2.0, (p[0][1] + p[2][1]) / 2.0];
        Some(Frame {
            offset: [center[0] / w - 0.5, center[1] / h - 0.5],
            scale: [e1[0].hypot(e1[1]) / w, across / h],
            angle: angle.to_degrees(),
            skew: (along / across).atan().to_degrees(),
        })
    }

    /// Its corners on a `w` × `h` box, in units of that box.
    pub fn corners(&self, w: f64, h: f64) -> [[f64; 2]; 4] {
        let center = [(0.5 + self.offset[0]) * w, (0.5 + self.offset[1]) * h];
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let lean = self.skew.to_radians().tan();
        [[-0.5, -0.5], [0.5, -0.5], [0.5, 0.5], [-0.5, 0.5]].map(|[u, v]| {
            let y = self.scale[1] * v * h;
            let x = self.scale[0] * u * w + lean * y;
            [
                (center[0] + x * cos - y * sin) / w,
                (center[1] + x * sin + y * cos) / h,
            ]
        })
    }
}

/// Unknown commands are read as they are, and written back the same.
mod lenient {
    use super::Recorded;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(r: &Recorded, s: S) -> Result<S::Ok, S::Error> {
        match r {
            Recorded::Unknown(value) => value.serialize(s),
            r => r.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Recorded, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Ok(Recorded::deserialize(&value).unwrap_or(Recorded::Unknown(value)))
    }
}

impl Recorded {
    /// An unknown step whose command (and the names it holds) this version
    /// knows: its settings were damaged, not written by a newer version.
    pub fn damaged(&self) -> bool {
        let Recorded::Unknown(value) = self else {
            return false;
        };
        matches!(Recorded::deserialize(value), Err(e) if !e.to_string().starts_with("unknown variant"))
    }
}

/// Adjustments in the form adjustment layers are saved in.
mod adjustment {
    use crate::pixel_ops::Adjustment;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    pub fn serialize<S: Serializer>(a: &Adjustment, s: S) -> Result<S::Ok, S::Error> {
        crate::document::adjustment_to_json(a).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Adjustment, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        crate::document::adjustment_from_json(&value).ok_or_else(|| D::Error::custom("adjustment"))
    }
}

/// Bytes (meshes, profiles) as a hexadecimal string.
mod hex {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        super::from_hex(&text).ok_or_else(|| D::Error::custom("hexadecimal bytes"))
    }
}

mod hex_option {
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match bytes {
            Some(b) => s.serialize_some(&super::to_hex(b)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            Some(text) => super::from_hex(&text)
                .map(Some)
                .ok_or_else(|| D::Error::custom("hexadecimal bytes")),
            None => Ok(None),
        }
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// What an actions file holds.
#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    sets: Vec<Set>,
}

/// Why an actions file could not be read or written.
#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    /// Not an actions file.
    Invalid,
    /// Made by a newer Atelier.
    Newer(u32),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Io(e) => f.write_str(&crate::i18n::io_error(e)),
            FileError::Invalid => f.write_str(&t!("actions-error-not-actions")),
            FileError::Newer(v) => f.write_str(&t!("file-error-newer-version", version = *v)),
        }
    }
}

impl Library {
    /// The saved actions, or the default ones the first time (tests start
    /// from the default ones). A file that can't be read is never
    /// overwritten: a damaged one is renamed aside first; one made by a
    /// newer Atelier, or unreadable, is left as it is and nothing is saved
    /// this session (`Load::saving` false).
    pub fn load() -> Load {
        if cfg!(test) {
            return Load::fine(Self::defaults(), None);
        }
        Self::load_from(&library_file())
    }

    fn load_from(path: &Path) -> Load {
        use crate::store::{Found, Store};
        let mut store = Store::new(path.to_path_buf(), LIBRARY_FILE)
            .accepting(|v| !matches!(sets_of(v), Err(FileError::Newer(_))));
        let (found, error) = match store.read() {
            Found::Value(value) => match sets_of(&value) {
                Ok(sets) => return Load::fine(Self { sets }, Some(store)),
                Err(e @ FileError::Newer(_)) => {
                    store.refuse_saving();
                    (None, e)
                }
                Err(e) => match store.set_aside() {
                    found @ Found::SetAside(_) => (Some(found), e),
                    _ => (None, e),
                },
            },
            Found::Missing => {
                // What this window's changes are made from.
                let defaults = Self::defaults();
                if let Ok(value) = defaults.file_value() {
                    store.adopt(&value);
                }
                return Load::fine(defaults, Some(store));
            }
            found => (Some(found), FileError::Invalid),
        };
        let notice = match found {
            Some(Found::SetAside(b)) => t!(
                "smart3-actions-damaged",
                error = error.to_string(),
                backup = b
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            ),
            Some(Found::Unreadable(e)) => {
                t!("smart4-actions-unreadable", error = FileError::Io(e).to_string())
            }
            _ => t!("smart4-actions-unreadable", error = error.to_string()),
        };
        Load {
            library: Self::defaults(),
            saving: store.saving(),
            notice: Some(notice),
            store: Some(store),
        }
    }

    /// Saved over the actions file, atomically, merged with what another
    /// window saved meanwhile (see `store`).
    pub fn save(&self, store: &mut crate::store::Store) -> Result<(), FileError> {
        store.save(self.file_value()?).map(drop).map_err(FileError::Io)
    }

    /// As in an actions file.
    fn file_value(&self) -> Result<serde_json::Value, FileError> {
        let file = File {
            version: VERSION,
            sets: self.sets.clone(),
        };
        serde_json::to_value(&file).map_err(|_| FileError::Invalid)
    }

    /// The actions Atelier comes with, built from its commands.
    pub fn defaults() -> Self {
        let vignette = vec![
            Step::new(Recorded::Menu {
                item: MenuItem::NewLayer,
            }),
            Step::new(Recorded::Select {
                shape: Shape::Ellipse,
                rect: [8.0, 8.0, 84.0, 84.0],
                percent: true,
                mode: Combine::New,
                feather: 0.0,
                anti_alias: true,
            }),
            Step::new(Recorded::InvertSelection),
            Step::with_dialog(Recorded::ModifySelection {
                modify: Modify::Feather,
                amount: 120.0,
                at_bounds: false,
            }),
            Step::new(Recorded::Fill {
                contents: Contents::Black,
                color: [0, 0, 0],
                opacity: 1.0,
                preserve_transparency: false,
                pattern: None,
                }),
            Step::new(Recorded::Deselect),
        ];
        let sepia = vec![
            Step::new(Recorded::Menu {
                item: MenuItem::DuplicateLayer,
            }),
            Step::new(Recorded::Adjust {
                adjustment: Adjustment::HueSaturation(HueSaturation {
                    hue: 35.0,
                    saturation: 0.25,
                    lightness: 0.0,
                    colorize: true,
                    ranges: Default::default(),
                }),
            }),
        ];
        let fit = |side| Recorded::FitImage {
            width: side,
            height: side,
            dont_enlarge: true,
        };
        let web = vec![
            Step::new(fit(2048)),
            Step::new(Recorded::Export {
                options: ExportOptions::web(),
                folder: None,
            }),
        ];
        let action = |name: String, steps| Sequence {
            name,
            key: None,
            steps,
            expanded: false,
        };
        Self {
            sets: vec![Set {
                name: t!("actions-default-set"),
                actions: vec![
                    action(t!("actions-default-vignette"), vignette),
                    action(t!("actions-default-sepia"), sepia),
                    action(t!("actions-default-resize"), vec![Step::new(fit(1080))]),
                    action(t!("actions-default-web"), web),
                ],
                expanded: true,
            }],
        }
    }

    /// The action with this function key.
    pub fn with_key(&self, key: FnKey) -> Option<(usize, usize)> {
        self.sets.iter().enumerate().find_map(|(s, set)| {
            let a = set.actions.iter().position(|a| a.key == Some(key))?;
            Some((s, a))
        })
    }

    /// Loaded sets added after the others. A function key plays one action
    /// only: a loaded action whose key is already used loses it. Returns
    /// how many lost theirs.
    pub fn add_sets(&mut self, sets: Vec<Set>) -> usize {
        let mut used: Vec<FnKey> = self
            .sets
            .iter()
            .flat_map(|s| s.actions.iter().filter_map(|a| a.key))
            .collect();
        let mut dropped = 0;
        for mut set in sets {
            for action in &mut set.actions {
                let Some(key) = action.key else { continue };
                if used.contains(&key) {
                    action.key = None;
                    dropped += 1;
                } else {
                    used.push(key);
                }
            }
            self.sets.push(set);
        }
        dropped
    }
}

/// The library read at start, and what the user must know about it.
pub struct Load {
    pub library: Library,
    /// The actions file may be written.
    pub saving: bool,
    pub notice: Option<String>,
    /// The actions file (none in tests).
    pub store: Option<crate::store::Store>,
}

impl Load {
    fn fine(library: Library, store: Option<crate::store::Store>) -> Self {
        Self {
            library,
            saving: true,
            notice: None,
            store,
        }
    }
}

fn library_file() -> PathBuf {
    crate::config::dir().join("actions.json")
}

/// The sets of an actions file.
pub fn read(path: &Path) -> Result<Vec<Set>, FileError> {
    let json = std::fs::read(path).map_err(FileError::Io)?;
    let value: serde_json::Value = serde_json::from_slice(&json).map_err(|_| FileError::Invalid)?;
    sets_of(&value)
}

/// The library's file: JSON, the actions this version reads written as it
/// writes them (what another window saved compares with what this one
/// read).
const LIBRARY_FILE: crate::store::Format = crate::store::Format {
    decode: |bytes| {
        let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        let same = sets_of(&value).ok().and_then(|sets| {
            let file = File {
                version: VERSION,
                sets,
            };
            serde_json::to_value(file).ok()
        });
        Some(same.unwrap_or(value))
    },
    encode: crate::store::JSON.encode,
};

/// The sets of an actions file's contents.
fn sets_of(value: &serde_json::Value) -> Result<Vec<Set>, FileError> {
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    match version {
        Some(v) if v > VERSION as u64 => Err(FileError::Newer(v as u32)),
        Some(_) => File::deserialize(value)
            .map(|f| f.sets)
            .map_err(|_| FileError::Invalid),
        None => Err(FileError::Invalid),
    }
}

/// Writes `sets` as an actions file.
pub fn write(path: &Path, sets: &[Set]) -> Result<(), FileError> {
    let file = File {
        version: VERSION,
        sets: sets.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&file).map_err(|_| FileError::Invalid)?;
    let written =
        crate::document::write_atomic(path, |out| Ok(std::io::Write::write_all(out, &json)?));
    written.map_err(|e| match e {
        crate::document::FileError::Io(e) => FileError::Io(e),
        e => FileError::Io(std::io::Error::other(e.to_string())),
    })
}

/// A part of the names Batch gives the files it saves, like Photoshop's
/// File Naming.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NamePart {
    DocumentName,
    DocumentNameLower,
    DocumentNameUpper,
    /// Serial number with at least this many digits (1 to 4).
    Serial(u8),
    /// a, b, c… z, aa, ab…
    SerialLetter,
    SerialLetterUpper,
    /// yyyymmdd.
    Date,
    Extension,
    ExtensionUpper,
    Text(String),
}

/// File name made of `parts` for the document named `stem`, the
/// `serial`-th one (from 1), saved with extension `ext` (without dot),
/// on `date` (year, month, day). Characters that file names can't hold
/// become "_"; without an extension part, the extension is added.
pub fn batch_name(
    parts: &[NamePart],
    stem: &str,
    serial: u32,
    ext: &str,
    date: (i32, u32, u32),
) -> String {
    let mut name = String::new();
    let mut has_ext = false;
    for part in parts {
        match part {
            NamePart::DocumentName => name.push_str(stem),
            NamePart::DocumentNameLower => name.push_str(&stem.to_lowercase()),
            NamePart::DocumentNameUpper => name.push_str(&stem.to_uppercase()),
            NamePart::Serial(digits) => {
                let width = (*digits).clamp(1, 4) as usize;
                name.push_str(&format!("{serial:0width$}"));
            }
            NamePart::SerialLetter => name.push_str(&serial_letter(serial)),
            NamePart::SerialLetterUpper => name.push_str(&serial_letter(serial).to_uppercase()),
            NamePart::Date => {
                let (y, m, d) = date;
                name.push_str(&format!("{y:04}{m:02}{d:02}"));
            }
            NamePart::Extension | NamePart::ExtensionUpper => {
                has_ext = true;
                name.push('.');
                if *part == NamePart::Extension {
                    name.push_str(&ext.to_lowercase());
                } else {
                    name.push_str(&ext.to_uppercase());
                }
            }
            NamePart::Text(text) => name.push_str(text),
        }
    }
    let mut name: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if name.trim_matches('.').trim().is_empty() || name.starts_with('.') {
        name.insert_str(0, stem);
    }
    if !has_ext {
        name.push('.');
        name.push_str(ext);
    }
    windows_safe(&name, stem)
}

/// `name` as Windows keeps it: without the trailing dots and spaces it
/// would drop (before the extension too), never a device name, never
/// hidden (then named after the document, `doc`).
fn windows_safe(name: &str, doc: &str) -> String {
    let trailing = |c: char| c == '.' || c.is_whitespace();
    let name = name.trim_end_matches(trailing);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    };
    let stem = match stem.trim_end_matches(trailing) {
        "" => doc,
        stem => stem,
    };
    if crate::document::windows_device(stem) {
        let n = stem.find('.').unwrap_or(stem.len());
        format!("{}_{}{ext}", &stem[..n], &stem[n..])
    } else {
        format!("{stem}{ext}")
    }
}

/// 1 → a, 26 → z, 27 → aa…
fn serial_letter(mut n: u32) -> String {
    let mut letters = Vec::new();
    while n > 0 {
        n -= 1;
        letters.push(b'a' + (n % 26) as u8);
        n /= 26;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_default()
}

/// Today (local time is not known without a time zone database: UTC).
pub fn today() -> (i32, u32, u32) {
    local_date().unwrap_or_else(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        civil_date((secs / 86_400) as i64)
    })
}

/// Today in the local time zone.
#[cfg(unix)]
fn local_date() -> Option<(i32, u32, u32)> {
    // SAFETY: `time(NULL)` has no preconditions; `localtime_r` writes into
    // the `tm` it is given and returns null on failure.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        Some((tm.tm_year + 1900, (tm.tm_mon + 1) as u32, tm.tm_mday as u32))
    }
}

#[cfg(windows)]
fn local_date() -> Option<(i32, u32, u32)> {
    #[repr(C)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut SystemTime);
    }
    let mut t = SystemTime {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    // SAFETY: GetLocalTime fills the SYSTEMTIME it is given.
    unsafe { GetLocalTime(&mut t) };
    Some((i32::from(t.year), u32::from(t.month), u32::from(t.day)))
}

#[cfg(not(any(unix, windows)))]
fn local_date() -> Option<(i32, u32, u32)> {
    None
}

/// Year, month, day of the `days`-th day since 1970-01-01 (Howard
/// Hinnant's algorithm).
pub(crate) fn civil_date(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::FilterKind;

    fn sample() -> Vec<Set> {
        let mut library = Library::defaults();
        library.sets[0].actions[0].steps.extend([
            Step::new(Recorded::Filter {
                filter: FilterKind::GaussianBlur.initial(),
            }),
            Step::new(Recorded::Transform {
                corners: [[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]],
                frame: Frame::of([[0.0, 0.0], [2.0, 0.0], [2.0, 1.0], [0.0, 1.0]], 1.0, 1.0),
            }),
            Step::new(Recorded::Stop {
                message: "Check".into(),
                allow_continue: true,
            }),
            Step::new(Recorded::Liquify {
                mesh: vec![0, 1, 0xab, 0xff],
            }),
            Step::new(Recorded::ConvertToProfile {
                profile: vec![7; 9],
                intent: crate::cms::Intent::Perceptual,
                bpc: false,
            }),
            Step::new(Recorded::AssignProfile { profile: None }),
            Step::new(Recorded::Grayscale),
        ]);
        library.sets[0].actions[0].key = Some(FnKey {
            number: 5,
            shift: false,
            ctrl: true,
        });
        library.sets
    }

    /// What isn't saved (panel state) left out.
    fn saved(mut sets: Vec<Set>) -> Vec<Set> {
        for set in &mut sets {
            set.expanded = false;
            for a in &mut set.actions {
                a.expanded = false;
            }
        }
        sets
    }

    #[test]
    fn actions_files_round_trip() {
        let dir = crate::testing::test_dir("actions-round-trip");
        let path = dir.join("set.json");
        let sets = sample();
        write(&path, &sets).unwrap();
        assert_eq!(read(&path).unwrap(), saved(sets));
    }

    #[test]
    fn unknown_commands_are_kept_and_newer_files_refused() {
        let json = serde_json::json!({
            "version": 1,
            "sets": [{"name": "S", "actions": [{"name": "A", "steps": [
                {"do": {"command": "liquify", "mesh": [1, 2]}, "enabled": false},
                {"do": {"command": "select-all"}},
            ]}]}],
        });
        let dir = crate::testing::test_dir("actions-unknown");
        let path = dir.join("set.json");
        std::fs::write(&path, json.to_string()).unwrap();
        let sets = read(&path).unwrap();
        let steps = &sets[0].actions[0].steps;
        assert!(matches!(&steps[0].command, Recorded::Unknown(v) if v["command"] == "liquify"));
        assert!(!steps[0].enabled && steps[1].enabled);
        assert_eq!(steps[1].command, Recorded::SelectAll);
        write(&path, &sets).unwrap();
        let back: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            back["sets"][0]["actions"][0]["steps"][0]["do"]["mesh"][1],
            2
        );

        std::fs::write(&path, r#"{"version": 9, "sets": []}"#).unwrap();
        assert!(matches!(read(&path), Err(FileError::Newer(9))));
        std::fs::write(&path, r#"{"layers": []}"#).unwrap();
        assert!(matches!(read(&path), Err(FileError::Invalid)));
    }

    #[test]
    fn an_unreadable_library_is_never_overwritten() {
        let dir = crate::testing::test_dir("actions-library-load");
        let path = dir.join("actions.json");
        // None yet: the defaults, saved as usual.
        let load = Library::load_from(&path);
        assert!(load.saving && load.notice.is_none());
        // Made by a newer version: kept as it is, nothing saved.
        let newer = r#"{"version": 9, "sets": []}"#;
        std::fs::write(&path, newer).unwrap();
        let load = Library::load_from(&path);
        assert!(!load.saving && load.notice.is_some());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);
        // Damaged: set aside before anything is saved over it.
        std::fs::write(&path, "{\"version\": 1, \"se").unwrap();
        let load = Library::load_from(&path);
        assert!(load.saving && load.notice.is_some());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("actions.json.bak")).unwrap(),
            "{\"version\": 1, \"se"
        );
        std::fs::write(&path, "x").unwrap();
        Library::load_from(&path);
        assert!(dir.join("actions.json.bak-2").exists());
    }

    /// Two windows of Atelier: an action recorded in one stays when the
    /// other changes its own library.
    #[test]
    fn two_windows_never_erase_each_others_actions() {
        let dir = crate::testing::test_dir("actions-two-windows");
        let path = dir.join("actions.json");
        let (mut a, mut b) = (Library::load_from(&path), Library::load_from(&path));
        let (mut sa, mut sb) = (a.store.take().unwrap(), b.store.take().unwrap());
        let recorded = Sequence {
            name: "Recorded in A".into(),
            key: None,
            steps: vec![Step::new(Recorded::Deselect)],
            expanded: false,
        };
        a.library.sets[0].actions.push(recorded.clone());
        a.library.save(&mut sa).unwrap();
        // B renames a default action, then adds a set.
        b.library.sets[0].actions[0].name = "Renamed in B".into();
        b.library.save(&mut sb).unwrap();
        b.library.sets.push(Set {
            name: "B's set".into(),
            actions: Vec::new(),
            expanded: true,
        });
        b.library.save(&mut sb).unwrap();
        let sets = read(&path).unwrap();
        assert_eq!(sets.len(), 2);
        let names: Vec<&str> = sets[0].actions.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"Recorded in A") && names.contains(&"Renamed in B"));
        assert_eq!(sets[0].actions.len(), Library::defaults().sets[0].actions.len() + 1);
        assert_eq!(sets[1].name, "B's set");
    }

    #[test]
    fn batch_names_follow_the_parts() {
        let date = (2026, 3, 7);
        let parts = [
            NamePart::DocumentName,
            NamePart::Text("_".into()),
            NamePart::Serial(3),
            NamePart::Extension,
        ];
        assert_eq!(batch_name(&parts, "Beach", 7, "jpg", date), "Beach_007.jpg");
        let parts = [
            NamePart::DocumentNameLower,
            NamePart::Text("-".into()),
            NamePart::SerialLetterUpper,
            NamePart::Text("-".into()),
            NamePart::Date,
            NamePart::ExtensionUpper,
        ];
        assert_eq!(
            batch_name(&parts, "Beach", 28, "png", date),
            "beach-AB-20260307.PNG"
        );
        // No extension part: added; forbidden characters replaced.
        let parts = [NamePart::Text("a/b:c".into())];
        assert_eq!(batch_name(&parts, "x", 1, "tif", date), "a_b_c.tif");
        // Nothing but an extension: the document's name comes first.
        assert_eq!(
            batch_name(&[NamePart::Extension], "x", 1, "gif", date),
            "x.gif"
        );
        // Names Windows would refuse or change: device names, trailing
        // dots and spaces.
        let text = |t: &str| [NamePart::Text(t.into())];
        assert_eq!(batch_name(&text("con"), "x", 1, "png", date), "con_.png");
        assert_eq!(
            batch_name(&text("NUL.a"), "x", 1, "png", date),
            "NUL_.a.png"
        );
        assert_eq!(
            batch_name(&text("draft. "), "x", 1, "png", date),
            "draft.png"
        );
        assert_eq!(batch_name(&text("  "), "x", 1, "png", date), "x.png");
        let parts = [NamePart::Extension, NamePart::Text(". ".into())];
        assert_eq!(batch_name(&parts, "x", 1, "jpg", date), "x.jpg");
        assert_eq!(serial_letter(1), "a");
        assert_eq!(serial_letter(26), "z");
        assert_eq!(serial_letter(27), "aa");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(19_723), (2024, 1, 1));
        assert_eq!(civil_date(19_782), (2024, 2, 29));
    }
}
