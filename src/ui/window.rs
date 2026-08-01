//! The settings window.
//!
//! Scribe spends almost all of its life with no window open at all — it is a
//! shortcut and an overlay. So this window is not a workspace, it is the place
//! where the handful of decisions live, and it is laid out as preferences
//! because that is what it is.
//!
//! Widgets here emit intent and are told what to show. They do not read or
//! write the config file; the application does that, the same way it does in
//! the sibling apps.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use gtk::glib::subclass::Signal;
use std::sync::OnceLock;

use crate::model::{Config, Delivery, Mode, Rule};
use crate::ui::{models, shortcut};

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    pub struct Window {
        pub mode: RefCell<Option<adw::ComboRow>>,
        pub delivery: RefCell<Option<adw::ComboRow>>,
        pub shortcut_row: RefCell<Option<adw::ActionRow>>,
        pub permission_row: RefCell<Option<adw::ActionRow>>,
        pub permission_button: RefCell<Option<gtk::Button>>,
        pub preview: RefCell<Option<adw::SwitchRow>>,
        pub numbers: RefCell<Option<adw::SwitchRow>>,
        pub cleanup: RefCell<Option<adw::SwitchRow>>,
        pub endpoint: RefCell<Option<adw::EntryRow>>,
        pub model_row: RefCell<Option<adw::ActionRow>>,
        pub model_button: RefCell<Option<gtk::Button>>,
        pub model_progress: RefCell<Option<gtk::ProgressBar>>,
        pub vocabulary: RefCell<Option<adw::PreferencesGroup>>,
        pub vocabulary_rows: RefCell<Vec<adw::EntryRow>>,
        pub toasts: RefCell<Option<adw::ToastOverlay>>,
        pub banner: RefCell<Option<adw::Banner>>,
        /// Set while the application is filling the widgets in, so the
        /// handlers do not report those changes back as the user's.
        pub loading: std::cell::Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "ScribeWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for Window {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().build();
        }

        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // The user changed something; the application re-reads the
                    // window and saves.
                    Signal::builder("settings-changed").build(),
                    Signal::builder("download-requested").build(),
                    Signal::builder("remove-model-requested").build(),
                    Signal::builder("shortcut-change-requested").build(),
                    Signal::builder("permission-requested").build(),
                ]
            })
        }
    }
    impl WidgetImpl for Window {}
    impl WindowImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
                    gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(app: &impl IsA<gtk::Application>) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn changed(&self) {
        if !self.imp().loading.get() {
            self.emit_by_name::<()>("settings-changed", &[]);
        }
    }

    fn build(&self) {
        let imp = self.imp();
        self.set_title(Some("Scribe"));
        self.set_default_size(560, 720);

        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();

        let menu = gio::Menu::new();
        menu.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
        menu.append(Some("About Scribe"), Some("app.about"));
        menu.append(Some("Quit"), Some("app.quit"));
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .menu_model(&menu)
            .tooltip_text("Main Menu")
            .build();
        header.pack_end(&menu_button);
        toolbar.add_top_bar(&header);

        let banner = adw::Banner::new("");
        banner.set_revealed(false);
        toolbar.add_top_bar(&banner);

        let page = adw::PreferencesPage::new();
        page.add(&self.build_dictation_group());
        page.add(&self.build_model_group());
        page.add(&self.build_text_group());
        let vocabulary = self.build_vocabulary_group();
        page.add(&vocabulary);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&page));
        toolbar.set_content(Some(&toasts));
        self.set_content(Some(&toolbar));

        *imp.toasts.borrow_mut() = Some(toasts);
        *imp.banner.borrow_mut() = Some(banner);
        *imp.vocabulary.borrow_mut() = Some(vocabulary);
    }

    fn build_dictation_group(&self) -> adw::PreferencesGroup {
        let imp = self.imp();
        let group = adw::PreferencesGroup::builder().title("Dictation").build();

        let modes = gtk::StringList::new(&["Accurate", "Live"]);
        let mode = adw::ComboRow::builder()
            .title("Mode")
            .subtitle("Accurate transcribes when you stop. Live shows words as you speak them.")
            .model(&modes)
            .build();
        mode.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        group.add(&mode);

        let preview = adw::SwitchRow::builder()
            .title("Show words as you speak")
            .subtitle("Fills the recording window while you talk")
            .build();
        preview.connect_active_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        group.add(&preview);

        let shortcut_row = adw::ActionRow::builder()
            .title("Shortcut")
            .subtitle("Press to start dictating, press again to stop")
            .build();
        let change = gtk::Button::builder()
            .label("Change…")
            .valign(gtk::Align::Center)
            .build();
        change.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.emit_by_name::<()>("shortcut-change-requested", &[])
        ));
        shortcut_row.add_suffix(&change);
        group.add(&shortcut_row);

        let deliveries = gtk::StringList::new(&["Type into the window", "Copy to the clipboard"]);
        let delivery = adw::ComboRow::builder()
            .title("When finished")
            .model(&deliveries)
            .build();
        delivery.connect_selected_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        group.add(&delivery);

        // Typing into another window needs GNOME's permission, and until this
        // row existed a refusal was invisible: the text quietly went to the
        // clipboard and nothing said why.
        let permission_row = adw::ActionRow::builder().title("Typing permission").build();
        let permission_button = gtk::Button::builder()
            .label("Allow typing")
            .valign(gtk::Align::Center)
            .build();
        permission_button.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.emit_by_name::<()>("permission-requested", &[])
        ));
        permission_row.add_suffix(&permission_button);
        group.add(&permission_row);

        *imp.mode.borrow_mut() = Some(mode);
        *imp.preview.borrow_mut() = Some(preview);
        *imp.delivery.borrow_mut() = Some(delivery);
        *imp.shortcut_row.borrow_mut() = Some(shortcut_row);
        *imp.permission_row.borrow_mut() = Some(permission_row);
        *imp.permission_button.borrow_mut() = Some(permission_button);
        group
    }

    /// The live preview needs the streaming model, which is a separate
    /// download; say so rather than leaving a switch that does nothing.
    pub fn show_preview_available(&self, mode: Mode, available: bool) {
        let Some(row) = self.imp().preview.borrow().clone() else {
            return;
        };
        // In Live mode the words always appear — that is the mode — so the
        // switch would be a control over something the user cannot change.
        let optional = mode == Mode::Batch;
        row.set_sensitive(optional && available);
        row.set_subtitle(if !available {
            "Needs the streaming model, which has not been downloaded"
        } else if optional {
            "Fills the recording window while you talk"
        } else {
            "Always on in Live mode"
        });
    }

    /// Say whether Scribe may type into other windows, and by what route.
    pub fn show_permission(
        &self,
        typing_wanted: bool,
        granted: bool,
        refused: bool,
        via_extension: bool,
    ) {
        let imp = self.imp();
        let Some(row) = imp.permission_row.borrow().clone() else {
            return;
        };
        row.set_visible(typing_wanted);
        row.remove_css_class("error");

        let (subtitle, button, show_button) = if via_extension {
            // The extension does the typing from inside the compositor, so
            // there is no permission to grant and nothing to ask for.
            (
                "Handled by the Scribe shell extension — no permission needed",
                "Allow typing",
                false,
            )
        } else if granted {
            (
                "Granted — Scribe can type into the focused window",
                "Ask again",
                false,
            )
        } else if refused {
            row.add_css_class("error");
            (
                "Refused — transcripts are going to the clipboard instead",
                "Ask again",
                true,
            )
        } else {
            (
                "GNOME will ask the first time Scribe types somewhere",
                "Allow typing",
                true,
            )
        };
        row.set_subtitle(subtitle);
        if let Some(action) = imp.permission_button.borrow().as_ref() {
            action.set_label(button);
            action.set_visible(show_button);
        }
    }

    fn build_model_group(&self) -> adw::PreferencesGroup {
        let imp = self.imp();
        let group = adw::PreferencesGroup::builder()
            .title("Speech model")
            .description("Speech recognition runs on this machine. Nothing you dictate leaves it.")
            .build();

        let row = adw::ActionRow::builder().title("Model").build();
        let button = gtk::Button::builder().valign(gtk::Align::Center).build();
        button.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |button| {
                let signal = if button.has_css_class("destructive-action") {
                    "remove-model-requested"
                } else {
                    "download-requested"
                };
                window.emit_by_name::<()>(signal, &[]);
            }
        ));
        row.add_suffix(&button);
        group.add(&row);

        let progress = gtk::ProgressBar::builder()
            .visible(false)
            .show_text(true)
            .margin_top(6)
            .build();
        group.add(&progress);

        *imp.model_row.borrow_mut() = Some(row);
        *imp.model_button.borrow_mut() = Some(button);
        *imp.model_progress.borrow_mut() = Some(progress);
        group
    }

    fn build_text_group(&self) -> adw::PreferencesGroup {
        let imp = self.imp();
        let group = adw::PreferencesGroup::builder().title("Text").build();

        let numbers = adw::SwitchRow::builder()
            .title("Numbers as digits")
            .subtitle("Write “2026” rather than “twenty twenty six”")
            .build();
        numbers.connect_active_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        group.add(&numbers);

        let cleanup = adw::SwitchRow::builder()
            .title("Tidy with the language model")
            .subtitle("Removes “um” and false starts. Adds a moment before the text appears.")
            .build();
        cleanup.connect_active_notify(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        group.add(&cleanup);

        let endpoint = adw::EntryRow::builder()
            .title("Language model address")
            .build();
        endpoint.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        group.add(&endpoint);

        *imp.numbers.borrow_mut() = Some(numbers);
        *imp.cleanup.borrow_mut() = Some(cleanup);
        *imp.endpoint.borrow_mut() = Some(endpoint);
        group
    }

    fn build_vocabulary_group(&self) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title("Vocabulary")
            .description(
                "Words the model keeps getting wrong. The text on the left is replaced \
                 with the text on the right, in every transcript.",
            )
            .build();

        let add = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add a word")
            .valign(gtk::Align::Center)
            .build();
        add.add_css_class("flat");
        add.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| {
                window.add_vocabulary_row(&Rule::default());
                window.changed();
            }
        ));
        group.set_header_suffix(Some(&add));
        group
    }

    /// One rule: heard on the left, written on the right.
    fn add_vocabulary_row(&self, rule: &Rule) {
        let imp = self.imp();
        let Some(group) = imp.vocabulary.borrow().clone() else {
            return;
        };

        let row = adw::EntryRow::builder()
            .title("Heard as")
            .text(&rule.heard)
            .build();

        let write = gtk::Entry::builder()
            .placeholder_text("Written as")
            .text(&rule.write)
            .valign(gtk::Align::Center)
            .width_chars(16)
            .build();
        row.add_suffix(&write);

        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Remove")
            .valign(gtk::Align::Center)
            .build();
        remove.add_css_class("flat");
        row.add_suffix(&remove);

        // The written-as entry is a suffix widget rather than a field of its
        // own, so it is fished back out by name when the rules are read.
        unsafe { row.set_data("scribe-write", write.clone()) };

        row.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        write.connect_changed(glib::clone!(
            #[weak(rename_to = window)]
            self,
            move |_| window.changed()
        ));
        remove.connect_clicked(glib::clone!(
            #[weak(rename_to = window)]
            self,
            #[weak]
            group,
            #[weak]
            row,
            move |_| {
                group.remove(&row);
                window
                    .imp()
                    .vocabulary_rows
                    .borrow_mut()
                    .retain(|r| r != &row);
                window.changed();
            }
        ));

        group.add(&row);
        imp.vocabulary_rows.borrow_mut().push(row);
    }

    /// Fill the widgets from `config` without reporting the changes back.
    pub fn show_config(&self, config: &Config) {
        let imp = self.imp();
        imp.loading.set(true);

        if let Some(mode) = imp.mode.borrow().as_ref() {
            mode.set_selected(match config.mode {
                Mode::Batch => 0,
                Mode::Streaming => 1,
            });
        }
        if let Some(delivery) = imp.delivery.borrow().as_ref() {
            delivery.set_selected(match config.delivery {
                Delivery::Type => 0,
                Delivery::Clipboard => 1,
            });
        }
        if let Some(preview) = imp.preview.borrow().as_ref() {
            preview.set_active(config.preview);
        }
        if let Some(numbers) = imp.numbers.borrow().as_ref() {
            numbers.set_active(config.spell_numbers);
        }
        if let Some(cleanup) = imp.cleanup.borrow().as_ref() {
            cleanup.set_active(config.cleanup);
            cleanup.set_sensitive(config.mode.allows_cleanup());
            cleanup.set_subtitle(if config.mode.allows_cleanup() {
                "Removes “um” and false starts. Adds a moment before the text appears."
            } else {
                "Not available in Live mode: there is no finished sentence to tidy."
            });
        }
        if let Some(endpoint) = imp.endpoint.borrow().as_ref() {
            if endpoint.text() != config.endpoint {
                endpoint.set_text(&config.endpoint);
            }
        }

        // Rebuild the vocabulary rows only when they do not already match, so
        // typing in one does not pull the cursor out from under the user.
        let existing: Vec<Rule> = self.vocabulary_rules();
        if existing != config.vocabulary.rules() {
            if let Some(group) = imp.vocabulary.borrow().as_ref() {
                for row in imp.vocabulary_rows.borrow_mut().drain(..) {
                    group.remove(&row);
                }
            }
            for rule in config.vocabulary.rules() {
                self.add_vocabulary_row(rule);
            }
        }

        imp.loading.set(false);
    }

    fn vocabulary_rules(&self) -> Vec<Rule> {
        self.imp()
            .vocabulary_rows
            .borrow()
            .iter()
            .map(|row| {
                let write: Option<gtk::Entry> = unsafe {
                    row.data::<gtk::Entry>("scribe-write")
                        .map(|p| p.as_ref().clone())
                };
                Rule {
                    heard: row.text().to_string(),
                    write: write.map(|e| e.text().to_string()).unwrap_or_default(),
                    match_case: false,
                }
            })
            .collect()
    }

    /// Read the widgets back into a config, keeping the parts the window does
    /// not show.
    pub fn read_config(&self, base: &Config) -> Config {
        let imp = self.imp();
        let mut config = base.clone();

        if let Some(mode) = imp.mode.borrow().as_ref() {
            config.mode = if mode.selected() == 1 {
                Mode::Streaming
            } else {
                Mode::Batch
            };
        }
        if let Some(delivery) = imp.delivery.borrow().as_ref() {
            config.delivery = if delivery.selected() == 1 {
                Delivery::Clipboard
            } else {
                Delivery::Type
            };
        }
        if let Some(preview) = imp.preview.borrow().as_ref() {
            config.preview = preview.is_active();
        }
        if let Some(numbers) = imp.numbers.borrow().as_ref() {
            config.spell_numbers = numbers.is_active();
        }
        if let Some(cleanup) = imp.cleanup.borrow().as_ref() {
            config.cleanup = cleanup.is_active();
        }
        if let Some(endpoint) = imp.endpoint.borrow().as_ref() {
            config.endpoint = endpoint.text().to_string();
        }
        config.vocabulary.set_rules(self.vocabulary_rules());
        config
    }

    /// Say what the shortcut is, and whether anything is wrong with it.
    ///
    /// A shortcut GNOME already uses is the failure worth shouting about: both
    /// actions fire, so the user gets dictation *and* whatever GNOME does with
    /// that key, and nothing anywhere reports an error.
    pub fn show_shortcut(&self, accel: &str, registered: bool) {
        let Some(row) = self.imp().shortcut_row.borrow().clone() else {
            return;
        };
        let key = shortcut::human_label(accel);

        match shortcut::conflict(accel) {
            Some(taken) => {
                row.set_subtitle(&format!(
                    "{key} — GNOME already uses this for “{taken}”. Both will happen."
                ));
                row.add_css_class("error");
            }
            None => {
                row.remove_css_class("error");
                row.set_subtitle(&if registered {
                    format!("{key} — press to start, press again to stop")
                } else {
                    format!("{key} — not registered with GNOME")
                });
            }
        }
    }

    /// Say what the model situation is and what the button does about it.
    pub fn show_model(&self, mode: Mode, installed: bool, busy: bool) {
        let imp = self.imp();
        let name = match mode {
            Mode::Batch => "Parakeet TDT 0.6B",
            Mode::Streaming => "Nemotron streaming 0.6B",
        };

        if let Some(row) = imp.model_row.borrow().as_ref() {
            row.set_title(name);
            row.set_subtitle(&if installed {
                format!(
                    "Ready · {}",
                    models::human_size(models::installed_size(mode))
                )
            } else {
                format!(
                    "Not downloaded · {}",
                    models::human_size(models::download_size(mode))
                )
            });
        }
        if let Some(button) = imp.model_button.borrow().as_ref() {
            button.set_sensitive(!busy);
            button.remove_css_class("destructive-action");
            button.remove_css_class("suggested-action");
            if installed {
                button.set_label("Remove");
                button.add_css_class("destructive-action");
            } else {
                button.set_label("Download");
                button.add_css_class("suggested-action");
            }
        }
    }

    pub fn show_download_progress(&self, fraction: f64, detail: &str) {
        if let Some(progress) = self.imp().model_progress.borrow().as_ref() {
            progress.set_visible(true);
            progress.set_fraction(fraction);
            progress.set_text(Some(detail));
        }
    }

    pub fn hide_download_progress(&self) {
        if let Some(progress) = self.imp().model_progress.borrow().as_ref() {
            progress.set_visible(false);
        }
    }

    pub fn toast(&self, message: &str) {
        if let Some(toasts) = self.imp().toasts.borrow().as_ref() {
            toasts.add_toast(adw::Toast::new(message));
        }
    }

    /// A condition that persists, rather than an event that happened.
    pub fn show_banner(&self, message: Option<&str>) {
        if let Some(banner) = self.imp().banner.borrow().as_ref() {
            match message {
                Some(text) => {
                    banner.set_title(text);
                    banner.set_revealed(true);
                }
                None => banner.set_revealed(false),
            }
        }
    }
}
