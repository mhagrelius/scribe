#!/usr/bin/env bash
# Build Scribe and install it, along with its GNOME Shell extension, under
# $PREFIX (~/.local by default).
#
#   ./install.sh              app and extension
#   ./install.sh --app-only   skip the shell extension
set -euo pipefail
cd "$(dirname "$0")"

PREFIX="${PREFIX:-$HOME/.local}"
APP_ID="us.hagreli.Scribe"
EXT_UUID="scribe@hagreli.us"
EXT_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/$EXT_UUID"
APP_ONLY=""
[ "${1:-}" = "--app-only" ] && APP_ONLY=1

echo "Building…"
cargo build --release --locked

echo "Installing into $PREFIX…"
install -Dm755 target/release/scribe                                       "$PREFIX/bin/scribe"
install -Dm644 "data/$APP_ID.desktop"                                     "$PREFIX/share/applications/$APP_ID.desktop"
install -Dm644 "data/$APP_ID.metainfo.xml"                                "$PREFIX/share/metainfo/$APP_ID.metainfo.xml"
install -Dm644 "data/icons/hicolor/scalable/apps/$APP_ID.svg"             "$PREFIX/share/icons/hicolor/scalable/apps/$APP_ID.svg"
install -Dm644 "data/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"    "$PREFIX/share/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg"

# The desktop file is DBusActivatable, so the session bus needs to know how to
# start Scribe when the shell asks for it.
install -d "$PREFIX/share/dbus-1/services"
cat > "$PREFIX/share/dbus-1/services/$APP_ID.service" <<EOF
[D-BUS Service]
Name=$APP_ID
Exec=$PREFIX/bin/scribe --gapplication-service
EOF

command -v gtk4-update-icon-cache >/dev/null && gtk4-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" || true
command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true

case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "Note: $PREFIX/bin is not on your PATH." ;;
esac

# ---- the shell extension --------------------------------------------------
if [ -n "$APP_ONLY" ]; then
  echo "Skipping the shell extension (--app-only)"
else
  echo "Installing the GNOME Shell extension"
  for f in extension.js interface.js metadata.json; do
    install -Dm644 "extension/$f" "$EXT_DIR/$f"
  done
  if command -v gnome-extensions >/dev/null; then
    gnome-extensions enable "$EXT_UUID" 2>/dev/null \
      || echo "  could not enable it automatically; run: gnome-extensions enable $EXT_UUID"
  fi
fi

cat <<EOF

Installed. Open Scribe once to download the speech model and set the shortcut;
it registers Super+Alt+D with GNOME by default.

Your settings live in  ~/.config/scribe/config.json
The speech models live in  ~/.local/share/scribe/models
Neither is touched by ./uninstall.sh.

The shell extension is what types your transcript into other windows. It
needs no permission, because it runs inside the compositor. If Scribe says
it is using the RemoteDesktop portal instead, the extension has not loaded
yet — on Wayland a newly installed extension is picked up at the next login.

  gnome-extensions info $EXT_UUID
EOF
