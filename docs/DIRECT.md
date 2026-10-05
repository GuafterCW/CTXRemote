# Direktverbindung und Fähigkeiten

Stand: 5. Oktober 2026. Der Code liegt in `crates/core/src/direct.rs` (TCP), `crates/core/src/punch.rs` (durch NAT), die Sitzungslogik in `host.rs` (`run_session`) und `viewer.rs`, der Test in `crates/server/tests/sessions.rs`.

## Ablauf

1. Jede Sitzung beginnt wie bisher über den Server (Relay) mit dem SPAKE2-Handshake.
2. Hat der Host einen Listener (Standard: TCP-Port **21301**) und meldet der Viewer die Fähigkeit `DIRECT`, schickt der Host nach `Welcome` innerhalb der verschlüsselten Sitzung `DirectOffer { addrs, token }`:
   - `addrs` enthält die Adresse der Netzwerkkarte Richtung Server, eine öffentliche IPv6-Adresse (falls vorhanden) und die Einträge aus `direct_addresses`.
   - `token` ist ein zufälliges Einmal-Token aus 32 Bytes.
3. Der Viewer probiert bis zu 8 Adressen gleichzeitig, jeweils mit 3 s Zeitlimit. Er sendet ein Klartext-`DirectHello { token }`, der Host antwortet mit demselben Token. Erst nach dieser Bestätigung gilt die Verbindung, damit ein anderes Gerät unter derselben LAN-Adresse nichts durcheinanderbringt.
4. Erst nach dieser Bestätigung sendet der Viewer `Switch` als **letzte** Nachricht über den Relay und sendet ab dann direkt.
   - Der Host antwortet mit seinem eigenen `Switch` über den Relay und folgt ebenfalls.
   - Gelesen wird der Relay jeweils bis zum `Switch` der Gegenseite, danach die Direktverbindung.
   - Die Schlüssel und Nonce-Zähler laufen einfach weiter (`SecureSender::reroute`, `SecureReceiver::reroute`). Die Direktverbindung ist deshalb genauso vertraulich wie der Relay.
   - Den Wechsel startet bewusst nur der Viewer. Gibt er kurz vor der Bestätigung auf, bleibt die Sitzung auf dem Relay, statt abzubrechen.
5. Klappt keine Adresse, bleibt die Sitzung ohne Meldung auf dem Relay.

## Durch NAT ohne Portweiterleitung (UDP-Hole-Punching)

Sitzen beide Seiten hinter einem Router, erreicht der Viewer den TCP-Port des Hosts nicht. Dafür gibt es seit 0.1.9 einen zweiten Weg, der parallel zum TCP-Versuch läuft. Wer zuerst bestätigt ist, gewinnt.

1. Kennen beide Seiten das Bit `PUNCH`, öffnet der Host nach dem `DirectOffer` einen UDP-Socket und fragt den **UDP-Reflektor des Servers** (gleicher Port wie TCP, also UDP 21300), unter welcher öffentlichen Adresse er ankommt (`proto::reflect`). Anfragen sind 64 Bytes groß, Antworten kleiner, damit sich der Reflektor nicht zur Verstärkung missbrauchen lässt.
2. Der Host schickt `PunchOffer { candidates, cert }` in der verschlüsselten Sitzung: die öffentliche Adresse und die LAN-Adressen des Sockets sowie den SHA-256 seines frisch erzeugten, selbstsignierten QUIC-Zertifikats.
3. Der Viewer macht dasselbe mit seinem eigenen Socket und antwortet mit `PunchAnswer { candidates }`.
4. Beide schicken ein paar UDP-Pakete an die Adressen der Gegenseite. Damit lässt ihr eigener Router die Antworten herein, ebenso die Windows-Firewall, die UDP-Antworten zustandsbehaftet erlaubt.
5. Der Viewer baut eine QUIC-Verbindung (quinn, rustls mit ring) zum Host auf. Er akzeptiert nur das Zertifikat mit dem Hash aus Schritt 2. Auf dem QUIC-Stream läuft dann dieselbe Token-Prüfung wie bei TCP (`DirectListener::admit_punched`), danach derselbe `Switch`.
6. Zeitlimit für alles: 10 s. Keep-Alive alle 5 s hält die NAT-Zuordnung offen.

