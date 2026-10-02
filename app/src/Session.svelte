<script lang="ts">
  import { onMount } from "svelte";
  import { Channel } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { api, errorText, type HostInfo, type InputEvent, type MouseButton } from "./lib/api";
  import Icon from "./lib/Icon.svelte";
  import { Player } from "./lib/player";

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
  let fullscreen = $state(false);

  const appWindow = getCurrentWindow();
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
        player.push(view.getUint8(1) === 1, view.getUint32(4, true), view.getUint32(8, true), new Uint8Array(buffer, 12));
      } else if (kind === 2) {
        closed = new TextDecoder().decode(new Uint8Array(buffer, 12)) || "Die Verbindung wurde beendet.";
        player.close();
      }
    };

    api
      .attach(session, channel)
      .then(({ host: info, id, label: name }) => {
        host = info;
        hostId = id;
        label = name;
        display = info.active_display;
      })
      .catch((e) => (closed = errorText(e)));

    hideToolbarSoon();
    return () => player.close();
  });

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

  function onKey(e: KeyboardEvent, down: boolean) {
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
      if (!keysOpen) toolbarVisible = false;
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
        <button class="tool" title="Tastenkombinationen" onclick={() => (keysOpen = !keysOpen)}>
          <Icon name="keyboard" size={17} />
        </button>
        {#if keysOpen}
          <div class="menu" role="menu">
            <button role="menuitem" onclick={sendSas}>Strg + Alt + Entf</button>
            <button role="menuitem" onclick={() => combo("MetaLeft")}>Windows-Taste</button>
            <button role="menuitem" onclick={() => combo("AltLeft", "Tab")}>Alt + Tab</button>
            <button role="menuitem" onclick={() => combo("ControlLeft", "ShiftLeft", "Escape")}>Task-Manager</button>
            <button role="menuitem" onclick={lockScreen}>Sperren</button>
          </div>
        {/if}
      </div>
      <button class="tool" title={fullscreen ? "Vollbild verlassen" : "Vollbild"} onclick={toggleFullscreen}>
        <Icon name={fullscreen ? "shrink" : "expand"} size={17} />
      </button>
      <button class="tool end" title="Trennen" onclick={disconnect}>
        <Icon name="power" size={17} />
      </button>
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
</style>
