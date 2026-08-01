//! Everything that touches the desktop.
//!
//! Widget trees are built in Rust — no `.ui` XML, no Blueprint, no GResource.
//! The structure of a window is then readable in the same file as the
//! behaviour that drives it, which for an app this size is worth more than a
//! designer could give back.

pub mod application;
pub mod cleanup;
pub mod engine;
pub mod inject;
pub mod models;
pub mod overlay;
pub mod portal;
pub mod recorder;
pub mod shortcut;
pub mod tray;
pub mod window;

pub use application::ScribeApplication;

pub const STYLE: &str = include_str!("style.css");

pub fn load_stylesheet(display: &gtk::gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLE);
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
