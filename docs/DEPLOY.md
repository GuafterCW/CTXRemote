# Auslieferung: Server-Pipeline und automatische Updates

Stand: 4. Oktober 2026. Jeder Push auf `master` startet `.github/workflows/release.yml`:

1. **Version**: `0.1.<Laufnummer>`, steigt also mit jedem Lauf.
2. **Server** (Ubuntu): Tests, dann ein statisches Linux-Binary (musl, etwa 2 MB, läuft auf jedem Linux).
3. **Client** (Windows):
   - Er baut den NSIS-Installer samt Dienst und, falls der Server eingetragen ist, die Schnellhilfe.
   - Serveradresse, Version und öffentlicher Update-Schlüssel werden fest eingebaut.
4. **Ausliefern** (nur von `master` und nur, wenn `DEPLOY_HOST` gesetzt ist):
   - Der Installer wird signiert.
   - Alles geht per SSH auf den Server.
   - `deploy/remote-deploy.sh` tauscht das Server-Binary und startet den Dienst neu. Kommt er nicht hoch, stellt das Skript die vorige Version wieder her.
   - Danach veröffentlicht es das Client-Update.

Ohne die Einstellungen unten laufen die Builds trotzdem. Installer und Schnellhilfe liegen dann als Artefakte am Workflow-Lauf.

## Serveradresse ohne Port

`remote.ctx.ink` reicht als Adresse, der Client ergänzt den Standardport 21300 selbst. Release-Builds haben die Adresse aus `CTXREMOTE_SERVER` schon eingebaut, auf neuen Geräten muss man also gar nichts eintragen. Bestehende Installationen behalten ihre gespeicherte Adresse.

## Updates

- Die Clients holen Updates **vom eigenen CTXRemote-Server** über das normale Protokoll, nicht von GitHub. Das funktioniert auch mit privatem Repo, ohne Webserver und ohne Zertifikat.
- **Vertrauen** kommt aus einer Ed25519-Signatur, die in der Pipeline entsteht:
  - Clients prüfen sie gegen den eingebauten öffentlichen Schlüssel.
  - Sie installieren nur Signiertes und nur Versionen, die neuer sind als die eigene.
  - Ein gekaperter Server kann deshalb kein eigenes Programm verteilen.
- **Mit Dienst** (normale Installation):
  - Der Dienst prüft 2 Minuten nach dem Start und danach alle 6 Stunden.
  - Er lädt nach `C:\ProgramData\CTXRemote\updates`. Dort dürfen nur SYSTEM und Administratoren schreiben.
  - Er installiert still, aber nur, solange niemand verbunden ist.
  - Der Installer stoppt den Dienst, ersetzt die Dateien und startet ihn wieder.
- **Ohne Dienst**: Die App zeigt oben „Update auf X“. Der Klick lädt das Update und startet den Installer mit UAC-Abfrage.
- **Schnellhilfe**: Sie aktualisiert sich nicht. Man lädt sie einfach neu aus dem letzten Workflow-Lauf.
- **Ältere Server** kennen die Update-Anfrage nicht und legen auf. Für den Client heißt das dann „kein Update“.
- **Server-Werkzeuge:**
  - `ctxremote-server update-keygen` erzeugt ein Schlüsselpaar.
  - `ctxremote-server update-sign …` signiert ein Update und legt es in `<data>/updates` ab. Der Server liest den Ordner bei jeder Anfrage, ein neues Release braucht also keinen Neustart.

## Einrichtung Schritt für Schritt

Einmalig nötig, etwa 30 Minuten. Du brauchst:

- **Server:** einen Linux-Server mit systemd (Debian, Ubuntu o. ä.) und Zugang als root per SSH.
- **Windows-PC:** Git, Rust und das Repo. OpenSSH (`ssh`, `scp`, `ssh-keygen`) ist in Windows 10/11 eingebaut.

In den Befehlen steht `remote.ctx.ink` für deinen Server; ggf. ersetzen. Befehle mit `PS>` gehören in die PowerShell auf deinem PC, Befehle mit `#` laufen als root auf dem Server.

**Neuer Server ohne bisherige CTXRemote-Installation?** Dann Schritt 1 und 4 überspringen und in Schritt 5 den zweiten Parameter (Datenordner) weglassen.

### Schritt 1: Alten Datenordner finden und sichern

Der Server merkt sich in `devices.json`, welche Geräte-ID zu welchem Gerät gehört. Diese Datei muss erhalten bleiben, sonst bekommen alle Geräte neue IDs.

```bash
ssh root@remote.ctx.ink
# find / -name devices.json -not -path "*/proc/*" 2>/dev/null
```

Den gefundenen Ordner merken (im Folgenden `/pfad/zum/datenordner`) und sichern:

