//! Getting the transcript into somebody else's window.
//!
//! Wayland gives a client no way to type into a window it does not own, which
//! is the entire difficulty of a dictation app on this desktop. The three
//! usual answers are `wtype`, which needs a virtual-keyboard protocol Mutter
//! has said it will not implement; `ydotool`, which needs a uinput device, a
//! udev rule, a group change and a daemon; and the RemoteDesktop portal, which
//! needs one consent dialog and nothing else. Scribe uses the portal.
//!
//! The session is created once and kept. `persist_mode` 2 asks the portal to
//! remember the grant until the user revokes it, and `Start` hands back a
//! `restore_token` that skips the dialog next time. The token is single-use:
//! each successful start returns a fresh one, so it is rewritten every time.
//!
//! Better still is the companion shell extension, which runs inside the
//! compositor and so needs no permission at all. When it is loaded it is used
//! in preference to the portal, and the portal remains for people who would
//! rather not install an extension.
//!
//! When both are unavailable the transcript still goes to the clipboard, which
//! needs no permission either and is the floor this app will not drop below.

use gio::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::{portal, shell};

const INTERFACE: &str = "org.freedesktop.portal.RemoteDesktop";

/// Keyboard device type, as the portal's `SelectDevices` numbers them.
const KEYBOARD: u32 = 1;
/// Persist until the user explicitly revokes the grant.
const PERSIST_UNTIL_REVOKED: u32 = 2;

/// What became of an attempt to deliver a transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivered {
    /// Typed into the focused window.
    Typed,
    /// Put on the clipboard, because that is what the user asked for or the
    /// only thing left after consent was refused.
    Copied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Starting,
    Ready,
    /// Consent was refused. Not retried for the rest of the run: asking again
    /// on every utterance would be its own kind of broken.
    Refused,
}

/// A transcript waiting on the portal, and the caller waiting to be told.
struct Queued {
    text: String,
    done: Box<dyn Fn(Delivered)>,
    /// How many sessions this text has already been tried against. A closed
    /// session earns one retry; a second failure means something else is
    /// wrong and the clipboard is the honest answer.
    attempts: u8,
}

/// Owns the portal session and hands out keystrokes.
pub struct Typist {
    connection: Option<gio::DBusConnection>,
    session: RefCell<Option<String>>,
    state: Cell<State>,
    /// Work waiting for the session to finish starting.
    pending: RefCell<Vec<Queued>>,
    /// Called when GNOME is about to ask the user for permission.
    on_prompt: RefCell<Option<Box<dyn Fn()>>>,
    /// Watches for GNOME closing the session out from under us.
    closed: RefCell<Option<gio::SignalSubscription>>,
}

impl Default for Typist {
    fn default() -> Self {
        Self::new()
    }
}

impl Typist {
    pub fn new() -> Self {
        Self {
            connection: gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok(),
            session: RefCell::new(None),
            state: Cell::new(State::Idle),
            pending: RefCell::new(Vec::new()),
            on_prompt: RefCell::new(None),
            closed: RefCell::new(None),
        }
    }

    /// Whether typing is still worth attempting.
    pub fn is_available(&self) -> bool {
        self.connection.is_some() && self.state.get() != State::Refused
    }

    /// Whether permission has actually been granted, rather than merely not
    /// refused yet.
    pub fn is_ready(&self) -> bool {
        self.state.get() == State::Ready || self.via_extension()
    }

    /// Whether the shell extension is loaded and will take the text.
    ///
    /// When it is, none of the portal machinery below runs at all: no session,
    /// no consent, nothing to be closed out from under us.
    pub fn via_extension(&self) -> bool {
        self.connection.as_ref().is_some_and(shell::is_available)
    }

    /// Whether the user has turned Scribe down this run.
    pub fn was_refused(&self) -> bool {
        self.state.get() == State::Refused
    }

    /// Say what to do when the consent dialog is about to appear.
    ///
    /// A dialog that arrives unannounced while the user is looking at another
    /// window reads as noise and gets dismissed, which is exactly what
    /// happened the first time this shipped.
    pub fn set_on_prompt(&self, notify: impl Fn() + 'static) {
        *self.on_prompt.borrow_mut() = Some(Box::new(notify));
    }

