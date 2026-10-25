//! AI features, run locally with ONNX Runtime: the models and their
//! download, Select Subject (BiRefNet), Object Selection (SAM 2.1),
//! Content-Aware Fill (LaMa).

pub mod inpaint;
pub mod models;
pub mod objects;
pub mod runtime;
pub mod subject;

use std::path::Path;
use std::time::Duration;

use crate::i18n::t;

/// An AI feature's error, in the interface's language and in English (for
/// the log and the report): the errors are made on the worker threads.
pub type Error = crate::i18n::Message;

/// Runs `f`, a panic becoming an error: a background job always reports
/// its end, and Select Subject can still fall back on the non-AI method.
pub fn catch_panic<R>(f: impl FnOnce() -> Result<R, Error>) -> Result<R, Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned());
        Err(message.map_or_else(|| Error::of(|| t!("ai-error-internal")), Error::from))
    })
}

/// `std::fs::rename`, tried again for a moment: on Windows, an antivirus
/// often holds a file that was just written.
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if tries < 8 && e.kind() != std::io::ErrorKind::NotFound => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50 * tries));
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    /// A panic in a background job becomes an error with its message.
    #[test]
    fn panics_become_errors() {
        let result: Result<(), super::Error> = super::catch_panic(|| panic!("model {} broken", 2));
        assert_eq!(
            result.err().map(|e| e.shown),
            Some("model 2 broken".to_owned())
        );
        assert_eq!(super::catch_panic(|| Ok(4)).ok(), Some(4));
    }
}
