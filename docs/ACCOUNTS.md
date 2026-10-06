# Konten und Geräteliste

Stand: 5. Oktober 2026. Code: `crates/proto/src/account.rs` (Protokoll), `crates/server/src/accounts.rs` (Server), `crates/core/src/account.rs` (Client, Verschlüsselung, Abgleich), `app/src-tauri/src/account.rs` (App). Test mit echtem Server: `cargo test -p ctxremote-server --test accounts`.

## Was es kann

Ein Konto hält die Geräteliste eines Nutzers (`Config::peers`, also Geräte mit Namen und zuletzt verbundene) auf allen seinen PCs gleich.

- Ein Konto entsteht mit „Konto anlegen“.
- Weitere Geräte kommen über einen **Einmal-Code** dazu, z. B. `7K2P-9XQM-4RTD`. Ein Gerät im Konto zeigt ihn unter „Gerät hinzufügen“, er gilt 10 Minuten und nur einmal.
- Es gibt **keine E-Mail und kein Passwort**. Das Konto ist die Menge der Geräteschlüssel (Ed25519), die schon jedes Gerät hat.
- Die App gleicht ab: beim Start, kurz nach jeder Änderung an der Liste (Umbenennen, Entfernen, neue Verbindung) und alle 5 Minuten. Mit Dienst gleicht die App ab und nicht der Dienst, denn die Liste gehört dem Benutzer.
- Die Schnellhilfe hat kein Konto.

## Sicherheit

- **Jede Anfrage ist signiert:** über die Nonce der Verbindung und die Anfrage selbst, mit eigenem Präfix `ctxremote/account/v1:`. Eine Registrierungs- oder Alias-Signatur lässt sich also nicht als Kontoanfrage wiederverwenden.
- **Die Geräteliste ist Ende-zu-Ende verschlüsselt:** ChaCha20-Poly1305 mit einem zufälligen **Kontoschlüssel**, den nur die Geräte des Kontos kennen (`config.account.key`). Der Server speichert nur einen undurchsichtigen Block.
- **Übergabe des Kontoschlüssels beim Koppeln:**
  - Der Code hat 12 Zeichen. Die ersten 4 benennen die Kopplung am Server und sind nicht geheim, die letzten 8 (40 Bit) sind geheim.
  - Aus dem geheimen Teil leitet Argon2id (19 MiB, 2 Durchläufe) zwei unabhängige Schlüssel ab: einen, der den Kontoschlüssel versiegelt, und einen **Nachweis**, den der Server prüft.
  - Der Server bekommt Siegel und Nachweis, aber nie den Code.
  - Wer den Code nicht kennt, scheitert am Nachweis. Nach 3 Fehlversuchen ist die Kopplung gelöscht, und die Anfragen sind wie Verbindungsversuche gedrosselt.
  - **Grenze:** Ein böswilliger Server könnte die 40 Bit offline durchprobieren, jeder Versuch kostet aber eine Argon2id-Rechnung. Das ist bewusst ein Kompromiss zwischen Sicherheit und Tippaufwand.
- **Was der Server sieht:** welche Geräteschlüssel ein Konto bilden, wie groß die Liste ist und wann sie sich ändert. Den Inhalt sieht er nicht.

## Abgleich

- Die Liste wird Feld für Feld zusammengeführt (`Book::merge`):
  - Name des Geräts: Der jüngere gewinnt (`alias_at`, Millisekunden).
  - Hostname und „zuletzt verbunden“: Es zählt die jüngere Verbindung.
  - Entfernte Geräte: Sie bleiben 90 Tage als Grabstein (`config.removed`) gemerkt und verschwinden überall. Kommen sie danach durch eine neue Verbindung oder Umbenennung zurück, sind sie wieder da.
- Der Server speichert mit Revisionsnummer. Wer auf einem veralteten Stand schreibt, bekommt `Conflict` und gleicht erneut ab, bis zu 4 Mal.
- Die Grenzen von `prune` gelten auch nach dem Abgleich: alle benannten Geräte, dazu die letzten 12 unbenannten.

## Server

- `accounts.json` im Datenordner, neben `devices.json`. Darin stehen die Konten mit den Schlüsseln ihrer Geräte, die Revision und der verschlüsselte Block (hex).
- Kopplungen liegen nur im Speicher. Ein Neustart beendet sie, dann braucht es einen neuen Code.
- Grenzen: 20 Geräte je Konto, 256 KB je Liste.
- Alte Server legen bei Kontoanfragen auf. Die App meldet dann „Server-Update nötig“.

## Ausbau (vom Nutzer gewünscht, 5. Oktober 2026)