    /// Let a refusal be reconsidered, so the settings window can offer a way
    /// back without restarting Scribe.
    pub fn allow_retry(&self) {
        if self.state.get() == State::Refused {
            self.state.set(State::Idle);
        }
    }

    /// Ask for permission now, rather than in the middle of a dictation.
    pub fn request_permission(self: &Rc<Self>) {
        self.allow_retry();
        self.ensure_session();
    }

    /// Open the session if it is not already open, without reopening a
    /// question the user has answered.
    ///
    /// Called when dictation starts rather than when the transcript is ready,
    /// so a session GNOME closed since last time is found and replaced during
    /// the seconds the user spends talking, instead of failing afterwards.
    pub fn ensure_session(self: &Rc<Self>) {
        if self.state.get() != State::Idle {
            return;
        }
        // Nothing to open. Asking for remote-desktop permission when the
        // extension is already able to do the job would be a dialog with no
        // purpose.
        if self.via_extension() {
            return;
        }
        if let Some(connection) = self.connection.clone() {
            self.start(connection);
        }
    }

    fn token_path() -> std::path::PathBuf {
        glib::user_data_dir()
            .join("scribe")
            .join("remote-desktop.token")
    }

    fn saved_token() -> Option<String> {
        let text = std::fs::read_to_string(Self::token_path()).ok()?;
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    fn save_token(token: &str) {
        let path = Self::token_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, token);
    }

    /// Deliver `text`, calling `done` with what actually happened.
    ///
    /// If the session is not up yet this starts it and queues the text, so the
    /// first dictation of a session is not lost to the consent dialog.
    pub fn deliver(self: &Rc<Self>, text: &str, done: impl Fn(Delivered) + 'static) {
        if text.is_empty() {
            done(Delivered::Typed);
            return;
        }
        let Some(connection) = self.connection.clone() else {
            copy_to_clipboard(text);
            done(Delivered::Copied);
            return;
        };

        // The extension first, every time. It is cheaper than the portal, it
        // cannot be closed by the compositor, and it needs no permission.
        match shell::insert(&connection, text) {
            Ok(()) => {
                done(Delivered::Typed);
                return;
            }
            Err(shell::ShellError::NotRunning) => {}
            Err(error) => eprintln!("scribe: {error}"),
        }

        match self.state.get() {
            State::Ready => {
                let session = self.session.borrow().clone();
                match session {
                    Some(session) => match type_text(&connection, &session, text) {
                        Ok(()) => done(Delivered::Typed),
                        Err(error) => {
                            // GNOME closes remote-desktop sessions on its own
                            // schedule. A handle that worked an hour ago is
                            // simply gone, and before this the app went on
                            // using it and failing every dictation for the
                            // rest of the run.
                            eprintln!("scribe: the typing session ended ({error}); reopening");
                            self.invalidate();
                            self.queue_with(text, Box::new(done), 1);
                            self.start(connection);
                        }
                    },
                    None => {
                        copy_to_clipboard(text);
                        done(Delivered::Copied);
                    }
                }
            }
            State::Refused => {
                copy_to_clipboard(text);
                done(Delivered::Copied);
            }
            // Nothing is reported yet. Whether this ends up typed or copied is
            // not known until the user has answered the portal, and claiming
            // "typed" here is precisely how a refusal became silent: the text
            // went to the clipboard and nobody was told.
            State::Starting => self.queue(text, done),
            State::Idle => {
                self.queue(text, done);
                self.start(connection);
            }
        }
    }

    fn queue(&self, text: &str, done: impl Fn(Delivered) + 'static) {
        self.queue_with(text, Box::new(done), 0);
    }

    fn queue_with(&self, text: &str, done: Box<dyn Fn(Delivered)>, attempts: u8) {
        self.pending.borrow_mut().push(Queued {
            text: text.to_string(),
            done,
            attempts,
        });
    }

    /// Notice when GNOME ends the session, rather than finding out by failing.
    ///
    /// The portal emits `Closed` on the session object. Without this the
    /// Typist sits in `Ready` holding a handle the compositor has forgotten,
    /// and every dictation from then on dies with `Invalid session`.
    fn watch_for_close(self: &Rc<Self>, connection: &gio::DBusConnection, session: &str) {
        let this = self.clone();
        let watch = connection.subscribe_to_signal(
            Some(portal::NAME),
            Some("org.freedesktop.portal.Session"),
            Some("Closed"),
            Some(session),
            None,
            gio::DBusSignalFlags::NONE,
            move |_| {
                this.invalidate();
            },
        );
        *self.closed.borrow_mut() = Some(watch);
    }

