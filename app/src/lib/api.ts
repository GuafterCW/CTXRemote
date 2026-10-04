import { Channel, invoke } from "@tauri-apps/api/core";

export type Presence =
  | { state: "connecting" }
  | { state: "online"; id: string }
  | { state: "offline"; reason: string };

export type HostEvent =
  | { kind: "sessionStarted"; session: number; peer: string }
  | { kind: "sessionEnded"; session: number }
  | { kind: "passwordChanged" };

/** Quick build: a viewer waits for the user's decision. */
export interface ApprovalRequest {
  id: number;
  peer: string;
}

export interface Peer {
  id: string;
  alias: string | null;
  /** Hostname from the last connection, "" if never connected. */
  name: string;
  /** Unix seconds, 0 if never connected. */
  lastSeen: number;
}

export function peerLabel(peer: Peer): string {
  return peer.alias ?? (peer.name || "Unbenanntes Gerät");
}

export interface Hosted {
  session: number;
  peer: string;
}

export interface Overview {
  presence: Presence;
  password: string;
  server: string;
  unattended: boolean;
  /** The installed Windows service hosts this device. */
  service: boolean;
  hostSupported: boolean;
  peers: Peer[];
  hosted: Hosted[];
  version: string;
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

export const api = {
  overview: () => invoke<Overview>("overview"),
  refreshPassword: () => invoke<string>("refresh_password"),
  saveSettings: (server: string, permanentPassword: string | null) =>
    invoke<void>("save_settings", { server, permanentPassword }),
  forgetPeer: (id: string) => invoke<void>("forget_peer", { id }),
  setAlias: (id: string, alias: string | null) => invoke<void>("set_alias", { id, alias }),
  /** `target` is an ID or an alias. */
  connect: (target: string, password: string) => invoke<number>("connect", { target, password }),
  attach: (session: number, channel: Channel<ArrayBuffer>) =>
    invoke<{ host: HostInfo; id: string; label: string }>("attach", { session, channel }),
  sendInput: (session: number, event: InputEvent) => invoke<void>("send_input", { session, event }),
  selectDisplay: (session: number, index: number) => invoke<void>("select_display", { session, index }),
  requestKeyframe: (session: number) => invoke<void>("request_keyframe", { session }),
  sendSas: (session: number) => invoke<void>("send_sas", { session }),
  lockScreen: (session: number) => invoke<void>("lock_screen", { session }),
  restartHost: (session: number) => invoke<void>("restart_host", { session }),
  setQuality: (session: number, quality: Quality) => invoke<void>("set_quality", { session, quality }),
  disconnect: (session: number) => invoke<void>("disconnect", { session }),
  endHostedSession: (session: number) => invoke<void>("end_hosted_session", { session }),
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
