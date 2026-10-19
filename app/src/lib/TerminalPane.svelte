<script lang="ts">
  import { onMount, type Snippet } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { Terminal } from "@xterm/xterm";
  import { FitAddon } from "@xterm/addon-fit";
  import "@xterm/xterm/css/xterm.css";
  import { api, errorText } from "./api";
  import Icon from "./Icon.svelte";

  // A command line at the host. Always dark and headed "Terminal", so nobody
  // takes it for screen control: what is typed here runs as a shell there.
  let {
    session,
    active = true,
    disabled = false,
    onclose,
    actions,
  }: {
    session: number;
    /** False while hidden (another tab, panel closed): no resizing then. */
    active?: boolean;
    /** The session itself ended: no restart. */
    disabled?: boolean;
    /** Shows a close button that hides the pane (the shell keeps running). */
    onclose?: () => void;
    /** More buttons in the header. */
    actions?: Snippet;
  } = $props();

  type Phase = "starting" | "running" | "ended" | "failed";
  let phase = $state<Phase>("starting");
  /** What runs, as the host describes it, e.g. "bash als max auf werkstatt". */
  let what = $state("");
  let problem = $state("");
  let host = $state<HTMLDivElement>();

  let term: Terminal | undefined;
  let fit: FitAddon | undefined;
  /** Each shell gets a new number, so late data of an ended one is ignored. */
  let current = 0;

  export function focus() {
    term?.focus();
  }

  function refit() {
    if (!active || !host || host.clientWidth === 0 || host.clientHeight === 0) return;
    try {
      fit?.fit();
    } catch {
      // Not laid out yet.
    }
  }

  // Becoming visible: size to the space now there, and take the keys.
  $effect(() => {
    if (active) {
      requestAnimationFrame(() => {
        refit();
        term?.focus();
      });
    }
  });

  async function start() {
    if (!term) return;
    const id = ++current;
    phase = "starting";
    what = "";
    problem = "";
    term.reset();
    refit();
    try {
      const sent = await api.terminalOpen(session, id, term.cols, term.rows);
      if (!sent && id === current) {
        phase = "failed";
        problem = "Das Gerät kennt das Terminal noch nicht (ältere Version).";
      }
    } catch (e) {
      if (id === current) {
        phase = "failed";
        problem = errorText(e);
      }
    }
  }

  const decode = (b64: string) => Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));

  onMount(() => {
    term = new Terminal({
      fontFamily: 'ui-monospace, "Cascadia Mono", Consolas, Menlo, "DejaVu Sans Mono", monospace',
      fontSize: 13,
      cursorBlink: true,
      scrollback: 5000,
      theme: {
        background: "#0f1311",
        foreground: "#d8d6d0",
        cursor: "#4fb495",
        selectionBackground: "rgba(79, 180, 149, 0.3)",
      },
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host!);
    refit();

    term.onData((data) => {
      if (phase === "running" || phase === "starting") api.terminalInput(session, current, data).catch(() => {});
    });
    term.onResize(({ cols, rows }) => {
      if (phase === "running" || phase === "starting") api.terminalResize(session, current, cols, rows).catch(() => {});
    });
    const observer = new ResizeObserver(refit);
    observer.observe(host!);

    const unlisten = getCurrentWindow().listen<{ kind: string; id: number; ok: boolean; text: string; data: string }>(
      `terminal-${session}`,
      (e) => {
        const m = e.payload;
        if (m.id !== current) return;
        if (m.kind === "opened") {
          if (m.ok) {
            phase = "running";
            what = m.text;
            refit();
            term?.focus();
          } else {
            phase = "failed";
            problem = m.text;
          }
        } else if (m.kind === "data") {
          term?.write(decode(m.data));
        } else if (m.kind === "close") {
          if (phase === "running" || phase === "starting") phase = "ended";
        }
      },
    );

    start();

    return () => {
      observer.disconnect();
      unlisten.then((off) => off());
      if (phase === "running" || phase === "starting") api.terminalClose(session, current).catch(() => {});
      term?.dispose();
    };
  });
</script>

<section class="terminal" aria-label="Terminal am fernen Gerät">
  <header>
    <span class="chip"><Icon name="terminal" size={16} /></span>
    <span class="head-text">
      <strong>Terminal</strong>
      <small title={what}>
        {#if phase === "starting"}Wird gestartet …{:else if phase === "running"}{what}{:else if phase === "ended"}Beendet{:else}Nicht gestartet{/if}
      </small>
    </span>
    <span class="head-actions">
      {@render actions?.()}
      {#if onclose}
        <button class="head-btn icon" title="Terminal ausblenden (die Befehlszeile läuft weiter)" aria-label="Terminal ausblenden" onclick={onclose}>
          <Icon name="close" size={14} />
        </button>
      {/if}
    </span>
  </header>
  <div class="body">
    <div class="screen" bind:this={host} class:dim={phase === "ended" || phase === "failed"}></div>
    {#if phase === "ended" || phase === "failed"}
      <div class="note" role="status">
        <strong>{phase === "ended" ? "Terminal beendet" : "Terminal nicht gestartet"}</strong>
        {#if problem}<span>{problem}</span>{/if}
        {#if !disabled}
          <button class="head-btn" onclick={start}>
            <Icon name="refresh" size={14} /> Neu starten
          </button>
        {/if}
      </div>
    {/if}
  </div>
</section>

<style>
  .terminal {
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
    background: #0f1311;
    color: #d8d6d0;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-bottom: 1px solid #25302b;
    background: #131916;
  }

  .chip {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: 7px;
    background: #0d5c4b;
    color: #e8f4ef;
  }

  .head-text {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    line-height: 1.25;
  }

  .head-text strong {
    color: #edebe6;
    font-size: 13.5px;
    font-weight: 600;
  }

  .head-text small {
    overflow: hidden;
    color: #4fb495;
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .head-actions {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }

  .head-actions :global(.head-btn),
  .head-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border: 1px solid #34413b;
    border-radius: 7px;
    background: transparent;
    color: #d8d6d0;
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
  }

  .head-actions :global(.head-btn:hover),
  .head-btn:hover {
    border-color: #4fb495;
    color: #fff;
  }

  .head-btn.icon {
    justify-content: center;
    width: 28px;
    padding: 0;
    border-color: transparent;
  }

  .body {
    position: relative;
    flex: 1;
    min-height: 0;
    padding: 6px 0 6px 10px;
  }

  .screen {
    width: 100%;
    height: 100%;
    overflow: hidden;
  }

  .screen.dim {
    opacity: 0.45;
  }

  .note {
    position: absolute;
    left: 50%;
    bottom: 18px;
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: calc(100% - 24px);
    padding: 8px 12px;
    border: 1px solid #34413b;
    border-radius: 9px;
    background: #131916;
    font-size: 13px;
    transform: translateX(-50%);
  }

  .note span {
    color: #b3b1aa;
  }
</style>
