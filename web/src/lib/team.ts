// Teams in the browser: the same steps as crates/core/src/team.rs, with the
// same cryptography (lib/crypto.ts). The team key never reaches the server;
// requests go to `POST /api/team` as the serde JSON of `AccountOp`.

import {
  normalizeCode,
  open,
  openWith,
  pairingKeys,
  newPairingCode,
  random,
  seal,
  sealTo,
  TEAM_AAD,
  teamAad,
  teamAccessAad,
  teamContext,
  teamTag,
  teamTagOk,
  exchangePublic,
  exchangeSecret,
  toHex,
} from "./crypto";
import { ApiError, call, key, type Book } from "./session";

export type Role = "Technician" | "Admin" | "Owner";

/** How the server sends bytes: arrays of numbers. */
type Bytes = number[];

interface MemberKeyJson {
  public_key: Bytes;
  tag: Bytes;
  sealed: Bytes;
}

export interface Member {
  id: number;
  role: Role;
  email: string | null;
  key: [Bytes, Bytes] | null;
  /** Unix seconds. */
  since: number;
}

interface TeamState {
  id: Bytes;
  epoch: number;
  revision: number;
  blob: Bytes;
  me: number;
  sealed: Bytes | null;
  members: Member[];
  stale: boolean;
  access: [number, Bytes][];
}

/** The shared address book: the team's name and its devices (`TeamBook` in Rust). */
export interface TeamBook {
  name: string;
  book: Book;
  [field: string]: unknown;
}

/** A device a member opened for the team without a password. */
export interface OpenHost {
  member: number;
  name: string;
}

/** The team, decrypted. */
export interface TeamView {
  /** Hex. */
  id: string;
  epoch: number;
  revision: number;
  me: number;
  role: Role;
  book: TeamBook;
  members: Member[];
  /** Members the team key does not vouch for (just joined, not yet confirmed). */
  unverified: number[];
  stale: boolean;
  /** Device ID → who opened it. */
  access: Record<string, OpenHost>;
}

export const manages = (role: Role) => role === "Admin" || role === "Owner";

const encode = new TextEncoder();
const decode = new TextDecoder();

// Error texts of the server (AccountError), which is all the web API tells.
const STALE_KEY = "Der Teamschlüssel wurde inzwischen erneuert";
const NO_TEAM = "Dieses Konto gehört zu keinem Team";

const isConflict = (e: unknown) => e instanceof ApiError && e.status === 409;
const isStale = (e: unknown) => e instanceof ApiError && e.message.startsWith(STALE_KEY);
const isNoTeam = (e: unknown) => e instanceof ApiError && e.message === NO_TEAM;

/** The key of a team this tab just created or joined, for the moment before
 * the server has a sealed copy for this account (`TeamLink` in Rust). */
let cached: { id: string; epoch: number; key: Uint8Array } | null = null;

/** Forgets what this tab holds (sign-out). */
export function forgetTeam() {
  cached = null;
}

type Reply = Record<string, any> | string;

async function request(op: unknown): Promise<Reply> {
  return call<Reply>("POST", "/team", { op });
}

function variant(reply: Reply, name: string): any {
  if (typeof reply === "object" && name in reply) return reply[name];
  throw new Error(`unerwartete Serverantwort: ${JSON.stringify(reply)}`);
}

const bytes = (b: Bytes) => Uint8Array.from(b);
const list = (b: Uint8Array) => Array.from(b);
const idBytes = (state: TeamState) => bytes(state.id);

function memberKey(teamKey: Uint8Array, ctx: Uint8Array, member: number, publicKey: Uint8Array): MemberKeyJson {
  return { public_key: list(publicKey), tag: list(teamTag(teamKey, member, publicKey)), sealed: list(sealTo(publicKey, ctx, teamKey)) };
}

function pairOf(k: [Bytes, Bytes] | null): [Uint8Array, Uint8Array] | null {
  return k ? [bytes(k[0]), bytes(k[1])] : null;
}

