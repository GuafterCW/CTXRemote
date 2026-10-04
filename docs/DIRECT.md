# Direktverbindung und Fähigkeiten

Stand: 4. Oktober 2026. Der Code liegt in `crates/core/src/direct.rs`, die Sitzungslogik in `host.rs` (`run_session`) und `viewer.rs`, der Test in `crates/server/tests/sessions.rs`.

## Ablauf

1. Jede Sitzung beginnt wie bisher über den Server (Relay) mit dem SPAKE2-Handshake.
2. Hat der Host einen Listener (Standard: TCP-Port **21301**) und meldet der Viewer die Fähigkeit `DIRECT`, schickt der Host nach `Welcome` innerhalb der verschlüsselten Sitzung `DirectOffer { addrs, token }`:
   - `addrs` enthält die Adresse der Netzwerkkarte Richtung Server, eine öffentliche IPv6-Adresse (falls vorhanden) und die Einträge aus `direct_addresses`.
   - `token` ist ein zufälliges Einmal-Token aus 32 Bytes.
3. Der Viewer probiert bis zu 8 Adressen gleichzeitig, jeweils mit 3 s Zeitlimit. Er sendet ein Klartext-`DirectHello { token }`, der Host antwortet mit demselben Token. Erst nach dieser Bestätigung gilt die Verbindung, damit ein anderes Gerät unter derselben LAN-Adresse nichts durcheinanderbringt.
4. Beide Seiten senden `Switch` als **letzte** Nachricht über den Relay, schließen ihn und senden ab dann direkt. Gelesen wird der Relay bis zum `Switch` der Gegenseite, danach die Direktverbindung. Die Schlüssel und Nonce-Zähler laufen einfach weiter (`SecureSender::reroute`, `SecureReceiver::reroute`). Die Direktverbindung ist deshalb genauso vertraulich wie der Relay.
5. Klappt keine Adresse, bleibt die Sitzung ohne Meldung auf dem Relay.

Der Server bleibt unverändert und muss nicht aktualisiert werden.

## Sicherheit des Listeners

- Läuft im Dienstmodus als SYSTEM und ist aus dem Netz erreichbar. Ein Fremder darf genau einen Frame von höchstens 256 Bytes senden, mit 5 s Zeitlimit. Höchstens 16 solcher Verbindungen sind gleichzeitig offen, alle weiteren werden sofort geschlossen.
- Das Token kommt nur innerhalb der verschlüsselten Sitzung zum Viewer, gilt einmal und verfällt mit dem Ende der Sitzung (`Drop for Offer`).
- Wer das `DirectHello` im LAN mitliest, kann die Verbindung höchstens stören (Sitzungsabbruch). Mitlesen oder eigene Eingaben einschleusen kann er nicht, weil alle Sitzungsdaten verschlüsselt und authentifiziert sind.

## Einstellungen (`config.json` bzw. `C:\ProgramData\CTXRemote\host.json`)

| Feld | Standard | Bedeutung |
|---|---|---|
| `direct` | `true` | Direktverbindungen anbieten. Die Schnellhilfe schaltet sie immer ab. |
| `direct_port` | `21301` | Port des Listeners. Für Direktverbindungen aus dem Internet am Router weiterleiten. |
| `direct_addresses` | `[]` | Zusätzliche Adressen im Format `host:port`, z. B. `["meinhaus.dyndns.org:21301"]` bei Portweiterleitung |

Eine Oberfläche für diese Felder gibt es noch nicht. `ctxremote-service --install` legt die Windows-Firewall-Regel „CTXRemote Direktverbindung“ für den Dienst an, `--uninstall` entfernt sie. Ohne Dienst fragt Windows beim ersten Start nach der Firewall-Freigabe.

## Fähigkeiten-Aushandlung (`Features`)

- postcard kann unbekannte Enum-Varianten nicht überspringen. Eine Nachricht, die die Gegenstelle nicht kennt, beendet deshalb deren Sitzung.
- Deshalb hängen beide Seiten an `Hello` und `Welcome` ein `Features`-Bitfeld an. Ältere Versionen ignorieren diesen Anhang.
- Der Host schickt `Cursor` und `DirectOffer` nur an Viewer, die das jeweilige Bit gesetzt haben.
- Der Viewer schickt Dateinachrichten, `Restart` und `SetQuality` nur an Hosts, die sie kennen. Fehlt die Fähigkeit, blendet das Sitzungsfenster den Knopf aus. Dateianfragen enden mit „braucht eine neuere CTXRemote-Version“.
- **Neue Funktionen:** Neue Variante anhängen, ein neues Bit in `Features` anlegen, es in `Features::CURRENT` aufnehmen und das Senden davon abhängig machen.

## Grenzen und nächste Schritte

- Hinter zwei NATs (typischer Heimanschluss nur mit IPv4) klappt die Direktverbindung nur mit Portweiterleitung. Ohne Weiterleitung bräuchte es UDP-Hole-Punching, das wäre eine eigene Transportschicht (z. B. QUIC).
- Der Server könnte dem Viewer die öffentliche IP des Hosts mitteilen. Das bräuchte aber eine Server-Änderung.
- Getestet unter Linux: Wechsel mit echtem Server, danach 50 Nachrichten in Folge. Ohne Listener bleibt die Sitzung auf dem Relay. **Unter Windows noch nicht getestet**, vor allem nicht Firewall, IPv6 und Dienstmodus.
