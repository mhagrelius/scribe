//! The config file on disk.
//!
//! One small JSON file under `$XDG_CONFIG_HOME/scribe/`. It is written the way
//! the sibling apps write theirs — to a temporary file that is flushed and
//! synced before being renamed over the real one — so a crash or a power cut
//! during a save leaves either the old file or the new one, never half of
//! either.
//!
//! A file this process cannot parse is moved aside rather than overwritten.
//! The settings in it are typed by hand and are worth more to the user than
//! they are to Scribe, so losing them silently is not an option.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::config::Config;

/// Bumped only when a change cannot be expressed as "new key, old default".
const VERSION: u32 = 1;

#[derive(serde::Deserialize, serde::Serialize)]
struct File {
    version: u32,
    #[serde(flatten)]
    config: Config,
}

#[derive(Debug)]
pub enum SaveError {
    /// The store has no path, which is what `detached` produces.
    NoPath,
    Io(std::io::Error),
    Serialise(serde_json::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::NoPath => write!(f, "This copy of the settings is not backed by a file."),
            SaveError::Io(e) => write!(f, "The settings could not be written to disk: {e}"),
            SaveError::Serialise(e) => write!(f, "The settings could not be encoded: {e}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SaveError::NoPath => None,
            SaveError::Io(e) => Some(e),
            SaveError::Serialise(e) => Some(e),
        }
    }
}

/// What happened when the store was opened, so the UI can say so.
#[derive(Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No file yet. A first run.
    Fresh,
    Loaded,
    /// The file could not be parsed and was renamed to the given path.
    Recovered(PathBuf),
    /// The file was written by a newer Scribe. Loaded as far as possible and
    /// held read-only, so a downgrade cannot quietly delete the new keys.
    TooNew {
        found: u32,
    },
}

pub struct Store {
    path: Option<PathBuf>,
    config: Config,
    read_only: bool,
}

impl Store {
    /// A store with no file behind it. Tests and the default application state
    /// use this, which is why it is a real value rather than an `Option`.
    pub fn detached() -> Self {
        Self {
            path: None,
            config: Config::default(),
            read_only: false,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The file Scribe uses when nobody says otherwise.
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        base.join("scribe").join("config.json")
    }

    pub fn open(path: impl Into<PathBuf>) -> (Self, LoadOutcome) {
        let path = path.into();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                return (
                    Self {
                        path: Some(path),
                        config: Config::default(),
                        read_only: false,
                    },
                    LoadOutcome::Fresh,
                )
            }
        };

        match serde_json::from_str::<File>(&text) {
            Ok(file) if file.version > VERSION => {
                let found = file.version;
                (
                    Self {
                        path: Some(path),
                        config: file.config,
                        read_only: true,
                    },
                    LoadOutcome::TooNew { found },
                )
            }
            Ok(file) => (
                Self {
                    path: Some(path),
                    config: file.config,
                    read_only: false,
                },
                LoadOutcome::Loaded,
            ),
            Err(_) => {
                let aside = set_aside(&path);
                (
                    Self {
                        path: Some(path),
                        config: Config::default(),
                        read_only: false,
                    },
                    LoadOutcome::Recovered(aside),
                )
            }
        }
    }

    /// Write the file, or explain why not. A read-only store reports success
    /// without writing: refusing to save is the point, not a failure.
    pub fn save(&self) -> Result<(), SaveError> {
        if self.read_only {
            return Ok(());
        }
        let path = self.path.as_ref().ok_or(SaveError::NoPath)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(SaveError::Io)?;
        }

        let file = File {
            version: VERSION,
            config: self.config.clone(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(SaveError::Serialise)?;

        let temporary = path.with_extension("json.tmp");
        {
            let mut handle = fs::File::create(&temporary).map_err(SaveError::Io)?;
            handle.write_all(text.as_bytes()).map_err(SaveError::Io)?;
            handle.write_all(b"\n").map_err(SaveError::Io)?;
            handle.flush().map_err(SaveError::Io)?;
            handle.sync_all().map_err(SaveError::Io)?;
        }
        fs::rename(&temporary, path).map_err(SaveError::Io)
    }
}

/// Move an unreadable file out of the way, keeping whatever is in it.
fn set_aside(path: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let aside = path.with_extension(format!("json.corrupt-{stamp}"));
    let _ = fs::rename(path, &aside);
    aside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::Mode;

    fn temp() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    #[test]
    fn a_missing_file_is_a_fresh_start_not_an_error() {
        let dir = temp();
        let (store, outcome) = Store::open(dir.path().join("config.json"));
        assert_eq!(outcome, LoadOutcome::Fresh);
        assert_eq!(store.config(), &Config::default());
    }

    #[test]
    fn what_was_saved_is_what_comes_back() {
        let dir = temp();
        let path = dir.path().join("config.json");

        let (mut store, _) = Store::open(&path);
        store.set_config(Config {
            mode: Mode::Streaming,
            cleanup: true,
            ..Config::default()
        });
        store.save().expect("save");

        let (reopened, outcome) = Store::open(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(reopened.config().mode, Mode::Streaming);
        assert!(reopened.config().cleanup);
    }

    #[test]
    fn saving_creates_the_directory_it_needs() {
        let dir = temp();
        let path = dir.path().join("nested").join("deeper").join("config.json");
        let (store, _) = Store::open(&path);
        store
            .save()
            .expect("save into a directory that does not exist yet");
        assert!(path.exists());
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_and_its_contents_kept() {
        let dir = temp();
        let path = dir.path().join("config.json");
        fs::write(&path, "{ this is not json").expect("write");

        let (store, outcome) = Store::open(&path);
        let LoadOutcome::Recovered(aside) = outcome else {
            panic!("expected the file to be recovered, got {outcome:?}");
        };
        assert!(
            aside.exists(),
            "the unreadable file should still be on disk"
        );
        assert_eq!(
            fs::read_to_string(&aside).expect("read aside"),
            "{ this is not json"
        );
        assert_eq!(store.config(), &Config::default());
        assert!(
            !store.is_read_only(),
            "a fresh start after recovery is writable"
        );
    }

    #[test]
    fn a_newer_file_is_held_read_only_and_never_written_over() {
        let dir = temp();
        let path = dir.path().join("config.json");
        let future = format!(r#"{{"version":{},"shortcut":"<Alt>z"}}"#, VERSION + 1);
        fs::write(&path, &future).expect("write");

        let (store, outcome) = Store::open(&path);
        assert_eq!(outcome, LoadOutcome::TooNew { found: VERSION + 1 });
        assert!(store.is_read_only());
        assert_eq!(store.config().shortcut, "<Alt>z");

        store
            .save()
            .expect("a read-only save is a no-op, not an error");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            future,
            "file untouched"
        );
    }

    #[test]
    fn a_detached_store_reports_that_it_has_nowhere_to_go() {
        let store = Store::detached();
        assert!(matches!(store.save(), Err(SaveError::NoPath)));
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = temp();
        let path = dir.path().join("config.json");
        let (store, _) = Store::open(&path);
        store.save().expect("save");

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }
}
