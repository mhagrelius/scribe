//! The global shortcut.
//!
//! The obvious answer here is the GlobalShortcuts portal, which hands out both
//! press and release and would give push-to-talk for free. It does not work.
//! Since xdg-desktop-portal 1.21 the interface refuses any caller without an
//! application identity, and the mechanism for a non-Flatpak app to declare
//! one — `org.freedesktop.host.portal.Registry` — is not exported by the
//! portal running on this desktop. A systemd scope named after the app does
//! not stand in for it: the call still comes back `NotAllowed: An app id is
//! required`. That was measured, not assumed.
//!
//! So Mynah registers a custom keybinding with gnome-settings-daemon, the same
//! way the Settings app's own Keyboard panel does, and that binding runs
//! `mynah --toggle`. It needs no consent dialog, no portal and no privileges,
//! and it survives reboots because it is stored in dconf rather than held by
//! this process.
//!
//! What it cannot do is tell a press from a release, because gsd spawns a
//! command on activation and there is no second event. Dictation is therefore
//! a toggle: press to start, press again to stop. Push-to-talk would need to
//! read the keyboard device directly, which means putting the user in the
//! `input` group and giving every process they run a keylogger.

use gio::prelude::*;

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_KEYBINDING: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/mynah/";
const NAME: &str = "Mynah dictation";

/// Why a shortcut could not be registered.
#[derive(Debug)]
pub enum ShortcutError {
    /// gnome-settings-daemon's schema is not installed, so this is not GNOME.
    Unsupported,
    /// The accelerator is not one GTK can parse.
    Unparsable(String),
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShortcutError::Unsupported => write!(
                f,
                "This desktop does not provide GNOME's custom keyboard shortcuts, \
                 so Mynah cannot register one. Bind a shortcut to “mynah --toggle” \
                 in your desktop's own keyboard settings instead."
            ),
            ShortcutError::Unparsable(accel) => {
                write!(f, "“{accel}” is not a keyboard shortcut Mynah understands.")
            }
        }
    }
}

impl std::error::Error for ShortcutError {}

/// Whether this desktop has the settings we would be writing into.
pub fn is_supported() -> bool {
    gio::SettingsSchemaSource::default()
        .and_then(|source| source.lookup(MEDIA_KEYS, true))
        .is_some()
        && gio::SettingsSchemaSource::default()
            .and_then(|source| source.lookup(CUSTOM_KEYBINDING, true))
            .is_some()
}

/// The command the shortcut runs.
///
/// An absolute path rather than a bare `mynah`: gnome-settings-daemon spawns
/// with its own environment, and a binary installed under `~/.local/bin` is
/// not reliably on the `PATH` it inherits.
fn command() -> String {
    let binary = std::env::current_exe()
        .ok()
        .filter(|path| path.is_absolute())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "mynah".to_string());
    format!("{binary} --toggle")
}

/// Check an accelerator before it is written anywhere.
pub fn is_parsable(accel: &str) -> bool {
    let Some(parsed) = gtk::accelerator_parse(accel) else {
        return false;
    };
    // `accelerator_parse` does not fail outright on nonsense: it hands back
    // the void symbol, or for an empty string a keyval of zero. Neither has a
    // key name, which is the one check that catches both.
    parsed.0 != gtk::gdk::Key::VoidSymbol && parsed.0.name().is_some()
}

/// Install or update the binding, so pressing `accel` toggles dictation.
pub fn install(accel: &str) -> Result<(), ShortcutError> {
    if !is_supported() {
        return Err(ShortcutError::Unsupported);
    }
    if !is_parsable(accel) {
        return Err(ShortcutError::Unparsable(accel.to_string()));
    }

    let binding = gio::Settings::with_path(CUSTOM_KEYBINDING, PATH);
    binding.set_string("name", NAME).ok();
    binding.set_string("binding", accel).ok();
    binding.set_string("command", &command()).ok();

    let media_keys = gio::Settings::new(MEDIA_KEYS);
    let mut paths: Vec<String> = media_keys
        .strv("custom-keybindings")
        .iter()
        .map(|s| s.to_string())
        .collect();
    if !paths.iter().any(|p| p == PATH) {
        paths.push(PATH.to_string());
        let borrowed: Vec<&str> = paths.iter().map(String::as_str).collect();
        media_keys.set_strv("custom-keybindings", borrowed).ok();
    }
    gio::Settings::sync();
    Ok(())
}

/// Take the binding back out, leaving the user's other shortcuts alone.
pub fn remove() {
    if !is_supported() {
        return;
    }
    let media_keys = gio::Settings::new(MEDIA_KEYS);
    let paths: Vec<String> = media_keys
        .strv("custom-keybindings")
        .iter()
        .map(|s| s.to_string())
        .filter(|p| p != PATH)
        .collect();
    let borrowed: Vec<&str> = paths.iter().map(String::as_str).collect();
    media_keys.set_strv("custom-keybindings", borrowed).ok();

    let binding = gio::Settings::with_path(CUSTOM_KEYBINDING, PATH);
    for key in ["name", "binding", "command"] {
        binding.reset(key);
    }
    gio::Settings::sync();
}

/// What is registered right now, if anything.
pub fn installed() -> Option<String> {
    if !is_supported() {
        return None;
    }
    let media_keys = gio::Settings::new(MEDIA_KEYS);
    let registered = media_keys
        .strv("custom-keybindings")
        .iter()
        .any(|p| p.as_str() == PATH);
    if !registered {
        return None;
    }
    let binding = gio::Settings::with_path(CUSTOM_KEYBINDING, PATH);
    let accel = binding.string("binding").to_string();
    (!accel.is_empty()).then_some(accel)
}

/// The accelerator as a person would read it: "Ctrl+Alt+D".
pub fn human_label(accel: &str) -> String {
    let Some((key, modifiers)) = gtk::accelerator_parse(accel) else {
        return accel.to_string();
    };
    if key == gtk::gdk::Key::VoidSymbol || key.name().is_none() {
        return accel.to_string();
    }
    let label = gtk::accelerator_get_label(key, modifiers);
    if label.is_empty() {
        accel.to_string()
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These need a display for `gtk::accelerator_parse`, so they live with the
    // widget tests rather than here. What can be checked without one is the
    // command string and the settings path, which is the part that decides
    // whether the binding lands where GNOME looks for it.

    #[test]
    fn the_command_carries_the_toggle_flag() {
        let command = command();
        assert!(command.ends_with(" --toggle"), "got {command}");
    }

    #[test]
    fn the_binding_lives_under_a_path_of_our_own() {
        // Reusing GNOME's "custom0" would collide with whatever the user has
        // already bound there.
        assert!(PATH.ends_with("/mynah/"));
        assert!(PATH.starts_with("/org/gnome/settings-daemon/"));
    }
}
