# Konzept: Bezahlte Pläne (Free, Pro, Ultra)

Stand: 5. Oktober 2026. Noch nichts davon ist gebaut. Das Konzept dient als Grundlage für die Entscheidungen unten (Abschnitt „Offene Entscheidungen“), bevor Code entsteht.

## Ausgangslage

- **Lizenz:** Der Code steht unter AGPL-3.0. Wer selbst einen Server betreibt, kann jede Sperre im Code entfernen. Pläne wirken deshalb vor allem auf **deinem gehosteten Dienst** (`remote.ctx.ink`). Für Selbsthoster gibt es zwei Wege:
  - alles frei lassen (wie heute),
  - oder **Doppellizenz**: AGPL für alle, dazu eine kaufbare kommerzielle Lizenz für Firmen, die die AGPL-Pflichten (Quellcode offenlegen bei Änderungen) nicht wollen. Das geht nur, solange du alle Rechte am Code hältst. Fremde Beiträge bräuchten dann eine Rechteübertragung (CLA).
- **Heute gibt es keine Konten:** Geräte melden sich nur mit ihrem Geräteschlüssel an (ID, Alias). Pläne brauchen Konten, an denen die Geräte hängen.
- **Der Server speichert in JSON-Dateien.** Für Konten, Zahlungen und Verlauf braucht es eine Datenbank. Empfehlung: SQLite, weil sie in derselben Binärdatei läuft und kein eigener Dienst nötig ist.

## Was die Pläne unterscheidet

Vorschlag, die Grenzen sind bewusst einfach gehalten und später leicht anzupassen:

| | **Free** | **Pro** | **Ultra** (Teams/Firmen) |
|---|---|---|---|
| Zielgruppe | Privat, eigene Geräte | Einzelne Techniker, Freiberufler | IT-Dienstleister, Firmen mit mehreren Technikern |
| Benutzer | 1 | 1 | mehrere, mit Rollen |
| Eigene Geräte mit unbeaufsichtigtem Zugriff | 3 | unbegrenzt | unbegrenzt |
| Gleichzeitige Sitzungen | 1 | 3 | je Benutzer 3 |
| Schnellhilfe-Sitzungen | ja, mit Hinweis „Kostenlose Version“ beim Kunden | ja | ja |
| Direktverbindung, Dateien, Chat | ja | ja | ja |
| Öffentlicher Alias | ab 8 Zeichen | ab 3 Zeichen | ab 3 Zeichen, mehrere |
| Helfer-Profil (Name, Firma, Logo) | nur Name | ja | ja |
| **Geprüftes Profil** (Haken „Geprüft“ beim Kunden) | – | ja (Firma geprüft) | ja |
| Adressbuch über den Server (auf allen Geräten gleich) | – | ja | geteilt im Team |
| Sitzungsverlauf (wer, wann, wie lange) | – | 30 Tage | 1 Jahr, exportierbar |
| **Eigene Schnellhilfe mit Branding** (White-Label) | – | – | ja |
| Preis (Vorschlag) | 0 € | ca. 9 € / Monat | ca. 29 € / Monat inkl. 3 Benutzer, jeder weitere 8 € |

Hinweise zu den Grenzen:

- Was Geld kostet, ist vor allem der **Relay-Verkehr** über deinen Server. Seit dem UDP-Weg läuft ein großer Teil direkt, das hält die Kosten niedrig. Eine harte Bandbreitengrenze für Free ist deshalb erst einmal nicht nötig. Beobachten und erst bei Bedarf drosseln.
- „Gleichzeitige Sitzungen“ ist einfacher durchzusetzen und zu verstehen als Minuten-Kontingente.

## Das wichtigste Verkaufsargument: geprüfte Profile

Das neue Helfer-Profil ist heute **selbst angegeben**. Jeder kann sich „Microsoft Support“ nennen, deshalb warnt die Zugriffsanfrage ausdrücklich davor. Bei bezahlten Konten kann der Server das ändern:

1. Du (oder ein automatischer Ablauf) prüfst Name und Firma eines Pro- oder Ultra-Kontos einmalig, z. B. über die Rechnungsadresse oder ein Impressum.
2. Der Server **signiert** das Profil (Name, Firma, Logo-Hash, Ablaufdatum) mit einem eigenen Schlüssel, so wie heute die Updates signiert werden.
3. Der Viewer schickt die Signatur im `Hello`-Anhang mit. Die Schnellhilfe kennt den öffentlichen Schlüssel und zeigt dann „Geprüft: Ecker IT-Service“ mit Haken statt der Warnung.

Das ist für Endkunden ein echter Sicherheitsgewinn und für Techniker ein klarer Grund für Pro.

## White-Label für Ultra-Kunden

Gewählt wurde „jeder Kunde sein eigenes Branding“. Zwei Wege:

- **A: Ein eigener Build je Kunde** (wie der Client-Generator von RustDesk). Die Pipeline baut eine Schnellhilfe mit Name, Logo und Farben des Kunden. Nachteile:
  - Jeder Build dauert etwa 10 Minuten.
  - Jede Datei bräuchte eine eigene Code-Signatur.
  - Updates müssten je Kunde gebaut werden.
