//! Recently opened or saved files, most recent first, kept in the settings
//! folder (one path per line, see [`encode`]; shared with the other
//! windows, see `store`), with what the start screen shows of them.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::config;
use crate::store::{self, Format, Store};

const MAX: usize = 8;
/// Thumbnail size (pixels), twice the displayed size for sharp screens.
const THUMB: (u32, u32) = (108, 68);

/// A recent file as shown: path, dimensions, thumbnail once loaded.
pub struct Entry {
    pub path: PathBuf,
    pub size: (u32, u32),
    pub thumb: Option<TextureHandle>,
    /// Its place in the list.
    rank: usize,
}

/// A recent file that is gone for good: missing from a folder that is
/// there. A file on an unplugged drive or a sleeping network share (its
/// folder unreachable too) stays in the list.
pub fn gone(path: &Path) -> bool {
    let missing = matches!(
        std::fs::metadata(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    );
    missing && path.parent().is_some_and(|dir| dir.is_dir())
}

#[derive(Default)]
pub struct Recent {
    paths: Vec<PathBuf>,
    /// Existing files with their dimensions; `None`: to read again.
    entries: Option<Vec<Entry>>,
    /// Dimensions, then thumbnails, read in the background.
    thumbs: Option<Receiver<Found>>,
    /// Number of dimensions asked for.
    expected: usize,
    /// The list's file (none in tests).
    store: Option<Store>,
}

/// What the background threads found.
enum Found {
    /// The dimensions of the file at a place in the list, `None` if it
    /// can't be read (missing): sent as each answers.
    Size(usize, PathBuf, Option<(u32, u32)>),
    Thumb(PathBuf, ColorImage),
}

/// How long the start screen waits for the dimensions before showing the
/// files that answered (the others come later; slow or network drives).
const SIZES_WAIT: Duration = Duration::from_millis(100);

impl Recent {
    pub fn load() -> Self {
        // Tests start with no recent files and never write the list.
        if cfg!(test) {
            return Self::default();
        }
        Self::load_from(file())
    }

    fn load_from(path: PathBuf) -> Self {
        let mut store = Store::new(path, LINES).tidying(|v| {
            if let Some(list) = v.as_array_mut() {
                list.truncate(MAX);
            }
        });
        let paths = match store.read() {
            store::Found::Value(v) => paths_of(&v),
            _ => Vec::new(),
        };
        store.adopt(&lines_of(&paths));
        Self {
            paths,
            store: Some(store),
            ..Default::default()
        }
    }

    /// Puts `path` first and saves the list.
    pub fn add(&mut self, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if encode(&path).is_none() {
            return;
        }
        self.paths.retain(|p| *p != path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX);
        self.entries = None;
        self.save();
    }

    /// The listed files, most recent first (File › Open Recent).
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Clear Recent File List.
    pub fn clear(&mut self) {
        self.paths.clear();
        self.entries = None;
        self.save();
    }

    /// Takes `path` off the list for good (the user removed it).
    pub fn forget(&mut self, path: &Path) {
        self.paths.retain(|p| p != path);
        self.remove(path);
        self.save();
    }

    /// Saved, merged with the files another window added or removed
    /// meanwhile, which then show here too.
    fn save(&mut self) {
        let Some(store) = &mut self.store else { return };
        if let Ok(merged) = store.save(lines_of(&self.paths)) {
            store.adopt(&merged);
            self.paths = paths_of(&merged);
            self.entries = None;
        }
    }

    /// Forgets a file that could not be opened.
    pub fn remove(&mut self, path: &Path) {
        if let Some(entries) = &mut self.entries {
            entries.retain(|e| e.path != path);
        }
    }

    /// The files that still exist, with their dimensions; thumbnails arrive
    /// as they are decoded.
    /// The files read off the UI thread (a drive may be slow or gone): a
    /// damaged file costs its entry, never the app.
    pub fn entries(&mut self, ctx: &egui::Context) -> &[Entry] {
        let mut first = false;
        let entries = self.entries.get_or_insert_with(|| {
            let (tx, rx) = channel();
            let paths = self.paths.clone();
            let repaint = ctx.clone();
            self.expected = paths.len();
            std::thread::spawn(move || {
                // Each file on a thread of its own: a sleeping disk or an
                // offline share holds back its own entry only.
                let (found, sizes) = channel();
                for (rank, path) in paths.into_iter().enumerate() {
                    let found = found.clone();
                    std::thread::spawn(move || {
                        let size = catch_unwind(|| crate::document::file_size(&path));
                        let _ = found.send((rank, path, size.ok().flatten()));
                    });
                }
                drop(found);
                // The thumbnails one at a time, the dimensions first.
                let mut queue = std::collections::VecDeque::new();
                let mut done = false;
                loop {
                    let mut answers = Vec::new();
                    if queue.is_empty() && !done {
                        match sizes.recv() {
                            Ok(a) => answers.push(a),
                            Err(_) => done = true,
                        }
                    }
                    answers.extend(sizes.try_iter());
                    for (rank, path, size) in answers {
                        if size.is_some() {
                            queue.push_back(path.clone());
                        }
                        if tx.send(Found::Size(rank, path, size)).is_err() {
                            return;
                        }
                        repaint.request_repaint();
                    }
                    let Some(path) = queue.pop_front() else {
                        if done {
                            return;
                        }
                        continue;
                    };
                    let thumb = catch_unwind(AssertUnwindSafe(|| {
                        crate::document::file_thumbnail(&path, THUMB.0, THUMB.1)
                    }));
                    let Ok(Some(small)) = thumb else {
                        continue;
                    };
                    let image = ColorImage::from_rgba_unmultiplied(
                        [small.width() as usize, small.height() as usize],
                        small.as_raw(),
                    );
                    if tx.send(Found::Thumb(path, image)).is_err() {
                        return;
                    }
                    repaint.request_repaint();
                }
            });
            self.thumbs = Some(rx);
            first = true;
            Vec::new()
        });
        if let Some(rx) = &self.thumbs {
            let mut waited = Vec::new();
            if first {
                let deadline = std::time::Instant::now() + SIZES_WAIT;
                let mut sizes = 0;
                while sizes < self.expected {
                    let left = deadline.saturating_duration_since(std::time::Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(found) => {
                            sizes += usize::from(matches!(found, Found::Size(..)));
                            waited.push(found);
                        }
                        Err(_) => break,
                    }
                }
            }
            for found in waited.into_iter().chain(rx.try_iter()) {
                match found {
                    Found::Size(rank, path, Some(size)) => {
                        let at = entries.partition_point(|e| e.rank < rank);
                        let entry = Entry {
                            path,
                            size,
                            thumb: None,
                            rank,
                        };
                        entries.insert(at, entry);
                    }
                    Found::Size(_, _, None) => {}
                    Found::Thumb(path, image) => {
                        if let Some(entry) = entries.iter_mut().find(|e| e.path == path) {
                            let name = format!("recent-{}", path.display());
                            let options = TextureOptions::LINEAR;
                            entry.thumb = Some(ctx.load_texture(name, image, options));
                        }
                    }
                }
            }
        }
        entries
    }
}

fn file() -> PathBuf {
    config::dir().join("recent")
}

/// The list's file: one line per path, as a list of lines to merge.
const LINES: Format = Format {
    decode: |bytes| {
        let lines = bytes.split(|&b| b == b'\n').filter_map(decode);
        Some(lines.filter_map(|p| line_of(&p)).map(serde_json::Value::String).collect())
    },
    encode: |value| {
        let lines = value.as_array().into_iter().flatten();
        lines
            .filter_map(serde_json::Value::as_str)
            .flat_map(|line| [line.as_bytes(), b"\n"].concat())
            .collect()
    },
};

/// A path as a line of text: [`encode`]d, with the bytes of a name that is
/// not UTF-8 written %XX (which [`decode`] reads back as well).
fn line_of(path: &Path) -> Option<String> {
    let line = encode(path)?;
    Some(String::from_utf8(line).unwrap_or_else(|e| {
        let mut text = String::new();
        for b in e.into_bytes() {
            match b {
                0..0x80 => text.push(b as char),
                b => text.push_str(&format!("%{b:02X}")),
            }
        }
        text
    }))
}

fn lines_of(paths: &[PathBuf]) -> serde_json::Value {
    paths.iter().filter_map(|p| line_of(p)).map(serde_json::Value::String).collect()
}

fn paths_of(list: &serde_json::Value) -> Vec<PathBuf> {
    let lines = list.as_array().into_iter().flatten();
    let lines = lines.filter_map(serde_json::Value::as_str);
    lines.filter_map(|l| decode(l.as_bytes())).take(MAX).collect()
}

/// A path as a line of the list: "%", line feeds and carriage returns are
/// written %XX, and on Unix a name that is not UTF-8 is kept byte for byte.
/// `None` for a path that cannot be written (not Unicode, on Windows).
fn encode(path: &Path) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    for &b in path_bytes(path)? {
        match b {
            b'%' | b'\n' | b'\r' => line.extend(format!("%{b:02X}").bytes()),
            b => line.push(b),
        }
    }
    Some(line)
}

