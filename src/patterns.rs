//! The pattern library: the built-in patterns, and those defined by the
//! user, kept between sessions (`patterns/index.json` and a PNG for each, in
//! the settings folder).
//!
//! What is on the disk is never replaced by what a session believes: a change
//! is applied to the index found there, under a lock (other windows of the
//! application save too), and a PNG only goes when the user removes its
//! pattern. So what can't be read today, or was added by another window, is
//! still there tomorrow.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
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
    /// The file of those read from the disk (not always named after the id:
    /// the index may have been edited).
    files: HashMap<u64, String>,
    /// The language of the built-in patterns' names.
    lang: crate::i18n::Lang,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            items: builtins(),
            files: HashMap::new(),
            lang: crate::i18n::lang(),
        }
    }
}

/// What [`Library::add`] did.
#[derive(Debug, PartialEq)]
pub enum Added {
    New,
    /// This very tile is already there, under this name.
    Already(String),
}

/// What [`Library::read`] found.
#[derive(Default)]
pub struct Loaded {
    /// The patterns, with their files.
    patterns: Vec<(Arc<Pattern>, String)>,
    /// Those listed that can't be read now; their entries and files stay.
    pub left_out: usize,
    /// The damaged index, set aside under this name.
    pub set_aside: Option<String>,
}

/// A change of the library on the disk.
pub enum Change {
    Add { file: String, pattern: Arc<Pattern> },
    Rename { file: String, name: String },
    Remove { file: String },
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
    pub fn user(&self) -> &[Arc<Pattern>] {
        &self.items[BUILTIN_COUNT.min(self.items.len())..]
    }

    /// The built-in patterns named in the current language, once it changed.
    pub fn follow_language(&mut self) {
        let lang = crate::i18n::lang();
        if lang != self.lang {
            self.lang = lang;
            for (i, item) in self.items.iter_mut().take(BUILTIN_COUNT).enumerate() {
                *item = item.renamed(builtin_name(i));
            }
        }
    }

    /// The file of pattern `id`.
    pub fn file_for(&self, id: u64) -> String {
        match self.files.get(&id) {
            Some(file) => file.clone(),
            None => file_of(id),
        }
    }

