<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText, type Outcome, type Visit } from "./lib/api";
  import Icon from "./lib/Icon.svelte";

  let { onclose }: { onclose: () => void } = $props();

  let visits = $state<Visit[] | null>(null);
  let busy = $state(false);
  let error = $state("");

  async function load() {
    busy = true;
    error = "";
    try {
      visits = await api.history();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  onMount(load);

  const labels: Record<Outcome, string> = {
    oneTimePassword: "Einmalpasswort",
    permanentPassword: "Festes Passwort",
    account: "Konto, ohne Passwort",
    wrongPassword: "Falsches Passwort",
    notMember: "Kein Kontomitglied",
    declined: "Abgelehnt",
  };

  const refused = (o: Outcome) => o === "wrongPassword" || o === "notMember" || o === "declined";

  function when(seconds: number): string {
    const date = new Date(seconds * 1000);
    const sameYear = date.getFullYear() === new Date().getFullYear();
    const day = date.toLocaleDateString("de-DE", {
      day: "numeric",
      month: "short",
      ...(sameYear ? {} : { year: "numeric" }),
    });
    const time = date.toLocaleTimeString("de-DE", { hour: "2-digit", minute: "2-digit" });
    return `${day}, ${time}`;
  }

  function duration(v: Visit): string {
    if (refused(v.outcome)) return "";
    if (v.ended === null) {
      return Date.now() / 1000 - v.started < 24 * 3600 ? "läuft" : "Ende unbekannt";
    }
    const minutes = Math.floor(Math.max(0, v.ended - v.started) / 60);
    if (minutes < 1) return "unter 1 Min.";
    if (minutes < 60) return `${minutes} Min.`;
    const rest = minutes % 60;
    return rest ? `${Math.floor(minutes / 60)} Std. ${rest} Min.` : `${Math.floor(minutes / 60)} Std.`;
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" role="presentation" onclick={onclose}></div>
<div class="sheet">
  <header>
    <h2>Verlauf</h2>
    <button type="button" class="icon-btn" title="Schließen" onclick={onclose}>
      <Icon name="close" />
    </button>
  </header>

  <div class="body">
    <div class="intro">
      <p class="note">
        Eingehende Verbindungen zu diesem Computer, auch abgewiesene. Der Verlauf bleibt auf diesem Computer.
      </p>
      <button type="button" class="link-btn" disabled={busy} onclick={load}>Aktualisieren</button>
    </div>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    {#if visits === null}
      {#if !error}<span class="note">Wird geladen …</span>{/if}
    {:else if visits.length === 0}
      <span class="note">Noch keine Verbindungen.</span>
    {:else}
      <ul class="visits">
        {#each visits as v, i (i)}
          {@const length = duration(v)}
          <li>
            <span class="top">
              <span class="who">
                {#if v.profile}
                  {v.profile}{#if v.peer}<span class="peer">{v.peer}</span>{/if}
                {:else}
                  {v.peer || "Unbekannt"}
                {/if}
              </span>
              <span class="when">{when(v.started)}</span>
            </span>
            <span class="meta">
              <span class="outcome" class:bad={refused(v.outcome)} class:accent={v.outcome === "account"}>
                {labels[v.outcome]}
              </span>
              {#if length}<span class="length">{length}</span>{/if}
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, #000 28%, transparent);
  }

  .sheet {
    position: fixed;
    top: 0;
    right: 0;
    bottom: 0;
    display: flex;
    flex-direction: column;
    width: min(420px, 100%);
    background: var(--bg);
    border-left: 1px solid var(--line);
    animation: slide 220ms var(--ease);
  }

  @keyframes slide {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 52px;
    padding: 0 12px 0 24px;
    border-bottom: 1px solid var(--line);
  }

  h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
  }

  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 24px;
    overflow-y: auto;
  }

  .intro {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .note {
    margin: 0;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  .error {
    margin: 0;
    color: var(--bad);
    font-size: 13px;
  }

  .link-btn {
    align-self: flex-start;
    padding: 0;
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 12.5px;
    text-decoration: underline;
    text-underline-offset: 3px;
    cursor: pointer;
  }

  .visits {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--line);
  }

  .visits li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 0;
    border-bottom: 1px solid var(--line);
  }

  .top,
  .meta {
    display: flex;
    justify-content: space-between;
    gap: 12px;
  }

  .who {
    min-width: 0;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .peer {
    margin-left: 6px;
    color: var(--ink-3);
    font-size: 12px;
    font-weight: 400;
  }

  .when,
  .length {
    flex: none;
    color: var(--ink-3);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }

  .outcome {
    color: var(--ink-2);
    font-size: 12.5px;
  }

  .outcome.accent {
    color: var(--accent);
  }

  .outcome.bad {
    color: var(--bad);
  }
</style>
