//! Settings files that every window of Atelier shares (actions, swatches,
//! recent files, Camera Raw presets). Each is written whole or not at all,
//! and under a lock, merged with what another window saved since this one
//! read it: no window erases another's changes. A file that can't be read
//! is set aside, never overwritten.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Writes `bytes` to `path` (a settings file that only one window keeps:
/// the tools, the workspace), whole or not at all, its folder made first.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    crate::document::write_atomic(path, |out| Ok(std::io::Write::write_all(out, bytes)?))
        .map_err(|e| match e {
            crate::document::FileError::Io(e) => e,
            e => std::io::Error::other(e.to_string()),
        })
}

/// How a store's file holds its value.
#[derive(Clone, Copy)]
pub struct Format {
    pub decode: fn(&[u8]) -> Option<Value>,
    pub encode: fn(&Value) -> Vec<u8>,
}

/// JSON, indented.
pub const JSON: Format = Format {
    decode: |bytes| serde_json::from_slice(bytes).ok(),
    encode: |value| serde_json::to_vec_pretty(value).unwrap_or_default(),
};

/// What was found on opening a store.
#[derive(Debug)]
pub enum Found {
    /// No file yet.
    Missing,
    Value(Value),
    /// The file could not be read as such: renamed to this, the defaults
    /// in its place.
    SetAside(PathBuf),
    /// The file could not be read (locked, no permission): left as it is,
    /// and never written this session.
    Unreadable(std::io::Error),
}

/// A shared settings file and what this window last read or wrote of it.
pub struct Store {
    path: PathBuf,
    format: Format,
    /// What this window's value comes from: the base of the merge.
    base: Option<Value>,
    saving: bool,
    /// Whether a value found on disk can be merged with (one written by a
    /// newer Atelier can't: it is left as it is, and nothing is saved).
    accepts: fn(&Value) -> bool,
    /// Applied to the merged value before it is written (a length limit).
    tidy: fn(&mut Value),
}

impl Store {
    pub fn new(path: PathBuf, format: Format) -> Self {
        Self {
            path,
            format,
            base: None,
            saving: true,
            accepts: |_| true,
            tidy: |_| {},
        }
    }

    pub fn accepting(self, accepts: fn(&Value) -> bool) -> Self {
        Self { accepts, ..self }
    }

    pub fn tidying(self, tidy: fn(&mut Value)) -> Self {
        Self { tidy, ..self }
    }

    /// Whether this window may write the file.
    pub fn saving(&self) -> bool {
        self.saving
    }

    /// Never writes the file this session (one this version can't take).
    pub fn refuse_saving(&mut self) {
        self.saving = false;
    }

    /// Reads the file. A damaged one is set aside (its value then for the
    /// caller to make); one locked a moment is read again once.
    pub fn read(&mut self) -> Found {
        let mut read = std::fs::read(&self.path);
        if let Err(e) = &read
            && e.kind() != std::io::ErrorKind::NotFound
        {
            std::thread::sleep(std::time::Duration::from_millis(300));
            read = std::fs::read(&self.path);
        }
        match read {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Found::Missing,
            Err(e) => {
                self.saving = false;
                Found::Unreadable(e)
            }
            Ok(bytes) => match (self.format.decode)(&bytes) {
                Some(value) => {
                    self.base = Some(value.clone());
                    Found::Value(value)
                }
                None => self.set_aside(),
            },
        }
    }

    /// The file renamed "name.bak" (the caller found it unusable): what
    /// this window saves goes to a new one.
    pub fn set_aside(&mut self) -> Found {
        self.base = None;
        match set_aside(&self.path) {
            Some(backup) => Found::SetAside(backup),
            None => {
                self.saving = false;
                Found::Unreadable(std::io::Error::other("not set aside"))
            }
        }
    }

