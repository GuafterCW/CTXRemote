<script lang="ts">
  import { api, canManageTeam, errorText, peerLabel, type Peer, type TeamDevice, type TeamInfo } from "./lib/api";
  import { formatId, isCompleteId, since } from "./lib/format";
  import Icon from "./lib/Icon.svelte";

  let {
    peers,
    team,
    onpick,
    onterminal,
    onchange,
  }: {
    peers: Peer[];
    team: TeamInfo | null;
    onpick: (peer: Peer) => void;
    onterminal: (peer: Peer) => void;
    onchange: () => void;
  } = $props();

  let editing = $state<string | null>(null);
  let draft = $state("");
  let adding = $state(false);
  let newId = $state("");
  let newAlias = $state("");
  let error = $state("");
  let notice = $state("");
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  async function wake(peer: Peer) {
    error = "";
    try {
      const via = await api.wakePeer(peer.id);
      const where =
        via === 0 ? "ins Netzwerk dieses Computers" : `hier und über ${via === 1 ? "ein Gerät" : `${via} Geräte`} des Kontos`;
      notice = `Weckpaket für ${peerLabel(peer)} ${where} gesendet. Bis das Gerät online ist, kann es eine Minute dauern.`;
      clearTimeout(noticeTimer);
      noticeTimer = setTimeout(() => (notice = ""), 8000);
    } catch (e) {
      error = errorText(e);
    }
  }

  // Groups: a filter over the list, and an editor per device.
  let group = $state<string | null>(null);
  let tagging = $state<string | null>(null);
  let tagDraft = $state("");
  const groups = $derived(
    [...new Set(peers.flatMap((p) => p.tags))].sort((a, b) => a.localeCompare(b, "de")),
  );
  const shown = $derived(group && groups.includes(group) ? peers.filter((p) => p.tags.includes(group!)) : peers);

  // The team's devices: their own view next to the groups.
  let teamView = $state(false);
  const inTeamView = $derived(teamView && team !== null);
  const manage = $derived(canManageTeam(team));
  const digits = (id: string) => id.replace(/\D/g, "");
  const inBook = (id: string) => team?.devices.some((d) => d.inBook && digits(d.id) === digits(id)) ?? false;

  // A team device as a list entry: it opens without a password if a member allowed that.
  const asPeer = (d: TeamDevice): Peer => ({
    id: d.id,
    alias: d.name || null,
    name: "",
    lastSeen: 0,
    access: d.open,
    wake: false,
    tags: [],
    online: null,
  });

  // The home screen hears about the result through `team-changed`.
  async function teamBook(action: () => Promise<unknown>) {
    error = "";
    try {
      await action();
    } catch (e) {
      error = errorText(e);
    }
  }

  function startTags(peer: Peer) {
    tagging = peer.id;
    tagDraft = peer.tags.join(", ");
    error = "";
  }

  async function saveTags(peer: Peer) {
    if (tagging !== peer.id) return;
    const tags = tagDraft.split(",").map((t) => t.trim()).filter(Boolean);
    if (tags.join("\n") === peer.tags.join("\n")) {
      tagging = null;
      return;
    }
    try {
      await api.setTags(peer.id, tags);
      tagging = null;
      error = "";
      onchange();
    } catch (e) {
      error = errorText(e);
    }
  }

  // Named devices first (alphabetically), then the history by recency.
  const sorted = $derived(
    [...shown].sort((a, b) =>
      a.alias && b.alias
        ? a.alias.localeCompare(b.alias, "de")
        : a.alias
          ? -1
          : b.alias
            ? 1
            : b.lastSeen - a.lastSeen,
    ),
  );

  function focusOnMount(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function startRename(peer: Peer) {
    editing = peer.id;
    draft = peer.alias ?? "";
    error = "";
  }

  async function saveAlias(peer: Peer) {
    if (editing !== peer.id) return;
    const alias = draft.trim() || null;
    if (alias === peer.alias) {
      editing = null;
      return;
    }
    try {
      await api.setAlias(peer.id, alias);
      editing = null;
      error = "";
      onchange();
    } catch (e) {
      error = errorText(e);
    }
  }

  async function forget(peer: Peer) {
    await api.forgetPeer(peer.id);
    onchange();
  }

  async function add(e: SubmitEvent) {
    e.preventDefault();
    try {
      await api.setAlias(newId, newAlias.trim());
      adding = false;
      newId = newAlias = error = "";
      onchange();
    } catch (err) {
      error = errorText(err);
    }
  }
</script>

<div class="head">
  <div class="label">Geräte</div>
  {#if !adding}
    <button class="add" onclick={() => ((adding = true), (error = ""))}>
      <Icon name="plus" size={14} /> Hinzufügen
    </button>
  {/if}
</div>

{#if adding}
  <form class="add-form" onsubmit={add}>
    <input
      class="field"
      placeholder="ID"
      inputmode="numeric"
      value={newId}
      oninput={(e) => (newId = formatId(e.currentTarget.value))}
      use:focusOnMount
    />
    <input class="field" placeholder="Alias, z. B. Büro-PC" bind:value={newAlias} maxlength="40" />
    <button class="btn btn-primary" disabled={!isCompleteId(newId) || !newAlias.trim()}>Speichern</button>
    <button type="button" class="icon-btn" title="Abbrechen" onclick={() => ((adding = false), (error = ""))}>
      <Icon name="close" size={16} />
    </button>
  </form>
{/if}

{#if error}
  <p class="error">{error}</p>
{:else if notice}
  <p class="notice">{notice}</p>
{/if}

{#if groups.length > 0 || team}
  <div class="groups" role="group" aria-label="Gruppen">
    <button class="chip" class:on={group === null && !inTeamView} onclick={() => ((group = null), (teamView = false))}>Alle</button>
    {#each groups as g (g)}
      <button
        class="chip"
        class:on={!inTeamView && group === g}
        onclick={() => ((teamView = false), (group = group === g ? null : g))}>{g}</button
      >
    {/each}
    {#if team}
      <button class="chip team" class:on={inTeamView} onclick={() => (teamView = !teamView)}>
        <Icon name="users" size={13} /> {team.name}
      </button>
    {/if}
  </div>
{/if}

{#if inTeamView && team}
  {#if team.devices.length > 0}
    <ul>
      {#each team.devices as device (device.id)}
        <li>
          <button class="row peer" onclick={() => onpick(asPeer(device))}>
            <span class="peer-icon"><Icon name="monitor" size={18} /></span>
            <span class="peer-text">
              <span class="peer-name">
                {device.name || "Unbenanntes Gerät"}{#if device.open}<span class="access">ohne Passwort</span>{/if}
              </span>
              <span class="peer-id">{device.id}</span>
              {#if device.open && device.openedBy}<span class="shared">freigegeben von {device.openedBy}</span>{/if}
            </span>
          </button>
          <div class="actions">
            <button class="icon-btn small" title="Nur Terminal öffnen, ohne Bildschirm" aria-label="Nur Terminal öffnen, ohne Bildschirm" onclick={() => onterminal(asPeer(device))}>
              <Icon name="terminal" size={15} />
            </button>
            {#if manage && device.inBook}
              <button
                class="icon-btn small"
                title="Aus Team-Adressbuch entfernen"
                aria-label="Aus Team-Adressbuch entfernen"
                onclick={() => teamBook(() => api.teamRemoveDevice(device.id))}
              >
                <Icon name="trash" size={15} />
              </button>
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="empty">
      Im Adressbuch von „{team.name}“ ist noch kein Gerät.{manage
        ? " Eigene Geräte nehmen Sie in der Liste „Alle“ mit dem Team-Symbol auf."
        : ""}
    </p>
  {/if}
{:else if sorted.length > 0}
  <ul>
    {#each sorted as peer (peer.id)}
      <li class:editing={editing === peer.id}>
        {#if editing === peer.id}
          <div class="row">
            <span class="peer-icon"><Icon name="monitor" size={18} /></span>
            <input
              class="field rename"
              bind:value={draft}
              maxlength="40"
              placeholder={peer.name || "Alias"}
              use:focusOnMount
              onblur={() => saveAlias(peer)}
              onkeydown={(e) => {
                if (e.key === "Enter") saveAlias(peer);
                if (e.key === "Escape") editing = null;
              }}
            />
            <span class="peer-id">{peer.id}</span>
          </div>
        {:else if tagging === peer.id}
          <div class="row">
            <span class="peer-icon"><Icon name="tag" size={18} /></span>
            <input
              class="field rename"
              bind:value={tagDraft}
              placeholder="Gruppen, mit Komma getrennt"
              use:focusOnMount
              onblur={() => saveTags(peer)}
              onkeydown={(e) => {
                if (e.key === "Enter") saveTags(peer);
                if (e.key === "Escape") tagging = null;
              }}
            />
            <span class="peer-id">{peerLabel(peer)}</span>
          </div>
        {:else}
          <button class="row peer" onclick={() => onpick(peer)}>
            <span class="peer-icon" class:named={peer.alias}>
              <Icon name="monitor" size={18} />
              {#if peer.online !== null}
                <span class="dot" class:up={peer.online} title={peer.online ? "Online" : "Offline"}></span>
              {/if}
            </span>
            <span class="peer-text">
              <span class="peer-name">
                {peerLabel(peer)}{#if peer.access}<span class="access">ohne Passwort</span>{/if}
              </span>
              <span class="peer-id">
                {peer.id}{#if peer.alias && peer.name}<span class="dim"> · {peer.name}</span>{/if}
                {#each peer.tags as t (t)}<span class="tag">{t}</span>{/each}
              </span>
            </span>
            <span class="peer-when">{peer.lastSeen ? since(peer.lastSeen) : "noch nie verbunden"}</span>
          </button>
          <div class="actions">
            <button class="icon-btn small" title="Nur Terminal öffnen, ohne Bildschirm" aria-label="Nur Terminal öffnen, ohne Bildschirm" onclick={() => onterminal(peer)}>
              <Icon name="terminal" size={15} />
            </button>
            {#if peer.wake}
              <button
                class="icon-btn small"
                title="Aufwecken (Wake-on-LAN, aus dem selben Netzwerk oder über ein eingeschaltetes Gerät des Kontos dort)"
                onclick={() => wake(peer)}
              >
                <Icon name="power" size={15} />
              </button>
            {/if}
            {#if manage && !inBook(peer.id)}
              <button
                class="icon-btn small"
                title="Ins Team-Adressbuch"
                aria-label="Ins Team-Adressbuch"
                onclick={() => teamBook(() => api.teamAddDevice(peer.id, peerLabel(peer)))}
              >
                <Icon name="users" size={15} />
              </button>
            {/if}
            <button class="icon-btn small" title="Gruppen" onclick={() => startTags(peer)}>
              <Icon name="tag" size={15} />
            </button>
            <button class="icon-btn small" title="Alias ändern" onclick={() => startRename(peer)}>
              <Icon name="pencil" size={15} />
            </button>
            <button class="icon-btn small" title="Aus der Liste entfernen" onclick={() => forget(peer)}>
              <Icon name="close" size={15} />
            </button>
          </div>
        {/if}
      </li>
    {/each}
  </ul>
{:else if !adding}
  <p class="empty">Geräte, mit denen Sie sich verbinden, erscheinen hier. Mit einem Alias finden Sie sie schneller wieder.</p>
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 24px;
  }

  .add {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 6px;
    margin-right: -6px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--ink-2);
    font-size: 12.5px;
    font-weight: 600;
  }

  .add:hover {
    background: color-mix(in srgb, var(--ink) 7%, transparent);
    color: var(--ink);
  }

  .add-form {
    display: grid;
    grid-template-columns: 9.5em 1fr auto auto;
    gap: 8px;
    align-items: center;
  }

  .add-form .field {
    height: 36px;
  }

  .add-form .btn {
    height: 36px;
    padding: 0 14px;
  }

  .error {
    margin: 0;
    color: var(--bad);
    font-size: 13px;
  }

  ul {
    margin: 0 0 0 -10px;
    padding: 0;
    list-style: none;
    overflow-y: auto;
  }

  li {
    position: relative;
    border-radius: var(--radius-sm);
  }

  li:hover:not(.editing) {
    background: var(--surface);
  }

  .row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 9px 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    text-align: left;
  }

  .peer-icon {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    color: var(--ink-2);
  }

  .peer-icon.named {
    border-color: color-mix(in srgb, var(--accent) 40%, var(--line));
    color: var(--accent);
  }

  .peer-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .peer-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .access {
    margin-left: 8px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11.5px;
    font-weight: 600;
  }

  .peer-id,
  .peer-when {
    color: var(--ink-3);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .dim {
    color: var(--ink-3);
  }

  .rename {
    height: 34px;
    font-weight: 600;
  }

  /* Row actions replace the timestamp on hover. */
  .actions {
    position: absolute;
    top: 50%;
    right: 8px;
    display: flex;
    gap: 2px;
    transform: translateY(-50%);
    opacity: 0;
    transition: opacity 120ms var(--ease);
  }

  li:hover .actions,
  .actions:focus-within {
    opacity: 1;
  }

  li:hover .peer-when {
    visibility: hidden;
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
  }

  .empty {
    margin: 0;
    color: var(--ink-3);
    font-size: 13px;
  }

  .notice {
    margin: 4px 0;
    color: var(--ink-2);
    font-size: 12.5px;
  }

  .groups {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 6px 0 8px;
  }

  .chip {
    height: 24px;
    padding: 0 10px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: transparent;
    color: var(--ink-2);
    font-size: 12px;
  }

  .chip.team {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  .shared {
    color: var(--ink-3);
    font-size: 11.5px;
  }

  .chip.on {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--ink);
  }

  .tag {
    margin-left: 6px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--ink-2);
    font-size: 11px;
  }

  .peer-icon {
    position: relative;
  }

  .dot {
    position: absolute;
    right: -2px;
    bottom: -2px;
    width: 8px;
    height: 8px;
    border: 2px solid var(--surface);
    border-radius: 50%;
    background: var(--line-strong);
  }

  .dot.up {
    background: var(--ok);
  }
</style>
