<script lang="ts">
  import { onMount } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import {
    api,
    errorText,
    hostedLabel,
    profileLabel,
    type ApprovalRequest,
    type HostEvent,
    type Hosted,
    type Overview,
    type Presence,
  } from "./lib/api";
  import RightsMenu from "./lib/RightsMenu.svelte";
  import ChatPanel, { type ChatMessage } from "./lib/ChatPanel.svelte";
  import Icon from "./lib/Icon.svelte";
  import ProfileCard from "./lib/ProfileCard.svelte";

  let overview = $state<Overview | null>(null);
  let presence = $state<Presence>({ state: "connecting" });
  let hosted = $state<Hosted[]>([]);
  let requests = $state<ApprovalRequest[]>([]);
  let copied = $state<"id" | "password" | null>(null);
  let denyButton = $state<HTMLButtonElement>();

  // Chat history per session number; it goes away with the session.
  let chats = $state<Record<number, ChatMessage[]>>({});

  async function sendChat(session: number, text: string) {
    await api.hostChat(session, text);
    chats[session] = [...(chats[session] ?? []), { mine: true, text, at: Date.now() }];
  }

  // Invitation: the helper whose link this quick help was downloaded from.
  // `team`: the link of a support team (no ID); the request waits in the team's queue.
  let inviter = $state<{ id: string | null; team: boolean } | null>(null);
  /** A team member took the queued request. */
  let claimed = $state(false);
  let sendState = $state<"idle" | "sending" | "sent" | "failed">("idle");
  let sendError = $state("");

  async function sendHelp() {
    sendState = "sending";
    sendError = "";
    try {
      await api.sendHelp();
      sendState = "sent";
    } catch (e) {
      sendError = errorText(e);
      sendState = "failed";
    }
  }

  const myId = $derived(presence.state === "online" ? presence.id : null);
  const request = $derived(requests[0] ?? null);

  async function refresh() {
    overview = await api.overview();
    presence = overview.presence;
    // The overview does not know about open terminals; events do.
    hosted = overview.hosted.map((h) => {
      const known = hosted.find((o) => o.session === h.session);
      return { ...h, terminal: known?.terminal ?? false, terminalOnly: known?.terminalOnly ?? false };
    });
  }

  onMount(() => {
    refresh();
    api
      .invitation()
      .then((i) => (inviter = i))
      .catch(() => {});
    const subscriptions = [
      listen<Presence>("presence", (e) => (presence = e.payload)),
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
        }
        if (event.kind === "chat") {
          chats[event.session] = [...(chats[event.session] ?? []), { mine: false, text: event.text, at: Date.now() }];
        }
        if (event.kind === "passwordChanged") refresh();
        if (event.kind === "helpClaimed") claimed = true;
      }),
      listen<ApprovalRequest>("approval-request", (e) => {
        if (!requests.some((r) => r.id === e.payload.id)) requests = [...requests, e.payload];
      }),
      listen<{ id: number }>("approval-closed", (e) => {
        requests = requests.filter((r) => r.id !== e.payload.id);
      }),
    ];
    return () => subscriptions.forEach((s) => s.then((unlisten) => unlisten()));
  });

  // Safe default: focus lands on "Ablehnen" whenever a new request comes up.
  $effect(() => {
    if (request) denyButton?.focus();
  });

  async function answer(allow: boolean) {
    if (!request) return;
    const { id } = request;
    requests = requests.filter((r) => r.id !== id);
    await api.answerApproval(id, allow);
  }

  async function copy(kind: "id" | "password", text: string) {
    await navigator.clipboard.writeText(text.replace(/\s/g, ""));
    copied = kind;
    setTimeout(() => (copied = copied === kind ? null : copied), 1400);
  }

  async function newPassword() {
    const pw = await api.refreshPassword();
    if (overview) overview.password = pw;
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
  <header>
    <div class="brand">
      <svg width="20" height="20" viewBox="250 220 640 640" aria-hidden="true"><path d="M330 760 V430 Q330 300 470 300" fill="none" stroke="currentColor" stroke-width="130" stroke-linecap="round"/><path d="M560 300 L560 700 L652 610 L716 748 L788 716 L724 580 L852 580 Z" fill="var(--accent)" stroke="var(--accent)" stroke-width="28" stroke-linejoin="round"/></svg>
      <span>Remocto Hilfe</span>
    </div>
  </header>

  {#if request}
    <main class="ask" aria-labelledby="ask-title">
      <div class="label">Zugriffsanfrage</div>
      {#if request.profile}
        <p id="ask-title" class="ask-title">
          <strong>{profileLabel(request.profile)}</strong> möchte auf diesen PC zugreifen.
        </p>
        <ProfileCard profile={request.profile} peer={request.peer} />
        <p class="hint">
          Diese Angaben macht die Person selbst, Remocto prüft sie nicht. Lassen Sie nur Personen zu, die
          Sie selbst um Hilfe gebeten haben.
        </p>
      {:else}
        <p id="ask-title" class="ask-title"><strong>{request.peer}</strong> möchte auf diesen PC zugreifen.</p>
        <p class="hint">Lassen Sie nur Personen zu, denen Sie vertrauen.</p>
      {/if}
      {#if requests.length > 1}
        <p class="hint">Weitere Anfragen warten: {requests.length - 1}</p>
      {/if}
      <div class="ask-actions">
        <button bind:this={denyButton} class="btn btn-quiet" onclick={() => answer(false)}>Ablehnen</button>
        <button class="btn btn-primary" onclick={() => answer(true)}>Zulassen</button>
      </div>
    </main>
  {:else}
    <main>
      {#if hosted.length > 0}
        <section class="live-box">
          {#each hosted as h (h.session)}
            <div class="live-item">
              <div class="live-row">
                <span class="live"></span>
                <span class="live-text">Verbunden mit <strong>{hostedLabel(h)}</strong></span>
                {#if h.recording}<span class="rec-tag" title="Die Gegenseite zeichnet die Sitzung als Video auf">● Aufnahme</span>{/if}
                {#if h.terminal}
                  <span class="terminal-tag" title="Die Gegenseite hat eine Befehlszeile auf diesem Gerät offen"><Icon name="terminal" size={14} /> Terminal offen</span>
                {/if}
                <RightsMenu hosted={h} allowPrivacy={false} class="quick-rights" />
                <button class="btn btn-quiet end" onclick={() => api.endHostedSession(h.session)}>Trennen</button>
              </div>
              {#if h.chat}
                <div class="chat-wrap">
                  <ChatPanel messages={chats[h.session] ?? []} onsend={(text) => sendChat(h.session, text)} />
                </div>
              {:else}
                <p class="hint no-chat">Die Gegenstelle hat eine ältere Version ohne Chat.</p>
              {/if}
            </div>
          {/each}
        </section>
      {/if}

      {#if inviter}
        <section class="invite">
          <strong>Sie wurden zur Fernhilfe eingeladen</strong>
          <span class="hint">{inviter.team ? "Ihr Support-Team" : `von ID ${inviter.id}`}</span>
          {#if inviter.team && claimed}
            <p class="invite-ok">
              Ein Techniker hat Ihre Anfrage übernommen und verbindet sich gleich. Bitte bestätigen Sie dann mit „Zulassen“.
            </p>
          {:else if sendState === "sent"}
            <p class="invite-ok">
              {inviter.team
                ? "Ihre Anfrage wartet. Sobald jemand aus dem Team übernimmt, sehen Sie es hier."
                : "Gesendet. Ihr Helfer verbindet sich gleich, Sie bestätigen den Zugriff wie gewohnt."}
            </p>
          {:else}
            {#if sendState === "failed"}
              <p class="invite-bad">{sendError.replace(/[.!]?$/, ".")} Lesen Sie ID und Passwort einfach vor.</p>
            {/if}
            <button class="btn btn-primary" disabled={!myId || sendState === "sending"} onclick={sendHelp}>
              {sendState === "sending"
                ? "Wird gesendet …"
                : sendState === "failed"
                  ? "Nochmal senden"
                  : inviter.team
                    ? "Anfrage an das Support-Team senden"
                    : "Zugangsdaten senden"}
            </button>
          {/if}
        </section>
      {/if}

      <p class="intro">Geben Sie diese Daten an Ihren Helfer weiter.</p>

      <section class="block">
        <div class="label">Ihre ID</div>
        <div class="row">
          <div class="id" class:placeholder={!myId}>{myId ?? "––– ––– –––"}</div>
          {#if myId}
            <button class="icon-btn" title="ID kopieren" onclick={() => copy("id", myId!)}>
              <Icon name={copied === "id" ? "check" : "copy"} size={18} />
            </button>
          {/if}
        </div>
      </section>

      <section class="block">
        <div class="label">Einmal-Passwort</div>
        <div class="row">
          <div class="password">{overview?.password ?? ""}</div>
          <div class="actions">
            <button class="icon-btn" title="Kopieren" onclick={() => overview && copy("password", overview.password)}>
              <Icon name={copied === "password" ? "check" : "copy"} size={18} />
            </button>
            <button class="icon-btn" title="Neues Passwort" onclick={newPassword}>
              <Icon name="refresh" size={18} />
            </button>
          </div>
        </div>
        <p class="hint">Gilt für eine Sitzung.</p>
      </section>

      <footer>
        <span class="dot" data-state={presence.state}></span>
        <span class="status-text">
          {statusText}{#if presence.state === "offline" && presence.reason}: {presence.reason}{/if}
        </span>
      </footer>
    </main>
  {/if}
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  header {
    display: flex;
    align-items: center;
    height: 52px;
    padding: 0 24px;
    border-bottom: 1px solid var(--line);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  main {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 24px;
    min-height: 0;
    padding: 24px 28px;
    background: var(--surface);
    overflow: auto;
  }

  .intro {
    margin: 0;
    color: var(--ink-2);
  }

  .block {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-top: 20px;
    border-top: 1px solid var(--line);
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .id {
    font: 600 38px/1.2 var(--font-display);
    letter-spacing: 0.01em;
    font-variant-numeric: tabular-nums;
    user-select: text;
  }

  .id.placeholder {
    color: var(--line-strong);
  }

  .password {
    font: 600 28px/1.3 var(--font-display);
    letter-spacing: 0.06em;
    user-select: text;
  }

  .actions {
    display: flex;
    gap: 2px;
  }

  .hint {
    margin: 4px 0 0;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: auto;
    padding-top: 14px;
    border-top: 1px solid var(--line);
    color: var(--ink-2);
    font-size: 13px;
  }

  .status-text {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .dot {
    flex: none;
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

  .live-box {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    background: var(--accent-soft);
  }

  .live-row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
  }

  .live-item + .live-item {
    border-top: 1px solid var(--line);
  }

  .live {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--accent);
  }

  .live-text {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .chat-wrap {
    display: flex;
    height: 260px;
    border-top: 1px solid var(--line);
    background: var(--surface);
    border-radius: 0 0 var(--radius) var(--radius);
  }

  .no-chat {
    margin: 0;
    padding: 0 14px 12px;
  }

  .end {
    height: 34px;
    padding: 0 14px;
  }

  .live-row :global(.quick-rights) {
    margin-left: auto;
  }

  .ask {
    justify-content: center;
    gap: 12px;
  }

  .ask-title {
    margin: 0;
    font: 600 22px/1.3 var(--font-display);
    overflow-wrap: anywhere;
  }

  .ask-actions {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    margin-top: 16px;
  }

  .invite {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 14px;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    background: var(--accent-soft);
  }

  .invite .btn {
    margin-top: 6px;
  }

  .invite p {
    margin: 0;
    font-size: 13.5px;
  }

  .invite-bad {
    color: var(--bad);
  }

  .terminal-tag {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 1px 8px;
    border: 1px solid currentColor;
    border-radius: 6px;
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
  }

  .rec-tag {
    color: var(--bad);
    font-size: 12.5px;
    font-weight: 600;
  }
</style>
