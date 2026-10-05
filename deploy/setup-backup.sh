#!/usr/bin/env bash
# One-time setup of the daily backup (as root, from the folder with this
# script, ctxremote-backup.sh and the two unit files). Safe to run again.
# See docs/DEPLOY.md, "Sicherung".
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "Bitte als root ausführen" >&2; exit 1; }
cd "$(dirname "$0")"

install -m 755 ctxremote-backup.sh /opt/ctxremote/ctxremote-backup.sh
install -m 644 ctxremote-backup.service ctxremote-backup.timer /etc/systemd/system/
install -d -m 750 /etc/ctxremote

# The passphrase encrypts every backup. Without it a backup cannot be read,
# so it must also be kept outside this server (password manager).
if [ ! -f /etc/ctxremote/backup.key ]; then
    umask 077
    openssl rand -base64 32 > /etc/ctxremote/backup.key
    echo
    echo "Neue Passphrase für die Sicherungen (/etc/ctxremote/backup.key)."
    echo "Bitte JETZT im Passwortmanager ablegen, ohne sie ist keine Sicherung lesbar:"
    echo
    echo "    $(cat /etc/ctxremote/backup.key)"
    echo
fi

if [ ! -f /etc/ctxremote/backup.env ]; then
    cat > /etc/ctxremote/backup.env <<'ENV'
# Settings for ctxremote-backup.sh (docs/DEPLOY.md, "Sicherung").
#BACKUP_KEEP_DAYS=14
# Copy to a Hetzner Storage Box (or any SSH/rsync target):
#BACKUP_REMOTE=u123456@u123456.your-storagebox.de:ctxremote
#BACKUP_SSH_PORT=23
#BACKUP_SSH_KEY=/root/.ssh/ctxremote-backup
ENV
    chmod 640 /etc/ctxremote/backup.env
fi

systemctl daemon-reload
systemctl enable --now ctxremote-backup.timer
/opt/ctxremote/ctxremote-backup.sh
echo
systemctl list-timers ctxremote-backup.timer --no-pager
