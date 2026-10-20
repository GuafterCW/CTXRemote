<script lang="ts">
  import { onMount } from "svelte";
  import { ApiError } from "./lib/session";
  import { defaultPolicy, loadPolicy, savePolicy, type TeamPolicy } from "./lib/team";

  let { canManage, onExpired }: { canManage: boolean; onExpired: () => void } = $props();

  // Bits as in `Permissions` (crates/proto/src/session.rs).
  const RIGHTS: [number, string][] = [
    [1, "Maus und Tastatur"],
    [2, "Dateien"],
    [4, "Zwischenablage"],
    [8, "Ton"],
    [16, "Neustart"],
    [32, "Bildschirm schwarz schalten (Privatsphäre-Modus)"],
    [64, "Port-Tunnel"],
    [128, "Terminal"],
    [256, "Prozesse und Dienste"],
  ];
  const COLUMNS: [keyof Pick<TeamPolicy, "denyTechnician" | "denyAdmin" | "denyOthers">, string][] = [
    ["denyTechnician", "Techniker"],
    ["denyAdmin", "Inhaber und Admins"],
    ["denyOthers", "Ohne Team"],
  ];

  let stored = $state<TeamPolicy | null>(null);
  let draft = $state<TeamPolicy>(defaultPolicy());
  let loaded = $state(false);
  let error = $state("");
  let busy = $state(false);
  let saved = $state(false);
  let confirmRemove = $state(false);

  onMount(async () => {
    try {
      stored = await loadPolicy();
      draft = stored ? { ...stored } : defaultPolicy();
    } catch (err) {
      fail(err);
    } finally {
      loaded = true;
    }
  });

  function fail(err: unknown) {
    if (err instanceof ApiError && err.status === 401) onExpired();
    else error = err instanceof Error ? err.message : "Unbekannter Fehler";
  }

  const allowed = (mask: number, bit: number) => (mask & bit) === 0;

  function toggle(column: (typeof COLUMNS)[number][0], bit: number, on: boolean) {
    draft[column] = on ? draft[column] & ~bit : draft[column] | bit;
  }

  async function save() {
    busy = true;
    error = "";
    saved = false;
    try {
      const next = { ...draft, changed: Math.floor(Date.now() / 1000) };
      await savePolicy(next);
      stored = next;
      draft = { ...next };
      saved = true;
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function remove() {
    busy = true;
    error = "";
    saved = false;
    try {
      await savePolicy(null);
      stored = null;
      draft = defaultPolicy();
      confirmRemove = false;
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  function when(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString("de-DE", { dateStyle: "short", timeStyle: "short" });
  }
</script>

<h3>Richtlinien</h3>
{#if !loaded}
  <p class="empty">Wird geladen …</p>
{:else}
  <p class="hint">
    Gilt für alle Geräte, die fürs Team freigegeben sind, ab der nächsten Sitzung. Eine Richtlinie kann nur einschränken,
    was die Einstellungen eines Geräts erlauben.
  </p>
  {#if error}<p class="error" role="alert">{error}</p>{/if}
  {#if !stored}
    <p class="hint">Keine Richtlinie: Teamgeräte folgen nur ihren eigenen Einstellungen.</p>
  {/if}
  {#if stored || canManage}
    <div class="scroll">
      <table>
        <thead>
          <tr>
            <th>Recht</th>
            {#each COLUMNS as [, label] (label)}<th class="center">{label}</th>{/each}
          </tr>
        </thead>
        <tbody>
          {#each RIGHTS as [bit, label] (bit)}
            <tr>
              <td>{label}</td>
              {#each COLUMNS as [column, columnLabel] (column)}
                <td class="center">
                  <input
                    type="checkbox"
                    aria-label="{label}: {columnLabel}"
                    checked={allowed(draft[column], bit)}
                    disabled={!canManage || busy}
                    onchange={(e) => toggle(column, bit, e.currentTarget.checked)}
                  />
                </td>
              {/each}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <div class="switches">
      <label><input type="checkbox" bind:checked={draft.oneTime} disabled={!canManage || busy} /> Einmalpasswort erlauben</label>
      <label><input type="checkbox" bind:checked={draft.permanent} disabled={!canManage || busy} /> Festes Passwort erlauben</label>
      <label>
        <input type="checkbox" bind:checked={draft.account} disabled={!canManage || busy} /> Zugriff über Geräte des eigenen Kontos erlauben
      </label>
      <label>
        <input type="checkbox" bind:checked={draft.requireCode} disabled={!canManage || busy} /> Festes Passwort nur mit Bestätigungscode
      </label>
    </div>

    {#if stored && stored.changed > 0}
      <p class="hint">Zuletzt geändert: {when(stored.changed)}</p>
    {/if}

    {#if canManage}
      <div class="actions">
        {#if confirmRemove}
          <button class="btn btn-small btn-danger" type="button" disabled={busy} onclick={remove}>Richtlinie wirklich entfernen</button>
          <button class="btn btn-small" type="button" disabled={busy} onclick={() => (confirmRemove = false)}>Abbrechen</button>
        {:else}
          <button class="btn btn-small btn-primary" type="button" disabled={busy} onclick={save}>Speichern</button>
          {#if stored}
            <button class="btn btn-small" type="button" disabled={busy} onclick={() => ((confirmRemove = true), (saved = false))}>
              Richtlinie entfernen
            </button>
          {/if}
          {#if saved}<span class="saved" role="status">Gespeichert</span>{/if}
        {/if}
      </div>
    {/if}
  {/if}
{/if}

<style>
  h3 {
    margin: 24px 0 8px;
    font-size: 15px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .scroll {
    overflow-x: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 14px;
  }

  th,
  td {
    padding: 8px 10px;
    border-bottom: 1px solid var(--line);
    text-align: left;
    vertical-align: middle;
  }

  th {
    color: var(--ink-3);
    font-weight: 600;
    white-space: nowrap;
  }

  .center {
    text-align: center;
  }

  .switches {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 14px 0 8px;
    font-size: 14px;
    color: var(--ink);
  }

  .switches label {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-top: 12px;
  }

  .saved {
    color: var(--ink-2);
    font-size: 14px;
  }
</style>
