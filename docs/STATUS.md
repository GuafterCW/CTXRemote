# Projektstand CTXRemote

Stand: 4. Oktober 2026. Übergabenotiz, damit die Arbeit auf einem anderen Rechner nahtlos weitergehen kann.

## Fertig

- **Server** (`crates/server`): ID-Vergabe (an einen Ed25519-Schlüssel gebunden), Vermittlung und Relay über TCP 21300, Drosselung pro IP
- **Protokoll und Krypto** (`crates/proto`): SPAKE2 mit mehreren Passwort-Slots (Einmal-Passwort und festes Passwort), danach ChaCha20-Poly1305 Ende-zu-Ende
- **Windows-Host** (`crates/core`): DXGI-Bildschirmaufnahme, H.264 (OpenH264), Eingabe per SendInput mit Scancodes, Sperre nach 5 Fehlversuchen, Einmal-Passwort wird nach jeder Sitzung erneuert
- **App** (`app`, Tauri 2 + Svelte 5): Hauptfenster, Sitzungsfenster (WebCodecs), Tray, Einstellungen, unbeaufsichtigter Zugriff, Geräte-Aliase mit Suche
- **Plattformen:** Der Code baut auf Windows und Linux. Auf Systemen ohne Host-Unterstützung greifen Platzhalter (`capture::HOST_SUPPORTED`).
- **CI:** `.github/workflows/ci.yml` für Windows, Linux und macOS

## Verifiziert

- Windows: der gesamte Workspace baut ohne Warnungen.
- Linux (Docker, `rust:1`): alle Tests grün, die komplette App baut.
- Windows-End-to-End-Test (`cargo run --release -p ctxremote-core --example loopback`): Sitzung nach 17 ms, erster 1920×1200-Keyframe nach 245 ms, dekodierbar.
- Die App lief einmal, und das Hauptfenster wurde per Screenshot geprüft.
- CI grün auf Windows, Linux und macOS. Damit ist auch der macOS-Build bestätigt.
- Loopback auf dem neuen Entwicklungsrechner: Sitzung nach 12 ms, erster 2560×1440-Keyframe nach 181 ms, zwei Monitore erkannt.
- GUI-Sitzung über das Netz (Viewer: Entwicklungsrechner, Host: Windows Server, Server im LAN): Verbindung, Bild, Maus, Tastatur und Aliase funktionieren.

**Noch nicht getestet:** der Monitorwechsel in einer GUI-Sitzung, weil der Test-Host nur einen Monitor hatte. Zwei Instanzen auf *einem* PC eignen sich nur, um Verbindung und Bild zu prüfen. Der Host bewegt dann die eigene Maus, und Tastatureingaben können sich im Kreis drehen.

## Offene Entscheidungen und To-dos

1. **Code-Signatur:** Smart App Control blockiert unsignierte Builds (os error 4551). Kandidat ist Microsoft Trusted Signing für etwa 10 $ im Monat.
2. **Monitorwechsel testen**, mit einem Host, der mehrere Monitore hat.

## Stand 2. Oktober 2026, abends

- **Zwischenablage** (Text, beide Richtungen): fertig und auf dem Windows Server getestet.
- **Windows-Dienst** (`crates/service`, Entwurf in `docs/WINDOWS-SERVICE.md`): fertig und auf dem Windows Server getestet:
  - Sperrbildschirm, Strg+Alt+Entf und UAC
  - Abmelden und Anmelden ohne Abbruch der Sitzung
  - App im Dienstmodus über die UI-Pipe mit ID, Passwort und Sitzungen
  - Einstellungen mit UAC-Abfrage
  - Installation und Deinstallation über den NSIS-Installer

  Zwei Windows-Eigenheiten: „Sperren“ ruft `LockWorkStation` auf, weil Windows ein eingespieltes Win+L ignoriert. `SendSAS` kommt aus dem Dienstprozess, weil Windows es von SYSTEM-Prozessen ignoriert. Unter Windows braucht `cargo build -p ctxremote` vorher `node app/scripts/prepare-service.mjs`, weil Tauri den Dienst als Sidecar schon beim Build verlangt.
- **Schnellhilfe „CTXRemote Hilfe“**: eine portable EXE (`npm run build:quick` mit `CTXREMOTE_QUICK_SERVER`). Die Konfiguration ist flüchtig, jede Verbindung muss mit „Zulassen/Ablehnen“ bestätigt werden (`Host::require_approval`). Die Ablehnung ist im Loopback getestet, den Dialog selbst hat noch niemand angeklickt.

## Stand 4. Oktober 2026 (Cloud-Sitzung, ohne Windows-Rechner)

Gebaut, aber **noch auf keinem Windows-Rechner ausgeführt**. Geprüft ist es nur durch Tests unter Linux, die Windows-Typprüfung (`scripts/check-windows.sh`) und Screenshots der Oberfläche mit nachgebildeter Tauri-API.

