//! The application.
//!
//! Everything that changes state passes through here. The window and the
//! overlay emit intent; this reads the config, drives the recorder, the speech
//! engine, the cleanup pass and the typist, and pushes results back out. It is
//! the only thing that writes the config file.
//!
//! It is also the thing that stays running. Mynah is a shortcut first and a
//! window second: `--toggle` on a second invocation is handed to the instance
//! already running, which is what makes a global keybinding work at all.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::model::{self, Config, Delivery, LoadOutcome, Mode, Store};
use crate::ui::{engine, inject, models, overlay, recorder, shortcut, window};

/// Whether a key is one of the ones held down alongside another.
///
/// Every keypress in the capture dialog arrives here, including the Ctrl that
/// begins "Ctrl+Alt+D". Treating one of those as the shortcut itself would
/// bind the modifier alone.
fn is_modifier_key(key: gtk::gdk::Key) -> bool {
    use gtk::gdk::Key;
    matches!(
        key,
        Key::Shift_L
            | Key::Shift_R
            | Key::Control_L
            | Key::Control_R
            | Key::Alt_L
            | Key::Alt_R
            | Key::Super_L
            | Key::Super_R
            | Key::Meta_L
            | Key::Meta_R
            | Key::Hyper_L
            | Key::Hyper_R
            | Key::Caps_Lock
            | Key::Shift_Lock
            | Key::Num_Lock
            | Key::ISO_Level3_Shift
            | Key::ISO_Level5_Shift
    )
}

/// What Mynah is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Listening,
    Working,
}

mod imp {
    use super::*;

    pub struct MynahApplication {
        pub store: RefCell<Store>,
        pub engine: RefCell<Option<Rc<engine::Engine>>>,
        pub typist: RefCell<Option<Rc<inject::Typist>>>,
        pub recorder: RefCell<Option<recorder::Recorder>>,
        pub overlay: RefCell<Option<overlay::Overlay>>,
        pub activity: Cell<Activity>,
        pub download: RefCell<Option<models::Download>>,
        /// Keeps the process alive with no window open. Dropping this guard
        /// is what would let Mynah exit the moment the settings window is
        /// closed, taking the shortcut with it.
        pub hold: RefCell<Option<gio::ApplicationHoldGuard>>,
        /// Streaming text accumulated across chunks.
        pub partial: RefCell<String>,
        /// Samples not yet handed to the streaming model.
        pub pending_audio: RefCell<Vec<f32>>,
    }

