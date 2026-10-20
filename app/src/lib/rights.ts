import { RIGHT } from "./api";

/** Bit, label, hint; `warn` marks a hint that must not be overlooked. */
export const RIGHTS: [number, string, string?, boolean?][] = [
  [RIGHT.INPUT, "Maus und Tastatur"],
  [RIGHT.FILES, "Dateien"],
  [RIGHT.CLIPBOARD, "Zwischenablage"],
  [RIGHT.AUDIO, "Ton"],
  [RIGHT.RESTART, "Neu starten"],
  [RIGHT.PRIVACY, "Bildschirm schwarz schalten"],
  [RIGHT.TUNNEL, "Port-Tunnel ins Netzwerk"],
  [RIGHT.TERMINAL, "Terminal", "Befehlszeile als angemeldeter Benutzer"],
  [RIGHT.PROCESSES, "Prozesse und Dienste", "Unter Windows mit Administratorrechten", true],
];