- **Dateiübertragung** in beide Richtungen mit eigenem Dateifenster (zwei Spalten), Ordnern, Drag & Drop, Abbruch und Fortschritt. Details in `docs/FILE-TRANSFER.md`. Im Dienstmodus laufen Dateizugriffe mit den Rechten des angemeldeten Benutzers, ohne Anmeldung gibt es keinen Zugriff.
- **Remote-Mauszeiger:** Der Host liest die Zeigerform aus DXGI (`Capturer::take_pointer`) und schickt sie als `HostMsg::Cursor`. Das Sitzungsfenster setzt sie als CSS-Cursor über das Bild (Paket-Typ 3). Invertierende Zeiger wie der Text-Cursor auf Weiß werden halbtransparent dunkel dargestellt.
- **Neu starten** im Tastenmenü der Sitzung, mit Bestätigung (`ViewerMsg::Restart`). Der Host-Prozess löst den Neustart aus, im Dienstmodus also SYSTEM. Mit installiertem Dienst ist das Gerät danach wieder erreichbar.
- **Bildqualität** in der Sitzungs-Toolbar (Schieberegler-Symbol): Schnell, Ausgewogen oder Scharf (`ViewerMsg::SetQuality`). Die Wahl ändert die Bitrate des Encoders und gilt bis zum Ende der Sitzung.

### Danach, ebenfalls am 4. Oktober (Cloud-Sitzung)

- **Fähigkeiten-Aushandlung** (`Features` als Anhang an Hello und Welcome): Ältere und neuere Versionen vertragen sich jetzt. Neue Nachrichten gehen nur an Gegenstellen, die sie kennen. Das Sitzungsfenster blendet Knöpfe aus, die das ferne Gerät nicht unterstützt. Regeln für neue Funktionen stehen in `docs/DIRECT.md`.
- **Direktverbindung:** Die Sitzung startet über den Server und wechselt dann ohne Unterbrechung auf eine direkte TCP-Verbindung (Port 21301), wenn das ferne Gerät erreichbar ist. Das gilt im LAN, mit IPv6 oder mit Portweiterleitung. Das Sitzungsfenster zeigt „Direkt“ oder „Über Server“ an. `ctxremote-service --install` legt die Firewall-Regel an. Details stehen in `docs/DIRECT.md`, getestet ist es unter Linux mit echtem Server (`crates/server/tests/sessions.rs`).

### Danach (weitere Cloud-Sitzung)

- **Automatische Bitratenregelung** (`crates/core/src/congestion.rs`):
  - Der Aufnahme-Thread misst, wie lange die Übergabe eines Bildes blockiert. Blockiert er mehr als 25 % einer Sekunde, sinkt die Bitrate auf 70 %, höchstens bis 20 %.
  - Nach drei ruhigen Sekunden steigt sie wieder, bis zur gewählten Qualitätsstufe.
  - Die Änderung kommt ohne neuen Keyframe aus (`VideoEncoder::set_bitrate_factor`).
- **Einstellungen der Direktverbindung** in der App (Ein/Aus, Port, zusätzliche Adressen):
  - Die Einstellungen wirken ohne Neustart.
  - Im Dienstmodus laufen sie über den erhöhten Helfer, der auch die Firewall-Regel anpasst.
  - Gespeichert wird nur, was sich geändert hat, damit nicht unnötig UAC-Abfragen kommen.
- **Start-Skript für Claude-Code-Cloud-Sitzungen** (`.claude/hooks/session-start.sh`): Es installiert die Linux-Bibliotheken für Tauri, führt `npm ci` aus, richtet das Windows-Target ein und lädt die Crates per `cargo fetch`. Es wirkt erst, wenn es im Standard-Branch liegt.

- **Chat** in beiden Richtungen (Fähigkeits-Bit `CHAT`):
  - Im Sitzungsfenster gibt es einen Chat-Knopf mit Zähler für ungelesene Nachrichten.
  - Im Hauptfenster hat jede gehostete Sitzung einen Chat-Knopf. In der Schnellhilfe ist der Chat immer sichtbar.
  - Eine eingehende Nachricht holt das Hauptfenster nach vorn.
  - Tippen im Chatfeld geht nicht an das ferne Gerät.
  - Der Verlauf liegt nur im Fenster und wird nicht gespeichert.

- **Release-Pipeline, Updates und Standardserver** (Details und Einrichtung in `docs/DEPLOY.md`):
  - Jeder Push auf `master` baut Server und Installer und liefert den Server per SSH auf den VPS aus, mit Rückfall auf die vorige Version, wenn der neue nicht startet.
  - Die Pipeline veröffentlicht außerdem ein signiertes Client-Update.
  - Installierte Clients holen Updates vom eigenen Server: mit Dienst automatisch, sobald niemand verbunden ist, ohne Dienst über einen Knopf in der App.
  - Release-Builds haben die Serveradresse eingebaut.
  - **Vor der ersten Nutzung** müssen Server, Secrets und Variablen eingerichtet werden (Abschnitt „Einrichtung Schritt für Schritt“).

### Erster Test durch den Nutzer (4. Oktober, abends)

- **Funktioniert:**
  - Auslieferung per Pipeline
  - Direktverbindung („Direkt verbunden über 10.10.0.11:…“)
  - Chat
- **Behoben:**
  - Das Dateifenster blieb weiß, und die App fror ein. Ursache war, dass `open_files` und `queue_drop` synchron ein Fenster gebaut haben, was unter Windows den Hauptthread blockiert. Beide sind jetzt `async`.
  - Ein vergrößertes Sitzungsfenster hat kleinere Fernbildschirme riesig hochskaliert. Jetzt wird standardmäßig nicht mehr vergrößert, umschaltbar im Menü „Bild“ (Schieberegler-Symbol).
