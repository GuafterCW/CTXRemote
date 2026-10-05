// The signed-in account in this browser tab. The account key lives only in
// memory: reloading the page or closing the tab forgets it, so the password
// is needed again. Everything sent to the server is either a derived `auth`
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
    if (response.status === 401 && accountKey && path !== "/login") accountKey = null;
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
  await call("POST", "/register", setupJson(setup));
  accountKey = fresh;
  return code;
}

export async function login(email: string, password: string): Promise<void> {
  const normalized = normalizeEmail(email);
  if (!normalized) throw new ApiError("Bitte eine gültige E-Mail-Adresse angeben", 400);
  const pre = await call<{ salt: string; kdf: Kdf }>("POST", "/prelogin", { email: normalized });
  const { auth, wrap } = await passwordKeys(password, fromBase64(pre.salt), pre.kdf);
  const { wrapped } = await call<{ wrapped: string }>("POST", "/login", { email: normalized, auth: toBase64(auth) });
  accountKey = open(wrap, AAD.login, fromBase64(wrapped));
}

/** Password forgotten: signs in with the recovery code and sets a new
 * password. Returns the new recovery code (the old one stops working). */
export async function recover(email: string, code: string, password: string): Promise<string> {
  const normalized = normalizeEmail(email);
  if (!normalized) throw new ApiError("Bitte eine gültige E-Mail-Adresse angeben", 400);
  const { auth, wrap } = recoveryKeys(code);
  const { wrapped } = await call<{ wrapped: string }>("POST", "/recover", { email: normalized, recoveryAuth: toBase64(auth) });
  accountKey = open(wrap, AAD.recovery, fromBase64(wrapped));
  return changeLogin(normalized, password);
}

/** New password or address; returns the new recovery code. Other browser
 * sessions of the account end. */
export async function changeLogin(email: string, password: string): Promise<string> {
  const { setup, code } = await loginSetup(email, password, key());
  await call("POST", "/login-setup", setupJson(setup));
  return code;
}

export async function logout(): Promise<void> {
  accountKey = null;
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
}

async function readBook(): Promise<{ revision: number; book: Book }> {
  const { revision, blob } = await call<{ revision: number; blob: string }>("GET", "/book");
  if (!blob) return { revision, book: { entries: {}, removed: {} } };
  const book = JSON.parse(decode.decode(open(key(), AAD.book, fromBase64(blob)))) as Book;
  return { revision, book: { entries: book.entries ?? {}, removed: book.removed ?? {} } };
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
