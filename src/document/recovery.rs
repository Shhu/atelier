//! Crash recovery files: copies of the modified documents, written aside
//! every few minutes into a folder of this window's own (locked while it
//! runs), removed once saved or closed. A folder whose window ended without
//! removing it (a crash, a kill, a logout) can be taken over by the next
//! window, which offers its documents again.

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Document, FileError, file_io, native};

/// Where the recovery folders go: the local (not roaming) settings on
/// Windows, the state folder elsewhere.
pub fn root() -> PathBuf {
    #[cfg(test)]
    {
        return crate::testing::root().join("recovery");
    }
    #[allow(unreachable_code)]
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(local) = var("LOCALAPPDATA") {
        return local.join("Atelier").join("Recovery");
    }
    let state = var("XDG_STATE_HOME")
        .or_else(|| var("HOME").map(|home| home.join(".local").join("state")))
        .unwrap_or_else(std::env::temp_dir);
    state.join("atelier").join("recovery")
}

/// What is said of a recovery file, beside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct About {
    /// The document's name in its tab.
    pub name: String,
    /// Its file, if it had one.
    pub file: Option<PathBuf>,
}

/// A document found in a folder left behind.
#[derive(Debug)]
pub struct Left {
    pub about: About,
    pub document: PathBuf,
}

/// A recovery folder held by this window: its lock is kept while it lives.
pub struct Folder {
    dir: PathBuf,
    _lock: File,
}

/// The lock file of a recovery folder.
const LOCK: &str = ".lock";

impl Folder {
    /// A new folder of this window's in `root`.
    pub fn start(root: &Path) -> std::io::Result<Self> {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let dir = root.join(format!("{}-{started}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let lock = File::create(dir.join(LOCK))?;
        lock.try_lock().map_err(std::io::Error::other)?;
        Ok(Self { dir, _lock: lock })
    }

    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The folders of `root` whose window is gone, taken over (locked, so
    /// that no other window offers them too), with their documents.
    pub fn left_in(root: &Path) -> Vec<(Folder, Vec<Left>)> {
        let Ok(entries) = std::fs::read_dir(root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let Ok(lock) = File::options()
                .write(true)
                .create(true)
                .truncate(false)
                .open(dir.join(LOCK))
            else {
                continue;
            };
            // Held: that window still runs (or another one took it over).
            if lock.try_lock().is_err() {
                continue;
            }
            let folder = Folder { dir, _lock: lock };
            let left = folder.documents();
            // Empty: removed, unless just made by a window starting now
            // (not locked yet).
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age.as_secs() > 60));
            if !left.is_empty() {
                found.push((folder, left));
            } else if old {
                folder.remove();
            }
        }
        found
    }

    /// The documents in the folder that can be told what they are.
    pub fn documents(&self) -> Vec<Left> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut left: Vec<(std::time::SystemTime, Left)> = entries
            .flatten()
            .filter_map(|e| {
                let document = e.path();
                if document.extension()? != native::EXTENSION {
                    return None;
                }
                let json = std::fs::read(document.with_extension("json")).ok()?;
                let about = serde_json::from_slice(&json).ok()?;
                let when = e.metadata().and_then(|m| m.modified()).ok()?;
                Some((when, Left { about, document }))
            })
            .collect();
        left.sort_by_key(|(when, _)| *when);
        left.into_iter().map(|(_, l)| l).collect()
    }

    /// Where the document `uid` goes.
    fn file(&self, uid: u64) -> PathBuf {
        self.dir.join(format!("{uid}.{}", native::EXTENSION))
    }

    /// Writes (on another thread) what `write_later` made of a document.
    pub fn write(&self, uid: u64, write: Later) -> Result<(), FileError> {
        let path = self.file(uid);
        std::fs::create_dir_all(&self.dir)?;
        (write.0)(&path)?;
        let about = serde_json::to_vec(&write.1).map_err(std::io::Error::other)?;
        crate::store::write(&path.with_extension("json"), &about)?;
        Ok(())
    }

    /// The document `uid` saved or closed: its copy goes.
    pub fn forget(&self, uid: u64) {
        let path = self.file(uid);
        let _ = std::fs::remove_file(path.with_extension("json"));
        let _ = std::fs::remove_file(path);
    }

    /// A document left behind, now this window's `uid`: moved in.
    pub fn take(&self, left: &Left, uid: u64) {
        let path = self.file(uid);
        if std::fs::rename(&left.document, &path).is_ok() {
            let about = serde_json::to_vec(&left.about).unwrap_or_default();
            let _ = crate::store::write(&path.with_extension("json"), &about);
        }
    }

    /// Removes the folder and what is in it.
    pub fn remove(self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A copy of a document to write on another thread, and what is said of it.
pub struct Later(WriteLater, About);

type WriteLater = Box<dyn FnOnce(&Path) -> Result<(), FileError> + Send>;

impl Document {
    /// A copy of this document (its pixels shared, not copied) and the
    /// work of writing it as a recovery file, to do on another thread.
    pub fn recovery_later(&self) -> Later {
        let copy = self.duplicate();
        let about = About {
            name: self.display_name(),
            file: self.path.clone().filter(|_| self.embedded_in.is_none()),
        };
        // Linked files recorded relative to the document's own folder.
        let folder = about.file.as_deref().and_then(Path::parent).map(Path::to_path_buf);
        Later(
            Box::new(move |path| {
                file_io::write_atomic(path, |out| native::write(&copy, out, folder.as_deref()))
            }),
            about,
        )
    }

    /// A recovery file opened again: modified (nothing of it is saved),
    /// with its file if it had one.
    pub fn recovered(left: &Left) -> Result<Document, FileError> {
        let mut doc = Document::open(&left.document)?.doc;
        doc.path = left.about.file.clone();
        doc.mark_modified();
        doc.saved_id = None;
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn documents_left_behind_come_back_once() {
        let root = crate::testing::test_dir("recovery-left");
        let running = Folder::start(&root).unwrap();
        let mut doc = Document::new(RgbaImage::from_pixel(8, 4, Rgba([9, 8, 7, 255])), None);
        doc.path = Some(root.join("photo.png"));
        doc.mark_modified();
        running.write(doc.uid(), doc.recovery_later()).unwrap();
        // That window still runs: nothing to take over.
        assert!(Folder::left_in(&root).is_empty());
        // It ended without removing its folder (a crash).
        let dir = running.dir().to_path_buf();
        drop(running);
        let mut left = Folder::left_in(&root);
        assert_eq!(left.len(), 1);
        let (folder, documents) = left.remove(0);
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].about.name, "photo.png");
        // Another window starting meanwhile doesn't offer them too.
        assert!(Folder::left_in(&root).is_empty());
        let back = Document::recovered(&documents[0]).unwrap();
        assert!(back.modified);
        assert_eq!(back.path, Some(root.join("photo.png")));
        assert_eq!(back.flatten(), doc.flatten());
        // Taken into a new window's folder, under its new identity.
        let next = Folder::start(&root).unwrap();
        next.take(&documents[0], back.uid());
        assert_eq!(next.documents().len(), 1);
        folder.remove();
        assert!(!dir.exists());
        // Saved or closed: gone.
        next.forget(back.uid());
        assert!(next.documents().is_empty());
    }
}