- **Behoben, auch aus dem ersten Update-Test:** Das stille Update brach ab, und der Dienst blieb gestoppt. Ursache: Der Tauri-Installer will die laufende App per Restart Manager schließen. Aus der Dienstsitzung (SYSTEM, Sitzung 0) erreicht er die App in der Benutzersitzung nicht und bricht im stillen Modus wortlos ab. Jetzt gilt:
  - Der Installer-Hook beendet die App im stillen Modus per `taskkill`.
  - Der Installer läuft über `cmd /C "installer /S & sc start CTXRemote"`. Der Dienst kommt also auch nach einem Abbruch zurück.
  - Nach dem Update startet der Dienst die App für den angemeldeten Benutzer wieder, mit `--tray`, also ohne Fenster.
  - Die Fehlermeldung im Installer-Hook hat `/SD IDOK`, damit sie ein stilles Update nicht blockiert.
- **Offen:** Ob die Auflösung des fernen Geräts der Fenstergröße folgen soll (dynamische Auflösung wie bei RDP), ist noch nicht entschieden. Dafür müsste der Host seine Bildschirmauflösung ändern.

### Bestätigt durch den Nutzer (5. Oktober)

- Release-Pipeline, Server-Auslieferung und signierte Updates funktionieren. Ein Gerät mit Dienst hat sich automatisch auf 0.1.6 aktualisiert.
- Die Dateiübertragung funktioniert, nachdem `open_files` nicht mehr synchron läuft.
- Noch offen ist der Sprung 0.1.6 → nächste Version mit geöffneter App. Der neue Installer-Hook beendet die App, und der Dienst startet sie danach im Tray wieder.

### Öffentlicher Alias (5. Oktober, Cloud-Sitzung)

Wie bei AnyDesk kann jedes installierte Gerät einen eigenen Namen festlegen, unter dem andere es erreichen. Dynamische Auflösung hat der Nutzer verworfen.

- **Bedienung:** Hauptfenster → unter „Ihre ID“ auf „+ Alias festlegen“. Andere geben den Alias statt der ID ins Verbindungsfeld ein. Das Passwort bleibt nötig.
- **Regeln:** 3–32 Zeichen aus `a-z 0-9 . - _`, vorne und hinten Buchstabe oder Ziffer, mindestens ein Buchstabe (sonst wäre er mit einer ID verwechselbar). Großschreibung wird ignoriert. Ein Gerät hat höchstens einen Alias, ein neuer ersetzt den alten.
- **Technik:**
  - Der Server speichert die Aliase in `registry.json` (`aliases`) und bindet sie an den Geräteschlüssel. Das Setzen ist mit dem Geräteschlüssel signiert (`ctxremote/alias/v1:` + Verbindungs-Nonce + Alias), siehe `proto::rendezvous::sign_alias_claim`.
  - Neue Nachrichten `ClaimAlias`/`ResolveAlias` bzw. `AliasClaimed`/`AliasResolved`, angehängt. Alte Server legen auf: Die App meldet dann „Server-Update nötig“.
  - Der Alias steht zusätzlich in der Konfiguration (`public_alias`). Nach jeder Anmeldung am Server setzt ihn der Host erneut, falls der Server seine Daten verloren hat.
  - Mit Dienst läuft das Setzen über die UI-Pipe (`UiRequest::SetPublicAlias`), ohne UAC. Die Schnellhilfe hat keinen Alias, weil sie keine feste Identität hat.
  - Test: `cargo test -p ctxremote-server --test sessions public_alias`.

### Direktverbindung durch NAT (5. Oktober, Cloud-Sitzung)

UDP-Hole-Punching mit QUIC, parallel zum bisherigen TCP-Weg. Details in `docs/DIRECT.md`, Abschnitt „Durch NAT ohne Portweiterleitung“.

- Der Server hat einen UDP-Reflektor auf seinem Port (UDP 21300). **Nutzer muss in der Hetzner-Robot-Firewall UDP 21300 eingehend freigeben**, sonst wird der Weg still ausgelassen.
- Neue Nachrichten `HostMsg::PunchOffer` und `ViewerMsg::PunchAnswer`, Bit `Features::PUNCH`. `framing::Transport` läuft jetzt über beliebige Byte-Streams (`transport_over`), damit ein QUIC-Stream dieselbe Sitzung tragen kann.
- Neue Abhängigkeiten in `ctxremote-core`: `quinn` (ohne aws-lc), `rustls` mit `ring`, `rcgen`.
- `CTXREMOTE_DIRECT=udp` beim Viewer lässt TCP weg, zum Ausprobieren im LAN.
- Tests: `cargo test -p ctxremote-core punch` und `cargo test -p ctxremote-server --test punch`. Die gemeinsamen Testhelfer liegen jetzt in `crates/server/tests/common/mod.rs`.

### Helfer-Profil (5. Oktober, Cloud-Sitzung)

Wunsch des Nutzers: Was der Endnutzer beim Verbinden sieht, soll sich personalisieren lassen, für jeden Kunden eigen. Umgesetzt ist das **Helfer-Profil**. Den eigenen gebrandeten Client und die Pläne beschreibt das Konzept `docs/PLANS.md`. Dort warten offene Entscheidungen auf den Nutzer.