    impl Default for MynahApplication {
        fn default() -> Self {
            Self {
                store: RefCell::new(Store::detached()),
                engine: RefCell::new(None),
                typist: RefCell::new(None),
                recorder: RefCell::new(None),
                overlay: RefCell::new(None),
                activity: Cell::new(Activity::Idle),
                download: RefCell::new(None),
                hold: RefCell::new(None),
                partial: RefCell::new(String::new()),
                pending_audio: RefCell::new(Vec::new()),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MynahApplication {
        const NAME: &'static str = "MynahApplication";
        type Type = super::MynahApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for MynahApplication {}

    impl ApplicationImpl for MynahApplication {
        fn startup(&self) {
            self.parent_startup();
            let obj = self.obj();

            if let Some(display) = gtk::gdk::Display::default() {
                crate::ui::load_stylesheet(&display);
            }
            obj.load_store();
            obj.install_actions();

            *self.engine.borrow_mut() = Some(Rc::new(engine::Engine::new()));
            *self.typist.borrow_mut() = Some(Rc::new(inject::Typist::new()));

            // Mynah is useful with no window open, so the process is held up
            // by this rather than by a window.
            *self.hold.borrow_mut() = Some(obj.hold());
        }

        fn activate(&self) {
            self.obj().present_window();
        }

        fn command_line(&self, command_line: &gio::ApplicationCommandLine) -> glib::ExitCode {
            let obj = self.obj();
            let options = command_line.options_dict();

            if options.contains("toggle") {
                obj.toggle();
                return glib::ExitCode::SUCCESS;
            }
            if options.contains("stop") {
                if obj.imp().activity.get() == Activity::Listening {
                    obj.toggle();
                }
                return glib::ExitCode::SUCCESS;
            }
            obj.present_window();
            glib::ExitCode::SUCCESS
        }

        fn shutdown(&self) {
            // A recording in flight is dropped rather than transcribed: the
            // user is quitting, not dictating.
            let _ = self.recorder.borrow_mut().take();
            if let Err(error) = self.store.borrow().save() {
                eprintln!("mynah: {error}");
            }
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for MynahApplication {}
    impl AdwApplicationImpl for MynahApplication {}
}

glib::wrapper! {
    pub struct MynahApplication(ObjectSubclass<imp::MynahApplication>)
        @extends adw::Application, gtk::Application, gio::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for MynahApplication {
    fn default() -> Self {
        Self::new()
    }
}

impl MynahApplication {
    pub fn new() -> Self {
        Self::with_application_id(crate::APP_ID)
    }

    /// Tests use an id of their own: sharing the real one would register the
    /// test process as a remote for a running Mynah and hand it the commands.
    pub fn with_application_id(application_id: &str) -> Self {
        let app: Self = glib::Object::builder()
            .property("application-id", application_id)
            .property("flags", gio::ApplicationFlags::HANDLES_COMMAND_LINE)
            .build();

        app.add_main_option(
            "toggle",
            glib::Char::from(b't'),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            "Start dictating, or stop if already dictating",
            None,
        );
        app.add_main_option(
            "stop",
            glib::Char::from(0),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            "Stop dictating, if dictating",
            None,
        );
        app
    }

    fn config(&self) -> Config {
        self.imp().store.borrow().config().clone()
    }

    fn load_store(&self) {
        let (store, outcome) = Store::open(Store::default_path());
        let accel = store.config().shortcut.clone();
        *self.imp().store.borrow_mut() = store;

        if let LoadOutcome::Recovered(aside) = &outcome {
            eprintln!(
                "mynah: the settings file could not be read and was kept at {}",
                aside.display()
            );
        }

        // Re-assert the keybinding on every start: the command embeds this
        // binary's path, which changes when Mynah is installed somewhere new.
        if shortcut::is_supported() {
            if let Err(error) = shortcut::install(&accel) {
                eprintln!("mynah: {error}");
            }
        }
    }

    fn save(&self) {
        if let Err(error) = self.imp().store.borrow().save() {
            eprintln!("mynah: {error}");
        }
    }

    fn install_actions(&self) {
        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| app.quit())
            .build();
        let toggle = gio::ActionEntry::builder("toggle")
            .activate(|app: &Self, _, _| app.toggle())
            .build();
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about())
            .build();
        self.add_action_entries([quit, toggle, about]);
        self.set_accels_for_action("app.quit", &["<Control>q"]);
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::builder()
            .application_name("Mynah")
            .application_icon(crate::APP_ID)
            .developer_name("Matthew Hagrelius")
            .version(env!("CARGO_PKG_VERSION"))
            .website("https://github.com/mhagrelius/mynah")
            .issue_url("https://github.com/mhagrelius/mynah/issues")
            .license_type(gtk::License::Gpl30)
            .comments(
                "Speak, and it types. Speech recognition runs on this machine; \
                 nothing you dictate leaves it.",
            )
            .build();
        about.present(self.active_window().as_ref());
    }

    fn present_window(&self) {
        let window = self
            .active_window()
            .and_downcast::<window::Window>()
            .unwrap_or_else(|| self.build_window());
        self.refresh_window(&window);
        window.present();
    }

    fn build_window(&self) -> window::Window {
        let window = window::Window::new(self);

        window.connect_local(
            "settings-changed",
            false,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let window = values[0].get::<window::Window>().expect("the window");
                    app.apply(&window);
                    None
                }
            ),
        );
        window.connect_local(
            "download-requested",
            false,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let window = values[0].get::<window::Window>().expect("the window");
                    app.download_model(&window);
                    None
                }
            ),
        );
        window.connect_local(
            "remove-model-requested",
            false,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let window = values[0].get::<window::Window>().expect("the window");
                    let mode = app.config().mode;
                    match models::remove(mode) {
                        Ok(()) => window.toast("The model was removed."),
                        Err(error) => {
                            window.toast(&format!("The model could not be removed: {error}"))
                        }
                    }
                    app.refresh_window(&window);
                    None
                }
            ),
        );
        window.connect_local(
            "shortcut-change-requested",
            false,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let window = values[0].get::<window::Window>().expect("the window");
                    app.ask_for_shortcut(&window);
                    None
                }
            ),
        );

        window
    }

    /// Read the window back into the store and save.
    fn apply(&self, window: &window::Window) {
        let base = self.config();
        let updated = window.read_config(&base);
        if updated == base {
            return;
        }
        self.imp().store.borrow_mut().set_config(updated.clone());
        self.save();

        // The mode decides whether cleanup is even offered, and which model
        // the download button is talking about.
        if updated.mode != base.mode {
            window.show_config(&updated);
            self.refresh_window(window);
        }
    }

    fn refresh_window(&self, window: &window::Window) {
        let config = self.config();
        window.show_config(&config);
        window.show_shortcut(&config.shortcut, shortcut::installed().is_some());
        window.show_model(
            config.mode,
            engine::is_installed(config.mode),
            self.imp().download.borrow().is_some(),
        );

        let banner = if !shortcut::is_supported() {
            Some(
                "This desktop does not provide GNOME's custom shortcuts. Bind a key to \
                 “mynah --toggle” yourself."
                    .to_string(),
            )
        } else if self.imp().store.borrow().is_read_only() {
            Some("These settings were written by a newer Mynah and will not be saved over.".into())
        } else {
            None
        };
        window.show_banner(banner.as_deref());
    }

    fn ask_for_shortcut(&self, window: &window::Window) {
        let dialog = adw::AlertDialog::new(
            Some("Set the dictation shortcut"),
            Some("Press the keys you want to use, then choose Set."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("set", "Set");
        dialog.set_response_appearance("set", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("set"));

        let preview = gtk::Label::builder()
            .label(shortcut::human_label(&self.config().shortcut))
            .margin_top(12)
            .margin_bottom(12)
            .build();
        preview.add_css_class("title-2");
        dialog.set_extra_child(Some(&preview));

        // The captured accelerator, in the form gsettings wants.
        let captured = Rc::new(RefCell::new(self.config().shortcut.clone()));

        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        controller.connect_key_pressed(glib::clone!(
            #[weak]
            preview,
            #[strong]
            captured,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, state| {
                let modifiers = state & gtk::accelerator_get_default_mod_mask();
                // A bare modifier is the user still reaching for the key.
                if is_modifier_key(key) {
                    return glib::Propagation::Stop;
                }
                // A shortcut with no modifier would swallow that key for the
                // whole desktop.
                if modifiers.is_empty() {
                    preview.set_label("Add Ctrl, Alt or Super");
                    return glib::Propagation::Stop;
                }
                let accel = gtk::accelerator_name(key, modifiers);
                preview.set_label(&gtk::accelerator_get_label(key, modifiers));
                *captured.borrow_mut() = accel.to_string();
                glib::Propagation::Stop
            }
        ));
        dialog.add_controller(controller);

        dialog.connect_response(
            None,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[weak]
                window,
                #[strong]
                captured,
                move |_, response| {
                    if response != "set" {
                        return;
                    }
                    let accel = captured.borrow().clone();
                    let mut config = app.config();
                    config.shortcut = accel.clone();
                    app.imp().store.borrow_mut().set_config(config);
                    app.save();

                    match shortcut::install(&accel) {
                        Ok(()) => window.toast(&format!(
                            "{} now starts dictation.",
                            shortcut::human_label(&accel)
                        )),
                        Err(error) => window.toast(&error.to_string()),
                    }
                    app.refresh_window(&window);
                }
            ),
        );

        dialog.present(Some(window));
    }

    fn download_model(&self, window: &window::Window) {
        let mode = self.config().mode;
        if self.imp().download.borrow().is_some() {
            return;
        }

        let download = models::fetch(
            mode,
            glib::clone!(
                #[weak]
                window,
                move |progress: models::Progress| {
                    window.show_download_progress(progress.fraction, &progress.detail);
                }
            ),
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[weak]
                window,
                move |result: Result<(), models::DownloadError>| {
                    *app.imp().download.borrow_mut() = None;
                    window.hide_download_progress();
                    match result {
                        Ok(()) => window.toast("The speech model is ready."),
                        Err(error) => window.toast(&error.to_string()),
                    }
                    app.refresh_window(&window);
                }
            ),
        );
        *self.imp().download.borrow_mut() = Some(download);
        self.refresh_window(window);
    }

    // ---- dictation ----------------------------------------------------

    /// The shortcut, and the only entry point into dictation.
    pub fn toggle(&self) {
        match self.imp().activity.get() {
            Activity::Idle => self.start_listening(),
            Activity::Listening => self.stop_listening(),
            // A second press while transcribing is a user who thinks nothing
            // happened. Starting another recording on top would lose the one
            // in flight, so it is ignored.
            Activity::Working => {}
        }
    }

    fn overlay(&self) -> overlay::Overlay {
        let imp = self.imp();
        if let Some(existing) = imp.overlay.borrow().clone() {
            return existing;
        }
        let overlay = overlay::Overlay::new();
        overlay.set_application(Some(self));
        *imp.overlay.borrow_mut() = Some(overlay.clone());
        overlay
    }

    fn start_listening(&self) {
        let imp = self.imp();
        let config = self.config();

        if !engine::is_installed(config.mode) {
            self.report(&format!(
                "{} Open Mynah to download it.",
                engine::EngineError::NotInstalled(config.mode)
            ));
            return;
        }

        let overlay = self.overlay();
        overlay.reset(config.mode);
        overlay.present();

        imp.partial.borrow_mut().clear();
        imp.pending_audio.borrow_mut().clear();
        if config.mode == Mode::Streaming {
            if let Some(engine) = imp.engine.borrow().as_ref() {
                engine.reset_stream();
            }
        }

        let recorder = recorder::Recorder::start(
            &config.source,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                #[weak]
                overlay,
                move |block: &[f32]| {
                    overlay.set_level(recorder::level(block));
                    if app.config().mode == Mode::Streaming {
                        app.feed_stream(block);
                    }
                }
            ),
        );

        match recorder {
            Ok(recorder) => {
                *imp.recorder.borrow_mut() = Some(recorder);
                imp.activity.set(Activity::Listening);
            }
            Err(error) => {
                overlay.set_visible(false);
                self.report(&error.to_string());
            }
        }
    }

    /// Hand whole chunks to the streaming model as they accumulate.
    fn feed_stream(&self, block: &[f32]) {
        let imp = self.imp();
        imp.pending_audio.borrow_mut().extend_from_slice(block);

        loop {
            let chunk = {
                let mut pending = imp.pending_audio.borrow_mut();
                if pending.len() < engine::STREAM_CHUNK {
                    return;
                }
                pending.drain(..engine::STREAM_CHUNK).collect::<Vec<f32>>()
            };

            let Some(engine) = imp.engine.borrow().clone() else {
                return;
            };
            engine.feed(
                chunk,
                glib::clone!(
                    #[weak(rename_to = app)]
                    self,
                    move |result: Result<String, engine::EngineError>| {
                        let Ok(text) = result else { return };
                        if text.is_empty() {
                            return;
                        }
                        app.imp().partial.borrow_mut().push_str(&text);
                        let shown = app.imp().partial.borrow().clone();
                        if let Some(overlay) = app.imp().overlay.borrow().as_ref() {
                            overlay.set_detail(shown.trim());
                        }
                    }
                ),
            );
        }
    }

    fn stop_listening(&self) {
        let imp = self.imp();
        let config = self.config();
        let Some(recorder) = imp.recorder.borrow_mut().take() else {
            imp.activity.set(Activity::Idle);
            return;
        };
        let audio = recorder.finish();
        imp.activity.set(Activity::Working);

        let overlay = self.overlay();

        if config.mode == Mode::Streaming {
            // The streaming model has already said everything it is going to.
            let text = imp.partial.borrow().trim().to_string();
            overlay.set_visible(false);
            self.finish(text);
            return;
        }

        overlay.set_phase(overlay::Phase::Transcribing);
        overlay.set_detail("");

        // Nothing was said. Transcribing silence produces a hallucinated
        // sentence often enough to be worth refusing outright.
        if audio.len() < recorder::SAMPLE_RATE as usize / 4 {
            overlay.set_visible(false);
            imp.activity.set(Activity::Idle);
            return;
        }

        let Some(engine) = imp.engine.borrow().clone() else {
            imp.activity.set(Activity::Idle);
            return;
        };
        engine.transcribe(
            audio,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |result: Result<String, engine::EngineError>| match result {
                    Ok(text) => app.clean_up(text),
                    Err(error) => {
                        if let Some(overlay) = app.imp().overlay.borrow().as_ref() {
                            overlay.set_visible(false);
                        }
                        app.imp().activity.set(Activity::Idle);
                        app.report(&error.to_string());
                    }
                }
            ),
        );
    }

    fn clean_up(&self, text: String) {
        let config = self.config();
        if !config.cleanup_runs() || text.trim().is_empty() {
            if let Some(overlay) = self.imp().overlay.borrow().as_ref() {
                overlay.set_visible(false);
            }
            self.finish(text);
            return;
        }

        if let Some(overlay) = self.imp().overlay.borrow().as_ref() {
            overlay.set_phase(overlay::Phase::Polishing);
        }

        let original = text.clone();
        crate::ui::cleanup::polish(
            &config.endpoint,
            &config.cleanup_model,
            &text,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |result: Result<String, crate::ui::cleanup::CleanupError>| {
                    if let Some(overlay) = app.imp().overlay.borrow().as_ref() {
                        overlay.set_visible(false);
                    }
                    match result {
                        Ok(polished) => app.finish(polished),
                        Err(error) => {
                            // The dictation is never lost to a cleanup failure.
                            eprintln!("mynah: {error}");
                            app.finish(original.clone());
                        }
                    }
                }
            ),
        );
    }

    /// Apply the user's own rules and deliver the result.
    fn finish(&self, raw: String) {
        let imp = self.imp();
        imp.activity.set(Activity::Idle);

        let config = self.config();
        let text = model::polish(&raw, &config.vocabulary, config.spell_numbers);
        if text.is_empty() {
            return;
        }

        let Some(typist) = imp.typist.borrow().clone() else {
            return;
        };

        if config.delivery == Delivery::Clipboard || !typist.is_available() {
            inject::copy_to_clipboard(&text);
            self.report("Copied. Press Ctrl+V to paste.");
            return;
        }

        typist.deliver(
            &text,
            glib::clone!(
                #[weak(rename_to = app)]
                self,
                move |outcome: inject::Delivered| {
                    if outcome == inject::Delivered::Copied {
                        app.report("Mynah cannot type into other windows, so the text was copied.");
                    }
                }
            ),
        );
    }

    /// Say something to the user, wherever they can see it.
    ///
    /// Dictation happens with no Mynah window in sight, so a toast in a window
    /// nobody is looking at is not enough; a notification is what reaches
    /// somebody typing in a terminal.
    fn report(&self, message: &str) {
        if let Some(window) = self.active_window().and_downcast::<window::Window>() {
            if window.is_visible() {
                window.toast(message);
                return;
            }
        }
        let notification = gio::Notification::new("Mynah");
        notification.set_body(Some(message));
        self.send_notification(Some("mynah-status"), &notification);
    }
}
