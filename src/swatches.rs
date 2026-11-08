//! Swatches and recent colors, kept in the settings folder
//! (`swatches.json`, shared with the other windows: see `store`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::color::{self, Hsb};
use crate::config;
use crate::store::{self, Found, Store};

/// Recent colors kept, most recent first.
const MAX_RECENT: usize = 12;

pub struct Swatches {
    pub colors: Vec<[u8; 3]>,
    pub recent: Vec<[u8; 3]>,
    /// The file (none in tests).
    store: Option<Store>,
    /// What the user must know about the file, once.
    pub notice: Option<String>,
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
            store: None,
            notice: None,
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
        Self::load_from(file())
    }

    /// A file that can't be read is set aside (or, unreadable, never
    /// written): the defaults come instead.
    fn load_from(path: PathBuf) -> Self {
        let mut store = Store::new(path, store::JSON).tidying(|v| {
            if let Some(recent) = v.get_mut("recent").and_then(|r| r.as_array_mut()) {
                recent.truncate(MAX_RECENT);
            }
        });
        let mut found = store.read();
        let mut saved = None;
        if let Found::Value(v) = &found {
            match serde_json::from_value::<Saved>(v.clone()) {
                Ok(s) => saved = Some(s),
                // Not swatches: set aside too.
                Err(_) => found = store.set_aside(),
            }
        }
        let mut swatches = saved.map_or_else(Self::default, Self::from_saved);
        if let Found::SetAside(backup) = found {
            let backup = backup.file_name().unwrap_or_default().to_string_lossy().into_owned();
            swatches.notice = Some(crate::i18n::t!("safety14-swatches-damaged", backup = backup));
        }
        // What this window's changes are made from, as it writes it.
        if let Ok(value) = serde_json::to_value(swatches.saved()) {
            store.adopt(&value);
        }
        swatches.store = Some(store);
        swatches
    }

    fn from_saved(saved: Saved) -> Self {
        let parse = |codes: Vec<String>| codes.iter().filter_map(|c| color::parse_hex(c)).collect();
        Self {
            colors: parse(saved.swatches),
            recent: parse(saved.recent),
            store: None,
            notice: None,
        }
    }

    fn saved(&self) -> Saved {
        let hex = |colors: &[[u8; 3]]| colors.iter().map(|&c| color::hex(c)).collect();
        Saved {
            swatches: hex(&self.colors),
            recent: hex(&self.recent),
        }
    }

    /// Saved, merged with what another window saved meanwhile, which
    /// then shows here too.
    fn save(&mut self) {
        let Ok(mine) = serde_json::to_value(self.saved()) else {
            return;
        };
        let Some(store) = &mut self.store else { return };
        let Ok(merged) = store.save(mine) else { return };
        if let Ok(saved) = serde_json::from_value::<Saved>(merged.clone()) {
            store.adopt(&merged);
            let Self { colors, recent, .. } = Self::from_saved(saved);
            (self.colors, self.recent) = (colors, recent);
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

    fn folder(name: &str) -> PathBuf {
        crate::testing::test_dir(&format!("swatches-{name}"))
    }

    #[test]
    fn saved_and_loaded_back() {
        let dir = folder("saved");
        let path = dir.join("swatches.json");
        let mut s = Swatches::load_from(path.clone());
        s.colors = vec![[1, 2, 3]];
        s.add([255, 136, 0]);
        s.use_color([9, 9, 9]);
        let back = Swatches::load_from(path);
        assert_eq!(back.colors, vec![[1, 2, 3], [255, 136, 0]]);
        assert_eq!(back.recent, vec![[9, 9, 9]]);
    }

    /// Two windows: the swatch one adds stays when the other picks a color;
    /// a damaged file is set aside, not overwritten.
    #[test]
    fn two_windows_and_damaged_files_keep_the_swatches() {
        let dir = folder("two-windows");
        let path = dir.join("swatches.json");
        let mut a = Swatches::load_from(path.clone());
        let mut b = Swatches::load_from(path.clone());
        a.add([1, 1, 1]);
        b.use_color([2, 2, 2]);
        assert_eq!(b.colors.last(), Some(&[1, 1, 1]));
        b.delete(0);
        a.use_color([3, 3, 3]);
        let back = Swatches::load_from(path.clone());
        assert_eq!(back.recent, vec![[3, 3, 3], [2, 2, 2]]);
        assert_eq!(back.colors.len(), default_colors().len());
        assert_eq!(back.colors.last(), Some(&[1, 1, 1]));
        assert_ne!(back.colors[0], default_colors()[0]);
        std::fs::write(&path, b"{\"swatches\": [\"#0").unwrap();
        let mut c = Swatches::load_from(path.clone());
        assert_eq!(c.colors, default_colors());
        assert!(c.notice.is_some());
        c.use_color([4, 4, 4]);
        assert_eq!(
            std::fs::read(dir.join("swatches.json.bak")).unwrap(),
            b"{\"swatches\": [\"#0"
        );
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
