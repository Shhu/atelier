//! The system's file dialogs, always made here: owned by Atelier's window
//! (in front of it, on its monitor, without a taskbar entry of their own),
//! and with filters that take an extension in any case on Linux too
//! (cameras write `IMG_0001.JPG`).

use std::cell::Cell;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};

thread_local! {
    /// Atelier's window, for the dialogs opened on the UI thread.
    static PARENT: Cell<Option<(RawWindowHandle, RawDisplayHandle)>> = const { Cell::new(None) };
}

/// Remembers Atelier's window, which lives as long as the app.
pub(super) fn set_parent(cc: &eframe::CreationContext<'_>) {
    if let (Ok(window), Ok(display)) = (cc.window_handle(), cc.display_handle()) {
        PARENT.set(Some((window.as_raw(), display.as_raw())));
    }
}

/// Atelier's window on Windows (its HWND).
pub(super) fn hwnd() -> Option<isize> {
    match PARENT.get()? {
        (RawWindowHandle::Win32(w), _) => Some(w.hwnd.get()),
        _ => None,
    }
}

struct Parent(RawWindowHandle, RawDisplayHandle);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the main window outlives every dialog (they are modal,
        // opened from its frames).
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

impl HasDisplayHandle for Parent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: as above.
        Ok(unsafe { DisplayHandle::borrow_raw(self.1) })
    }
}

/// A new file dialog over Atelier's window.
pub(super) fn new() -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new();
    match PARENT.get() {
        Some((window, display)) => dialog.set_parent(&Parent(window, display)),
        None => dialog,
    }
}

/// `add_filter` with the extensions in any case.
pub(super) trait Filter: Sized {
    fn filter(self, name: impl Into<String>, extensions: &[&str]) -> Self;
}

impl Filter for rfd::FileDialog {
    fn filter(self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.add_filter(name, &patterns(extensions))
    }
}

/// What the dialog is given for `extensions`. GTK (also behind the Flatpak's
/// portal) matches `*.jpg` in that case only: each letter becomes `[jJ]`.
/// Windows and macOS ignore the case themselves.
fn patterns(extensions: &[&str]) -> Vec<String> {
    extensions
        .iter()
        .map(|e| {
            if cfg!(target_os = "linux") {
                e.chars()
                    .map(|c| {
                        if c.is_ascii_alphabetic() {
                            format!("[{}{}]", c.to_ascii_lowercase(), c.to_ascii_uppercase())
                        } else {
                            c.to_string()
                        }
                    })
                    .collect()
            } else {
                (*e).to_owned()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_take_any_case() {
        let got = patterns(&["jpg", "cr3", "*"]);
        if cfg!(target_os = "linux") {
            assert_eq!(got, ["[jJ][pP][gG]", "[cC][rR]3", "*"]);
        } else {
            assert_eq!(got, ["jpg", "cr3", "*"]);
        }
    }

    /// Every dialog goes through [`new`] and [`Filter::filter`].
    #[test]
    fn no_dialog_is_made_elsewhere() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![dir];
        let mut offenders = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && !path.ends_with("file_dialog.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    if text.contains("rfd::FileDialog::new") || text.contains(".add_filter(") {
                        offenders.push(path);
                    }
                }
            }
        }
        assert!(offenders.is_empty(), "{offenders:?}");
    }
}
