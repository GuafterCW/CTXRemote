<script lang="ts">
  import { onMount, untrack, type Snippet } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import {
    api,
    errorText,
    type ProcessInfo,
    type ProcessListing,
    type ProcessUpdate,
    type ServiceActionName,
    type ServiceInfo,
    type ServiceState,
    type StartMode,
  } from "./api";
  import { formatProgress, formatSize } from "./format";
  import Icon from "./Icon.svelte";

  // A lean task manager for the host: running programs and, where it has them,
  // services. Ending and stopping run with the host's rights, so each asks first.
  let {
    session,
    hostname = "",
    hostOs = "",
    active = true,
    disabled = false,
    onclose,
    actions,
  }: {
    session: number;
    /** Shown under the title. */
    hostname?: string;
    /** The host's system; under the Windows service actions run as an administrator. */
    hostOs?: string;
    /** False while hidden (another tab, panel closed): no polling then. */
    active?: boolean;
    /** The session itself ended. */
    disabled?: boolean;
    /** Shows a close button that hides the pane. */
    onclose?: () => void;
    /** More buttons in the header. */
    actions?: Snippet;
  } = $props();

  const admin = $derived(/windows/i.test(hostOs));
  /** Said in the pane and in every question before an action. */
  const rightsNote = $derived(
    admin
      ? "Aktionen laufen dort mit Administratorrechten, also mit mehr Rechten, als der angemeldete Benutzer selbst hat."
      : "Aktionen laufen dort mit den Rechten, mit denen Remocto auf dem Gerät läuft.",
  );

  const POLL_MS = 3000;
  /** Starting a service can take this long on the host; after that the buttons come back anyway. */
  const BUSY_MAX_MS = 45_000;

  type SortKey = "name" | "pid" | "user" | "cpu" | "memory";

  let listing = $state<ProcessListing | null>(null);
  /** Why there is no table (right missing, old host); polling waits for the next rights change. */
  let problem = $state("");
  let tab = $state<"processes" | "services">("processes");
  let query = $state("");
  let sortKey = $state<SortKey>("cpu");
  let sortDown = $state(true);
  let selectedPid = $state<number | null>(null);
  let selectedService = $state<string | null>(null);
  let busy = $state(false);
  let message = $state<{ ok: boolean; text: string } | null>(null);
  let confirming = $state<{ title: string; text: string; label: string; run: () => void } | null>(null);
  let root = $state<HTMLElement>();

  const hasServices = $derived(listing?.services != null);
  const cpus = $derived(Math.max(1, listing?.cpus ?? 1));
  const perCent = new Intl.NumberFormat("de-DE", { minimumFractionDigits: 1, maximumFractionDigits: 1 });

  const share = (p: ProcessInfo) => p.cpu / cpus;

  const processRows = $derived.by(() => {
    if (!listing) return [];
    const q = query.trim().toLowerCase();
    const rows = q
      ? listing.processes.filter(
          (p) => p.name.toLowerCase().includes(q) || String(p.pid) === q || String(p.pid).startsWith(q) || p.user.toLowerCase().includes(q),
        )
      : [...listing.processes];
    const sign = sortDown ? -1 : 1;
    const text = (a: string, b: string) => a.localeCompare(b, "de", { sensitivity: "base" });
    rows.sort((a, b) => {
      let d = 0;
      if (sortKey === "name") d = text(a.name, b.name);
      else if (sortKey === "user") d = text(a.user, b.user);
      else if (sortKey === "pid") d = a.pid - b.pid;
      else if (sortKey === "cpu") d = a.cpu - b.cpu;
      else d = a.memory - b.memory;
      return d * sign || a.pid - b.pid;
    });
    return rows;
  });

  const serviceRows = $derived.by(() => {
    const all = listing?.services ?? [];
    const q = query.trim().toLowerCase();
    const rows = q ? all.filter((s) => s.name.toLowerCase().includes(q) || s.display.toLowerCase().includes(q)) : [...all];
    return rows.sort((a, b) => (a.display || a.name).localeCompare(b.display || b.name, "de", { sensitivity: "base" }));
  });

  const totalCpu = $derived(listing ? Math.min(100, listing.processes.reduce((n, p) => n + p.cpu, 0) / cpus) : 0);
  const totalMemory = $derived(listing ? listing.processes.reduce((n, p) => n + p.memory, 0) : 0);

  const selectedProcess = $derived(listing?.processes.find((p) => p.pid === selectedPid) ?? null);
  const selectedSvc = $derived(listing?.services?.find((s) => s.name === selectedService) ?? null);

  function sortBy(key: SortKey) {
    if (sortKey === key) {
      sortDown = !sortDown;
    } else {
      sortKey = key;
      sortDown = key !== "name" && key !== "user";
    }
  }

  const ariaSort = (key: SortKey) => (sortKey === key ? (sortDown ? "descending" : "ascending") : "none");

  async function load() {
    try {
      if (!(await api.processesList(session))) {
        problem = "Das Gerät kennt diese Ansicht noch nicht (ältere Version).";
      }
    } catch (e) {
      problem = errorText(e);
    }
  }

  // Poll while visible; a refusal (missing right) stops it until the rights change.
  $effect(() => {
    if (!active || disabled || problem) return;
    untrack(load);
    const timer = setInterval(load, POLL_MS);
    return () => clearInterval(timer);
  });

  // Take the keys, so none reach the remote device.
  $effect(() => {
    if (active) requestAnimationFrame(() => root?.focus({ preventScroll: true }));
  });

  let dismiss: ReturnType<typeof setTimeout> | undefined;
  let busyGuard: ReturnType<typeof setTimeout> | undefined;

  function say(ok: boolean, text: string) {
    message = { ok, text };
    clearTimeout(dismiss);
    if (ok) dismiss = setTimeout(() => (message = null), 6000);
  }

  function begin() {
    busy = true;
    message = null;
    clearTimeout(busyGuard);
    busyGuard = setTimeout(() => (busy = false), BUSY_MAX_MS);
  }

  function finish() {
    busy = false;
    clearTimeout(busyGuard);
  }

  async function send(run: () => Promise<boolean>) {
    begin();
    try {
      if (!(await run())) {
        finish();
        say(false, "Das Gerät kennt diese Ansicht noch nicht (ältere Version).");
      }
    } catch (e) {
      finish();
      say(false, errorText(e));
    }
  }

  function askKill() {
    const p = selectedProcess;
    if (!p) return;
    confirming = {
      title: `${p.name} beenden?`,
      text: `${p.name} (PID ${p.pid}) wirklich beenden? Nicht gespeicherte Daten gehen verloren.`,
      label: "Beenden",
      run: () => send(() => api.processKill(session, p.pid, p.name)),
    };
  }

  function act(action: ServiceActionName) {
    const s = selectedSvc;
    if (!s) return;
    const run = () => send(() => api.serviceAction(session, s.name, action));
    if (action === "Start") return run();
    const name = s.display || s.name;
    confirming = {
      title: action === "Stop" ? `${name} anhalten?` : `${name} neu starten?`,
      text:
        action === "Stop"
          ? `Den Dienst „${name}“ wirklich anhalten? Programme, die ihn brauchen, können ausfallen.`
          : `Den Dienst „${name}“ wirklich neu starten? Er ist dabei kurz nicht erreichbar.`,
      label: action === "Stop" ? "Anhalten" : "Neu starten",
      run,
    };
  }

  function confirmed() {
    const run = confirming?.run;
    confirming = null;
    root?.focus();
    run?.();
  }

  const STATE_LABEL: Record<ServiceState, string> = {
    Running: "Läuft",
    Stopped: "Angehalten",
    Starting: "Startet …",
    Stopping: "Hält an …",
    Paused: "Pausiert",
    Other: "–",
  };
  const START_LABEL: Record<StartMode, string> = {
    Automatic: "Automatisch",
    Manual: "Manuell",
    Disabled: "Deaktiviert",
    Other: "–",
  };

  const canStart = (s: ServiceInfo) => s.state === "Stopped" || s.state === "Other";
  const canStop = (s: ServiceInfo) => s.state === "Running" || s.state === "Paused" || s.state === "Starting";
  const canRestart = (s: ServiceInfo) => s.state === "Running" || s.state === "Paused";

  onMount(() => {
    const win = getCurrentWindow();
    const updates = win.listen<ProcessUpdate>(`processes-${session}`, (e) => {
      const m = e.payload;
      if (m.kind === "listing") {
        if (m.ok && m.listing) {
          listing = m.listing;
          problem = "";
          if (selectedPid !== null && !m.listing.processes.some((p) => p.pid === selectedPid)) selectedPid = null;
          if (selectedService !== null && !m.listing.services?.some((s) => s.name === selectedService)) selectedService = null;
          if (m.listing.services === null) tab = "processes";
        } else {
          problem = m.error || "Die Liste ist nicht verfügbar.";
        }
      } else if (m.kind === "done") {
        finish();
        say(m.ok, m.ok ? m.what : m.error || m.what || "Die Aktion ist fehlgeschlagen.");
        untrack(load);
      }
    });
    const rights = win.listen<number>(`rights-${session}`, () => {
      if (problem) problem = "";
    });
    return () => {
      clearTimeout(dismiss);
      clearTimeout(busyGuard);
      updates.then((off) => off());
      rights.then((off) => off());
    };
  });

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape" && confirming) {
      confirming = null;
      e.stopPropagation();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<!-- `data-chat` and the focusable root keep typed keys local (see `inChat` in Session). -->
