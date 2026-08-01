//! Everything the user can change, and nothing they cannot.

use serde::{Deserialize, Serialize};

use super::vocabulary::Vocabulary;

/// Which speech model runs, which decides what else is possible.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Record first, transcribe once at the end. Most accurate, and the only
    /// mode where the language-model cleanup pass can run, because that pass
    /// needs a finished sentence to work on.
    #[default]
    Batch,
    /// Transcribe while the user is still speaking, a chunk at a time. Text
    /// appears as it is said; there is no finished transcript to clean up
    /// until the user stops, so cleanup is skipped.
    Streaming,
}

impl Mode {
    /// Whether the cleanup pass can run at all in this mode.
    pub fn allows_cleanup(self) -> bool {
        matches!(self, Mode::Batch)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Batch => "batch",
            Mode::Streaming => "streaming",
        }
    }
}

/// How the transcript reaches the window the user is looking at.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Synthesize the keystrokes through the RemoteDesktop portal. Costs one
    /// consent dialog, ever, and then works in every window including the
    /// terminal.
    #[default]
    Type,
    /// Put the transcript on the clipboard and say so. Needs no permission at
    /// all, and is what Scribe falls back to when consent is refused.
    Clipboard,
}

fn default_endpoint() -> String {
    // The llama-server this machine already runs.
    "http://127.0.0.1:8080".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    pub delivery: Delivery,

    /// The accelerator, in the form GTK and gnome-settings-daemon both parse.
    pub shortcut: String,

    /// Rewrite spoken numbers as digits before delivering the transcript.
    #[serde(default = "default_true")]
    pub spell_numbers: bool,

    /// Run the transcript past the local language model to strip the "um"s.
    pub cleanup: bool,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    /// Empty means "whatever the server has loaded", which is the common case
    /// for a single-model llama-server.
    pub cleanup_model: String,

    /// PipeWire target for `pw-record`. Empty means the system default, which
    /// is what almost everyone wants and what follows the user's choice in
    /// Settings without Scribe having to track it.
    pub source: String,

    pub vocabulary: Vocabulary,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Batch,
            delivery: Delivery::Type,
            shortcut: "<Super><Alt>d".to_string(),
            spell_numbers: true,
            cleanup: false,
            endpoint: default_endpoint(),
            cleanup_model: String::new(),
            source: String::new(),
            vocabulary: Vocabulary::default(),
        }
    }
}

impl Config {
    /// Whether cleanup should actually run, which is not the same as whether
    /// the user asked for it: streaming has nothing finished to clean up.
    pub fn cleanup_runs(&self) -> bool {
        self.cleanup && self.mode.allows_cleanup()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_is_off_in_streaming_even_when_asked_for() {
        let mut config = Config {
            cleanup: true,
            ..Config::default()
        };
        assert!(config.cleanup_runs());
        config.mode = Mode::Streaming;
        assert!(!config.cleanup_runs());
    }

    #[test]
    fn an_empty_file_deserialises_to_the_defaults() {
        let config: Config = serde_json::from_str("{}").expect("empty object");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn an_older_file_missing_new_keys_keeps_their_defaults() {
        // A config written before `spell_numbers` existed must not come back
        // with it silently off.
        let config: Config =
            serde_json::from_str(r#"{"mode":"batch","shortcut":"<Alt>d"}"#).expect("partial");
        assert!(config.spell_numbers);
        assert_eq!(config.shortcut, "<Alt>d");
        assert_eq!(config.endpoint, default_endpoint());
    }

    #[test]
    fn config_survives_a_round_trip() {
        let config = Config {
            mode: Mode::Streaming,
            delivery: Delivery::Clipboard,
            cleanup: true,
            ..Config::default()
        };
        let text = serde_json::to_string(&config).expect("serialise");
        assert_eq!(
            serde_json::from_str::<Config>(&text).expect("parse"),
            config
        );
    }
}
