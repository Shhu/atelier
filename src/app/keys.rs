//! Key and shortcut labels, written the same way everywhere (menus,
//! tooltips, palette, status bar) and in the UI's language: like Photoshop,
//! "Shift+Ctrl+S" ("Maj+Ctrl+S" in French) on Windows and Linux, "⇧⌘S" on
//! macOS.

use eframe::egui::{Context, Key, KeyboardShortcut, Modifiers};

use crate::i18n::t;

/// A shortcut: "Ctrl+O", "Alt+Suppr", "Ctrl+−".
pub(super) fn shortcut(ctx: &Context, shortcut: &KeyboardShortcut) -> String {
    combo(ctx, shortcut.modifiers, &key(shortcut.logical_key))
}

/// `what` (a key, a click…) with `modifiers` held: "Maj+Clic", "Alt+[ ]".
pub(super) fn combo(ctx: &Context, modifiers: Modifiers, what: &str) -> String {
    let mut names = modifier_names(ctx, modifiers);
    if names.is_empty() {
        return what.to_owned();
    }
    if !is_mac(ctx) {
        names.push('+');
    }
    names + what
}

/// A click with `modifiers` held: "Alt+Click", "⌥Clic".
pub(super) fn click(ctx: &Context, modifiers: Modifiers) -> String {
    combo(ctx, modifiers, &t!("hint-key-click"))
}

/// Modifier keys alone: "Ctrl", "Maj+Alt".
pub(super) fn modifiers(ctx: &Context, modifiers: Modifiers) -> String {
    modifier_names(ctx, modifiers)
}

/// Name of a key: translated ("Suppr", "Entrée"), else its symbol ("−",
/// "]") or egui's name ("F5").
pub(super) fn key(key: Key) -> String {
    match key {
        Key::Delete => t!("key-delete"),
        Key::Backspace => t!("hint-key-backspace"),
        Key::Enter => t!("hint-key-enter"),
        Key::Escape => t!("hint-key-esc"),
        Key::Space => t!("hint-key-space"),
        // egui has no symbol for it, only the name "Quote".
        Key::Quote => "'".to_owned(),
        _ => {
            let symbol = key.symbol_or_name();
            if symbol.chars().count() == 1 {
                symbol.to_owned()
            } else {
                key.name().to_owned()
            }
        }
    }
}

/// Where an AZERTY keyboard has `shortcut`, if its key is elsewhere: the
/// "'" of Ctrl+' is ù (the 4 key is the green channel), "[ ]" are ^ $, and
/// "/" is typed with Shift on the ":" key.
pub(super) fn azerty(ctx: &Context, shortcut: &KeyboardShortcut) -> Option<String> {
    let m = shortcut.modifiers;
    match shortcut.logical_key {
        Key::Quote => Some(combo(ctx, m, "ù")),
        Key::OpenBracket => Some(combo(ctx, m, "^")),
        Key::CloseBracket => Some(combo(ctx, m, "$")),
        Key::Slash => Some(combo(ctx, m.plus(Modifiers::SHIFT), ":")),
        _ => None,
    }
}

fn is_mac(ctx: &Context) -> bool {
    ctx.os().is_mac() && mac_symbols(ctx)
}

/// macOS writes modifiers as symbols, when the fonts have them.
fn mac_symbols(ctx: &Context) -> bool {
    let probe = KeyboardShortcut::new(Modifiers::MAC_CMD, Key::A);
    ctx.format_shortcut(&probe) == "⌘A"
}

/// Photoshop's order: Alt, Shift, Ctrl on Windows; ⌃⌥⇧⌘ on macOS.
fn modifier_names(ctx: &Context, m: Modifiers) -> String {
    if is_mac(ctx) {
        let mut s = String::new();
        for (on, symbol) in [
            (m.ctrl, "⌃"),
            (m.alt, "⌥"),
            (m.shift, "⇧"),
            (m.mac_cmd || m.command, "⌘"),
        ] {
            if on {
                s.push_str(symbol);
            }
        }
        return s;
    }
    let mac = ctx.os().is_mac();
    let (alt, command) = if mac { ("Option", "Cmd") } else { ("Alt", "Ctrl") };
    // On macOS, Control (`Modifiers::CTRL`) and Command are two keys.
    let (control, command_on) = if mac {
        (m.ctrl, m.mac_cmd || m.command)
    } else {
        (false, m.ctrl || m.command || m.mac_cmd)
    };
    [
        (m.alt, alt.to_owned()),
        (m.shift, t!("hint-key-shift")),
        (control, "Ctrl".to_owned()),
        (command_on, command.to_owned()),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect::<Vec<_>>()
    .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{self, Lang};
    use eframe::egui::os::OperatingSystem;

    fn ctx() -> Context {
        let ctx = Context::default();
        ctx.set_os(OperatingSystem::Windows);
        ctx
    }

    #[test]
    fn shortcuts_follow_the_language_and_photoshop_order() {
        let ctx = ctx();
        let save_as = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
        let fill = KeyboardShortcut::new(Modifiers::ALT, Key::Delete);
        let zoom_out = KeyboardShortcut::new(Modifiers::COMMAND, Key::Minus);
        let raise = KeyboardShortcut::new(Modifiers::COMMAND, Key::CloseBracket);
        i18n::set_lang(Lang::Fr);
        assert_eq!(shortcut(&ctx, &save_as), "Maj+Ctrl+S");
        assert_eq!(shortcut(&ctx, &fill), "Alt+Suppr");
        assert_eq!(combo(&ctx, Modifiers::SHIFT, "Clic"), "Maj+Clic");
        i18n::set_lang(Lang::En);
        assert_eq!(shortcut(&ctx, &save_as), "Shift+Ctrl+S");
        assert_eq!(shortcut(&ctx, &zoom_out), "Ctrl+−");
        assert_eq!(shortcut(&ctx, &raise), "Ctrl+]");
        assert_eq!(modifiers(&ctx, Modifiers::COMMAND), "Ctrl");
        // Grid: egui only names the key "Quote".
        let grid = KeyboardShortcut::new(Modifiers::COMMAND, Key::Quote);
        assert_eq!(shortcut(&ctx, &grid), "Ctrl+'");
        // Control is Ctrl on every system, Command only on macOS.
        assert_eq!(modifiers(&ctx, Modifiers::CTRL), "Ctrl");
        ctx.set_os(OperatingSystem::Mac);
        // The fonts (for the symbols) come with a first frame.
        ctx.run_ui(Default::default(), |_| {}).textures_delta.clear();
        if mac_symbols(&ctx) {
            assert_eq!(modifiers(&ctx, Modifiers::CTRL), "⌃");
            assert_eq!(modifiers(&ctx, Modifiers::COMMAND), "⌘");
        } else {
            assert_eq!(modifiers(&ctx, Modifiers::CTRL), "Ctrl");
            assert_eq!(modifiers(&ctx, Modifiers::COMMAND), "Cmd");
        }
    }
}
