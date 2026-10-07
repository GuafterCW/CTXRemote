// The signed-in account in this browser tab. The account key lives in
// memory; for reloads and other tabs it is also kept XORed with a random pad
// of the server session in localStorage. Without that session (logout,
// expiry, password change, server restart) what is stored is worthless. Everything sent to the server is either a derived `auth`
// value or sealed with the account key (see docs/ACCOUNTS.md).

import {
  AAD,
  fromBase64,
  loginSetup,
  newPairingCode,
  normalizeEmail,
  open,
  pairingKeys,
  passwordKeys,
  random,
  recoveryKeys,
  seal,
  toBase64,
  type Kdf,
} from "./crypto";

let accountKey: Uint8Array | null = null;

const STORED = "ctxremote.key";

function xor(a: Uint8Array, b: Uint8Array): Uint8Array {
  return a.map((x, i) => x ^ b[i]);
}

/** Keeps the key for a reload, masked with the session's pad. */
function keep(key: Uint8Array, pad: string) {
  accountKey = key;
  try {
    const p = fromBase64(pad);
    if (p.length === key.length) localStorage.setItem(STORED, toBase64(xor(key, p)));
  } catch {
    // No storage (private mode, blocked): a reload then asks for the password.
  }
}

function forget() {
  accountKey = null;
  try {
    localStorage.removeItem(STORED);
  } catch {
    // Nothing stored.
  }
}

function stored(): string | null {
  try {
    return localStorage.getItem(STORED);
  } catch {
    return null;
  }
}

/** Calls `onChange` when another tab signs out (or in as someone else). */
export function watchOtherTabs(onChange: () => void) {
  window.addEventListener("storage", (event) => {
    if (event.key === STORED || event.key === null) onChange();
  });
}

/** Whether this tab may still be signed in from before a reload. */
export function mayRestore(): boolean {
  return accountKey === null && stored() !== null;
}

/** After a reload: unmasks the kept key with the session's pad. */
export async function restore(): Promise<boolean> {
  if (accountKey) return true;
  const masked = stored();
  if (!masked) return false;
  try {
    const { pad } = await call<{ pad: string }>("GET", "/session-pad");
    const m = fromBase64(masked);
    const p = fromBase64(pad);
    if (m.length !== 32 || p.length !== 32) throw new ApiError("Bitte erneut anmelden", 401);
    accountKey = xor(m, p);
    return true;
  } catch (err) {
    if (err instanceof ApiError && err.status === 401) forget();
    return false;
  }
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
  ) {
    super(message);
  }
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    credentials: "same-origin",
    headers: body === undefined ? {} : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    // A wrong password is a 401 too, but no reason to sign out.
    if (response.status === 401 && !["/login", "/recover", "/account/delete", "/login-setup"].includes(path)) forget();
    throw new ApiError(data.error ?? `Fehler ${response.status}`, response.status);
  }
  return data as T;
}

function key(): Uint8Array {
  if (!accountKey) throw new ApiError("Bitte erneut anmelden", 401);
  return accountKey;
}

export function signedIn(): boolean {
  return accountKey !== null;
}

type Setup = Awaited<ReturnType<typeof loginSetup>>["setup"];

function setupJson(s: Setup) {
  return {
    email: s.email,
    salt: toBase64(s.salt),
    kdf: s.kdf,
    auth: toBase64(s.auth),
    wrapped: toBase64(s.wrapped),
    recoveryAuth: toBase64(s.recoveryAuth),
    recoveryWrapped: toBase64(s.recoveryWrapped),
  };
}

/** Creates an account; returns the recovery code to show once. */
export async function register(email: string, password: string): Promise<string> {
  const fresh = random(32);
  const { setup, code } = await loginSetup(email, password, fresh);
  const { pad } = await call<{ pad: string }>("POST", "/register", setupJson(setup));
  keep(fresh, pad);
  return code;
}

export async function login(email: string, password: string): Promise<void> {
  const normalized = normalizeEmail(email);
  if (!normalized) throw new ApiError("Bitte eine gültige E-Mail-Adresse angeben", 400);
  const pre = await call<{ salt: string; kdf: Kdf }>("POST", "/prelogin", { email: normalized });
  const { auth, wrap } = await passwordKeys(password, fromBase64(pre.salt), pre.kdf);
  const { wrapped, pad } = await call<{ wrapped: string; pad: string }>("POST", "/login", { email: normalized, auth: toBase64(auth) });
  keep(open(wrap, AAD.login, fromBase64(wrapped)), pad);
}

/** Password forgotten: signs in with the recovery code and sets a new
 * password. Returns the new recovery code (the old one stops working). */
export async function recover(email: string, code: string, password: string): Promise<string> {
  const normalized = normalizeEmail(email);
  if (!normalized) throw new ApiError("Bitte eine gültige E-Mail-Adresse angeben", 400);
  const { auth, wrap } = recoveryKeys(code);
  const { wrapped, pad } = await call<{ wrapped: string; pad: string }>("POST", "/recover", { email: normalized, recoveryAuth: toBase64(auth) });
  keep(open(wrap, AAD.recovery, fromBase64(wrapped)), pad);
  return changeLogin(normalized, password);
}

/** New password or address; returns the new recovery code. Other browser
 * sessions of the account end. Unless the session has just begun, the
 * server wants the current password too (`current`, with the address it
 * belongs to). */
