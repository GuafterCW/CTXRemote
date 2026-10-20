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
  reportAad,
  policyAad,
  reportCheck,
  reportSecrets,
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
  fromHex,
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
  /** Keys of earlier epochs (hex), so old reports stay readable. */
  past?: Record<string, string>;
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

/** Owners and admins: hands the server the check for this epoch's report token.
 * Failures are only logged; reports are not worth blocking the team page. */
async function announceReportCheck(state: TeamState, teamKey: Uint8Array): Promise<void> {
  try {
    const { token } = reportSecrets(teamKey);
    await request({ SetReportCheck: { epoch: state.epoch, check: list(reportCheck(token)) } });
  } catch (err) {
    console.warn("Berichtsprüfwert nicht gesetzt", err);
  }
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
  if (manages(team.role)) {
    await announceReportCheck(state, teamKey);
    await carryOverPolicy(state, teamKey, team);
  }
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
    // Keep the old key under its epoch, as the app does, for old reports.
    const nextBook: TeamBook = { ...old.book, past: { ...old.book.past, [String(state.epoch)]: toHex(oldKey) } };
    const blob = seal(newKey, teamAad(TEAM_AAD.book, id, next), encode.encode(JSON.stringify(nextBook)));
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
      await announceReportCheck(fresh, newKey);
      const rotated = view(fresh, newKey);
      await carryOverPolicy(fresh, newKey, rotated);
      return rotated;
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

/** One session report as the members' devices wrote it (`Report` in Rust). */
export type Outcome =
  | "oneTimePassword"
  | "permanentPassword"
  | "account"
  | "wrongPassword"
  | "notMember"
  | "declined"
  | "wrongCode"
  | "team"
  | "policy";

export interface Report {
  side: "viewer" | "host";
  device: number | null;
  deviceName: string;
  peer: number | null;
  peerName: string;
  profile: string | null;
  /** Unix seconds. */
  started: number;
  ended: number;
  outcome: Outcome | null;
  /** Ready-made German texts of what happened in the session (at most 50); absent in older reports. */
  actions?: string[];
}

/** A stored report; `report` is null if its epoch's key is missing or it is damaged. */
export interface ReportRow {
  seq: number;
  /** When the server got it (Unix seconds). */
  at: number;
  member: number | null;
  memberEmail: string | null;
  report: Report | null;
}

/** All reports of the team (owners, admins), oldest first, decrypted. */
export async function loadReports(): Promise<ReportRow[]> {
  const state = await must();
  const teamKey = teamKeyOf(state);
  const team = view(state, teamKey);
  const id = idBytes(state);
  const keyOf = (epoch: number): Uint8Array | null => {
    if (epoch === state.epoch) return teamKey;
    const hex = team.book.past?.[String(epoch)];
    return hex && /^[0-9a-f]{64}$/i.test(hex) ? fromHex(hex) : null;
  };
  const rows: ReportRow[] = [];
  let after = 0;
  for (;;) {
    const { entries, more } = variant(await request({ GetReports: { after } }), "Reports") as {
      entries: { seq: number; at: number; epoch: number; member: number | null; blob: Bytes }[];
      more: boolean;
    };
    for (const e of entries) {
      let report: Report | null = null;
      const k = keyOf(e.epoch);
      if (k) {
        try {
          report = JSON.parse(decode.decode(open(reportSecrets(k).key, reportAad(id, e.epoch), bytes(e.blob)))) as Report;
        } catch {
          report = null;
        }
      }
      const memberEmail = e.member === null ? null : (state.members.find((m) => m.id === e.member)?.email ?? null);
      rows.push({ seq: e.seq, at: e.at, member: e.member, memberEmail, report });
    }
    if (!more || entries.length === 0) return rows;
    after = entries[entries.length - 1].seq;
  }
}

/** The team policy (`TeamPolicy` in Rust). The deny masks hold the rights taken away. */
export interface TeamPolicy {
  denyTechnician: number;
  denyAdmin: number;
  denyOthers: number;
  oneTime: boolean;
  permanent: boolean;
  account: boolean;
  requireCode: boolean;
  /** Unix seconds. */
  changed: number;
}

export function defaultPolicy(): TeamPolicy {
  return { denyTechnician: 0, denyAdmin: 0, denyOthers: 0, oneTime: true, permanent: true, account: true, requireCode: false, changed: 0 };
}

/** The policy's fields in the order Rust writes them, with defaults for missing ones. */
function normalizePolicy(raw: Partial<TeamPolicy>): TeamPolicy {
  const d = defaultPolicy();
  const mask = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
  const flag = (v: unknown, fallback: boolean) => (typeof v === "boolean" ? v : fallback);
  return {
    denyTechnician: mask(raw.denyTechnician),
    denyAdmin: mask(raw.denyAdmin),
    denyOthers: mask(raw.denyOthers),
    oneTime: flag(raw.oneTime, d.oneTime),
    permanent: flag(raw.permanent, d.permanent),
    account: flag(raw.account, d.account),
    requireCode: flag(raw.requireCode, d.requireCode),
    changed: mask(raw.changed),
  };
}

/** Fetches the stored policy blob for the current epoch's token. */
async function fetchPolicy(state: TeamState, teamKey: Uint8Array): Promise<{ epoch: number; blob: Bytes }> {
  const { token } = reportSecrets(teamKey);
  return variant(await request({ GetPolicy: { team: list(idBytes(state)), epoch: state.epoch, token: list(token) } }), "Policy");
}

/** The team's policy, decrypted; null if there is none. */
export async function loadPolicy(): Promise<TeamPolicy | null> {
  const state = await must();
  const teamKey = teamKeyOf(state);
  const team = view(state, teamKey);
  const stored = await fetchPolicy(state, teamKey);
  if (stored.blob.length === 0) return null;
  let k: Uint8Array | null = teamKey;
  if (stored.epoch !== state.epoch) {
    const hex = team.book.past?.[String(stored.epoch)];
    k = hex && /^[0-9a-f]{64}$/i.test(hex) ? fromHex(hex) : null;
  }
  if (!k) throw new Error("Die Richtlinie ist mit einem früheren Teamschlüssel verschlüsselt und nicht lesbar");
  const plain = open(reportSecrets(k).key, policyAad(idBytes(state), stored.epoch), bytes(stored.blob));
  return normalizePolicy(JSON.parse(decode.decode(plain)) as Partial<TeamPolicy>);
}

/** Stores `policy` (owners, admins); null removes it. */
export async function savePolicy(policy: TeamPolicy | null): Promise<void> {
  const state = await must();
  const teamKey = teamKeyOf(state);
  let blob: Uint8Array = new Uint8Array(0);
  if (policy) {
    const json = JSON.stringify(normalizePolicy(policy));
    blob = seal(reportSecrets(teamKey).key, policyAad(idBytes(state), state.epoch), encode.encode(json));
  }
  await request({ SetPolicy: { epoch: state.epoch, blob: list(blob) } });
}

/** Owners and admins: after a new epoch, seals a policy of an earlier epoch with
 * the current key. Failures are only logged. */
async function carryOverPolicy(state: TeamState, teamKey: Uint8Array, team: TeamView): Promise<void> {
  try {
    const stored = await fetchPolicy(state, teamKey);
    if (stored.epoch === state.epoch || stored.blob.length === 0) return;
    const hex = team.book.past?.[String(stored.epoch)];
    if (!hex || !/^[0-9a-f]{64}$/i.test(hex)) throw new Error("Schlüssel der früheren Epoche fehlt");
    const plain = open(reportSecrets(fromHex(hex)).key, policyAad(idBytes(state), stored.epoch), bytes(stored.blob));
    const blob = seal(reportSecrets(teamKey).key, policyAad(idBytes(state), state.epoch), plain);
    await request({ SetPolicy: { epoch: state.epoch, blob: list(blob) } });
  } catch (err) {
    console.warn("Richtlinie nicht übernommen", err);
  }
}

/** A device for the team's book, as the app stores it. */
export function newEntry(name: string) {
  return { alias: null, alias_at: 0, name, last_seen: 0 };
}

