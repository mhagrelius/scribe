//! Fetching the speech models.
//!
//! The models are not shipped with Mynah. They are NVIDIA's, they are hundreds
//! of megabytes each, and they are downloaded on first use into the user's own
//! data directory. Nothing here runs without being asked for.
//!
//! Each file is fetched to a `.part` beside its destination and renamed only
//! once the whole thing has arrived, because a truncated ONNX file is not a
//! smaller model, it is a protobuf parse error the next time the app starts.
//! Any file already present at its full size is skipped, so an interrupted
//! download resumes at file granularity rather than starting over.

use gio::prelude::*;
use gtk::glib;
use soup::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::model::Mode;

/// One file to fetch.
struct Source {
    name: &'static str,
    url: &'static str,
    /// What the server should report, so a truncated file is caught before it
    /// reaches the ONNX Runtime rather than after.
    bytes: u64,
}

const BATCH: &[Source] = &[
    Source {
        name: "encoder-model.int8.onnx",
        url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/encoder-model.int8.onnx",
        bytes: 652_183_999,
    },
    Source {
        name: "decoder_joint-model.int8.onnx",
        url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/decoder_joint-model.int8.onnx",
        bytes: 18_202_004,
    },
    Source {
        name: "nemo128.onnx",
        url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/nemo128.onnx",
        bytes: 139_764,
    },
    Source {
        name: "vocab.txt",
        url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/vocab.txt",
        bytes: 93_939,
    },
    Source {
        name: "config.json",
        url: "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main/config.json",
        bytes: 97,
    },
];

const STREAMING: &[Source] = &[
    Source {
        name: "encoder.onnx",
        url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/encoder.onnx",
        bytes: 880_555_453,
    },
    Source {
        name: "decoder_joint.onnx",
        url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/decoder_joint.onnx",
        bytes: 10_962_697,
    },
    Source {
        name: "tokenizer.model",
        url: "https://huggingface.co/lokkju/nemotron-speech-streaming-en-0.6b-int8/resolve/main/tokenizer.model",
        bytes: 251_056,
    },
];

fn sources(mode: Mode) -> &'static [Source] {
    match mode {
        Mode::Batch => BATCH,
        Mode::Streaming => STREAMING,
    }
}

fn destination(mode: Mode) -> PathBuf {
    match mode {
        Mode::Batch => super::engine::batch_dir(),
        Mode::Streaming => super::engine::streaming_dir(),
    }
}

/// Total download for a mode, for the UI to show before the user commits.
pub fn download_size(mode: Mode) -> u64 {
    sources(mode).iter().map(|s| s.bytes).sum()
}

/// A size in the units a person reads.
pub fn human_size(bytes: u64) -> String {
    const MB: f64 = 1_000_000.0;
    let mb = bytes as f64 / MB;
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[derive(Debug)]
pub enum DownloadError {
    Http {
        file: &'static str,
        status: u32,
    },
    Network {
        file: &'static str,
        detail: String,
    },
    /// The file arrived a different size than the server promised.
    Truncated {
        file: &'static str,
        got: u64,
        want: u64,
    },
    Io {
        file: &'static str,
        detail: String,
    },
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Http { file, status } => {
                write!(f, "Downloading {file} failed with HTTP {status}.")
            }
            DownloadError::Network { file, detail } => {
                write!(f, "Downloading {file} failed: {detail}")
            }
            DownloadError::Truncated { file, got, want } => write!(
                f,
                "{file} arrived incomplete ({got} bytes of {want}). \
                 The download was discarded rather than saved."
            ),
            DownloadError::Io { file, detail } => {
                write!(f, "{file} could not be written to disk: {detail}")
            }
        }
    }
}

impl std::error::Error for DownloadError {}

/// Called once when every file has arrived, or at the first failure.
type Done = Box<dyn FnOnce(Result<(), DownloadError>)>;

/// Progress, as a fraction and a sentence.
pub struct Progress {
    pub fraction: f64,
    pub detail: String,
}

/// A download in flight. Dropping it does not stop the transfer; call
/// [`Download::cancel`].
pub struct Download {
    cancellable: gio::Cancellable,
}