    /// The session handle is no longer good. Ask for another one next time
    /// rather than failing every dictation from here on.
    fn invalidate(&self) {
        *self.session.borrow_mut() = None;
        let _ = self.closed.borrow_mut().take();
        if self.state.get() == State::Ready {
            self.state.set(State::Idle);
        }
    }

    /// Take everything waiting. The borrow is released before any callback
    /// runs, because a callback may start the next dictation and come back
    /// through `deliver`.
    fn take_pending(&self) -> Vec<Queued> {
        self.pending.borrow_mut().drain(..).collect()
    }

    /// Bring the portal session up, asking for consent if there is no token.
    pub fn start(self: &Rc<Self>, connection: gio::DBusConnection) {
        if self.state.get() != State::Idle {
            return;
        }
        self.state.set(State::Starting);

        // Only the very first attempt puts a dialog on screen; a saved token
        // makes the rest silent, so there is nothing to announce then.
        if Self::saved_token().is_none() {
            if let Some(notify) = self.on_prompt.borrow().as_ref() {
                notify();
            }
        }

        let this = self.clone();
        portal::request(
            &connection.clone(),
            INTERFACE,
            "CreateSession",
            |handle_token| portal::tup(vec![portal::session_options(handle_token, "scriberd")]),
            move |code, results| {
                if code != portal::SUCCESS {
                    this.give_up();
                    return;
                }
                let Some(session) = portal::handle(&results, "session_handle") else {
                    this.give_up();
                    return;
                };
                *this.session.borrow_mut() = Some(session.clone());
                this.select_devices(connection.clone(), session);
            },
        );
    }

    fn select_devices(self: &Rc<Self>, connection: gio::DBusConnection, session: String) {
        let this = self.clone();
        let for_start = session.clone();
        portal::request(
            &connection.clone(),
            INTERFACE,
            "SelectDevices",
            move |handle_token| {
                let mut options = vec![
                    ("handle_token", handle_token.to_variant()),
                    ("types", KEYBOARD.to_variant()),
                    ("persist_mode", PERSIST_UNTIL_REVOKED.to_variant()),
                ];
                let saved = Self::saved_token();
                if let Some(token) = saved.as_deref() {
                    options.push(("restore_token", token.to_variant()));
                }
                portal::tup(vec![portal::opath(&session), portal::vdict(options)])
            },
            move |code, _| {
                if code != portal::SUCCESS {
                    this.give_up();
                    return;
                }
                this.start_session(connection.clone(), for_start.clone());
            },
        );
    }

    fn start_session(self: &Rc<Self>, connection: gio::DBusConnection, session: String) {
        let this = self.clone();
        let for_flush = session.clone();
        portal::request(
            &connection.clone(),
            INTERFACE,
            "Start",
            move |handle_token| {
                portal::tup(vec![
                    portal::opath(&session),
                    "".to_variant(),
                    portal::vdict(vec![("handle_token", handle_token.to_variant())]),
                ])
            },
            move |code, results| {
                if code != portal::SUCCESS {
                    this.give_up();
                    return;
                }
                if let Some(token) = portal::get::<String>(&results, "restore_token") {
                    Self::save_token(&token);
                }
                this.state.set(State::Ready);
                this.watch_for_close(&connection, &for_flush);

                // The compositor drops input sent immediately after Start, so
                // the first character of the first dictation goes missing
                // without this pause.
                let this = this.clone();
                let connection = connection.clone();
                let session = for_flush.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                    for queued in this.take_pending() {
                        match type_text(&connection, &session, &queued.text) {
                            Ok(()) => (queued.done)(Delivered::Typed),
                            Err(error) if queued.attempts < 1 => {
                                eprintln!("scribe: typing failed ({error}); trying once more");
                                this.invalidate();
                                this.queue_with(&queued.text, queued.done, queued.attempts + 1);
                                this.start(connection.clone());
                            }
                            Err(error) => {
                                eprintln!(
                                    "scribe: could not type into the focused window: {error}"
                                );
                                copy_to_clipboard(&queued.text);
                                (queued.done)(Delivered::Copied);
                            }
                        }
                    }
                });
            },
        );
    }

    /// Consent was refused or the session died. Everything queued goes to the
    /// clipboard, and every caller is told so.
    fn give_up(&self) {
        self.state.set(State::Refused);
        *self.session.borrow_mut() = None;
        for queued in self.take_pending() {
            copy_to_clipboard(&queued.text);
            (queued.done)(Delivered::Copied);
        }
    }
}

