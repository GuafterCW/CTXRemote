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
- CI grün auf Windows, Linux und macOS. Damit ist auch der macOS-Build bestätigt.
- Loopback auf dem neuen Entwicklungsrechner: Sitzung nach 12 ms, erster 2560×1440-Keyframe nach 181 ms, zwei Monitore erkannt.
- GUI-Sitzung über das Netz (Viewer: Entwicklungsrechner, Host: Windows Server, Server im LAN): Verbindung, Bild, Maus, Tastatur und Aliase funktionieren.

**Noch nicht getestet:** der Monitorwechsel in einer GUI-Sitzung, weil der Test-Host nur einen Monitor hatte. Zwei Instanzen auf *einem* PC eignen sich nur, um Verbindung und Bild zu prüfen. Der Host bewegt dann die eigene Maus, und Tastatureingaben können sich im Kreis drehen.

## Offene Entscheidungen und To-dos

1. **Code-Signatur:** Smart App Control blockiert unsignierte Builds (os error 4551). Kandidat ist Microsoft Trusted Signing für etwa 10 $ im Monat.
2. **Monitorwechsel testen**, mit einem Host, der mehrere Monitore hat.

## In Arbeit (Stand 2. Oktober 2026, abends, noch nicht committet)

- **Zwischenablage** (Text, beide Richtungen): fertig und auf dem Windows Server getestet.
- **Windows-Dienst** (`crates/service`, Entwurf in `docs/WINDOWS-SERVICE.md`): Die Schritte 1, 2, 3 und 5 sind umgesetzt und auf dem Windows Server getestet: Sperrbildschirm, Strg+Alt+Entf, UAC sowie Abmelden und Anmelden ohne Abbruch der Sitzung. „Sperren“ ruft `LockWorkStation` auf, weil Windows ein eingespieltes Win+L ignoriert. `SendSAS` kommt aus dem Dienstprozess, weil Windows es von SYSTEM-Prozessen ignoriert. Offen sind noch die UI-Pipe mit Anbindung der App (Schritt 4) und der Installer (Schritt 6).
- **Schnellhilfe „CTXRemote Hilfe“**: eine portable EXE (`npm run build:quick` mit `CTXREMOTE_QUICK_SERVER`). Die Konfiguration ist flüchtig, jede Verbindung muss mit „Zulassen/Ablehnen“ bestätigt werden (`Host::require_approval`). Die Ablehnung ist im Loopback getestet, der Dialog selbst noch nicht.

## Nächste Schritte

1. Windows-Dienst fertigstellen: UI-Pipe und App-Anbindung, dann der Installer.
2. Dateiübertragung
3. Danach P2P, Hardware-Encoder und macOS-Host (zum Testen ist ein Mac nötig).

Der Linux-Host (X11, später Wayland) ist zurückgestellt und kommt später.

Weitere Roadmap: P2P-Hole-Punching, Remote-Mauszeiger, Hardware-Encoder, Adressbuch auf dem Server.

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
