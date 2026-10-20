<script lang="ts">
  import { onMount } from "svelte";
  import { api, errorText, type TeamInfo, type Waiting } from "./api";
  import { formatId } from "./format";

  // The team's support queue, for every member. Only shown while the team has a help link.
  let { team, onclaim }: { team: TeamInfo | null; onclaim: (r: { id: string; password: string; name: string }) => void } =
    $props();

  const POLL_MS = 10_000;

  let waiting = $state<Waiting[]>([]);
  /** Tickets already shown once; later ones are highlighted as new. */
  let seen = new Set<number>();
  let fresh = $state<Set<number>>(new Set());
  let first = true;
  let claiming = $state<number | null>(null);
  let error = $state("");
  /** Re-renders the "waiting since" texts between polls. */
  let now = $state(Date.now());

  const active = $derived(!!team?.helpLink);

  async function load() {
    try {
      const list = await api.teamQueue();
      const added = list.filter((w) => !seen.has(w.ticket)).map((w) => w.ticket);
      list.forEach((w) => seen.add(w.ticket));
      // The first load only fills the list; it is not news.
      if (!first && added.length > 0) {
        fresh = new Set([...fresh, ...added]);
        setTimeout(() => (fresh = new Set([...fresh].filter((t) => !added.includes(t)))), 8000);
      }
      first = false;
      waiting = list;
    } catch {
      // Silent: e.g. not signed in or offline; the next poll tries again.
    }
  }

  $effect(() => {
    if (!active) {
      waiting = [];
      seen = new Set();
      first = true;
      return;
    }
    load();
    const poll = setInterval(load, POLL_MS);
    return () => clearInterval(poll);
  });

  onMount(() => {
    const tick = setInterval(() => (now = Date.now()), 30_000);
    return () => clearInterval(tick);
  });

  function minutes(w: Waiting): string {
    const m = Math.floor((now / 1000 - w.since) / 60);
    return m < 1 ? "wartet seit weniger als 1 Min." : `wartet seit ${m} Min.`;
  }

  async function claim(w: Waiting) {
    claiming = w.ticket;
    error = "";
    try {
      onclaim(await api.teamClaimHelp(w.ticket));
    } catch (e) {
      error = errorText(e);
      await load();
    } finally {
      claiming = null;
    }
  }
</script>

{#if active}
  <section class="queue" aria-label="Warteschlange">
    <div class="queue-head">Warteschlange</div>
    {#each waiting as w (w.ticket)}
      <div class="item" class:fresh={fresh.has(w.ticket)}>
        <div class="item-text">
          <strong>{w.name || "Unbekannt"}</strong>
          <span class="item-meta">ID {formatId(w.id)} · {minutes(w)}</span>
        </div>
        <button class="btn btn-primary" type="button" disabled={claiming !== null} onclick={() => claim(w)}>
          {claiming === w.ticket ? "Übernehme …" : "Übernehmen"}
        </button>
      </div>
    {:else}
      <p class="empty">Keine wartenden Anfragen</p>
    {/each}
    {#if error}<p class="error">{error}</p>{/if}
  </section>
{/if}

<style>
  .queue {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 24px;
    border-bottom: 1px solid var(--line);
  }

  .queue-head {
    color: var(--ink-2);
    font-size: 13px;
    font-weight: 600;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 14px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    transition: background 0.4s, border-color 0.4s;
  }

  .item.fresh {
    border-color: var(--accent);
    background: var(--accent-soft);
  }

  .item-text {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .item-meta {
    color: var(--ink-3);
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
  }

  .empty {
    margin: 0;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  .error {
    margin: 0;
    color: var(--bad);
    font-size: 13px;
  }
</style>