    /// Adds a pattern; a tile is only there once, under the name it has.
    pub fn add(&mut self, pattern: Arc<Pattern>) -> Added {
        match self.get(pattern.id) {
            Some(p) => Added::Already(p.name.clone()),
            None => {
                self.items.push(pattern);
                Added::New
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

    /// Merges the user's patterns read from the disk (those not there yet).
    pub fn merge(&mut self, read: Loaded) {
        for (p, file) in read.patterns {
            if self.get(p.id).is_none() {
                self.files.insert(p.id, file);
                self.items.push(p);
            }
        }
    }

    /// Applies `changes` to the library in `folder`, whole or not at all: the
    /// new PNG files, the index, then the files of what was removed. Entries
    /// it can't read, and those of other windows, stay as they are.
    pub fn apply(folder: &Path, changes: &[Change]) -> std::io::Result<()> {
        std::fs::create_dir_all(folder)?;
        // Not truncated: another window may hold it.
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(folder.join(".lock"))?;
        // Where locks don't exist (some network drives), saving still works.
        let _ = lock.lock();
        // Under the lock, a temporary file is what a crash left.
        for entry in std::fs::read_dir(folder)?.flatten() {
            if entry.file_name().to_string_lossy().ends_with(".tmp") {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let (mut entries, _) = load_index(folder)?;
        let file_is = |entry: &serde_json::Value, file: &str| {
            entry.get("file").and_then(serde_json::Value::as_str) == Some(file)
        };
        let mut removed = Vec::new();
        for change in changes {
            match change {
                Change::Add { file, pattern } => {
                    write_tile(&folder.join(file), &pattern.image)?;
                    removed.retain(|r| r != file);
                    let entry = serde_json::to_value(IndexEntry {
                        name: pattern.name.clone(),
                        file: file.clone(),
                    })?;
                    match entries.iter_mut().find(|e| file_is(e, file)) {
                        Some(found) => *found = entry,
                        None => entries.push(entry),
                    }
                }
                Change::Rename { file, name } => {
                    let entry = serde_json::to_value(IndexEntry {
                        name: name.clone(),
                        file: file.clone(),
                    })?;
                    match entries.iter_mut().find(|e| file_is(e, file)) {
                        Some(found) => *found = entry,
                        // Taken in from the folder, the index having no name
                        // for it (see `Library::read`).
                        None if folder.join(file).exists() => entries.push(entry),
                        None => {}
                    }
                }
                Change::Remove { file } => {
                    entries.retain(|e| !file_is(e, file));
                    removed.push(file.clone());
                }
            }
        }
        let json = serde_json::to_vec_pretty(&entries)?;
        let temp = folder.join("index.json.tmp");
        std::fs::write(&temp, json)?;
        std::fs::rename(&temp, folder.join("index.json"))?;
        for file in removed {
            // Only a file's name: the index may have been edited.
            if Path::new(&file).file_name().is_some_and(|n| *n == *file) {
                let _ = std::fs::remove_file(folder.join(file));
            }
        }
        Ok(())
    }

    /// The patterns saved in `folder`. What can't be read is left out, not
    /// lost; a PNG the index doesn't list (its index was damaged, or a crash
    /// came between the two) is taken in again.
    pub fn read(folder: &Path) -> Loaded {
        let mut out = Loaded::default();
        let entries = match load_index(folder) {
            Ok((entries, set_aside)) => {
                out.set_aside = set_aside;
                entries
            }
            Err(e) => {
                crate::diag::warn(format!("pattern library not read: {e}"));
                out.left_out = 1;
                return out;
            }
        };
        let mut memory = crate::document::free_memory();
        let mut listed = Vec::new();
        for value in entries {
            if let Some(file) = value.get("file").and_then(serde_json::Value::as_str) {
                listed.push(file.to_owned());
            }
            let tile = serde_json::from_value::<IndexEntry>(value)
                .ok()
                .and_then(|e| Some((read_tile(folder, &e.file, &mut memory)?, e)));
            match tile {
                Some((image, e)) => out.patterns.push((Pattern::new(e.name, image), e.file)),
                None => out.left_out += 1,
            }
        }
        let Ok(dir) = std::fs::read_dir(folder) else {
            return out;
        };
        let mut unlisted: Vec<String> = dir
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".png") && !listed.contains(name))
            .collect();
        unlisted.sort();
        // Their names in the index set aside, where it still tells them.
        let old_names = out
            .set_aside
            .as_ref()
            .and_then(|b| std::fs::read(folder.join(b)).ok())
            .map(|json| entries_in(&String::from_utf8_lossy(&json)))
            .unwrap_or_default();
        let old_name = |file: &str| {
            old_names.iter().find(|e| e.file == file).map(|e| e.name.clone())
        };
        for file in unlisted {
            match read_tile(folder, &file, &mut memory) {
                Some(image) => {
                    let name = old_name(&file).unwrap_or_else(|| {
                        let names: Vec<&str> =
                            out.patterns.iter().map(|(p, _)| p.name.as_str()).collect();
                        default_name(&names, out.patterns.len())
                    });
                    out.patterns.push((Pattern::new(name, image), file));
                }
                None => out.left_out += 1,
            }
        }
        out
    }
}

/// The entries still whole in a damaged index: each `{…}` that reads as one.
fn entries_in(text: &str) -> Vec<IndexEntry> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start..];
        let Some(end) = rest.find('}') else { break };
        if let Ok(e) = serde_json::from_str::<IndexEntry>(&rest[..=end]) {
            out.push(e);
        }
        rest = &rest[1..];
    }
    out
}

