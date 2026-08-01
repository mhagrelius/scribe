//! Whole scenarios against the config file, with no display.
//!
//! These are the paths a user takes over time rather than in one sitting:
//! settings written today and read next week, a file corrupted by something
//! else, a Mynah newer than this one having been here first.

use mynah::model::{polish, Config, Delivery, LoadOutcome, Mode, Rule, Store, Vocabulary};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("temp dir")
}

#[test]
fn settings_survive_a_restart() {
    let dir = temp();
    let path = dir.path().join("config.json");

    {
        let (mut store, outcome) = Store::open(&path);
        assert_eq!(outcome, LoadOutcome::Fresh);
        store.set_config(Config {
            mode: Mode::Streaming,
            delivery: Delivery::Clipboard,
            shortcut: "<Super>d".into(),
            cleanup: true,
            vocabulary: Vocabulary::from_rules(vec![Rule::new("mina", "Mynah")]),
            ..Config::default()
        });
        store.save().expect("save");
    }

    let (store, outcome) = Store::open(&path);
    assert_eq!(outcome, LoadOutcome::Loaded);
    let config = store.config();
    assert_eq!(config.mode, Mode::Streaming);
    assert_eq!(config.delivery, Delivery::Clipboard);
    assert_eq!(config.shortcut, "<Super>d");
    assert!(config.cleanup);
    assert_eq!(config.vocabulary.rules().len(), 1);
}

#[test]
fn a_config_from_an_older_mynah_keeps_working() {
    // Written before `spell_numbers`, `delivery` and `vocabulary` existed.
    let dir = temp();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        r#"{"version":1,"mode":"batch","shortcut":"<Alt>space"}"#,
    )
    .expect("write");

    let (store, outcome) = Store::open(&path);
    assert_eq!(outcome, LoadOutcome::Loaded);
    assert_eq!(store.config().shortcut, "<Alt>space");
    assert!(
        store.config().spell_numbers,
        "a missing key takes its default"
    );
    assert_eq!(store.config().delivery, Delivery::Type);
    assert!(store.config().vocabulary.is_empty());

    // And saving upgrades the file rather than refusing.
    store.save().expect("save");
    let (reopened, _) = Store::open(&path);
    assert_eq!(reopened.config().shortcut, "<Alt>space");
}

#[test]
fn a_file_mangled_by_something_else_does_not_lose_the_user_their_app() {
    let dir = temp();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "\u{0}\u{0}truncated garbage").expect("write");

    let (mut store, outcome) = Store::open(&path);
    let LoadOutcome::Recovered(aside) = outcome else {
        panic!("expected recovery");
    };
    assert!(aside.exists());
    assert_eq!(store.config(), &Config::default());

    // Mynah carries on and the next save works.
    store.set_config(Config {
        shortcut: "<Super>v".into(),
        ..Config::default()
    });
    store.save().expect("save");
    let (reopened, outcome) = Store::open(&path);
    assert_eq!(outcome, LoadOutcome::Loaded);
    assert_eq!(reopened.config().shortcut, "<Super>v");
}

#[test]
fn a_newer_config_is_never_written_over() {
    let dir = temp();
    let path = dir.path().join("config.json");
    let original = r#"{"version":99,"shortcut":"<Super>z","something_new":true}"#;
    std::fs::write(&path, original).expect("write");

    let (mut store, outcome) = Store::open(&path);
    assert!(matches!(outcome, LoadOutcome::TooNew { .. }));
    assert!(store.is_read_only());

    store.set_config(Config {
        shortcut: "<Alt>x".into(),
        ..Config::default()
    });
    store
        .save()
        .expect("a read-only save reports success without writing");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        original,
        "the newer file must be exactly as it was"
    );
}

#[test]
fn a_dictation_goes_through_every_rule_the_user_set() {
    // The whole text pipeline, in the order the application runs it.
    let vocabulary = Vocabulary::from_rules(vec![
        Rule::new("kubernetes", "k8s"),
        Rule::new("2026", "FY26"),
    ]);

    let spoken = "we ship kubernetes in twenty twenty six";
    assert_eq!(polish(spoken, &vocabulary, true), "we ship k8s in FY26");

    // With number rewriting off, the rule written against digits no longer
    // matches — which is the user's choice, not a bug.
    assert_eq!(
        polish(spoken, &vocabulary, false),
        "we ship k8s in twenty twenty six"
    );
}

#[test]
fn an_empty_dictation_produces_nothing_to_deliver() {
    let vocabulary = Vocabulary::default();
    assert_eq!(polish("", &vocabulary, true), "");
    assert_eq!(polish("   \n  ", &vocabulary, true), "");
}