- **Festlegen:** Einstellungen → „Ihr Profil“. Felder: Name, Firma, Nachricht, Logo. Das Logo wird in der App auf höchstens 128 px verkleinert, als PNG mit höchstens 64 KB. Eine Vorschau zeigt, wie es die Gegenseite sieht. Gespeichert wird in der Benutzer-Konfiguration (`profile`), ohne UAC, auch im Dienstmodus.
- **Anzeige beim Host:**
  - In der Zugriffsanfrage der Schnellhilfe erscheint eine Profilkarte mit Logo, Name, Firma, Nachricht und dem technischen Gerätenamen. Dazu der Hinweis, dass die Angaben nicht geprüft sind.
  - In „Verbunden mit …“ und im Banner „… steuert dieses Gerät“ des Hauptfensters steht der Profilname, mit Logo.
- **Protokoll:**
  - Das Profil reist im `Hello`-Anhang als `HelloExtras { features, profile }`. `features` steht vorne, deshalb lesen ältere Hosts weiter nur die Features (Test `hello_extras_are_compatible_both_ways`).
  - Neues Bit `Features::PROFILE`, nur zur Information.
  - Der Host säubert alles (`HelperProfile::sanitized`): Längen, Steuerzeichen, Logo nur als PNG bis 64 KB.
- **Dienstmodus:** `HostEvent::SessionStarted.profile` und `ServiceState.session_profiles`, beide mit `serde(default)`.
- Test mit echtem Server: `helper_profile_reaches_the_host`.

### Website (5. Oktober, Cloud-Sitzung)

- **Inhalt:** Statische Seiten in `website/`, ohne JavaScript und ohne fremde Inhalte. Gestaltet wie die App, mit Dark Mode und für Handys geeignet.
  - Start mit Funktionen und App-Screenshot (hell und dunkel)
  - Download: Installer und Schnellhilfe, mit SmartScreen-Hinweis
  - Preise: Free privat kostenlos, Pro und Ultra „bald verfügbar“, ohne Preise
  - Impressum und Datenschutz als Vorlagen mit **markierten Platzhaltern** (`<span class="todo">`), die der Nutzer füllen muss
- **Adresse und Server:** https://ctxremote.ctx.ink (**online seit 5. Oktober**, hinter dem Cloudflare-Proxy) auf dem Hetzner-Server, ausgeliefert vom Caddy-Docker-Container des Nutzers. Nicht `remote.ctx.ink`, weil dort das Cloudflare-Origin-Zertifikat für `*.ctx.ink` greifen würde (siehe DEPLOY.md). Einrichtung einmalig mit `deploy/setup-website.sh`, siehe `docs/DEPLOY.md`, Abschnitt „Website“.
  - **Caddy des Nutzers läuft in Docker** mit weiteren Seiten. Deshalb `setup-website.sh --docker`: Es legt nur den Ordner an und gibt Volume und Caddy-Block aus. Ohne `--docker` bricht das Skript ab, wenn die Ports fremd belegt sind.
  - **Offen beim Nutzer:** Platzhalter in Impressum und Datenschutz füllen, AVV mit Hetzner abschließen.
- **Pipeline:** `release.yml` packt `website/`, die Version und die Downloads unter festen Namen dazu. `remote-deploy.sh` schaltet die Seite per Symlink atomar um, aber nur, wenn `/var/www/ctxremote` existiert.
- **Selbst-Hosting** bietet die Seite bewusst nicht an (Entscheidung des Nutzers).
- Die App-Screenshots in `website/img/` stammen aus dem Frontend mit nachgebauter Tauri-API. Bei sichtbaren UI-Änderungen neu erzeugen.

### UDP-Weg auch für die Schnellhilfe (5. Oktober, Cloud-Sitzung)

Die Schnellhilfe hatte die Direktverbindung ganz abgeschaltet, weil der TCP-Listener eine Firewall-Abfrage auslösen würde. Gerade beim typischen Fall (helfen bei jemandem zu Hause) lief deshalb alles über den Server. Jetzt läuft sie mit `direct: true, direct_listen: false`: kein TCP-Port, aber der UDP-Weg durch NAT. Siehe `docs/DIRECT.md`.

### Konten und gemeinsame Geräteliste (5. Oktober, Cloud-Sitzung)

Ohne E-Mail und Passwort: Ein Konto ist eine Menge von Geräteschlüsseln, neue Geräte kommen über einen Einmal-Code dazu. Die Geräteliste wird Ende-zu-Ende verschlüsselt abgeglichen. Alles Weitere in `docs/ACCOUNTS.md`.

- **Bedienung:** Einstellungen → „Konto und Geräteliste“: „Konto anlegen“, „Gerät hinzufügen“ (zeigt den Code), „Mit Code verbinden“, „Dieses Gerät vom Konto abmelden“.
- Server-Update nötig, kommt mit dem Release.
- Neue Felder in `config.json`: `account`, `removed`, `peers[].alias_at`.
- Tests: `accounts::tests` im Server und im Kern, sowie `--test accounts` mit echtem Server.

### Verschlüsselte Verbindung zum Server (5. Oktober, Cloud-Sitzung)

