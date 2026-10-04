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

## Nächste Schritte

1. Die Testliste oben abarbeiten und den Zustimmungsdialog der Schnellhilfe testen.
2. Direktverbindung durch NAT ohne Portweiterleitung (UDP-Hole-Punching, siehe `docs/DIRECT.md`), Hardware-Encoder, macOS-Host (zum Testen ist ein Mac nötig).
3. Einstellungsoberfläche für `direct`, `direct_port` und `direct_addresses`.

Der Linux-Host (X11, später Wayland) ist zurückgestellt und kommt später.

Weitere Roadmap: Remote-Mauszeiger, Adressbuch auf dem Server.

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
