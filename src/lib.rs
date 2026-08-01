//! Scribe — dictation for the GNOME desktop.
//!
//! The split is the same one the sibling apps use. `model` is the part that
//! could be reasoned about on paper: settings, the vocabulary rules, the
//! spelled-number rewriting, and the on-disk file that holds them. It pulls in
//! no GTK types and needs no display, so all of it is tested directly.
//!
//! `ui` is everything that touches the desktop: the windows, the microphone,
//! the speech model, the portals that put text into somebody else's window,
//! and the HTTP call to the local language model. Those are boundaries, and
//! they are driven from the GLib main loop rather than from threads of our own
//! wherever GLib offers a way to do it.

pub mod model;
pub mod ui;

pub const APP_ID: &str = "us.hagreli.Scribe";