```bash
# cp -a /pfad/zum/datenordner /root/ctxremote-backup-$(date +%F)
```

### Schritt 2: Deploy-Schlüssel erzeugen

Damit meldet sich GitHub später auf dem Server an. In einem Ordner außerhalb des Repos, z. B. `C:\ctxremote-keys`:

```powershell
PS> mkdir C:\ctxremote-keys; cd C:\ctxremote-keys
PS> ssh-keygen -t ed25519 -f ctxremote-deploy -N '""' -C ctxremote-deploy
```

Danach liegen dort zwei Dateien: `ctxremote-deploy` (privat) und `ctxremote-deploy.pub` (öffentlich).

### Schritt 3: Einrichtungsdateien auf den Server kopieren

Im Repo-Ordner auf deinem PC:

```powershell
PS> scp -r deploy root@remote.ctx.ink:/root/ctxremote-deploy
```

### Schritt 4: Alten Server stoppen

Port 21300 muss frei sein, bevor der neue Dienst startet. Wie du den alten Server stoppst, hängt davon ab, wie du ihn gestartet hast:

- **Von Hand / mit `nohup`, `screen` oder `tmux`:** `# pkill -f ctxremote-server`
- **Mit einer eigenen systemd-Unit:** `# systemctl disable --now <name-der-unit>`
- **In Docker:** `# docker stop <container>` und den Container aus dem Autostart nehmen (`docker update --restart=no <container>`)

Prüfen: `# ss -ltnp | grep 21300` darf nichts mehr ausgeben.

### Schritt 5: Einrichtung ausführen

Den Inhalt von `ctxremote-deploy.pub` anzeigen und die eine Zeile kopieren (beginnt mit `ssh-ed25519`):

```powershell
PS> Get-Content C:\ctxremote-keys\ctxremote-deploy.pub
```

Auf dem Server, mit der kopierten Zeile in Anführungszeichen und dem Datenordner aus Schritt 1:

```bash
# cd /root/ctxremote-deploy
# bash setup-server.sh "ssh-ed25519 AAAA…  ctxremote-deploy" /pfad/zum/datenordner
```

Erwartete Ausgabe: „Geräte-IDs aus … übernommen.“ und „Eingerichtet. Der Server startet mit der ersten Auslieferung …“.

Firewall (nur falls `ufw` aktiv ist; bei einer Firewall des Anbieters, z. B. Hetzner Robot, dort TCP 21300 **und UDP 21300** eingehend freigeben):

```bash
# ufw allow 21300/tcp
# ufw allow 21300/udp
```

UDP 21300 braucht nur die Direktverbindung durch NAT (siehe `docs/DIRECT.md`). Ohne diese Freigabe laufen Sitzungen weiter, dann aber über den Server, sobald beide Seiten hinter einem Router sitzen.

### Schritt 6: Anmeldung testen

Vom PC aus. Das muss **ohne** Passwortabfrage `ok` ausgeben:

```powershell
PS> ssh -i C:\ctxremote-keys\ctxremote-deploy ctxremote@remote.ctx.ink "echo ok"
```

Wird nach einem Passwort gefragt, stimmt der Schlüssel in Schritt 5 nicht. Den Befehl aus Schritt 5 dann mit der richtigen Zeile wiederholen; das Skript darf mehrfach laufen.

### Schritt 7: Fingerabdruck des Servers holen

```powershell
PS> ssh-keyscan remote.ctx.ink
```

Die ganze Ausgabe (mehrere Zeilen) kopieren, sie wird in Schritt 9 gebraucht.

### Schritt 8: Update-Schlüssel erzeugen

Im Repo-Ordner:

```powershell
PS> cargo run -p ctxremote-server -- update-keygen
```

Die Ausgabe enthält einen **privaten** und einen **öffentlichen** Schlüssel. Den privaten sofort im Passwortmanager sichern. Geht er verloren, nehmen bestehende Geräte keine Updates mehr an, bis man sie einmal von Hand neu installiert.

### Schritt 9: In GitHub eintragen

Im Repo auf github.com: **Settings → Secrets and variables → Actions**.

Reiter **Secrets**, je „New repository secret“:

| Name | Inhalt |
|---|---|
| `DEPLOY_SSH_KEY` | kompletter Inhalt von `C:\ctxremote-keys\ctxremote-deploy` (ohne `.pub`), einschließlich der Zeilen `-----BEGIN…` und `-----END…`. Anzeigen mit `Get-Content C:\ctxremote-keys\ctxremote-deploy` |
| `DEPLOY_KNOWN_HOSTS` | Ausgabe aus Schritt 7 |
| `CTXREMOTE_UPDATE_SIGNING_KEY` | **privater** Schlüssel aus Schritt 8 |

