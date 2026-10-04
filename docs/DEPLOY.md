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

## Einmalige Einrichtung

### 1. Server

Auf dem Server als root, im Ordner `deploy/` dieses Repos. Bitte zuerst ein Backup des bisherigen Datenordners machen:

```bash
bash setup-server.sh "<Inhalt von ctxremote-deploy.pub>" /pfad/zum/bisherigen/datenordner
ufw allow 21300/tcp   # falls ufw läuft
```

Das Skript richtet Folgendes ein:

- den Benutzer `ctxremote`
- die Ordner `/opt/ctxremote` und `/var/lib/ctxremote`
- die systemd-Unit `ctxremote-server`
- eine sudo-Regel, die nur den Neustart dieses Dienstes erlaubt

Die `devices.json` aus dem alten Datenordner übernimmt es, damit alle Geräte-IDs gültig bleiben. Den bisher von Hand gestarteten Server danach beenden, sonst ist Port 21300 belegt.

### 2. Schlüssel (auf deinem Rechner)

```powershell
ssh-keygen -t ed25519 -f ctxremote-deploy -N '""' -C ctxremote-deploy   # Deploy-Schlüssel
ssh-keyscan remote.ctx.ink                                              # Ausgabe -> DEPLOY_KNOWN_HOSTS
cargo run -p ctxremote-server -- update-keygen                           # Update-Schlüsselpaar
```

### 3. GitHub: Settings → Secrets and variables → Actions

| Art | Name | Inhalt |
|---|---|---|
| Secret | `DEPLOY_SSH_KEY` | Inhalt der Datei `ctxremote-deploy` (privat) |
| Secret | `DEPLOY_KNOWN_HOSTS` | Ausgabe von `ssh-keyscan` |
| Secret | `CTXREMOTE_UPDATE_SIGNING_KEY` | privater Schlüssel aus `update-keygen` |
| Variable | `CTXREMOTE_UPDATE_KEY` | öffentlicher Schlüssel aus `update-keygen` |
| Variable | `CTXREMOTE_SERVER` | `remote.ctx.ink` |
| Variable | `DEPLOY_HOST` | `remote.ctx.ink` (oder IP) |
| Variable | `DEPLOY_USER` | optional, Standard `ctxremote` |

Den privaten Update-Schlüssel sicher aufbewahren, z. B. im Passwortmanager. Geht er verloren, nehmen bestehende Clients keine Updates mehr an, bis man sie einmal von Hand neu installiert.

### 4. Erste Installation

Clients von vor der Pipeline kennen den Update-Schlüssel nicht. Den Installer aus dem ersten Pipeline-Lauf deshalb einmal von Hand installieren (Artefakt `client` am Workflow-Lauf). Danach aktualisieren sich die Geräte selbst.

## Getestet

- Unter Linux:
  - Signieren, Ausliefern über mehrere Blöcke, falscher Schlüssel, getauschte Datei und zurückgezogenes Release (`crates/server/tests/updates.rs`)
  - `deploy/remote-deploy.sh` mit nachgebildetem systemd, inklusive Rückfall auf die vorige Version, wenn der neue Server nicht startet
  - der statische musl-Build
- **Nicht getestet**:
  - der echte Workflow-Lauf, der erst nach der Einrichtung möglich ist
  - die stille Installation durch den Dienst auf Windows
  - der Installer-Start mit UAC aus der App
