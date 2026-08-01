# Mynah — design

## Scope

Dictation for GNOME. A global shortcut, speech recognised on this machine, and
the text delivered into whatever window has focus. A vocabulary the user
controls, spoken numbers written as digits, and an optional tidying pass
through a local language model.

Out of scope: transcribing files, translation, speaker diarization, a
cloud fallback, anything that sends audio off the machine.

## What it does

1. The user presses the shortcut. GNOME runs `mynah --toggle`, which is handed
   to the already-running instance.
2. `pw-record` starts and a small overlay appears with a level meter.
3. The user presses the shortcut again.
4. **Accurate** mode transcribes the whole utterance at once. **Live** mode has
   been transcribing 560 ms chunks all along and simply stops.
5. The transcript is optionally sent to a local language model to have the
   "um"s taken out.
6. Spoken numbers become digits, then the user's vocabulary rules are applied.
7. The result is typed into the focused window through the RemoteDesktop
   portal, or copied to the clipboard.

## Architecture

`model/` has no GTK types and needs no display: the config, the vocabulary
rules, the spelled-number rewriting, and the file they live in. All of it is
tested directly.

`ui/` is the boundaries — windows, microphone, speech models, portals, HTTP —
driven from the GLib main loop.

`MynahApplication` owns the `Store` and is the only thing that writes it.
Widgets emit intent (`settings-changed`, `download-requested`) and are pushed
state back by method call.

### Threading

One worker thread, for the speech model. Inference takes about a fifth of a
second, which is six dropped frames on the main loop, and the model costs most
of a second to load so it stays resident between utterances. It is spoken to
over an `mpsc` channel and answers with `glib::idle_add_once`.

Everything else is main-loop async: `gio` for the microphone pipe, the model
downloads and D-Bus; `soup3` for HTTP. No tokio, no ashpd, no `async-channel`.

### Text pipeline

Order matters. Numbers are rewritten first, because Parakeet writes "twenty
twenty six" and a user's rule is far likelier to be written against `2026`.
Vocabulary substitution second, longest rule first so "code review" wins over
"code". Trimming last.

## Testing

| Layer | How |
| --- | --- |
| `model/**` | inline unit tests; no display |
| `portal.rs` | GVariant signatures asserted directly — a `Variant` needs no display |
| `inject.rs` | character to keysym, including the Latin-1 boundary |
| `cleanup.rs` | reply parsing, and the guard that rejects an answer |
| `tests/widgets.rs` | one `#[test]`, a table of cases, because GTK is thread-affine |
| `tests/session.rs` | settings across restarts, corruption, version skew |

## Dependencies

- `gtk4` / `libadwaita` — the platform
- `gio` — D-Bus, subprocesses, async I/O, at feature level 2.80
- `parakeet-rs` — both speech models over one ONNX Runtime
- `soup3` — HTTP, because it ships in `org.gnome.Sdk`
- `serde` / `serde_json` — the config file

## Milestones

1. Speech engine spike — Parakeet TDT v3 int8 on CPU ✓
2. Portal spike — GVariant marshalling, session lifecycle ✓
3. Model layer, config, vocabulary, numbers ✓
4. Recorder, engine worker, cleanup, downloader ✓
5. Window, overlay, application ✓
6. End to end: shortcut → speech → text ✓

## Built differently, or not built

**Push-to-talk was designed for and dropped.** The plan was the
`GlobalShortcuts` portal, which delivers key press *and* release. Measured on
GNOME 50 / xdg-desktop-portal 1.21.1, `CreateSession` returns `NotAllowed: An
app id is required`. Since portal 1.21 an application identity is mandatory,
and the mechanism a non-Flatpak app uses to declare one,
`org.freedesktop.host.portal.Registry`, is not exported by this portal build —
the bus name is not owned by anything. Launching under a systemd scope named
`app-us.hagreli.Mynah-N.scope` was tested too, in three naming variants, and
changes nothing.

So the shortcut is a gnome-settings-daemon custom keybinding, which is
press-only, and dictation is a toggle. Reading `/dev/input` directly would
restore push-to-talk at the cost of putting the user in the `input` group,
which hands a keylogger to every process they run. Not worth one key.

**The clipboard does not go through GDK.** It did, and it silently failed:
taking the Wayland clipboard needs a serial from a recent input event on one of
our own surfaces, and Mynah by design has no focused window when a dictation
ends. `wl-copy` uses the data-control protocol, which does not need focus. GDK
is kept only as a fallback for when `wl-copy` is absent.

**No contextual biasing.** The plan was a hotwords file, which sherpa-onnx
supports and `parakeet-rs` does not. Sherpa's biasing needs
`modified_beam_search`, which has an open hallucination bug against
parakeet-tdt-v3. Vocabulary is applied to the finished transcript instead:
less clever, more predictable, and it survives a model change.

**No CUDA.** The 5090 is right there, and `ort`'s CUDA execution provider needs
a CUDA 12 runtime and cuDNN 9 that this machine does not have. Parakeet int8 on
32 CPU cores runs at about 48× real time — a ten-second dictation transcribes
in a fifth of a second. Installing a toolchain to make an already-imperceptible
wait shorter is not a trade worth making. The `cuda` feature is one line in
`Cargo.toml` if that ever changes.

**No Meson.** Cargo plus `install.sh`, as in the sibling apps.

## Settled

- Application id `us.hagreli.Mynah`, binary `mynah`
- Config at `$XDG_CONFIG_HOME/mynah/config.json`; models under
  `$XDG_DATA_HOME/mynah/models`
- Ctrl+Alt+D by default
- Models are downloaded on request, never bundled