    /// Saves `mine`, merged under the lock with what the file holds now:
    /// the changes another window made since this one read it stay. Returns
    /// the value written, which [`Self::adopt`] makes this window's.
    pub fn save(&mut self, mine: Value) -> std::io::Result<Value> {
        if !self.saving {
            return Err(std::io::Error::other("this file is not saved"));
        }
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let name = self.path.file_name().unwrap_or_default().to_string_lossy();
        // Not truncated: another window may hold it.
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(format!(".{name}.lock")))?;
        // Where locks don't exist (some network drives), saving still works.
        let _ = lock.lock();
        let theirs = match std::fs::read(&self.path) {
            Ok(bytes) => match (self.format.decode)(&bytes) {
                Some(value) if (self.accepts)(&value) => Some(value),
                Some(_) => {
                    // Written meanwhile by a newer version: left to it.
                    self.saving = false;
                    return Err(std::io::Error::other("written by a newer version"));
                }
                None => {
                    set_aside(&self.path);
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let mut merged = match &theirs {
            Some(theirs) => merge(self.base.as_ref(), &mine, theirs),
            None => mine.clone(),
        };
        (self.tidy)(&mut merged);
        if theirs.as_ref() != Some(&merged) {
            write(&self.path, &(self.format.encode)(&merged))?;
        }
        // This window's next value comes from `mine`, unless it adopts.
        self.base = Some(mine);
        Ok(merged)
    }

    /// The value [`Self::save`] returned is now this window's.
    pub fn adopt(&mut self, value: &Value) {
        self.base = Some(value.clone());
    }
}

/// `path` renamed to "name.bak", "name.bak-2"…: its new path.
pub fn set_aside(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let backup = (1..1000)
        .map(|n| match n {
            1 => path.with_file_name(format!("{name}.bak")),
            n => path.with_file_name(format!("{name}.bak-{n}")),
        })
        .find(|p| !p.exists())?;
    std::fs::rename(path, &backup).ok()?;
    Some(backup)
}

/// Three-way merge: `mine` and `theirs` both made from `base`. What only
/// one side changed is taken from it; fields of objects merge one by one,
/// as do lists of names or values (actions, presets, colors, paths) item
/// by item; anything else both changed is this window's.
pub fn merge(base: Option<&Value>, mine: &Value, theirs: &Value) -> Value {
    if Some(mine) == base || mine == theirs {
        return theirs.clone();
    }
    if Some(theirs) == base {
        return mine.clone();
    }
    match (mine, theirs) {
        (Value::Object(m), Value::Object(t)) => {
            let b = base.and_then(Value::as_object);
            Value::Object(merge_fields(b, m, t))
        }
        (Value::Array(m), Value::Array(t)) if keyed(m) && keyed(t) => {
            let b = base
                .and_then(Value::as_array)
                .filter(|b| keyed(b))
                .map_or(&[][..], Vec::as_slice);
            Value::Array(merge_items(b, m, t))
        }
        _ => mine.clone(),
    }
}

fn merge_fields(
    base: Option<&Map<String, Value>>,
    mine: &Map<String, Value>,
    theirs: &Map<String, Value>,
) -> Map<String, Value> {
    let was = |key: &str| base.and_then(|b| b.get(key));
    let mut out = Map::new();
    for (key, m) in mine {
        match theirs.get(key) {
            Some(t) => {
                out.insert(key.clone(), merge(was(key), m, t));
            }
            // Removed by the other window, unless changed by this one.
            None if was(key) == Some(m) => {}
            None => {
                out.insert(key.clone(), m.clone());
            }
        }
    }
    for (key, t) in theirs {
        if !mine.contains_key(key) && was(key) != Some(t) {
            out.insert(key.clone(), t.clone());
        }
    }
    out
}

/// A list whose items can be told apart: values, or objects with a name.
fn keyed(items: &[Value]) -> bool {
    items.iter().all(|v| match v {
        Value::Object(o) => o.get("name").is_some_and(Value::is_string),
        Value::Array(_) => false,
        _ => true,
    })
}

/// An item's identity in a list: its name (or value), and how many items
/// before it share it.
fn keys(items: &[Value]) -> Vec<(String, usize)> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    items
        .iter()
        .map(|v| {
            let id = match v {
                Value::Object(o) => format!("name:{}", o["name"]),
                v => v.to_string(),
            };
            let n = seen.entry(id.clone()).or_default();
            *n += 1;
            (id, *n - 1)
        })
        .collect()
}

fn merge_items(base: &[Value], mine: &[Value], theirs: &[Value]) -> Vec<Value> {
    let (bk, mk, tk) = (keys(base), keys(mine), keys(theirs));
    let find = |keys: &[(String, usize)], k: &(String, usize)| keys.iter().position(|x| x == k);
    let mut out: Vec<((String, usize), Value)> = Vec::new();
    // This window's order, each item merged with the other's.
    for (k, m) in mk.iter().zip(mine) {
        let b = find(&bk, k).map(|i| &base[i]);
        match find(&tk, k) {
            Some(i) => out.push((k.clone(), merge(b, m, &theirs[i]))),
            // Removed by the other window, unless changed by this one.
            None if b == Some(m) => {}
            None => out.push((k.clone(), m.clone())),
        }
    }
    // Then what only the other window added (or changed and this one
    // removed), after the item it follows there (this window's own
    // additions stay first: the most recent).
    for (i, (k, t)) in tk.iter().zip(theirs).enumerate() {
        if find(&mk, k).is_some() || find(&bk, k).map(|j| &base[j]) == Some(t) {
            continue;
        }
        let place = |k: &(String, usize)| out.iter().position(|(o, _)| o == k);
        let at = match tk[..i].iter().rev().find_map(place) {
            Some(before) => before + 1,
            // First there: before what follows it there, or last.
            None => tk[i + 1..].iter().find_map(place).unwrap_or(out.len()),
        };
        out.insert(at, (k.clone(), t.clone()));
    }
    out.into_iter().map(|(_, v)| v).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(name: &str) -> PathBuf {
        crate::testing::test_dir(&format!("store-{name}"))
    }

    #[test]
    fn what_each_side_changed_is_kept() {
        let base = json!({"sets": [{"name": "A", "actions": [{"name": "a1"}]}], "v": 1});
        let mine = json!({"sets": [{"name": "A", "actions": [{"name": "a1"}, {"name": "mine"}]}], "v": 1});
        let theirs = json!({"sets": [
            {"name": "A", "actions": [{"name": "a1"}]},
            {"name": "B", "actions": []}
        ], "v": 1});
        assert_eq!(
            merge(Some(&base), &mine, &theirs),
            json!({"sets": [
                {"name": "A", "actions": [{"name": "a1"}, {"name": "mine"}]},
                {"name": "B", "actions": []}
            ], "v": 1})
        );
        // Removed on one side, untouched on the other: removed.
        let colors = json!(["#111111", "#222222", "#111111"]);
        let mine = json!(["#111111", "#222222"]);
        let theirs = json!(["#333333", "#111111", "#222222", "#111111"]);
        assert_eq!(
            merge(Some(&colors), &mine, &theirs),
            json!(["#333333", "#111111", "#222222"])
        );
        // Both changed what can't be merged item by item: this window's.
        let steps = json!([{"do": 1}]);
        let mine = json!([{"do": 2}]);
        let theirs = json!([{"do": 3}]);
        assert_eq!(merge(Some(&steps), &mine, &theirs), mine);
    }

    /// Two windows, each with what it read at start: neither erases the
    /// other's changes.
    #[test]
    fn two_windows_never_erase_each_other() {
        let dir = dir("two-windows");
        let path = dir.join("presets.json");
        std::fs::write(&path, json!([{"name": "Old"}]).to_string()).unwrap();
        let mut a = Store::new(path.clone(), JSON);
        let mut b = Store::new(path.clone(), JSON);
        let Found::Value(read_a) = a.read() else { panic!() };
        let Found::Value(read_b) = b.read() else { panic!() };
        assert_eq!(read_a, read_b);
        a.save(json!([{"name": "Old"}, {"name": "From A"}])).unwrap();
        // B never saw A's preset, and removes "Old".
        let written = b.save(json!([{"name": "From B"}])).unwrap();
        assert_eq!(written, json!([{"name": "From B"}, {"name": "From A"}]));
        // A goes on without adopting: B's preset stays, and A's removal of
        // its own preset is one.
        a.save(json!([{"name": "Old"}])).unwrap();
        let on_disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk, json!([{"name": "From B"}]));
    }

    #[test]
    fn damaged_or_newer_files_are_never_overwritten() {
        let dir = dir("damaged");
        let path = dir.join("swatches.json");
        std::fs::write(&path, b"{\"swatches\": [").unwrap();
        let mut store = Store::new(path.clone(), JSON);
        let Found::SetAside(backup) = store.read() else { panic!() };
        assert_eq!(std::fs::read(&backup).unwrap(), b"{\"swatches\": [");
        store.save(json!({"swatches": []})).unwrap();
        assert!(path.exists());
        // A newer version wrote it meanwhile: left as it is.
        let newer = json!({"version": 9});
        std::fs::write(&path, newer.to_string()).unwrap();
        let mut store = store.accepting(|v| v.get("version").is_none());
        assert!(store.save(json!({"swatches": []})).is_err());
        assert!(!store.saving());
        assert_eq!(std::fs::read(&path).unwrap(), newer.to_string().as_bytes());
    }
}
