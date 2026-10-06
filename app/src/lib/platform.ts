/** Phones and tablets: one window, touch input, no tray. */
export const MOBILE = /Android|iPhone|iPad/.test(navigator.userAgent);

/** On a phone: back to the start page (a session or file view replaced it).
 * Replaces the current entry, so "back" on the start page does not return
 * to the ended session. */
export function goHome() {
  location.replace(location.href.split("#")[0]);
}
