import { Channel, invoke } from "@tauri-apps/api/core";

export type Presence =
  | { state: "connecting" }
  | { state: "online"; id: string }
  | { state: "offline"; reason: string };

/** Mirrors `Profile` in crates/core/src/profile.rs: how a helper presents themselves. */
export interface Profile {
  name: string;
  company: string;
  message: string;
  /** PNG as base64, "" for none. */
  logo: string;
}

/** "Name (Firma)", or whichever is set. */
export function profileLabel(profile: Profile): string {
  if (profile.name && profile.company) return `${profile.name} (${profile.company})`;
  return profile.name || profile.company;
}

export type HostEvent =
  | { kind: "sessionStarted"; session: number; peer: string; chat: boolean; profile?: Profile | null; rights?: number }
  | { kind: "rights"; session: number; rights: number; privacy: boolean }
  | { kind: "sessionEnded"; session: number }
  | { kind: "chat"; session: number; text: string }
  | { kind: "passwordChanged" };

/** Quick build: a viewer waits for the user's decision. */
export interface ApprovalRequest {
  id: number;
  peer: string;
  /** Self-declared by the viewer; older viewers send none. */
  profile: Profile | null;
}

export interface Peer {
  id: string;
  alias: string | null;
  /** Hostname from the last connection, "" if never connected. */
  name: string;
  /** Unix seconds, 0 if never connected. */
  lastSeen: number;
  /** This device lets devices of my account connect without a password. */
  access: boolean;
  /** Its network cards are known, so it can be woken over the network. */
  wake: boolean;
}

export function peerLabel(peer: Peer): string {
  return peer.alias ?? (peer.name || "Unbenanntes Gerät");
}

export interface Hosted {
  session: number;
  peer: string;
  /** The viewer's version understands chat messages. */
  chat: boolean;
  /** How the viewer presents itself (self-declared). */
  profile: Profile | null;
  /** `RIGHT` bits the viewer has; changeable while the session runs. */
  rights: number;
  /** This computer's screen is blank for the viewer (privacy mode). */
  privacy: boolean;
}

/** Who controls a hosted session, as shown on this device. */
export function hostedLabel(hosted: Hosted): string {
  return hosted.profile ? profileLabel(hosted.profile) : hosted.peer;
}

/** Mirrors `DirectSettings` in crates/core/src/config.rs. */
export interface DirectSettings {
  enabled: boolean;
  port: number;
  addresses: string[];
}

export interface Overview {
  presence: Presence;
  password: string;
  server: string;
  unattended: boolean;
  direct: DirectSettings;
  /** The listener for direct connections is running. */
  directActive: boolean;
  /** The installed Windows service hosts this device. */
  service: boolean;
  hostSupported: boolean;
  peers: Peer[];
  hosted: Hosted[];
  version: string;
  /** A newer version the app can install (app mode; the service updates itself). */
  update: string | null;
  /** Public alias others can connect with instead of the ID. */
  publicAlias: string | null;
  /** False in the portable helper, which has no lasting identity. */
  aliasSupported: boolean;
  /** How this user presents themselves when connecting to others. */
  profile: Profile | null;
  /** Set while the address book syncs with an account (not in the portable helper). */
  account?: AccountView | null;
  /** Devices of the account may connect to this device without a password. */
  accountAccess: boolean;
  /** `RIGHT` bits for new sessions with the one-time password. */
  rightsAttended: number;
  /** The same for the permanent password and the account's devices. */
  rightsUnattended: number;
  /** The permanent password also needs the code from an authenticator app. */
  codeEnabled: boolean;
}

export interface AccountView {
  devices: number;
  /** Why the last sync failed, if it did. */
  error: string | null;
}

/** Mirrors `AccountDevice` in crates/core/src/account.rs. */
export interface AccountDevice {
  publicKey: string;
  id: string | null;
  online: boolean;
  name: string;
  this: boolean;
}

export interface AccountDetails {
  email: string | null;
  verified: boolean;
  devices: AccountDevice[];
}

export interface DisplayInfo {
  index: number;
  name: string;
  width: number;
  height: number;
  primary: boolean;
}

export interface HostInfo {
  hostname: string;
  username: string;
  os: string;
  displays: DisplayInfo[];
  active_display: number;
}

/** What the remote device supports; older versions report nothing. */
export interface HostFeatures {
  files: boolean;
  restart: boolean;
  quality: boolean;
  chat: boolean;
  /** Sends its sound on request. */
  audio: boolean;
  /** Can blank its screen (privacy mode). */
  privacy: boolean;
  /** Takes and offers files through the clipboard. */
  filePaste: boolean;
  /** Tells about its computer (system info). */
  sysinfo: boolean;
}

