#!/usr/bin/env bash
# One-time setup of the website on the server, after setup-server.sh. Run as root:
#
#   bash setup-website.sh [domain]            (default: ctxremote.ctx.ink)
#   bash setup-website.sh --docker [domain]   Caddy runs in a container: only
#                                             creates the folder and prints the
#                                             block and the volume to add
#
# Installs Caddy unless it already runs (existing sites stay as they are; this
# only adds /etc/caddy/ctxremote.caddy and one import line, and undoes both if
# the result does not validate). Caddy fetches the HTTPS certificate on its own
# (ports 80 and 443 must be reachable) and serves /var/www/ctxremote. The release workflow
# fills that folder as user `ctxremote`: site/ (a symlink to the newest
# website) and download/ (the newest installer and quick helper).
# See docs/DEPLOY.md, section "Website".
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Bitte als root ausführen." >&2
  exit 1
fi
docker=false
if [ "${1:-}" = "--docker" ]; then docker=true; shift; fi
domain="${1:-ctxremote.ctx.ink}"
id ctxremote >/dev/null 2>&1 || { echo "Erst setup-server.sh ausführen." >&2; exit 1; }

if $docker; then
  install -d -o ctxremote -g ctxremote -m 755 /var/www/ctxremote /var/www/ctxremote/download
  cat <<HINT
Ordner /var/www/ctxremote angelegt; die Pipeline füllt ihn ab dem nächsten Push.

1. Im docker-compose.yml des Caddy-Containers ergänzen:

      volumes:
        - /var/www/ctxremote:/srv/ctxremote:ro
      extra_hosts:
        - "host.docker.internal:host-gateway"

   Und den CTXRemote-Server die Web-API auf der Docker-Brücke anbieten lassen
   (systemctl edit ctxremote-server, siehe docs/DEPLOY.md):
      --http 172.17.0.1:21380

2. In die Caddyfile des Containers diesen Block einfügen:

$domain {
	encode zstd gzip
	header {
		Strict-Transport-Security "max-age=31536000"
		X-Content-Type-Options "nosniff"
		Referrer-Policy "no-referrer"
		Content-Security-Policy "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
		-Server
	}
	# The web interface's API on the host (ctxremote-server --http, see docs/DEPLOY.md).
	handle /api/* {
		reverse_proxy host.docker.internal:21380
	}
	handle /download/* {
		root * /srv/ctxremote
		header Cache-Control "no-cache"
		file_server
	}
	handle {
		root * /srv/ctxremote/site
		file_server
	}
}

3. Container mit dem neuen Volume neu erstellen (z. B. docker compose up -d caddy).
   Nur die Caddyfile neu laden ginge ohne Unterbrechung:
   docker compose exec -w /etc/caddy caddy caddy reload
HINT
  exit 0
fi

caddyfile=/etc/caddy/Caddyfile
if command -v caddy >/dev/null && systemctl is-enabled caddy >/dev/null 2>&1; then
  echo "Vorhandenes Caddy gefunden (Dienst caddy); bestehende Seiten bleiben unverändert."
elif command -v ss >/dev/null && ss -ltnH '( sport = :80 or sport = :443 )' | grep -q .; then
  # Something else (Caddy in Docker, nginx, …) holds the ports: do not install a second web server.
  echo "Port 80/443 ist schon belegt, aber nicht vom Dienst caddy (Docker? anderer Webserver?)." >&2
  echo "Nichts geändert. Für Caddy in Docker: bash setup-website.sh --docker $domain" >&2
  exit 1
else
  apt-get update -q >/dev/null
  DEBIAN_FRONTEND=noninteractive apt-get install -y -q caddy >/dev/null
fi
if grep -qsE "^[[:space:]]*(https?://)?$domain([:,[:space:]{]|$)" "$caddyfile" /etc/caddy/*.caddy 2>/dev/null \
   && ! grep -qs "Written by setup-website.sh" /etc/caddy/ctxremote.caddy; then
  echo "$domain ist in der Caddy-Konfiguration schon eingetragen. Bitte diesen Block entfernen oder" >&2
  echo "den Block aus docs/DEPLOY.md (Abschnitt „Website“) von Hand einfügen. Nichts geändert." >&2
  exit 1
fi

web=/var/www/ctxremote
install -d -o ctxremote -g ctxremote -m 755 "$web" "$web/download"

# Everything below is undone if the resulting configuration does not validate,
# so a failed setup never leaves Caddy unable to start with the existing sites.
backup="$(mktemp)"
had_caddyfile=false
if [ -f "$caddyfile" ]; then cp -p "$caddyfile" "$backup"; had_caddyfile=true; fi
had_site=false
[ -f /etc/caddy/ctxremote.caddy ] && cp -p /etc/caddy/ctxremote.caddy "$backup.site" && had_site=true
restore() {
  if $had_caddyfile; then cp -p "$backup" "$caddyfile"; else rm -f "$caddyfile"; fi
  if $had_site; then cp -p "$backup.site" /etc/caddy/ctxremote.caddy; else rm -f /etc/caddy/ctxremote.caddy; fi
  echo "Caddy-Konfiguration ungültig, alles zurückgesetzt. Deine Seiten laufen unverändert weiter." >&2
}

install -d /etc/caddy
cat > /etc/caddy/ctxremote.caddy <<CADDY
# Written by setup-website.sh. Access logs stay off (see datenschutz.html).
$domain {
	encode zstd gzip
	header {
		Strict-Transport-Security "max-age=31536000"
		X-Content-Type-Options "nosniff"
		Referrer-Policy "no-referrer"
		Content-Security-Policy "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"
		-Server
	}
	# The web interface's API (ctxremote-server --http 127.0.0.1:21380).
	handle /api/* {
		reverse_proxy 127.0.0.1:21380
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

if ! grep -qs 'import /etc/caddy/ctxremote.caddy' "$caddyfile"; then
  # The package's default file only serves a placeholder page on :80; replace it.
  if [ ! -s "$caddyfile" ] || grep -q 'The Caddyfile is an easy way' "$caddyfile"; then
    echo 'import /etc/caddy/ctxremote.caddy' > "$caddyfile"
  else
    printf '\nimport /etc/caddy/ctxremote.caddy\n' >> "$caddyfile"
  fi
fi
if ! caddy validate --config "$caddyfile" --adapter caddyfile >/dev/null 2>"$backup.err"; then
  cat "$backup.err" >&2
  restore
  exit 1
fi
rm -f "$backup" "$backup.site" "$backup.err"
systemctl enable caddy >/dev/null
# Reload keeps the running sites up; only a stopped Caddy is started.
if systemctl is-active caddy >/dev/null; then systemctl reload caddy; else systemctl start caddy; fi

if command -v ufw >/dev/null && ufw status | grep -q 'Status: active'; then
  ufw allow 80/tcp >/dev/null
  ufw allow 443/tcp >/dev/null
fi
echo "Website eingerichtet für https://$domain"
echo "Inhalte kommen mit der nächsten Auslieferung aus GitHub Actions."
echo "Firewall des Anbieters: TCP 80 und 443 eingehend freigeben."
