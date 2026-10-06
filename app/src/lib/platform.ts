/** Phones and tablets: one window, touch input, no tray. */
export const MOBILE = /Android|iPhone|iPad/.test(navigator.userAgent);

/** On a phone: back to the start page (a session or file view replaced it). */
export function goHome() {
  location.hash = "";
  location.reload();
}