Konten mit Anmeldung in App **und** Website, Geräte im Webinterface verwalten, alles Ende-zu-Ende verschlüsselt. Entscheidungen des Nutzers:
- **Passwort vergessen:** Wiederherstellungscode, der bei der Registrierung einmal angezeigt wird. Ohne Code und ohne Passwort bleibt nur ein neues Konto. Jedes angemeldete Gerät kann ein neues Passwort setzen.
- **Mails:** gleich mit einbauen, der Nutzer hat SMTP-Zugangsdaten. Gemeint sind die Bestätigung der Adresse und ein Hinweis bei einer neuen Anmeldung.

**Stand:** Schritt 1 (verschlüsselte Verbindung), Schritt 2 (Anmeldung) und Schritt 3 (Webinterface) sind gebaut. Offen ist Schritt 4 (Mails).

**Webinterface** (`web/`, ausgeliefert unter `/konto/`):
- Anmelden, Registrieren, Wiederherstellen, angemeldete Geräte (entfernen, per Code hinzufügen), Geräteliste (umbenennen, entfernen), E-Mail und Passwort ändern.
- E-Mail oder Passwort ändern verlangt das aktuelle Passwort, außer in den ersten 10 Minuten nach der Anmeldung oder Wiederherstellung. Ein gestohlenes Cookie reicht also nicht, um das Konto zu übernehmen.
- Eine Adresse, die niemand innerhalb von 48 Stunden bestätigt, kann danach jemand anderes registrieren. Das alte Konto verliert dann die Anmeldung, behält aber seine Geräte. Wer sich mit einer fremden Adresse registriert, blockiert sie also nicht dauerhaft.
- Der Kontoschlüssel liegt im Arbeitsspeicher des Tabs. Damit Neuladen nicht abmeldet, legt der Browser ihn zusätzlich in `localStorage` ab, aber nur XOR-verknüpft mit einem Zufallswert (`pad`) seiner Server-Sitzung. Den Wert gibt der Server bei der Anmeldung und danach nur mit gültigem Cookie heraus (`GET /api/session-pad`). Endet die Sitzung (Abmelden, Ablauf, Passwortwechsel, Server-Neustart), ist der gespeicherte Rest wertlos und wird gelöscht. Alle Tabs teilen sich die Anmeldung. Abmelden in einem Tab lädt die anderen neu, sie zeigen dann das Anmeldeformular. Nach dem Schließen des Browsers bleibt man angemeldet, solange die Sitzung gilt (12 h ohne Nutzung, höchstens 7 Tage).
- Die Krypto (`web/src/lib/crypto.ts`) entspricht `crates/core/src/account.rs` bitgenau, geprüft mit `npm test`.
- **Server:** `crates/server/src/web.rs` mit axum auf `--http` (Standard 127.0.0.1:21380), hinter Caddy unter `/api`. Sitzungen als HttpOnly-, Secure- und SameSite=Strict-Cookie, 12 h ohne Nutzung bzw. höchstens 7 Tage, nur im Speicher. Eine Passwortänderung beendet die anderen Browser-Sitzungen. Mit `--web-origin` lehnt der Server Änderungen von fremden Herkünften ab. Der Browser darf alles, was keinen Geräteschlüssel braucht (`Accounts::handle_web`).
- Test: `cargo test -p ctxremote-server --test web`.

Schlüsselschema, umgesetzt in `crates/core/src/account.rs` (`password_keys`, `recovery_keys`, `login_setup`):
- **Hauptschlüssel:** `Argon2id(Passwort, Salz des Kontos)` mit 64 MiB und 3 Durchläufen (`Kdf::CURRENT`). Die Werte stehen am Konto, damit sie sich später erhöhen lassen. Clients akzeptieren nur Werte in `Kdf::acceptable`.
- Daraus per HKDF zwei unabhängige Werte:
  - **Anmeldewert:** geht an den Server, der nur seinen Hash speichert.
  - **Wickelschlüssel:** verschlüsselt den Kontoschlüssel, verlässt aber nie das Gerät.