impl Download {
    pub fn cancel(&self) {
        self.cancellable.cancel();
    }
}

/// Fetch every file a mode needs.
///
/// `on_progress` runs on the main loop as bytes arrive; `done` runs once at
/// the end. Both are main-loop closures — this is gio async I/O, not a thread.
pub fn fetch(
    mode: Mode,
    on_progress: impl Fn(Progress) + 'static,
    done: impl FnOnce(Result<(), DownloadError>) + 'static,
) -> Download {
    let cancellable = gio::Cancellable::new();
    let directory = destination(mode);
    let total: u64 = download_size(mode);

    let state = Rc::new(State {
        session: soup::Session::new(),
        cancellable: cancellable.clone(),
        directory,
        total,
        finished_bytes: RefCell::new(0),
        on_progress: Box::new(on_progress),
        done: RefCell::new(Some(Box::new(done))),
    });

    next(state, mode, 0);
    Download { cancellable }
}

struct State {
    session: soup::Session,
    cancellable: gio::Cancellable,
    directory: PathBuf,
    total: u64,
    /// Bytes belonging to files already completed.
    finished_bytes: RefCell<u64>,
    on_progress: Box<dyn Fn(Progress)>,
    done: RefCell<Option<Done>>,
}

impl State {
    fn finish(&self, outcome: Result<(), DownloadError>) {
        if let Some(done) = self.done.borrow_mut().take() {
            done(outcome);
        }
    }

    fn report(&self, so_far: u64, name: &str) {
        let done = *self.finished_bytes.borrow() + so_far;
        let fraction = if self.total == 0 {
            0.0
        } else {
            (done as f64 / self.total as f64).clamp(0.0, 1.0)
        };
        (self.on_progress)(Progress {
            fraction,
            detail: format!(
                "{} of {} · {name}",
                human_size(done),
                human_size(self.total)
            ),
        });
    }
}

/// Fetch source `index`, then the one after it.
fn next(state: Rc<State>, mode: Mode, index: usize) {
    let list = sources(mode);
    let Some(source) = list.get(index) else {
        state.finish(Ok(()));
        return;
    };

    let target = state.directory.join(source.name);
    // Already here and the right size: nothing to do.
    if std::fs::metadata(&target).map(|m| m.len()).ok() == Some(source.bytes) {
        *state.finished_bytes.borrow_mut() += source.bytes;
        state.report(0, source.name);
        next(state, mode, index + 1);
        return;
    }

    if let Err(error) = std::fs::create_dir_all(&state.directory) {
        state.finish(Err(DownloadError::Io {
            file: source.name,
            detail: error.to_string(),
        }));
        return;
    }

    let message = match soup::Message::new("GET", source.url) {
        Ok(message) => message,
        Err(error) => {
            state.finish(Err(DownloadError::Network {
                file: source.name,
                detail: error.to_string(),
            }));
            return;
        }
    };

    let state_for_send = state.clone();
    let sent = message.clone();
    state.session.send_async(
        &message,
        glib::Priority::DEFAULT,
        Some(&state.cancellable),
        move |result| {
            let state = state_for_send;
            let status = sent.status_code();
            let stream = match result {
                Ok(stream) => stream,
                Err(error) => {
                    state.finish(Err(DownloadError::Network {
                        file: source.name,
                        detail: error.to_string(),
                    }));
                    return;
                }
            };
            if !(200..300).contains(&status) {
                state.finish(Err(DownloadError::Http {
                    file: source.name,
                    status,
                }));
                return;
            }
            drain(state, mode, index, source, stream, target);
        },
    );
}

