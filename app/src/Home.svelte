<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import {
    api,
    CODE_NEEDED,
    errorText,
    hostedLabel,
    peerLabel,
    type HostEvent,
    type Hosted,
    type Overview,
    type Peer,
    type Presence,
    type TeamInfo,
  } from "./lib/api";
  import { formatId, isCompleteId, isPublicAlias } from "./lib/format";
  import { MOBILE } from "./lib/platform";
  import RightsMenu from "./lib/RightsMenu.svelte";
  import ChatPanel, { type ChatMessage } from "./lib/ChatPanel.svelte";
  import Icon from "./lib/Icon.svelte";
  import SupportQueue from "./lib/SupportQueue.svelte";
  import PeerList from "./PeerList.svelte";
  import Settings from "./Settings.svelte";
  import Account from "./Account.svelte";
  import History from "./History.svelte";

  let overview = $state<Overview | null>(null);
  let presence = $state<Presence>({ state: "connecting" });
  let hosted = $state<Hosted[]>([]);
  let revealed = $state(false);
  let copied = $state<"id" | "password" | "alias" | null>(null);
  let settingsOpen = $state(false);
  let accountOpen = $state(false);
  let historyOpen = $state(false);
  let chatOpen = $state<Record<number, boolean>>({});

  // Invitation link dialog (desktop only).
  let inviteOpen = $state(false);
  let inviteLink = $state("");
  let inviteError = $state("");
  let inviteBusy = $state(false);
  let inviteCopied = $state(false);

  async function openInvite() {
    if (!myId) return;
    inviteOpen = true;
    inviteLink = "";
    inviteError = "";
    inviteCopied = false;
    inviteBusy = true;
    try {
      inviteLink = await api.createInvite(myId);
    } catch (e) {
      inviteError = errorText(e);
    } finally {
      inviteBusy = false;
    }
  }

  async function copyInvite() {
    try {
      await navigator.clipboard.writeText(inviteLink);
      inviteCopied = true;
      setTimeout(() => (inviteCopied = false), 1400);
    } catch (e) {
      inviteError = errorText(e);
    }
  }

  const inviteMail = $derived(
    `mailto:?subject=${encodeURIComponent("Fernhilfe mit Remocto")}&body=${encodeURIComponent(
      `Guten Tag,\n\nbitte öffnen Sie diesen Link, damit ich Ihnen per Fernhilfe helfen kann:\n${inviteLink}\n`,
    )}`,
  );

  // Help requests sent by invited people; the password is kept but never shown.
  type HelpRequest = { id: string; password: string; name: string };
  let helpRequests = $state<HelpRequest[]>([]);

  function connectRequest(r: HelpRequest) {
    helpRequests = helpRequests.filter((x) => x.id !== r.id);
    target = { id: r.id, label: r.name || null };
    password = r.password;
    query = "";
    step = "password";
    codeAsked = false;
    code = "";
    error = "";
    connect();
  }

  // Chat history per session number; it goes away with the session.
  let chats = $state<Record<number, ChatMessage[]>>({});

  async function sendChat(session: number, text: string) {
    await api.hostChat(session, text);
    chats[session] = [...(chats[session] ?? []), { mine: true, text, at: Date.now() }];
  }

  // Connect flow: ID or alias → password → connecting.
  let query = $state("");
  let queryFocused = $state(false);
  let highlighted = $state(0);
  let target = $state<{ id: string; label: string | null }>({ id: "", label: null });
  let step = $state<"id" | "password">("id");
  let password = $state("");
  /** Shown once the host asked for its authenticator code (two-factor). */
  let codeAsked = $state(false);
  let code = $state("");
  let codeInput = $state<HTMLInputElement>();
  let connecting = $state(false);
  let error = $state("");
  let passwordInput = $state<HTMLInputElement>();

  const myId = $derived(presence.state === "online" ? presence.id : null);

  let updating = $state(false);
  let updateError = $state("");

  // The installer asks for administrator rights, then closes and restarts the app.
  async function installUpdate() {
    updating = true;
    updateError = "";
    try {
      await api.installUpdate();
    } catch (e) {
      updateError = errorText(e);
      updating = false;
    }
  }

  async function refresh() {
    overview = await api.overview();
    presence = overview.presence;
    // The overview does not know about open terminals; events do.
    hosted = overview.hosted.map((h) => {
      const known = hosted.find((o) => o.session === h.session);
      return { ...h, terminal: known?.terminal ?? false, terminalOnly: known?.terminalOnly ?? false };
    });
  }

  // The account's team, for its devices in the list; null without one.
  let team = $state<TeamInfo | null>(null);
  const loadTeam = () =>
    api
      .teamInfo()
      .then((t) => (team = t))
      .catch(() => {});

  /** macOS: what still has to be allowed before this Mac can be controlled. */
  let permissions = $state<{ screen: boolean; input: boolean } | null>(null);
  const checkPermissions = () =>
    api
      .hostPermissions()
      .then((p) => (permissions = p))
      .catch(() => {});

  async function askPermission(kind: "screen" | "input") {
    await api.requestHostPermission(kind).catch(() => {});
    setTimeout(checkPermissions, 1500);
  }

  onMount(() => {
    checkPermissions();
    // Coming back from System Settings: show the new state.
    const onFocus = () => checkPermissions();
    window.addEventListener("focus", onFocus);
    const unlistenFocus = () => window.removeEventListener("focus", onFocus);
    refresh();
    loadTeam();
    const subscriptions = [
      listen("team-changed", () => loadTeam()),
      listen<string>("update-available", () => refresh()),
      listen<Presence>("presence", (e) => {
        presence = e.payload;
        // The service reports its sessions with its state; resync after a reconnect.
        if (overview?.service) refresh();
      }),
      // The address book changed through the account's sync.
      listen("peers-changed", () => refresh()),
      listen<HelpRequest>("help-request", (e) => {
        helpRequests = [e.payload, ...helpRequests.filter((r) => r.id !== e.payload.id)];
      }),
      listen<HostEvent>("host-event", (e) => {
        const event = e.payload;
        if (event.kind === "sessionStarted")
          hosted = [
            ...hosted,
            {
              session: event.session,
              peer: event.peer,
              chat: event.chat,
              profile: event.profile ?? null,
              rights: event.rights ?? 0,
              privacy: false,
              recording: false,
            },
          ];
        if (event.kind === "recording")
          hosted = hosted.map((h) => (h.session === event.session ? { ...h, recording: event.on } : h));
        if (event.kind === "terminal")
          hosted = hosted.map((h) =>
            h.session === event.session ? { ...h, terminal: event.open, terminalOnly: event.only ?? false } : h,
          );
        if (event.kind === "rights")
          hosted = hosted.map((h) =>
            h.session === event.session ? { ...h, rights: event.rights, privacy: event.privacy } : h,
          );
        if (event.kind === "sessionEnded") {
          hosted = hosted.filter((h) => h.session !== event.session);
          delete chats[event.session];
          delete chatOpen[event.session];
        }
        if (event.kind === "chat") {
          chats[event.session] = [...(chats[event.session] ?? []), { mine: false, text: event.text, at: Date.now() }];
          chatOpen[event.session] = true;
        }
        if (event.kind === "passwordChanged") refresh();
      }),
    ];
    return () => {
      unlistenFocus();
      subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
    };
  });

  // Public alias: shown under the ID, edited in place.
  let aliasEditing = $state(false);
  let aliasDraft = $state("");
  let aliasSaving = $state(false);
  let aliasError = $state("");

  function editAlias() {
    aliasDraft = overview?.publicAlias ?? "";
    aliasError = "";
    aliasEditing = true;
  }

  async function saveAlias(remove = false) {
    aliasSaving = true;
    aliasError = "";
    try {
      await api.setPublicAlias(remove || !aliasDraft.trim() ? null : aliasDraft);
      aliasEditing = false;
      await refresh();
    } catch (e) {
      aliasError = errorText(e);
    } finally {
      aliasSaving = false;
    }
  }

  async function copy(kind: "id" | "password" | "alias", text: string) {
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
    typingId ? isCompleteId(query) || suggestions.length === 1 : suggestions.length > 0 || isPublicAlias(query),
  );

  function onQuery(value: string) {
    query = /[^\d\s-]/.test(value) ? value : formatId(value);
    highlighted = 0;
    error = "";
  }

  /** The next connection opens with the command line only (the "Terminal" action). */
  let terminalMode = $state(false);

  function choose(peer: Peer, terminal = false) {
    terminalMode = terminal;
    target = { id: peer.id, label: peer.alias || peer.name ? peerLabel(peer) : null };
    query = "";
    error = "";
    password = "";
    step = "password";
    if (peer.access) return connectWithAccount();
    queueMicrotask(() => passwordInput?.focus());
  }

  // Passwordless attempt via the account; falls back to the password step.
  async function connectWithAccount() {
    connecting = true;
    try {
      await api.connect(target.id, "", undefined, terminalMode);
      step = "id";
      refresh();
    } catch (e) {
      error = `Ohne Passwort ging es nicht (${errorText(e)}). Bitte das Passwort eingeben.`;
      queueMicrotask(() => passwordInput?.focus());
    } finally {
      connecting = false;
    }
  }

  function submitQuery(terminal = false) {
    if (showSuggestions) return choose(suggestions[highlighted] ?? suggestions[0], terminal);
    if (typingId && isCompleteId(query)) {
      const known = peers.find((p) => p.id === query);
      return choose(known ?? { id: query, alias: null, name: "", lastSeen: 0, access: false, wake: false, tags: [], online: null }, terminal);
    }
    if (suggestions.length === 1) return choose(suggestions[0], terminal);
    // Not in the own list: a public alias, which the server resolves.
    if (!typingId && isPublicAlias(query)) {
      const alias = query.trim().toLowerCase();
      return choose({ id: alias, alias, name: "", lastSeen: 0, access: false, wake: false, tags: [], online: null }, terminal);
    }
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
    if (!password || connecting || (codeAsked && code.trim().length < 6)) return;
    connecting = true;
    error = "";
    try {
      await api.connect(target.id, password, codeAsked ? code : undefined, terminalMode);
      step = "id";
      password = "";
      code = "";
      codeAsked = false;
      refresh();
    } catch (e) {
      const text = errorText(e);
      if (text === CODE_NEEDED) {
        codeAsked = true;
        queueMicrotask(() => codeInput?.focus());
      } else {
        error = text;
        code = "";
      }
    } finally {
      connecting = false;
    }
  }

  function back() {
    step = "id";
    error = "";
    codeAsked = false;
    code = "";
  }

  const statusText = $derived(
    presence.state === "online"
      ? "Bereit"
      : presence.state === "connecting"
        ? "Verbinde mit Server …"
        : presence.reason.startsWith("Remocto-Dienst")
          ? "Dienst nicht erreichbar"
          : "Server nicht erreichbar",
  );
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && inviteOpen && (inviteOpen = false)} />

