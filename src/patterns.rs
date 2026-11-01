//! The pattern library: the built-in patterns, and those defined by the
//! user, kept between sessions (`patterns/index.json` and a PNG for each, in
//! the settings folder).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use image::{Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

pub use crate::compose::pattern::Pattern;
use crate::i18n::t;

/// The built-in tiles (English keys of the file, the name when not
/// localized).
fn builtin_tile(index: usize) -> RgbaImage {
    const INK: Rgba<u8> = Rgba([70, 70, 70, 255]);
    const PAPER: Rgba<u8> = Rgba([235, 235, 235, 255]);
    match index {
        // Checkerboard.
        0 => RgbaImage::from_fn(
            16,
            16,
            |x, y| {
                if (x / 8 + y / 8) % 2 == 0 { INK } else { PAPER }
            },
        ),
        // Diagonal lines.
        1 => RgbaImage::from_fn(12, 12, |x, y| if (x + y) % 12 < 3 { INK } else { PAPER }),
        // Dots.
        2 => RgbaImage::from_fn(12, 12, |x, y| {
            let (dx, dy) = (x as f32 - 5.5, y as f32 - 5.5);
            if dx * dx + dy * dy < 9.0 { INK } else { PAPER }
        }),
        // Grid.
        3 => RgbaImage::from_fn(16, 16, |x, y| if x == 0 || y == 0 { INK } else { PAPER }),
        // Bricks.
        4 => RgbaImage::from_fn(32, 16, |x, y| {
            let shift = if y < 8 { 0 } else { 16 };
            if y % 8 == 0 || (x + shift) % 32 == 0 {
                PAPER
            } else {
                Rgba([165, 80, 60, 255])
            }
        }),
        // Canvas: a fixed grain.
        _ => {
            let mut seed = 0x2545_f491u32;
            RgbaImage::from_fn(64, 64, |_, _| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let v = 215 + (seed % 30) as u8;
                Rgba([v, v, v.saturating_sub(8), 255])
            })
        }
    }
}

/// The first patterns of a library; the others can be renamed and deleted.
pub const BUILTIN_COUNT: usize = 6;

fn builtin_name(index: usize) -> String {
    match index {
        0 => t!("pattern8-builtin-checker"),
        1 => t!("pattern8-builtin-lines"),
        2 => t!("pattern8-builtin-dots"),
        3 => t!("pattern8-builtin-grid"),
        4 => t!("pattern8-builtin-bricks"),
        _ => t!("pattern8-builtin-canvas"),
    }
}

fn builtins() -> Vec<Arc<Pattern>> {
    (0..BUILTIN_COUNT)
        .map(|i| Pattern::new(builtin_name(i), builtin_tile(i)))
        .collect()
}

/// The first built-in pattern (the default of a Pattern Overlay).
pub fn default_pattern() -> Arc<Pattern> {
    static FIRST: OnceLock<Arc<Pattern>> = OnceLock::new();
    FIRST
        .get_or_init(|| Pattern::new(builtin_name(0), builtin_tile(0)))
        .clone()
}

#[derive(Serialize, Deserialize)]
struct IndexEntry {
    name: String,
    file: String,
}

/// The patterns of the pickers.
pub struct Library {
    /// Built-in, then the user's, oldest first.
    pub items: Vec<Arc<Pattern>>,
}

impl Default for Library {
    fn default() -> Self {
        Self { items: builtins() }
    }
}

/// File of pattern `id` in the library's folder.
fn file_of(id: u64) -> String {
    format!("{id:016x}.png")
}

/// Largest tile read back: what a document may hold.
const MAX_SIDE: u32 = crate::document::MAX_DIMENSION;

impl Library {
    pub fn get(&self, id: u64) -> Option<&Arc<Pattern>> {
        self.items.iter().find(|p| p.id == id)
    }

    /// The user's patterns.
    fn user(&self) -> &[Arc<Pattern>] {
        &self.items[BUILTIN_COUNT.min(self.items.len())..]
    }

    /// Adds a pattern; the same tile again only takes the new name (not a
    /// built-in's). Whether the library changed.
    pub fn add(&mut self, pattern: Arc<Pattern>) -> bool {
        match self.items.iter().position(|p| p.id == pattern.id) {
            Some(i) if i < BUILTIN_COUNT => false,
            Some(i) => {
                let same = self.items[i].name == pattern.name;
                self.items[i] = pattern;
                !same
            }
            None => {
                self.items.push(pattern);
                true
            }
        }
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.items.len();
        let mut i = 0;
        self.items.retain(|p| {
            i += 1;
            i <= BUILTIN_COUNT || p.id != id
        });
        self.items.len() != before
    }

    pub fn rename(&mut self, id: u64, name: &str) -> bool {
        let found = self.items.iter().position(|p| p.id == id);
        match found {
            Some(i) if i >= BUILTIN_COUNT && self.items[i].name != name => {
                self.items[i] = self.items[i].renamed(name);
                true
            }
            _ => false,
        }
    }