/// "Pattern N" for a new pattern, `count` user patterns being there: N is
/// `count` + 1, or the next number none of `names` has.
pub fn default_name(names: &[&str], count: usize) -> String {
    (count + 1..)
        .map(|n| t!("pattern8-default-name", number = n))
        .find(|name| !names.contains(&name.as_str()))
        .unwrap_or_default()
}

/// The entries of the index, each as it is written (a bad one stays), and
/// the name a damaged index was set aside under. An index that can't be read
/// for now is an error: nothing may be written over it.
fn load_index(folder: &Path) -> std::io::Result<(Vec<serde_json::Value>, Option<String>)> {
    let path = folder.join("index.json");
    let json = match std::fs::read(&path) {
        Ok(json) => json,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), None)),
        Err(e) => return Err(e),
    };
    if let Ok(serde_json::Value::Array(entries)) = serde_json::from_slice(&json) {
        return Ok((entries, None));
    }
    let backup = crate::actions::set_aside(&path).ok_or_else(|| {
        std::io::Error::other("the damaged index of the pattern library can't be set aside")
    })?;
    let name = backup.file_name().unwrap_or_default();
    Ok((Vec::new(), Some(name.to_string_lossy().into_owned())))
}

/// The pixels of a file of the library, if readable within `memory`, and what
/// they will take is taken from it.
fn read_tile(folder: &Path, file: &str, memory: &mut u64) -> Option<RgbaImage> {
    // Only a file's name: the index may have been edited.
    if Path::new(file).file_name()? != file {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(*memory);
    let mut reader = image::ImageReader::open(folder.join(file))
        .ok()?
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    let image = reader.decode().ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    // The tile and its smaller versions.
    *memory = memory.saturating_sub(w as u64 * h as u64 * 6);
    (w > 0 && h > 0).then_some(image)
}

/// Writes a tile's file, unless it is there and good. One that is damaged,
/// or holds something else, is set aside; one that can't be read for now
/// (memory) is kept.
fn write_tile(path: &Path, image: &RgbaImage) -> std::io::Result<()> {
    if path.exists() {
        if tile_there(path, image) != Some(false) {
            return Ok(());
        }
        crate::actions::set_aside(path)
            .ok_or_else(|| std::io::Error::other("a damaged pattern can't be set aside"))?;
    }
    let temp = path.with_extension("png.tmp");
    image
        .save_with_format(&temp, image::ImageFormat::Png)
        .map_err(std::io::Error::other)?;
    std::fs::rename(&temp, path)
}

/// Whether the file at `path` holds `image`; `None` when that can't be told
/// now (memory, or the file can't be opened). A file is opened before it is
/// read, so an error reading it is in its data.
fn tile_there(path: &Path, image: &RgbaImage) -> Option<bool> {
    use image::ImageError;
    let open = || image::ImageReader::open(path).ok()?.with_guessed_format().ok();
    match open()?.into_dimensions() {
        Ok(d) if d != image.dimensions() => return Some(false),
        Ok(_) => {}
        Err(ImageError::Limits(_)) => return None,
        Err(_) => return Some(false),
    }
    let bytes = image.as_raw().len() as u64;
    if bytes > crate::document::free_memory() {
        return None;
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(2 * bytes + (1 << 20));
    let mut reader = open()?;
    reader.limits(limits);
    match reader.decode() {
        Ok(d) => Some(d.into_rgba8() == *image),
        // Cut short or garbled reads as an IO error too: damaged.
        Err(ImageError::Limits(_)) => None,
        Err(_) => Some(false),
    }
}

/// The changes of the library on their way to the disk: one thread, in order,
/// what failed being tried again with the next ones. What is waiting is
/// written before the application ends.
#[derive(Default)]
pub struct Saver {
    /// Another folder than the settings' (tests).
    folder: Option<PathBuf>,
    worker: Option<Worker>,
}

struct Worker {
    queue: Sender<Change>,
    errors: Receiver<String>,
    thread: std::thread::JoinHandle<()>,
}

impl Saver {
    #[cfg(test)]
    fn in_folder(folder: &Path) -> Self {
        Self {
            folder: Some(folder.to_path_buf()),
            worker: None,
        }
    }

    pub fn send(&mut self, change: Change) {
        // Tests of the interface don't touch the user's library.
        if cfg!(test) && self.folder.is_none() {
            return;
        }
        let folder = self.folder.clone().unwrap_or_else(folder);
        let worker = self.worker.get_or_insert_with(|| {
            let (queue, changes) = channel();
            let (report, errors) = channel();
            let thread = std::thread::spawn(move || save_changes(&folder, &changes, &report));
            Worker {
                queue,
                errors,
                thread,
            }
        });
        let _ = worker.queue.send(change);
    }

    /// The last error since the last call, if any.
    pub fn error(&self) -> Option<String> {
        let mut last = None;
        let worker = self.worker.as_ref()?;
        loop {
            match worker.errors.try_recv() {
                Ok(e) => last = Some(e),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return last,
            }
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            drop(worker.queue);
            let _ = worker.thread.join();
        }
    }
}

/// The saving thread: until the queue is closed, then a last try.
fn save_changes(folder: &Path, changes: &Receiver<Change>, report: &Sender<String>) {
    let mut waiting = Vec::new();
    while let Ok(change) = changes.recv() {
        waiting.push(change);
        waiting.extend(changes.try_iter());
        match Library::apply(folder, &waiting) {
            Ok(()) => waiting.clear(),
            Err(e) => {
                crate::diag::warn(format!("pattern library not saved: {e}"));
                // In the interface's language where it has words for it.
                let _ = report.send(crate::document::FileError::Io(e).to_string());
            }
        }
    }
    if !waiting.is_empty()
        && let Err(e) = Library::apply(folder, &waiting)
    {
        crate::diag::warn(format!("pattern library not saved: {e}"));
    }
}

/// Folder of the library in the settings.
pub fn folder() -> std::path::PathBuf {
    crate::config::dir().join("patterns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tile(v: u8) -> RgbaImage {
        RgbaImage::from_pixel(3, 2, Rgba([v, 1, 2, 255]))
    }

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("atelier-patterns-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn add(p: &Arc<Pattern>) -> Change {
        Change::Add {
            file: file_of(p.id),
            pattern: p.clone(),
        }
    }

    fn pngs(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".png"))
            .count()
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = Library::read(dir)
            .patterns
            .iter()
            .map(|(p, _)| p.name.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn builtins_tile_and_stay_put() {
        let mut lib = Library::default();
        assert_eq!(lib.items.len(), BUILTIN_COUNT);
        let first = lib.items[0].id;
        assert_eq!(default_pattern().id, first);
        assert!(!lib.remove(first) && !lib.rename(first, "x"));
        let name = lib.items[0].name.clone();
        assert_eq!(lib.add(lib.items[0].renamed("x")), Added::Already(name));
        assert_eq!(lib.items.len(), BUILTIN_COUNT);
        // All different.
        let mut ids: Vec<u64> = lib.items.iter().map(|p| p.id).collect();
        ids.dedup();
        assert_eq!(ids.len(), BUILTIN_COUNT);
    }

    #[test]
    fn defined_patterns_come_back_in_the_next_session() {
        let dir = folder("next");
        let mut lib = Library::default();
        let (a, b) = (Pattern::new("A", tile(1)), Pattern::new("B", tile(2)));
        assert_eq!(lib.add(a.clone()), Added::New);
        assert_eq!(lib.add(b.clone()), Added::New);
        assert!(lib.rename(b.id, "Brick"));
        let changes = [
            add(&a),
            add(&b),
            Change::Rename {
                file: lib.file_for(b.id),
                name: "Brick".into(),
            },
        ];
        Library::apply(&dir, &changes).unwrap();
        let mut next = Library::default();
        next.merge(Library::read(&dir));
        assert_eq!(next.items.len(), BUILTIN_COUNT + 2);
        let back = next.get(b.id).unwrap();
        assert_eq!(back.name, "Brick");
        assert_eq!(*back.image, tile(2));
        // Deleted: its file goes too; the others stay.
        assert!(next.remove(b.id));
        let file = next.file_for(b.id);
        Library::apply(&dir, &[Change::Remove { file }]).unwrap();
        assert!(!dir.join(file_of(b.id)).exists());
        assert_eq!(Library::read(&dir).patterns.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_tile_again_changes_nothing() {
        let mut lib = Library::default();
        lib.add(Pattern::new("A", tile(1)));
        assert_eq!(
            lib.add(Pattern::new("B", tile(1))),
            Added::Already("A".into())
        );
        assert_eq!(lib.items.len(), BUILTIN_COUNT + 1);
        assert_eq!(lib.items.last().unwrap().name, "A");
    }

    #[test]
    fn a_damaged_index_loses_nothing() {
        let dir = folder("index");
        let (a, b) = (Pattern::new("A", tile(1)), Pattern::new("B", tile(2)));
        Library::apply(&dir, &[add(&a), add(&b)]).unwrap();
        let json = std::fs::read(dir.join("index.json")).unwrap();
        std::fs::write(dir.join("index.json"), &json[..json.len() / 2]).unwrap();
        // Set aside, its tiles taken in again from the folder.
        let read = Library::read(&dir);
        assert_eq!(read.set_aside.as_deref(), Some("index.json.bak"));
        assert_eq!(read.patterns.len(), 2);
        assert_eq!(
            std::fs::read(dir.join("index.json.bak")).unwrap(),
            json[..json.len() / 2]
        );
        // The next change leaves them on the disk, and a later session finds
        // all three.
        let c = Pattern::new("C", tile(3));
        Library::apply(&dir, &[add(&c)]).unwrap();
        assert_eq!(pngs(&dir), 3);
        let read = Library::read(&dir);
        assert_eq!(read.patterns.len(), 3);
        assert_eq!(read.left_out, 0);
        // Renamed once taken in: its entry is written.
        let file = file_of(a.id);
        Library::apply(
            &dir,
            &[Change::Rename {
                file,
                name: "Back".into(),
            }],
        )
        .unwrap();
        assert!(names(&dir).contains(&"Back".to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_entry_or_png_is_left_out_and_kept() {
        let dir = folder("entry");
        let (a, b) = (Pattern::new("A", tile(1)), Pattern::new("B", tile(2)));
        Library::apply(&dir, &[add(&a), add(&b)]).unwrap();
        // A bad entry beside the good ones; a PNG cut in half.
        let mut entries: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(dir.join("index.json")).unwrap()).unwrap();
        entries.push(serde_json::json!({ "name": 3 }));
        entries.push(serde_json::json!({ "name": "x", "file": "../x" }));
        std::fs::write(
            dir.join("index.json"),
            serde_json::to_vec(&entries).unwrap(),
        )
        .unwrap();
        let path = dir.join(file_of(a.id));
        let good = std::fs::read(&path).unwrap();
        std::fs::write(&path, &good[..good.len() / 2]).unwrap();
        let read = Library::read(&dir);
        assert_eq!(read.patterns.len(), 1);
        assert_eq!(read.left_out, 3);
        // A later change keeps every one of them: repaired, A comes back.
        let c = Pattern::new("C", tile(3));
        Library::apply(&dir, &[add(&c)]).unwrap();
        assert_eq!(pngs(&dir), 3);
        std::fs::write(&path, &good).unwrap();
        assert_eq!(names(&dir), ["A", "B", "C"]);
        let index = std::fs::read_to_string(dir.join("index.json")).unwrap();
        assert!(index.contains("../x"), "{index}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tile_defined_again_replaces_its_damaged_file() {
        let dir = folder("redefine");
        let a = Pattern::new("A", tile(1));
        Library::apply(&dir, &[add(&a)]).unwrap();
        let path = dir.join(file_of(a.id));
        let good = std::fs::read(&path).unwrap();
        std::fs::write(&path, &good[..good.len() / 2]).unwrap();
        assert!(Library::read(&dir).patterns.is_empty());
        Library::apply(&dir, &[add(&a)]).unwrap();
        assert_eq!(names(&dir), ["A"]);
        assert!(dir.join(format!("{}.bak", file_of(a.id))).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_windows_keep_each_others_patterns() {
        let dir = folder("two");
        let (a, b) = (Pattern::new("A", tile(1)), Pattern::new("B", tile(2)));
        Library::apply(&dir, &[add(&a)]).unwrap();
        // Another window, its library older than this.
        Library::apply(&dir, &[add(&b)]).unwrap();
        assert_eq!(pngs(&dir), 2);
        assert_eq!(names(&dir), ["A", "B"]);
        // And at the same time.
        let threads: Vec<_> = (10..18)
            .map(|v| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let p = Pattern::new(format!("{v}"), tile(v));
                    Library::apply(&dir, &[add(&p)]).unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(names(&dir).len(), 10);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_library_unreadable_for_now_is_not_written_over() {
        let dir = folder("unreadable");
        std::fs::create_dir_all(dir.join("index.json")).unwrap();
        let a = Pattern::new("A", tile(1));
        assert!(Library::apply(&dir, &[add(&a)]).is_err());
        assert_eq!(Library::read(&dir).left_out, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_saver_keeps_the_order_retries_and_finishes_before_the_end() {
        let dir = folder("saver");
        // A folder that can't be written yet: a file in its place.
        std::fs::write(&dir, b"").unwrap();
        let mut saver = Saver::in_folder(&dir);
        let (a, b) = (Pattern::new("A", tile(1)), Pattern::new("B", tile(2)));
        saver.send(add(&a));
        let mut error = None;
        for _ in 0..500 {
            error = saver.error();
            if error.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(error.is_some(), "the failure is reported");
        std::fs::remove_file(&dir).unwrap();
        saver.send(add(&b));
        saver.send(Change::Rename {
            file: file_of(b.id),
            name: "B2".into(),
        });
        saver.send(Change::Remove {
            file: file_of(a.id),
        });
        // At the end of the session everything is written.
        drop(saver);
        assert_eq!(names(&dir), ["B2"]);
        assert!(!dir.join(file_of(a.id)).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_a_crash_left_goes_at_the_next_save() {
        let dir = folder("temp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("0000.png.tmp"), b"half").unwrap();
        let a = Pattern::new("A", tile(1));
        Library::apply(&dir, &[add(&a)]).unwrap();
        assert!(!dir.join("0000.png.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Past the reader's default limit (512 MiB): what is saved reads back.
    #[test]
    fn a_big_tile_comes_back() {
        let dir = folder("big");
        let p = Pattern::new(
            "big",
            RgbaImage::from_pixel(11600, 11600, Rgba([9, 9, 9, 255])),
        );
        Library::apply(&dir, &[add(&p)]).unwrap();
        drop(p);
        assert_eq!(names(&dir), ["big"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn built_in_names_follow_the_language() {
        crate::i18n::set_lang(crate::i18n::Lang::En);
        let mut lib = Library::default();
        let english = lib.items[0].name.clone();
        lib.add(Pattern::new("Mine", tile(1)));
        crate::i18n::set_lang(crate::i18n::Lang::Fr);
        lib.follow_language();
        let french = lib.items[0].name.clone();
        crate::i18n::set_lang(crate::i18n::Lang::En);
        assert_ne!(english, french);
        assert_eq!(lib.items.last().unwrap().name, "Mine");
    }
}
