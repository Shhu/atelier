//! Swatches and recent colors, kept in the settings folder
//! (`swatches.json`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::{self, Hsb};
use crate::config;

/// Recent colors kept, most recent first.
const MAX_RECENT: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct Swatches {
    pub colors: Vec<[u8; 3]>,
    pub recent: Vec<[u8; 3]>,
}

/// File contents: colors as hex codes.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Saved {
    swatches: Vec<String>,
    recent: Vec<String>,
}

impl Default for Swatches {
    fn default() -> Self {
        Self {
            colors: default_colors(),
            recent: Vec::new(),
        }
    }
}

impl Swatches {
    /// The saved swatches, or the default ones. Tests start from the
    /// default ones.
    pub fn load() -> Self {
        if cfg!(test) {
            return Self::default();
        }
        Self::load_from(&file()).unwrap_or_default()
    }

    fn load_from(path: &Path) -> Option<Self> {
        let saved: Saved = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        let parse = |codes: Vec<String>| codes.iter().filter_map(|c| color::parse_hex(c)).collect();
        Some(Self {
            colors: parse(saved.swatches),
            recent: parse(saved.recent),
        })
    }

    fn save(&self) {
        if !cfg!(test) {
            self.save_to(&file());
        }
    }

    fn save_to(&self, path: &Path) {
        let hex = |colors: &[[u8; 3]]| colors.iter().map(|&c| color::hex(c)).collect();
        let saved = Saved {
            swatches: hex(&self.colors),
            recent: hex(&self.recent),
        };
        if let (Some(dir), Ok(json)) = (path.parent(), serde_json::to_vec_pretty(&saved)) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(path, json);
        }
    }

    /// New swatch, at the end (Add to Swatches).
    pub fn add(&mut self, rgb: [u8; 3]) {
        self.colors.push(rgb);
        self.save();
    }

    pub fn delete(&mut self, index: usize) {
        if index < self.colors.len() {
            self.colors.remove(index);
            self.save();
        }
    }

    /// A color just chosen goes first in the recent colors.
    pub fn use_color(&mut self, rgb: [u8; 3]) {
        if self.recent.first() == Some(&rgb) {
            return;
        }
        self.recent.retain(|&c| c != rgb);
        self.recent.insert(0, rgb);
        self.recent.truncate(MAX_RECENT);
        self.save();
    }
}

fn file() -> PathBuf {
    config::dir().join("swatches.json")
}

/// Rows of 12: grays, then the hues every 30° in five tones (pastel,
/// light, pure, dark, darker).
fn default_colors() -> Vec<[u8; 3]> {
    let grays = (0..12).map(|i| {
        let v = (255.0 * (1.0 - i as f32 / 11.0)).round() as u8;
        [v; 3]
    });
    let tones = [(0.35, 1.0), (0.6, 1.0), (1.0, 1.0), (1.0, 0.6), (1.0, 0.35)];
    let hues = tones.into_iter().flat_map(|(s, b)| {
        (0..12).map(move |i| {
            color::to_u8(color::hsb_to_rgb(Hsb {
                h: i as f32 * 30.0,
                s,
                b,
            }))
        })
    });
    grays.chain(hues).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_and_loaded_back() {
        let path = std::env::temp_dir()
            .join(format!("atelier-swatches-{}", std::process::id()))
            .join("swatches.json");
        let s = Swatches {
            colors: vec![[1, 2, 3], [255, 136, 0]],
            recent: vec![[9, 9, 9]],
        };
        s.save_to(&path);
        assert_eq!(Swatches::load_from(&path), Some(s));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn recent_colors_are_unique_most_recent_first() {
        let mut s = Swatches::default();
        for c in [[1, 0, 0], [2, 0, 0], [1, 0, 0]] {
            s.use_color(c);
        }
        assert_eq!(s.recent, vec![[1, 0, 0], [2, 0, 0]]);
        for i in 0..20 {
            s.use_color([i, 1, 1]);
        }
        assert_eq!(s.recent.len(), MAX_RECENT);
        assert_eq!(s.recent[0], [19, 1, 1]);
    }

    #[test]
    fn default_palette_has_grays_then_hues() {
        let c = default_colors();
        assert_eq!((c[0], c[11]), ([255; 3], [0; 3]));
        // Pure hues: red, yellow, blue.
        assert_eq!(
            (c[36], c[38], c[44]),
            ([255, 0, 0], [255, 255, 0], [0, 0, 255])
        );
        assert_eq!(c.len(), 72);
    }
}
