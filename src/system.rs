//! What the system asks of Atelier outside its window (files after the
//! first one on the command line; macOS: files opened from the Finder or
//! dropped on the Dock icon, Quit from the app menu, the Dock or a logout),
//! and a start that fails before any window shows.

use std::path::PathBuf;

use crate::i18n::tm;

#[cfg(target_os = "macos")]
pub mod macos;

#[derive(Default)]
struct Requests {
    open: Vec<PathBuf>,
    quit: bool,
    /// Woken when a request comes (unset before the app starts).
    ctx: Option<eframe::egui::Context>,
}

#[cfg(not(test))]
static REQUESTS: std::sync::Mutex<Requests> = std::sync::Mutex::new(Requests {
    open: Vec::new(),
    quit: false,
    ctx: None,
});

#[cfg(not(test))]
fn requests<R>(f: impl FnOnce(&mut Requests) -> R) -> R {
    f(&mut REQUESTS.lock().unwrap_or_else(|e| e.into_inner()))
}

// One per test thread: a test's Quit never reaches another's app.
#[cfg(test)]
thread_local! {
    static REQUESTS: std::cell::RefCell<Requests> = Default::default();
}

#[cfg(test)]
fn requests<R>(f: impl FnOnce(&mut Requests) -> R) -> R {
    REQUESTS.with_borrow_mut(f)
}

/// Requests come to the app through `ctx` from now on.
pub fn set_context(ctx: &eframe::egui::Context) {
    requests(|r| r.ctx = Some(ctx.clone()));
}

/// Files to open, as if picked in Open.
pub fn open(paths: impl IntoIterator<Item = PathBuf>) {
    requests(|r| {
        r.open.extend(paths);
        if let Some(ctx) = &r.ctx {
            ctx.request_repaint();
        }
    });
}

/// Quit, asked the way the title bar's close button asks (unsaved
/// documents first).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn quit() {
    requests(|r| {
        r.quit = true;
        if let Some(ctx) = &r.ctx {
            ctx.request_repaint();
        }
    });
}

/// The files to open since the last call.
pub fn take_open() -> Vec<PathBuf> {
    requests(|r| std::mem::take(&mut r.open))
}

/// Whether quitting was asked since the last call.
pub fn take_quit() -> bool {
    requests(|r| std::mem::take(&mut r.quit))
}

/// The window couldn't be made (most often the graphics card or its driver):
/// written to the log, and on Windows, where a double-clicked app has no
/// console, said in a message box.
pub fn startup_failed(error: &eframe::Error) {
    let log = crate::diag::log_file();
    let message = tm!(
        "platforms14-startup-failed",
        error = error.to_string(),
        log = log.display().to_string()
    );
    crate::diag::error(message.clone());
    eprintln!("{}", message.shown);
    #[cfg(windows)]
    message_box(&crate::i18n::t!("platforms14-startup-failed-title"), &message.shown);
}

#[cfg(windows)]
fn message_box(title: &str, text: &str) {
    const MB_ICONERROR: u32 = 0x10;
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide(title));
    // SAFETY: zero-terminated strings that outlive the call, no owner.
    unsafe {
        MessageBoxW(0, text.as_ptr(), title.as_ptr(), MB_ICONERROR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_taken_once() {
        open([PathBuf::from("/a.png"), PathBuf::from("/b.psd")]);
        quit();
        assert_eq!(take_open(), [PathBuf::from("/a.png"), PathBuf::from("/b.psd")]);
        assert!(take_quit());
        assert!(take_open().is_empty());
        assert!(!take_quit());
    }
}