Wunsch des Nutzers: Konten in App und Website, Geräteverwaltung im Web, alles Ende-zu-Ende verschlüsselt. Plan in 4 Schritten (Details in `docs/ACCOUNTS.md`, Abschnitt „Ausbau“):
1. Verschlüsselte Verbindung App ↔ Server: **erledigt**
2. Anmeldung mit E-Mail und Passwort, mit Wiederherstellungscode: **erledigt** (App: Personen-Symbol oben rechts → Konto-Panel; der Kontobereich ist aus den Einstellungen dorthin umgezogen)
3. Webinterface: **erledigt**, https://ctxremote.ctx.ink/konto/ (`web/`, Svelte; Server-API `crates/server/src/web.rs`). Die Oberfläche hat ein Sonnet-Agent nach Vorgabe gebaut, Krypto und Sitzungslogik (`web/src/lib/crypto.ts`, `session.ts`) sind selbst geschrieben.
   - Interop im echten Browser gegen echten Server getestet: Registrieren im Browser, App koppelt sich mit dem Browser-Code und meldet sich mit dem Browser-Passwort an. Gerätenamen und Geräteliste aus der App erscheinen im Browser, Umbenennen im Browser kommt in der App an.
   - `npm test` in `web/` prüft die Browser-Krypto gegen feste Werte aus Rust (`vectors_stay_stable`).
   - **Erledigt beim Nutzer:** Web-API für Caddy in Docker freigeschaltet.
4. Mails per SMTP: **erledigt** (`crates/server/src/mail.rs`, lettre mit rustls/ring; der statische Build braucht `musl-tools`, steht in `release.yml`). Bestätigung der Adresse (Link `/konto/#bestaetigen=…`, 48 h), Hinweise bei neuer Adresse, neuem Passwort, neuer Anmeldung und benutztem Wiederherstellungscode. **Erledigt beim Nutzer:** `/etc/ctxremote/mail.env` angelegt, Mails kommen an.

### GitHub Actions (5. Oktober)

Das Repository ist **öffentlich** (AGPL-3.0, `LICENSE`), damit kosten die Standard-Runner nichts. Vorher waren 2700 von 3000 Minuten verbraucht.
- **CI** (`ci.yml`) läuft bei jedem Push auf Branches und bei Pull Requests: Windows und Linux, dazu das Webinterface.
- **Release** startet nicht bei reinen Doku-Änderungen (`paths-ignore`).
- **Keine selbst gehosteten Runner** verwenden: Bei einem öffentlichen Repository könnten fremde Pull Requests darauf Code ausführen.
- Wird das Repository wieder privat, zählen die Minuten wieder (Windows doppelt, macOS zehnfach). Dann CI wieder auf Pull Requests und Linux beschränken.

Zu Schritt 1:
- **Technik:** Noise NK mit festem Server-Schlüssel (`tunnel.key`), Datensatz-Schicht `SecureIo` unter dem Framing (`crates/proto/src/tunnel.rs`). Der Server leitet Sitzungen weiter, indem er entschlüsselt und wieder verschlüsselt. Die Inhalte bleiben dabei Ende-zu-Ende verschlüsselt.
- **Übergang:** Unverschlüsselte Clients werden noch angenommen.
- **Tests:** Alle Tests mit echtem Server laufen verschlüsselt (fester Testschlüssel in `tests/common`). `unencrypted_clients_still_work` prüft den Übergang.
- **Erledigt beim Nutzer** (seit mehreren Releases): `CTXREMOTE_SERVER_KEY` ist als GitHub-Variable eingetragen, die ausgelieferten Clients verschlüsseln. UDP 21300 ist in der Hetzner-Firewall offen.

### Eigenes Setup-Fenster (5. Oktober, Cloud-Sitzung)

Der Download auf der Website ist jetzt `CTXRemote-Setup.exe` aus `crates/setup` statt des NSIS-Assistenten („Weg 2“):
- **Aussehen:** Ein randloses Fenster im App-Design mit Logo, Titel, Text, einem Knopf und einem Fortschrittsbalken. Es hat einen echten Dark Mode und folgt dem Windows-Theme. Es zeichnet sich selbst mit winit, softbuffer, tiny-skia und fontdue mit Segoe UI. Es braucht also kein WebView und läuft auch dort, wo WebView2 noch fehlt.
- **Ablauf:**
  - Das Fenster läuft als Benutzer (`asInvoker`).
  - „Installieren“ entpackt den eingebetteten NSIS-Installer in einen Temp-Ordner und startet ihn per UAC mit `/S`.
  - Danach kommt „CTXRemote starten“, die App startet dann als Benutzer und nicht als Administrator.
  - UAC abgelehnt: Hinweis, „Installieren“ bleibt.
  - NSIS-Fehlercode: Fehlerseite mit „Erneut versuchen“.
- **Fortschritt:** NSIS meldet keinen. Der Balken nähert sich geschätzt 90 % und springt am Ende auf fertig.
- **Erkennt vorhandene Installationen** über den Uninstall-Eintrag in der Registry (`DisplayVersion`, `InstallLocation`, `MainBinaryName`):
  - ältere Version: „Aktualisieren“
  - gleiche Version: „Erneut installieren“ oder „CTXRemote starten“
  - neuere Version: nur „CTXRemote starten“
