# Windows-Dienst

Ziel: Fernzugriff auch auf Anmeldebildschirm, Sperrbildschirm und UAC-Dialoge, Strg+Alt+Entf, unbeaufsichtigter Betrieb ohne angemeldeten Benutzer.

## Entscheidungen (2. Oktober 2026)

| Frage | Entscheidung |
|---|---|
| Aufteilung | Dienst und Bildschirm-Agent, verbunden über eine Named Pipe, die nur SYSTEM öffnen kann |
| Installation | Der Dienst wird **immer** installiert, der Installer läuft pro Rechner mit Adminrechten |
| Festes Passwort und Serveradresse | ändern nur Administratoren (UAC-Abfrage) |
| Angezeigte Sitzung | die Konsole, also der physische Bildschirm. Ohne angemeldeten Benutzer ist das der Anmeldebildschirm. RDP-Sitzungen werden nicht angezeigt. |

## Aufbau

```
Server ◀─TCP─▶ Dienst  (ctxremote-service.exe, LocalSystem, Session 0)
                 │  Anmeldung beim Server, Passwörter, Sperre, SPAKE2 + E2E
                 │  \\.\pipe\ctxremote-agent   (ACL: nur SYSTEM)
                 ▼
               Agent   (ctxremote-service.exe --agent, SYSTEM, Konsolensitzung)
                 │  DXGI-Aufnahme, H.264, SendInput, Zwischenablage
                 │  folgt dem Eingabedesktop: Default ↔ Winlogon (Sperre, UAC)
                 │
App (Benutzer) ◀─ \\.\pipe\ctxremote-ui (ACL: angemeldete Benutzer lesen,
                    Admin-Befehle nur über einen erhöhten Aufruf)
```

- **Dienst:** übernimmt Anmeldung, Passwörter, Sperre, Handshake und Verschlüsselung aus dem heutigen `host.rs`. Pro Sitzung leitet er entschlüsselte `ViewerMsg` an den Agenten und reicht dessen `HostMsg` verschlüsselt weiter.
- **Agent:** übernimmt den heutigen Sitzungsteil (`run_session` ohne Netzwerk): Aufnahme, Encoder, Eingabe, Zwischenablage. Er läuft als SYSTEM in der Konsolensitzung. Nur so darf er per `OpenInputDesktop` und `SetThreadDesktop` auf den Winlogon-Desktop wechseln. Wechselt der Eingabedesktop (`DXGI_ERROR_ACCESS_LOST`, Prüfung der Desktop-Namen), ordnen sich der Aufnahme- und der Eingabe-Thread neu zu.
- **Agent starten:** Der Dienst dupliziert sein eigenes Token, setzt `TokenSessionId` auf `WTSGetActiveConsoleSessionId()` und startet den Agenten per `CreateProcessAsUserW` auf `winsta0\default`. Bei `WTS_CONSOLE_CONNECT`, An- und Abmeldung startet er den Agenten in der neuen Sitzung neu. Ein Agent lebt nur so lange, wie eine Sitzung läuft (Start bei Bedarf).
- **Strg+Alt+Entf:** löst der Dienst per `SendSAS(FALSE)` aus. Dafür setzt der Installer `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\SoftwareSASGeneration = 1`. Das Protokoll bekommt dazu eine neue `ViewerMsg`-Variante.
- **App:** Ist der Dienst erreichbar, startet die App keinen eigenen Host. Sie zeigt ID, Einmal-Passwort, Status und laufende Sitzungen über die UI-Pipe an. Für das feste Passwort und die Serveradresse startet sie `ctxremote-service.exe --configure …` mit UAC-Abfrage. Ohne Dienst, also in der Entwicklung oder mit `CTXREMOTE_CONFIG`, bleibt alles wie bisher.
- **Konfiguration des Geräts:** liegt in `%ProgramData%\CTXRemote\host.json` und ist nur für SYSTEM und Administratoren lesbar. Enthalten sind Server, Geräte-ID, Geräteschlüssel und festes Passwort. Bei der Installation werden ID und Schlüssel aus der Benutzerkonfiguration übernommen, falls es sie schon gibt, damit die ID erhalten bleibt. Aliase und die Geräteliste bleiben pro Benutzer.

## Umsetzung in Schritten

Stand 2. Oktober 2026: Alle Schritte sind umgesetzt und auf dem Windows Server getestet.

1. **Sitzung von der Übertragung trennen** (`crates/core`): `run_session` aufteilen in die Netzseite und eine Agent-Seite, die nur `ViewerMsg` liest und `HostMsg` schreibt, über einen allgemeinen Kanal. In der App verbinden In-Process-Kanäle beide Seiten, das Verhalten bleibt unverändert.
2. **Crate `crates/service`** (`windows-service`): Dienstmodus, `--agent`, `--install` und `--uninstall`, `--configure`. Pipe-Framing wie im Protokoll (postcard).
3. **Agent:** dem Eingabedesktop folgen, Start in der Konsolensitzung, Neustart bei einem Sitzungswechsel.
4. **UI-Pipe und App-Anbindung:** Status, ID und Passwort, Sitzungen, Sitzung beenden. Admin-Befehle laufen über UAC.
5. **Strg+Alt+Entf:** Protokollvariante, `SendSAS`, Schaltfläche im Sitzungsfenster.
6. **Installer:** NSIS mit `installMode: perMachine` und Hooks, die den Dienst installieren, starten und entfernen sowie `SoftwareSASGeneration` setzen.
7. **Test auf dem Windows Server:** Sperrbildschirm, Abmelden und Anmelden, UAC-Dialog, Strg+Alt+Entf, Neustart ohne Anmeldung.

## Offene Punkte

- Das feste Passwort ist in `host.json` weiterhin Klartext, aber nur für SYSTEM und Administratoren lesbar. Zusätzlich DPAPI mit Maschinenbindung wäre möglich.
- Wenn an der Konsole niemand angemeldet ist und eine RDP-Sitzung läuft, zeigt der Dienst den Anmeldebildschirm der Konsole.
