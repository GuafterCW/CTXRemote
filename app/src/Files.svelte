<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import { api, errorText, type FileEntry, type Listing, type TransferUpdate } from "./lib/api";
  import FilePane from "./lib/FilePane.svelte";
  import { formatProgress } from "./lib/format";
  import Icon from "./lib/Icon.svelte";
  import { MOBILE } from "./lib/platform";

  let { session }: { session: number } = $props();

  interface Transfer {
    id: number;
    up: boolean;
    name: string;
    done: number;
    total: number;
    status: "running" | "done" | "failed" | "cancelled";
    message: string;
    /** Folder the file lands in (remote for uploads, local for downloads). */
    target: string;
    /** Local path of a finished download. */
    saved: string | null;
  }

  let local = $state<FilePane>();
  let remote = $state<FilePane>();
  let localDir = $state("");
  let remoteDir = $state("");
  let remoteEl = $state<HTMLElement>();
  let start = $state<string | null>(null);
  let ended = $state(false);
  let dropActive = $state(false);
  let transfers = $state<Transfer[]>([]);
  let collapsed = $state(false);
  let note = $state("");
  const early = new Map<number, TransferUpdate[]>();

  const ENDED = "Die Sitzung ist beendet.";

  function guard<T>(call: () => Promise<T>): Promise<T> {
    return call().catch((e) => {
      if (errorText(e).includes("Die Sitzung ist beendet")) ended = true;
      throw e;
    });
  }

  const remoteApi = {
    list: (path: string): Promise<Listing> => guard(() => api.remoteList(session, path)),
    createDir: (path: string) => guard(() => api.remoteCreateDir(session, path)),
    rename: (path: string, name: string) => guard(() => api.remoteRename(session, path, name)),
    remove: (paths: string[]) => guard(() => api.remoteDelete(session, paths)),
  };

  const baseName = (p: string) => p.split(/[\\/]/).filter(Boolean).pop() ?? p;
  const norm = (p: string) => p.replace(/[\\/]+$/, "");
  const canUpload = $derived(!ended && remoteDir !== "");
  const canDownload = $derived(!ended && localDir !== "");
  const running = $derived(transfers.filter((t) => t.status === "running").length);

  onMount(() => {
    api
      .localHome()
      .catch(() => "")
      .then((home) => (start = home));

    const unlisten = listen<TransferUpdate>("transfer", (e) => {
      if (e.payload.session === session) apply(e.payload);
    });
    // Files dropped on the session window go to the remote desktop.
    const unlistenQueued = listen<number>("files-drop", () => uploadDrops());
    uploadDrops();

    const unlistenDrop = getCurrentWebview().onDragDropEvent((event) => {
      const p = event.payload;
      if (p.type === "leave") {
        dropActive = false;
        return;
      }
      const over = overRemote(p.position.x, p.position.y);
      if (p.type === "drop") {
        dropActive = false;
        if (over && canUpload) for (const path of p.paths) startUpload(path);
      } else {
        dropActive = over && canUpload;
      }
    });

    return () => {
      unlisten.then((off) => off());
      unlistenQueued.then((off) => off());
      unlistenDrop.then((off) => off());
    };
  });

  function overRemote(px: number, py: number): boolean {
    if (!remoteEl) return false;
    const r = remoteEl.getBoundingClientRect();
    const x = px / window.devicePixelRatio;
    const y = py / window.devicePixelRatio;
    return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
  }

  function apply(update: TransferUpdate) {
    const t = transfers.find((x) => x.id === update.id);
    if (!t) {
      early.set(update.id, [...(early.get(update.id) ?? []), update]);
      return;
    }
    if (t.status !== "running") return;
    if (update.kind === "progress") {
      t.done = update.done;
      t.total = update.total;
    } else if (update.kind === "finished") {
      t.status = "done";
      t.done = t.total;
      t.saved = update.path;
      if (t.up && norm(remoteDir) === norm(t.target)) remote?.reload();
      if (!t.up && norm(localDir) === norm(t.target)) local?.reload();
    } else {
      t.status = "failed";
      t.message = update.message;
    }
  }

  function add(id: number, up: boolean, name: string, target: string) {
    transfers.push({ id, up, name, done: 0, total: 0, status: "running", message: "", target, saved: null });
    collapsed = false;
    for (const u of early.get(id) ?? []) apply(u);
    early.delete(id);
  }

  async function uploadDrops() {
    try {
      const paths = await api.takeDrops(session);
      if (paths.length === 0) return;
      const top = await guard(() => api.remoteList(session, ""));
      const place =
        top.entries.find((e) => e.kind === "Place" && e.name === "Desktop") ??
        top.entries.find((e) => e.kind === "Place");
      if (!place) {
        note = "Kein Zielordner auf dem ferngesteuerten Gerät gefunden";
        return;
      }
      for (const path of paths) startUpload(path, place.path);
    } catch (e) {
      note = errorText(e);
    }
  }

  async function startUpload(path: string, dir = remoteDir) {
    try {
      add(await guard(() => api.upload(session, path, dir)), true, baseName(path), dir);
    } catch (e) {
      note = errorText(e);
    }
  }

  async function startDownload(entry: FileEntry) {
    const dir = localDir;
    try {
      add(await guard(() => api.download(session, entry.path, dir)), false, entry.name, dir);
    } catch (e) {
      note = errorText(e);
    }
  }

  function upload(entries: FileEntry[] = local?.selectedEntries() ?? []) {
    if (!canUpload) return;
    note = "";
    for (const e of entries) startUpload(e.path);
  }

  function download(entries: FileEntry[] = remote?.selectedEntries() ?? []) {
    if (!canDownload) return;
    note = "";
    for (const e of entries) startDownload(e);
  }

  function cancel(t: Transfer) {
    t.status = "cancelled";
    api.cancelTransfer(session, t.id).catch((e) => (note = errorText(e)));
  }

  function clearDone() {
    transfers = transfers.filter((t) => t.status === "running");
  }

  const percent = (t: Transfer) =>
    t.status === "done" ? 100 : t.total > 0 ? Math.min(100, (t.done / t.total) * 100) : 0;
