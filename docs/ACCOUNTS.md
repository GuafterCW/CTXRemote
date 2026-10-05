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

Geplantes Schlüsselschema (wie bei Bitwarden):
- **Hauptschlüssel:** `Argon2id(Passwort, Salz des Kontos)`.
- Daraus per HKDF zwei unabhängige Werte:
  - **Anmeldewert:** geht an den Server, der nur seinen Hash speichert.
  - **Wickelschlüssel:** verschlüsselt den Kontoschlüssel, verlässt aber nie das Gerät.
- **Unbekannte E-Mail-Adressen:** Der Server gibt dafür ein festes, vorgetäuschtes Salz aus, damit sich nicht testen lässt, welche Adressen ein Konto haben.
- **Wiederherstellungscode:** 128 Bit, wickelt den Kontoschlüssel ein zweites Mal ein.
- Die App spricht über die verschlüsselte Verbindung (Schritt 1), das Webinterface über HTTPS mit einer HTTP-API des Servers hinter Caddy. Die Krypto läuft im Browser, ChaCha20-Poly1305 und Argon2id sind ins Webinterface eingebunden.

## Später

- Anmeldung mit E-Mail und Zahlungen über die Website (`docs/PLANS.md`). Das Konto bekommt dann zusätzlich eine E-Mail. Die Geräteschlüssel bleiben der Weg, auf dem Geräte sprechen.
- Eigene Geräte automatisch in die Liste aufnehmen, damit „Meine Geräte“ überall erscheinen, ohne dass man sich einmal verbunden hat.