/** Decrypts `state` with `teamKey` (`view` in Rust). */
function view(state: TeamState, teamKey: Uint8Array): TeamView {
  const id = idBytes(state);
  let book: TeamBook = { name: "", book: { entries: {}, removed: {} } };
  if (state.blob.length > 0) {
    const parsed = JSON.parse(decode.decode(open(teamKey, teamAad(TEAM_AAD.book, id, state.epoch), bytes(state.blob)))) as TeamBook;
    // Keep every field, also ones this page does not know, or writing back would drop them.
    book = { ...parsed, name: parsed.name ?? "", book: { ...parsed.book, entries: parsed.book?.entries ?? {}, removed: parsed.book?.removed ?? {} } };
  }
  const access: Record<string, OpenHost> = {};
  for (const [member, blob] of state.access) {
    // A list that does not open is ignored rather than failing the team.
    try {
      const plain = JSON.parse(decode.decode(open(teamKey, teamAccessAad(id, state.epoch, member), bytes(blob)))) as {
        hosts?: Record<string, [string, number]>;
      };
      for (const [host, [name]] of Object.entries(plain.hosts ?? {})) access[host] = { member, name };
    } catch {
      continue;
    }
  }
  const role = state.members.find((m) => m.id === state.me)?.role ?? "Technician";
  const unverified = state.members.filter((m) => !teamTagOk(teamKey, m.id, pairOf(m.key))).map((m) => m.id);
  return {
    id: toHex(id),
    epoch: state.epoch,
    revision: state.revision,
    me: state.me,
    role,
    book,
    members: state.members,
    unverified,
    stale: state.stale,
    access,
  };
}

async function get(): Promise<TeamState | null> {
  return variant(await request("GetTeam"), "Team");
}

async function must(): Promise<TeamState> {
  const state = await get();
  if (!state) throw new ApiError(NO_TEAM, 400);
  return state;
}

/** The team key from the server's state: sealed for this account, or, right
 * after joining, the one this tab still holds for this epoch. */
function teamKeyOf(state: TeamState): Uint8Array {
  const ctx = teamContext(idBytes(state), state.epoch);
  if (state.sealed) {
    const plain = openWith(exchangeSecret(key()), ctx, bytes(state.sealed));
    if (plain.length !== 32) throw new Error("Teamschlüssel ist beschädigt");
    return plain;
  }
  if (cached && cached.id === toHex(idBytes(state)) && cached.epoch === state.epoch) return cached.key;
  throw new Error("Der Beitritt zum Team ist nicht abgeschlossen, bitte neu einladen lassen");
}

function remember(state: TeamState, teamKey: Uint8Array) {
  cached = { id: toHex(idBytes(state)), epoch: state.epoch, key: teamKey };
}

/** Creates a team named `name` with this account as its owner. */
export async function createTeam(name: string): Promise<TeamView> {
  // The server picks the team's ID, which the blobs are bound to; so the
  // first book goes in empty and the name follows right after.
  const teamKey = random(32);
  const publicKey = exchangePublic(key());
  const placeholder: MemberKeyJson = { public_key: list(publicKey), tag: new Array(32).fill(0), sealed: [] };
  const state: TeamState | null = variant(await request({ CreateTeam: { key: placeholder, blob: [] } }), "Team");
  if (!state) throw new Error("unerwartete Serverantwort");
  const ctx = teamContext(idBytes(state), state.epoch);
  await request({ ConfirmMember: { epoch: state.epoch, key: memberKey(teamKey, ctx, state.me, publicKey) } });
  remember(state, teamKey);
  const book: TeamBook = { name: name.trim(), book: { entries: {}, removed: {} } };
  const blob = seal(teamKey, teamAad(TEAM_AAD.book, idBytes(state), state.epoch), encode.encode(JSON.stringify(book)));
  await request({ PutTeamBook: { epoch: state.epoch, base: state.revision, blob: list(blob) } });
  return view(await must(), teamKey);
}

/** The account's team, decrypted; finishes a join whose confirmation got lost
 * and, as an admin, starts the new epoch someone's leaving asks for. */
export async function syncTeam(): Promise<TeamView | null> {
  const state = await get();
  if (!state) return null;
  const teamKey = teamKeyOf(state);
  if (!state.sealed) {
    const ctx = teamContext(idBytes(state), state.epoch);
    await request({ ConfirmMember: { epoch: state.epoch, key: memberKey(teamKey, ctx, state.me, exchangePublic(key())) } });
  }
  const team = view(state, teamKey);
  if (team.stale && manages(team.role)) return rotateTeam([]);
  return team;
}

/** Opens an invitation for someone with `role`; returns the code, `ABCD-EFGH-JKMN`.
 * Valid for ten minutes and once. */
export async function inviteMember(role: Exclude<Role, "Owner">): Promise<string> {
  const state = await must();
  const teamKey = teamKeyOf(state);
  for (let attempt = 0; attempt < 5; attempt++) {
    const code = newPairingCode();
    const salt = random(16);
    const { sealKey, proof } = await pairingKeys(code.secret, salt);
    const sealed = seal(sealKey, teamAad(TEAM_AAD.invite, idBytes(state), state.epoch), teamKey);
    try {
      await request({ OfferInvite: { code_id: code.codeId, salt: list(salt), verifier: list(proof), sealed: list(sealed), role } });
      return code.display;
    } catch (err) {
      // That code name is in use right now: pick another.
      if (!isConflict(err)) throw err;
    }
  }
  throw new ApiError("Kein freier Code gefunden, bitte nochmal versuchen", 409);
}