- **Für Administratoren:** `CTXRemote-Setup.exe /S` installiert ohne Fenster.
- **Deinstallation** läuft weiter über „Apps & Features“, also über den NSIS-Uninstaller.
- **Updates** nutzen weiter den NSIS-Installer direkt.
- **Pipeline:** `release.yml` baut das Fenster nach dem NSIS-Installer mit `CTXREMOTE_SETUP_PAYLOAD`, auf der Website liegt es als `download/CTXRemote-Setup.exe`.
- **Ohne Payload**, also bei Entwickler-Builds, wird die Installation nur simuliert.
- **Design prüfen ohne Windows:** `CTXRemote-Setup --preview <ready|update|same|newer|installing|done|failed|declined> bild.png [dark] [primary|secondary|close]` rendert einen Zustand als PNG.

### Zugriff ohne Passwort und Konto löschen (5. Oktober, Cloud-Sitzung)

- **Zugriff ohne Passwort für Geräte des eigenen Kontos:** vom Nutzer gewünscht, wie bei AnyDesk/RustDesk. Ein Schalter im Kontopanel, Standard aus. Sicherheitsmodell und Technik stehen in `docs/ACCOUNTS.md`, „Zugriff ohne Passwort“. Kurz: Kontoschlüssel (SPAKE2-Slot) **und** Mitgliedschaft laut Server (signierter Nachweis, `SameAccount`). Mit Dienst geht der Schalter über UAC. Freigegebene Geräte tragen in der Geräteliste das Label „ohne Passwort“.
- **Konto löschen** selbst im Webinterface, mit Passwort. Die Apps lösen ihre Verknüpfung beim nächsten Abgleich.
- **Webinterface:** Die Anmeldung übersteht Neuladen und gilt über Tabs hinweg (`localStorage`, maskiert mit dem `pad` der Server-Sitzung).
- **Mails** tragen `MIME-Version` und `Content-Type` (vorher zeigten manche Programme Quoted-Printable roh).
- Die App-Oberfläche hat ein Agent nach Vorgabe gebaut. Protokoll, Krypto, Host- und Viewer-Logik habe ich selbst geschrieben. Getestet: `--test access` mit echtem Server (Mitglied rein; Fremder mit Kontopasswort, ohne Nachweis, fremdes Host-Passwort und entferntes Gerät abgewiesen), `--test web` (Löschen), Browser-Durchläufe für Neuladen, Tabs und Löschen.

### Verbindungsprotokoll (5. Oktober, Cloud-Sitzung)

- Jeder Host führt einen **Verlauf** der eingehenden Verbindungen (`crates/core/src/history.rs`). Er hält fest: Start, Ende, Gegenstelle mit Profilnamen und den Weg (Einmalpasswort, festes Passwort, Konto). Abgewiesene Versuche stehen auch drin: falsches Passwort, kein Kontomitglied, am Gerät abgelehnt.
- Datei neben der Config (`config.history.json`, beim Dienst also im geschützten Ordner des Dienstes), höchstens 200 Einträge, nichts geht an den Server.
- App: Icon „Verlauf“ neben dem Konto-Knopf. Mit Dienst holt die App den Verlauf über die UI-Pipe (`UiRequest::History`). Die Oberfläche hat ein Agent gebaut.
- Test: `--test access` prüft die Einträge.

### Sicherung der Serverdaten (5. Oktober, Cloud-Sitzung)

- `deploy/ctxremote-backup.sh` mit Timer (`ctxremote-backup.timer`, täglich 3:30) und einmaliger Einrichtung `deploy/setup-backup.sh`. Gesichert wird der Datenordner ohne `updates/`. Verschlüsselt wird mit AES-256 und einer Passphrase in `/etc/ctxremote/backup.key`, die sicher verwahrt werden muss. Aufbewahrt wird 14 Tage, optional mit rsync auf eine Hetzner Storage Box (nur verschlüsselt). `--restore` spielt eine Sicherung ein und bewahrt den alten Stand auf.
- Lokal getestet: Sicherung, Verschlüsselung, Kopie per rsync, Aufbewahrung, Wiederherstellen, falsche Passphrase.
- **Offen beim Nutzer:** einrichten nach `docs/DEPLOY.md`, „Sicherung der Serverdaten“, Passphrase im Passwortmanager ablegen, optional Storage Box.

### Testliste für den nächsten Windows-Termin

1. `node app/scripts/prepare-service.mjs`, dann App und Dienst wie gewohnt bauen. CI muss auf Windows und Linux grün sein.
2. Dateien, **ohne Dienst**:
   - Hochladen und Herunterladen einer Datei und eines Ordners mit Unterordnern
   - Ein zweites Mal übertragen: Ziel `Name (2)`
   - Großen Download abbrechen: Es darf keine `.ctxpart`-Datei übrig bleiben.
3. Dateien, **mit Dienst**:
   - Gleiche Liste wie bei Punkt 2.
   - Gehören die angelegten Dateien dem angemeldeten Benutzer (Eigenschaften → Sicherheit) und nicht SYSTEM?
   - Am Sperrbildschirm funktionieren Dateien. Vor jeder Anmeldung muss eine klare Fehlermeldung kommen.
   - Stimmen die Orte (Desktop, Dokumente, Downloads), auch bei OneDrive-Umleitung?
