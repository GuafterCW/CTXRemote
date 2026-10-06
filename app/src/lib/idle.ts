/** Ends sessions without input from this computer after a while (Settings). */
export const IDLE_KEY = "ctxremote.idleMinutes";

/** Minutes without input before a session ends; 0 = never. */
export function readIdleMinutes(): number {
  try {
    const minutes = Number(localStorage.getItem(IDLE_KEY) ?? 0);
    return Number.isFinite(minutes) && minutes > 0 ? minutes : 0;
  } catch {
    return 0;
  }
}
