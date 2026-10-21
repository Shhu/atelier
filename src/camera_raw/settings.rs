//! Settings of Camera Raw, in its units (sliders from −100 to +100 unless
//! said otherwise), serializable (presets, Smart Filters), and the presets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::source::Source;

/// Everything Camera Raw does to an image. The default changes nothing to
/// an 8-bit image; a RAW file starts from [`Settings::for_source`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub profile: Profile,
    pub white_balance: WhiteBalance,
    /// RAW files: kelvins and tint of the light (see [`super::TEMPERATURE_RANGE`]);
    /// other images: relative, −100 to +100.
    pub temperature: f32,
    pub tint: f32,
    /// Stops, −5 to +5.
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub texture: f32,
    pub clarity: f32,
    pub dehaze: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub curve: ToneCurve,
    pub mixer: ColorMixer,
    pub grading: ColorGrading,
    pub detail: Detail,
    pub effects: Effects,
    pub lens: Lens,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    #[default]
    Color,
    Monochrome,
}

/// White balance presets: what the camera recorded, an estimate, the usual
/// lights (RAW files only), or the sliders' values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhiteBalance {
    #[default]
    AsShot,
    Auto,
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
    Custom,
}

impl WhiteBalance {
    /// Offered for RAW files.
    pub const RAW: [WhiteBalance; 9] = [
        WhiteBalance::AsShot,
        WhiteBalance::Auto,
        WhiteBalance::Daylight,
        WhiteBalance::Cloudy,
        WhiteBalance::Shade,
        WhiteBalance::Tungsten,
        WhiteBalance::Fluorescent,
        WhiteBalance::Flash,
        WhiteBalance::Custom,
    ];
    /// Offered for the other images.
    pub const IMAGE: [WhiteBalance; 3] = [
        WhiteBalance::AsShot,
        WhiteBalance::Auto,
        WhiteBalance::Custom,
    ];

    /// Temperature and tint of a light preset.
    pub fn light(self) -> Option<(f32, f32)> {
        match self {
            WhiteBalance::Daylight => Some((5500.0, 10.0)),
            WhiteBalance::Cloudy => Some((6500.0, 10.0)),
            WhiteBalance::Shade => Some((7500.0, 10.0)),
            WhiteBalance::Tungsten => Some((2850.0, 0.0)),
            WhiteBalance::Fluorescent => Some((3800.0, 21.0)),
            WhiteBalance::Flash => Some((5500.0, 0.0)),
            _ => None,
        }
    }
}

/// Tone Curve: parametric (four regions, three split points in % of the
/// tones) then point curves (composite, red, green, blue; 0 to 255).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToneCurve {
    pub highlights: f32,
    pub lights: f32,
    pub darks: f32,
    pub shadows: f32,
    pub splits: [f32; 3],
    pub points: [Vec<[f32; 2]>; 4],
}

impl Default for ToneCurve {
    fn default() -> Self {
        let line = vec![[0.0, 0.0], [255.0, 255.0]];
        Self {
            highlights: 0.0,
            lights: 0.0,
            darks: 0.0,
            shadows: 0.0,
            splits: [25.0, 50.0, 75.0],
            points: [line.clone(), line.clone(), line.clone(), line],
        }
    }
}

/// Color Mixer: hue, saturation and luminance of eight hue ranges (red,
/// orange, yellow, green, aqua, blue, purple, magenta); in Monochrome, the
/// gray they become.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorMixer {
    pub hue: [f32; 8],
    pub saturation: [f32; 8],
    pub luminance: [f32; 8],
    pub gray: [f32; 8],
}

/// A color wheel: hue (degrees), saturation (0 to 100), luminance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wheel {
    pub hue: f32,
    pub saturation: f32,
    pub luminance: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    /// Overlap of the ranges, 0 to 100.
    pub blending: f32,
    /// Toward the shadows (−) or the highlights (+).
    pub balance: f32,
}

impl Default for ColorGrading {
    fn default() -> Self {
        Self {
            shadows: Wheel::default(),
            midtones: Wheel::default(),
            highlights: Wheel::default(),
            global: Wheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

/// Detail: sharpening (amount 0 to 150, radius 0.5 to 3 px, detail and
/// masking 0 to 100) and noise reduction (0 to 100).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Detail {
    pub sharpen_amount: f32,
    pub sharpen_radius: f32,
    pub sharpen_detail: f32,
    pub sharpen_masking: f32,
    pub luminance: f32,
    pub luminance_detail: f32,
    pub color: f32,
    pub color_detail: f32,
}

impl Default for Detail {
    fn default() -> Self {
        Self {
            sharpen_amount: 0.0,
            sharpen_radius: 1.0,
            sharpen_detail: 25.0,
            sharpen_masking: 0.0,
            luminance: 0.0,
            luminance_detail: 50.0,
            color: 0.0,
            color_detail: 50.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VignetteStyle {
    #[default]
    HighlightPriority,
    ColorPriority,
    PaintOverlay,
}

/// Effects: post-crop vignetting (midpoint and feather 0 to 100) and grain
/// (0 to 100).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    pub vignette_style: VignetteStyle,
    pub vignette: f32,
    pub midpoint: f32,
    pub roundness: f32,
    pub feather: f32,
    pub vignette_highlights: f32,
    pub grain: f32,
    pub grain_size: f32,
    pub grain_roughness: f32,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            vignette_style: VignetteStyle::HighlightPriority,
            vignette: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
            vignette_highlights: 0.0,
            grain: 0.0,
            grain_size: 25.0,
            grain_roughness: 50.0,
        }
    }
}

/// Manual lens corrections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lens {
    /// + corrects barrel distortion, − pincushion.
    pub distortion: f32,
    /// + lightens the corners.
    pub vignetting: f32,
    pub midpoint: f32,
}

impl Default for Lens {
    fn default() -> Self {
        Self {
            distortion: 0.0,
            vignetting: 0.0,
            midpoint: 50.0,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profile: Profile::Color,
            white_balance: WhiteBalance::AsShot,
            temperature: 0.0,
            tint: 0.0,
            exposure: 0.0,
            contrast: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            whites: 0.0,
            blacks: 0.0,
            texture: 0.0,
            clarity: 0.0,
            dehaze: 0.0,
            vibrance: 0.0,
            saturation: 0.0,
            curve: ToneCurve::default(),
            mixer: ColorMixer::default(),
            grading: ColorGrading::default(),
            detail: Detail::default(),
            effects: Effects::default(),
            lens: Lens::default(),
        }
    }
}

impl Settings {
    /// Defaults for `source`: a RAW file starts from its white balance as
    /// shot, with some sharpening and color noise reduction.
    pub fn for_source(source: &Source) -> Settings {
        let mut s = Settings::default();
        if let Some(camera) = &source.camera {
            (s.temperature, s.tint) = camera.as_shot_temperature();
            s.detail.sharpen_amount = 40.0;
            s.detail.color = 25.0;
        }
        s
    }

