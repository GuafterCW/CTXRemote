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

### Testliste für den nächsten Windows-Termin

1. `node app/scripts/prepare-service.mjs`, dann App und Dienst wie gewohnt bauen. CI muss auf allen drei Systemen grün sein.
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

## Nächste Schritte

0. Die Pipeline einrichten (`docs/DEPLOY.md`, Abschnitt „Einrichtung Schritt für Schritt“), dann den Branch nach `master` mergen. Den ersten Installer von Hand installieren, danach zweimal pushen und prüfen, ob sich ein Gerät selbst aktualisiert.
1. Die Testliste oben abarbeiten und den Zustimmungsdialog der Schnellhilfe testen.
2. Direktverbindung durch NAT ohne Portweiterleitung (UDP-Hole-Punching, siehe `docs/DIRECT.md`), Hardware-Encoder, macOS-Host (zum Testen ist ein Mac nötig).
3. Adressbuch und Geräteverwaltung über den Server, Code-Signatur (braucht ein Konto bei Microsoft Trusted Signing).

Der Linux-Host (X11, später Wayland) ist zurückgestellt und kommt später.

Weitere Roadmap: Remote-Mauszeiger, Adressbuch auf dem Server.

## Ideen für später

- **Eigenes Setup-Fenster (vom Nutzer gewählt: Weg 2):** Der NSIS-Assistent von Tauri sieht im Originalzustand nach Windows XP aus. Statt ihn nur mit Bildern aufzuhübschen (Weg 1, verworfen) soll ein eigenes Setup-Programm im App-Design entstehen:
  - **Aussehen:** Logo, Text, ein Knopf „Installieren“ und ein Fortschrittsbalken, wie bei Discord oder Spotify. Gestaltung nach den Regeln unten.
  - **Technik (Vorschlag):**
    - eine kleine eigene Rust-Anwendung (Tauri-Fenster oder natives Fenster), die den NSIS-Installer eingebettet mitbringt
    - sie startet diesen per UAC mit `/S` unsichtbar und zeigt den Fortschritt an
    - Abschlussseite mit „CTXRemote starten“
  - **Pipeline:** `release.yml` baut das Setup-Programm zusätzlich und legt es als Artefakt ab, z. B. `CTXRemote-Setup.exe`. Für automatische Updates bleibt der NSIS-Installer zuständig, sie laufen ja still.
  - **Offen:**
    - Fortschrittsanzeige: NSIS meldet keinen Fortschritt nach außen. Entweder Zwischenschritte schätzen oder die Dateien selbst kopieren statt NSIS zu nutzen.
    - Deinstallation über „Apps & Features“ muss weiter funktionieren.

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
