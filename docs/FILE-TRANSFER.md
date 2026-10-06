# Dateiübertragung

Stand: 4. Oktober 2026. Der Code liegt in `crates/core/src/files/`, die Oberfläche in `app/src/Files.svelte` und `app/src/lib/FilePane.svelte`.

## Bedienung

- Das Sitzungsfenster hat in der Toolbar den Knopf **Dateien**. Er öffnet ein eigenes Fenster (`files-{n}`) mit zwei Spalten: links dieser Computer, rechts das ferngesteuerte Gerät.
- **Hochladen →** überträgt die Auswahl links in den geöffneten Ordner rechts, **← Herunterladen** geht in die andere Richtung. Ein Doppelklick auf eine Datei überträgt sie ebenfalls auf die andere Seite.
- Dateien aus dem Explorer lassen sich auf die rechte Spalte ziehen. Fallen sie auf das Sitzungsfenster, landen sie auf dem Desktop des fernen Geräts, und das Dateifenster zeigt den Fortschritt.
- In beiden Spalten gibt es Neuer Ordner, Umbenennen (F2), Löschen (Entf, mit Rückfrage), Backspace für eine Ebene nach oben und Strg/Shift für Mehrfachauswahl.
- Ganze Ordner werden mitsamt Unterordnern übertragen. Bestehende Dateien werden nie überschrieben: Ist der Name schon vergeben, heißt das Ziel `Name (2).ext`.

## Protokoll (`crates/proto/src/session.rs`)

| Nachricht | Richtung | Zweck |
|---|---|---|
| `ViewerMsg::File { req, op }` | V → H | `List`, `CreateDir`, `Rename`, `Delete`, `Download { id, path }`, `Upload { id, dir }` |
| `HostMsg::FileReply { req, result }` | H → V | `Listing` oder `Done`, sonst Fehlertext |
| `…::Transfer { id, msg }` | beide | Inhalt einer Übertragung: `Start { total }`, `Dir { rel }`, `File { rel, size }`, `Data`, `End`, `Failed`, `Cancel` |
| `HostMsg::TransferAck { id, bytes }` | H → V | geschriebene Bytes eines Uploads |

- Eine Übertragung umfasst genau eine Datei oder einen Ordnerbaum. Pfade darin sind relativ, mit `/` getrennt, ohne `..`. Der Empfänger prüft jeden Teil mit `join_relative` und schreibt nur unterhalb des Zielordners.
- Einzeldateien entstehen zunächst als `Name.<Größe>.ctxpart` und werden am Ende umbenannt. Bei Abbruch oder Fehler löscht der Empfänger alles, was er für diese Übertragung angelegt hat (`Drop for Incoming`). Nur eine beim Sitzungsende unterbrochene Einzeldatei bleibt zum Fortsetzen liegen (siehe unten).
- Flusskontrolle beim Upload: Höchstens `WINDOW` = 1 MiB darf unbestätigt unterwegs sein, weil Eingaben sich dahinter anstellen. Downloads bremst der Host über den kleinen Ausgangskanal des Agents.
- Die neuen Varianten sind **angehängt**, `PROTOCOL_VERSION` bleibt 1. Ein alter Host, der eine Dateinachricht bekommt, beendet die Sitzung mit „ungültige Nachricht“. Beide Seiten müssen also aktualisiert sein.

## Fortsetzen nach einem Abbruch (6. Oktober)

- Gilt für **einzelne Dateien** in beide Richtungen, wenn beide Seiten die Fähigkeit `RESUME` haben. Ordner beginnen nach einem Abbruch von vorn.
- Die angefangene Datei heißt `Name.<Größe>.ctxpart`. Endet die **Sitzung** mitten in der Übertragung, bleibt sie liegen. Bei **Abbrechen** oder einem Fehler wird sie wie bisher gelöscht.
- **Download:** Der Viewer findet im Zielordner eine passende Teildatei (`find_part`) und schickt `FileOp::DownloadFrom { offset, size }`. Der Host setzt nur fort, wenn seine Datei noch genau `size` Bytes hat. Er antwortet mit `FileReply::Offset` (0 heißt von vorn) und schickt nur den Rest. Passt es nicht mehr, löscht der Viewer das alte Stück.
- **Upload:** Der Viewer fragt mit `FileOp::UploadFrom { name, size }`, der Host antwortet mit der Länge seiner Teildatei, und der Viewer schickt ab dort. Der Host behält Teildateien beim Sitzungsende nur für Uploads, die so begonnen wurden. Ältere Viewer hinterlassen also nichts.
- Der Empfänger prüft, dass die Teildatei genau die angekündigte Länge hat. Sonst bricht er ab, statt eine falsche Datei zusammenzusetzen.
- Getestet: `an_interrupted_file_continues` (Stream), `an_interrupted_download_continues` und `an_interrupted_upload_continues` (Client mit Host-Dienst).

## Host-Seite und Rechte

- `FileService` läuft im Agent (`agent.rs`) auf einem eigenen Thread und startet erst mit der ersten Dateianfrage.
- **Dienstmodus:** Der Agent läuft als SYSTEM. Der Datei-Thread übernimmt deshalb per `WTSQueryUserToken` und `ImpersonateLoggedOnUser` die Rechte des angemeldeten Benutzers (`files/user.rs`). Ohne angemeldeten Benutzer werden Dateianfragen abgelehnt. Ein Viewer kann also nie mehr als der Benutzer an der Tastatur.
- Die Orte (Persönlicher Ordner, Desktop, Dokumente, Downloads) kommen per `SHGetKnownFolderPath` mit dem Token des Benutzers. Umgeleitete Ordner wie OneDrive werden so korrekt aufgelöst.

## Getestet

- Unit- und Ende-zu-Ende-Tests unter Linux (`cargo test -p ctxremote-core files`): Baum hin und zurück, Namenskonflikte, Pfadausbrüche, Abbruch, Verbindungsende.
- Windows-Code: nur Typprüfung (`scripts/check-windows.sh`). **Auf echtem Windows noch nicht ausgeführt.**
- Oberfläche: Screenshots in Chromium mit nachgebildeter Tauri-API, in hell und dunkel. In der echten App noch nicht ausgeführt.
