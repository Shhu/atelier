//! Recently opened or saved files, most recent first, kept in the settings
//! folder (one path per line, see [`encode`]), with what the start screen
//! shows of them.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};

use crate::config;

const MAX: usize = 8;
/// Thumbnail size (pixels), twice the displayed size for sharp screens.
const THUMB: (u32, u32) = (108, 68);

/// A recent file as shown: path, dimensions, thumbnail once loaded.
pub struct Entry {
    pub path: PathBuf,
    pub size: (u32, u32),
    pub thumb: Option<TextureHandle>,
}

#[derive(Default)]
pub struct Recent {
    paths: Vec<PathBuf>,
    /// Existing files with their dimensions; `None`: to read again.
    entries: Option<Vec<Entry>>,
    /// Dimensions, then thumbnails, read in the background.
    thumbs: Option<Receiver<Found>>,
}

/// What the background thread found.
enum Found {
    /// The files that exist, with their dimensions, in the list's order.
    Sizes(Vec<(PathBuf, (u32, u32))>),
    Thumb(PathBuf, ColorImage),
}

/// How long the start screen waits for the dimensions before showing the
/// list without them (they come later; slow or network drives).
const SIZES_WAIT: Duration = Duration::from_millis(100);

impl Recent {
    pub fn load() -> Self {
        let paths = std::fs::read(file())
            .unwrap_or_default()
            .split(|&b| b == b'\n')
            .filter_map(decode)
            .take(MAX)
            .collect();
        Self {
            paths,
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

    fn save(&self) {
        let text: Vec<u8> = self
            .paths
            .iter()
            .filter_map(|p| encode(p))
            .flat_map(|mut line| {
                line.push(b'\n');
                line
            })
            .collect();
        let _ = std::fs::create_dir_all(config::dir());
        let _ = std::fs::write(file(), text);
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
            std::thread::spawn(move || {
                let sizes: Vec<(PathBuf, (u32, u32))> = paths
                    .into_iter()
                    .filter_map(|path| {
                        let size = catch_unwind(|| crate::document::file_size(&path));
                        Some((path.clone(), size.ok()??))
                    })
                    .collect();
                let paths: Vec<PathBuf> = sizes.iter().map(|(p, _)| p.clone()).collect();
                if tx.send(Found::Sizes(sizes)).is_err() {
                    return;
                }
                repaint.request_repaint();
                for path in paths {
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
            let waited = first.then(|| rx.recv_timeout(SIZES_WAIT).ok()).flatten();
            for found in waited.into_iter().chain(rx.try_iter()) {
                match found {
                    Found::Sizes(sizes) => {
                        *entries = sizes
                            .into_iter()
                            .map(|(path, size)| Entry {
                                path,
                                size,
                                thumb: None,
                            })
                            .collect();
                    }
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
        let dir = std::env::temp_dir().join("atelier-recent-entries");
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.png");
        image::RgbaImage::new(3, 2).save(&good).unwrap();
        let damaged = dir.join("damaged.psd");
        std::fs::write(&damaged, b"8BPS\0\x01garbage").unwrap();
        let gone = dir.join("gone.png");
        let mut recent = Recent::with_paths(vec![damaged, gone, good.clone()]);
        let ctx = egui::Context::default();
        let mut entries = Vec::new();
        for _ in 0..100 {
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

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_kept() {
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.png"));
        assert_eq!(decode(&encode(path).unwrap()).as_deref(), Some(path));
    }
}
