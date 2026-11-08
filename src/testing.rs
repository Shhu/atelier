//! Helpers shared by the tests of every module.

use std::path::PathBuf;
use std::sync::Once;

/// Prefix of the folders tests write in, one per test process.
const PREFIX: &str = "atelier-tests-";

/// This process's folder for test files: `atelier-tests-{pid}` in the
/// system temp folder, removed when the process exits. Folders left by
/// processes that are gone (killed, crashed) are removed on first use.
pub fn root() -> PathBuf {
    static START: Once = Once::new();
    let root = std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id()));
    START.call_once(|| {
        remove_stale();
        #[cfg(unix)]
        {
            extern "C" fn remove_root() {
                let _ = std::fs::remove_dir_all(
                    std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id())),
                );
            }
            // SAFETY: registers a plain function with no captured state.
            unsafe { libc::atexit(remove_root) };
        }
    });
    root
}

/// An empty folder for `name` (unique in the test process), in this
/// process's folder: concurrent test runs never share it.
pub fn test_dir(name: &str) -> PathBuf {
    let dir = root().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("test folder");
    dir
}

/// The turns of a wait loop: as many as fit in two minutes, whatever the
/// machine's load (a count of turns is too short when many tests share
/// the cores). The loop ends itself once what it waits for is there.
pub fn patience() -> impl Iterator<Item = ()> {
    let started = std::time::Instant::now();
    std::iter::from_fn(move || (started.elapsed().as_secs() < 120).then_some(()))
}

/// Removes the test folders of processes that are no longer running
/// (elsewhere than on Linux: those over a day old).
fn remove_stale() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(PREFIX)) else { continue };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if pid != std::process::id() && gone(pid, &entry) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(target_os = "linux")]
fn gone(pid: u32, _: &std::fs::DirEntry) -> bool {
    !std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn gone(_: u32, entry: &std::fs::DirEntry) -> bool {
    let age = entry.metadata().and_then(|m| m.modified()).ok();
    age.and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() > 24 * 3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_folders_are_per_process_and_empty() {
        let dir = test_dir("testing-empty");
        std::fs::write(dir.join("a"), b"a").unwrap();
        let again = test_dir("testing-empty");
        assert_eq!(dir, again);
        assert!(std::fs::read_dir(&again).unwrap().next().is_none());
        assert!(dir.starts_with(root()));
        assert!(root().ends_with(format!("{PREFIX}{}", std::process::id())));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn folders_of_finished_runs_are_removed() {
        // A pid past the kernel's maximum: never running.
        let stale = std::env::temp_dir().join(format!("{PREFIX}4294967290"));
        std::fs::create_dir_all(stale.join("x")).unwrap();
        remove_stale();
        assert!(!stale.exists());
    }
}