/** Mirrors `SystemInfo` in crates/proto/src/session.rs. */
export interface SystemInfo {
  hostname: string;
  user: string;
  os: string;
  os_build: string;
  model: string;
  cpu: string;
  cores: number;
  memory_total: number;
  memory_used: number;
  uptime_secs: number;
  disks: { mount: string; label: string; total: number; free: number }[];
  networks: { name: string; mac: string; addresses: string[] }[];
  app_version: string;
}

/** The connect error of a host that wants its authenticator code (`CodeNeeded` in core). */
export const CODE_NEEDED = "Bitte den Bestätigungscode aus der Authenticator-App eingeben";

/** Mirrors `Permissions` in crates/proto/src/session.rs. */
export const RIGHT = {
  INPUT: 1 << 0,
  FILES: 1 << 1,
  CLIPBOARD: 1 << 2,
  AUDIO: 1 << 3,
  RESTART: 1 << 4,
  PRIVACY: 1 << 5,
} as const;

/** Mirrors `Quality` in crates/proto/src/session.rs. */
export type Quality = "Speed" | "Balanced" | "Sharp";

export type MouseButton = "Left" | "Right" | "Middle" | "Back" | "Forward";

/** Mirrors `InputEvent` in crates/proto/src/session.rs (serde external tagging). */
export type InputEvent =
  | { MouseMove: { x: number; y: number } }
  | { MouseButton: { button: MouseButton; down: boolean } }
  | { Wheel: { dx: number; dy: number } }
  | { Key: { code: string; down: boolean } }
  | "ReleaseAll";

export type FileKind = "File" | "Dir" | "Drive" | "Place";

export interface FileEntry {
  name: string;
  path: string;
  kind: FileKind;
  size: number;
  /** Unix seconds, 0 if unknown. */
  modified: number;
}

export interface Listing {
  path: string;
  /** null at the top level; "" if the top level lies above. */
  parent: string | null;
  entries: FileEntry[];
}

export type TransferUpdate =
  | { session: number; kind: "progress"; id: number; done: number; total: number }
  | { session: number; kind: "finished"; id: number; path: string | null }
  | { session: number; kind: "failed"; id: number; message: string };

export type Outcome =
  | "oneTimePassword"
  | "permanentPassword"
  | "account"
  | "wrongPassword"
  | "notMember"
  | "declined";

export interface Visit {
  /** Unix seconds. */
  started: number;
  /** Unix seconds; null = still running or the machine went off. */
  ended: number | null;
  /** E.g. "anna (LAPTOP)"; empty if it failed before the introduction. */
  peer: string;
  profile: string | null;
  outcome: Outcome;
}

