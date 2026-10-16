# Store-Eintrag (Microsoft Store)

Texte und Bilder zum Einfügen im Partner Center (Store-Einträge → Deutsch). Die technische Einrichtung steht in `docs/DEPLOY.md`, „Microsoft Store (MSIX)“.

## Screenshots

In `docs/store/`, 1920×1080, PNG, in dieser Reihenfolge:

| Datei | Bildunterschrift (optional, max. 200 Zeichen) |
|---|---|
| `1-start.png` | ID, Einmal-Passwort und Ihre Geräte auf einen Blick. |
| `2-sitzung.png` | Den fernen Bildschirm steuern, mehrere Sitzungen als Tabs. |
| `3-chat.png` | Chat in der Sitzung, ohne dass etwas auf dem fernen Bildschirm landet. |
| `4-dateien.png` | Dateien in beide Richtungen übertragen. |
| `5-einstellungen.png` | Sie legen fest, was die Gegenseite darf. |

Die Bilder zeigen die echte Oberfläche (`app/dist`) mit Beispieldaten. Das Bild des fernen Desktops ist `docs/store/desktop.html`. Neu erzeugen: `app/dist` bauen und auf Port 8765 ausliefern (`python3 -m http.server 8765` in `app/dist`), `desktop.html` mit Playwright als `desktop.png` (1920×1080) in einen Ordner legen, dann `node docs/store/screenshots.cjs <Ordner>`.

### Handy-Screenshots für Google Play

In `docs/store/android/`, 1081×1922 (hochkant) bzw. 1922×1081 (quer), echte Oberfläche im Handy-Layout:

| Datei | Bildunterschrift |
|---|---|
| `1-start.png` | Ihre Geräte auf einen Blick, Verbinden per ID oder Alias. |
| `2-sitzung.png` | Den PC vom Handy aus steuern: Tippen ist ein Klick. |
| `3-zoom.png` | Mit zwei Fingern zoomen und verschieben. |
| `4-tastatur.png` | Tastatur mit Strg, Alt, Esc und Pfeiltasten. |
| `5-dateien.png` | Dateien zwischen PC und Handy übertragen. |

Neu erzeugen wie oben, dann `node docs/store/android.cjs <Ordner>`. Das Skript prüft dabei auch die Touch-Bedienung (Tippen, langes Drücken, Ziehen, Scrollen, Zoomen, Tastatur) und endet mit Fehler, wenn etwas nicht stimmt.

## Store-Logos

In `docs/store/logos/`, aus `app/app-icon.svg` (neu: `node docs/store/logos.cjs app/app-icon.svg docs/store/logos`):

| Feld im Partner Center | Datei |
|---|---|
| Verpackungsgrafik 1:1 (1080 bzw. 2160) | `logo-1080.png`, `logo-2160.png` |
| Symbol für die App-Kachel 300×300 | `logo-300.png` |
| 1:1 150×150 | `logo-150.png` |
| 1:1 71×71 | `logo-71.png` |

Die Postergrafik 9:16 braucht nur Xbox und bleibt leer.

## Produktname

Remocto

## Kurzbeschreibung (max. 1000 Zeichen, erscheint oben)

Fernwartung für Ihre eigenen Geräte und für Menschen, denen Sie helfen wollen. Ende-zu-Ende verschlüsselt, in einer Minute verbunden, ohne Konto nutzbar.

## Beschreibung (max. 10.000 Zeichen)

Remocto verbindet Sie mit Ihren eigenen Geräten und mit Menschen, denen Sie helfen wollen. Sie sehen den fernen Bildschirm, bedienen Maus und Tastatur und übertragen Dateien, als säßen Sie daneben.

Verbinden in einer Minute
Jedes Gerät hat eine neunstellige ID und ein Einmal-Passwort. Ihr Gegenüber nennt Ihnen beides, bestätigt Ihren Zugriff mit „Zulassen“, und Sie sind verbunden. Ein Konto ist dafür nicht nötig.

Ende-zu-Ende verschlüsselt
Jede Sitzung wird mit dem Passwort verschlüsselt (SPAKE2, ChaCha20-Poly1305). Unser Server vermittelt nur die Verbindung und sieht ausschließlich verschlüsselte Daten. Wo es geht, wechselt die Sitzung auf eine direkte Verbindung zwischen den Geräten.

Unbeaufsichtigter Zugriff auf eigene Geräte
Remocto läuft als Windows-Dienst. Mit einem festen Passwort erreichen Sie Ihre Geräte auch, wenn niemand davor sitzt: am Anmeldebildschirm, am Sperrbildschirm und bei Abfragen der Benutzerkontensteuerung, mit Strg+Alt+Entf und Neustart aus der Ferne. Auf Wunsch zusätzlich mit einem Zwei-Faktor-Code aus einer Authenticator-App.

