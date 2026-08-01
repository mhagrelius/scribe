//! The speech models.
//!
//! Two models, one runtime. The batch model is Parakeet TDT 0.6B v3, which on
//! this machine transcribes eleven seconds of speech in under a quarter of a
//! second on the CPU and gets the punctuation right. The streaming model is
//! Nemotron's cache-aware streaming encoder, which takes 560 ms chunks and
//! emits text as it goes. Both come through `parakeet-rs`, so there is one
//! ONNX Runtime to build and one model layout to ship.
//!
//! Neither is fast enough to run on the main loop — a quarter of a second is
//! six dropped frames — so the model lives on a worker thread of its own and
//! is spoken to over a channel. It stays loaded between utterances because
//! loading it costs most of a second, which would be the longest part of a
//! short dictation.
//!
//! Results come back with `glib::idle_add_once`, the `Send` hop onto the main
//! loop. That is the whole of the threading story: no runtime, no executor,
//! and no widget ever touched from the worker.

use gtk::glib;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use parakeet_rs::{Nemotron, ParakeetTDT, TimestampMode, Transcriber};

use crate::model::Mode;

/// Where model files live once fetched.
pub fn models_dir() -> std::path::PathBuf {
    glib::user_data_dir().join("scribe").join("models")
}

pub fn batch_dir() -> std::path::PathBuf {
    models_dir().join("parakeet-tdt-0.6b-v3-int8")
}

pub fn streaming_dir() -> std::path::PathBuf {
    models_dir().join("nemotron-streaming-en-0.6b")
}

/// Chunk the streaming encoder is built around, in samples at 16 kHz.
pub const STREAM_CHUNK: usize = 8_960; // 560 ms

/// Whether the files a mode needs are present.
pub fn is_installed(mode: Mode) -> bool {
    let dir = match mode {
        Mode::Batch => batch_dir(),
        Mode::Streaming => streaming_dir(),
    };
    let required: &[&str] = match mode {
        Mode::Batch => &["vocab.txt", "nemo128.onnx"],
        Mode::Streaming => &["tokenizer.model"],
    };
    dir.is_dir() && required.iter().all(|name| dir.join(name).exists()) && has_encoder(&dir)
}

fn has_encoder(dir: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("encoder") && name.ends_with(".onnx")
    })
}

/// A failure the user needs told about.
#[derive(Debug)]
pub enum EngineError {
    NotInstalled(Mode),
    Load(String),
    Transcribe(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EngineError::NotInstalled(Mode::Batch) => {
                write!(f, "The speech model has not been downloaded yet.")
            }
            EngineError::NotInstalled(Mode::Streaming) => {
                write!(f, "The streaming speech model has not been downloaded yet.")
            }
            EngineError::Load(e) => write!(f, "The speech model could not be loaded: {e}"),
            EngineError::Transcribe(e) => write!(f, "The audio could not be transcribed: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// What the worker is asked to do.
enum Job {
    /// Transcribe a whole utterance.
    Batch {
        audio: Vec<f32>,
        reply: Box<dyn FnOnce(Result<String, EngineError>) + Send>,
    },
    /// Feed one chunk to the streaming model.
    Chunk {
        audio: Vec<f32>,
        reply: Box<dyn FnOnce(Result<String, EngineError>) + Send>,
    },
    /// Forget streaming state so the next utterance starts clean.
    ResetStream,
    Stop,
}

/// A handle to the worker thread.
pub struct Engine {
    jobs: Sender<Job>,
    /// Set while a batch transcription is outstanding, so a second stop cannot
    /// queue a second pass over the same audio.
    busy: Arc<Mutex<bool>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        let (jobs, inbox) = mpsc::channel();
        std::thread::Builder::new()
            .name("scribe-speech".into())
            .spawn(move || worker(inbox))
            .expect("the speech worker thread could not be started");
        Self {
            jobs,
            busy: Arc::new(Mutex::new(false)),
        }
    }

    pub fn is_busy(&self) -> bool {
        *self.busy.lock().expect("engine lock")
    }

    /// Transcribe a finished utterance. `done` runs on the main loop.
    pub fn transcribe(
        &self,
        audio: Vec<f32>,
        done: impl FnOnce(Result<String, EngineError>) + 'static,
    ) {
        {
            let mut busy = self.busy.lock().expect("engine lock");
            if *busy {
                return;
            }
            *busy = true;
        }
        let busy = self.busy.clone();
        let done = main_loop_hop(done);
        let _ = self.jobs.send(Job::Batch {
            audio,
            reply: Box::new(move |result| {
                *busy.lock().expect("engine lock") = false;
                done(result);
            }),
        });
    }

    /// Feed one chunk to the streaming model. `done` runs on the main loop.
    pub fn feed(&self, audio: Vec<f32>, done: impl FnOnce(Result<String, EngineError>) + 'static) {
        let _ = self.jobs.send(Job::Chunk {
            audio,
            reply: Box::new(main_loop_hop(done)),
        });
    }

    pub fn reset_stream(&self) {
        let _ = self.jobs.send(Job::ResetStream);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Stop);
    }
}

/// Wrap a main-loop closure so a worker thread can call it.
///
/// The closure itself is not `Send` — it captures widgets — so it is moved to
/// the main loop by `idle_add_once` and only the result crosses the boundary.
fn main_loop_hop<T: Send + 'static>(
    done: impl FnOnce(T) + 'static,
) -> impl FnOnce(T) + Send + 'static {
    let done = glib::thread_guard::ThreadGuard::new(done);
    move |value: T| {
        glib::idle_add_once(move || {
            (done.into_inner())(value);
        });
    }
}

