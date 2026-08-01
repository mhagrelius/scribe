//! The companion GNOME Shell extension.
//!
//! Typing into somebody else's window needs code running inside the
//! compositor, and there are only two ways to get there: ask the
//! RemoteDesktop portal, whose consent dialog talks about remote interaction
//! and whose session GNOME closes on its own schedule, or put the code in the
//! compositor yourself as an extension. The extension needs no permission at
//! all, because the shell already has one.
//!
//! This is the client half. The contract is in `extension/interface.js` and
//! the two are versioned together.

use gio::prelude::*;
use gtk::glib;

pub const BUS_NAME: &str = "us.hagreli.Scribe.Shell";
pub const OBJECT_PATH: &str = "/us/hagreli/Scribe/Shell";
const INTERFACE: &str = "us.hagreli.Scribe.Shell";

/// Bumped together with `PROTOCOL_VERSION` in `extension/interface.js`.
pub const PROTOCOL_VERSION: u32 = 1;

/// The extension's uuid, for telling the user what to enable.
pub const UUID: &str = "scribe@hagreli.us";

#[derive(Debug)]
pub enum ShellError {
    /// Nothing owns the extension's bus name.
    NotRunning,
    /// The extension is older or newer than this build of Scribe.
    Mismatched {
        found: u32,
    },
    Call(glib::Error),
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::NotRunning => write!(
                f,
                "The Scribe shell extension is not running. Enable it with \
                 “gnome-extensions enable {UUID}”."
            ),
            ShellError::Mismatched { found } => write!(
                f,
                "The Scribe shell extension speaks version {found}, but this \
                 Scribe speaks version {PROTOCOL_VERSION}. Reinstall so the two match."
            ),
            ShellError::Call(error) => {
                write!(f, "The shell extension refused to insert the text: {error}")
            }
        }
    }
}

impl std::error::Error for ShellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ShellError::Call(error) => Some(error),
            _ => None,
        }
    }
}

/// Whether the extension is loaded and speaks our version.
///
/// Checked per dictation rather than cached: an extension can be enabled or
/// disabled at any moment, and a stale "no" would send every transcript to the
/// portal for the rest of the session.
pub fn is_available(connection: &gio::DBusConnection) -> bool {
    matches!(version(connection), Ok(found) if found == PROTOCOL_VERSION)
}

fn version(connection: &gio::DBusConnection) -> Result<u32, ShellError> {
    let reply = connection
        .call_sync(
            Some(BUS_NAME),
            OBJECT_PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            Some(&(INTERFACE, "ProtocolVersion").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            // Short: the extension either answers at once or is not there, and
            // a dictation must not hang waiting to find out.
            2_000,
            gio::Cancellable::NONE,
        )
        .map_err(|_| ShellError::NotRunning)?;

    reply
        .child_value(0)
        .as_variant()
        .and_then(|inner| inner.get::<u32>())
        .ok_or(ShellError::NotRunning)
}

/// Type `text` into the focused window through the extension.
pub fn insert(connection: &gio::DBusConnection, text: &str) -> Result<(), ShellError> {
    match version(connection) {
        Ok(found) if found == PROTOCOL_VERSION => {}
        Ok(found) => return Err(ShellError::Mismatched { found }),
        Err(error) => return Err(error),
    }

    connection
        .call_sync(
            Some(BUS_NAME),
            OBJECT_PATH,
            INTERFACE,
            "Insert",
            Some(&(text,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5_000,
            gio::Cancellable::NONE,
        )
        .map(|_| ())
        .map_err(ShellError::Call)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_names_match_the_ones_the_extension_exports() {
        // extension/interface.js declares these; extension/test.js asserts the
        // same three. If one side is edited alone, one of the two fails.
        assert_eq!(BUS_NAME, "us.hagreli.Scribe.Shell");
        assert_eq!(OBJECT_PATH, "/us/hagreli/Scribe/Shell");
        assert_eq!(UUID, "scribe@hagreli.us");
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn the_insert_argument_is_a_plain_string_tuple() {
        // `Insert` takes `(s)`. Getting this wrong is the GVariant trap that
        // portal.rs exists to avoid, and it fails the same silent way.
        let args = ("hello",).to_variant();
        assert_eq!(args.type_().as_str(), "(s)");
    }

    #[test]
    fn a_missing_extension_reads_as_not_running_rather_than_an_error() {
        let error = ShellError::NotRunning.to_string();
        assert!(
            error.contains(UUID),
            "the message should say what to enable"
        );
    }
}