    /// Merges the user's patterns read from a file (they come after those
    /// that are already there only if new).
    pub fn merge(&mut self, read: Vec<Arc<Pattern>>) {
        for p in read {
            if self.items.iter().all(|q| q.id != p.id) {
                self.items.push(p);
            }
        }
    }

    /// Writes the user's patterns in `folder`: the missing PNG files, the
    /// index (whole or not at all), then drops those no longer used. A later
    /// error leaves the former library readable.
    pub fn save(user: &[Arc<Pattern>], folder: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        let mut entries = Vec::with_capacity(user.len());
        for p in user {
            let file = file_of(p.id);
            let path = folder.join(&file);
            if !path.exists() {
                let temp = path.with_extension("png.tmp");
                p.image
                    .save_with_format(&temp, image::ImageFormat::Png)
                    .map_err(std::io::Error::other)?;
                std::fs::rename(&temp, &path)?;
            }
            entries.push(IndexEntry {
                name: p.name.clone(),
                file,
            });
        }
        let json = serde_json::to_vec_pretty(&entries).map_err(std::io::Error::other)?;
        let temp = folder.join("index.json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, folder.join("index.json"))?;
        for entry in std::fs::read_dir(folder)?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".png") && entries.iter().all(|e| e.file != *name) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    /// The patterns saved in `folder`; what is missing or damaged is left
    /// out.
    pub fn read(folder: &Path) -> Vec<Arc<Pattern>> {
        let Ok(json) = std::fs::read(folder.join("index.json")) else {
            return Vec::new();
        };
        let entries: Vec<IndexEntry> = serde_json::from_slice(&json).unwrap_or_default();
        entries
            .into_iter()
            .filter_map(|e| {
                // Only a file's name: the index may have been edited.
                if Path::new(&e.file).file_name()? != e.file.as_str() {
                    return None;
                }
                let reader = image::ImageReader::open(folder.join(&e.file)).ok()?;
                let image = reader
                    .with_guessed_format()
                    .ok()?
                    .decode()
                    .ok()?
                    .into_rgba8();
                let (w, h) = image.dimensions();
                (w > 0 && h > 0 && w <= MAX_SIDE && h <= MAX_SIDE)
                    .then(|| Pattern::new(e.name, image))
            })
            .collect()
    }

    /// A copy of the user's patterns, to be saved on another thread.
    pub fn user_patterns(&self) -> Vec<Arc<Pattern>> {
        self.user().to_vec()
    }
}

/// Folder of the library in the settings.
pub fn folder() -> std::path::PathBuf {
    crate::config::dir().join("patterns")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(v: u8) -> RgbaImage {
        RgbaImage::from_pixel(3, 2, Rgba([v, 1, 2, 255]))
    }

    #[test]
    fn builtins_tile_and_stay_put() {
        let mut lib = Library::default();
        assert_eq!(lib.items.len(), BUILTIN_COUNT);
        let first = lib.items[0].id;
        assert_eq!(default_pattern().id, first);
        assert!(!lib.remove(first) && !lib.rename(first, "x"));
        assert!(
            !lib.add(lib.items[0].renamed("x")),
            "built-ins keep their name"
        );
        assert_eq!(lib.items.len(), BUILTIN_COUNT);
        // All different.
        let mut ids: Vec<u64> = lib.items.iter().map(|p| p.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN_COUNT);
    }

    #[test]
    fn defined_patterns_come_back_in_the_next_session() {
        let dir = std::env::temp_dir().join(format!("atelier-patterns-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut lib = Library::default();
        assert!(lib.add(Pattern::new("A", tile(1))));
        assert!(lib.add(Pattern::new("B", tile(2))));
        let b = lib.items.last().unwrap().id;
        assert!(lib.rename(b, "Brick"));
        Library::save(&lib.user_patterns(), &dir).unwrap();
        let mut next = Library::default();
        next.merge(Library::read(&dir));
        assert_eq!(next.items.len(), BUILTIN_COUNT + 2);
        let back = next.get(b).unwrap();
        assert_eq!(back.name, "Brick");
        assert_eq!(*back.image, tile(2));
        // Deleted: its file goes too; the others stay.
        assert!(next.remove(b));
        Library::save(&next.user_patterns(), &dir).unwrap();
        assert!(!dir.join(file_of(b)).exists());
        assert_eq!(Library::read(&dir).len(), 1);
        // Damage leaves out what it touched.
        std::fs::write(
            dir.join("index.json"),
            b"[{\"name\":\"x\",\"file\":\"../x\"}]",
        )
        .unwrap();
        assert!(Library::read(&dir).is_empty());
        std::fs::write(dir.join("index.json"), b"nonsense").unwrap();
        assert!(Library::read(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_tile_again_only_takes_the_name() {
        let mut lib = Library::default();
        lib.add(Pattern::new("A", tile(1)));
        assert!(lib.add(Pattern::new("B", tile(1))));
        assert_eq!(lib.items.len(), BUILTIN_COUNT + 1);
        assert_eq!(lib.items.last().unwrap().name, "B");
        lib.merge(vec![Pattern::new("C", tile(1)), Pattern::new("D", tile(3))]);
        assert_eq!(lib.items.len(), BUILTIN_COUNT + 2);
    }
}
