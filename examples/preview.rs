//! Render the recording window to a PNG.
//!
//! Screenshotting a live GNOME Wayland session needs interactive consent,
//! which makes "does this look right?" hard to answer while iterating. This
//! builds the real overlay and paints it offscreen instead, at a few levels
//! and in both phases, so the meter can be looked at in one command.
//!
//! ```sh
//! cargo run --example preview -- /tmp/preview
//! cargo run --example preview -- /tmp/preview dark
//! ```

use adw::prelude::*;
use gtk::glib;
use scribe::model::Mode;
use scribe::ui::overlay::{Overlay, Phase};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "/tmp/preview".to_string());
    let dark = args.next().is_some_and(|scheme| scheme == "dark");

    gtk::init().expect("a display — run under xvfb-run if there is none");
    adw::init().expect("libadwaita");

    // An animating widget is a widget that is not finished being laid out.
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_enable_animations(false);
    }
    adw::StyleManager::default().set_color_scheme(if dark {
        adw::ColorScheme::ForceDark
    } else {
        adw::ColorScheme::ForceLight
    });
    if let Some(display) = gtk::gdk::Display::default() {
        scribe::ui::load_stylesheet(&display);
    }
    std::fs::create_dir_all(&out).expect("output directory");

    // Silence, mid-speech and a shout, so the meter can be compared against
    // its own centre line at three heights.
    for (name, level) in [("quiet", 0.0), ("speech", 0.45), ("loud", 1.0)] {
        let overlay = Overlay::new();
        overlay.reset(Mode::Batch);
        overlay.set_level(level);
        overlay.set_detail("And so, my fellow Americans, ask not what your country");
        show(&overlay);
        snapshot(&overlay, &format!("{out}/overlay-{name}.png"));
    }

    let working = Overlay::new();
    working.reset(Mode::Batch);
    working.set_phase(Phase::Transcribing);
    show(&working);
    snapshot(&working, &format!("{out}/overlay-transcribing.png"));

    println!("wrote previews to {out}");
}

/// Realise the window and let the layout settle before painting it.
fn show(window: &impl IsA<gtk::Window>) {
    window.as_ref().present();
    let context = glib::MainContext::default();
    for _ in 0..200 {
        if !context.iteration(false) {
            break;
        }
    }
}

/// Paint the widget at the size it actually asks for.
///
/// `WidgetPaintable` scales its content to whatever size it is snapshotted at,
/// so passing a guessed height silently stretches the result and makes the
/// picture useless for judging whether something is a pixel out.
fn snapshot(window: &impl IsA<gtk::Widget>, path: &str) {
    let widget = window.as_ref();
    let (_, width) = widget.preferred_size();
    let width = (width.width().max(1), width.height().max(1));
    let (width, height) = width;
    let paintable = gtk::WidgetPaintable::new(Some(widget));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);

    let Some(node) = snapshot.to_node() else {
        eprintln!("{path}: nothing was drawn");
        return;
    };
    let renderer = gtk::gsk::CairoRenderer::new();
    renderer
        .realize(gtk::gdk::Surface::NONE)
        .expect("a renderer");
    let texture = renderer.render_texture(&node, None);
    texture.save_to_png(path).expect("write the png");
    renderer.unrealize();
}
