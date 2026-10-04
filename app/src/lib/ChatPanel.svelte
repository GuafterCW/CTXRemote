<script lang="ts" module>
  export interface ChatMessage {
    mine: boolean;
    text: string;
    /** Milliseconds since the epoch. */
    at: number;
  }

  export const CHAT_MAX = 4000;
</script>

<script lang="ts">
  import { tick } from "svelte";
  import { errorText } from "./api";
  import Icon from "./Icon.svelte";

  let {
    messages,
    onsend,
    onclose,
    disabled = false,
    variant = "app",
    placeholder = "Nachricht schreiben …",
  }: {
    messages: ChatMessage[];
    /** Rejects with the reason (string) if the message could not be sent. */
    onsend: (text: string) => Promise<void>;
    /** Shows a header with a close button; Escape closes as well. */
    onclose?: () => void;
    disabled?: boolean;
    /** "dark" is fixed dark for the session window, "app" follows the app tokens. */
    variant?: "app" | "dark";
    placeholder?: string;
  } = $props();

  let draft = $state("");
  let error = $state("");
  let sending = $state(false);
  let list = $state<HTMLDivElement>();
  let input = $state<HTMLTextAreaElement>();

  export function focus() {
    input?.focus();
  }

  const clock = (at: number) =>
    new Date(at).toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" });

  $effect(() => {
    messages.length;
    tick().then(() => list && (list.scrollTop = list.scrollHeight));
  });

  async function submit() {
    const text = draft.trim();
    if (!text || sending || disabled) return;
    sending = true;
    error = "";
    try {
      await onsend(text);
      draft = "";
    } catch (e) {
      error = errorText(e);
    } finally {
      sending = false;
      input?.focus();
    }
  }

  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape" && onclose) {
      e.preventDefault();
      onclose();
    }
  }
</script>

<!-- data-chat marks the panel so the session window does not forward its keys to the remote device. -->
<div class="chat" class:dark={variant === "dark"} data-chat>
  {#if onclose}
    <div class="head">
      <span>Chat</span>
      <button class="close" title="Schließen" onclick={onclose}><Icon name="close" size={14} /></button>
    </div>
  {/if}

  <div class="list" bind:this={list}>
    {#if messages.length === 0}
      <p class="empty">Noch keine Nachrichten.</p>
    {/if}
    {#each messages as m, i (i)}
      <div class="msg" class:mine={m.mine}>
        <div class="bubble">{m.text}</div>
        <span class="time">{clock(m.at)}</span>
      </div>
    {/each}
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  <form
    class="compose"
    onsubmit={(e) => {
      e.preventDefault();
      submit();
    }}
  >
    <textarea
      bind:this={input}
      bind:value={draft}
      rows="1"
      maxlength={CHAT_MAX}
      {placeholder}
      disabled={disabled}
      spellcheck="false"
      {onkeydown}
      oninput={() => (error = "")}
    ></textarea>
    <button class="send" title="Senden" disabled={disabled || sending || !draft.trim()}>
      <Icon name="arrowRight" size={16} />
    </button>
  </form>
</div>

<style>
  .chat {
    --cp-ink: var(--ink);
    --cp-ink-2: var(--ink-2);
    --cp-ink-3: var(--ink-3);
    --cp-line: var(--line);
    --cp-line-strong: var(--line-strong);
    --cp-field: var(--raised);
    --cp-mine: var(--accent-soft);
    --cp-accent: var(--accent);
    --cp-accent-ink: var(--accent-ink);
    --cp-bad: var(--bad);
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    color: var(--cp-ink);
    font-size: 13px;
  }

  .chat.dark {
    --cp-ink: #edebe6;
    --cp-ink-2: #b3b1aa;
    --cp-ink-3: #85837c;
    --cp-line: #2f2e2b;
    --cp-line-strong: #3b3a37;
    --cp-field: #11110f;
    --cp-mine: #1d2f29;
    --cp-accent: #4fb495;
    --cp-accent-ink: #0d1a16;
    --cp-bad: #e07a66;
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex: none;
    height: 36px;
    padding: 0 6px 0 14px;
    border-bottom: 1px solid var(--cp-line);
    font-weight: 600;
  }

  .close {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--cp-ink-3);
  }

  .close:hover {
    background: color-mix(in srgb, var(--cp-ink) 8%, transparent);
    color: var(--cp-ink);
  }

  .list {
    display: flex;
    flex-direction: column;
    gap: 10px;
    flex: 1;
    min-height: 110px;
    padding: 12px 14px;
    overflow-y: auto;
  }

  .empty {
    margin: auto;
    color: var(--cp-ink-3);
  }

  .msg {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    max-width: 85%;
  }

  .msg.mine {
    align-self: flex-end;
    align-items: flex-end;
  }

  .bubble {
    padding: 7px 11px;
    border: 1px solid var(--cp-line);
    border-radius: 10px;
    line-height: 1.4;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
    user-select: text;
  }

  .mine .bubble {
    border-color: transparent;
    background: var(--cp-mine);
  }

  .time {
    color: var(--cp-ink-3);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
  }

  .error {
    flex: none;
    margin: 0;
    padding: 0 14px 6px;
    color: var(--cp-bad);
    font-size: 12.5px;
  }

  .compose {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    flex: none;
    padding: 10px;
    border-top: 1px solid var(--cp-line);
  }

  textarea {
    flex: 1;
    min-width: 0;
    min-height: 34px;
    max-height: 96px;
    padding: 7px 10px;
    border: 1px solid var(--cp-line-strong);
    border-radius: 8px;
    background: var(--cp-field);
    color: var(--cp-ink);
    font: inherit;
    line-height: 1.4;
    resize: none;
    field-sizing: content;
    outline: none;
  }

  textarea::placeholder {
    color: var(--cp-ink-3);
  }

  textarea:focus {
    border-color: var(--cp-accent);
  }

  textarea:disabled {
    opacity: 0.55;
  }

  .send {
    display: grid;
    place-items: center;
    flex: none;
    width: 34px;
    height: 34px;
    border: 0;
    border-radius: 8px;
    background: var(--cp-accent);
    color: var(--cp-accent-ink);
  }

  .send:disabled {
    opacity: 0.4;
  }
</style>
