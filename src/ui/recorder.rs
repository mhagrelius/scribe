//! The microphone.
//!
//! `pw-record` is spawned and its raw samples are read off a pipe by the GLib
//! main loop. There is no audio library here and no thread of our own: this
//! machine runs PipeWire, `pw-record` ships with it, and `gio::Subprocess`
//! already knows how to read a pipe without blocking anything. It is the same
//! shape Magpie uses to follow a downloader.
//!
//! Samples come back as 16-bit little-endian mono at 16 kHz, which is what
//! both speech models want, so PipeWire does the resampling and Mynah does
//! none. They are handed on as `f32` in −1.0..1.0, the form the models take.

use gio::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// What the models expect. Not configurable, because nothing else is correct.
pub const SAMPLE_RATE: u32 = 16_000;

/// How much audio to read per pass. 40 ms is short enough that the level meter
/// looks live and long enough that the main loop is not woken constantly.
const READ_BYTES: usize = (SAMPLE_RATE as usize / 25) * 2;

/// Handed each new block of samples as it is read off the pipe.
type OnChunk = Rc<dyn Fn(&[f32])>;

/// A refusal to start recording, in terms the user can act on.
#[derive(Debug)]
pub enum StartError {
    /// `pw-record` is not installed.
    ToolMissing,
    Spawn(glib::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::ToolMissing => write!(
                f,
                "pw-record is not installed. It comes with PipeWire, in the pipewire-bin package \
                 on Debian and Ubuntu and in pipewire-utils on Fedora."
            ),
            StartError::Spawn(e) => write!(f, "The microphone could not be opened: {e}"),
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StartError::ToolMissing => None,
            StartError::Spawn(e) => Some(e),
        }
    }
}

/// A running capture.
pub struct Recorder {
    process: gio::Subprocess,
    /// Everything heard so far, for the batch pass to transcribe at the end.
    samples: Rc<RefCell<Vec<f32>>>,
    stopped: Rc<Cell<bool>>,
}

impl Recorder {
    /// Start recording.
    ///
    /// `on_chunk` is called on the main loop with each new block of samples —
    /// the overlay uses it to draw a level, and streaming mode feeds it
    /// straight to the model.
    pub fn start(source: &str, on_chunk: impl Fn(&[f32]) + 'static) -> Result<Self, StartError> {
        if glib::find_program_in_path("pw-record").is_none() {
            return Err(StartError::ToolMissing);
        }

        let mut argv: Vec<&str> = vec![
            "pw-record",
            "--rate",
            "16000",
            "--channels",
            "1",
            "--format",
            "s16",
        ];
        if !source.trim().is_empty() {
            argv.push("--target");
            argv.push(source.trim());
        }
        // `-` is stdout, which is where we read from.
        argv.push("-");

        let process = gio::Subprocess::newv(
            &argv.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>(),
            gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
        )
        .map_err(StartError::Spawn)?;

        let samples = Rc::new(RefCell::new(Vec::new()));
        let stopped = Rc::new(Cell::new(false));

        if let Some(stdout) = process.stdout_pipe() {
            read_loop(stdout, samples.clone(), stopped.clone(), Rc::new(on_chunk));
        }

        Ok(Self {
            process,
            samples,
            stopped,
        })
    }

    /// Stop recording and take everything heard.
    pub fn finish(self) -> Vec<f32> {
        self.stopped.set(true);
        self.process.force_exit();
        let taken = std::mem::take(&mut *self.samples.borrow_mut());
        taken
    }

    /// How long has been recorded so far.
    pub fn duration(&self) -> std::time::Duration {
        let count = self.samples.borrow().len();
        std::time::Duration::from_secs_f64(count as f64 / SAMPLE_RATE as f64)
    }
}

/// Read the pipe until it ends, one block at a time.
///
/// Each read schedules the next, so there is exactly one outstanding read and
/// no way to overlap two into the same buffer.
fn read_loop(
    stream: gio::InputStream,
    samples: Rc<RefCell<Vec<f32>>>,
    stopped: Rc<Cell<bool>>,
    on_chunk: OnChunk,
) {
    glib::spawn_future_local(async move {
        loop {
            if stopped.get() {
                return;
            }
            let buffer = vec![0u8; READ_BYTES];
            let (buffer, read) = match stream.read_future(buffer, glib::Priority::DEFAULT).await {
                Ok(result) => result,
                Err(_) => return,
            };
            if read == 0 || stopped.get() {
                return;
            }

            // An odd byte count would split a sample across two reads. Reading
            // a whole number of frames each time keeps that from happening.
            let usable = read - (read % 2);
            let block: Vec<f32> = buffer[..usable]
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0)
                .collect();

            samples.borrow_mut().extend_from_slice(&block);
            on_chunk(&block);
        }
    });
}

/// Loudness of a block, as a 0.0..1.0 figure for the level meter.
///
/// Root mean square rather than peak: a peak meter on speech spends its time
/// pinned by consonants and tells the user nothing about whether they are
/// being heard.
///
/// The exponent then bends the scale. Speech sits around an RMS of 0.05 to
/// 0.2, which on a linear meter is a bar that never leaves the left-hand
/// tenth; raising it to a fractional power lifts that range into the middle of
/// the bar while still leaving somewhere for a shout to go. Anything that
/// reached 1.0 before a real shout would make the meter useless as a sign that
/// the microphone is working.
pub fn level(block: &[f32]) -> f64 {
    if block.is_empty() {
        return 0.0;
    }
    let sum: f64 = block.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    let rms = (sum / block.len() as f64).sqrt();
    rms.clamp(0.0, 1.0).powf(0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_reads_as_no_level() {
        assert_eq!(level(&[0.0; 128]), 0.0);
        assert_eq!(level(&[]), 0.0);
    }

    #[test]
    fn a_full_scale_tone_pins_the_meter() {
        let square: Vec<f32> = (0..128)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        assert_eq!(level(&square), 1.0);
    }

    #[test]
    fn louder_input_reads_higher() {
        let quiet: Vec<f32> = (0..128)
            .map(|i| if i % 2 == 0 { 0.01 } else { -0.01 })
            .collect();
        let loud: Vec<f32> = (0..128)
            .map(|i| if i % 2 == 0 { 0.3 } else { -0.3 })
            .collect();
        assert!(level(&quiet) < level(&loud));
        assert!(level(&loud) < 1.0, "speech must leave headroom above it");
    }

    #[test]
    fn ordinary_speech_lands_in_the_middle_of_the_bar() {
        // The meter exists to tell the user the microphone is picking them up.
        // A bar that sits near zero while they talk normally fails at that,
        // and so does one that is already pinned.
        let speech: Vec<f32> = (0..128)
            .map(|i| if i % 2 == 0 { 0.1 } else { -0.1 })
            .collect();
        let reading = level(&speech);
        assert!((0.25..0.75).contains(&reading), "read {reading}");
    }

    #[test]
    fn the_meter_never_leaves_its_range() {
        let over: Vec<f32> = vec![9.0; 64];
        assert!((0.0..=1.0).contains(&level(&over)));
    }

    #[test]
    fn a_read_size_is_a_whole_number_of_frames() {
        assert_eq!(READ_BYTES % 2, 0);
    }
}