/// Reverse of [`encode`] (a carriage return left by a CRLF file is dropped).
fn decode(line: &[u8]) -> Option<PathBuf> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut bytes = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        let hex = line
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (line[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                bytes.push(b);
                i += 3;
            }
            (b, _) => {
                bytes.push(b);
                i += 1;
            }
        }
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    path_from(bytes)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Option<&[u8]> {
    use std::os::unix::ffi::OsStrExt;
    Some(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Option<&[u8]> {
    path.to_str().map(str::as_bytes)
}

#[cfg(unix)]
fn path_from(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(std::ffi::OsString::from_vec(bytes).into())
}

#[cfg(not(unix))]
fn path_from(bytes: Vec<u8>) -> Option<PathBuf> {
    String::from_utf8(bytes).ok().map(PathBuf::from)
}

#[cfg(test)]
impl Recent {
    /// List not read from (nor saved to) the settings.
    pub fn with_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Forgotten only when its folder is there and the file isn't.
    #[test]
    fn a_file_on_a_missing_drive_is_not_gone() {
        let dir = std::env::temp_dir();
        assert!(gone(&dir.join("atelier-surely-not-here.png")));
        assert!(!gone(&dir.join("no-such-folder-atelier/photo.png")));
        assert!(!gone(&dir));
    }

    #[test]
    fn paths_survive_the_list_file() {
        let paths = [
            "/home/me/photo.png",
            "/home/me/two\nlines.atelier",
            "/home/me/100% done\r.png",
            "C:\\Users\\me\\été.jpg",
        ];
        for p in paths {
            let line = encode(Path::new(p)).unwrap();
            assert!(!line.contains(&b'\n') && !line.contains(&b'\r'), "{p:?}");
            assert_eq!(decode(&line).as_deref(), Some(Path::new(p)));
        }
        // Lines written before: as they are.
        assert_eq!(
            decode(b"/old/list/a.png\r").as_deref(),
            Some(Path::new("/old/list/a.png"))
        );
        assert_eq!(decode(b"  "), None);
    }

    #[test]
    fn entries_are_read_off_the_ui_thread_and_damaged_files_skipped() {
        let dir = crate::testing::test_dir("recent-entries");
        let good = dir.join("good.png");
        image::RgbaImage::new(3, 2).save(&good).unwrap();
        let damaged = dir.join("damaged.psd");
        std::fs::write(&damaged, b"8BPS\0\x01garbage").unwrap();
        let gone = dir.join("gone.png");
        let mut recent = Recent::with_paths(vec![damaged, gone, good.clone()]);
        let ctx = egui::Context::default();
        let mut entries = Vec::new();
        for _ in crate::testing::patience() {
            entries = recent
                .entries(&ctx)
                .iter()
                .map(|e| (e.path.clone(), e.size))
                .collect();
            if !entries.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(entries, [(good, (3, 2))]);
    }

    /// A file that doesn't answer (an offline share: here a pipe nobody
    /// writes to) holds back its own entry only; the list keeps its order.
    #[cfg(unix)]
    #[test]
    fn a_silent_file_holds_back_only_its_entry() {
        let dir = crate::testing::test_dir("recent-silent");
        let silent = dir.join("share.png");
        let made = std::process::Command::new("mkfifo").arg(&silent).status();
        assert!(made.is_ok_and(|s| s.success()));
        let (a, b) = (dir.join("a.png"), dir.join("b.png"));
        image::RgbaImage::new(3, 2).save(&a).unwrap();
        image::RgbaImage::new(5, 4).save(&b).unwrap();
        let mut recent = Recent::with_paths(vec![b.clone(), silent.clone(), a.clone()]);
        let ctx = egui::Context::default();
        let mut entries = Vec::new();
        for _ in crate::testing::patience() {
            entries = recent
                .entries(&ctx)
                .iter()
                .map(|e| (e.path.clone(), e.size))
                .collect();
            if entries.len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(entries, [(b, (5, 4)), (a, (3, 2))]);
        // The pipe answers at last (nothing readable): its reader is let go.
        drop(std::fs::OpenOptions::new().write(true).open(&silent));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_kept() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.png"));
        assert_eq!(decode(&encode(path).unwrap()).as_deref(), Some(path));
        // Through the shared list too.
        let dir = crate::testing::test_dir("recent-non-utf8");
        let mut recent = Recent::load_from(dir.join("recent"));
        recent.add(path);
        assert_eq!(Recent::load_from(dir.join("recent")).paths(), &[path.to_path_buf()]);
    }

    /// Two windows: the files each opens stay in the list, most recent
    /// first; one removed in a window is removed.
    #[test]
    fn two_windows_share_the_list() {
        let dir = crate::testing::test_dir("recent-two-windows");
        let file = dir.join("recent");
        let p = |name: &str| dir.join(name);
        let mut a = Recent::load_from(file.clone());
        a.add(&p("one.png"));
        let mut b = Recent::load_from(file.clone());
        a.add(&p("two.png"));
        b.add(&p("three.png"));
        assert_eq!(b.paths(), &[p("three.png"), p("two.png"), p("one.png")]);
        a.forget(&p("one.png"));
        for i in 0..MAX {
            b.add(&p(&format!("more{i}.png")));
        }
        let read = Recent::load_from(file).paths().to_vec();
        assert_eq!(read.len(), MAX);
        assert!(!read.contains(&p("one.png")));
        assert_eq!(read[0], p(&format!("more{}.png", MAX - 1)));
    }
}