Grenzen:
- Funktioniert mit üblichen Heimroutern (gleiche öffentliche Portzuordnung für alle Ziele). Bei symmetrischem NAT (manche Mobilfunknetze, Firmen-Firewalls) bleibt die Sitzung auf dem Relay.
- Nur IPv4. Öffentliches IPv6 deckt schon der TCP-Weg ab.
- Die **Schnellhilfe** bietet als Host nur den UDP-Weg an, ohne TCP-Listener (`direct_listen: false`, `DirectListener::without_tcp`). Sie öffnet also keinen Port, der eine Firewall-Abfrage auslösen würde, und ist trotzdem direkt erreichbar. Das `DirectOffer` hat dann keine Adressen und trägt nur das Token. Es geht nur an Viewer mit dem Bit `PUNCH`, ältere Viewer bekommen nichts. Test: `host_without_listener_is_reached_through_udp`.
- Ein Server ohne Reflektor (vor 0.1.9) oder ohne UDP-Freigabe in der Firewall: Der UDP-Weg wird stillschweigend ausgelassen.

**Zum Ausprobieren im LAN:** Mit der Umgebungsvariable `CTXREMOTE_DIRECT=udp` lässt der Viewer den TCP-Versuch weg und nimmt nur den UDP-Weg. Das nutzt auch der Test `cargo test -p ctxremote-server --test punch`.

Für den TCP-Weg muss der Server nicht aktualisiert werden, für den UDP-Weg braucht er den Reflektor.

## Sicherheit des Listeners

- Läuft im Dienstmodus als SYSTEM und ist aus dem Netz erreichbar. Ein Fremder darf genau einen Frame von höchstens 256 Bytes senden, mit 3 s Zeitlimit. Offen sind gleichzeitig höchstens 16 solcher Verbindungen und höchstens 4 je Quelladresse, alle weiteren werden sofort geschlossen.
- Das Token kommt nur innerhalb der verschlüsselten Sitzung zum Viewer, gilt einmal und verfällt mit dem Ende der Sitzung (`Drop for Offer`).
- Wer das `DirectHello` im LAN mitliest und das Token zuerst einlöst, verhindert nur die Direktverbindung, denn der echte Viewer bekommt dann keine Bestätigung und bleibt auf dem Relay. Mitlesen oder eigene Eingaben einschleusen kann er nicht, weil alle Sitzungsdaten verschlüsselt und authentifiziert sind.

## Einstellungen (`config.json` bzw. `C:\ProgramData\CTXRemote\host.json`)

| Feld | Standard | Bedeutung |
|---|---|---|
| `direct` | `true` | Direktverbindungen anbieten. Die Schnellhilfe schaltet sie immer ab. |
| `direct_port` | `21301` | Port des Listeners. Für Direktverbindungen aus dem Internet am Router weiterleiten. |
| `direct_addresses` | `[]` | Zusätzliche Adressen im Format `host:port`, z. B. `["meinhaus.dyndns.org:21301"]` bei Portweiterleitung |

Die Felder sind in der App unter Einstellungen > Direktverbindung bearbeitbar und wirken ohne Neustart (der Listener wird neu gestartet, laufende Sitzungen bleiben). Im Dienstmodus braucht das Speichern eine Administrator-Bestätigung; der erhöhte Helfer passt dann auch die Firewall-Regel an. `ctxremote-service --install` legt die Windows-Firewall-Regel „CTXRemote Direktverbindung“ für den Dienst an, `--uninstall` entfernt sie. Ohne Dienst fragt Windows beim ersten Start nach der Firewall-Freigabe.

## Fähigkeiten-Aushandlung (`Features`)

- postcard kann unbekannte Enum-Varianten nicht überspringen. Eine Nachricht, die die Gegenstelle nicht kennt, beendet deshalb deren Sitzung.
- Deshalb hängen beide Seiten an `Hello` und `Welcome` ein `Features`-Bitfeld an. Ältere Versionen ignorieren diesen Anhang.
- Der Host schickt `Cursor` und `DirectOffer` nur an Viewer, die das jeweilige Bit gesetzt haben.
- Der Viewer schickt Dateinachrichten, `Restart` und `SetQuality` nur an Hosts, die sie kennen. Fehlt die Fähigkeit, blendet das Sitzungsfenster den Knopf aus. Dateianfragen enden mit „braucht eine neuere CTXRemote-Version“.
- **Neue Funktionen:** Neue Variante anhängen, ein neues Bit in `Features` anlegen, es in `Features::CURRENT` aufnehmen und das Senden davon abhängig machen.

## Grenzen und nächste Schritte

- Hinter symmetrischem NAT gibt es keinen Direktweg. Dafür bräuchte es einen eigenen Relay-Dienst nahe am Nutzer (TURN), was hier der Server schon ist.
- Getestet unter Linux: Wechsel mit echtem Server, danach 50 Nachrichten in Folge. Ohne Listener bleibt die Sitzung auf dem Relay. **Unter Windows noch nicht getestet**, vor allem nicht Firewall, IPv6 und Dienstmodus.
