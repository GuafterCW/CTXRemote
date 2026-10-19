import { RIGHT } from "./api";

/**
 * Rights that hand over more than the screen does: switching one on asks
 * first, with what it means for this computer (see `RiskConfirm`).
 */
export const RISKY: Record<number, { title: string; points: string[] }> = {
  [RIGHT.PROCESSES]: {
    title: "Prozesse und Dienste erlauben?",
    points: [
      "Die Gegenseite kann auf diesem Gerät Programme beenden und Dienste anhalten, starten oder neu starten.",
      "Unter Windows geschieht das mit Administratorrechten, also mit mehr Rechten, als der angemeldete Benutzer selbst hat. Damit lassen sich zum Beispiel auch Virenschutz, Firewall oder Sicherungen anhalten.",
      "Nicht gespeicherte Arbeit in beendeten Programmen geht verloren.",
    ],
  },
  [RIGHT.TERMINAL]: {
    title: "Terminal erlauben?",
    points: [
      "Die Gegenseite kann auf diesem Gerät beliebige Befehle ausführen, mit allen Rechten des angemeldeten Benutzers: Dateien lesen, ändern und löschen, Programme installieren und starten.",
      "Was im Terminal geschieht, ist auf dem Bildschirm hier nicht zu sehen.",
    ],
  },
  [RIGHT.TUNNEL]: {
    title: "Port-Tunnel erlauben?",
    points: [
      "Die Gegenseite kann über dieses Gerät Verbindungen zu anderen Geräten in diesem Netzwerk aufbauen, etwa zu Router, Drucker oder NAS.",
      "Das gilt auch für Geräte, die sonst nur von innen erreichbar sind.",
    ],
  },
};
