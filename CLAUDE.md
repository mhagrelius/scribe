# Scribe

Dictation for GNOME. A global shortcut records, a local speech model
transcribes, and the text is typed into whatever window has focus.

## Stack

Rust, GTK 4.22, libadwaita 1.9, gio at feature level 2.80. Speech through
`parakeet-rs` (ONNX Runtime); HTTP through `soup3`. Cargo only — no Meson, no
`.ui` files, no Blueprint, no GResource. Widget trees are built in Rust.

## Commands

```sh
./install.sh     # build and install under ~/.local, register the shortcut
./uninstall.sh   # reverse it; never touches settings or models
./test.sh        # the gate: fmt, clippy -D warnings, tests
cargo run        # the app
cargo run -- --toggle                      # what the shortcut does
XDG_CONFIG_HOME=/tmp/scratch cargo run     # throwaway settings
```

## Layout

```
src/model/   no GTK, no display, tested directly
src/ui/      windows, microphone, speech models, portals, HTTP
tests/       widgets.rs (one #[test]), session.rs (scenarios)
data/        desktop entry, metainfo, icons
```

## Conventions

- **The application owns the store.** `ScribeApplication` is the only thing that
  reads or writes `config.json`. Widgets emit intent signals and are told what
  to show; they never touch the file.
- **`model/` imports no GTK.** If something needs a display it belongs in
  `ui/`.
- **No async runtime.** GLib's main loop runs the futures. The speech model
  gets one worker thread because it is genuinely CPU-bound; results come back
  through `glib::idle_add_once`. Do not add tokio, ashpd or `async-channel`.
- **Errors are hand-written enums** with a `Display` that writes a full
  sentence aimed at the user, and a `source()`. No `anyhow`, no `thiserror`.
  Surface them as `adw::Toast` for events and `adw::Banner` for conditions —
  and as a `gio::Notification` when dictation failed with no window open,
  because that is the only place the user will see it.
- **Exceptions are not control flow.** Failure that is expected — consent
  refused, server down, model missing — is a typed value, and the dictation is
  never lost to it. Cleanup failing falls back to the raw transcript; typing
  failing falls back to the clipboard.
- **D-Bus argument tuples go through `portal::tup`.** A `glib::Variant` inside
  a Rust tuple is boxed as `v`, which produces the wrong signature and a flat
  rejection. There is a test asserting the wrong form is still wrong.

## Two things that were measured, not assumed

The `GlobalShortcuts` portal cannot be used: it requires an app id that a
non-Flatpak app has no way to declare on this portal build. The shortcut is a
gnome-settings-daemon keybinding, and dictation is therefore a toggle rather
than push-to-talk. Do not "fix" this by reaching for the portal again without
re-testing `CreateSession`.

The clipboard goes through `wl-copy`, not GDK, because a Wayland client with no
focused surface cannot take the selection — and Scribe never has focus when a
dictation ends. Both are written up in `DESIGN.md` under "Built differently, or
not built"; append there rather than editing the design when reality diverges.

The sibling apps (`magpie`, `brain`, `stickies`, `planner`, `llama-tray`) share
this layout and these scripts; a pattern established in one is the pattern
here. The `developing-gtk-apps` and `designing-gnome-ui` skills are the source
of truth for widget, threading and HIG decisions.
