//! Where Atelier keeps its settings (language, recent files).

use std::path::PathBuf;

/// `%APPDATA%\Atelier` on Windows, `~/.config/atelier` elsewhere.
pub fn dir() -> PathBuf {
    // Tests never touch the user's settings.
    #[cfg(test)]
    {
        return crate::testing::root().join("config");
    }
    #[allow(unreachable_code)]
    if let Some(roaming) = std::env::var_os("APPDATA") {
        PathBuf::from(roaming).join("Atelier")
    } else if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(config).join("atelier")
    } else {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home).join(".config").join("atelier")
    }
}
