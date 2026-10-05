// The account cryptography of the app (crates/core/src/account.rs), for the
// browser. Every function must give byte-identical results to its Rust
// counterpart; scripts/vectors.mjs checks this against values from Rust.
// Keys and passwords never leave this module except as sealed blobs and the
// server's `auth` values.

import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { hkdf } from "@noble/hashes/hkdf.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { argon2id } from "hash-wasm";

export const AAD = {
  login: "ctxremote/login/v1",
  recovery: "ctxremote/recovery/v1",
  book: "ctxremote/book/v1",
  pairing: "ctxremote/pairing/v1",
  label: "ctxremote/label/v1",
} as const;

/** Crockford base32, as in proto::account::CODE_ALPHABET. */
const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
export const MIN_PASSWORD = 10;

export interface Kdf {
  memoryKib: number;
  iterations: number;
  parallelism: number;
}

/** Kdf::CURRENT in Rust. */
export const KDF_CURRENT: Kdf = { memoryKib: 64 * 1024, iterations: 3, parallelism: 1 };

/** Kdf::acceptable: a server must not be able to weaken or inflate it. */
export function kdfAcceptable(k: Kdf): boolean {
  return (
    k.memoryKib >= 19 * 1024 &&
    k.memoryKib <= 1024 * 1024 &&
    k.iterations >= 2 &&
    k.iterations <= 10 &&
    k.parallelism >= 1 &&
    k.parallelism <= 4
  );
}

const text = new TextEncoder();

export function random(n: number): Uint8Array {
  const bytes = new Uint8Array(n);
  crypto.getRandomValues(bytes);
  return bytes;
}

function randomCode(length: number): string {
  // 256 is a multiple of 32, so taking the low five bits is unbiased.
  return Array.from(random(length), (b) => ALPHABET[b & 31]).join("");
}

async function argon(secret: string, salt: Uint8Array, k: Kdf, length: number): Promise<Uint8Array> {
  return argon2id({
    password: text.encode(secret),
    salt,
    parallelism: k.parallelism,
    iterations: k.iterations,
    memorySize: k.memoryKib,
    hashLength: length,
    outputType: "binary",
  });
}

/** `split` in Rust: two independent 32-byte values from `ikm`. */
function split(ikm: Uint8Array, salt: string): { auth: Uint8Array; wrap: Uint8Array } {
  return {
    auth: hkdf(sha256, ikm, text.encode(salt), text.encode("auth"), 32),
    wrap: hkdf(sha256, ikm, text.encode(salt), text.encode("wrap"), 32),
  };
}

/** `password_keys`: what the server checks, and the key that seals the account key. */
export async function passwordKeys(password: string, salt: Uint8Array, k: Kdf) {
  if (!kdfAcceptable(k)) throw new Error("Der Server verlangt ungültige Einstellungen für die Schlüsselableitung");
  return split(await argon(password, salt, k, 32), "ctxremote/login/v1");
}

/** Canonical form of a typed recovery code, or null. */
export function normalizeRecoveryCode(code: string): string | null {
  const clean = code.replace(/[^0-9a-z]/gi, "").toUpperCase();
  return clean.length === 25 && [...clean].every((c) => ALPHABET.includes(c)) ? clean : null;
}

/** `recovery_keys`: 125 random bits, so no slow hash. */
export function recoveryKeys(code: string) {
  const clean = normalizeRecoveryCode(code);
  if (!clean) throw new Error("Der Wiederherstellungscode hat 25 Zeichen, z. B. ABCDE-FGHJK-MNPQR-STVWX-YZ012");
  return split(text.encode(clean), "ctxremote/recovery/v1");
}

/** `new_recovery_code`: ABCDE-FGHJK-MNPQR-STVWX-YZ012. */
export function newRecoveryCode(): string {
  return randomCode(25).match(/.{5}/g)!.join("-");
}

/** `derive` for pairing codes: Argon2id over the secret part, 19 MiB, 2 passes. */
export async function pairingKeys(secret: string, salt: Uint8Array) {
  const out = await argon(secret, salt, { memoryKib: 19 * 1024, iterations: 2, parallelism: 1 }, 64);
  return { sealKey: out.slice(0, 32), proof: out.slice(32) };
}

/** A pairing code: 4 characters naming it on the server, 8 secret ones. */
export function newPairingCode(): { codeId: string; secret: string; display: string } {
  const code = randomCode(12);
  return { codeId: code.slice(0, 4), secret: code.slice(4), display: `${code.slice(0, 4)}-${code.slice(4, 8)}-${code.slice(8)}` };
}

/** `seal`: random nonce, then ChaCha20-Poly1305 with `aad`. */
export function seal(key: Uint8Array, aad: string, plain: Uint8Array): Uint8Array {
  const nonce = random(12);
  const sealed = chacha20poly1305(key, nonce, text.encode(aad)).encrypt(plain);
  const out = new Uint8Array(12 + sealed.length);
  out.set(nonce);
  out.set(sealed, 12);
  return out;
}

/** `open`: throws if the key or the data is wrong. */
export function open(key: Uint8Array, aad: string, sealed: Uint8Array): Uint8Array {
  if (sealed.length < 12 + 16) throw new Error("Daten sind beschädigt");
  return chacha20poly1305(key, sealed.slice(0, 12), text.encode(aad)).decrypt(sealed.slice(12));
}

/** `normalize_email` in proto::account. */
export function normalizeEmail(email: string): string | null {
  const e = email.trim().toLowerCase();
  const at = e.indexOf("@");
  if (at < 1 || e.indexOf("@", at + 1) !== -1) return null;
  const domain = e.slice(at + 1);
  const ok =
    domain.includes(".") && !domain.startsWith(".") && !domain.endsWith(".") && e.length <= 254 && !/[\s\p{Cc}]/u.test(e);
  return ok ? e : null;
}

/** Everything the server keeps for a login (`login_setup`), plus the new recovery code. */
export async function loginSetup(email: string, password: string, accountKey: Uint8Array) {
  const normalized = normalizeEmail(email);
  if (!normalized) throw new Error("Bitte eine gültige E-Mail-Adresse angeben");
  if ([...password].length < MIN_PASSWORD) throw new Error(`Das Passwort braucht mindestens ${MIN_PASSWORD} Zeichen`);
  const salt = random(16);
  const kdf = KDF_CURRENT;
  const { auth, wrap } = await passwordKeys(password, salt, kdf);
  const code = newRecoveryCode();
  const recovery = recoveryKeys(code);
  return {
    setup: {
      email: normalized,
      salt,
      kdf,
      auth,
      wrapped: seal(wrap, AAD.login, accountKey),
      recoveryAuth: recovery.auth,
      recoveryWrapped: seal(recovery.wrap, AAD.recovery, accountKey),
    },
    code,
  };
}

export function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

export function fromBase64(text: string): Uint8Array {
  return Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
}

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function fromHex(text: string): Uint8Array {
  return Uint8Array.from(text.match(/../g) ?? [], (h) => parseInt(h, 16));
}

export { sha256 };
