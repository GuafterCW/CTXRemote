#!/bin/bash
# Prepares a Claude Code cloud session: system libraries for the Tauri app,
# frontend packages, the Windows target for scripts/check-windows.sh, and
# the crates, so builds and tests work right away. Idempotent.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

cd "$CLAUDE_PROJECT_DIR"

# Tauri needs WebKitGTK and friends to build the app on Linux (same list as CI).
if ! pkg-config --exists webkit2gtk-4.1 2>/dev/null; then
  SUDO=""
  [ "$(id -u)" -ne 0 ] && SUDO="sudo -n"
  $SUDO apt-get update -q >/dev/null
  DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y -q \
    libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev >/dev/null
fi

# Frontend packages. `npm ci` keeps package-lock.json as committed (`npm install`
# rewrites it with this npm version); skipped while node_modules is current.
if [ ! -f app/node_modules/.package-lock.json ] || [ app/package-lock.json -nt app/node_modules/.package-lock.json ]; then
  (cd app && npm ci --no-audit --no-fund --loglevel=error)
fi
# The app embeds app/dist, so build it once if missing.
[ -d app/dist ] || (cd app && npm run build >/dev/null)

# Type-checking the Windows code from Linux (scripts/check-windows.sh).
rustup target add x86_64-pc-windows-msvc >/dev/null 2>&1 || true

# Download all crates now, so the first build does not wait for the network.
cargo fetch --locked >/dev/null 2>&1 || cargo fetch >/dev/null