export const api = {
  history: () => invoke<Visit[]>("history"),
  overview: () => invoke<Overview>("overview"),
  refreshPassword: () => invoke<string>("refresh_password"),
  saveSettings: (server: string, permanentPassword: string | null) =>
    invoke<void>("save_settings", { server, permanentPassword }),
  saveDirect: (settings: DirectSettings) => invoke<void>("save_direct", { settings }),
  accountCreate: () => invoke<void>("account_create"),
  /** A one-time code for another device, e.g. "ABCD-EFGH-JKMN"; valid for ten minutes. */
  accountPairingCode: () => invoke<string>("account_pairing_code"),
  accountJoin: (code: string) => invoke<void>("account_join", { code }),
  accountLeave: () => invoke<void>("account_leave"),
  accountSync: () => invoke<void>("account_sync"),
  /** Returns the recovery code, to be shown once. */
  accountRegister: (email: string, password: string) => invoke<string>("account_register", { email, password }),
  accountLogin: (email: string, password: string) => invoke<void>("account_login", { email, password }),
  /** Returns the new recovery code. */
  accountRecover: (email: string, code: string, password: string) =>
    invoke<string>("account_recover", { email, code, password }),
  /** Returns the new recovery code. */
  accountSetLogin: (email: string, password: string) => invoke<string>("account_set_login", { email, password }),
  /** With the service this triggers a Windows UAC prompt. */
  accountSetAccess: (enabled: boolean) => invoke<void>("account_set_access", { enabled }),
  accountDetails: () => invoke<AccountDetails>("account_details"),
  accountRemoveDevice: (publicKey: string) => invoke<void>("account_remove_device", { publicKey }),
  /** An empty profile removes it; returns the profile as stored. */
  saveProfile: (profile: Profile) => invoke<Profile | null>("save_profile", { profile }),
  forgetPeer: (id: string) => invoke<void>("forget_peer", { id }),
  /** Sends the Wake-on-LAN packet into the local networks. */
  wakePeer: (id: string) => invoke<void>("wake_peer", { id }),
  setAlias: (id: string, alias: string | null) => invoke<void>("set_alias", { id, alias }),
  /** `target` is an ID or an alias. */
  /** `code`: from the host's authenticator app, once it asked (see `CODE_NEEDED`). */
  connect: (target: string, password: string, code?: string) =>
    invoke<number>("connect", { target, password, code: code ?? null }),
  attach: (session: number, channel: Channel<ArrayBuffer>) =>
    invoke<{
      host: HostInfo;
      id: string;
      label: string;
      features: HostFeatures;
      direct: string | null;
      /** `RIGHT` bits; null from hosts that do not say (all allowed). */
      rights: number | null;
      privacy: boolean;
    }>(
      "attach",
      { session, channel },
    ),
  sendInput: (session: number, event: InputEvent) => invoke<void>("send_input", { session, event }),
  selectDisplay: (session: number, index: number) => invoke<void>("select_display", { session, index }),
  requestKeyframe: (session: number) => invoke<void>("request_keyframe", { session }),
  sendSas: (session: number) => invoke<void>("send_sas", { session }),
  lockScreen: (session: number) => invoke<void>("lock_screen", { session }),
  restartHost: (session: number) => invoke<void>("restart_host", { session }),
  setQuality: (session: number, quality: Quality) => invoke<void>("set_quality", { session, quality }),
  disconnect: (session: number) => invoke<void>("disconnect", { session }),
  /** Turns the host's sound on or off; false if the host has none. */
  setAudio: (session: number, on: boolean) => invoke<boolean>("set_audio", { session, on }),
  /** Blanks the host's screen and blocks its local input, or ends that. */
  setPrivacy: (session: number, on: boolean) => invoke<boolean>("set_privacy", { session, on }),
  /** Asks the host about its computer; the answer comes as the `system-info` event. */
  requestSystemInfo: (session: number) => invoke<boolean>("request_system_info", { session }),
  /** Ctrl+V: files on this computer's clipboard go onto the host's; false if there are none. */
  pasteFiles: (session: number) => invoke<boolean>("paste_files", { session }),
  /** Fetches the files last copied at the host onto this computer's clipboard. */
  fetchHostFiles: (session: number) => invoke<number>("fetch_host_files", { session }),
  /** Changes what the viewer of a session at this computer may do. */
  /** A new authenticator secret with its QR code (SVG); stored only by `codeEnable`. */
  codeSetup: () => invoke<{ secret: string; uri: string; qr: string }>("code_setup"),
  /** Turns the code on (with a matching code) or off (`secret` null). */
  codeEnable: (secret: string | null, code: string | null) => invoke<void>("code_enable", { secret, code }),
  saveRights: (attended: number, unattended: number) => invoke<void>("save_rights", { attended, unattended }),
  setHostedRights: (session: number, rights: number) => invoke<void>("set_hosted_rights", { session, rights }),
  endHostedSession: (session: number) => invoke<void>("end_hosted_session", { session }),
  installUpdate: () => invoke<void>("install_update"),
  /** `null` drops the alias; returns it as stored (lowercase). */
  setPublicAlias: (alias: string | null) => invoke<string | null>("set_public_alias", { alias }),
  /** Viewer side: rejects with the reason, e.g. an empty message. */
  sendChat: (session: number, text: string) => invoke<void>("send_chat", { session, text }),
  /** Host side. */
  hostChat: (session: number, text: string) => invoke<void>("host_chat", { session, text }),
  openFiles: (session: number) => invoke<void>("open_files", { session }),
  /** Files dropped on the session window, uploaded by its file window. */
  queueDrop: (session: number, paths: string[]) => invoke<void>("queue_drop", { session, paths }),
  takeDrops: (session: number) => invoke<string[]>("take_drops", { session }),
  remoteList: (session: number, path: string) => invoke<Listing>("remote_list", { session, path }),
  remoteCreateDir: (session: number, path: string) => invoke<void>("remote_create_dir", { session, path }),
  remoteRename: (session: number, path: string, name: string) =>
    invoke<void>("remote_rename", { session, path, name }),
  remoteDelete: (session: number, paths: string[]) => invoke<void>("remote_delete", { session, paths }),
  localList: (path: string) => invoke<Listing>("local_list", { path }),
  localCreateDir: (path: string) => invoke<void>("local_create_dir", { path }),
  localRename: (path: string, name: string) => invoke<void>("local_rename", { path, name }),
  localDelete: (paths: string[]) => invoke<void>("local_delete", { paths }),
  upload: (session: number, path: string, dir: string) => invoke<number>("upload", { session, path, dir }),
  download: (session: number, path: string, dir: string) => invoke<number>("download", { session, path, dir }),
  cancelTransfer: (session: number, id: number) => invoke<void>("cancel_transfer", { session, id }),
  localHome: () => invoke<string>("local_home"),
  reveal: (path: string) => invoke<void>("reveal", { path }),
  /** Quick build only. */
  answerApproval: (id: number, allow: boolean) => invoke<void>("answer_approval", { id, allow }),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Unbekannter Fehler";
}