/// Read the body to a `.part` file, then rename it into place.
fn drain(
    state: Rc<State>,
    mode: Mode,
    index: usize,
    source: &'static Source,
    stream: gio::InputStream,
    target: PathBuf,
) {
    let partial = target.with_extension("part");
    let file = gio::File::for_path(&partial);
    let output = match file.replace(
        None,
        false,
        gio::FileCreateFlags::REPLACE_DESTINATION,
        Some(&state.cancellable),
    ) {
        Ok(output) => output,
        Err(error) => {
            state.finish(Err(DownloadError::Io {
                file: source.name,
                detail: error.to_string(),
            }));
            return;
        }
    };

    glib::spawn_future_local(async move {
        let mut written: u64 = 0;
        loop {
            // 1 MiB per read: large enough that a 650 MB file is not a million
            // trips through the main loop, small enough that the progress bar
            // still moves.
            let buffer = vec![0u8; 1 << 20];
            let (buffer, read) = match stream.read_future(buffer, glib::Priority::DEFAULT).await {
                Ok(pair) => pair,
                Err((_, error)) => {
                    let _ = std::fs::remove_file(&partial);
                    state.finish(Err(DownloadError::Network {
                        file: source.name,
                        detail: error.to_string(),
                    }));
                    return;
                }
            };
            if read == 0 {
                break;
            }
            let chunk = glib::Bytes::from_owned(buffer[..read].to_vec());
            if let Err(error) = output
                .write_bytes_future(&chunk, glib::Priority::DEFAULT)
                .await
            {
                let _ = std::fs::remove_file(&partial);
                state.finish(Err(DownloadError::Io {
                    file: source.name,
                    detail: error.to_string(),
                }));
                return;
            }
            written += read as u64;
            state.report(written, source.name);
        }

        if let Err(error) = output.close_future(glib::Priority::DEFAULT).await {
            let _ = std::fs::remove_file(&partial);
            state.finish(Err(DownloadError::Io {
                file: source.name,
                detail: error.to_string(),
            }));
            return;
        }

        // The check that stops a half-downloaded encoder from being loaded as
        // a whole one. This is the failure that costs an hour to diagnose.
        if written != source.bytes {
            let _ = std::fs::remove_file(&partial);
            state.finish(Err(DownloadError::Truncated {
                file: source.name,
                got: written,
                want: source.bytes,
            }));
            return;
        }

        if let Err(error) = std::fs::rename(&partial, &target) {
            let _ = std::fs::remove_file(&partial);
            state.finish(Err(DownloadError::Io {
                file: source.name,
                detail: error.to_string(),
            }));
            return;
        }

        *state.finished_bytes.borrow_mut() += source.bytes;
        next(state, mode, index + 1);
    });
}

/// Remove a downloaded model.
pub fn remove(mode: Mode) -> std::io::Result<()> {
    let directory = destination(mode);
    if directory.exists() {
        std::fs::remove_dir_all(directory)?;
    }
    Ok(())
}

/// How much disk a mode's files take up right now.
pub fn installed_size(mode: Mode) -> u64 {
    fn total(path: &Path) -> u64 {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|meta| meta.len())
            .sum()
    }
    total(&destination(mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_a_person_would_say_them() {
        assert_eq!(human_size(97), "0 MB");
        assert_eq!(human_size(652_183_999), "652 MB");
        assert_eq!(human_size(1_500_000_000), "1.5 GB");
    }

    #[test]
    fn every_source_declares_a_size_so_truncation_can_be_caught() {
        for mode in [Mode::Batch, Mode::Streaming] {
            for source in sources(mode) {
                assert!(source.bytes > 0, "{} has no declared size", source.name);
                assert!(
                    source.url.starts_with("https://"),
                    "{} is not fetched over https",
                    source.name
                );
            }
        }
    }

    #[test]
    fn each_mode_brings_an_encoder_a_decoder_and_a_vocabulary() {
        for mode in [Mode::Batch, Mode::Streaming] {
            let names: Vec<&str> = sources(mode).iter().map(|s| s.name).collect();
            assert!(
                names.iter().any(|n| n.starts_with("encoder")),
                "{mode:?} has no encoder"
            );
            assert!(
                names.iter().any(|n| n.contains("decoder")),
                "{mode:?} has no decoder"
            );
            assert!(
                names
                    .iter()
                    .any(|n| n.contains("vocab") || n.contains("tokenizer")),
                "{mode:?} has no vocabulary"
            );
        }
    }

    #[test]
    fn the_download_is_the_sum_of_its_files() {
        assert_eq!(
            download_size(Mode::Batch),
            BATCH.iter().map(|s| s.bytes).sum::<u64>()
        );
        assert!(download_size(Mode::Batch) > 600_000_000);
    }
}
