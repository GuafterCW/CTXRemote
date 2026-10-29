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

Kostenlos
Remocto ist kostenlos, ohne Abo und ohne Werbung.

Keine Verweise auf Downloads außerhalb des Stores (andere Plattformen, Website als Bezugsquelle, „herunterladen unter …“) in Beschreibung, Features, „Neues in dieser Version“ und Screenshots: Microsoft lehnt das nach Richtlinie 10.1.5 ab (am 21. Oktober 2026 passiert). Die Website gehört nur in die Felder „Website“ und „Datenschutzrichtlinie“.

## Neues in dieser Version

Erste Version im Microsoft Store.

Für die Umbenennung (Update nach 0.1.50):

> CTXRemote heißt jetzt Remocto: neuer Name, neues Logo. Ihre ID, Ihre Geräte und Einstellungen bleiben. Neu: Einladungslinks, über die Ihr Gegenüber Ihnen mit einem Klick ID und Passwort schickt.

## Umbenennung im Partner Center

Die Paket-Identität `PhilippEcker.CTXRemote` und die Store-ID bleiben, damit ist es ein Update und kein neues Produkt. Der `DisplayName` im Paket („Remocto“) muss aber ein für diese App reservierter Name sein, sonst lehnt das Partner Center das Paket beim Hochladen ab. Deshalb zuerst unter Produktverwaltung → Produktidentität „Remocto“ als weiteren Namen reservieren, dann in der Einreichung Paket, Produktname im Store-Eintrag, Texte, Screenshots und Logos tauschen.

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
- Datenschutzrichtlinie: https://remocto.com/datenschutz.html
- Website: https://remocto.com
- Support-Kontakt: die Adresse aus dem Impressum
- Copyright: © 2026 Remocto
- Preis: kostenlos
- Altersfreigabe (IARC-Fragebogen): keine Gewalt, keine Glücksspiele, kein Kauf in der App. Nutzer können miteinander kommunizieren (Chat) und Inhalte teilen (Bildschirm, Dateien), das im Fragebogen mit „Ja“ angeben.

## Begründung der eingeschränkten Fähigkeiten

Das Partner Center fragt je Fähigkeit einzeln (Englisch, für die Prüfer):

**runFullTrust**

> Remocto is a remote support and remote desktop application built as a Win32 desktop app (Tauri/WebView2). runFullTrust is required to run the desktop executable itself: it captures the screen with DXGI, encodes video with Media Foundation, injects mouse and keyboard input, accesses the clipboard, and opens network connections to the Remocto relay server and directly to other devices. None of this is possible from an AppContainer.

Die erste Begründung (kurz) hat Microsoft am 29. Oktober 2026 abgelehnt und für jede Fähigkeit eine lange Fragenliste geschickt. Die Texte unten beantworten jede Frage einzeln. Sie gehen in die drei Begründungsfelder; der Abschnitt „Notes for certification“ kommt ins gleichnamige Feld der Einreichung.

**packagedServices**

