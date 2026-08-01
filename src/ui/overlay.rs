//! The window that appears while you are talking.
//!
//! Dictation is modeless and mostly invisible: the user presses a key in
//! another application and starts speaking, and what they need back is a
//! reassurance that the microphone is live and an idea of when to stop. So
//! this is a small window with a level meter and a line of status, and it goes
//! away by itself.
//!
//! It cannot place itself. Wayland gives a client no say in where its windows
//! land, so this is an ordinary window that the compositor puts where it
//! likes, rather than the corner-pinned overlay the same app would be on X11.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;

use crate::model::Mode;

/// What the overlay is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Listening,
    Transcribing,
    Polishing,
}

impl Phase {
    fn title(self) -> &'static str {
        match self {
            Phase::Listening => "Listening",
            Phase::Transcribing => "Transcribing",
            Phase::Polishing => "Cleaning up",
        }
    }
}

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    pub struct Overlay {
        pub meter: RefCell<Option<gtk::DrawingArea>>,
        pub title: RefCell<Option<gtk::Label>>,
        pub detail: RefCell<Option<gtk::Label>>,
        pub spinner: RefCell<Option<adw::Spinner>>,
        /// Most recent level, and a decayed peak so the meter does not flicker
        /// between syllables.
        pub level: Cell<f64>,
        pub peak: Cell<f64>,
        pub phase: Cell<Phase>,
    }

    impl Default for Overlay {
        fn default() -> Self {
            Self {
                meter: RefCell::new(None),
                title: RefCell::new(None),
                detail: RefCell::new(None),
                spinner: RefCell::new(None),
                level: Cell::new(0.0),
                peak: Cell::new(0.0),
                phase: Cell::new(Phase::Listening),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Overlay {
        const NAME: &'static str = "ScribeOverlay";
        type Type = super::Overlay;
        type ParentType = adw::Window;
    }

    impl ObjectImpl for Overlay {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }
    }
    impl WidgetImpl for Overlay {}
    impl WindowImpl for Overlay {}
    impl AdwWindowImpl for Overlay {}
}

glib::wrapper! {
    pub struct Overlay(ObjectSubclass<imp::Overlay>)
        @extends adw::Window, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
                    gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Overlay {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("resizable", false)
            .property("deletable", false)
            .property("modal", false)
            .build()
    }

    fn build(&self) {
        let imp = self.imp();
        self.set_default_size(340, -1);
        self.add_css_class("scribe-overlay");
        self.set_title(Some("Scribe"));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .build();

        let spinner = adw::Spinner::new();
        spinner.set_size_request(20, 20);
        spinner.set_visible(false);
        header.append(&spinner);

        let labels = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        let title = gtk::Label::builder()
            .label(Phase::Listening.title())
            .xalign(0.0)
            .build();
        title.add_css_class("heading");
        let detail = gtk::Label::builder()
            .label("Press the shortcut again to stop")
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(3)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        detail.add_css_class("dimmed");
        detail.add_css_class("caption");
        labels.append(&title);
        labels.append(&detail);
        header.append(&labels);
        root.append(&header);

        let meter = gtk::DrawingArea::builder().content_height(28).build();
        meter.add_css_class("scribe-meter");
        meter.set_draw_func(glib::clone!(
            #[weak(rename_to = overlay)]
            self,
            move |_area, context, width, height| {
                overlay.draw_meter(context, width, height);
            }
        ));
        root.append(&meter);

        self.set_content(Some(&root));

        *imp.meter.borrow_mut() = Some(meter);
        *imp.title.borrow_mut() = Some(title);
        *imp.detail.borrow_mut() = Some(detail);
        *imp.spinner.borrow_mut() = Some(spinner);
    }

    /// A row of bars whose heights follow the recent level.
    ///
    /// Drawn rather than assembled from a `GtkLevelBar` because the point is
    /// motion — a bar that moves says "you are being heard" at a glance, and a
    /// discrete-block level bar reads as a value to be interpreted.
    fn draw_meter(&self, context: &gtk::cairo::Context, width: i32, height: i32) {
        let imp = self.imp();
        let level = imp.level.get();
        let peak = imp.peak.get();

        let colour = self.color();
        context.set_source_rgba(
            colour.red() as f64,
            colour.green() as f64,
            colour.blue() as f64,
            0.85,
        );

        const BARS: i32 = 24;
        let width = width as f64;
        let height = height as f64;
        let slot = width / BARS as f64;
        let bar_width = (slot * 0.45).max(2.0);

        for index in 0..BARS {
            // A fixed profile across the row so the middle is tallest, scaled
            // by the live level: the shape reads as a waveform rather than a
            // progress bar, which is what stops it looking like a measurement.
            let position = index as f64 / (BARS - 1) as f64;
            let shape = ((position * std::f64::consts::PI).sin()).powf(0.7);
            let reach = level * shape;
            let floor = 0.06;
            let magnitude = (reach.max(floor * shape.max(0.35))).clamp(0.0, 1.0);

            let bar_height = (magnitude * height).max(2.0);
            let x = index as f64 * slot + (slot - bar_width) / 2.0;
            let y = (height - bar_height) / 2.0;
            rounded_bar(context, x, y, bar_width, bar_height);
            let _ = context.fill();
        }

        // A faint line showing the recent peak, so a level that has just
        // dropped still shows what it was.
        if peak > 0.02 {
            context.set_source_rgba(
                colour.red() as f64,
                colour.green() as f64,
                colour.blue() as f64,
                0.25,
            );
            let bar_height = (peak * height).max(2.0);
            let y = (height - bar_height) / 2.0;
            rounded_bar(context, 0.0, y, width, 1.5_f64.min(bar_height));
            let _ = context.fill();
        }
    }

    /// Feed a new block of audio level in.
    pub fn set_level(&self, level: f64) {
        let imp = self.imp();
        // Rise immediately, fall slowly: speech is full of short gaps, and a
        // meter that tracks them exactly spends its time at zero.
        let previous = imp.level.get();
        let smoothed = if level > previous {
            level
        } else {
            previous * 0.72 + level * 0.28
        };
        imp.level.set(smoothed.clamp(0.0, 1.0));
        imp.peak.set(imp.peak.get().max(smoothed) * 0.99);

        if let Some(meter) = imp.meter.borrow().as_ref() {
            meter.queue_draw();
        }
    }

    pub fn set_phase(&self, phase: Phase) {
        let imp = self.imp();
        imp.phase.set(phase);
        if let Some(title) = imp.title.borrow().as_ref() {
            title.set_label(phase.title());
        }
        if let Some(spinner) = imp.spinner.borrow().as_ref() {
            spinner.set_visible(phase != Phase::Listening);
        }
        if let Some(meter) = imp.meter.borrow().as_ref() {
            meter.set_visible(phase == Phase::Listening);
        }
        if phase != Phase::Listening {
            imp.level.set(0.0);
        }
    }

    /// The line under the title: the live partial in streaming mode, or the
    /// hint about stopping in batch mode.
    pub fn set_detail(&self, text: &str) {
        if let Some(detail) = self.imp().detail.borrow().as_ref() {
            detail.set_label(text);
        }
    }

    /// Put the overlay back to how it looks at the start of an utterance.
    pub fn reset(&self, mode: Mode) {
        let imp = self.imp();
        imp.level.set(0.0);
        imp.peak.set(0.0);
        self.set_phase(Phase::Listening);
        self.set_detail(match mode {
            Mode::Batch => "Press the shortcut again to stop",
            Mode::Streaming => "Listening…",
        });
    }
}

fn rounded_bar(context: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64) {
    let radius = (width / 2.0).min(height / 2.0);
    context.new_sub_path();
    context.arc(
        x + width - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    context.arc(
        x + width - radius,
        y + height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    context.arc(
        x + radius,
        y + height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    context.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        1.5 * std::f64::consts::PI,
    );
    context.close_path();
}
