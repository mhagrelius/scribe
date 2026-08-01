//! Widget tests.
//!
//! All of them live in one `#[test]`. GTK is thread-affine and `cargo test`
//! runs each test function on a thread of its own; `--test-threads=1`
//! serialises them but does not make them share a thread, so a second test
//! function would find the display initialised on somebody else's thread and
//! abort the process. One test, a table of cases, and a runner that catches
//! each panic so one failure does not hide the rest.

use adw::prelude::*;
use mynah::model::{Config, Delivery, Mode, Rule, Vocabulary};
use mynah::ui::{shortcut, window::Window};

/// Runs named cases, collecting failures rather than stopping at the first.
struct Runner {
    failures: Vec<String>,
}

impl Runner {
    fn new() -> Self {
        Self {
            failures: Vec::new(),
        }
    }

    fn case(&mut self, name: &str, body: impl FnOnce()) {
        // The cases share widgets and an application, none of which are
        // `UnwindSafe`. Nothing is resumed after a panic here — the failure is
        // recorded and the next case builds its own window — so the assertion
        // the marker would make is one this harness genuinely upholds.
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
            let detail = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panicked".to_string());
            self.failures.push(format!("{name}: {detail}"));
        }
    }

    fn finish(self) {
        assert!(
            self.failures.is_empty(),
            "{} case(s) failed:\n  {}",
            self.failures.len(),
            self.failures.join("\n  ")
        );
    }
}

/// One application for the whole run.
///
/// The id differs from the real one on purpose: sharing it would register this
/// process as a remote for a running Mynah and hand it the commands. It is
/// registered up front because `GtkApplication` warns — and under
/// `G_DEBUG=fatal-criticals` aborts — on a window added before startup has
/// been emitted, and it is built only once because a second registration of
/// the same id collides on the bus.
fn test_application() -> adw::Application {
    let app = adw::Application::builder()
        .application_id("us.hagreli.Mynah.Test")
        .flags(gio::ApplicationFlags::IS_SERVICE)
        .build();
    app.register(gio::Cancellable::NONE)
        .expect("register the test application");
    app
}

#[test]
fn widgets() {
    if gtk::init().is_err() {
        eprintln!("no display; skipping widget tests");
        return;
    }
    adw::init().expect("libadwaita");
    let app = test_application();
    let test_window = || Window::new(&app);
    let mut runner = Runner::new();

    runner.case("a config shown is a config read back", || {
        let window = test_window();
        let config = Config {
            mode: Mode::Streaming,
            delivery: Delivery::Clipboard,
            spell_numbers: false,
            cleanup: true,
            endpoint: "http://127.0.0.1:9999".into(),
            ..Config::default()
        };
        window.show_config(&config);
        let read = window.read_config(&Config::default());

        assert_eq!(read.mode, Mode::Streaming);
        assert_eq!(read.delivery, Delivery::Clipboard);
        assert!(!read.spell_numbers);
        assert!(read.cleanup);
        assert_eq!(read.endpoint, "http://127.0.0.1:9999");
    });

    runner.case("vocabulary rules survive a round trip", || {
        let window = test_window();
        let config = Config {
            vocabulary: Vocabulary::from_rules(vec![
                Rule::new("mina", "Mynah"),
                Rule::new("kubernetes", "k8s"),
            ]),
            ..Config::default()
        };
        window.show_config(&config);
        let read = window.read_config(&Config::default());
        assert_eq!(read.vocabulary.rules().len(), 2);
        assert_eq!(read.vocabulary.rules()[0].heard, "mina");
        assert_eq!(read.vocabulary.rules()[0].write, "Mynah");
        assert_eq!(read.vocabulary.rules()[1].write, "k8s");
    });

    runner.case(
        "showing the same config twice does not duplicate rules",
        || {
            // `show_config` is called on every refresh, and a naive implementation
            // appends the rules again each time.
            let window = test_window();
            let config = Config {
                vocabulary: Vocabulary::from_rules(vec![Rule::new("mina", "Mynah")]),
                ..Config::default()
            };
            window.show_config(&config);
            window.show_config(&config);
            window.show_config(&config);
            assert_eq!(
                window
                    .read_config(&Config::default())
                    .vocabulary
                    .rules()
                    .len(),
                1
            );
        },
    );

    runner.case("cleanup is disabled in streaming mode", || {
        let window = test_window();
        window.show_config(&Config {
            mode: Mode::Batch,
            ..Config::default()
        });
        window.show_config(&Config {
            mode: Mode::Streaming,
            ..Config::default()
        });
        // Reading back must not silently turn the user's preference off; the
        // row is insensitive, and `cleanup_runs` is what decides.
        let read = window.read_config(&Config {
            cleanup: true,
            ..Config::default()
        });
        assert_eq!(read.mode, Mode::Streaming);
        assert!(
            !read.cleanup_runs(),
            "streaming must not run the cleanup pass"
        );
    });

    runner.case("fields the window does not show are preserved", || {
        // The audio source has no widget. Reading the window back must not
        // wipe it.
        let window = test_window();
        let base = Config {
            source: "alsa_input.thing".into(),
            ..Config::default()
        };
        window.show_config(&base);
        assert_eq!(window.read_config(&base).source, "alsa_input.thing");
    });

    runner.case("model and shortcut rows can be filled in", || {
        let window = test_window();
        window.show_model(Mode::Batch, false, false);
        window.show_model(Mode::Batch, true, false);
        window.show_model(Mode::Streaming, false, true);
        window.show_shortcut("<Control><Alt>d", true);
        window.show_shortcut("<Control><Alt>d", false);
        window.show_download_progress(0.5, "half");
        window.hide_download_progress();
        window.toast("hello");
        window.show_banner(Some("a condition"));
        window.show_banner(None);
    });

    runner.case("accelerators are checked before being written", || {
        assert!(shortcut::is_parsable("<Control><Alt>d"));
        assert!(shortcut::is_parsable("<Super>space"));
        assert!(!shortcut::is_parsable("not an accelerator"));
        assert!(!shortcut::is_parsable(""));
    });

    runner.case("an accelerator is shown the way a person reads it", || {
        let label = shortcut::human_label("<Control><Alt>d");
        assert!(label.contains('D'), "got {label}");
        assert!(!label.contains('<'), "got {label}");
        // Something unparsable comes back as itself rather than as an empty
        // row.
        assert_eq!(shortcut::human_label("gibberish"), "gibberish");
    });

    runner.finish();
}