- **Unbekannte E-Mail-Adressen:** Der Server gibt dafür ein festes, vorgetäuschtes Salz aus, damit sich nicht testen lässt, welche Adressen ein Konto haben.
- **Wiederherstellungscode:** 25 Zeichen (125 Bit), wickelt den Kontoschlüssel ein zweites Mal ein. Jede neue Anmeldung (Passwort ändern, Wiederherstellen) erzeugt einen neuen Code, der alte gilt dann nicht mehr.
- **Server:** speichert nur SHA-256 der abgeleiteten Werte und die beiden versiegelten Schlüssel. Er nimmt Anmeldedaten **nur über die verschlüsselte Verbindung** an (`AccountError::Unencrypted`). Nach 10 Fehlversuchen ist das Konto 15 Minuten gesperrt.
- **Konten, die per Code entstanden sind**, bekommen E-Mail und Passwort nachträglich (`SetLogin`). Ein Konto mit Anmeldung bleibt bestehen, auch wenn alle Geräte es verlassen.
- **Geräte im Konto:** Jedes Gerät meldet seinen Computernamen verschlüsselt (`SetLabel`). Der Server ergänzt ID und Online-Status. Andere Geräte lassen sich entfernen (`RemoveDevice`).
- **App:** Personen-Symbol oben rechts im Hauptfenster öffnet das Konto-Panel (`app/src/Account.svelte`). Ohne Konto zeigt es Anmelden, Registrieren, Mit Code und „Passwort vergessen?“. Mit Konto zeigt es Geräte, „Gerät per Code hinzufügen“, Passwort oder E-Mail ändern und Abmelden.
- **Tests:** `login_with_password_and_recovery_code` und `logins_need_an_encrypted_connection` (`--test accounts`), dazu `accounts::tests::logins` im Server.
- Die App spricht über die verschlüsselte Verbindung (Schritt 1), das Webinterface über HTTPS mit einer HTTP-API des Servers hinter Caddy. Die Krypto läuft im Browser, ChaCha20-Poly1305 und Argon2id sind ins Webinterface eingebunden.

## Zugriff ohne Passwort (5. Oktober 2026)

Geräte eines Kontos können sich ohne Passwort mit einem Gerät verbinden, das das erlaubt (Kontopanel: „Geräte dieses Kontos dürfen sich ohne Passwort mit diesem Gerät verbinden“). Standard ist aus, denn es ist unbeaufsichtigter Zugriff.

- **Zwei Nachweise**, damit weder der Server allein noch ein entferntes Gerät allein hineinkommt:
  1. **Kontoschlüssel:** Der Host bietet einen zusätzlichen SPAKE2-Slot mit dem **Zugangspasswort** `HKDF(Kontoschlüssel, "ctxremote/access/v1", Host-ID)` an (`account::access_password`). Der Server kennt den Kontoschlüssel nicht. Pro Host verschieden, eine Freigabe öffnet also keinen anderen Host.
  2. **Mitgliedschaft:** Der Viewer signiert mit seinem Geräteschlüssel die Host-ID und einen Bindungswert der Sitzung (`SecureReceiver::binding`, aus dem SPAKE2-Geheimnis), als `MemberProof` im `Hello`-Anhang. Der Host fragt den Server mit `ClientMsg::SameAccount`, ob dieser Schlüssel im selben Konto ist wie sein **Zeugen-Schlüssel** (der Schlüssel der App auf diesem Rechner). Ein entferntes Gerät kennt den Kontoschlüssel noch, ist aber kein Mitglied mehr.
  - Grenze: Ein böswilliger Server **zusammen** mit einem entfernten Gerät käme hinein.
- **Host-Seite:** `Config::account_access` (`AccessGrant`: Host-ID, Zugangspasswort, Zeuge). Ohne Dienst setzt die App sie selbst. Mit Dienst geht sie über den UAC-Helfer (`UiRequest::ConfigureAccountAccess`, nur erhöhte Administratoren), sonst könnte sich ein eingeschränkter Windows-Benutzer selbst Fernzugriff geben.
- **Viewer-Seite:** Welche Geräte freigegeben sind, steht verschlüsselt in der Geräteliste (`Book::access`, Gerät → `{open, at}`, die jüngere Wahl gewinnt). Bei solchen Geräten verbindet die App ohne Passwortdialog (`connect` mit leerem Passwort) und fällt bei einem Fehler auf den Passwortdialog zurück.
- Ein Fehlversuch über diesen Weg zählt wie ein falsches Passwort zur Sperre des Hosts.
- Feature-Bit `ACCOUNT`. Alte Viewer beantworten den zusätzlichen Slot mit ihrem Passwort, das schadet nicht. Alte Server legen bei `SameAccount` auf, dann gilt das als „kein Mitglied“.
- Das Webinterface schreibt unbekannte Felder der Geräteliste unverändert zurück. Ältere App-Versionen kennen `access` nicht und lassen es beim Zurückschreiben weg, bis sie aktualisiert sind.
- Test: `cargo test -p ctxremote-server --test access`.

## Konto löschen

Im Webinterface unter „Konto löschen“, mit dem Passwort zur Bestätigung (`POST /api/account/delete`). Der Server entfernt Konto, Anmeldung, Geräteliste und Mitgliedschaften, beendet alle Browser-Sitzungen und schickt eine Mail. Die Apps merken es beim nächsten Abgleich (`NotLinked`) und entfernen ihre Verknüpfung selbst.

## Später

- Anmeldung mit E-Mail und Zahlungen über die Website (`docs/PLANS.md`). Das Konto bekommt dann zusätzlich eine E-Mail. Die Geräteschlüssel bleiben der Weg, auf dem Geräte sprechen.
- Eigene Geräte automatisch in die Liste aufnehmen, damit „Meine Geräte“ überall erscheinen, ohne dass man sich einmal verbunden hat.