/// The keysym for a character.
///
/// Latin-1 maps onto keysyms one for one; everything above it uses the Unicode
/// range the X11 keysym registry set aside for exactly this. Going through
/// keysyms rather than keycodes is what makes this layout-independent: the
/// compositor resolves the symbol against whatever layout is active, so an
/// accented character does not arrive stripped the way `ydotool` delivers it.
pub fn keysym_for(ch: char) -> u32 {
    match ch {
        '\n' => 0xff0d, // Return
        '\r' => 0xff0d,
        '\t' => 0xff09, // Tab
        '\u{8}' => 0xff08,
        c if (c as u32) < 0x100 => c as u32,
        c => 0x0100_0000 + c as u32,
    }
}

/// Type `text` through the portal, or say why it could not.
///
/// The failure that matters is `AccessDenied: Invalid session`: GNOME closes a
/// remote-desktop session on its own schedule, and a session handle that
/// worked ten minutes ago is simply gone. The caller re-creates and retries
/// rather than treating this as the end of the road.
fn type_text(
    connection: &gio::DBusConnection,
    session: &str,
    text: &str,
) -> Result<(), glib::Error> {
    for ch in text.chars() {
        let keysym = keysym_for(ch) as i32;
        for state in [1u32, 0u32] {
            connection.call_sync(
                Some(portal::NAME),
                portal::PATH,
                INTERFACE,
                "NotifyKeyboardKeysym",
                Some(&portal::tup(vec![
                    portal::opath(session),
                    portal::vdict(vec![]),
                    keysym.to_variant(),
                    state.to_variant(),
                ])),
                None,
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
            )?;
        }
    }
    Ok(())
}

/// Put text on the clipboard.
///
/// `wl-copy` first, and not as a fallback. Taking the Wayland clipboard
/// through GDK needs a serial from a recent input event on one of our own
/// surfaces, and Scribe by design has no focused window when a dictation ends —
/// the user is typing in something else. The call succeeds and the clipboard
/// does not change. `wl-copy` goes through the data-control protocol, which
/// exists for exactly this case and does not need focus.
///
/// GDK is still tried when `wl-copy` is missing, because on X11 and in a
/// window that does happen to be focused it works.
pub fn copy_to_clipboard(text: &str) {
    if glib::find_program_in_path("wl-copy").is_some() {
        let spawned = gio::Subprocess::newv(
            &[std::ffi::OsStr::new("wl-copy"), std::ffi::OsStr::new("--")],
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
        );
        match spawned {
            Ok(process) => {
                // wl-copy holds the selection until it is replaced, so it
                // deliberately outlives this call.
                process.communicate_utf8_async(
                    Some(text.to_string()),
                    gio::Cancellable::NONE,
                    |result| {
                        if let Err(error) = result {
                            eprintln!("scribe: wl-copy failed: {error}");
                        }
                    },
                );
                return;
            }
            Err(error) => eprintln!("scribe: wl-copy could not be started: {error}"),
        }
    }
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::keysym_for;

    #[test]
    fn ascii_maps_to_itself() {
        assert_eq!(keysym_for('h'), 0x68);
        assert_eq!(keysym_for(' '), 0x20);
        assert_eq!(keysym_for('~'), 0x7e);
    }

    #[test]
    fn latin_one_is_still_direct() {
        assert_eq!(keysym_for('é'), 0xe9);
        assert_eq!(keysym_for('ÿ'), 0xff);
    }

    #[test]
    fn beyond_latin_one_uses_the_unicode_range() {
        // The boundary is the whole subtlety here: 0x100 is the first
        // character that has to be offset rather than sent as itself.
        assert_eq!(keysym_for('Ā'), 0x0100_0100);
        assert_eq!(keysym_for('€'), 0x0100_20ac);
        assert_eq!(keysym_for('😀'), 0x0101_f600);
    }

    #[test]
    fn newline_is_return_not_a_literal() {
        assert_eq!(keysym_for('\n'), 0xff0d);
        assert_eq!(keysym_for('\t'), 0xff09);
    }
}
