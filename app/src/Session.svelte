<script lang="ts">
  import { onMount, tick } from "svelte";
  import { Channel } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, CODE_NEEDED, errorText, RIGHT, type SystemInfo, type HostInfo, type InputEvent, type HostFeatures, type MouseButton, type Quality } from "./lib/api";
  import ChatPanel, { type ChatMessage } from "./lib/ChatPanel.svelte";
  import Icon from "./lib/Icon.svelte";
  import { Player } from "./lib/player";
  import { SoundPlayer } from "./lib/sound";
  import { MicSender } from "./lib/mic";
  import { readIdleMinutes } from "./lib/idle";
  import { MOBILE, goHome } from "./lib/platform";
  import { TouchControl } from "./lib/touch";
  import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";

  let { session }: { session: number } = $props();

  let canvas = $state<HTMLCanvasElement>();
  let host = $state<HostInfo | null>(null);
  let hostId = $state("");
  let label = $state("");
  let display = $state(0);
  let streaming = $state(false);
  let closed = $state<string | null>(null);
  let toolbarVisible = $state(true);
  let keysOpen = $state(false);
  let qualityOpen = $state(false);
  let quality = $state<Quality>("Balanced");
  /** "fit" never enlarges a smaller remote screen; "fill" scales it to the window. */
  let scaleMode = $state<"fit" | "fill">(loadScaleMode());
  let video = $state({ width: 0, height: 0 });
  let confirmRestart = $state(false);
  /** What the host supports; older hosts get no buttons for newer features. */
  let features = $state<HostFeatures>({ files: false, restart: false, quality: false, chat: false, audio: false, privacy: false, filePaste: false, sysinfo: false, recording: false, tunnel: false, draw: false, mic: false, typeText: false });
  /** What the host allows; null from older hosts, which allow everything. */
  let rights = $state<number | null>(null);
  const can = (right: number) => rights === null || (rights & right) !== 0;
  /** The host's screen is blank for the person there (privacy mode). */
  let privacy = $state(false);
  /** A short message under the toolbar, e.g. why privacy mode failed. */
  let notice = $state("");
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  /** Ctrl+V with files: the V waits until they are on the host's clipboard. */
  let pasting = false;
  /** Names of files copied at the host, offered to fetch. */
  let hostFiles = $state<string[]>([]);
  /** Unix ms when the recording started; null while not recording. */
  let recordingSince = $state<number | null>(null);
  let now = $state(Date.now());
  let clock: ReturnType<typeof setInterval> | undefined;
  const recordingFor = $derived.by(() => {
    if (recordingSince === null) return "";
    const secs = Math.max(0, Math.floor((now - recordingSince) / 1000));
    return `${Math.floor(secs / 60)}:${String(secs % 60).padStart(2, "0")}`;
  });

  /** Connecting again after the session ended: same password first. */
  let reconnecting = $state(false);
  let reError = $state("");
  let rePassword = $state("");
  let reCode = $state("");
  let askPassword = $state(false);
  let askCode = $state(false);

  async function reconnect(e?: SubmitEvent) {
    e?.preventDefault();
    reconnecting = true;
    reError = "";
    try {
      await api.reconnect(session, askPassword ? rePassword : undefined, askCode ? reCode : undefined);
      // The new session has its own window (on a phone it already took this one).
      if (!MOBILE) await appWindow.close();
    } catch (err) {
      const text = errorText(err);
      if (text === CODE_NEEDED) {
        askCode = true;
      } else {
        reError = text;
        // A one-time password is used up; a changed one needs typing.
        askPassword = true;
        reCode = "";
      }
    } finally {
      reconnecting = false;
    }
  }

  /** This computer's clipboard text, typed at the host key by key. */
  async function typeClipboard() {
    keysOpen = false;
    try {
      if (MOBILE) {
        // The app's clipboard reader has no phone clipboard: the page reads it.
        const text = await readText();
        if (!text) throw "In der Zwischenablage ist kein Text";
        send({ Text: text });
      } else {
        await api.typeClipboard(session);
      }
    } catch (e) {
      showNotice(errorText(e));
    }
    canvas?.focus();
  }

  // Ends the session after the time set in Settings without input here.
  let lastActivity = Date.now();
  let idleWarned = false;
  const touch = () => {
    lastActivity = Date.now();
    if (idleWarned) {
      idleWarned = false;
      notice = "";
    }
  };

  function checkIdle() {
    const minutes = readIdleMinutes();
    if (!minutes || closed !== null) return;
    const left = minutes * 60_000 - (Date.now() - lastActivity);
    if (left <= 0) {
      api.disconnect(session).catch(() => {});
      closed = `Wegen ${minutes} Minuten Inaktivität getrennt.`;
    } else if (left <= 60_000 && !idleWarned) {
      idleWarned = true;
      showNotice("Keine Eingabe: Die Sitzung endet in einer Minute. Maus bewegen, um sie zu halten.");
    }
  }

  /** The current picture as a PNG in the pictures folder. */
  async function screenshot() {
    if (!canvas) return;
    try {
      const blob = await new Promise<Blob | null>((done) => canvas!.toBlob(done, "image/png"));
      if (!blob) throw new Error("Kein Bild");
      const file = await api.saveScreenshot(session, await blob.arrayBuffer());
      showNotice(`Bildschirmfoto gespeichert: ${file}`);
    } catch (e) {
      showNotice(`Bildschirmfoto fehlgeschlagen: ${errorText(e)}`);
    }
  }

  /** Lock the host's screen when the session ends here; remembered per device. */
  let lockOnEnd = $state(false);

  async function toggleLockOnEnd() {
    const next = !lockOnEnd;
    try {
      await api.setLockOnEnd(session, next);
      lockOnEnd = next;
    } catch (e) {
      showNotice(errorText(e));
    }
  }

  async function toggleRecording() {
    try {
      if (recordingSince === null) {
        await api.startRecording(session);
        recordingSince = Date.now();
        now = recordingSince;
        clock = setInterval(() => (now = Date.now()), 1000);
      } else {
        const files = await api.stopRecording(session);
        recordingSince = null;
        clearInterval(clock);
        showNotice(files.length ? `Aufnahme gespeichert: ${files.join(", ")}` : "Es wurde nichts aufgezeichnet.");
      }
    } catch (e) {
      showNotice(errorText(e));
    }
  }

  /** This computer's microphone, played at the host. */
  let mic: MicSender | null = null;
  let micOn = $state(false);

  async function toggleMic() {
    if (micOn) {
      mic?.stop();
      mic = null;
      micOn = false;
      return;
    }
    const sender = new MicSender();
    try {
      await sender.start((packet) => api.micPacket(session, packet).catch(() => {}));
      mic = sender;
      micOn = true;
    } catch (e) {
      sender.stop();
      showNotice(`Mikrofon nicht verfügbar: ${errorText(e)}`);
    }
  }

  /** Drawing over the host's screen instead of controlling it. */
  let drawing = $state(false);
  let drawColor = $state(0xe5484d);
  const DRAW_COLORS = [0xe5484d, 0xf5b300, 0x2f8bff, 0x30a46c];
  let stroke: [number, number][] = [];
  let strokeTimer: ReturnType<typeof setInterval> | undefined;

  function toggleDrawing() {
    drawing = !drawing;
    if (!drawing) {
      flushStroke();
      api.drawClear(session).catch(() => {});
    }
    canvas?.focus();
  }

  function drawPoint(e: MouseEvent): [number, number] {
    const p = toRemote(e);
    const c = canvas!;
    return [Math.round((p.x / Math.max(c.width - 1, 1)) * 65535), Math.round((p.y / Math.max(c.height - 1, 1)) * 65535)];
  }

  // Sent in pieces while drawing, so the person at the host sees the line grow.
  function flushStroke() {
    if (stroke.length > 0) api.drawStroke(session, drawColor, 4, stroke).catch(() => {});
    // The next piece starts where this one ended.
    stroke = stroke.length > 0 ? [stroke[stroke.length - 1]] : [];
  }

  function drawDown(e: MouseEvent) {
    e.preventDefault();
    stroke = [drawPoint(e)];
    clearInterval(strokeTimer);
    strokeTimer = setInterval(flushStroke, 60);
  }

  function drawMove(e: PointerEvent) {
    if (strokeTimer === undefined) return;
    stroke.push(drawPoint(e));
  }

  function drawUp() {
    if (strokeTimer === undefined) return;
    clearInterval(strokeTimer);
    strokeTimer = undefined;
    flushStroke();
    stroke = [];
  }

  /** Port tunnels: local ports leading into the host's network. */
  let tunnelOpen = $state(false);
  let tunnels = $state<{ port: number; target: string }[]>([]);
  let tunnelTarget = $state("");
  let tunnelPort = $state("");
  let tunnelError = $state("");

  async function toggleTunnels() {
    tunnelOpen = !tunnelOpen;
    infoOpen = false;
    // Keys still held on the remote device would stay pressed while typing in the fields.
    if (tunnelOpen) send("ReleaseAll");
    if (tunnelOpen) tunnels = await api.listTunnels(session).catch(() => []);
  }

  async function openTunnel(e: SubmitEvent) {
    e.preventDefault();
    tunnelError = "";
    // Without a port the button used to stay greyed out with no word why.
    if (!/:\d+$/.test(tunnelTarget.trim())) {
      tunnelError = "Bitte das Ziel mit Port angeben, z. B. 192.168.1.10:3389";
      return;
    }
    const port = tunnelPort.trim() ? Number(tunnelPort) : undefined;
    try {
      await api.openTunnel(session, tunnelTarget, port);
      tunnelTarget = "";
      tunnelPort = "";
      tunnels = await api.listTunnels(session);
    } catch (err) {
      tunnelError = errorText(err);
    }
  }

  async function closeTunnel(port: number) {
    await api.closeTunnel(session, port).catch(() => {});
    tunnels = await api.listTunnels(session).catch(() => []);
  }

  /** The info panel about the host's computer. */
  let infoOpen = $state(false);
  let info = $state<SystemInfo | null>(null);
  let fetching = $state(false);
  /** The host's sound; remembered across sessions. */
  let soundOn = $state(loadSound());
  let sound: SoundPlayer | null = null;
  let soundReady = $state(false);
  let chatOpen = $state(false);
  let chatMessages = $state<ChatMessage[]>([]);
  let unread = $state(0);
  let chatPanel = $state<ReturnType<typeof ChatPanel>>();
  /** Address of the direct connection, null while the server relays. */
  let direct = $state<string | null>(null);
  /** CSS cursor showing the host's pointer shape, once it has sent one. */
  let cursor = $state("default");
  let fullscreen = $state(false);

  const appWindow = getCurrentWindow();
  const SCALES: ["fit" | "fill", string, string][] = [
    ["fit", "Nicht vergrößern", "kleinere Bildschirme in Originalgröße"],
    ["fill", "Fenster füllen", "immer auf Fenstergröße skalieren"],
  ];
  const QUALITIES: [Quality, string, string][] = [
    ["Speed", "Schnell", "für langsame Verbindungen"],
    ["Balanced", "Ausgewogen", "Standard"],
    ["Sharp", "Scharf", "für schnelle Verbindungen"],
  ];
  const BUTTONS: MouseButton[] = ["Left", "Middle", "Right", "Back", "Forward"];
  const send = (event: InputEvent) => {
    // View only: the host would drop it anyway.
    if (closed !== null || !can(RIGHT.INPUT)) return;
    // Only watching by choice: nothing but letting go of held keys.
    if (watchOnly && event !== "ReleaseAll") return;
    api.sendInput(session, event);
  };

  /** The user chose to only watch: no mouse or keyboard reaches the host. */
  let watchOnly = $state(false);

  function toggleWatchOnly() {
    if (!watchOnly) send("ReleaseAll");
    watchOnly = !watchOnly;
    qualityOpen = false;
    showNotice(watchOnly ? "Nur ansehen: Maus und Tastatur gehen nicht an das Gerät." : "Maus und Tastatur wieder an.");
  }

  /** Frames and bytes of the last second, shown on request. */
  let showStats = $state(loadFlag("ctxremote.stats"));
  let stats = $state({ fps: 0, kbps: 0 });
  let frameCount = 0;
  let byteCount = 0;

  function toggleStats() {
    showStats = !showStats;
    try {
      localStorage.setItem("ctxremote.stats", showStats ? "on" : "off");
    } catch {
      // Not remembered, but still applied.
    }
    qualityOpen = false;
  }

  function loadFlag(key: string): boolean {
    try {
      return localStorage.getItem(key) === "on";
    } catch {
      return false;
    }
  }

  function showNotice(text: string) {
    notice = text;
    toolbarVisible = true;
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ""), 6000);
  }

  function applyRights(next: number) {
    const before = rights;
    rights = next;
    const had = (bit: number) => before === null || (before & bit) !== 0;
    if (had(RIGHT.INPUT) && !can(RIGHT.INPUT)) showNotice("Die Gegenseite erlaubt nur noch das Ansehen.");
    else if (!had(RIGHT.INPUT) && can(RIGHT.INPUT)) showNotice("Maus und Tastatur sind jetzt erlaubt.");
    // Taking sound away stops it at the host; giving it back starts it again if wanted.
    if (!had(RIGHT.AUDIO) && can(RIGHT.AUDIO) && soundOn && sound) api.setAudio(session, true).catch(() => {});
    if (!can(RIGHT.AUDIO) && micOn) {
      mic?.stop();
      mic = null;
      micOn = false;
    }
  }

  async function togglePrivacy() {
    try {
      await api.setPrivacy(session, !privacy);
    } catch (e) {
      showNotice(errorText(e));
    }
  }

  /** Phones: the local view's zoom and offset, set by pinching. */
  let view = $state({ zoom: 1, x: 0, y: 0 });
  const fingers = new TouchControl({
    toRemote: (clientX, clientY) => toRemote({ clientX, clientY } as MouseEvent),
    origin: () => {
      const rect = canvas?.getBoundingClientRect();
      return { x: rect?.left ?? 0, y: rect?.top ?? 0 };
    },
    send: (event) => {
      if (streaming && !drawing) send(event);
    },
    view: (zoom, x, y) => (view = { zoom, x, y }),
  });

  /** Phones: the system keyboard, through a hidden field, and a row of extra keys. */
  let keyboardOpen = $state(false);
  let typing = $state<HTMLTextAreaElement>();
  /** Modifiers pressed on the extra keys, held for the next key or text. */
  let sticky = $state<Set<string>>(new Set());
  // The field always holds this; what is added or removed is typed or deleted.
  const SENTINEL = "\u200b";

  function toggleKeyboard() {
    keyboardOpen = !keyboardOpen;
    if (keyboardOpen) {
      tick().then(() => {
        if (typing) {
          typing.value = SENTINEL;
          typing.focus();
        }
      });
    } else {
      typing?.blur();
      sticky = new Set();
    }
  }

  /** A key with the held modifiers, which are let go afterwards. */
  function pressKey(code: string) {
    const mods = [...sticky];
    for (const m of mods) send({ Key: { code: m, down: true } });
    send({ Key: { code, down: true } });
    send({ Key: { code, down: false } });
    for (const m of mods.reverse()) send({ Key: { code: m, down: false } });
    if (mods.length) sticky = new Set();
  }

  function toggleSticky(code: string) {
    const next = new Set(sticky);
    if (next.has(code)) next.delete(code);
    else next.add(code);
    sticky = next;
  }

  function typeText(text: string) {
    if (!text) return;
    // With Ctrl or Alt held, letters are shortcuts: send them as keys.
    if (sticky.size > 0 && /^[a-z0-9]$/i.test(text)) {
      pressKey(/[0-9]/.test(text) ? `Digit${text}` : `Key${text.toUpperCase()}`);
      return;
    }
    // Anything else goes as text: the held modifiers do not apply to it.
    sticky = new Set();
    if (features.typeText) {
      send({ Text: text });
    } else {
      // Older hosts know keys only: letters (with Shift for capitals), digits, space.
      let dropped = false;
      for (const c of text) {
        if (/[a-z]/i.test(c)) {
          if (c !== c.toLowerCase()) sticky = new Set(["ShiftLeft"]);
          pressKey(`Key${c.toUpperCase()}`);
        } else if (/[0-9]/.test(c)) pressKey(`Digit${c}`);
        else if (c === " ") pressKey("Space");
        else dropped = true;
      }
      if (dropped) showNotice("Dieses Gerät nimmt nur Buchstaben, Ziffern und Leerzeichen an");
    }
  }

  let composing = false;
  function onTypingInput() {
    if (!typing || composing) return;
    const value = typing.value;
    if (value.length < SENTINEL.length || !value.startsWith(SENTINEL)) {
      // The sentinel itself was deleted: Backspace.
      const removed = Math.max(1, SENTINEL.length - value.length);
      for (let i = 0; i < removed; i++) pressKey("Backspace");
    } else {
      typeText(value.slice(SENTINEL.length).replace(/\n/g, ""));
      if (value.includes("\n")) pressKey("Enter");
    }
    typing.value = SENTINEL;
  }

  function onTypingKey(e: KeyboardEvent) {
    // Keys the system keyboard reports as such (not as text).
    if (e.key === "Enter") {
      e.preventDefault();
      pressKey("Enter");
    } else if (e.key === "Backspace" && typing?.value === SENTINEL) {
      e.preventDefault();
      pressKey("Backspace");
    }
  }

  /** Phones: the phone's clipboard to the host, by hand. */
  async function sendPhoneClipboard() {
    try {
      const text = await readText();
      if (!text) return showNotice("Die Zwischenablage ist leer");
      await api.sendClipboard(session, text);
      showNotice("Zwischenablage gesendet");
    } catch (e) {
      showNotice(errorText(e));
    }
  }

  const EXTRA_KEYS: [string, string][] = [
    ["Escape", "Esc"],
    ["Tab", "Tab"],
    ["ArrowLeft", "←"],
    ["ArrowUp", "↑"],
    ["ArrowDown", "↓"],
    ["ArrowRight", "→"],
    ["Delete", "Entf"],
    ["Home", "Pos1"],
    ["End", "Ende"],
  ];
  const STICKY_KEYS: [string, string][] = [
    ["ControlLeft", "Strg"],
    ["AltLeft", "Alt"],
    ["MetaLeft", "Win"],
    ["ShiftLeft", "⇧"],
  ];

  onMount(() => {
    if (MOBILE && canvas) {
      // Not passive: the page must not scroll or zoom itself under the fingers.
      const opts = { passive: false } as const;
      // In drawing mode one finger draws; otherwise it controls the host.
      const at = (e: TouchEvent) => {
        const t = e.targetTouches[0] ?? e.changedTouches[0];
        return { clientX: t.clientX, clientY: t.clientY, preventDefault() {} } as unknown as PointerEvent;
      };
      const start = (e: TouchEvent) => {
        if (!drawing) return fingers.onStart(e);
        e.preventDefault();
        if (e.targetTouches.length === 1) drawDown(at(e));
      };
      const move = (e: TouchEvent) => {
        if (!drawing) return fingers.onMove(e);
        e.preventDefault();
        if (e.targetTouches.length === 1) drawMove(at(e));
      };
      const end = (e: TouchEvent) => {
        if (!drawing) return fingers.onEnd(e);
        e.preventDefault();
        if (e.targetTouches.length === 0) drawUp();
      };
      canvas.addEventListener("touchstart", start, opts);
      canvas.addEventListener("touchmove", move, opts);
      canvas.addEventListener("touchend", end, opts);
      canvas.addEventListener("touchcancel", end, opts);
    }
    const player = new Player(
      canvas!,
      () => api.requestKeyframe(session),
      () => (streaming = true),
    );

    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (message) => {
      const buffer = message instanceof ArrayBuffer ? message : new Uint8Array(message as number[]).buffer;
      const view = new DataView(buffer);
      const kind = view.getUint8(0);
      if (kind === 1) {
        frameCount++;
        byteCount += buffer.byteLength;
        const width = view.getUint32(4, true);
        const height = view.getUint32(8, true);
        if (width !== video.width || height !== video.height) video = { width, height };
        player.push(view.getUint8(1) === 1, view.getUint32(4, true), view.getUint32(8, true), new Uint8Array(buffer, 12));
      } else if (kind === 3) {
        showCursor(view.getUint32(4, true), view.getUint32(8, true), buffer);
      } else if (kind === 4) {
        sound?.push(new Uint8Array(buffer, 12));
      } else if (kind === 2) {
        closed = new TextDecoder().decode(new Uint8Array(buffer, 12)) || "Die Verbindung wurde beendet.";
        player.close();
        sound?.close();
      }
    };

    api
      .attach(session, channel)
      .then((attached) => {
        const { host: info, id, label: name, features: supported, direct: route } = attached;
        host = info;
        hostId = id;
        label = name;
        display = info.active_display;
        features = supported;
        direct = route ?? direct;
        if (attached.rights !== null) applyRights(attached.rights);
        privacy = attached.privacy;
        lockOnEnd = attached.lockOnEnd;
        if (supported.audio && SoundPlayer.supported()) {
          sound = new SoundPlayer();
          soundReady = true;
          sound.setMuted(!soundOn);
          if (soundOn) api.setAudio(session, true).catch(() => {});
        }
      })
      .catch((e) => (closed = errorText(e)));

    hideToolbarSoon();

    const unlistenRoute = getCurrentWindow().listen<string>("route", (e) => (direct = e.payload));
    const unlistenRights = getCurrentWindow().listen<number>("rights", (e) => applyRights(e.payload));
    const unlistenInfo = getCurrentWindow().listen<SystemInfo>("system-info", (e) => (info = e.payload));
    const unlistenFiles = getCurrentWindow().listen<string[]>("host-files", (e) => {
      hostFiles = e.payload;
      toolbarVisible = true;
    });
    const unlistenPrivacy = getCurrentWindow().listen<{ on: boolean; error: string | null }>("privacy", (e) => {
      privacy = e.payload.on;
      if (e.payload.error) showNotice(e.payload.error);
    });
    // Phones: text copied on the host lands in the phone's clipboard.
    const unlistenClipboard = MOBILE
      ? getCurrentWindow().listen<string>("remote-clipboard", (e) => {
          if (can(RIGHT.CLIPBOARD)) writeText(e.payload).catch(() => {});
        })
      : Promise.resolve(() => {});
    const unlistenChat = getCurrentWindow().listen<string>("chat", (e) => {
      chatMessages = [...chatMessages, { mine: false, text: e.payload, at: Date.now() }];
      if (!chatOpen) {
        unread += 1;
        toolbarVisible = true;
        hideToolbarSoon();
      }
    });

    // Files dropped from the OS go to the remote desktop; the files window shows the progress.
    const unlistenDrop = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "drop") uploadToDesktop(event.payload.paths);
    });

    // Audio may only start after a user gesture; any click or key will do.
    const wake = () => sound?.resume();
    window.addEventListener("pointerdown", wake);
    window.addEventListener("keydown", wake);

    window.addEventListener("pointermove", touch);
    window.addEventListener("pointerdown", touch);
    window.addEventListener("keydown", touch);
    window.addEventListener("wheel", touch);
    const idleTimer = setInterval(checkIdle, 10_000);

    const statsTimer = setInterval(() => {
      stats = { fps: frameCount, kbps: Math.round((byteCount * 8) / 1000) };
      frameCount = 0;
      byteCount = 0;
    }, 1000);

    return () => {
      clearInterval(statsTimer);
      clearInterval(idleTimer);
      window.removeEventListener("pointermove", touch);
      window.removeEventListener("pointerdown", touch);
      window.removeEventListener("keydown", touch);
      window.removeEventListener("wheel", touch);
      player.close();
      sound?.close();
      mic?.stop();
      window.removeEventListener("pointerdown", wake);
      window.removeEventListener("keydown", wake);
      unlistenDrop.then((off) => off());
      unlistenRoute.then((off) => off());
      unlistenRights.then((off) => off());
      unlistenFiles.then((off) => off());
      unlistenInfo.then((off) => off());
      clearInterval(clock);
      unlistenPrivacy.then((off) => off());
      clearTimeout(noticeTimer);
      unlistenChat.then((off) => off());
      unlistenClipboard.then((off) => off());
    };
  });

  function loadSound(): boolean {
    try {
      return localStorage.getItem("ctxremote.sound") !== "off";
    } catch {
      return true;
    }
  }

  function toggleSound() {
    soundOn = !soundOn;
    try {
      localStorage.setItem("ctxremote.sound", soundOn ? "on" : "off");
    } catch {
      // Not remembered then.
    }
    sound?.setMuted(!soundOn);
    sound?.resume();
    // Off also stops the capture on the host, which saves bandwidth.
    api.setAudio(session, soundOn).catch(() => {});
  }

  // The files window does the upload, so its progress and errors show there.
  function uploadToDesktop(paths: string[]) {
    if (closed !== null || paths.length === 0 || !features.files) return;
    api.queueDrop(session, paths).catch(() => {});
  }

  // The host's pointer image becomes the local cursor over the stream, so text
  // fields, resize handles and busy states look as they do on the host.
  function showCursor(width: number, height: number, buffer: ArrayBuffer) {
    if (width === 0 || height === 0 || width > 128 || height > 128) return;
    if (buffer.byteLength < 20 + width * height * 4) return;
    const view = new DataView(buffer);
    const hotX = view.getUint32(12, true);
    const hotY = view.getUint32(16, true);
    const pixels = new Uint8ClampedArray(buffer, 20, width * height * 4);
    const draw = document.createElement("canvas");
    draw.width = width;
    draw.height = height;
    draw.getContext("2d")!.putImageData(new ImageData(new Uint8ClampedArray(pixels), width, height), 0, 0);
    cursor = `url(${draw.toDataURL("image/png")}) ${hotX} ${hotY}, default`;
  }

  // Mouse moves are coalesced to one message per animation frame.
  let pendingMove: { x: number; y: number } | null = null;
  function flushMove() {
    if (pendingMove) send({ MouseMove: pendingMove });
    pendingMove = null;
  }

  function toRemote(e: MouseEvent) {
    const c = canvas!;
    const rect = c.getBoundingClientRect();
    const scale = Math.min(rect.width / c.width, rect.height / c.height);
    const left = rect.left + (rect.width - c.width * scale) / 2;
    const top = rect.top + (rect.height - c.height * scale) / 2;
    return {
      x: Math.max(0, Math.min(c.width - 1, Math.round((e.clientX - left) / scale))),
      y: Math.max(0, Math.min(c.height - 1, Math.round((e.clientY - top) / scale))),
    };
  }

  function onMove(e: PointerEvent) {
    if (!streaming) return;
    // Fingers are handled by `fingers` on phones.
    if (MOBILE && e.pointerType === "touch") return;
    if (drawing) return drawMove(e);
    if (pendingMove === null) requestAnimationFrame(flushMove);
    pendingMove = toRemote(e);
  }

  function onButton(e: MouseEvent, down: boolean) {
    if (!streaming) return;
    if (drawing) return down ? drawDown(e) : drawUp();
    e.preventDefault();
    canvas?.focus();
    flushMove();
    send({ MouseMove: toRemote(e) });
    send({ MouseButton: { button: BUTTONS[e.button] ?? "Left", down } });
  }

  let wheel = { dx: 0, dy: 0 };
  function onWheel(e: WheelEvent) {
    if (!streaming) return;
    e.preventDefault();
    // Windows counts 120 per notch; Chromium reports ~100 px per notch.
    const factor = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 1200 : 1.2;
    wheel.dx += e.deltaX * factor;
    wheel.dy -= e.deltaY * factor;
    const dx = Math.trunc(wheel.dx);
    const dy = Math.trunc(wheel.dy);
    if (dx || dy) {
      wheel = { dx: wheel.dx - dx, dy: wheel.dy - dy };
      send({ Wheel: { dx, dy } });
    }
  }

  // Keys typed into the chat panel must stay local.
  // Typing in the chat or in a field of a panel (port tunnel, …) stays on this computer.
  const inChat = (e: Event) =>
    e.target instanceof Element && e.target.closest("[data-chat], input, textarea, select") !== null;

  async function sendChat(text: string) {
    await api.sendChat(session, text);
    chatMessages = [...chatMessages, { mine: true, text, at: Date.now() }];
  }

  function toggleChat() {
    if (chatOpen) return closeChat();
    // Keys still held on the remote device would otherwise stay pressed while typing here.
    send("ReleaseAll");
    chatOpen = true;
    unread = 0;
    keysOpen = false;
    qualityOpen = false;
    clearTimeout(hideTimer);
    tick().then(() => chatPanel?.focus());
  }

  function closeChat() {
    chatOpen = false;
    canvas?.focus();
    hideToolbarSoon();
  }

  // On a Mac, Cmd does what Ctrl does on the Windows device (copy, paste, …),
  // and Ctrl stands in for the Windows key.
  const MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  const MAC_SWAP: Record<string, string> = {
    MetaLeft: "ControlLeft",
    MetaRight: "ControlRight",
    ControlLeft: "MetaLeft",
    ControlRight: "MetaRight",
  };
  /** Keys held down here, as sent to the host. */
  const held = new Set<string>();

  function onKey(e: KeyboardEvent, down: boolean) {
    if (inChat(e)) return;
    if (!streaming || closed !== null || !e.code) return;
    e.preventDefault();
    e.stopPropagation();
    const shortcut = MAC ? e.metaKey : e.ctrlKey;
    if (e.code === "KeyV" && (shortcut || pasting) && canPasteFiles()) {
      if (down && !pasting) pasteThenV();
      return;
    }
    const code = MAC ? (MAC_SWAP[e.code] ?? e.code) : e.code;
    if (down) {
      held.add(code);
    } else {
      held.delete(code);
    }
    send({ Key: { code, down } });
    // macOS reports no key-up for keys released while Cmd is down, so they
    // would stay pressed at the host: letting go of Cmd lets go of them.
    if (MAC && !down && (e.code === "MetaLeft" || e.code === "MetaRight")) {
      for (const other of [...held]) {
        if (other.startsWith("Shift") || other.startsWith("Alt") || other.startsWith("Meta") || other.startsWith("Control")) continue;
        held.delete(other);
        send({ Key: { code: other, down: false } });
      }
    }
  }

  const canPasteFiles = () => features.filePaste && can(RIGHT.FILES) && can(RIGHT.CLIPBOARD) && can(RIGHT.INPUT);

  // Files on this computer's clipboard go to the host's first; then Ctrl+V
  // there pastes them. Without files it is a plain Ctrl+V.
  async function pasteThenV() {
    pasting = true;
    let files = false;
    try {
      const pending = api.pasteFiles(session);
      // Only a paste that takes a moment gets a notice.
      const slow = setTimeout(() => showNotice("Dateien werden übertragen …"), 400);
      files = await pending.finally(() => clearTimeout(slow));
    } catch (e) {
      showNotice(`Einfügen fehlgeschlagen: ${errorText(e)}`);
      pasting = false;
      return;
    }
    if (files) notice = "";
    pasting = false;
    combo("ControlLeft", "KeyV");
  }

  function toggleInfo() {
    infoOpen = !infoOpen;
    tunnelOpen = false;
    // Fresh each time it opens: disks and memory change.
    if (infoOpen) api.requestSystemInfo(session).catch(() => {});
  }

  function bytes(n: number): string {
    const units = ["B", "KB", "MB", "GB", "TB"];
    let i = 0;
    while (n >= 1024 && i < units.length - 1) {
      n /= 1024;
      i++;
    }
    return `${n.toLocaleString("de-DE", { maximumFractionDigits: n < 10 && i > 0 ? 1 : 0 })} ${units[i]}`;
  }

  function uptime(secs: number): string {
    const days = Math.floor(secs / 86400);
    const hours = Math.floor((secs % 86400) / 3600);
    const minutes = Math.floor((secs % 3600) / 60);
    if (days > 0) return `${days} ${days === 1 ? "Tag" : "Tage"}, ${hours} Std.`;
    if (hours > 0) return `${hours} Std., ${minutes} Min.`;
    return `${minutes} Min.`;
  }

  async function fetchHostFiles() {
    fetching = true;
    try {
      const count = await api.fetchHostFiles(session);
      hostFiles = [];
      showNotice(`${count === 1 ? "Die Datei liegt" : `${count} Dateien liegen`} in der Zwischenablage, einfügen mit Strg+V.`);
    } catch (e) {
      showNotice(errorText(e));
    } finally {
      fetching = false;
    }
  }

  function combo(...codes: string[]) {
    for (const code of codes) send({ Key: { code, down: true } });
    for (const code of [...codes].reverse()) send({ Key: { code, down: false } });
    keysOpen = false;
    canvas?.focus();
  }

  function sendSas() {
    api.sendSas(session);
    keysOpen = false;
    canvas?.focus();
  }

  // Windows ignores an injected Win+L, so locking is its own request.
  function lockScreen() {
    api.lockScreen(session);
    keysOpen = false;
    canvas?.focus();
  }

  function restart() {
    if (!confirmRestart) {
      confirmRestart = true;
      return;
    }
    api.restartHost(session);
    confirmRestart = false;
    keysOpen = false;
  }

  function loadScaleMode(): "fit" | "fill" {
    try {
      return localStorage.getItem("ctxremote.scale") === "fill" ? "fill" : "fit";
    } catch {
      return "fit";
    }
  }

  function chooseScale(mode: "fit" | "fill") {
    scaleMode = mode;
    try {
      localStorage.setItem("ctxremote.scale", mode);
    } catch {
      // Not remembered, but still applied.
    }
    qualityOpen = false;
    canvas?.focus();
  }

  function chooseQuality(value: Quality) {
    quality = value;
    api.setQuality(session, value);
    qualityOpen = false;
    canvas?.focus();
  }

  function chooseDisplay(index: number) {
    display = index;
    api.selectDisplay(session, index);
  }

  async function toggleFullscreen() {
    fullscreen = !(await appWindow.isFullscreen());
    await appWindow.setFullscreen(fullscreen);
    canvas?.focus();
  }

  async function disconnect() {
    await api.disconnect(session);
    await closeView();
  }

  /** The window on a computer; on a phone, back to the start page. */
  async function closeView() {
    if (MOBILE) {
      await api.disconnect(session).catch(() => {});
      goHome();
    } else {
      await appWindow.close();
    }
  }

  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  function hideToolbarSoon() {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (!keysOpen && !qualityOpen && !chatOpen) toolbarVisible = false;
    }, 2400);
  }

  function onWindowMove(e: MouseEvent) {
    if (e.clientY < 56) {
      toolbarVisible = true;
      clearTimeout(hideTimer);
    } else if (toolbarVisible) {
      hideToolbarSoon();
    }
  }
