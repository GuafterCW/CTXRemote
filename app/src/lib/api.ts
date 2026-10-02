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

export type MouseButton = "Left" | "Right" | "Middle" | "Back" | "Forward";

/** Mirrors `InputEvent` in crates/proto/src/session.rs (serde external tagging). */
export type InputEvent =
  | { MouseMove: { x: number; y: number } }
  | { MouseButton: { button: MouseButton; down: boolean } }
  | { Wheel: { dx: number; dy: number } }
  | { Key: { code: string; down: boolean } }
  | "ReleaseAll";

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
  disconnect: (session: number) => invoke<void>("disconnect", { session }),
  endHostedSession: (session: number) => invoke<void>("end_hosted_session", { session }),
  /** Quick build only. */
  answerApproval: (id: number, allow: boolean) => invoke<void>("answer_approval", { id, allow }),
};

export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Unbekannter Fehler";
}
