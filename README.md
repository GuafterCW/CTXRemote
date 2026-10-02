# CTXRemote

Selbst gehosteter Fernzugriff für eigene Geräte, eine Alternative zu RustDesk, TeamViewer & Co.

- **Ende-zu-Ende-verschlüsselt:** Viewer und Host einigen sich per SPAKE2 auf Schlüssel, die vom Passwort abgeleitet sind (ChaCha20-Poly1305). Der Server leitet nur verschlüsselte Bytes weiter und kann das Passwort nicht prüfen oder erraten, ohne dass der Host es merkt.
- **Feste Geräte-ID:** Jedes Gerät besitzt einen Ed25519-Schlüssel. Die neunstellige ID gehört dauerhaft diesem Schlüssel und lässt sich nicht übernehmen.
- **Einmal-Passwort** pro Sitzung, optional ein **festes Passwort** für unbeaufsichtigten Zugriff. Nach 5 Fehlversuchen sperrt der Host für 5 Minuten.
- **H.264-Bildübertragung** (OpenH264) mit DXGI-Bildschirmaufnahme. Dekodiert wird im Viewer hardwarebeschleunigt per WebCodecs.
- **Aliase:** Geräte lassen sich benennen („Büro-PC“) und im Verbindungsfeld per Alias statt ID finden.
- **Physische Tastatur:** Übertragen werden Scancodes statt Zeichen, Tastaturlayouts funktionieren daher auf beiden Seiten korrekt.

## Aufbau

| Pfad | Inhalt |
|---|---|
| `crates/proto` | Protokoll, Framing, Handshake und Verschlüsselung |
| `crates/server` | `ctxremote-server`: ID-Vergabe, Vermittlung und Relay |
| `crates/core` | Engine: Bildschirmaufnahme, Encoder, Eingabe, Host- und Viewer-Sitzung |
| `app` | Desktop-App (Tauri 2 + Svelte 5) |

```
Viewer ──TCP──▶ ctxremote-server ◀──TCP── Host (dauerhaft angemeldet)
   └──────── E2E-verschlüsselte Sitzung (Relay) ────────┘
```

## Voraussetzungen (Windows)

- Rust (stable, MSVC): `winget install Rustlang.Rustup`
- Visual Studio Build Tools mit C++-Workload (als Administrator ausführen):
  `winget install Microsoft.VisualStudio.2022.BuildTools --override "--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"`
- Node.js ≥ 20

## Starten

```powershell
# Server (z. B. auf einem VPS oder lokal)
cargo run --release -p ctxremote-server -- --listen 0.0.0.0:21300 --data ./data

# App (Entwicklung)
cd app
npm install
npm run tauri dev

# App-Installer bauen
npm run tauri build   # NSIS-Installer inkl. Windows-Dienst (braucht Adminrechte bei der Installation)
```

### Schnellhilfe („CTXRemote Hilfe“)

Portable EXE ohne Installation, Einstellungen und Tray, mit der sich ein PC einmalig fernsteuern lässt. Die Serveradresse wird beim Bauen eingebaut:

```powershell
cd app
$env:CTXREMOTE_QUICK_SERVER = "remote.example.org:21300"; npm run build:quick
# Ergebnis: target\release\CTXRemote-Hilfe.exe
```

Eingehende Verbindungen müssen dort immer per „Zulassen“ bestätigt werden; beim Schließen des Fensters endet die App samt Sitzung.

In der App unter **Einstellungen → Server** die Serveradresse eintragen. Firewall: TCP 21300 eingehend auf dem Server.

## Plattformen

| | Viewer (steuern) | Host (gesteuert werden) |
|---|---|---|
| Windows 10/11 | ja | ja |
| Linux | baut, ungetestet | noch nicht (geplant: X11, später Wayland über Portale) |
| macOS | baut laut CI, ungetestet | noch nicht (geplant: ScreenCaptureKit + CGEvent) |

Auf Systemen ohne Host-Unterstützung meldet sich das Gerät trotzdem mit ID an. Wer sich verbinden will, bekommt eine klare Meldung statt eines Abbruchs.

### Tests unter Linux (Docker)

```powershell
docker run --rm -v "${PWD}:/src" -v ctxremote-target:/target -e CARGO_TARGET_DIR=/target -w /src rust:1 `
  cargo test -p ctxremote-proto -p ctxremote-core -p ctxremote-server
```

### Hinweis: Smart App Control / Code-Signatur

Ist unter Windows 11 Smart App Control aktiv, blockiert es frisch kompilierte, unsignierte Programme (os error 4551). Für die Weitergabe braucht CTXRemote deshalb eine Code-Signatur, zum Beispiel über Microsoft Trusted Signing.

Zum lokalen Testen zweier Instanzen auf einem PC: `CTXREMOTE_CONFIG=C:pfadzweite.json` setzen.

## Stand und Roadmap

Funktioniert (v0.1): ID und Passwort, Bild, Maus, Tastatur, mehrere Monitore, Vollbild, Tray-Betrieb, Geräteliste mit Aliasen.

Als Nächstes:
1. Zwischenablage synchronisieren (Protokoll steht schon)
2. Dateiübertragung
3. Windows-Dienst für Anmeldebildschirm, UAC-Dialoge und Strg+Alt+Entf
4. Direkte P2P-Verbindung (UDP-Hole-Punching), Relay nur als Rückfall
5. Remote-Mauszeiger, Qualitätsregelung, Hardware-Encoder (NVENC/QSV/AMF)
6. Adressbuch und Geräteverwaltung über den Server (die „Pro“-Funktionen)
7. Linux-Host (X11), dann macOS-Host, dann Wayland
8. Code-Signatur der Windows-Builds
