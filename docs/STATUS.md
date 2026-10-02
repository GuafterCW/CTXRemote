# Projektstand CTXRemote

Stand: 2. Oktober 2026. Übergabenotiz, damit die Arbeit auf einem anderen Rechner nahtlos weitergehen kann.

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

**Noch nicht getestet:** eine volle Sitzung in der GUI mit Sitzungsfenster, Maus und Tastatur, die Alias-Oberfläche in der laufenden App und der macOS-Build. Der Grund ist Smart App Control auf dem bisherigen Entwicklungsrechner (siehe unten).

## Offene Entscheidungen und To-dos

1. **Erster Commit und Push** auf GitHub. Danach läuft die CI, die auch den macOS-Build prüft.
2. **Code-Signatur:** Smart App Control blockiert unsignierte Builds (os error 4551). Kandidat ist Microsoft Trusted Signing für etwa 10 $ im Monat.
3. **GUI-Sitzung testen:** zwei Instanzen auf einem PC, eine davon mit `CTXREMOTE_CONFIG=<andere.json>`.

## Nächste Schritte (vereinbarte Reihenfolge)

1. Linux-Host für X11 (XShm/XTest)
2. macOS-Host (ScreenCaptureKit, CGEvent; Bildschirmaufnahme- und Bedienungshilfen-Rechte)
3. Wayland (PipeWire und Desktop-Portale)

Weitere Roadmap: Zwischenablage (Protokoll vorhanden), Dateiübertragung, Windows-Dienst (Anmeldebildschirm, UAC, Strg+Alt+Entf), P2P-Hole-Punching, Remote-Mauszeiger, Hardware-Encoder, Adressbuch auf dem Server.

## Bekannte Kompromisse

- Das feste Passwort liegt im Klartext in der Benutzerkonfiguration, weil SPAKE2 das Passwort selbst braucht. Es soll später in den Windows-Anmeldeinformationsspeicher.
- Verbindungen laufen noch nur über das Relay, eine direkte P2P-Verbindung fehlt.

## Entwicklungsumgebung (Windows)

- Rust (MSVC) und Visual Studio Build Tools mit C++-Workload. Ist „Programme“ auf ein anderes Laufwerk umgelegt, braucht der Installer einen kurzen Pfad: `--installPath C:\VS\BuildTools`.
- Node.js ≥ 20, im Ordner `app` einmal `npm install` ausführen.
- Tests unter Linux per Docker: siehe `README.md`.

## Gestaltung

Ruhig und typografisch, ausdrücklich nicht im typischen KI-Look:

- warme Neutraltöne und ein echter Dark Mode
- eine einzige Akzentfarbe, Petrol (`#0d5c4b`, im Dark Mode `#4fb495`)
- Haarlinien statt Schatten
- Segoe UI Variable als Schrift
- eigene Strich-Icons mit 1,5 px

Keine Verläufe, kein Glas-Effekt, keine Emojis.