/// The worker thread. Loads each model the first time it is needed and keeps
/// it, because loading costs most of a second.
fn worker(inbox: Receiver<Job>) {
    let mut batch: Option<ParakeetTDT> = None;
    let mut stream: Option<Nemotron> = None;

    while let Ok(job) = inbox.recv() {
        match job {
            Job::Stop => return,

            Job::ResetStream => {
                // Dropping the model is the reliable way to clear the encoder
                // cache; it is reloaded on the next chunk.
                stream = None;
            }

            Job::Batch { audio, reply } => {
                if !is_installed(Mode::Batch) {
                    reply(Err(EngineError::NotInstalled(Mode::Batch)));
                    continue;
                }
                if batch.is_none() {
                    match ParakeetTDT::from_pretrained(batch_dir(), None) {
                        Ok(model) => batch = Some(model),
                        Err(error) => {
                            reply(Err(EngineError::Load(error.to_string())));
                            continue;
                        }
                    }
                }
                let model = batch.as_mut().expect("just loaded");
                let result = model
                    .transcribe_samples(
                        audio,
                        crate::ui::recorder::SAMPLE_RATE,
                        1,
                        Some(TimestampMode::Sentences),
                    )
                    .map(|transcription| transcription.text)
                    .map_err(|error| EngineError::Transcribe(error.to_string()));
                reply(result);
            }

            Job::Chunk { audio, reply } => {
                if !is_installed(Mode::Streaming) {
                    reply(Err(EngineError::NotInstalled(Mode::Streaming)));
                    continue;
                }
                if stream.is_none() {
                    match Nemotron::from_pretrained(streaming_dir(), None) {
                        Ok(model) => stream = Some(model),
                        Err(error) => {
                            reply(Err(EngineError::Load(error.to_string())));
                            continue;
                        }
                    }
                }
                let model = stream.as_mut().expect("just loaded");
                let result = model
                    .transcribe_chunk(&audio)
                    .map_err(|error| EngineError::Transcribe(error.to_string()));
                reply(result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_directory_does_not_count_as_installed() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(!has_encoder(dir.path()));
    }

    #[test]
    fn an_encoder_is_found_under_either_of_its_names() {
        // The int8 and float builds of the same model are named differently,
        // and both are valid downloads.
        for name in [
            "encoder-model.int8.onnx",
            "encoder.onnx",
            "encoder-model.onnx",
        ] {
            let dir = tempfile::tempdir().expect("temp dir");
            std::fs::write(dir.path().join(name), b"").expect("write");
            assert!(has_encoder(dir.path()), "{name} should count as an encoder");
        }
    }

    #[test]
    fn a_decoder_alone_is_not_an_encoder() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("decoder_joint-model.onnx"), b"").expect("write");
        assert!(!has_encoder(dir.path()));
    }

    #[test]
    fn the_streaming_chunk_is_the_size_the_encoder_was_built_for() {
        // 560 ms at 16 kHz. Nemotron is cache-aware and this is not a free
        // parameter: a different chunk changes what the model was trained on.
        assert_eq!(STREAM_CHUNK, 560 * 16_000 / 1000);
    }
}