    /// The look of `preset` (everything but the white balance) on these
    /// settings.
    pub fn with_look(&self, preset: &Settings) -> Settings {
        Settings {
            white_balance: self.white_balance,
            temperature: self.temperature,
            tint: self.tint,
            ..preset.clone()
        }
    }
}

/// A user preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub settings: Settings,
}

/// Presets that come with Atelier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltIn {
    Vivid,
    Matte,
    BlackWhite,
    WarmFilm,
}

pub const BUILT_IN_PRESETS: [BuiltIn; 4] = [
    BuiltIn::Vivid,
    BuiltIn::Matte,
    BuiltIn::BlackWhite,
    BuiltIn::WarmFilm,
];

impl BuiltIn {
    /// Its look over `base` (from which the white balance is kept).
    pub fn settings(self, base: &Settings) -> Settings {
        let mut s = base.with_look(&Settings {
            detail: base.detail.clone(),
            ..Settings::default()
        });
        match self {
            BuiltIn::Vivid => {
                s.contrast = 15.0;
                s.clarity = 10.0;
                s.vibrance = 35.0;
                s.saturation = 5.0;
            }
            BuiltIn::Matte => {
                s.contrast = -15.0;
                s.highlights = -20.0;
                s.curve.shadows = 30.0;
                s.curve.highlights = -15.0;
                s.saturation = -10.0;
            }
            BuiltIn::BlackWhite => {
                s.profile = Profile::Monochrome;
                s.contrast = 25.0;
                s.clarity = 15.0;
                s.mixer.gray = [-10.0, -5.0, 10.0, 0.0, -10.0, -20.0, -5.0, 0.0];
            }
            BuiltIn::WarmFilm => {
                s.contrast = 10.0;
                s.curve.shadows = 15.0;
                s.vibrance = -10.0;
                s.grading.shadows = Wheel {
                    hue: 200.0,
                    saturation: 15.0,
                    luminance: 0.0,
                };
                s.grading.highlights = Wheel {
                    hue: 40.0,
                    saturation: 20.0,
                    luminance: 0.0,
                };
                s.effects.grain = 20.0;
            }
        }
        s
    }
}

/// `camera-raw-presets.json` in the settings folder.
fn presets_file() -> PathBuf {
    crate::config::dir().join("camera-raw-presets.json")
}

/// The user's presets (none in tests).
pub fn load_presets() -> Vec<Preset> {
    if cfg!(test) {
        return Vec::new();
    }
    load_presets_from(&presets_file())
}

pub fn save_presets(presets: &[Preset]) -> std::io::Result<()> {
    if cfg!(test) {
        return Ok(());
    }
    save_presets_to(&presets_file(), presets)
}

fn load_presets_from(path: &Path) -> Vec<Preset> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_presets_to(path: &Path, presets: &[Preset]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_vec_pretty(presets).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_saved_and_loaded_back() {
        let dir = std::env::temp_dir().join("atelier-raw-presets");
        let path = dir.join("presets.json");
        let mut settings = Settings {
            exposure: 0.7,
            ..Settings::default()
        };
        settings.curve.points[0].insert(1, [64.0, 80.0]);
        settings.grading.highlights.hue = 40.0;
        let presets = vec![Preset {
            name: "Mine".into(),
            settings,
        }];
        save_presets_to(&path, &presets).unwrap();
        assert_eq!(load_presets_from(&path), presets);
        // Older files miss the newer fields: they get their defaults.
        std::fs::write(&path, r#"[{"name":"Old","settings":{"exposure":1.0}}]"#).unwrap();
        let old = load_presets_from(&path);
        assert_eq!(old[0].settings.exposure, 1.0);
        assert_eq!(old[0].settings.effects, Effects::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_preset_keeps_the_white_balance() {
        let base = Settings {
            white_balance: WhiteBalance::Custom,
            temperature: 4200.0,
            tint: 5.0,
            exposure: 1.0,
            ..Settings::default()
        };
        for preset in BUILT_IN_PRESETS {
            let s = preset.settings(&base);
            assert_eq!(
                (s.white_balance, s.temperature, s.tint),
                (WhiteBalance::Custom, 4200.0, 5.0)
            );
            assert_eq!(s.exposure, 0.0, "the look replaces the rest");
            assert_ne!(s, Settings::default());
        }
    }
}