- **B: Eine Datei, Branding zur Laufzeit (Empfehlung):**
  - Es gibt nur eine signierte Schnellhilfe.
  - Der Kunde lädt sie über seinen Link herunter, z. B. `ctxremote.ctx.ink/h/ecker-it`.
  - Der Server hängt ein kleines, signiertes Branding-Paket an die Datei an (Name, Logo, Farbe, Texte, Telefonnummer), ohne die Code-Signatur zu brechen. Windows erlaubt Daten im Signatur-Bereich am Dateiende, wie es Chrome bei Installer-Tags macht.
  - Beim Start liest die Schnellhilfe das Paket, prüft die Signatur des Servers und zeigt das Branding.
  - Ohne gültiges Paket erscheint das normale CTXRemote-Aussehen.

  Vorteile: kein Build je Kunde, eine Code-Signatur, Updates für alle gleichzeitig, und Branding-Änderungen gelten sofort.

Was sich anpassen lässt (beide Wege):
- Fenstertitel und Name
- Logo
- Akzentfarbe, mit Kontrastprüfung
- Begrüßungstext („Geben Sie diese Daten an …“)
- Telefonnummer oder Webseite des Supports
- optional: Verbindungen nur von Technikern des eigenen Teams annehmen

## Technischer Aufbau

1. **Konten** (Grundlage für alles):
   - Anmeldung mit E-Mail und Passwort (Argon2), Bestätigung per E-Mail-Link.
   - Die App meldet sich einmal an. Danach bindet der Server die Geräte-ID an das Konto, das Gerät beweist das mit seinem Schlüssel (wie beim Alias).
   - Ein Anmelde-Token liegt in der Konfiguration, damit man das Passwort nicht jedes Mal eingeben muss.
2. **Datenbank:** SQLite im Datenordner des Servers. Die bestehenden JSON-Dateien werden beim ersten Start übernommen.
3. **Plan-Prüfung:** Der Server kennt zu jedem Konto den Plan und das Ablaufdatum und setzt die Grenzen dort durch, wo sie entstehen:
   - Registrierung: Geräte zählen
   - Sitzungsaufbau: gleichzeitige Sitzungen zählen
   - Alias: Länge prüfen
   - Profil: signieren oder nicht

   Die App zeigt die Grenzen nur an. Durchgesetzt wird nie im Client.
4. **Bezahlen:**
   - Empfehlung: ein **Merchant of Record** wie Paddle oder Lemon Squeezy. Er verkauft im eigenen Namen, kümmert sich um Umsatzsteuer in allen EU-Ländern (OSS), Rechnungen und Widerruf. Das spart als Einzelunternehmer sehr viel Aufwand, kostet aber etwa 5 % plus 0,50 € je Zahlung.
   - Alternative: Stripe mit Stripe Tax ist günstiger, aber Steuer, Rechnungen und Rechtstexte liegen dann bei dir.
   - In beiden Fällen meldet ein Webhook dem Server „Plan gekauft, verlängert oder gekündigt“, und der Server setzt den Plan.
5. **Kundenbereich:** Eine kleine Webseite auf dem Server (HTTPS, z. B. hinter Caddy) für Konto, Plan, Rechnungen (Link zum Zahlungsanbieter), Team-Mitglieder und Branding. Alternativ alles in der App. Für Branding mit Logo-Upload und für Teams ist eine Webseite aber angenehmer.
6. **Rechtliches** (vor dem ersten Verkauf):
   - Gewerbeanmeldung
   - Impressum
   - AGB
   - Datenschutzerklärung (Sitzungsverlauf und Konten sind personenbezogene Daten)
   - Auftragsverarbeitungsvertrag (AVV) für Firmenkunden, weil der Relay deren Verbindungsdaten sieht, wenn auch keine Inhalte

## Reihenfolge der Umsetzung

Jeder Schritt ist für sich nutzbar:

1. Konten und Gerätezuordnung, alles noch kostenlos. Dazu das Adressbuch über den Server: Mehrwert ohne Bezahlung, und die Konten bekommen echte Nutzer.
2. SQLite und Plan-Feld am Konto. Grenzen zuerst nur protokollieren, nicht durchsetzen, um zu sehen, wer wo landet.
3. Zahlungsanbieter mit Webhook und Kundenbereich.
4. Geprüfte, signierte Profile (Pro).
5. Teams und geteiltes Adressbuch (Ultra).
6. White-Label-Schnellhilfe nach Weg B (Ultra). Setzt eine Code-Signatur voraus, siehe STATUS.md.

## Entscheidungen des Nutzers (5. Oktober 2026)

- **Noch keine Firma.** Verkauft wird erst, wenn es eine gibt. Bis dahin baue ich nichts, was Geld annimmt.
- **Später über die eigene Website mit Stripe**, noch nicht endgültig festgelegt. Die Website gibt es jetzt (`website/`, https://ctxremote.ctx.ink). Konten und Kundenbereich gehören später dorthin.
- **Free nur für private Nutzung**, gewerblich nicht. Das steht so auf der Preisseite. Durchsetzen lässt es sich erst mit Konten und AGB.
- **Selbst-Hosting wird erst einmal nicht angeboten.** Die Website erwähnt es nicht. Der Code bleibt AGPL, die Frage nach einer kommerziellen Lizenz stellt sich damit vorerst nicht.

## Offene Entscheidungen (für den Nutzer)

1. Preise und Grenzen aus der Tabelle oben: passen sie so ungefähr? Die Preisseite zeigt bisher „Preis folgt“.
2. ~~Darf Free gewerblich genutzt werden?~~ Nein, nur privat (entschieden).
3. Zahlungsanbieter: Tendenz Stripe über die Website, noch offen. Bei Stripe liegen USt (OSS), Rechnungen und Widerruf beim Verkäufer, Stripe Tax hilft dabei.
4. Rechtsform, sobald es eine Firma gibt. Gilt die Kleinunternehmerregelung? Das bestimmt, wie Rechnungen aussehen müssen.