> Remocto is remote support software for the user's own computers (like TeamViewer or AnyDesk). The packaged service (remocto-service.exe, desktop6:Service "Remocto", LocalSystem, auto start) keeps the computer reachable before anyone signs in and while the screen is locked. Without it, remote access only works while a user is signed in and the app is open.
>
> Operations: the service performs no maintenance or repair operations on the system. It (1) holds an outbound TCP connection to the Remocto relay server, (2) authenticates incoming viewers (one-time password confirmed by the person at the computer, a fixed password set by an administrator, or a device of the user's own account that an administrator allowed), (3) starts the screen agent in the console session for the duration of a session, (4) triggers Ctrl+Alt+Del with SendSAS on the viewer's request, (5) applies the administrator-only settings listed under allowElevation. At every start it sets HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\SoftwareSASGeneration to 1 (required by SendSAS; 2 becomes 3) and, only if the user enabled direct connections, adds one inbound firewall rule for its own executable and the chosen port.
>
> Architecture: remocto.exe (the UI) always runs unelevated as the signed-in user. It talks to the service over the named pipe \\.\pipe\remocto-ui. Pipe DACL: SYSTEM, Administrators and owner full access; Authenticated Users read and write data only (no FILE_CREATE_PIPE_INSTANCE, so the name cannot be squatted). Every message is one typed JSON request of a fixed set (at most 64 KB); there is no request that carries a command line, script, executable or file path. The screen agent (remocto-service.exe --agent) talks to the service over \\.\pipe\remocto-agent, which only SYSTEM can open.
>
> Requests any signed-in user may send: new one-time password, end a session, change what the viewer of a running session may do, chat with the viewer, set the device's display name, read the local connection log. All other requests are administrator-only (see allowElevation); the service checks the caller with ImpersonateNamedPipeClient and CheckTokenMembership(BUILTIN\Administrators), so a filtered (non-elevated) token is refused.
>
> User disclosure: the app shows the person at the computer every running session, who is connected and what they may do, and can end it with one click. A one-time-password session must be confirmed at the computer. A connection log (who, when, how long, how authenticated, refused attempts; at most 200 entries, stored only on the computer) is shown in the app. After 5 wrong passwords the service refuses all password logins for 5 minutes. The user can revoke unattended access at any time by removing the fixed password and the account/team access (each an administrator change with a UAC prompt). Uninstalling the app removes the service.

**localSystemServices**

> Why LocalSystem: only a SYSTEM process can open and capture the secure desktop (Winlogon: sign-in screen, lock screen, UAC prompts) with OpenInputDesktop/SetThreadDesktop, and only a service can trigger Ctrl+Alt+Del with SendSAS. Starting the screen agent in the console session needs SeTcbPrivilege (duplicate the service token, set TokenSessionId to WTSGetActiveConsoleSessionId(), CreateProcessAsUserW on winsta0\default).
>
> Limits on CreateProcessAsUser: the only program started this way is remocto-service.exe from the package's own install folder, with the fixed arguments "--agent <pipe name> [--protect]". The pipe name is generated by the service. No path, program or argument comes from the network or from the UI. Everything that runs on behalf of a remote viewer and could execute code (the optional terminal, file transfer) runs with the signed-in user's token (WTSQueryUserToken), never as SYSTEM; without a signed-in user the terminal is unavailable. The optional process view (off by default, must be switched on per device or session) can only list processes, end a process by PID, and start or stop an existing service by name; it cannot start programs.
>
> Privileged service vs. UI: see packagedServices (two pipes, DACLs, fixed typed requests, elevated-administrator check for every settings change).
>
> Stored secrets: %ProgramData%\Remocto\host.json, with a protected DACL (SYSTEM and Administrators only, inheritance blocked). It holds the device ID, the device's Ed25519 signing key, and, if set, the fixed password and the authenticator (TOTP) secret. The fixed password and the TOTP secret are additionally encrypted with DPAPI (CryptProtectData, machine scope), so a copy of the file is useless on another computer. The fixed password is never sent over the network: viewers prove knowledge of it with a SPAKE2 password-authenticated key exchange; the resulting key encrypts the session end to end (ChaCha20-Poly1305). The relay server cannot read sessions.
>
> Unattended-access controls: unattended access is off until an administrator sets a fixed password or allows account/team devices. Revocation: remove the password or the access (UAC prompt), or end any session from the app. Retry limit: 5 wrong passwords lock password logins for 5 minutes. An optional authenticator code (TOTP) can be required in addition to the fixed password. Administrators can restrict what unattended sessions may do (input, files, clipboard, audio, restart, terminal, processes) separately from attended sessions. Every connection and refused attempt is logged locally and shown in the app.
>
> Secure desktop: it cannot be switched off on its own. The session always shows the console's current input desktop, which is the purpose of the service (so the remote helper can answer a UAC prompt or unlock the computer). Remote input can be switched off per device or per running session, which makes the secure desktop view-only.
>
> Websites: remocto.com is the product website, privacy policy at https://remocto.com/datenschutz.html. pecker.cc is the developer's personal domain (Philipp Ecker); both are controlled by the same developer, see the imprint at https://remocto.com/impressum.html, which lists the pecker.cc address. "CTXRemote" is the product's former name; the product is now called Remocto. Any CTX page you may have seen is outdated; the listing now points to remocto.com.

**allowElevation**

> The elevated process is remocto-service.exe from the package, started by the app with ShellExecuteEx verb "runas" and the arguments "--configure <request file> <SHA-256 of the request>". It is the only thing that ever runs elevated, and only when the user changes one of these settings in the app's settings dialog: (1) relay server address and fixed password, (2) direct connections (on/off, port, extra addresses), (3) which devices of the user's account may connect without a password, (4) which team's members may connect without a password, (5) what attended and unattended sessions may do, (6) the authenticator (TOTP) secret.
>
> Request and authentication: the app writes the request (JSON, one of the six types above) to a new file with an unguessable name in the user's temp folder and passes its SHA-256 on the helper's command line. The helper refuses the file if its hash differs (the file is writable by the user's other processes). It then connects to the service's pipe and sends exactly one typed request; the service checks that the caller holds an elevated Administrators token. The helper accepts no commands, scripts, executables, paths or configuration destinations: unknown fields and request types fail to parse and nothing is applied.
>
> Validation: server address non-empty; fixed password at least 8 characters (empty removes it); direct-connection port 1024 to 65535, at most 8 extra addresses, each in host:port form; account and team grants are structured data the service checks against the stored account and team; permissions are a fixed bit set.
>
> Write targets: the helper itself writes only its answer ({"ok":true} or {"error":"..."}) back into the request file. The service writes %ProgramData%\Remocto\host.json (through a temporary file and an atomic rename, so a failure leaves the previous configuration untouched). Only for setting (2), the elevated helper adds or removes the inbound firewall rule "Remocto" (netsh advfirewall, TCP, the chosen port, program remocto-service.exe only). The service re-applies the same rule at each start, because the package folder changes with every update; the helper applies it at once when the user changes the setting.
>
> Lifetime and failures: the helper waits at most 10 seconds for the service's answer, then exits; it never stays running. If the service refuses or does not answer, nothing changes and the app shows the error. If the user declines UAC, ShellExecuteEx returns ERROR_CANCELLED, the app shows that administrator rights are required and nothing changes. The service logs each applied change.
>
> Why not without elevation: these settings decide who can control the computer without anyone present. If any signed-in (possibly standard) user or any process running as that user could change them, it could grant itself unattended SYSTEM-level screen access. The UAC prompt is the user's explicit confirmation of each such change.

**Notes for certification** (Feld in der Einreichung)

> Remocto (formerly CTXRemote) is remote support software for the user's own computers. Product website: https://remocto.com, privacy policy: https://remocto.com/datenschutz.html, imprint: https://remocto.com/impressum.html. pecker.cc is the developer's personal domain; both domains belong to the same developer (Philipp Ecker). The restricted capability justifications were expanded to answer each question from the previous review. To test: install on two PCs, open Remocto on both, enter the first PC's ID and one-time password on the second, confirm the request on the first PC. Unattended access: Settings → fixed password (UAC prompt), then lock the first PC and connect again; the lock screen is shown and Ctrl+Alt+Del works.

# Google Play

Grafiken in `docs/store/android/` (neu: `node docs/store/play.cjs app/app-icon.svg docs/store/android`):

| Feld in der Play Console | Datei |
|---|---|
| App-Symbol 512×512 | `icon-512.png` |
| Vorstellungsgrafik 1024×500 | `feature-1024x500.png` |
| Screenshots Smartphone | `1-start.png` … `5-dateien.png` (Bildunterschriften oben) |

## App-Name (max. 30 Zeichen)

Remocto – Fernwartung

## Kurzbeschreibung (max. 80 Zeichen)

Den PC vom Handy aus steuern. Ende-zu-Ende verschlüsselt, ohne Konto nutzbar.

## Vollständige Beschreibung (max. 4000 Zeichen)

Mit Remocto steuern Sie Ihren Windows-PC, Mac oder Linux-Rechner vom Handy aus: Sie sehen den Bildschirm, klicken per Tippen, tippen Text über die Tastatur und übertragen Dateien. Ideal, um unterwegs schnell etwas am eigenen PC zu erledigen oder Familie und Freunden bei Computerproblemen zu helfen.

So einfach geht es
Auf dem Computer läuft Remocto (kostenlos unter remocto.com). Er zeigt eine neunstellige ID und ein Einmal-Passwort. Diese geben Sie in der App ein, Ihr Gegenüber bestätigt mit „Zulassen“, und Sie sind verbunden. Ein Konto ist nicht nötig.

Für den Touchscreen gemacht
• Tippen ist ein Klick, langes Drücken ein Rechtsklick
• Mit zwei Fingern scrollen, zoomen und verschieben
• Tastatur mit Strg, Alt, Esc, Tab und Pfeiltasten
• Mehrere Bildschirme und wählbare Bildqualität

Sicher
Jede Sitzung ist Ende-zu-Ende verschlüsselt (SPAKE2, ChaCha20-Poly1305). Unser Server vermittelt nur die Verbindung und sieht ausschließlich verschlüsselte Daten. Wo möglich, gehen die Daten direkt zwischen den Geräten. Die Person am Computer bestimmt, was Sie dürfen, und kann jede Sitzung mit einem Klick beenden.

Auch ohne jemanden davor
Mit einem festen Passwort (auf Wunsch mit Zwei-Faktor-Code) erreichen Sie Ihre eigenen Windows-PCs auch am Anmelde- und Sperrbildschirm.

Weitere Funktionen
• Dateien zwischen Handy und PC übertragen
• Chat in der Sitzung
• Geräteliste, auf Wunsch über ein Konto mit allen Ihren Geräten abgeglichen
• Eigener Alias statt der ID

Hinweis: Die Android-App steuert andere Geräte. Das Handy selbst lässt sich nicht fernsteuern.

Remocto ist kostenlos, ohne Abo und ohne Werbung.

## Weitere Angaben

- Kategorie: Tools. Tags: Fernzugriff, Remote Desktop.
- Kontakt-E-Mail: die Adresse aus dem Impressum; Website `https://remocto.com`; Datenschutzerklärung `https://remocto.com/datenschutz.html`.
- Werbung: Nein. In-App-Käufe: Nein.
- Zielgruppe: ab 18 (keine Kinder-App, vermeidet die Familien-Richtlinien).
- Altersfreigabe (IARC): Kategorie „Dienstprogramm, Produktivität, Kommunikation“; Nutzer können miteinander kommunizieren (Chat) und Inhalte teilen: Ja. Sonst überall Nein.
- App-Zugriff: „Für alle oder einige Funktionen ist ein Zugriff erforderlich“. Die Prüfer brauchen einen Rechner, mit dem sie sich verbinden können: ID und festes Passwort eines Demo-Rechners eintragen (läuft dauerhaft, ohne persönliche Daten, Rechte nur „Bildschirm sehen“ und „Maus und Tastatur“).

## Datensicherheit

- Werden Daten erhoben oder weitergegeben? Erhoben: Ja. Weitergegeben an Dritte: Nein.
- Verschlüsselung bei der Übertragung: Ja. Löschung anfordern: Ja (Konto löschen im Webinterface, `https://remocto.com/datenschutz.html`).
- Erhobene Daten:
  - Persönliche Daten → E-Mail-Adresse: optional (nur mit Konto), Zweck „Kontoverwaltung“.
  - Geräte- oder andere IDs: erforderlich (die Remocto-ID des Geräts), Zweck „App-Funktionen“.
  - Nachrichten → Andere In-App-Nachrichten (Chat) und Dateien: werden Ende-zu-Ende verschlüsselt nur durchgeleitet, nicht gespeichert. Google zählt Ende-zu-Ende verschlüsselte Daten nicht als „erhoben“, daher hier nicht angeben.