<section class="pane" role="group" aria-label="Prozesse und Dienste am fernen Gerät" data-chat tabindex="-1" bind:this={root} onkeydown={onKeydown}>
  <header>
    <span class="chip"><Icon name="processes" size={16} /></span>
    <span class="head-text">
      <strong>Prozesse und Dienste</strong>
      <small title={hostname}>{hostname || "Fernes Gerät"}</small>
    </span>
    <span class="head-actions">
      {@render actions?.()}
      {#if onclose}
        <button class="head-btn icon" title="Ansicht ausblenden" aria-label="Prozesse ausblenden" onclick={onclose}>
          <Icon name="close" size={14} />
        </button>
      {/if}
    </span>
  </header>

  <p class="rights-note" class:admin><Icon name="info" size={14} /><span>{rightsNote}</span></p>

  <div class="bar">
    {#if hasServices}
      <div class="tabs" role="tablist">
        <button role="tab" aria-selected={tab === "processes"} class:on={tab === "processes"} onclick={() => (tab = "processes")}>Prozesse</button>
        <button role="tab" aria-selected={tab === "services"} class:on={tab === "services"} onclick={() => (tab = "services")}>Dienste</button>
      </div>
    {/if}
    <input
      class="search"
      type="search"
      placeholder={tab === "services" ? "Dienst suchen" : "Name, PID oder Benutzer suchen"}
      aria-label="Suchen"
      autocomplete="off"
      spellcheck="false"
      bind:value={query}
    />
    {#if listing}
      <span class="totals" title="Auslastung des ganzen Geräts">
        <span>CPU <b>{perCent.format(totalCpu)} %</b></span>
        <span>Speicher <b>{formatProgress(totalMemory, listing.memory_total)}</b></span>
      </span>
    {/if}
  </div>

  <div class="table-wrap">
    {#if problem}
      <div class="state" role="alert">
        <strong>Keine Liste</strong>
        <span>{problem}</span>
      </div>
    {:else if !listing}
      <div class="state"><span class="spinner"></span><span>Wird geladen …</span></div>
    {:else if tab === "processes"}
      <table>
        <colgroup>
          <col />
          <col class="c-pid" />
          <col class="c-user" />
          <col class="c-cpu" />
          <col class="c-mem" />
        </colgroup>
        <thead>
          <tr>
            {#each [["name", "Name", ""], ["pid", "PID", "num"], ["user", "Benutzer", ""], ["cpu", "CPU", "num"], ["memory", "Speicher", "num"]] as [key, label, cls] (key)}
              <th class={cls} aria-sort={ariaSort(key as SortKey)}>
                <button onclick={() => sortBy(key as SortKey)}>
                  {label}{#if sortKey === key}<span class="arrow" aria-hidden="true">{sortDown ? "↓" : "↑"}</span>{/if}
                </button>
              </th>
            {/each}
          </tr>
        </thead>
        <tbody>
          {#each processRows as p (p.pid)}
            <tr class:selected={p.pid === selectedPid} title={p.exe || p.name} onclick={() => (selectedPid = p.pid)}>
              <td class="name">{p.name}</td>
              <td class="num">{p.pid}</td>
              <td class="user">{p.user}</td>
              <td class="num">{perCent.format(share(p))} %</td>
              <td class="num">{formatSize(p.memory)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if processRows.length === 0}<p class="none">Nichts gefunden.</p>{/if}
    {:else}
      <table>
        <colgroup>
          <col />
          <col class="c-state" />
          <col class="c-start" />
        </colgroup>
        <thead>
          <tr>
            <th><span class="plain">Dienst</span></th>
            <th><span class="plain">Status</span></th>
            <th><span class="plain">Start</span></th>
          </tr>
        </thead>
        <tbody>
          {#each serviceRows as s (s.name)}
            <tr class:selected={s.name === selectedService} onclick={() => (selectedService = s.name)}>
              <td class="name two" title={s.display || s.name}>
                <span>{s.display || s.name}</span>
                {#if s.display && s.display !== s.name}<small>{s.name}</small>{/if}
              </td>
              <td>
                <span class="state-cell">
                  <i class="dot {s.state}" aria-hidden="true"></i>{STATE_LABEL[s.state]}
                </span>
              </td>
              <td class="dim">{START_LABEL[s.start]}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if serviceRows.length === 0}<p class="none">Nichts gefunden.</p>{/if}
    {/if}
  </div>

  <footer>
    <div class="status" role="status" aria-live="polite">
      {#if busy}
        <span class="spinner"></span><span>Wird ausgeführt …</span>
      {:else if message}
        <span class="msg" class:bad={!message.ok} title={message.text}>{message.text}</span>
        <button class="dismiss" aria-label="Meldung schließen" onclick={() => (message = null)}><Icon name="close" size={12} /></button>
      {:else if listing && !problem}
        <span class="hint">
          {#if tab === "processes"}
            {processRows.length === 1 ? "1 Prozess" : `${processRows.length} Prozesse`}
          {:else}
            {serviceRows.length === 1 ? "1 Dienst" : `${serviceRows.length} Dienste`}
          {/if}
        </span>
      {/if}
    </div>
    <div class="buttons">
      {#if tab === "processes"}
        {#if selectedProcess}
          <button class="btn btn-danger small" disabled={busy || disabled} onclick={askKill}>Beenden</button>
        {/if}
      {:else if selectedSvc}
        <button class="btn btn-quiet small" disabled={busy || disabled || !canStart(selectedSvc)} onclick={() => act("Start")}>Starten</button>
        <button class="btn btn-quiet small" disabled={busy || disabled || !canStop(selectedSvc)} onclick={() => act("Stop")}>Anhalten</button>
        <button class="btn btn-quiet small" disabled={busy || disabled || !canRestart(selectedSvc)} onclick={() => act("Restart")}>Neu starten</button>
      {/if}
    </div>
  </footer>

  {#if confirming}
    <div class="scrim" role="presentation">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="proc-confirm-{session}">
        <h3 id="proc-confirm-{session}">{confirming.title}</h3>
        <p>{confirming.text}</p>
        <p class="confirm-note" class:admin>{rightsNote}</p>
        <div class="actions">
          <button class="btn btn-quiet small" onclick={() => ((confirming = null), root?.focus())}>Abbrechen</button>
          <button class="btn btn-danger small" onclick={confirmed}>{confirming.label}</button>
        </div>
      </div>
    </div>
  {/if}
</section>

<style>
  .pane {
    position: relative;
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
    min-height: 0;
    background: var(--surface);
    color: var(--ink);
    outline: none;
  }

  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--surface);
  }

  .chip {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: 7px;
    background: var(--accent);
    color: var(--accent-ink);
  }

  .head-text {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    line-height: 1.25;
  }

  .head-text strong {
    font-size: 13.5px;
    font-weight: 600;
  }

  .head-text small {
    overflow: hidden;
    color: var(--accent);
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

  .head-actions :global(.head-btn) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 10px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: transparent;
    color: var(--ink);
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
  }

  .head-actions :global(.head-btn:hover) {
    border-color: var(--accent);
  }

  .head-btn.icon {
    display: inline-grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--ink-2);
  }

  .head-btn.icon:hover {
    color: var(--ink);
    background: color-mix(in srgb, var(--ink) 7%, transparent);
  }

  .bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
  }

  .tabs {
    display: inline-flex;
    padding: 2px;
    border: 1px solid var(--line-strong);
    border-radius: 8px;
  }

  .tabs button {
    height: 24px;
    padding: 0 12px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--ink-2);
    font-size: 12.5px;
    font-weight: 600;
  }

  .tabs button.on {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .search {
    flex: 1;
    min-width: 150px;
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--line-strong);
    border-radius: 7px;
    background: var(--raised);
    color: var(--ink);
    font: inherit;
    font-size: 12.5px;
    user-select: text;
  }

  .search:focus {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--focus);
  }

  .totals {
    display: flex;
    gap: 14px;
    flex: none;
    color: var(--ink-2);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .totals b {
    color: var(--ink);
    font-weight: 600;
  }

  .table-wrap {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  table {
    width: 100%;
    border-collapse: separate;
    border-spacing: 0;
    table-layout: fixed;
    font-size: 12.5px;
  }

  .c-pid {
    width: 58px;
  }

  .c-user {
    width: 104px;
  }

  .c-cpu {
    width: 60px;
  }

  .c-mem {
    width: 78px;
  }

  .c-state {
    width: 112px;
  }

  .c-start {
    width: 96px;
  }

  th {
    position: sticky;
    top: 0;
    z-index: 1;
    padding: 0;
    border-bottom: 1px solid var(--line);
    background: var(--surface);
    color: var(--ink-3);
    font-size: 11.5px;
    font-weight: 600;
    text-align: left;
  }

  th button,
  th .plain {
    display: block;
    width: 100%;
    padding: 6px 8px;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: inherit;
    white-space: nowrap;
  }

  th button:hover {
    color: var(--ink);
  }

  th.num {
    text-align: right;
  }

  .arrow {
    margin-left: 3px;
    color: var(--accent);
  }

  td {
    overflow: hidden;
    padding: 5px 8px;
    border-bottom: 1px solid var(--line);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  td.num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  td.user,
  td.dim {
    color: var(--ink-2);
  }

  tbody tr:hover {
    background: color-mix(in srgb, var(--ink) 4%, transparent);
  }

  tbody tr.selected {
    background: var(--accent-soft);
  }

  td.two {
    padding-top: 4px;
    padding-bottom: 4px;
  }

  td.two span,
  td.two small {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  td.two small {
    color: var(--ink-3);
    font-size: 11px;
  }

  .state-cell {
    display: inline-flex;
    align-items: center;
    gap: 7px;
  }

  .dot {
    width: 7px;
    height: 7px;
    flex: none;
    border-radius: 50%;
    background: var(--ink-3);
  }

  .dot.Running {
    background: var(--ok);
  }

  .dot.Starting,
  .dot.Stopping,
  .dot.Paused {
    background: var(--warn);
  }

  .state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    height: 100%;
    padding: 20px;
    color: var(--ink-2);
    text-align: center;
  }

  .state strong {
    color: var(--ink);
  }

  .none {
    margin: 0;
    padding: 18px;
    color: var(--ink-3);
    text-align: center;
  }

  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    min-height: 46px;
    padding: 7px 10px;
    border-top: 1px solid var(--line);
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    color: var(--ink-2);
    font-size: 12.5px;
  }

  .hint {
    color: var(--ink-3);
  }

  .msg {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .msg.bad {
    color: var(--bad);
    font-weight: 600;
  }

  .dismiss {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    flex: none;
    padding: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--ink-3);
  }

  .dismiss:hover {
    color: var(--ink);
  }

  .buttons {
    display: flex;
    gap: 6px;
    flex: none;
  }

  .scrim {
    position: absolute;
    inset: 0;
    z-index: 2;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--bg) 75%, transparent);
  }

  .dialog {
    width: min(320px, 90%);
    padding: 16px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--raised);
  }

  .dialog h3 {
    margin: 0 0 4px;
    font: 600 14px var(--font-display);
    overflow-wrap: anywhere;
  }

  .dialog p {
    margin: 0 0 14px;
    color: var(--ink-2);
    font-size: 13px;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .small {
    height: 30px;
    padding: 0 14px;
    font-size: 12.5px;
  }

  .btn-danger {
    background: var(--bad);
    color: #fff;
  }

  .btn-danger:disabled {
    opacity: 0.5;
  }
  .rights-note {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 0;
    padding: 8px 16px;
    border-bottom: 1px solid var(--line);
    color: var(--ink-2);
    font-size: 12px;
    line-height: 1.4;
  }
  .rights-note :global(svg) {
    flex: none;
    margin-top: 1px;
  }
  .rights-note.admin {
    color: var(--warn);
  }
  .confirm-note {
    font-size: 12.5px;
    color: var(--ink-2);
  }
  .confirm-note.admin {
    color: var(--warn);
  }
</style>