4. Dateien aus dem Explorer auf das Sitzungsfenster ziehen: Sie müssen auf dem fernen Desktop landen, und das Dateifenster muss sich öffnen.
5. Mauszeiger: Text-Cursor, Größenänderungs-Pfeile und Sanduhr müssen im Viewer sichtbar sein. Was passiert bei 150 % Skalierung?
6. Neu starten mit Dienst: Das Gerät startet neu und ist danach wieder online.
7. Bildqualität umschalten: Das Bild muss weiterlaufen, und bei „Scharf“ ist Text sichtbar schärfer.
8. Direktverbindung im LAN:
   - Den Dienst neu installieren, damit die Firewall-Regel angelegt wird.
   - Nach dem Verbinden muss das Sitzungsfenster nach etwa 1 s „Direkt“ zeigen.
   - Bild, Maus, Tastatur und Dateien müssen danach normal weiterlaufen.
   - Gegenprobe ohne Firewall-Regel: Dort muss „Über Server“ stehen bleiben, ohne Abbruch.
9. Alte Version gegen neue: Ein Host mit dem Stand vom 2. Oktober muss sich mit dem neuen Viewer bedienen lassen. Die neuen Knöpfe sind dort ausgeblendet.
10. Einstellungen, Bereich Direktverbindung, mit Dienst:
    - Den Port ändern: Es muss eine UAC-Abfrage kommen, danach „Aktiv auf Port X“.
    - Die Firewall-Regel muss den neuen Port haben (`wf.msc`).
    - Abschalten muss „Aus“ zeigen, und neue Sitzungen bleiben dann „Über Server“.
11. Bitratenregelung: Eine Sitzung über eine gedrosselte Leitung, z. B. einen Handy-Hotspot oder die Netzwerkdrosselung in einer VM. Das Bild muss flüssig bleiben, statt immer weiter hinterherzuhinken. Im Log stehen „Bitrate angepasst“-Zeilen (`RUST_LOG=debug`).
12. Chat in drei Varianten:
    - App gegen App
    - App gegen Dienst: Der Chat muss über die UI-Pipe im Hauptfenster ankommen.
    - App gegen Schnellhilfe

    Dabei prüfen:
    - Steht das Hauptfenster im Tray, muss es sich bei einer Nachricht öffnen.
    - Kein Buchstabe aus dem Chatfeld darf beim fernen Gerät ankommen.
    - Gegen einen alten Host muss der Chat-Knopf fehlen.
13. Öffentlicher Alias (braucht das Server-Update aus derselben Version):
    - Im Hauptfenster einen Alias festlegen, mit und ohne Dienst. Er muss danach unter der ID stehen und sich kopieren lassen.
    - Von einem zweiten Gerät mit dem Alias verbinden, auch in Großbuchstaben.
    - Denselben Alias auf dem zweiten Gerät versuchen: „Dieser Alias ist schon vergeben“.
    - Alias entfernen: Danach muss „Den Alias … gibt es nicht“ kommen.
14. Direktverbindung durch NAT (vorher UDP 21300 in der Server-Firewall freigeben):
    - Im LAN: Viewer mit `CTXREMOTE_DIRECT=udp` starten (PowerShell: `$env:CTXREMOTE_DIRECT="udp"; & "C:\Program Files\CTXRemote\ctxremote.exe"`). Das Sitzungsfenster muss nach wenigen Sekunden „Direkt“ zeigen. Kommt eine Firewall-Abfrage von Windows?
    - Echt: Viewer im Handy-Hotspot, Host zu Hause, ohne Portweiterleitung. Erwartet: „Direkt“ über die öffentliche Adresse des Routers.
    - Bild, Dateien und Chat laufen danach normal, auch nach 5 Minuten ohne Bildänderung (Keep-Alive).
16. Schnellhilfe direkt: Mit der App auf eine Schnellhilfe in einem anderen Netz verbinden, z. B. über einen Handy-Hotspot. Nach wenigen Sekunden sollte „Direkt“ im Sitzungsfenster stehen. **Kommt bei der Schnellhilfe eine Windows-Firewall-Abfrage?** Das sollte nicht passieren, weil sie nur antwortet und keinen Port öffnet.
18. Anmeldung (erst nachdem `CTXREMOTE_SERVER_KEY` eingetragen und neu gebaut ist, sonst kommt „nicht verschlüsselt“):
    - Auf PC A im Konto-Panel registrieren und den Wiederherstellungscode notieren.
    - Auf PC B mit E-Mail und Passwort anmelden.
    - Die Geräteliste in „Geräte im Konto“ muss beide Geräte mit Computernamen und Online-Punkt zeigen.
    - Ein falsches Passwort muss abgelehnt werden.
    - „Passwort vergessen?“ mit dem notierten Code ausprobieren.
17. Konten: Auf PC A „Konto anlegen“, dann „Gerät hinzufügen“. Auf PC B „Mit Code verbinden“ und den Code eingeben.
    - Danach muss die Geräteliste beider PCs zusammengeführt sein.
    - Ein Gerät auf A umbenennen: Spätestens nach wenigen Sekunden muss der neue Name auf B stehen, eventuell nach erneutem Öffnen des Fensters.
    - Ein Gerät auf B entfernen: Es muss auch auf A verschwinden.
    - Mit Dienst auf einem der PCs wiederholen.
