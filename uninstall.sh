#!/usr/bin/env bash
# Remove what install.sh put down. User data is left alone.
set -euo pipefail
cd "$(dirname "$0")"

PREFIX="${PREFIX:-$HOME/.local}"
APP_ID="us.hagreli.Mynah"

# Take the keyboard shortcut back out of GNOME before the binary goes, so the
# key stops being bound to a command that no longer exists.
if command -v gsettings >/dev/null; then
  KEY=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/mynah/
  CURRENT=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings 2>/dev/null || echo "[]")
  if [[ "$CURRENT" == *"$KEY"* ]]; then
    UPDATED=$(python3 - "$CURRENT" "$KEY" <<'PY'
import ast, sys
paths = ast.literal_eval(sys.argv[1])
print(repr([p for p in paths if p != sys.argv[2]]))
PY
)
    gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "$UPDATED"
  fi
fi

rm -f "$PREFIX/bin/mynah" \
      "$PREFIX/share/applications/$APP_ID.desktop" \
      "$PREFIX/share/metainfo/$APP_ID.metainfo.xml" \
      "$PREFIX/share/icons/hicolor/scalable/apps/$APP_ID.svg" \
      "$PREFIX/share/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg" \
      "$PREFIX/share/dbus-1/services/$APP_ID.service"

command -v gtk4-update-icon-cache >/dev/null && gtk4-update-icon-cache -qtf "$PREFIX/share/icons/hicolor" || true
command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true

cat <<EOF
Removed.

Your settings and the downloaded speech models were left where they are.
To delete those too:

  rm -r ~/.config/mynah ~/.local/share/mynah
EOF
