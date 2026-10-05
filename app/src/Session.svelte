<script lang="ts">
  import { onMount, tick } from "svelte";
  import { Channel } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, errorText, type HostInfo, type InputEvent, type HostFeatures, type MouseButton, type Quality } from "./lib/api";
  import ChatPanel, { type ChatMessage } from "./lib/ChatPanel.svelte";
  import Icon from "./lib/Icon.svelte";
  import { Player } from "./lib/player";
  import { SoundPlayer } from "./lib/sound";

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
  let features = $state<HostFeatures>({ files: false, restart: false, quality: false, chat: false, audio: false });
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
    if (closed === null) api.sendInput(session, event);
  };

  onMount(() => {
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
      .then(({ host: info, id, label: name, features: supported, direct: route }) => {
        host = info;
        hostId = id;
        label = name;
        display = info.active_display;
        features = supported;
        direct = route ?? direct;
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

    return () => {
      player.close();
      sound?.close();
      window.removeEventListener("pointerdown", wake);
      window.removeEventListener("keydown", wake);
      unlistenDrop.then((off) => off());
      unlistenRoute.then((off) => off());
      unlistenChat.then((off) => off());
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
    if (pendingMove === null) requestAnimationFrame(flushMove);
    pendingMove = toRemote(e);
  }

  function onButton(e: MouseEvent, down: boolean) {
    if (!streaming) return;
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
  const inChat = (e: Event) => e.target instanceof Element && e.target.closest("[data-chat]") !== null;

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

  function onKey(e: KeyboardEvent, down: boolean) {
    if (inChat(e)) return;
    if (!streaming || closed !== null || !e.code) return;
    e.preventDefault();
    e.stopPropagation();
    send({ Key: { code: e.code, down } });
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
    await appWindow.close();
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
    style:cursor={streaming ? cursor : "default"}
    style:max-width={scaleMode === "fit" && video.width ? `${video.width / devicePixelRatio}px` : null}
    style:max-height={scaleMode === "fit" && video.height ? `${video.height / devicePixelRatio}px` : null}
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
        <button class="btn btn-quiet" onclick={() => appWindow.close()}>Fenster schließen</button>
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
            {#if features.restart}
              <div class="menu-sep"></div>
              <button role="menuitem" class:danger={confirmRestart} onclick={restart}>
                {confirmRestart ? "Wirklich neu starten?" : "Neu starten …"}
              </button>
            {/if}
          </div>
        {/if}
      </div>
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
                  <span class="label">{title}<small>{hint}</small></span>
                </button>
              {/each}
              <div class="menu-sep"></div>
            {/if}
            {#each SCALES as [value, title, hint] (value)}
              <button role="menuitemradio" aria-checked={scaleMode === value} onclick={() => chooseScale(value)}>
                <span class="mark">{#if scaleMode === value}<Icon name="check" size={14} />{/if}</span>
                <span class="label">{title}<small>{hint}</small></span>
              </button>
            {/each}
          </div>
        {/if}
      </div>
      {#if features.chat}
        <button class="tool chat-tool" class:active={chatOpen} title="Chat" onclick={toggleChat}>
          <Icon name="chat" size={17} />
          {#if unread > 0}<span class="badge">{unread > 9 ? "9+" : unread}</span>{/if}
        </button>
      {/if}
      {#if features.files}
        <button class="tool" title="Dateien" onclick={() => api.openFiles(session)}>
          <Icon name="folder" size={17} />
        </button>
      {/if}
      {#if features.audio && soundReady}
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
    min-width: 180px;
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

  .menu-sep {
    height: 1px;
    margin: 4px 6px;
    background: #2f2e2b;
  }

  .menu button[role="menuitemradio"] {
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
    color: #4fb495;
  }

  .label {
    display: flex;
    flex-direction: column;
  }

  .label small {
    color: #85837c;
    font-size: 11.5px;
  }
</style>