export async function changeLogin(
  email: string,
  password: string,
  current?: { email: string; password: string },
): Promise<string> {
  let currentAuth: string | undefined;
  if (current) {
    const pre = await call<{ salt: string; kdf: Kdf }>("POST", "/prelogin", { email: normalizeEmail(current.email) ?? current.email });
    currentAuth = toBase64((await passwordKeys(current.password, fromBase64(pre.salt), pre.kdf)).auth);
  }
  const { setup, code } = await loginSetup(email, password, key());
  await call("POST", "/login-setup", { ...setupJson(setup), current: currentAuth });
  return code;
}

/** Deletes the account for good; needs the password once more. */
export async function deleteAccount(email: string, password: string): Promise<void> {
  const normalized = normalizeEmail(email);
  if (!normalized) throw new ApiError("Unbekannte E-Mail-Adresse", 400);
  const pre = await call<{ salt: string; kdf: Kdf }>("POST", "/prelogin", { email: normalized });
  const { auth } = await passwordKeys(password, fromBase64(pre.salt), pre.kdf);
  await call("POST", "/account/delete", { auth: toBase64(auth) });
  forget();
}

export async function logout(): Promise<void> {
  forget();
  await call("POST", "/logout").catch(() => undefined);
}

export interface AccountInfo {
  email: string | null;
  verified: boolean;
  devices: number;
}

export function account(): Promise<AccountInfo> {
  return call("GET", "/account");
}

/** A device signed in to the account. */
export interface Device {
  publicKey: string;
  id: string | null;
  online: boolean;
  /** Its computer name, decrypted; "" if unknown. */
  name: string;
}

const decode = new TextDecoder();

export async function devices(): Promise<Device[]> {
  const list = await call<{ publicKey: string; id: string | null; online: boolean; label: string }[]>("GET", "/devices");
  return list.map((d) => {
    let name = "";
    try {
      name = d.label ? decode.decode(open(key(), AAD.label, fromBase64(d.label))) : "";
    } catch {
      // A label from before a key change cannot be read; show the device anyway.
    }
    return { publicKey: d.publicKey, id: d.id, online: d.online, name };
  });
}

export async function removeDevice(publicKey: string): Promise<void> {
  await call("DELETE", `/devices/${publicKey}`);
}

/** The shared device list, as the app stores it (`Book` in Rust). */
export interface Entry {
  alias: string | null;
  /** Unix milliseconds of the last name change. */
  alias_at: number;
  name: string;
  /** Unix seconds of the last connection. */
  last_seen: number;
}

export interface Book {
  entries: Record<string, Entry>;
  /** Device ID → Unix milliseconds of its removal. */
  removed: Record<string, number>;
  /** Device ID → whether it lets the account's devices in without a password. */
  access?: Record<string, { open: boolean; at: number }>;
  /** Fields of newer app versions: kept as they are when writing back. */
  [field: string]: unknown;
}

async function readBook(): Promise<{ revision: number; book: Book }> {
  const { revision, blob } = await call<{ revision: number; blob: string }>("GET", "/book");
  if (!blob) return { revision, book: { entries: {}, removed: {} } };
  const book = JSON.parse(decode.decode(open(key(), AAD.book, fromBase64(blob)))) as Book;
  // Keep every field, also ones this page does not know, or writing back would drop them.
  return { revision, book: { ...book, entries: book.entries ?? {}, removed: book.removed ?? {} } };
}

export async function loadBook(): Promise<Book> {
  return (await readBook()).book;
}

/** Applies `change` to the newest list and stores it; retries when a device
 * wrote in between. The app merges field by field, so timestamps matter. */
export async function updateBook(change: (book: Book) => void): Promise<Book> {
  for (let attempt = 0; attempt < 4; attempt++) {
    const { revision, book } = await readBook();
    change(book);
    const blob = seal(key(), AAD.book, new TextEncoder().encode(JSON.stringify(book)));
    try {
      await call("PUT", "/book", { base: revision, blob: toBase64(blob) });
      return book;
    } catch (err) {
      if (!(err instanceof ApiError) || err.status !== 409) throw err;
    }
  }
  throw new ApiError("Die Geräteliste ändert sich gerade ständig, bitte später nochmal versuchen", 409);
}

export function renameEntry(id: string, alias: string): Promise<Book> {
  return updateBook((book) => {
    const entry = book.entries[id];
    if (!entry) return;
    entry.alias = alias.trim() || null;
    entry.alias_at = Date.now();
  });
}

export function removeEntry(id: string): Promise<Book> {
  return updateBook((book) => {
    delete book.entries[id];
    book.removed[id] = Date.now();
  });
}

/** A one-time code to add a device; valid for ten minutes. */
export async function pairingCode(): Promise<string> {
  for (let attempt = 0; attempt < 5; attempt++) {
    const code = newPairingCode();
    const salt = random(16);
    const { sealKey, proof } = await pairingKeys(code.secret, salt);
    const sealed = seal(sealKey, AAD.pairing, key());
    try {
      await call("POST", "/pairing", { codeId: code.codeId, salt: toBase64(salt), verifier: toBase64(proof), sealed: toBase64(sealed) });
      return code.display;
    } catch (err) {
      // That code name is in use by another account right now: pick another.
      if (!(err instanceof ApiError) || err.status !== 409) throw err;
    }
  }
  throw new ApiError("Kein freier Code gefunden, bitte nochmal versuchen", 409);
}

/** Confirms the address with the token from the mail's link. */
export async function verifyEmail(token: string): Promise<void> {
  await call("POST", "/verify", { token });
}

/** Sends the confirmation mail again (at most every few minutes). */
export async function resendVerification(): Promise<void> {
  await call("POST", "/verify/resend");
}