Reiter **Variables**, je „New repository variable“:

| Name | Inhalt |
|---|---|
| `CTXREMOTE_UPDATE_KEY` | **öffentlicher** Schlüssel aus Schritt 8 |
| `CTXREMOTE_SERVER` | `remote.ctx.ink` |
| `DEPLOY_HOST` | `remote.ctx.ink` (oder die IP des Servers) |

Danach die privaten Schlüsseldateien in `C:\ctxremote-keys` löschen oder sicher verwahren. Sie liegen jetzt in GitHub.

### Schritt 10: Nach `master` bringen

Entweder auf github.com einen Pull Request vom Arbeitsbranch nach `master` öffnen und mergen, oder lokal:

```powershell
PS> git checkout master
PS> git pull
PS> git merge claude/vigilant-lovelace-rvm0ax
PS> git push
```

### Schritt 11: Ersten Lauf beobachten

Auf github.com im Reiter **Actions** den Lauf „Release“ öffnen. Nach etwa 10–15 Minuten sind alle vier Jobs grün: version, server, client, deploy.

Auf dem Server prüfen:

```bash
# systemctl status ctxremote-server      # „active (running)“
# ls /var/lib/ctxremote/updates          # CTXRemote-0.1.N-windows-x86_64.exe und windows-x86_64.json
# journalctl -u ctxremote-server -n 20   # Log; hier sieht man Anmeldungen und Update-Abrufe
```

Bei einem Fehler im Job „deploy“ zeigt dessen Log den Grund. Die häufigsten stehen unten unter „Wenn etwas nicht klappt“.

### Schritt 12: Geräte einmal von Hand aktualisieren

Bereits installierte Geräte kennen den Update-Schlüssel noch nicht. Deshalb jedes Gerät einmal von Hand aktualisieren:

1. Im Lauf aus Schritt 11 unten unter **Artifacts** das Paket `client` herunterladen und entpacken.
2. `CTXRemote-0.1.N-setup.exe` auf jedem Gerät ausführen, über die alte Installation drüber. Geräte-ID und Einstellungen bleiben erhalten.

`CTXRemote-Hilfe.exe` im selben Paket ist die neue Schnellhilfe mit eingebauter Serveradresse.

### Schritt 13: Automatisches Update prüfen

1. Eine kleine Änderung nach `master` pushen und warten, bis der neue Release-Lauf grün ist.
2. Auf einem Gerät mit Dienst (PowerShell als Administrator) den Dienst neu starten. Er prüft dann nach 2 Minuten statt erst nach bis zu 6 Stunden:

   ```powershell
   PS> Restart-Service CTXRemote
   ```

3. Nach etwa 3 Minuten muss die App-Version (unten in den Einstellungen) die neue Nummer zeigen.
4. Das Protokoll steht in `C:\ProgramData\CTXRemote\logs\service.log` und enthält „Update verfügbar“ und „Update wird installiert“.

## Website (https://remote.ctx.ink)

Die Website liegt im Ordner `website/`: statische Seiten ohne JavaScript, im Design der App. Jeder Push nach `master` liefert sie zusammen mit dem Server aus:
- Die Seiten gehen nach `/var/www/ctxremote/site`. Das ist ein Symlink auf die neueste Fassung und wird atomar umgeschaltet.
- In `download.html` setzt die Pipeline die Versionsnummer ein, und zwar zwischen `<!--version-->` und `<!--/version-->`.
- Der neueste Installer und die Schnellhilfe liegen unter festen Namen in `/var/www/ctxremote/download/` (`CTXRemote-Setup.exe`, `CTXRemote-Hilfe.exe`). Die Links auf der Website ändern sich also nie.

Caddy liefert alles aus und holt sich das HTTPS-Zertifikat selbst. Zugriffe werden nicht protokolliert, so steht es auch in der Datenschutzerklärung.

### Einmalig einrichten

