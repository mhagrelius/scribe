# Scribe

Speak, and it types.

Press a shortcut anywhere, say what you mean, press it again. The words appear
in whatever window you were already working in — a terminal, a browser, a
commit message. Speech recognition runs on this machine, so nothing you dictate
leaves it.

```
        ┌──────────────────────────────┐
   ⌨    │ ● Listening                  │
        │   Press the shortcut again   │
        │   ▁▃▅█▇▅▃▂▁▂▄▆█▇▅▃▁          │
        └──────────────────────────────┘
                     ↓
   git commit -m "drop the retry loop; the caller already retries"
```

## Features

- A global shortcut that works in every application, GNOME's own way — no
  `ydotool`, no udev rules, no group changes
- **Accurate** mode transcribes when you stop talking; **Live** mode shows the
  words as you say them
- A vocabulary of your own, for the names and jargon the model keeps mishearing
- Spoken numbers written as digits — "twenty twenty six" becomes `2026`
- An optional tidying pass through a local language model, to drop the "um"s
  and the false starts

## Install

```sh
./install.sh     # builds, installs under ~/.local, registers the shortcut
./uninstall.sh   # reverses it; leaves your settings and models alone
./test.sh        # the gate: fmt, clippy -D warnings, tests
```

Open Scribe once afterwards to download the speech model. It is about 670 MB and
lands in `~/.local/share/scribe/models`.

### Requirements

GTK 4.22 and libadwaita 1.9 (GNOME 49 or newer), PipeWire's `pw-record`
(`pipewire-bin` on Debian and Ubuntu, `pipewire-utils` on Fedora), and a Rust
toolchain of 1.80 or later.

The tidying pass wants an OpenAI-shaped endpoint on `127.0.0.1:8080` — a
`llama-server`, say. It is off by default and Scribe works without it.

## Using it

**Super+Alt+D** starts and stops dictation. Change it in the window; Scribe
registers it with GNOME so it keeps working after a reboot.

The first time Scribe types into another window, GNOME asks whether to allow it.
That is the RemoteDesktop portal, and saying yes once is the whole of the
setup — the grant is remembered until you revoke it in Settings → Privacy.
Saying no is fine too: Scribe falls back to putting the transcript on the
clipboard and telling you so.

### Vocabulary

The model spells unfamiliar names the way they sounded, and it does it
consistently, so a list of corrections fixes them for good. Put what it hears
on the left and what you meant on the right. Rules are applied to the finished
transcript, longest first, so a rule for "code review" wins over one for
"code".

### Accurate or Live

Accurate records first and transcribes the whole utterance at the end, which is
where the punctuation and the best accuracy come from. It is fast enough that
the wait is not really a wait — about a fifth of a second for ten seconds of
speech.

Live streams the audio to a cache-aware model that emits text every 560 ms, so
words appear while you are still talking. It cannot run the tidying pass, since
there is no finished sentence to tidy, and it is a little less accurate.

## How it works

```
src/
  model/            no GTK, no display, all of it tested directly
    config.rs       what the user can change
    numbers.rs      "twenty twenty six" → 2026
    vocabulary.rs   the user's own corrections
    store.rs        the JSON file: atomic write, corruption recovery
  ui/               everything that touches the desktop
    application.rs  owns the store; the only thing that writes it
    window.rs       settings
    overlay.rs      the small window with the level meter
    recorder.rs     pw-record, read off a pipe by the main loop
    engine.rs       the speech models, on a worker thread
    models.rs       downloading them
    cleanup.rs      the local language model
    inject.rs       typing into somebody else's window
    portal.rs       xdg-desktop-portal over gio D-Bus
    shortcut.rs     the GNOME keybinding
```

**The application owns the store.** Widgets emit intent — "settings changed",
"download this" — and are told what to show. Nothing else writes the file.

**The speech model lives on a worker thread** and is spoken to over a channel,
because a quarter of a second of inference on the main loop is six dropped
frames. It stays loaded between utterances; loading it costs most of a second.

**There is no async runtime.** GLib's main loop runs the futures, `gio` does
the I/O and the D-Bus, and the one genuinely CPU-bound thing gets a thread of
its own. No tokio, no ashpd.

### Why the shortcut is a toggle and not push-to-talk

The GlobalShortcuts portal hands out key press *and* release, which is exactly
what push-to-talk needs. It refuses to talk to Scribe. Since
xdg-desktop-portal 1.21 the interface requires an application identity, and the
mechanism a non-Flatpak app would use to declare one —
`org.freedesktop.host.portal.Registry` — is not exported by the portal on this
desktop. A systemd scope named after the app does not stand in for it; the call
still comes back `NotAllowed: An app id is required`.

So the shortcut is a GNOME custom keybinding instead, which is press-only.
Push-to-talk would mean reading the keyboard device directly, which means
putting you in the `input` group and handing every process you run a keylogger.
That is a bad trade for one key.

## Development

```sh
cargo run                                  # the app
XDG_CONFIG_HOME=/tmp/scratch cargo run     # with throwaway settings
cargo run -- --toggle                      # what the shortcut does
```

### Tests

| File | Covers |
| --- | --- |
| `src/model/**` | numbers, vocabulary, the config file — no display needed |
| `src/ui/portal.rs` | the GVariant signatures the portal accepts |
| `src/ui/inject.rs` | character to keysym |
| `src/ui/cleanup.rs` | parsing replies, and refusing ones that answered instead of tidying |
| `tests/widgets.rs` | the widgets, in one test, because GTK is thread-affine |
| `tests/session.rs` | settings surviving a restart |

## Licence

GPL-3.0-or-later.
