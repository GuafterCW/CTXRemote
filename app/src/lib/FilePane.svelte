<script lang="ts">
  import { onMount } from "svelte";
  import { errorText, type FileEntry, type Listing } from "./api";
  import { formatDate, formatSize } from "./format";
  import Icon, { type IconName } from "./Icon.svelte";

  let {
    title,
    start,
    list,
    createDir,
    rename,
    remove,
    locked = null,
    dropActive = false,
    onTransfer,
    onError,
    path = $bindable(""),
    el = $bindable(),
  }: {
    title: string;
    start: string;
    list: (path: string) => Promise<Listing>;
    createDir: (path: string) => Promise<void>;
    rename: (path: string, name: string) => Promise<void>;
    remove: (paths: string[]) => Promise<void>;
    /** Reason why the pane is unusable (e.g. ended session). */
    locked?: string | null;
    dropActive?: boolean;
    onTransfer: (entries: FileEntry[]) => void;
    onError?: (message: string) => void;
    path?: string;
    el?: HTMLElement;
  } = $props();

  let listing = $state<Listing | null>(null);
  let loading = $state(false);
  let error = $state("");
  let selection = $state<string[]>([]);
  let anchor = $state(-1);
  let creating = $state<{ name: string } | null>(null);
  let renaming = $state<{ path: string; name: string } | null>(null);
  let confirming = $state<FileEntry[] | null>(null);
  let seq = 0;

  const RANK = { Place: 0, Drive: 1, Dir: 2, File: 3 } as const;
  const ICONS: Record<FileEntry["kind"], IconName> = {
    File: "file",
    Dir: "folder",
    Drive: "drive",
    Place: "home",
  };

  const atTop = $derived(listing !== null && listing.path === "");
  const rows = $derived.by(() => {
    const entries = [...(listing?.entries ?? [])];
    entries.sort((a, b) => {
      const byKind = RANK[a.kind] - RANK[b.kind];
      if (byKind !== 0 || atTop) return byKind;
      return a.name.localeCompare(b.name, "de", { numeric: true, sensitivity: "base" });
    });
    return entries;
  });
  const selected = $derived(rows.filter((r) => selection.includes(r.path)));
  const transferable = $derived(selected.filter((r) => r.kind === "File" || r.kind === "Dir"));
  const editable = $derived(selected.length > 0 && transferable.length === selected.length);
  const disabled = $derived(locked !== null);

  function separator(p: string): string {
    return p.includes("\\") ? "\\" : "/";
  }

  function join(dir: string, name: string): string {
    const sep = separator(dir);
    return dir.endsWith(sep) ? dir + name : dir + sep + name;
  }

  const crumbs = $derived.by(() => {
    const p = listing?.path ?? "";
    if (!p) return [];
    const sep = separator(p);
    const parts = p.split(sep).filter(Boolean);
    const out: { name: string; path: string }[] = [];
    let acc = sep === "/" ? "/" : "";
    if (sep === "/") out.push({ name: "/", path: "/" });
    parts.forEach((part, i) => {
      acc = i === 0 && sep === "\\" ? part + "\\" : join(acc, part);
      out.push({ name: part, path: acc });
    });
    return out;
  });

  export function selectedEntries(): FileEntry[] {
    return transferable;
  }

  export function reload() {
    return go(listing?.path ?? path, true);
  }

  async function go(target: string, keepSelection = false) {
    const token = ++seq;
    loading = true;
    try {
      const next = await list(target);
      if (token !== seq) return;
      listing = next;
      path = next.path;
      error = "";
      selection = keepSelection ? selection.filter((s) => next.entries.some((e) => e.path === s)) : [];
      if (!keepSelection) anchor = -1;
    } catch (e) {
      if (token === seq) report(e);
    } finally {
      if (token === seq) loading = false;
    }
  }

  function report(e: unknown) {
    error = errorText(e);
    onError?.(error);
  }

  onMount(() => {
    go(start).then(() => {
      if (listing === null && start !== "") go("");
    });
  });

  function up() {
    if (listing && listing.parent !== null) go(listing.parent);
  }

  function open(entry: FileEntry) {
    if (disabled) return;
    if (entry.kind === "File") onTransfer([entry]);
    else go(entry.path);
  }

  function pick(e: MouseEvent, index: number) {
    const entry = rows[index];
    if (e.shiftKey && anchor >= 0) {
      const [a, b] = anchor < index ? [anchor, index] : [index, anchor];
      selection = rows.slice(a, b + 1).map((r) => r.path);
    } else if (e.ctrlKey || e.metaKey) {
      selection = selection.includes(entry.path)
        ? selection.filter((s) => s !== entry.path)
        : [...selection, entry.path];
      anchor = index;
    } else {
      selection = [entry.path];
      anchor = index;
    }
  }

  /** Keeps the end of a long path (the current folder) in view. */
  function toEnd(node: HTMLElement, _path: string | undefined) {
    node.scrollLeft = node.scrollWidth;
    return { update: () => (node.scrollLeft = node.scrollWidth) };
  }

  function focusSelect(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function startCreate() {
    if (disabled || !listing || atTop) return;
    renaming = null;
    creating = { name: "Neuer Ordner" };
  }

  async function commitCreate() {
    const draft = creating;
    creating = null;
    el?.focus();
    const name = draft?.name.trim();
    if (!draft || !name || !listing) return;
    try {
      const full = join(listing.path, name);
      await createDir(full);
      await go(listing.path);
      selection = [full];
    } catch (e) {
      report(e);
    }
  }

  function startRename() {
    if (disabled || selected.length !== 1 || !editable) return;
    creating = null;
    renaming = { path: selected[0].path, name: selected[0].name };
  }

  async function commitRename() {
    const draft = renaming;
    renaming = null;
    el?.focus();
    const name = draft?.name.trim();
    const entry = rows.find((r) => r.path === draft?.path);
    if (!draft || !name || !entry || name === entry.name) return;
    try {
      await rename(entry.path, name);
      await go(listing?.path ?? path);
    } catch (e) {
      report(e);
    }
  }

  function askDelete() {
    if (disabled || !editable) return;
    confirming = [...transferable];
  }

  async function doDelete() {
    const items = confirming;
    confirming = null;
    el?.focus();
    if (!items) return;
    try {
      await remove(items.map((i) => i.path));
    } catch (e) {
      report(e);
    }
    await go(listing?.path ?? path);
  }

  function onKey(e: KeyboardEvent) {
    if (disabled || e.target instanceof HTMLInputElement || confirming) return;
    const first = selected[0];
    if (e.key === "Backspace") up();
    else if (e.key === "Delete") askDelete();
    else if (e.key === "F2") startRename();
    else if (e.key === "Enter" && first) open(first);
    else if ((e.key === "a" || e.key === "A") && (e.ctrlKey || e.metaKey)) selection = rows.map((r) => r.path);
    else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (rows.length === 0) return;
      const current = rows.findIndex((r) => r.path === selection[selection.length - 1]);
      const next = Math.max(0, Math.min(rows.length - 1, current + (e.key === "ArrowDown" ? 1 : -1)));
      selection = [rows[next].path];
      anchor = next;
    } else return;
    e.preventDefault();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div role="group"
  class="pane"
  class:drop={dropActive}
  bind:this={el}
  tabindex="-1"
  aria-label={title}
  onkeydown={onKey}
  onmousedown={(e) => {
    if (!(e.target instanceof HTMLInputElement)) el?.focus();
  }}
>
  <header>
    <h2>{title}</h2>
    <div class="nav">
      <button class="icon-btn" title="Nach oben (Rücktaste)" disabled={disabled || !listing || listing.parent === null} onclick={up}>
        <Icon name="arrowUp" size={17} />
      </button>
      <button class="icon-btn" title="Oberste Ebene" disabled={disabled || atTop} onclick={() => go("")}>
        <Icon name="monitor" size={17} />
      </button>
      <div class="crumbs" title={listing?.path} use:toEnd={listing?.path}>
        {#if atTop}
          <span class="crumb current">Oberste Ebene</span>
        {:else}
          {#each crumbs as c, i (c.path)}
            {#if i > 0 && !(i === 1 && crumbs[0].name === "/")}<span class="slash">›</span>{/if}
            <button class="crumb" class:current={i === crumbs.length - 1} disabled={disabled} onclick={() => go(c.path)}>
              {c.name}
            </button>
          {/each}
        {/if}
      </div>
    </div>
    <div class="tools">
      <button class="tool" disabled={disabled || atTop || !listing} onclick={startCreate}>
        <Icon name="folderPlus" size={16} /> Neuer Ordner
      </button>
      <button class="tool" disabled={disabled || selected.length !== 1 || !editable} onclick={startRename}>
        <Icon name="pencil" size={16} /> Umbenennen
      </button>
      <button class="tool" disabled={disabled || !editable} onclick={askDelete}>
        <Icon name="trash" size={16} /> Löschen
      </button>
      <button class="tool" disabled={disabled || loading} onclick={() => reload()}>
        <Icon name="refresh" size={16} /> Aktualisieren
      </button>
    </div>
  </header>

  <div class="table" role="listbox" aria-multiselectable="true" aria-label={title}>
    <div class="row head" role="presentation">
      <span>Name</span><span class="num">Größe</span><span>Geändert</span>
    </div>
    <div class="body" role="presentation" onclick={() => (selection = [])}>
      {#if locked !== null}
        <p class="note">{locked}</p>
      {:else}
        {#if creating}
          <div class="row" role="presentation">
            <span class="name">
              <Icon name="folder" size={17} />
              <input
                class="inline"
                use:focusSelect
                bind:value={creating.name}
                onkeydown={(e) => {
                  e.stopPropagation();
                  if (e.key === "Enter") commitCreate();
                  else if (e.key === "Escape") {
                    creating = null;
                    el?.focus();
                  }
                }}
                onblur={() => (creating = null)}
              />
            </span><span></span><span></span>
          </div>
        {/if}
        {#each rows as entry, i (entry.path)}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row item"
            class:selected={selection.includes(entry.path)}
            role="option"
            tabindex="-1"
            aria-selected={selection.includes(entry.path)}
            onclick={(e) => {
              e.stopPropagation();
              pick(e, i);
            }}
            ondblclick={() => open(entry)}
          >
            <span class="name">
              <Icon name={ICONS[entry.kind]} size={17} />
              {#if renaming?.path === entry.path}
                <input
                  class="inline"
                  use:focusSelect
                  bind:value={renaming.name}
                  onclick={(e) => e.stopPropagation()}
                  ondblclick={(e) => e.stopPropagation()}
                  onkeydown={(e) => {
                    e.stopPropagation();
                    if (e.key === "Enter") commitRename();
                    else if (e.key === "Escape") {
                      renaming = null;
                      el?.focus();
                    }
                  }}
                  onblur={() => (renaming = null)}
                />
              {:else}
                <span class="text">{entry.name}</span>
              {/if}
            </span>
            <span class="num">{entry.kind === "File" ? formatSize(entry.size) : ""}</span>
            <span class="date">{formatDate(entry.modified)}</span>
          </div>
        {/each}
        {#if listing && rows.length === 0 && !creating}
          <p class="note">Dieser Ordner ist leer.</p>
        {/if}
      {/if}
    </div>
  </div>

  <footer>
    {#if error}
      <span class="error" role="alert">{error}</span>
    {:else if loading}
      <span class="hint">Wird geladen …</span>
    {:else if selected.length > 0}
      <span class="hint">{selected.length} ausgewählt</span>
    {:else if listing}
      <span class="hint">{rows.length === 1 ? "1 Eintrag" : `${rows.length} Einträge`}</span>
    {/if}
  </footer>

  {#if confirming}
    <div class="scrim" role="presentation">
      <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="del-{title}">
        <h3 id="del-{title}">
          {confirming.length === 1 ? `„${confirming[0].name}“ löschen?` : `${confirming.length} Einträge löschen?`}
        </h3>
        <p>Das kann nicht rückgängig gemacht werden.</p>
        <div class="actions">
          <button class="btn btn-quiet small" onclick={() => ((confirming = null), el?.focus())}>Abbrechen</button>
          <button class="btn btn-danger small" onclick={doDelete}>Löschen</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .pane {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    outline: none;
    overflow: hidden;
  }

  .pane.drop {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  header {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px 8px;
    border-bottom: 1px solid var(--line);
  }

  h2 {
    margin: 0;
    font: 600 11px var(--font);
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--ink-3);
  }

  .nav {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
  }

  .icon-btn:disabled,
  .tool:disabled,
  .crumb:disabled {
    opacity: 0.4;
    cursor: default;
  }

  .crumbs {
    display: flex;
    align-items: center;
    flex: 1;
    min-width: 0;
    margin-left: 6px;
    padding: 0 8px;
    height: 30px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--raised);
    overflow-x: auto;
    white-space: nowrap;
    scrollbar-width: none;
  }

  .crumb {
    padding: 2px 4px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--ink-2);
    font-size: 13px;
  }

  button.crumb:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ink) 7%, transparent);
    color: var(--ink);
  }

  .crumb.current {
    color: var(--ink);
    font-weight: 600;
  }

  .slash {
    color: var(--ink-3);
  }

  .tools {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
  }

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

  .tool:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ink) 7%, transparent);
    color: var(--ink);
  }

  .table {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 78px 128px;
    align-items: center;
    gap: 8px;
    min-height: 30px;
    padding: 0 12px;
    font-size: 13px;
  }

  .row.head {
    min-height: 26px;
    border-bottom: 1px solid var(--line);
    color: var(--ink-3);
    font-size: 11.5px;
  }

  .item.selected {
    background: var(--accent-soft);
  }

  .name {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    color: var(--ink-2);
  }

  .name > :global(svg) {
    flex: none;
  }

  /* Phones: no date column, rows big enough for a finger. */
  :global(body.mobile) .row {
    grid-template-columns: minmax(0, 1fr) 72px;
  }

  :global(body.mobile) .row > :nth-child(3) {
    display: none;
  }

  :global(body.mobile) .row.item {
    min-height: 44px;
  }

  .item.selected .name {
    color: var(--accent);
  }

  .text {
    overflow: hidden;
    color: var(--ink);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .num {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .num,
  .date {
    color: var(--ink-3);
    font-size: 12.5px;
    white-space: nowrap;
  }

  .inline {
    flex: 1;
    min-width: 0;
    height: 24px;
    padding: 0 6px;
    border: 1px solid var(--accent);
    border-radius: 5px;
    background: var(--raised);
    outline: none;
    user-select: text;
  }

  .note {
    margin: 0;
    padding: 28px 16px;
    color: var(--ink-3);
    text-align: center;
  }

  footer {
    min-height: 28px;
    padding: 5px 12px;
    border-top: 1px solid var(--line);
    font-size: 12px;
  }

  .hint {
    color: var(--ink-3);
  }

  .error {
    color: var(--bad);
  }

  .scrim {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--bg) 75%, transparent);
  }

  .dialog {
    width: min(300px, 90%);
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
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .small {
    height: 32px;
    padding: 0 14px;
  }

  .btn-danger {
    background: var(--bad);
    color: #fff;
  }
</style>