</script>

<svelte:window
  onkeydown={(e) => onKey(e, true)}
  onkeyup={(e) => onKey(e, false)}
  onblur={() => send("ReleaseAll")}
  onmousemove={onWindowMove}
/>
<svelte:document onvisibilitychange={() => document.hidden && send("ReleaseAll")} />

<div class="stage">
  <canvas
    bind:this={canvas}
    class:live={streaming}
    style:cursor={drawing ? "crosshair" : streaming ? cursor : "default"}
    style:max-width={scaleMode === "fit" && video.width ? `${video.width / devicePixelRatio}px` : null}
    style:max-height={scaleMode === "fit" && video.height ? `${video.height / devicePixelRatio}px` : null}
    style:transform={MOBILE && view.zoom > 1 ? `translate(${view.x}px, ${view.y}px) scale(${view.zoom})` : null}
    class:touch={MOBILE}
    tabindex="-1"
    onpointermove={onMove}
    onmousedown={(e) => onButton(e, true)}
    onmouseup={(e) => onButton(e, false)}
    oncontextmenu={(e) => e.preventDefault()}
    onwheel={onWheel}
  ></canvas>

  {#if !streaming && closed === null}
    <div class="overlay">
      <span class="spinner"></span>
      <span>{host ? `Warte auf Bild von ${host.hostname} …` : "Sitzung wird geöffnet …"}</span>
    </div>
  {/if}

  {#if closed !== null}
    <div class="overlay ended">
      <div class="card">
        <h1>Sitzung beendet</h1>
        <p>{closed}</p>
        <form class="again" onsubmit={reconnect}>
          {#if askPassword}
            <input class="again-field" type="password" bind:value={rePassword} placeholder="Passwort des Geräts" autocomplete="off" />
          {/if}
          {#if askCode}
            <input class="again-field" bind:value={reCode} inputmode="numeric" maxlength="7" placeholder="Bestätigungscode" autocomplete="one-time-code" />
          {/if}
          {#if reError}<p class="again-error">{reError}</p>{/if}
          <div class="again-actions">
            <button type="button" class="btn btn-quiet" onclick={closeView}>{MOBILE ? "Zurück" : "Fenster schließen"}</button>
            <button class="btn btn-primary" disabled={reconnecting}>{reconnecting ? "Verbinde …" : "Erneut verbinden"}</button>
          </div>
        </form>
      </div>
    </div>
  {/if}

  {#if host && closed === null}
    <div
      class="toolbar"
      class:hidden={!toolbarVisible}
      role="toolbar"
      tabindex="-1"
      onmouseenter={() => {
        toolbarVisible = true;
        clearTimeout(hideTimer);
      }}
      onmouseleave={hideToolbarSoon}
    >
      <div class="who">
        <span class="live-dot"></span>
        <span class="host">{label || host.hostname}</span>
        {#if hostId}<span class="id">{hostId}</span>{/if}
        <span class="route" title={direct ? `Direkt verbunden über ${direct}` : "Die Verbindung läuft über den Server"}>
          {direct ? "Direkt" : "Über Server"}
        </span>
        {#if !can(RIGHT.INPUT)}
          <span class="route" title="Die Gegenseite erlaubt Maus und Tastatur nicht">Nur ansehen</span>
        {/if}
        {#if privacy}
          <span class="route private" title="Der Bildschirm am Gerät ist schwarz, Maus und Tastatur dort sind gesperrt">Privat</span>
        {/if}
      </div>

      {#if host.displays.length > 1}
        <div class="sep"></div>
        <div class="displays">
          {#each host.displays as d (d.index)}
            <button
              class="seg"
              class:active={d.index === display}
              title={`${d.name} · ${d.width}×${d.height}`}
              onclick={() => chooseDisplay(d.index)}>{d.index + 1}</button
            >
          {/each}
        </div>
      {/if}

      <div class="sep"></div>
      {#if can(RIGHT.INPUT)}
      <div class="menu-anchor">
        <button
          class="tool"
          title="Tastenkombinationen"
          onclick={() => {
            keysOpen = !keysOpen;
            qualityOpen = false;
            confirmRestart = false;
          }}
        >
          <Icon name="keyboard" size={17} />
        </button>
        {#if keysOpen}
          <div class="menu" role="menu">
            <button role="menuitem" onclick={sendSas}>Strg + Alt + Entf</button>
            <button role="menuitem" onclick={() => combo("MetaLeft")}>Windows-Taste</button>
            <button role="menuitem" onclick={() => combo("AltLeft", "Tab")}>Alt + Tab</button>
            <button role="menuitem" onclick={() => combo("ControlLeft", "ShiftLeft", "Escape")}>Task-Manager</button>
            <button role="menuitem" onclick={lockScreen}>Sperren</button>
            {#if features.typeText}
              <button role="menuitem" title="Für Stellen, an denen Einfügen nicht geht, z. B. die Anmeldung" onclick={typeClipboard}>
                Zwischenablage eintippen
              </button>
            {/if}
            <button role="menuitemcheckbox" aria-checked={lockOnEnd} onclick={toggleLockOnEnd}>
              <span class="mark">{#if lockOnEnd}<Icon name="check" size={14} />{/if}</span>
              <span class="item-label">Beim Trennen sperren</span>
            </button>
            {#if features.restart && can(RIGHT.RESTART)}
              <div class="menu-sep"></div>
              <button role="menuitem" class:danger={confirmRestart} onclick={restart}>
                {confirmRestart ? "Wirklich neu starten?" : "Neu starten …"}
              </button>
            {/if}
          </div>
        {/if}
      </div>
      {/if}
      <div class="menu-anchor">
        <button
          class="tool"
          title="Bild"
          onclick={() => {
            qualityOpen = !qualityOpen;
            keysOpen = false;
          }}
        >
          <Icon name="sliders" size={17} />
        </button>
        {#if qualityOpen}
          <div class="menu" role="menu">
            {#if features.quality}
              {#each QUALITIES as [value, title, hint] (value)}
                <button role="menuitemradio" aria-checked={quality === value} onclick={() => chooseQuality(value)}>
                  <span class="mark">{#if quality === value}<Icon name="check" size={14} />{/if}</span>
                  <span class="item-label">{title}<small>{hint}</small></span>
                </button>
              {/each}
              <div class="menu-sep"></div>
            {/if}
            {#each SCALES as [value, title, hint] (value)}
              <button role="menuitemradio" aria-checked={scaleMode === value} onclick={() => chooseScale(value)}>
                <span class="mark">{#if scaleMode === value}<Icon name="check" size={14} />{/if}</span>
                <span class="item-label">{title}<small>{hint}</small></span>
              </button>
            {/each}
            <div class="menu-sep"></div>
            <button role="menuitemcheckbox" aria-checked={showStats} onclick={toggleStats}>
              <span class="mark">{#if showStats}<Icon name="check" size={14} />{/if}</span>
              <span class="item-label">Verbindungsdaten<small>Bilder pro Sekunde, Datenrate, Weg</small></span>
            </button>
            {#if can(RIGHT.INPUT)}
              <button role="menuitemcheckbox" aria-checked={watchOnly} onclick={toggleWatchOnly}>
                <span class="mark">{#if watchOnly}<Icon name="check" size={14} />{/if}</span>
                <span class="item-label">Nur ansehen<small>keine Maus und Tastatur senden</small></span>
              </button>
            {/if}
          </div>
        {/if}
      </div>
      {#if features.chat}
        <button class="tool chat-tool" class:active={chatOpen} title="Chat" onclick={toggleChat}>
          <Icon name="chat" size={17} />
          {#if unread > 0}<span class="badge">{unread > 9 ? "9+" : unread}</span>{/if}
        </button>
      {/if}
      {#if features.files && can(RIGHT.FILES)}
        <button class="tool" title="Dateien" onclick={() => api.openFiles(session)}>
          <Icon name="folder" size={17} />
        </button>
      {/if}
      <button class="tool" title="Bildschirmfoto speichern" disabled={!streaming} onclick={screenshot}>
        <Icon name="camera" size={17} />
      </button>
      <button
        class="tool"
        class:recording={recordingSince !== null}
        title={recordingSince === null ? "Sitzung als Video aufzeichnen" : "Aufnahme beenden"}
        onclick={toggleRecording}
      >
        <Icon name="record" size={17} />
        {#if recordingSince !== null}<span class="rec-time">{recordingFor}</span>{/if}
      </button>
      {#if features.draw && can(RIGHT.INPUT)}
        <button class="tool" class:active={drawing} title={drawing ? "Zeichnen beenden" : "Auf dem Bildschirm zeichnen"} onclick={toggleDrawing}>
          <Icon name="pencil" size={17} />
        </button>
        {#if drawing}
          <div class="draw-colors">
            {#each DRAW_COLORS as color (color)}
              <button
                class="swatch"
                class:chosen={drawColor === color}
                style:background={`#${color.toString(16).padStart(6, "0")}`}
                title="Farbe"
                aria-label="Farbe"
                onclick={() => (drawColor = color)}
              ></button>
            {/each}
            <button class="tool" title="Alles löschen" onclick={() => api.drawClear(session)}>
              <Icon name="trash" size={15} />
            </button>
          </div>
        {/if}
      {/if}
      {#if features.tunnel && can(RIGHT.TUNNEL)}
        <button class="tool" class:active={tunnelOpen} title="Port-Tunnel ins Netzwerk des Geräts" onclick={toggleTunnels}>
          <Icon name="tunnel" size={17} />
        </button>
      {/if}
      {#if features.sysinfo}
        <button class="tool" class:active={infoOpen} title="Informationen zum Gerät" onclick={toggleInfo}>
          <Icon name="info" size={17} />
        </button>
      {/if}
      {#if features.privacy && can(RIGHT.PRIVACY)}
        <button
          class="tool"
          class:active={privacy}
          title={privacy ? "Privatsphäre-Modus beenden" : "Privatsphäre-Modus: Bildschirm am Gerät schwarz, Maus und Tastatur dort gesperrt"}
          onclick={togglePrivacy}
        >
          <Icon name={privacy ? "eyeOff" : "eye"} size={17} />
        </button>
      {/if}
      {#if features.mic && can(RIGHT.AUDIO) && MicSender.supported()}
        <button class="tool" class:active={micOn} title={micOn ? "Mikrofon aus" : "Mikrofon an: Ihre Stimme beim Gerät"} onclick={toggleMic}>
          <Icon name={micOn ? "mic" : "micOff"} size={17} />
        </button>
      {/if}
      {#if features.audio && soundReady && can(RIGHT.AUDIO)}
        <button class="tool" title={soundOn ? "Ton aus" : "Ton an"} onclick={toggleSound}>
          <Icon name={soundOn ? "volume" : "volumeOff"} size={17} />
        </button>
      {/if}
      <button class="tool" title={fullscreen ? "Vollbild verlassen" : "Vollbild"} onclick={toggleFullscreen}>
        <Icon name={fullscreen ? "shrink" : "expand"} size={17} />
      </button>
      <button class="tool end" title="Trennen" onclick={disconnect}>
        <Icon name="power" size={17} />
      </button>
    </div>
  {/if}

  {#if notice && closed === null}
    <div class="notice" role="status">{notice}</div>
  {:else if hostFiles.length > 0 && closed === null && canPasteFiles()}
    <div class="notice offer" role="status">
      <span>
        {hostFiles.length === 1 ? `„${hostFiles[0]}“` : `${hostFiles.length} Dateien`} am Gerät kopiert
      </span>
      <button class="offer-btn" disabled={fetching} onclick={fetchHostFiles}>
        {fetching ? "Wird geholt …" : "Hierher holen"}
      </button>
      <button class="offer-close" title="Ausblenden" onclick={() => (hostFiles = [])}>
        <Icon name="close" size={14} />
      </button>
    </div>
  {/if}

  {#if showStats && streaming && closed === null}
    <div class="stats" aria-label="Verbindungsdaten">
      {stats.fps} fps · {stats.kbps >= 1000 ? `${(stats.kbps / 1000).toFixed(1).replace(".", ",")} Mbit/s` : `${stats.kbps} kbit/s`}
      · {video.width}×{video.height} · {direct ? "direkt" : "über Server"}{watchOnly ? " · nur ansehen" : ""}
    </div>
  {/if}

  {#if tunnelOpen && closed === null}
    <aside class="info-box" aria-label="Port-Tunnel">
      <header>
        <strong>Port-Tunnel</strong>
        <button class="offer-close" title="Schließen" onclick={() => (tunnelOpen = false)}>
          <Icon name="close" size={14} />
        </button>
      </header>
      <p class="info-wait">
        Verbindungen zu einem Port auf diesem Computer gehen über das Gerät an ein Ziel in dessen Netz, z. B. Remotedesktop
        (Port 3389) auf einem Server dort.
      </p>
      {#each tunnels as t (t.port)}
        <div class="tunnel-row">
          <code>localhost:{t.port}</code>
          <span>→ {t.target}</span>
          <button class="offer-close" title="Tunnel schließen" onclick={() => closeTunnel(t.port)}>
            <Icon name="close" size={14} />
          </button>
        </div>
      {/each}
      <form class="tunnel-form" onsubmit={openTunnel}>
        <input class="tunnel-field" bind:value={tunnelTarget} placeholder="Ziel, z. B. 192.168.1.10:3389" spellcheck="false" />
        <input class="tunnel-field port" bind:value={tunnelPort} inputmode="numeric" placeholder="Port hier (frei)" />
        <button class="offer-btn" disabled={!tunnelTarget.trim()}>Öffnen</button>
      </form>
      {#if tunnelError}<p class="tunnel-error">{tunnelError}</p>{/if}
    </aside>
  {/if}

  {#if infoOpen && closed === null}
    <aside class="info-box" aria-label="Informationen zum Gerät">
      <header>
        <strong>{info?.hostname ?? host?.hostname ?? "Gerät"}</strong>
        <button class="offer-close" title="Schließen" onclick={() => (infoOpen = false)}>
          <Icon name="close" size={14} />
        </button>
      </header>
      {#if info}
        <dl>
          <dt>System</dt>
          <dd>{info.os}{#if info.os_build}<small>Build {info.os_build}</small>{/if}</dd>
          {#if info.model}<dt>Modell</dt><dd>{info.model}</dd>{/if}
          <dt>Prozessor</dt>
          <dd>{info.cpu || "unbekannt"}{#if info.cores}<small>{info.cores} Kerne</small>{/if}</dd>
          <dt>Arbeitsspeicher</dt>
          <dd>
            {bytes(info.memory_used)} von {bytes(info.memory_total)} belegt
            <span class="bar"><span style:width={`${Math.round((info.memory_used / Math.max(info.memory_total, 1)) * 100)}%`}></span></span>
          </dd>
          <dt>Läuft seit</dt>
          <dd>{uptime(info.uptime_secs)}</dd>
          {#if info.user}<dt>Benutzer</dt><dd>{info.user}</dd>{/if}
          {#each info.disks as disk (disk.mount)}
            <dt>Laufwerk {disk.mount}</dt>
            <dd>
              {bytes(disk.free)} frei von {bytes(disk.total)}
              <span class="bar"><span style:width={`${Math.round(((disk.total - disk.free) / Math.max(disk.total, 1)) * 100)}%`}></span></span>
            </dd>
          {/each}
          {#each info.networks as net (net.name)}
            <dt>{net.name}</dt>
            <dd>{net.addresses.join(", ")}<small>{net.mac}</small></dd>
          {/each}
          <dt>CTXRemote</dt>
          <dd>{info.app_version}</dd>
        </dl>
      {:else}
        <p class="info-wait">Wird abgefragt …</p>
      {/if}
    </aside>
  {/if}

  {#if chatOpen && features.chat}
    <div class="chat-box">
      <ChatPanel
        bind:this={chatPanel}
        variant="dark"
        messages={chatMessages}
        onsend={sendChat}
        onclose={closeChat}
        disabled={closed !== null}
      />
    </div>
  {/if}
  {#if MOBILE && streaming && closed === null}
    <div class="phone-controls">
      <button class="phone-btn" title="Leiste" onclick={() => (toolbarVisible = !toolbarVisible)}>
        <Icon name="sliders" size={18} />
      </button>
      {#if can(RIGHT.INPUT)}
        <button
          class="phone-btn"
          class:active={keyboardOpen}
          title="Tastatur"
          onpointerdown={(e) => e.preventDefault()}
          onclick={toggleKeyboard}
        >
          <Icon name="keyboard" size={18} />
        </button>
      {/if}
      {#if can(RIGHT.CLIPBOARD)}
        <button class="phone-btn" title="Zwischenablage senden" onclick={sendPhoneClipboard}>
          <Icon name="copy" size={18} />
        </button>
      {/if}
      {#if view.zoom > 1}
        <button class="phone-btn" title="Ganzes Bild" onclick={() => fingers.resetView()}>
          <Icon name="shrink" size={18} />
        </button>
      {/if}
    </div>
    {#if keyboardOpen}
      <textarea
        bind:this={typing}
        class="typing"
        autocapitalize="off"
        autocomplete="off"
        spellcheck="false"
        oninput={onTypingInput}
        onkeydown={onTypingKey}
        oncompositionstart={() => (composing = true)}
        oncompositionend={() => {
          composing = false;
          onTypingInput();
        }}
        onblur={() => {
          keyboardOpen = false;
          sticky = new Set();
        }}
      ></textarea>
      <div class="extra-keys" role="toolbar" aria-label="Sondertasten">
        {#each STICKY_KEYS as [code, label] (code)}
          <button class:held={sticky.has(code)} onpointerdown={(e) => e.preventDefault()} onclick={() => toggleSticky(code)}>{label}</button>
        {/each}
        {#each EXTRA_KEYS as [code, label] (code)}
          <button onpointerdown={(e) => e.preventDefault()} onclick={() => pressKey(code)}>{label}</button>
        {/each}
      </div>
    {/if}
  {/if}
</div>

<style>
  /* Scoped by class: all component CSS ships in one bundle, the main window included. */
  :global(body.session) {
    background: #0b0b0a;
    color: #edebe6;
    overflow: hidden;
  }

  .stage {
    position: relative;
    display: grid;
    place-items: center;
    height: 100%;
  }

  canvas {
    width: 100%;
    height: 100%;
    object-fit: contain;
    outline: none;
    opacity: 0;
    transition: opacity 200ms var(--ease);
  }

  canvas.live {
    opacity: 1;
  }

  /* Phones: fingers are handled in code; the view zooms from its corner. */
  canvas.touch {
    touch-action: none;
    transform-origin: 0 0;
  }

  .phone-controls {
    position: absolute;
    right: 10px;
    bottom: 10px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .phone-btn {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border: 1px solid #2f2e2b;
    border-radius: 50%;
    background: rgb(24 24 22 / 0.85);
    color: #d8d6d0;
  }

  .phone-btn.active {
    background: #2c2c29;
    color: #fff;
  }

  /* Off screen but focusable, so the system keyboard opens. */
  .typing {
    position: fixed;
    left: 0;
    bottom: 0;
    width: 1px;
    height: 1px;
    opacity: 0;
    border: 0;
    padding: 0;
    font-size: 16px;
  }

  .extra-keys {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    gap: 4px;
    padding: 6px calc(6px + var(--safe-right, 0px)) calc(6px + var(--safe-bottom, 0px)) calc(6px + var(--safe-left, 0px));
    overflow-x: auto;
    background: #181816;
    border-top: 1px solid #2f2e2b;
  }

  .extra-keys button {
    flex: none;
    min-width: 44px;
    height: 36px;
    padding: 0 10px;
    border: 1px solid #3b3a37;
    border-radius: 6px;
    background: #222220;
    color: #edebe6;
    font-size: 14px;
  }

  .extra-keys button.held {
    background: #4fb495;
    border-color: #4fb495;
    color: #0b0b0a;
  }

  /* Phones: the bar wraps instead of scrolling, so its menus are not cut off. */
  :global(body.mobile) .toolbar {
    flex-wrap: wrap;
    justify-content: center;
    width: max-content;
    max-width: calc(100% - 16px);
    height: auto;
    min-height: 40px;
    padding: 2px 4px;
  }

  /* Menus on a phone: centred on the screen, below the bar, never off its edges. */
  :global(body.mobile) .menu {
    position: fixed;
    top: calc(var(--safe-top) + 96px);
    left: 50%;
    max-width: calc(100vw - 24px);
    max-height: 60vh;
    overflow-y: auto;
    z-index: 5;
  }

  /* Room for the buttons: name only. */
  :global(body.mobile) .toolbar .id,
  :global(body.mobile) .toolbar .route {
    display: none;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 12px;
    color: #a5a39c;
    font-size: 13px;
  }

  .overlay.ended {
    background: color-mix(in srgb, #0b0b0a 82%, transparent);
  }

  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    text-align: center;
  }

  h1 {
    margin: 0;
    color: #edebe6;
    font: 600 18px var(--font-display);
  }

  .card p {
    max-width: 360px;
    margin: 0 0 14px;
  }

  .card .btn-quiet {
    border-color: #3b3a37;
    color: #edebe6;
  }

  .card .btn-quiet:hover {
    background: #222220;
  }

  .again {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 8px;
    width: 300px;
  }

  .again-field {
    height: 36px;
    padding: 0 10px;
    border: 1px solid #3b3a37;
    border-radius: 8px;
    background: #181816;
    color: #edebe6;
    font-size: 14px;
  }

  .again-error {
    margin: 0 !important;
    color: #e07a66;
    font-size: 12.5px;
  }

  .again-actions {
    display: flex;
    justify-content: center;
    gap: 8px;
  }

  .toolbar {
    position: absolute;
    top: 10px;
    left: 50%;
    display: flex;
    align-items: center;
    gap: 2px;
    height: 40px;
    padding: 0 4px 0 14px;
    border: 1px solid #2f2e2b;
    border-radius: 10px;
    background: #181816;
    color: #d8d6d0;
    transform: translateX(-50%);
    transition: transform 220ms var(--ease), opacity 220ms var(--ease);
  }

  .toolbar.hidden {
    opacity: 0;
    transform: translate(-50%, -150%);
  }

  .who {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-right: 8px;
    font-size: 13px;
    white-space: nowrap;
  }

  .live-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #57b384;
  }

  .host {
    font-weight: 600;
  }

  .id {
    color: #85837c;
    font-variant-numeric: tabular-nums;
  }

  .sep {
    width: 1px;
    height: 18px;
    margin: 0 6px;
    background: #2f2e2b;
  }

  .displays {
    display: flex;
    gap: 2px;
  }

  .seg,
  .tool {
    display: grid;
    place-items: center;
    min-width: 32px;
    height: 30px;
    padding: 0 8px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: inherit;
    font-size: 12.5px;
    font-weight: 600;
  }

  .seg:hover,
  .tool:hover {
    background: #262624;
  }

  .seg.active {
    background: #2c2c29;
    color: #fff;
  }

  .tool.end:hover {
    background: color-mix(in srgb, #e07a66 18%, transparent);
    color: #e07a66;
  }

  .menu-anchor {
    position: relative;
  }

  .chat-tool {
    position: relative;
  }

  .chat-tool.active {
    background: #2c2c29;
    color: #fff;
  }

  .badge {
    position: absolute;
    top: 1px;
    right: 1px;
    min-width: 14px;
    height: 14px;
    padding: 0 3px;
    border-radius: 7px;
    background: #4fb495;
    color: #0d1a16;
    font-size: 10px;
    line-height: 14px;
  }

  .info-box {
    position: absolute;
    top: 60px;
    right: 14px;
    width: 340px;
    max-height: calc(100% - 80px);
    overflow: auto;
    padding: 12px 14px;
    border: 1px solid #2f2e2b;
    border-radius: 10px;
    background: #181816;
    color: #d8d6d0;
    font-size: 13px;
  }

  .info-box header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .info-box dl {
    display: grid;
    grid-template-columns: 110px 1fr;
    gap: 6px 10px;
    margin: 0;
  }

  .info-box dt {
    color: #8f8d86;
  }

  .info-box dd {
    display: grid;
    gap: 3px;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .info-box small {
    color: #8f8d86;
    font-size: 11.5px;
  }

  .bar {
    display: block;
    height: 4px;
    border-radius: 2px;
    background: #2f2e2b;
  }

  .bar span {
    display: block;
    height: 100%;
    border-radius: 2px;
    background: #4fb495;
  }

  .draw-colors {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 4px;
  }

  .swatch {
    width: 16px;
    height: 16px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
  }

  .swatch.chosen {
    border-color: #d8d6d0;
  }

  .tunnel-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 6px 0;
  }

  .tunnel-row span {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .tunnel-form {
    display: flex;
    gap: 6px;
    margin-top: 10px;
  }

  .tunnel-field {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border: 1px solid #3b3a37;
    border-radius: 6px;
    background: #23221f;
    color: inherit;
    font: inherit;
  }

  .tunnel-field.port {
    flex: 0 0 110px;
  }

  .tunnel-error {
    margin: 6px 0 0;
    color: #e5654f;
  }

  .info-wait {
    margin: 0;
    color: #8f8d86;
  }

  .chat-box {
    position: absolute;
    right: 14px;
    bottom: 14px;
    display: flex;
    width: 320px;
    max-height: 50%;
    overflow: hidden;
    border: 1px solid #2f2e2b;
    border-radius: 10px;
    background: #181816;
    color: #d8d6d0;
  }

  .menu {
    position: absolute;
    top: 38px;
    left: 50%;
    display: flex;
    flex-direction: column;
    min-width: 230px;
    padding: 4px;
    border: 1px solid #2f2e2b;
    border-radius: 9px;
    background: #181816;
    transform: translateX(-50%);
  }

  .menu button {
    height: 32px;
    padding: 0 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: inherit;
    font-size: 13px;
    text-align: left;
  }

  .menu button:hover {
    background: #262624;
  }

  .menu button.danger {
    color: #e07a66;
  }

  .route {
    padding: 1px 7px;
    border: 1px solid #2f2e2b;
    border-radius: 999px;
    color: #a5a39c;
    font-size: 11.5px;
  }

  .notice.offer {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .offer-btn {
    height: 26px;
    padding: 0 10px;
    border: 1px solid #3b3a37;
    border-radius: 6px;
    background: #23221f;
    color: inherit;
    font-size: 12.5px;
    font-weight: 600;
  }

  .offer-close {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border: 0;
    background: transparent;
    color: #a5a39c;
  }

  .tool.recording {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 8px;
    color: #e5654f;
  }

  .rec-time {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .route.private {
    border-color: #4a4537;
    color: #e0c98a;
  }

  .stats {
    position: absolute;
    left: 10px;
    bottom: 10px;
    padding: 4px 8px;
    border-radius: 6px;
    background: rgb(0 0 0 / 0.6);
    color: #e8e6e0;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    pointer-events: none;
  }

  .notice {
    position: absolute;
    top: 60px;
    left: 50%;
    max-width: min(520px, calc(100% - 32px));
    padding: 8px 14px;
    border: 1px solid #2f2e2b;
    border-radius: 8px;
    background: #181816;
    color: #d8d6d0;
    font-size: 13px;
    transform: translateX(-50%);
  }

  .menu-sep {
    height: 1px;
    margin: 4px 6px;
    background: #2f2e2b;
  }

  .menu button[role="menuitemradio"],
  .menu button[role="menuitemcheckbox"] {
    display: flex;
    align-items: center;
    gap: 6px;
    height: auto;
    padding: 6px 10px 6px 6px;
  }

  .mark {
    display: grid;
    place-items: center;
    width: 16px;
    flex: none;
    color: #4fb495;
  }

  .item-label {
    display: flex;
    flex-direction: column;
  }

  .item-label small {
    color: #85837c;
    font-size: 11.5px;
  }
</style>
