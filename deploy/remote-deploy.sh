#!/usr/bin/env bash
# Runs on the server as user `ctxremote`, called by the release workflow after
# it uploaded the files to ~/incoming. Swaps the server binary (keeping the
# previous one for a rollback), restarts the service and checks it comes up;
# then publishes the client update, installer before manifest.
set -euo pipefail

incoming="$HOME/incoming"
bin=/opt/ctxremote/ctxremote-server

if [ -f "$incoming/ctxremote-server" ]; then
  install -m 755 "$incoming/ctxremote-server" "$bin.new"
  [ -f "$bin" ] && cp -p "$bin" "$bin.previous"
  mv -f "$bin.new" "$bin"
  sudo /usr/bin/systemctl restart ctxremote-server
  sleep 3
  if ! sudo /usr/bin/systemctl is-active ctxremote-server >/dev/null \
     || ! (exec 3<>/dev/tcp/127.0.0.1/21300) 2>/dev/null; then
    echo "Neuer Server startet nicht, vorherige Version wird wiederhergestellt." >&2
    if [ -f "$bin.previous" ]; then
      mv -f "$bin.previous" "$bin"
      sudo /usr/bin/systemctl restart ctxremote-server
    fi
    exit 1
  fi
  echo "Server aktualisiert."
fi

if [ -d "$incoming/updates" ]; then
  updates=/var/lib/ctxremote/updates
  # Installers first, manifests last: clients never see a release without its file.
  find "$incoming/updates" -maxdepth 1 -type f ! -name '*.json' -exec install -m 644 {} "$updates/" \;
  for manifest in "$incoming/updates"/*.json; do
    [ -e "$manifest" ] || continue
    install -m 644 "$manifest" "$updates/.$(basename "$manifest").tmp"
    mv -f "$updates/.$(basename "$manifest").tmp" "$updates/$(basename "$manifest")"
  done
  # Keep the three newest installers per platform.
  ls -1t "$updates"/CTXRemote-*-windows-x86_64.* 2>/dev/null | tail -n +4 | xargs -r rm -f
  echo "Client-Update veröffentlicht."
fi

rm -rf "$incoming"