19. Setup-Fenster (von der Website herunterladen):
    - Auf einem Rechner ohne CTXRemote: Fenster, Schrift und Dark Mode prüfen, bei 100 % und 150 % Skalierung.
    - „Installieren“, dann muss die UAC-Abfrage kommen. Ablehnen: Hinweis, nochmal klicken, zustimmen. Danach kommt der Balken und schließlich „CTXRemote ist bereit“.
    - „CTXRemote starten“: Die App muss **ohne** Administratorrechte laufen (Task-Manager, Spalte „Erhöht“).
    - Erneut starten: „CTXRemote ist installiert“ mit „Erneut installieren“.
    - Fenster ziehen, Esc schließt, Enter drückt den Hauptknopf.
    - Deinstallation über „Apps & Features“ muss weiter funktionieren.
    - Wie lange dauert die Installation wirklich? Danach den Schätzwert in `crates/setup/src/ui.rs` (`estimated_progress`) anpassen.
20. Zugriff ohne Passwort (zwei PCs im selben Konto, beide aktualisiert):
    - Auf PC A im Kontopanel „Geräte dieses Kontos dürfen sich ohne Passwort …“ einschalten. Mit Dienst muss eine UAC-Abfrage kommen.
    - Auf PC B muss PC A nach dem Abgleich (spätestens nach 5 Minuten oder nach Neustart der App) das Label „ohne Passwort“ tragen.
    - Klick auf PC A: Die Sitzung startet ohne Passwortdialog.
    - Auf A ausschalten: B fällt auf den Passwortdialog zurück, mit Hinweis.
    - PC B im Webinterface aus dem Konto entfernen: B kommt nicht mehr ohne Passwort hinein („Dieses Gerät gehört nicht mehr zum Konto“).
21. Konto löschen im Webinterface: Danach muss die App auf beiden PCs beim nächsten Abgleich ohne Konto dastehen, ohne Fehlermeldung.
22. Verlauf: Mit und ohne Dienst je eine Verbindung mit Einmalpasswort und eine mit falschem Passwort machen. Im Verlauf müssen beide stehen, die erfolgreiche mit Dauer. Nach einem Neustart der App bzw. des Dienstes muss der Verlauf noch da sein.
15. Helfer-Profil:
    - In den Einstellungen Name, Firma, Nachricht und ein Logo setzen, z. B. ein großes JPG. Die Vorschau muss stimmen.
    - Mit der Schnellhilfe verbinden: Die Zugriffsanfrage zeigt die Profilkarte, danach steht „Verbunden mit <Profil>“ dort.
    - Mit einem Gerät mit Dienst verbinden: Das Banner im Hauptfenster zeigt Profilname und Logo.
    - Profil leeren und speichern: Danach erscheint wieder nur „benutzer (PC)“.

## Nächste Schritte

0. Die Pipeline einrichten (`docs/DEPLOY.md`, Abschnitt „Einrichtung Schritt für Schritt“), dann den Branch nach `master` mergen. Den ersten Installer von Hand installieren, danach zweimal pushen und prüfen, ob sich ein Gerät selbst aktualisiert.
1. Die Testliste oben abarbeiten und den Zustimmungsdialog der Schnellhilfe testen.
2. Hardware-Encoder, macOS-Host (zum Testen ist ein Mac nötig).
3. Adressbuch und Geräteverwaltung über den Server, Code-Signatur (braucht ein Konto bei Microsoft Trusted Signing).
4. Bezahlte Pläne nach `docs/PLANS.md`. Erst wenn es eine Firma gibt. Schritt 1 (Konten und Adressbuch) ginge auch vorher schon. Konten gehören auf die Website (ctxremote.ctx.ink), später mit Stripe.

Der Linux-Host (X11, später Wayland) ist zurückgestellt und kommt später.

Weitere Roadmap: Remote-Mauszeiger, Adressbuch auf dem Server.

## Ideen für später

## Bekannte Kompromisse

- Das feste Passwort liegt im Klartext, weil SPAKE2 das Passwort selbst braucht. Mit Dienst steht es in `C:\ProgramData\CTXRemote\host.json`, lesbar nur für SYSTEM und Administratoren. Ohne Dienst steht es in der Benutzerkonfiguration. Möglich wäre später DPAPI mit Maschinenbindung.
- Direktverbindungen gibt es nur per TCP: im LAN, mit IPv6 oder mit Portweiterleitung. Zwischen zwei IPv4-NATs bleibt die Sitzung auf dem Relay.

## Entwicklungsumgebung (Windows)

- Rust (MSVC) und Visual Studio Build Tools mit C++-Workload. Ist „Programme“ auf ein anderes Laufwerk umgelegt, braucht der Installer einen kurzen Pfad: `--installPath C:\VS\BuildTools`.
- Node.js ≥ 20, im Ordner `app` einmal `npm install` ausführen.
- Tests unter Linux per Docker: siehe `README.md`.
- Ohne Windows-Rechner (z. B. in einer Claude-Code-Cloud-Sitzung): `scripts/check-windows.sh` prüft den Windows-Code unter Linux auf Typfehler (MSVC-Attrappen statt Build). Die Oberfläche lässt sich mit Playwright und nachgebildetem `window.__TAURI_INTERNALS__` gegen `app/dist` per Screenshot prüfen.

## Gestaltung

Ruhig und typografisch, ausdrücklich nicht im typischen KI-Look:

- warme Neutraltöne und ein echter Dark Mode
- eine einzige Akzentfarbe, Petrol (`#0d5c4b`, im Dark Mode `#4fb495`)
- Haarlinien statt Schatten
- Segoe UI Variable als Schrift
- eigene Strich-Icons mit 1,5 px

Keine Verläufe, kein Glas-Effekt, keine Emojis.
