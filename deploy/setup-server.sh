#!/usr/bin/env bash
# One-time setup of a Linux server (Debian, Ubuntu or similar with systemd)
# for automatic deployments from GitHub Actions. Run as root:
#
#   bash setup-server.sh "<öffentlicher Deploy-Schlüssel>" [vorhandener Datenordner]
#
# Creates the user `ctxremote`, which runs the server and receives deployments
# over SSH, the folders /opt/ctxremote (program) and /var/lib/ctxremote (data:
# device IDs, updates), the systemd unit, and a sudo rule that lets the
# deployment restart exactly this one service. An existing data folder (with
# devices.json) is copied over so all device IDs stay valid. See docs/DEPLOY.md.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Bitte als root ausführen." >&2
  exit 1
fi
if [ $# -lt 1 ] || [[ "$1" != ssh-* ]]; then
  echo "Aufruf: bash setup-server.sh \"ssh-ed25519 AAAA… ctxremote-deploy\" [alter Datenordner]" >&2
  exit 1
fi
deploy_key="$1"
old_data="${2:-}"
here="$(cd "$(dirname "$0")" && pwd)"

id ctxremote >/dev/null 2>&1 || useradd --system --create-home --home-dir /home/ctxremote --shell /bin/bash ctxremote
install -d -o ctxremote -g ctxremote -m 755 /opt/ctxremote
install -d -o ctxremote -g ctxremote -m 750 /var/lib/ctxremote /var/lib/ctxremote/updates

if [ -n "$old_data" ]; then
  if [ -f "$old_data/devices.json" ] && [ ! -f /var/lib/ctxremote/devices.json ]; then
    install -o ctxremote -g ctxremote -m 640 "$old_data/devices.json" /var/lib/ctxremote/devices.json
    echo "Geräte-IDs aus $old_data übernommen."
  else
    echo "Hinweis: $old_data/devices.json nicht übernommen (fehlt, oder es gibt schon eine)." >&2
  fi
fi

install -d -o ctxremote -g ctxremote -m 700 /home/ctxremote/.ssh
keys=/home/ctxremote/.ssh/authorized_keys
touch "$keys"
grep -qxF "$deploy_key" "$keys" || echo "$deploy_key" >> "$keys"
chown ctxremote:ctxremote "$keys"
chmod 600 "$keys"

install -m 644 "$here/ctxremote-server.service" /etc/systemd/system/ctxremote-server.service
cat > /etc/sudoers.d/ctxremote-deploy <<'SUDO'
# Lets the GitHub deployment restart the server, nothing else.
ctxremote ALL=(root) NOPASSWD: /usr/bin/systemctl restart ctxremote-server, /usr/bin/systemctl is-active ctxremote-server
SUDO
chmod 440 /etc/sudoers.d/ctxremote-deploy
visudo -cf /etc/sudoers.d/ctxremote-deploy >/dev/null

systemctl daemon-reload
systemctl enable ctxremote-server >/dev/null
if [ -x /opt/ctxremote/ctxremote-server ]; then
  systemctl restart ctxremote-server
  echo "Server läuft."
else
  echo "Eingerichtet. Der Server startet mit der ersten Auslieferung aus GitHub Actions."
fi
echo "Firewall: TCP 21300 eingehend freigeben (z. B. ufw allow 21300/tcp)."
