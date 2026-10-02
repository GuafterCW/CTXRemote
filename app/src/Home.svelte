<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import {
    api,
    errorText,
    peerLabel,
    type HostEvent,
    type Hosted,
    type Overview,
    type Peer,
    type Presence,
  } from "./lib/api";
  import { formatId, isCompleteId } from "./lib/format";
  import Icon from "./lib/Icon.svelte";
  import PeerList from "./PeerList.svelte";
  import Settings from "./Settings.svelte";

  let overview = $state<Overview | null>(null);
  let presence = $state<Presence>({ state: "connecting" });
  let hosted = $state<Hosted[]>([]);
  let revealed = $state(false);
  let copied = $state<"id" | "password" | null>(null);
  let settingsOpen = $state(false);

  // Connect flow: ID or alias → password → connecting.
  let query = $state("");
  let queryFocused = $state(false);
  let highlighted = $state(0);
  let target = $state<{ id: string; label: string | null }>({ id: "", label: null });
  let step = $state<"id" | "password">("id");
  let password = $state("");
  let connecting = $state(false);
  let error = $state("");
  let passwordInput = $state<HTMLInputElement>();

  const myId = $derived(presence.state === "online" ? presence.id : null);

  async function refresh() {
    overview = await api.overview();
    presence = overview.presence;
    hosted = overview.hosted;
  }

  onMount(() => {
    refresh();
    const subscriptions = [
      listen<Presence>("presence", (e) => (presence = e.payload)),
      listen<HostEvent>("host-event", (e) => {
        const event = e.payload;
        if (event.kind === "sessionStarted") hosted = [...hosted, { session: event.session, peer: event.peer }];
        if (event.kind === "sessionEnded") hosted = hosted.filter((h) => h.session !== event.session);
        if (event.kind === "passwordChanged") refresh();
      }),
    ];
    return () => subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
  });

  async function copy(kind: "id" | "password", text: string) {
    await navigator.clipboard.writeText(text.replace(/\s/g, ""));
    copied = kind;
    setTimeout(() => (copied = copied === kind ? null : copied), 1400);
  }

  async function newPassword() {
    const pw = await api.refreshPassword();
    if (overview) overview.password = pw;
  }

  const peers = $derived(overview?.peers ?? []);
  // Digits, spaces and dashes only: the user is typing an ID, otherwise an alias.
  const typingId = $derived(!/[^\d\s-]/.test(query));
  const suggestions = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const digits = q.replace(/\D/g, "");
    return peers
      .filter((p) =>
        typingId
          ? digits && p.id.replace(/\D/g, "").startsWith(digits)
          : (p.alias ?? "").toLowerCase().includes(q) || p.name.toLowerCase().includes(q),
      )
      .slice(0, 5);
  });
  const showSuggestions = $derived(
    queryFocused && suggestions.length > 0 && !(typingId && isCompleteId(query)),
  );
  const canSubmit = $derived(
    typingId ? isCompleteId(query) || suggestions.length === 1 : suggestions.length > 0,
  );

  function onQuery(value: string) {
    query = /[^\d\s-]/.test(value) ? value : formatId(value);
    highlighted = 0;
    error = "";
  }

  function choose(peer: Peer) {
    target = { id: peer.id, label: peer.alias || peer.name ? peerLabel(peer) : null };
    query = "";
    error = "";
    password = "";
    step = "password";
    queueMicrotask(() => passwordInput?.focus());
  }

  function submitQuery() {
    if (showSuggestions) return choose(suggestions[highlighted] ?? suggestions[0]);
    if (typingId && isCompleteId(query)) {
      const known = peers.find((p) => p.id === query);
      return choose(known ?? { id: query, alias: null, name: "", lastSeen: 0 });
    }
    if (suggestions.length === 1) return choose(suggestions[0]);
  }

  function onQueryKey(e: KeyboardEvent) {
    if (!showSuggestions) return;
    if (e.key === "ArrowDown") highlighted = (highlighted + 1) % suggestions.length;
    else if (e.key === "ArrowUp") highlighted = (highlighted - 1 + suggestions.length) % suggestions.length;
    else if (e.key === "Escape") queryFocused = false;
    else return;
    e.preventDefault();
  }

  async function connect() {
    if (!password || connecting) return;
    connecting = true;
    error = "";
    try {
      await api.connect(target.id, password);
      step = "id";
      password = "";
      refresh();
    } catch (e) {
      error = errorText(e);
    } finally {
      connecting = false;
    }
  }

  function back() {
    step = "id";
    error = "";
  }

  const statusText = $derived(
    presence.state === "online"
      ? "Bereit"
      : presence.state === "connecting"
        ? "Verbinde mit Server …"
        : "Server nicht erreichbar",
  );
</script>

