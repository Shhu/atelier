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
    /// The Crop tool: x, y, width, height (canvas pixels).
    Crop {
        rect: [i64; 4],
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
    /// Select › Modify, by `amount` px.
    ModifySelection {
        modify: Modify,
        amount: f32,
        at_bounds: bool,
    },
    /// Free Transform: where the corners of the transformed element's box
    /// go (top-left first, clockwise), in units of that box (0,0 its
    /// top-left corner, 1,1 its bottom-right one): a scale, a turn, a skew
    /// or a distortion, of whatever is transformed when played.
    Transform {
        corners: [[f64; 2]; 4],
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
            FileError::Io(e) => write!(f, "{e}"),
            FileError::Invalid => f.write_str(&t!("actions-error-not-actions")),
            FileError::Newer(v) => f.write_str(&t!("file-error-newer-version", version = *v)),
        }
    }
}

impl Library {
    /// The saved actions, or the default ones the first time (tests start
    /// from the default ones).
    pub fn load() -> Self {
        if cfg!(test) {
            return Self::defaults();
        }
        read(&library_file()).map_or_else(|_| Self::defaults(), |sets| Self { sets })
    }

    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        let _ = std::fs::create_dir_all(crate::config::dir());
        let _ = write(&library_file(), &self.sets);
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
}

fn library_file() -> PathBuf {
    crate::config::dir().join("actions.json")
}

/// The sets of an actions file.
pub fn read(path: &Path) -> Result<Vec<Set>, FileError> {
    let json = std::fs::read(path).map_err(FileError::Io)?;
    let value: serde_json::Value = serde_json::from_slice(&json).map_err(|_| FileError::Invalid)?;
    let version = value.get("version").and_then(serde_json::Value::as_u64);
    match version {
        Some(v) if v > VERSION as u64 => Err(FileError::Newer(v as u32)),
        Some(_) => File::deserialize(&value)
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
    std::fs::write(path, json).map_err(FileError::Io)
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
    name
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
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date((secs / 86_400) as i64)
}

/// Year, month, day of the `days`-th day since 1970-01-01 (Howard
/// Hinnant's algorithm).
fn civil_date(days: i64) -> (i32, u32, u32) {
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
        let dir = std::env::temp_dir().join("atelier-actions-round-trip");
        std::fs::create_dir_all(&dir).unwrap();
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
        let dir = std::env::temp_dir().join("atelier-actions-unknown");
        std::fs::create_dir_all(&dir).unwrap();
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