Sie bestimmen, was geht
Für Sitzungen mit Einmal-Passwort und für unbeaufsichtigte Sitzungen legen Sie getrennt fest, ob die Gegenseite steuern, Dateien übertragen, die Zwischenablage nutzen oder den Ton hören darf. Während einer Sitzung lassen sich die Rechte jederzeit ändern, und jede Sitzung ist mit einem Klick beendet.

Alles, was eine Fernwartung braucht
• Dateien in beide Richtungen, mit eigenem Dateibrowser für das ferne Gerät
• Dateien und Text per Kopieren und Einfügen
• Chat in der Sitzung
• Mehrere Sitzungen als Tabs in einem Fenster
• Mehrere Bildschirme, wählbare Bildqualität, Ton vom fernen Gerät
• Bildschirm des fernen Geräts schwarz schalten (Privatsphäre-Modus)
• Sitzungen aufzeichnen, mit sichtbarem Hinweis auf dem fernen Gerät
• Eigener Alias statt der neunstelligen ID
• Geräteliste mit Gruppen, auf Wunsch über ein Konto auf allen Ihren Geräten

Für Windows, Mac, Linux und Android
Remocto gibt es auch für macOS, Linux und als Viewer für Android (Vorschau). Downloads unter https://ctxremote.ctx.ink

Kostenlos
Remocto ist kostenlos, ohne Abo und ohne Werbung.

## Neues in dieser Version

Erste Version im Microsoft Store.

## Produktfeatures (je max. 200 Zeichen, bis zu 20)

- Ende-zu-Ende verschlüsselt: Der Server sieht nur verschlüsselte Daten
- Verbinden mit ID und Einmal-Passwort, ohne Konto
- Unbeaufsichtigter Zugriff als Windows-Dienst, auch am Sperrbildschirm
- Strg+Alt+Entf und Neustart aus der Ferne
- Zwei-Faktor-Code für das feste Passwort
- Direkte Verbindung zwischen den Geräten, wo möglich
- Dateien in beide Richtungen übertragen
- Chat in der Sitzung
- Mehrere Sitzungen als Tabs
- Rechte der Gegenseite pro Sitzung festlegen
- Privatsphäre-Modus: fernen Bildschirm schwarz schalten
- Sitzungen aufzeichnen

## Suchbegriffe (bis zu 7)

Fernwartung, Fernzugriff, Remote Desktop, Fernsteuerung, Bildschirmfreigabe, Support, Dateiübertragung

## Weitere Angaben

- Kategorie: Dienstprogramme und Tools, sekundäre Kategorie: Produktivität. Personenbezogene Daten: „Ja“ (Konto-E-Mail, Geräteliste, IP-Adressen am Server, Sitzungsinhalte).
- Datenschutzrichtlinie: https://ctxremote.ctx.ink/datenschutz.html
- Website: https://ctxremote.ctx.ink
- Support-Kontakt: die Adresse aus dem Impressum
- Copyright: © 2026 Remocto
- Preis: kostenlos
- Altersfreigabe (IARC-Fragebogen): keine Gewalt, keine Glücksspiele, kein Kauf in der App. Nutzer können miteinander kommunizieren (Chat) und Inhalte teilen (Bildschirm, Dateien), das im Fragebogen mit „Ja“ angeben.

## Begründung der eingeschränkten Fähigkeiten

Das Partner Center fragt je Fähigkeit einzeln (Englisch, für die Prüfer):

**runFullTrust**

> Remocto is a remote support and remote desktop application built as a Win32 desktop app (Tauri/WebView2). runFullTrust is required to run the desktop executable itself: it captures the screen with DXGI, encodes video with Media Foundation, injects mouse and keyboard input, accesses the clipboard, and opens network connections to the Remocto relay server and directly to other devices. None of this is possible from an AppContainer.

**packagedServices**

> For unattended access to the user's own computers, Remocto installs a Windows service (remocto-service.exe, declared with desktop6:Service). The service starts at boot, keeps the device reachable before anyone signs in, and starts the screen agent in the active console session when a session begins. Without a service, remote access would only work while a user is signed in and the app is open. The service only accepts connections with a one-time password confirmed by the person at the device, or with a fixed password set by an administrator.

**localSystemServices**

> The service must run as LocalSystem because only SYSTEM can show and control the Windows sign-in screen, the lock screen and User Account Control prompts (the secure desktop), and can trigger Ctrl+Alt+Del via SendSAS. It also needs SeTcbPrivilege to start the screen agent in the console session (CreateProcessAsUser with the console session id). Its configuration (device key, fixed password) is stored in ProgramData, readable only by SYSTEM and administrators. All sessions are end-to-end encrypted.

**allowElevation**

> Security-relevant settings of the service (the fixed password for unattended access, the server address, and which devices of the user's account may connect without a password) may only be changed by an administrator. When the user changes them in the app, the app starts remocto-service.exe from the package with a UAC prompt (ShellExecute "runas"). The elevated process writes the settings to the service's protected configuration and exits. Nothing else runs elevated.
