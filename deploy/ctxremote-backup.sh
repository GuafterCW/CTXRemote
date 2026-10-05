#!/usr/bin/env bash
# Backs up the CTXRemote server's data folder (accounts, devices, aliases,
# tunnel key) and restores it. Runs daily as root from ctxremote-backup.timer;
# set up once with setup-backup.sh. See docs/DEPLOY.md, "Sicherung".
#
#   ctxremote-backup.sh                 make a backup now
#   ctxremote-backup.sh --list          show the backups
#   ctxremote-backup.sh --restore FILE  put a backup back (stops the server meanwhile)
#
# Settings come from /etc/ctxremote/backup.env (all optional):
#   BACKUP_DIR=/var/backups/ctxremote      where backups go
#   BACKUP_KEEP_DAYS=14                    how long they stay
#   BACKUP_PASSPHRASE_FILE=/etc/ctxremote/backup.key
#                                          encrypts backups when the file exists
#                                          (AES-256, openssl, PBKDF2)
#   BACKUP_REMOTE=u123456@u123456.your-storagebox.de:ctxremote
#                                          copies the backup folder there (rsync over SSH);
#                                          only with encryption, the data holds the tunnel key
#   BACKUP_SSH_PORT=23                     SSH port of the remote (Storage Box: 23)
#   BACKUP_SSH_KEY=/root/.ssh/ctxremote-backup
set -euo pipefail

CONFIG=${CTXREMOTE_BACKUP_CONFIG:-/etc/ctxremote/backup.env}
# shellcheck disable=SC1090
[ -f "$CONFIG" ] && . "$CONFIG"

DATA=${CTXREMOTE_DATA:-/var/lib/ctxremote}
BACKUP_DIR=${BACKUP_DIR:-/var/backups/ctxremote}
BACKUP_KEEP_DAYS=${BACKUP_KEEP_DAYS:-14}
BACKUP_PASSPHRASE_FILE=${BACKUP_PASSPHRASE_FILE:-/etc/ctxremote/backup.key}
BACKUP_REMOTE=${BACKUP_REMOTE:-}
BACKUP_SSH_PORT=${BACKUP_SSH_PORT:-23}
BACKUP_SSH_KEY=${BACKUP_SSH_KEY:-/root/.ssh/ctxremote-backup}
SERVICE=${CTXREMOTE_SERVICE:-ctxremote-server}

log() { echo "ctxremote-backup: $*"; }
fail() { echo "ctxremote-backup: $*" >&2; exit 1; }

encrypted() { [ -f "$BACKUP_PASSPHRASE_FILE" ]; }

backup() {
    [ -d "$DATA" ] || fail "Datenordner $DATA fehlt"
    umask 077
    mkdir -p "$BACKUP_DIR"
    chmod 700 "$BACKUP_DIR"
    local stamp name tmp
    stamp=$(date +%Y%m%d-%H%M%S)
    name="ctxremote-$stamp.tar.gz"
    tmp="$BACKUP_DIR/.incoming"
    rm -f "$tmp"
    # The installers in updates/ come back with the next release; everything
    # else is written atomically by the server, so each file is consistent.
    tar -C "$DATA" --exclude=./updates -czf "$tmp" .
    tar -tzf "$tmp" >/dev/null || fail "Sicherung ist unlesbar, nichts gespeichert"
    if encrypted; then
        openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -salt -pass "file:$BACKUP_PASSPHRASE_FILE" \
            -in "$tmp" -out "$tmp.enc"
        rm -f "$tmp"
        mv "$tmp.enc" "$BACKUP_DIR/$name.enc"
        name="$name.enc"
    else
        mv "$tmp" "$BACKUP_DIR/$name"
    fi
    log "gesichert: $BACKUP_DIR/$name ($(du -h "$BACKUP_DIR/$name" | cut -f1))"

    find "$BACKUP_DIR" -maxdepth 1 -name 'ctxremote-*.tar.gz*' -mtime +"$BACKUP_KEEP_DAYS" -delete

    if [ -n "$BACKUP_REMOTE" ]; then
        encrypted || fail "BACKUP_REMOTE ohne Verschlüsselung: $BACKUP_PASSPHRASE_FILE fehlt, nichts hochgeladen"
        command -v rsync >/dev/null || fail "rsync fehlt (apt install rsync)"
        # Mirrors the folder, so the retention applies there too.
        rsync -a --delete --include='ctxremote-*.tar.gz.enc' --exclude='*' \
            -e "ssh -p $BACKUP_SSH_PORT -i $BACKUP_SSH_KEY -o BatchMode=yes" \
            "$BACKUP_DIR/" "$BACKUP_REMOTE/"
        log "kopiert nach $BACKUP_REMOTE"
    fi
}

list() {
    ls -lh "$BACKUP_DIR"/ctxremote-*.tar.gz* 2>/dev/null || log "keine Sicherungen in $BACKUP_DIR"
}

restore() {
    local file=$1
    [ -f "$file" ] || fail "$file gibt es nicht"
    local work
    work=$(mktemp -d)
    trap 'rm -rf "$work"' RETURN
    case "$file" in
        *.enc)
            encrypted || fail "$BACKUP_PASSPHRASE_FILE fehlt, die Sicherung ist verschlüsselt"
            openssl enc -d -aes-256-cbc -pbkdf2 -iter 200000 -pass "file:$BACKUP_PASSPHRASE_FILE" \
                -in "$file" -out "$work/backup.tar.gz" || fail "Entschlüsseln fehlgeschlagen (falsche Passphrase?)"
            ;;
        *) cp "$file" "$work/backup.tar.gz" ;;
    esac
    mkdir "$work/data"
    tar -C "$work/data" -xzf "$work/backup.tar.gz"
    [ -f "$work/data/tunnel.key" ] || fail "In der Sicherung fehlt tunnel.key, sie passt nicht zu einem CTXRemote-Server"

    local owner aside
    owner=$(stat -c %U:%G "$DATA" 2>/dev/null || echo root:root)
    aside="$DATA.vor-wiederherstellung-$(date +%Y%m%d-%H%M%S)"
    if command -v systemctl >/dev/null; then systemctl stop "$SERVICE" || true; fi
    # Keep what was there; the installers stay in place.
    cp -a "$DATA" "$aside"
    find "$DATA" -mindepth 1 -maxdepth 1 ! -name updates -exec rm -rf {} +
    cp -a "$work/data/." "$DATA/"
    chown -R "$owner" "$DATA"
    if command -v systemctl >/dev/null; then systemctl start "$SERVICE"; fi
    log "wiederhergestellt aus $file; der vorherige Stand liegt in $aside"
}

case "${1:-}" in
    "") backup ;;
    --list) list ;;
    --restore) [ -n "${2:-}" ] || fail "Aufruf: $0 --restore DATEI"; restore "$2" ;;
    *) fail "Aufruf: $0 [--list | --restore DATEI]" ;;
esac