<div class="shell">
  {#if hosted.length > 0}
    <div class="banner">
      {#each hosted as h (h.session)}
        <div class="banner-row">
          <span class="live"></span>
          <span><strong>{h.peer}</strong> steuert dieses Gerät</span>
          <button class="banner-btn" onclick={() => api.endHostedSession(h.session)}>Trennen</button>
        </div>
      {/each}
    </div>
  {/if}

  <header>
    <div class="brand">
      <svg width="20" height="20" viewBox="0 0 20 20" aria-hidden="true">
        <rect x="1.5" y="3.5" width="13" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" />
        <rect x="6.5" y="7.5" width="12" height="9" rx="2" fill="var(--accent)" />
      </svg>
      <span>CTXRemote</span>
    </div>
    <div class="header-end">
      <span class="status" title={presence.state === "offline" ? presence.reason : ""}>
        <span class="dot" data-state={presence.state}></span>
        {statusText}
      </span>
      <button class="icon-btn" title="Einstellungen" onclick={() => (settingsOpen = true)}>
        <Icon name="sliders" />
      </button>
    </div>
  </header>

  <main>
    <section class="this-device">
      <div class="label">Dieses Gerät</div>

      <div class="id-block">
        <div class="id" class:placeholder={!myId}>{myId ?? "––– ––– –––"}</div>
        <div class="id-meta">
          <span>Ihre ID</span>
          {#if myId}
            <button class="icon-btn small" title="ID kopieren" onclick={() => copy("id", myId!)}>
              <Icon name={copied === "id" ? "check" : "copy"} size={16} />
            </button>
          {/if}
        </div>
      </div>

      {#if overview && !overview.hostSupported}
        <p class="unsupported">
          Von diesem Gerät aus können Sie andere steuern. Ferngesteuert werden kann es mit diesem
          Betriebssystem noch nicht.
        </p>
      {:else}
      <div class="secret">
        <div class="secret-head">Einmal-Passwort</div>
        <div class="secret-row">
          <span class="password" class:masked={!revealed}>
            {overview ? (revealed ? overview.password : "•".repeat(overview.password.length)) : ""}
          </span>
          <div class="secret-actions">
            <button class="icon-btn small" title={revealed ? "Verbergen" : "Anzeigen"} onclick={() => (revealed = !revealed)}>
              <Icon name={revealed ? "eyeOff" : "eye"} size={16} />
            </button>
            <button class="icon-btn small" title="Kopieren" onclick={() => overview && copy("password", overview.password)}>
              <Icon name={copied === "password" ? "check" : "copy"} size={16} />
            </button>
            <button class="icon-btn small" title="Neues Passwort" onclick={newPassword}>
              <Icon name="refresh" size={16} />
            </button>
          </div>
        </div>
        <p class="hint">Gilt für eine Sitzung und wird danach automatisch erneuert.</p>
      </div>

      <button class="unattended" onclick={() => (settingsOpen = true)}>
        <span>Unbeaufsichtigter Zugriff</span>
        <span class="unattended-state" class:on={overview?.unattended}>
          {overview?.unattended ? "Aktiv" : "Aus"}
          <Icon name="chevron" size={14} />
        </span>
      </button>
      {/if}
    </section>

    <section class="remote">
      <div class="label">Fernsteuern</div>

      {#if step === "id"}
        <form
          class="connect"
          onsubmit={(e) => {
            e.preventDefault();
            submitQuery();
          }}
        >
          <div class="query">
            <input
              class="field id-field"
              class:text={!typingId}
              placeholder="ID oder Alias"
              autocomplete="off"
              spellcheck="false"
              value={query}
              oninput={(e) => onQuery(e.currentTarget.value)}
              onfocus={() => (queryFocused = true)}
              onblur={() => (queryFocused = false)}
              onkeydown={onQueryKey}
            />
            {#if showSuggestions}
              <ul class="suggestions" role="listbox">
                {#each suggestions as peer, i (peer.id)}
                  <li role="option" aria-selected={i === highlighted}>
                    <button
                      type="button"
                      class:active={i === highlighted}
                      onmousedown={(e) => e.preventDefault()}
                      onmouseenter={() => (highlighted = i)}
                      onclick={() => choose(peer)}
                    >
                      <span class="s-name">{peerLabel(peer)}</span>
                      <span class="s-id">{peer.id}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
          <button class="btn btn-primary" disabled={!canSubmit}>
            Verbinden <Icon name="arrowRight" size={16} />
          </button>
        </form>
      {:else}
        <form
          class="connect"
          onsubmit={(e) => {
            e.preventDefault();
            connect();
          }}
        >
          <div class="to">
            <button type="button" class="icon-btn small" title="Zurück" onclick={back} disabled={connecting}>
              <Icon name="arrowLeft" size={16} />
            </button>
            <span class="to-text">
              <span>Passwort für <strong>{target.label ?? target.id}</strong></span>
              {#if target.label}<span class="to-id">{target.id}</span>{/if}
            </span>
          </div>
          <input
            bind:this={passwordInput}
            bind:value={password}
            class="field"
            type="password"
            placeholder="Passwort"
            autocomplete="off"
            disabled={connecting}
            onkeydown={(e) => e.key === "Escape" && back()}
          />
          <button class="btn btn-primary" disabled={!password || connecting}>
            {#if connecting}
              <span class="spinner"></span> Verbinde …
            {:else}
              Verbinden <Icon name="arrowRight" size={16} />
            {/if}
          </button>
        </form>
      {/if}

      {#if error}
        <p class="error">{error}</p>
      {/if}

      <div class="recent">
        <PeerList {peers} onpick={choose} onchange={refresh} />
      </div>
    </section>
  </main>
</div>

{#if settingsOpen && overview}
  <Settings
    server={overview.server}
    unattended={overview.unattended}
    version={overview.version}
    onclose={() => {
      settingsOpen = false;
      refresh();
    }}
  />
{/if}

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 52px;
    padding: 0 16px 0 24px;
    border-bottom: 1px solid var(--line);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .header-end {
    display: flex;
    align-items: center;
    gap: 14px;
  }

  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink-2);
    font-size: 13px;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ok);
  }

  .dot[data-state="connecting"] {
    background: var(--warn);
    animation: pulse 1.4s ease-in-out infinite;
  }

  .dot[data-state="offline"] {
    background: var(--bad);
  }

  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  main {
    flex: 1;
    display: grid;
    grid-template-columns: minmax(320px, 5fr) 6fr;
    min-height: 0;
  }

  section {
    padding: 28px 32px;
    min-height: 0;
  }

  .this-device {
    display: flex;
    flex-direction: column;
    gap: 28px;
    background: var(--surface);
    border-right: 1px solid var(--line);
  }

  .id-block {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-top: -12px;
  }

  .id {
    font: 600 38px/1.15 var(--font-display);
    letter-spacing: 0.01em;
    font-variant-numeric: tabular-nums;
    user-select: text;
  }

  .id.placeholder {
    color: var(--line-strong);
  }

  .id-meta {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--ink-3);
    font-size: 13px;
    height: 28px;
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
  }

  .secret {
    border-top: 1px solid var(--line);
    padding-top: 20px;
  }

  .secret-head {
    color: var(--ink-2);
    font-size: 13px;
  }

  .secret-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 4px;
  }

  .password {
    font: 600 22px/1.3 var(--font-display);
    letter-spacing: 0.06em;
    user-select: text;
  }

  .password.masked {
    letter-spacing: 0.18em;
    color: var(--ink-2);
  }

  .secret-actions {
    display: flex;
    gap: 2px;
  }

  .hint {
    margin: 8px 0 0;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  .unattended {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-top: auto;
    padding: 14px 0 0;
    border: 0;
    border-top: 1px solid var(--line);
    background: none;
    color: var(--ink-2);
    font-size: 13px;
    text-align: left;
  }

  .unattended:hover {
    color: var(--ink);
  }

  .unattended-state {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--ink-3);
  }

  .unattended-state.on {
    color: var(--accent);
    font-weight: 600;
  }

  .remote {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .connect {
    display: grid;
    gap: 10px;
    max-width: 420px;
  }

  .id-field {
    height: 52px;
    font: 600 22px var(--font-display);
    letter-spacing: 0.04em;
    font-variant-numeric: tabular-nums;
  }

  .id-field::placeholder {
    font: 400 15px var(--font);
    letter-spacing: 0;
  }

  .to {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 32px;
    margin-left: -6px;
    color: var(--ink-2);
  }

  .to strong {
    color: var(--ink);
    font-variant-numeric: tabular-nums;
  }

  .error {
    margin: 0;
    max-width: 420px;
    color: var(--bad);
    font-size: 13px;
  }

  .recent {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 16px;
    min-height: 0;
  }

  .unsupported {
    margin: 0;
    padding-top: 20px;
    border-top: 1px solid var(--line);
    color: var(--ink-2);
    font-size: 13px;
  }

  .query {
    position: relative;
  }

  .id-field.text {
    font: 600 17px var(--font);
    letter-spacing: 0;
  }

  .suggestions {
    position: absolute;
    z-index: 10;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    margin: 0;
    padding: 4px;
    list-style: none;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--raised);
    box-shadow: 0 8px 24px color-mix(in srgb, #000 12%, transparent);
  }

  .suggestions button {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-radius: 6px;
    background: none;
    text-align: left;
  }

  .suggestions button.active {
    background: var(--accent-soft);
  }

  .s-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .s-id,
  .to-id {
    color: var(--ink-3);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .to-text {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
  }

  .banner {
    background: var(--ink);
    color: var(--bg);
  }

  .banner-row {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 40px;
    padding: 0 16px 0 24px;
    font-size: 13px;
  }

  .banner-row + .banner-row {
    border-top: 1px solid color-mix(in srgb, var(--bg) 15%, transparent);
  }

  .live {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--bad);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--bad) 30%, transparent);
  }

  .banner-btn {
    margin-left: auto;
    height: 26px;
    padding: 0 12px;
    border: 1px solid color-mix(in srgb, var(--bg) 35%, transparent);
    border-radius: 6px;
    background: transparent;
    color: inherit;
    font-size: 12.5px;
    font-weight: 600;
  }

  .banner-btn:hover {
    background: color-mix(in srgb, var(--bg) 12%, transparent);
  }
</style>
