#!/usr/bin/env bash
# Type-checks the Windows code from Linux (e.g. in a Claude Code cloud session).
#
# `cargo check` for x86_64-pc-windows-msvc runs openh264's build script, which
# wants MSVC's cl.exe and lib.exe. Checking never links, so stand-ins that only
# create the expected output files are enough. Not a build: CI on Windows is
# still the real test.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
fake="$root/target/fake-msvc"
mkdir -p "$fake"

cat > "$fake/cl.exe" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do case "$a" in -Fo*|/Fo*) touch "${a:3}";; esac; done
EOF
cat > "$fake/lib.exe" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do
  case "$(echo "$a" | tr A-Z a-z)" in -out:*|/out:*) touch "${a:5}";; esac
done
EOF
chmod +x "$fake/cl.exe" "$fake/lib.exe"

rustup target add x86_64-pc-windows-msvc >/dev/null
# Tauri expects the service sidecar to exist; an empty file satisfies the check.
mkdir -p "$root/app/src-tauri/binaries"
sidecar="$root/app/src-tauri/binaries/ctxremote-service-x86_64-pc-windows-msvc.exe"
[ -e "$sidecar" ] || touch "$sidecar"
# The app embeds the frontend.
[ -d "$root/app/dist" ] || (cd "$root/app" && npm ci && npm run build)

export PATH="$fake:$PATH"
export CC_x86_64_pc_windows_msvc=cl.exe CXX_x86_64_pc_windows_msvc=cl.exe AR_x86_64_pc_windows_msvc=lib.exe
cd "$root"
cargo check --target x86_64-pc-windows-msvc -p ctxremote-core -p ctxremote-service -p ctxremote "$@"
CTXREMOTE_QUICK_SERVER=check.invalid:21300 \
  cargo check --target x86_64-pc-windows-msvc -p ctxremote --features quick "$@"
