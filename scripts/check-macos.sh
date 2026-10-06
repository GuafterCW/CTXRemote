#!/usr/bin/env bash
# Type-checks the macOS code from Linux (e.g. in a Claude Code cloud session).
#
# Build scripts of C dependencies (ring, openh264, opus) want Apple's clang and
# SDK. Checking never links, so a stand-in compiler that only creates the
# expected output files is enough. Not a build: CI on macOS is the real test.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fake="$root/target/fake-apple"
mkdir -p "$fake"

cat > "$fake/apple-cc" <<'SH'
#!/usr/bin/env bash
out=""; prev=""
for a in "$@"; do
  [ "$prev" = "-o" ] && out="$a"
  case "$a" in --version|-v) echo "Apple clang version 16.0.0"; exit 0;; esac
  prev="$a"
done
[ -n "$out" ] && touch "$out"
exit 0
SH
cat > "$fake/apple-ar" <<'SH'
#!/usr/bin/env bash
# ar <flags> <archive> <objects...>: create the archive.
[ -n "${2:-}" ] && touch "$2"
exit 0
SH
chmod +x "$fake/apple-cc" "$fake/apple-ar"

target="${CTXREMOTE_MAC_TARGET:-aarch64-apple-darwin}"
rustup target add "$target" >/dev/null
[ -d "$root/app/dist" ] || (cd "$root/app" && npm ci && npm run build)

var="$(echo "$target" | tr - _)"
export "CC_$var=$fake/apple-cc" "CXX_$var=$fake/apple-cc" "AR_$var=$fake/apple-ar"
cd "$root"
cargo check --target "$target" -p ctxremote-proto -p ctxremote-core -p ctxremote "$@"