1. **Firewall des Anbieters** (Hetzner Robot): TCP **80** und **443** eingehend freigeben. Port 80 braucht Caddy für das Zertifikat.
2. **DNS:** `remote.ctx.ink` zeigt schon auf den Server. Bei Cloudflare muss der Eintrag auf „Nur DNS“ (graue Wolke) bleiben, sonst kommen weder die App (Port 21300) noch die Zertifikatsabfrage durch.
3. **Auf dem Server** als root, aus dem Ordner mit den Einrichtungsdateien (Schritt 3 oben; `deploy/setup-website.sh` vorher mit hineinkopieren):

   ```bash
   # bash setup-website.sh remote.ctx.ink
   ```

   Erwartet: „Website eingerichtet für https://remote.ctx.ink“. Läuft Caddy dort schon als Dienst, bleiben deine bestehenden Seiten unverändert: Das Skript legt nur `/etc/caddy/ctxremote.caddy` an, hängt eine `import`-Zeile an die `Caddyfile` und nimmt beides zurück, falls die Konfiguration danach ungültig wäre.

   **Caddy im Docker-Container** (so beim Nutzer): stattdessen

   ```bash
   # bash setup-website.sh --docker remote.ctx.ink
   ```

   Das legt nur den Ordner `/var/www/ctxremote` an und gibt zwei Dinge aus, die du selbst einträgst. Am Server selbst ändert es nichts.
   - die Volume-Zeile für `docker-compose.yml`: `- /var/www/ctxremote:/srv/ctxremote:ro`
   - den Block für die Caddyfile des Containers, mit den Pfaden `/srv/ctxremote`

   Danach den Container mit dem neuen Volume neu erstellen (`docker compose up -d caddy`). Der Symlink `site` ist relativ, funktioniert also auch unter dem anderen Pfad im Container.
4. **Einmal nach `master` pushen** (oder den Release-Workflow von Hand starten). Danach zeigt https://remote.ctx.ink die Seite.
5. **Vor dem Veröffentlichen** in `website/impressum.html` und `website/datenschutz.html` die markierten Platzhalter ersetzen, also Name, Anschrift und E-Mail. Bei Hetzner im Robot den **Vertrag zur Auftragsverarbeitung** abschließen, die Datenschutzerklärung verweist darauf.

Prüfen: `curl -I https://remote.ctx.ink` muss `HTTP/2 200` liefern, `ls -l /var/www/ctxremote` zeigt `site -> site-…` und `download/`.

Ohne `setup-website.sh` lässt die Pipeline die Website einfach aus. Server und Updates laufen wie bisher.

## Wenn etwas nicht klappt

| Symptom | Ursache und Lösung |
|---|---|
| Job „deploy“ wird übersprungen | Variable `DEPLOY_HOST` fehlt, oder der Lauf war nicht auf `master` |
| `Permission denied (publickey)` | `DEPLOY_SSH_KEY` unvollständig (BEGIN/END-Zeilen fehlen) oder öffentlicher Schlüssel nicht auf dem Server (Schritt 5 wiederholen) |
| `Host key verification failed` | `DEPLOY_KNOWN_HOSTS` fehlt oder passt nicht zu `DEPLOY_HOST` (Schritt 7 mit genau dem Namen aus `DEPLOY_HOST` wiederholen) |
| Website lädt nicht oder Zertifikatsfehler | TCP 80/443 in der Firewall freigegeben? `journalctl -u caddy -n 50` zeigt, warum das Zertifikat nicht kam. Bei Cloudflare muss „Nur DNS“ eingestellt sein |
| „Neuer Server startet nicht, vorherige Version wird wiederhergestellt“ | Port 21300 ist noch belegt (Schritt 4) oder es gibt einen echten Fehler: `journalctl -u ctxremote-server -n 50` |
| Warnung „Kein CTXREMOTE_UPDATE_SIGNING_KEY“ | Secret fehlt; der Server wurde trotzdem aktualisiert, nur kein Client-Update veröffentlicht |
| Geräte aktualisieren sich nicht | Gerät noch nicht einmal von Hand aktualisiert (Schritt 12), oder `CTXREMOTE_UPDATE_KEY` passt nicht zum privaten Schlüssel. Im `service.log` steht dann „nicht gültig signiert“ |
| Alle Geräte haben neue IDs | `devices.json` wurde nicht übernommen: Server stoppen, Datei aus der Sicherung (Schritt 1) nach `/var/lib/ctxremote/` kopieren, `chown ctxremote:ctxremote` darauf, Server starten |

## Getestet

- Unter Linux:
  - Signieren, Ausliefern über mehrere Blöcke, falscher Schlüssel, getauschte Datei und zurückgezogenes Release (`crates/server/tests/updates.rs`)
  - `deploy/remote-deploy.sh` mit nachgebildetem systemd, inklusive Rückfall auf die vorige Version, wenn der neue Server nicht startet
  - der statische musl-Build
- Auf GitHub: ein Probelauf des Workflows auf dem Arbeitsbranch, ohne Auslieferung. Version, Server und Client waren grün, Installer und Server lagen als Artefakte vor. Den Schritt Schnellhilfe hat der Lauf übersprungen, weil die Variable fehlte. Lokal baut er mit derselben Konfiguration.
- **Nicht getestet**:
  - der echte Workflow-Lauf, der erst nach der Einrichtung möglich ist
  - die stille Installation durch den Dienst auf Windows
  - der Installer-Start mit UAC aus der App
