#!/usr/bin/env bash
# Build Scribe and install it under $PREFIX (~/.local by default).
set -euo pipefail
cd "$(dirname "$0")"

PREFIX="${PREFIX:-$HOME/.local}"
APP_ID="us.hagreli.Scribe"

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

cat <<EOF

Installed. Open Scribe once to download the speech model and set the shortcut;
it registers Super+Alt+D with GNOME by default.

Your settings live in  ~/.config/scribe/config.json
The speech models live in  ~/.local/share/scribe/models
Neither is touched by ./uninstall.sh.
EOF