</script>

<main>
  <header>
    {#if MOBILE}
      <button class="btn btn-quiet" onclick={() => (location.hash = `#/session/${session}`)}>
        <Icon name="arrowLeft" size={16} /> Zur Sitzung
      </button>
    {/if}
    <h1>Dateien</h1>
    {#if note}<span class="note" role="alert">{note}</span>{/if}
  </header>

  <div class="panes">
    {#if start !== null}
      <FilePane
        bind:this={local}
        bind:path={localDir}
        title="Dieser Computer"
        {start}
        list={api.localList}
        createDir={api.localCreateDir}
        rename={api.localRename}
        remove={api.localDelete}
        onTransfer={(entries) => upload(entries)}
      />
    {:else}
      <div></div>
    {/if}

    <div class="actions">
      <button class="btn btn-primary" disabled={!canUpload} onclick={() => upload()} title="Auswahl links in den aktuellen Ordner rechts laden">
        Hochladen <Icon name="arrowRight" size={16} />
      </button>
      <button class="btn btn-quiet" disabled={!canDownload} onclick={() => download()} title="Auswahl rechts in den aktuellen Ordner links laden">
        <Icon name="arrowLeft" size={16} /> Herunterladen
      </button>
    </div>

    <FilePane
      bind:this={remote}
      bind:path={remoteDir}
      bind:el={remoteEl}
      title="Ferngesteuertes Gerät"
      start=""
      list={remoteApi.list}
      createDir={remoteApi.createDir}
      rename={remoteApi.rename}
      remove={remoteApi.remove}
      locked={ended ? ENDED + " Dateien können nicht mehr übertragen werden." : null}
      {dropActive}
      onTransfer={(entries) => download(entries)}
    />
  </div>

  {#if transfers.length > 0}
    <section class="transfers">
      <div class="bar">
        <button class="toggle" onclick={() => (collapsed = !collapsed)} aria-expanded={!collapsed}>
          <span class="chev" class:open={!collapsed}><Icon name="chevron" size={16} /></span>
          Übertragungen
          <span class="count">{running > 0 ? `${running} aktiv` : transfers.length}</span>
        </button>
        <button class="tool" disabled={running === transfers.length} onclick={clearDone}>Erledigte entfernen</button>
      </div>
      {#if !collapsed}
        <ul>
          {#each transfers as t (t.id)}
            <li>
              <span class="dir"><Icon name={t.up ? "upload" : "download"} size={17} /></span>
              <span class="tname" title={t.name}>{t.name}</span>
              <div
                class="track"
                role="progressbar"
                aria-valuemin="0"
                aria-valuemax="100"
                aria-valuenow={Math.round(percent(t))}
              >
                <div class="fill" class:bad={t.status === "failed" || t.status === "cancelled"} style:width="{percent(t)}%"></div>
              </div>
              <span class="status" class:err={t.status === "failed"}>
                {#if t.status === "failed"}
                  {t.message || "Fehlgeschlagen"}
                {:else if t.status === "cancelled"}
                  Abgebrochen
                {:else if t.status === "done"}
                  Fertig
                {:else if t.total > 0}
                  {formatProgress(t.done, t.total)}
                {:else}
                  Wird übertragen …
                {/if}
              </span>
              <span class="end">
                {#if t.status === "running"}
                  <button class="tool" onclick={() => cancel(t)}>Abbrechen</button>
                {:else if t.status === "done" && !t.up && t.saved}
                  <button class="tool" onclick={() => api.reveal(t.saved!).catch((e) => (note = errorText(e)))}>Im Ordner zeigen</button>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {/if}
</main>

<style>
  :global(body.files) {
    overflow: hidden;
  }

  main {
    display: flex;
    flex-direction: column;
    gap: 12px;
    height: 100%;
    padding: 14px 16px 16px;
  }

  header {
    display: flex;
    align-items: baseline;
    gap: 16px;
  }

  h1 {
    margin: 0;
    font: 600 18px var(--font-display);
  }

  .note {
    color: var(--bad);
    font-size: 12.5px;
  }

  .panes {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto minmax(0, 1fr);
    gap: 12px;
    flex: 1;
    min-height: 0;
  }

  .actions {
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 10px;
  }

  .actions .btn {
    justify-content: space-between;
    width: 168px;
  }

  .actions .btn :global(svg) {
    flex: none;
  }

  .transfers {
    flex: none;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
  }

  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 8px 4px 4px;
  }

  .toggle,
  .tool {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 8px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--ink-2);
    font-size: 12.5px;
  }

  .toggle {
    font-weight: 600;
    color: var(--ink);
  }

  .toggle:hover,
  .tool:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ink) 7%, transparent);
  }

  .tool:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .chev {
    display: inline-grid;
    transition: transform 120ms var(--ease);
  }

  .chev.open {
    transform: rotate(90deg);
  }

  .count {
    color: var(--ink-3);
    font-weight: 400;
  }

  ul {
    max-height: 190px;
    margin: 0;
    padding: 0;
    border-top: 1px solid var(--line);
    list-style: none;
    overflow-y: auto;
  }

  li {
    display: grid;
    grid-template-columns: 20px minmax(120px, 1fr) minmax(120px, 1.2fr) minmax(130px, 200px) 120px;
    align-items: center;
    gap: 12px;
    min-height: 38px;
    padding: 0 12px;
    font-size: 13px;
  }

  li + li {
    border-top: 1px solid var(--line);
  }

  .dir {
    display: grid;
    color: var(--ink-3);
  }

  .tname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .track {
    height: 6px;
    border: 1px solid var(--line-strong);
    border-radius: 4px;
    overflow: hidden;
  }

  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 160ms var(--ease);
  }

  .fill.bad {
    background: var(--ink-3);
  }

  .status {
    overflow: hidden;
    color: var(--ink-3);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .status.err {
    color: var(--bad);
  }

  .end {
    display: flex;
    justify-content: flex-end;
    white-space: nowrap;
  }
</style>
