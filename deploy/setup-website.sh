#!/usr/bin/env bash
# One-time setup of the website on the server, after setup-server.sh. Run as root:
#
#   bash setup-website.sh [domain]      (default: remote.ctx.ink)
#
# Installs Caddy, which fetches the HTTPS certificate on its own (ports 80 and
# 443 must be reachable), and serves /var/www/ctxremote. The release workflow
# fills that folder as user `ctxremote`: site/ (a symlink to the newest
# website) and download/ (the newest installer and quick helper).
# See docs/DEPLOY.md, section "Website".
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Bitte als root ausführen." >&2
  exit 1
fi
domain="${1:-remote.ctx.ink}"
id ctxremote >/dev/null 2>&1 || { echo "Erst setup-server.sh ausführen." >&2; exit 1; }

if ! command -v caddy >/dev/null; then
  apt-get update -q >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q caddy >/dev/null
fi

web=/var/www/ctxremote
install -d -o ctxremote -g ctxremote -m 755 "$web" "$web/download"

install -d /etc/caddy
cat > /etc/caddy/ctxremote.caddy <<CADDY
# Written by setup-website.sh. Access logs stay off (see datenschutz.html).
$domain {
	encode zstd gzip
	header {
		Strict-Transport-Security "max-age=31536000"
		X-Content-Type-Options "nosniff"
		Referrer-Policy "no-referrer"
		Content-Security-Policy "default-src 'none'; style-src 'self' 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
		-Server
	}
	handle /download/* {
		root * $web
		header Cache-Control "no-cache"
		file_server
	}
	handle {
		root * $web/site
		file_server
	}
}
CADDY

caddyfile=/etc/caddy/Caddyfile
if ! grep -qs 'import /etc/caddy/ctxremote.caddy' "$caddyfile"; then
  # The package's default file only serves a placeholder page on :80; replace it.
  if [ ! -s "$caddyfile" ] || grep -q 'The Caddyfile is an easy way' "$caddyfile"; then
    echo 'import /etc/caddy/ctxremote.caddy' > "$caddyfile"
  else
    echo 'import /etc/caddy/ctxremote.caddy' >> "$caddyfile"
  fi
fi
caddy validate --config "$caddyfile" --adapter caddyfile >/dev/null
systemctl enable caddy >/dev/null
systemctl reload caddy 2>/dev/null || systemctl restart caddy

if command -v ufw >/dev/null && ufw status | grep -q 'Status: active'; then
  ufw allow 80/tcp >/dev/null
  ufw allow 443/tcp >/dev/null
fi
echo "Website eingerichtet für https://$domain"
echo "Inhalte kommen mit der nächsten Auslieferung aus GitHub Actions."
echo "Firewall des Anbieters: TCP 80 und 443 eingehend freigeben."