/** Joins the team whose admin gave out `code`. */
export async function joinTeam(code: string): Promise<TeamView> {
  const clean = normalizeCode(code);
  if (!clean) throw new ApiError("Der Code hat 12 Zeichen, z. B. ABCD-EFGH-JKMN", 400);
  const codeId = clean.slice(0, 4);
  const salt = bytes(variant(await request({ InviteSalt: { code_id: codeId } }), "Salt"));
  const { sealKey, proof } = await pairingKeys(clean.slice(4), salt);
  const publicKey = exchangePublic(key());
  const joined = variant(await request({ JoinTeam: { code_id: codeId, proof: list(proof), public_key: list(publicKey) } }), "JoinedTeam");
  const state: TeamState = joined.team;
  const teamKey = open(sealKey, teamAad(TEAM_AAD.invite, idBytes(state), state.epoch), bytes(joined.sealed));
  if (teamKey.length !== 32) throw new Error("Teamschlüssel ist beschädigt");
  remember(state, teamKey);
  const ctx = teamContext(idBytes(state), state.epoch);
  await request({ ConfirmMember: { epoch: state.epoch, key: memberKey(teamKey, ctx, state.me, publicKey) } });
  return view(await must(), teamKey);
}

/** Changes the shared address book with `change` (owner, admins); retries on
 * top of what others wrote meanwhile. */
export async function updateTeamBook(change: (book: TeamBook) => void): Promise<TeamView> {
  for (let attempt = 0; attempt < 4; attempt++) {
    const state = await must();
    const teamKey = teamKeyOf(state);
    const team = view(state, teamKey);
    change(team.book);
    const blob = seal(teamKey, teamAad(TEAM_AAD.book, idBytes(state), state.epoch), encode.encode(JSON.stringify(team.book)));
    try {
      const { revision } = variant(await request({ PutTeamBook: { epoch: state.epoch, base: state.revision, blob: list(blob) } }), "Stored");
      return { ...team, revision };
    } catch (err) {
      if (!isConflict(err) && !isStale(err)) throw err;
    }
  }
  throw new ApiError("Das Team-Adressbuch ändert sich gerade ständig, bitte später nochmal versuchen", 409);
}

export function setRole(member: number, role: Role): Promise<unknown> {
  return request({ SetRole: { member, role } });
}

/** Removes `remove` (may be empty) and moves the team to a new epoch, also
 * dropping members the old key does not vouch for. */
export async function rotateTeam(remove: number[]): Promise<TeamView> {
  for (let attempt = 0; attempt < 4; attempt++) {
    const state = await must();
    const oldKey = teamKeyOf(state);
    const id = idBytes(state);
    const old = view(state, oldKey);
    const gone = [...new Set([...remove, ...old.unverified.filter((m) => m !== state.me)])].sort((a, b) => a - b);

    const newKey = random(32);
    const next = state.epoch + 1;
    const ctx = teamContext(id, next);
    const members: [number, MemberKeyJson][] = [];
    for (const m of old.members.filter((m) => !gone.includes(m.id))) {
      if (!m.key) throw new Error("Mitglied ohne Schlüssel");
      members.push([m.id, memberKey(newKey, ctx, m.id, bytes(m.key[0]))]);
    }
    const blob = seal(newKey, teamAad(TEAM_AAD.book, id, next), encode.encode(JSON.stringify(old.book)));
    const access: [number, Bytes][] = [];
    for (const [member, sealed] of state.access) {
      if (gone.includes(member)) continue;
      try {
        const plain = open(oldKey, teamAccessAad(id, state.epoch, member), bytes(sealed));
        access.push([member, list(seal(newKey, teamAccessAad(id, next, member), plain))]);
      } catch {
        continue;
      }
    }
    const rotation = { epoch: next, base: state.revision, blob: list(blob), members, access };
    try {
      const fresh: TeamState | null = variant(await request({ Rotate: { remove: gone, rotation } }), "Team");
      if (!fresh) throw new Error("unerwartete Serverantwort");
      remember(fresh, newKey);
      return view(fresh, newKey);
    } catch (err) {
      if (!isConflict(err) && !isStale(err)) throw err;
    }
  }
  throw new ApiError("Das Team ändert sich gerade ständig, bitte später nochmal versuchen", 409);
}

export async function leaveTeam(): Promise<void> {
  try {
    await request("LeaveTeam");
  } catch (err) {
    if (!isNoTeam(err)) throw err;
  }
  cached = null;
}

export async function deleteTeam(): Promise<void> {
  await request("DeleteTeam");
  cached = null;
}

/** A device for the team's book, as the app stores it. */
export function newEntry(name: string) {
  return { alias: null, alias_at: 0, name, last_seen: 0 };
}

