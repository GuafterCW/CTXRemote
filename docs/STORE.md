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

## Produktname

CTXRemote

## Kurzbeschreibung (max. 1000 Zeichen, erscheint oben)

Fernwartung für Ihre eigenen Geräte und für Menschen, denen Sie helfen wollen. Ende-zu-Ende verschlüsselt, in einer Minute verbunden, ohne Konto nutzbar.

## Beschreibung (max. 10.000 Zeichen)

CTXRemote verbindet Sie mit Ihren eigenen Geräten und mit Menschen, denen Sie helfen wollen. Sie sehen den fernen Bildschirm, bedienen Maus und Tastatur und übertragen Dateien, als säßen Sie daneben.

Verbinden in einer Minute
Jedes Gerät hat eine neunstellige ID und ein Einmal-Passwort. Ihr Gegenüber nennt Ihnen beides, bestätigt Ihren Zugriff mit „Zulassen“, und Sie sind verbunden. Ein Konto ist dafür nicht nötig.

Ende-zu-Ende verschlüsselt
Jede Sitzung wird mit dem Passwort verschlüsselt (SPAKE2, ChaCha20-Poly1305). Unser Server vermittelt nur die Verbindung und sieht ausschließlich verschlüsselte Daten. Wo es geht, wechselt die Sitzung auf eine direkte Verbindung zwischen den Geräten.

Unbeaufsichtigter Zugriff auf eigene Geräte
CTXRemote läuft als Windows-Dienst. Mit einem festen Passwort erreichen Sie Ihre Geräte auch, wenn niemand davor sitzt: am Anmeldebildschirm, am Sperrbildschirm und bei Abfragen der Benutzerkontensteuerung, mit Strg+Alt+Entf und Neustart aus der Ferne. Auf Wunsch zusätzlich mit einem Zwei-Faktor-Code aus einer Authenticator-App.

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
CTXRemote gibt es auch für macOS, Linux und als Viewer für Android (Vorschau). Downloads unter https://ctxremote.ctx.ink

Kostenlos für die private Nutzung
Für Ihre eigenen Geräte, Familie und Freunde ist CTXRemote kostenlos. Pläne für die gewerbliche Nutzung sind in Vorbereitung.

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

- Kategorie: Produktivität (Unterkategorie, falls gefragt: Hilfsprogramme und Tools)
- Datenschutzrichtlinie: https://ctxremote.ctx.ink/datenschutz.html
- Website: https://ctxremote.ctx.ink
- Support-Kontakt: die Adresse aus dem Impressum
- Copyright: © 2026 CTXRemote
- Preis: kostenlos
- Altersfreigabe (IARC-Fragebogen): keine Gewalt, keine Glücksspiele, kein Kauf in der App. Nutzer können miteinander kommunizieren (Chat) und Inhalte teilen (Bildschirm, Dateien), das im Fragebogen mit „Ja“ angeben.

## Begründung der eingeschränkten Fähigkeiten

Im Partner Center unter „Einreichungsoptionen“ → „Eingeschränkte Fähigkeiten“:

> CTXRemote ist eine Fernwartungs-App. Für den unbeaufsichtigten Zugriff auf eigene Geräte läuft ein Windows-Dienst als LocalSystem (packagedServices, localSystemServices). Nur so lassen sich Anmeldebildschirm, Sperrbildschirm und Dialoge der Benutzerkontensteuerung übertragen und Strg+Alt+Entf auslösen. Der Dienst nimmt Verbindungen nur mit dem Einmal-Passwort und Bestätigung am Gerät oder mit einem festen Passwort an, das der Administrator setzt. allowElevation wird für genau diese Einstellung genutzt (festes Passwort, Server): Die App startet dafür den Dienst mit einer Abfrage der Benutzerkontensteuerung. runFullTrust ist für die Desktop-App nötig.
