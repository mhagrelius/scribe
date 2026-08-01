#!/usr/bin/env bash
# The gate: formatting, lints, tests. What CI would run, if there were CI.
set -euo pipefail
cd "$(dirname "$0")"

export GTK_A11Y=none
export GSETTINGS_BACKEND=memory
export RUST_BACKTRACE=1

HEADLESS=""
if [[ "${1:-}" == "--headless" ]]; then
  HEADLESS="xvfb-run -a dbus-run-session --"
fi

echo "==> cargo fmt"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --all-targets -- -D warnings

echo "==> cargo test"
$HEADLESS cargo test --all-targets

if command -v gjs >/dev/null; then
  echo "==> shell extension"
  gjs -m extension/test.js
else
  echo "==> shell extension (skipped: no gjs)"
fi

echo "All green."