<div class="shell">
  {#if hosted.length > 0}
    <div class="banner">
      {#each hosted as h (h.session)}
        <div class="banner-row">
          <span class="live"></span>
          {#if h.profile?.logo}
            <img class="banner-logo" src={`data:image/png;base64,${h.profile.logo}`} alt="" />
          {/if}
          <span title={h.profile ? `Gerät: ${h.peer}` : undefined}><strong>{hostedLabel(h)}</strong>
            {h.terminalOnly ? "nutzt das Terminal auf diesem Gerät (ohne Bildschirm)" : "steuert dieses Gerät"}</span>
          {#if h.chat}
            <button class="banner-btn chat-btn" onclick={() => (chatOpen[h.session] = !chatOpen[h.session])}>
              <Icon name="chat" size={14} /> Chat
            </button>
          {:else}
            <span class="no-chat" title="Gegenstelle hat eine ältere Version ohne Chat">Kein Chat</span>
          {/if}
          {#if h.recording}<span class="private-tag" title="Die Gegenseite zeichnet die Sitzung als Video auf">● Aufnahme</span>{/if}
          {#if h.terminal}
            <span class="terminal-tag" title="Die Gegenseite hat eine Befehlszeile auf diesem Gerät offen"><Icon name="terminal" size={14} /> Terminal offen</span>
          {/if}
          {#if h.privacy}<span class="private-tag" title="Der Bildschirm hier ist für die Gegenseite schwarz">Privat</span>{/if}
          <RightsMenu hosted={h} class="banner-rights" />
          <button class="banner-btn" onclick={() => api.endHostedSession(h.session)}>Trennen</button>
        </div>
      {/each}
    </div>
    {#each hosted as h (h.session)}
      {#if h.chat && chatOpen[h.session]}
        <div class="chat-wrap">
          <ChatPanel messages={chats[h.session] ?? []} onsend={(text) => sendChat(h.session, text)} />
        </div>
      {/if}
    {/each}
  {/if}

  <header>
    <div class="brand">
      <svg width="20" height="20" viewBox="250 220 640 640" aria-hidden="true"><path d="M330 760 V430 Q330 300 470 300" fill="none" stroke="currentColor" stroke-width="130" stroke-linecap="round"/><path d="M560 300 L560 700 L652 610 L716 748 L788 716 L724 580 L852 580 Z" fill="var(--accent)" stroke="var(--accent)" stroke-width="28" stroke-linejoin="round"/></svg>
      <span>Remocto</span>
    </div>
    <div class="header-end">
      {#if overview?.update}
        <button
          class="update"
          disabled={updating}
          title={updateError || `Version ${overview.update} herunterladen und installieren`}
          onclick={installUpdate}
        >
          {updating ? "Wird geladen …" : `Update auf ${overview.update}`}
        </button>
      {/if}
      <span class="status" title={presence.state === "offline" ? presence.reason : ""}>
        <span class="dot" data-state={presence.state}></span>
        {statusText}
      </span>
      <button class="icon-btn" title="Verlauf" aria-label="Verlauf" onclick={() => (historyOpen = true)}>
        <Icon name="history" />
      </button>
      {#if overview?.aliasSupported}
        <button class="icon-btn" title="Konto" onclick={() => (accountOpen = true)}>
          <Icon name="user" />
        </button>
      {/if}
      <button class="icon-btn" title="Einstellungen" onclick={() => (settingsOpen = true)}>
        <Icon name="sliders" />
      </button>
    </div>
  </header>

  {#if !MOBILE && helpRequests.length > 0}
    <div class="requests">
      {#each helpRequests as r (r.id)}
        <div class="request">
          <div class="request-text">
            <strong>{r.name || "Jemand"} bittet um Hilfe</strong>
            <span class="request-id">ID {r.id}</span>
          </div>
          <button class="btn btn-quiet" type="button" onclick={() => (helpRequests = helpRequests.filter((x) => x.id !== r.id))}>
            Ablehnen
          </button>
          <button class="btn btn-primary" type="button" disabled={connecting} onclick={() => connectRequest(r)}>
            Verbinden
          </button>
        </div>
      {/each}
    </div>
  {/if}

  {#if !MOBILE}
    <SupportQueue {team} onclaim={connectRequest} />
  {/if}

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
        {#if overview?.aliasSupported && myId}
          {#if aliasEditing}
            <form
              class="alias-edit"
              onsubmit={(e) => {
                e.preventDefault();
                saveAlias();
              }}
            >
              <input
                class="field"
                bind:value={aliasDraft}
                placeholder="z. B. philipp-pc"
                maxlength="32"
                spellcheck="false"
                autocomplete="off"
                disabled={aliasSaving}
              />
              <button class="btn btn-primary" type="submit" disabled={aliasSaving}>Speichern</button>
              <button class="btn btn-quiet" type="button" disabled={aliasSaving} onclick={() => (aliasEditing = false)}>
                Abbrechen
              </button>
            </form>
            <p class="alias-note" class:bad={!!aliasError}>
              {aliasError ||
                "Andere können sich mit diesem Namen statt mit der ID verbinden. 3–32 Zeichen: Kleinbuchstaben, Ziffern, Punkt, Binde- und Unterstrich."}
            </p>
            {#if overview.publicAlias}
              <button class="link-btn" type="button" disabled={aliasSaving} onclick={() => saveAlias(true)}>
                Alias entfernen
              </button>
            {/if}
          {:else if overview.publicAlias}
            <div class="id-meta">
              <span>oder</span>
              <span class="alias">{overview.publicAlias}</span>
              <button class="icon-btn small" title="Alias kopieren" onclick={() => copy("alias", overview!.publicAlias!)}>
                <Icon name={copied === "alias" ? "check" : "copy"} size={16} />
              </button>
              <button class="icon-btn small" title="Alias ändern" onclick={editAlias}>
                <Icon name="pencil" size={15} />
              </button>
            </div>
          {:else}
            <button class="link-btn" type="button" onclick={editAlias}>
              <Icon name="plus" size={14} /> Alias festlegen
            </button>
          {/if}
        {/if}
        {#if !MOBILE && myId}
          <button class="link-btn" type="button" onclick={openInvite}>Einladungslink</button>
        {/if}
      </div>

      {#if permissions && (!permissions.screen || !permissions.input)}
        <div class="permissions">
          <strong>Damit dieser Mac ferngesteuert werden kann</strong>
          <p>macOS verlangt zwei Freigaben. Nach der Freigabe der Bildschirmaufnahme Remocto einmal neu starten.</p>
          <div class="permission-row">
            <span class:ok={permissions.screen}>{permissions.screen ? "✓" : "•"} Bildschirmaufnahme</span>
            {#if !permissions.screen}<button class="link-btn" onclick={() => askPermission("screen")}>Freigeben …</button>{/if}
          </div>
          <div class="permission-row">
            <span class:ok={permissions.input}>{permissions.input ? "✓" : "•"} Bedienungshilfen (Maus und Tastatur)</span>
            {#if !permissions.input}<button class="link-btn" onclick={() => askPermission("input")}>Freigeben …</button>{/if}
          </div>
        </div>
      {/if}

      {#if overview && !overview.hostSupported}
        <p class="unsupported">
          Von diesem Gerät aus können Sie andere steuern. Ferngesteuert werden kann es mit diesem
          Betriebssystem noch nicht.
        </p>
      {:else}
      {#if overview?.hostLimitation}
        <p class="unsupported">{overview.hostLimitation}</p>
      {/if}
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
                      {#if peer.access}<span class="s-access">ohne Passwort</span>{/if}
                      <span class="s-id">{peer.id}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
          <div class="connect-actions">
            <button class="btn btn-primary" disabled={!canSubmit}>
              Verbinden <Icon name="arrowRight" size={16} />
            </button>
            <button
              type="button"
              class="btn terminal-btn"
              title="Nur Terminal öffnen, ohne Bildschirm"
              aria-label="Nur Terminal öffnen, ohne Bildschirm"
              disabled={!canSubmit}
              onclick={() => submitQuery(true)}
            >
              <Icon name="terminal" size={18} />
            </button>
          </div>
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
              <span>{terminalMode ? "Terminal: Passwort für" : "Passwort für"} <strong>{target.label ?? target.id}</strong></span>
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
          {#if codeAsked}
            <input
              bind:this={codeInput}
              bind:value={code}
              class="field"
              inputmode="numeric"
              maxlength="7"
              placeholder="Code aus der Authenticator-App"
              autocomplete="one-time-code"
              disabled={connecting}
              onkeydown={(e) => e.key === "Escape" && back()}
            />
          {/if}
          <button class="btn btn-primary" disabled={!password || connecting || (codeAsked && code.trim().length < 6)}>
            {#if connecting}
              <span class="spinner"></span> Verbinde …
            {:else if terminalMode}
              <Icon name="terminal" size={16} /> Terminal öffnen
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
        <PeerList {peers} {team} onpick={(peer) => choose(peer)} onterminal={(peer) => choose(peer, true)} onchange={refresh} />
      </div>
    </section>
  </main>
</div>

{#if inviteOpen}
  <div class="scrim" role="presentation" onclick={() => (inviteOpen = false)}></div>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby="invite-title">
    <h2 id="invite-title">Einladungslink</h2>
    <p class="dialog-text">
      Schicken Sie diesen Link der Person, der Sie helfen wollen. Sie lädt darüber die Schnellhilfe und kann Ihnen
      ID und Passwort mit einem Klick senden. Der Link gilt 7 Tage.
    </p>
    <input class="field" readonly value={inviteBusy ? "Link wird erstellt …" : inviteLink} onfocus={(e) => e.currentTarget.select()} />
    {#if inviteError}<p class="error">{inviteError}</p>{/if}
    <div class="dialog-actions">
      <button class="btn btn-quiet" type="button" onclick={() => (inviteOpen = false)}>Schließen</button>
      {#if inviteLink}
        <a class="btn btn-quiet mail" href={inviteMail}>Per E-Mail</a>
        <button class="btn btn-primary" type="button" onclick={copyInvite}>
          {inviteCopied ? "Kopiert" : "Kopieren"}
        </button>
      {/if}
    </div>
  </div>
{/if}

{#if accountOpen && overview}
  <Account
    account={overview.account ?? null}
    onclose={() => {
      accountOpen = false;
      refresh();
    }}
  />
{/if}

{#if historyOpen}
  <History onclose={() => (historyOpen = false)} />
{/if}

{#if settingsOpen && overview}
  <Settings
    profile={overview.profile}
    server={overview.server}
    unattended={overview.unattended}
    direct={overview.direct}
    directActive={overview.directActive}
    rightsAttended={overview.rightsAttended}
    rightsUnattended={overview.rightsUnattended}
    codeEnabled={overview.codeEnabled}
    service={overview.service}
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

  .update {
    height: 28px;
    padding: 0 10px;
    border: 1px solid var(--accent);
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 12.5px;
    font-weight: 600;
  }

  .update:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-ink);
  }

  .update:disabled {
    cursor: default;
    opacity: 0.7;
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

  /* Phones only view others: one column with the connect box and devices. */
  :global(body.mobile) main {
    display: block;
    overflow-y: auto;
  }

  :global(body.mobile) .this-device {
    display: none;
  }

  :global(body.mobile) section {
    padding: 20px 16px calc(20px + env(safe-area-inset-bottom));
  }

  :global(body.mobile) .connect {
    max-width: none;
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

  .alias {
    color: var(--ink);
    font-weight: 600;
    user-select: text;
  }

  .alias-edit {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .alias-edit .field {
    flex: 1;
    min-width: 0;
  }

  .alias-note {
    margin: 6px 0 0;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  .alias-note.bad {
    color: var(--bad);
  }

  .link-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: 13px;
    font-weight: 600;
  }

  .link-btn:hover:not(:disabled) {
    color: var(--accent-hover);
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

  .connect-actions {
    display: flex;
    gap: 8px;
  }

  .connect-actions .btn-primary {
    flex: 1;
  }

  .terminal-btn {
    width: 44px;
    padding: 0;
    border: 1px solid var(--line-strong);
    background: transparent;
    color: var(--ink-2);
  }

  .terminal-btn:hover:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
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

  .permissions {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-bottom: 14px;
    padding: 12px 14px;
    border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--line));
    border-radius: 10px;
    background: color-mix(in srgb, var(--accent) 7%, transparent);
    font-size: 13px;
  }

  .permissions p {
    margin: 0;
    color: var(--ink-2);
  }

  .permission-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .permission-row .ok {
    color: var(--accent);
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

  .s-access {
    margin-right: auto;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11.5px;
    font-weight: 600;
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

  .banner-logo {
    width: 20px;
    height: 20px;
    border-radius: 4px;
    object-fit: contain;
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

  .chat-btn {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .chat-btn + .banner-btn,
  .no-chat + .banner-btn {
    margin-left: 0;
  }

  .no-chat {
    margin-left: auto;
    opacity: 0.65;
    font-size: 12.5px;
  }

  .terminal-tag {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-left: 8px;
    padding: 1px 8px;
    border: 1px solid currentColor;
    border-radius: 6px;
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
  }

  .private-tag {
    margin-left: 8px;
    font-size: 12.5px;
    font-weight: 600;
  }

  .banner-row :global(.banner-rights) {
    margin-left: 8px;
  }

  .banner-row :global(.banner-rights + .banner-btn) {
    margin-left: 8px;
  }

  .chat-wrap {
    display: flex;
    height: 260px;
    border-bottom: 1px solid var(--line);
    background: var(--surface);
  }

  .requests {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 24px;
    border-bottom: 1px solid var(--line);
  }

  .request {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    background: var(--accent-soft);
  }

  .request-text {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .request-id {
    color: var(--ink-3);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }

  .scrim {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, #000 28%, transparent);
  }

  .dialog {
    position: fixed;
    top: 50%;
    left: 50%;
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: min(460px, calc(100% - 32px));
    padding: 24px;
    transform: translate(-50%, -50%);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg);
  }

  .dialog h2 {
    margin: 0;
    font-size: 18px;
  }

  .dialog-text {
    margin: 0;
    color: var(--ink-2);
    font-size: 13.5px;
  }

  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .dialog-actions a {
    display: inline-flex;
    align-items: center;
    text-decoration: none;
  }

  .banner-btn:hover {
    background: color-mix(in srgb, var(--bg) 12%, transparent);
  }

  /* A link styled as a button keeps the button's color, visited or not. */
  .dialog-actions a.mail,
  .dialog-actions a.mail:visited {
    color: inherit;
    text-decoration: none;
  }
</style>
