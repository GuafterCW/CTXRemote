<script lang="ts">
  import { api, defaultPolicy, errorText, type TeamPolicy } from "./api";
  import { formatDate } from "./format";
  import { RIGHTS } from "./rights";

  // The team policy; every member may read it, owners and admins edit it (`manage`).
  let { manage }: { manage: boolean } = $props();

  let open = $state(false);
  let loaded = $state(false);
  /** Whether the team has a policy at all. */
  let exists = $state(false);
  let draft = $state<TeamPolicy>(defaultPolicy());
  let busy = $state(false);
  let error = $state("");
  let status = $state("");
  let removing = $state(false);

  const COLUMNS: [string, "denyTechnician" | "denyAdmin" | "denyOthers"][] = [
    ["Techniker", "denyTechnician"],
    ["Inhaber und Admins", "denyAdmin"],
    ["Ohne Team", "denyOthers"],
  ];

  async function load() {
    busy = true;
    error = "";
    try {
      const policy = await api.teamPolicy();
      exists = policy !== null;
      draft = policy ?? defaultPolicy();
      loaded = true;
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  function toggle() {
    open = !open;
    if (open && !loaded) load();
  }

  /** Ticked = allowed = the bit is not in the deny mask. */
  const allowed = (key: "denyTechnician" | "denyAdmin" | "denyOthers", bit: number) => (draft[key] & bit) === 0;
  function setAllowed(key: "denyTechnician" | "denyAdmin" | "denyOthers", bit: number, on: boolean) {
    draft[key] = on ? draft[key] & ~bit : draft[key] | bit;
    status = "";
  }

  async function save() {
    busy = true;
    error = "";
    status = "";
    try {
      await api.teamSetPolicy(draft);
      await load();
      if (!error) status = "Gespeichert";
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  async function remove() {
    busy = true;
    error = "";
    status = "";
    try {
      await api.teamSetPolicy(null);
      removing = false;
      await load();
      if (!error) status = "Richtlinie entfernt";
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="policy">
  <div class="bar">
    <button type="button" class="link-btn quiet" aria-expanded={open} onclick={toggle}>
      {open ? "Richtlinien ausblenden" : "Richtlinien anzeigen"}
    </button>
  </div>

  {#if open}
    <h4>Richtlinien</h4>
    <span class="note">
      Gilt für alle Geräte, die fürs Team freigegeben sind, ab der nächsten Sitzung. Eine Richtlinie kann nur
      einschränken, was die Einstellungen eines Geräts erlauben.
    </span>

    {#if !loaded}
      {#if busy}<span class="note">Wird geladen …</span>{/if}
    {:else}
      {#if !exists}
        <span class="note">Keine Richtlinie: Teamgeräte folgen nur ihren eigenen Einstellungen.</span>
      {/if}
      {#if !manage}
        <span class="note">Nur Inhaber und Admins können die Richtlinie ändern.</span>
      {/if}

      <div class="rights" role="table">
        <div class="rights-row head" role="row">
          <span role="columnheader"></span>
          {#each COLUMNS as [title] (title)}
            <span role="columnheader">{title}</span>
          {/each}
        </div>
        {#each RIGHTS as [bit, label, hint, warn] (bit)}
          <div class="rights-row" role="row">
            <span role="cell">{label}{#if hint}<small class:warn>{hint}</small>{/if}</span>
            {#each COLUMNS as [title, key] (key)}
              <span role="cell">
                <input
                  type="checkbox"
                  aria-label={`${label}, ${title}`}
                  disabled={!manage || busy}
                  checked={allowed(key, bit)}
                  onchange={(e) => setAllowed(key, bit, e.currentTarget.checked)}
                />
              </span>
            {/each}
          </div>
        {/each}
      </div>

      <div class="switches">
        <label><input type="checkbox" disabled={!manage || busy} bind:checked={draft.oneTime} onchange={() => (status = "")} /> Einmalpasswort erlauben</label>
        <label><input type="checkbox" disabled={!manage || busy} bind:checked={draft.permanent} onchange={() => (status = "")} /> Festes Passwort erlauben</label>
        <label>
          <input type="checkbox" disabled={!manage || busy} bind:checked={draft.account} onchange={() => (status = "")} />
          Zugriff über Geräte des eigenen Kontos erlauben
        </label>
        <label>
          <input type="checkbox" disabled={!manage || busy} bind:checked={draft.requireCode} onchange={() => (status = "")} />
          Festes Passwort nur mit Bestätigungscode
        </label>
      </div>

      {#if draft.changed > 0}
        <span class="note">Zuletzt geändert: {formatDate(draft.changed)}</span>
      {/if}

      {#if manage}
        {#if removing}
          <div class="confirm">
            <p>Die Richtlinie entfernen? Teamgeräte folgen danach nur noch ihren eigenen Einstellungen.</p>
            <div class="row">
              <button type="button" class="btn btn-danger small" disabled={busy} onclick={remove}>Entfernen</button>
              <button type="button" class="btn btn-quiet small" onclick={() => (removing = false)}>Abbrechen</button>
            </div>
          </div>
        {:else}
          <div class="row">
            <button type="button" class="btn btn-primary small" disabled={busy} onclick={save}>Speichern</button>
            {#if exists}
              <button type="button" class="btn btn-quiet small" disabled={busy} onclick={() => (removing = true)}>
                Richtlinie entfernen
              </button>
            {/if}
          </div>
        {/if}
      {/if}
      {#if status}
        <p class="note" role="status">{status}</p>
      {/if}
    {/if}

    {#if error}
      <p class="error">{error}</p>
    {/if}
  {/if}
</div>

<style>
  .policy {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  h4 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .bar,
  .row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 14px;
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
    padding: 0;
    border: 0;
    background: none;
    color: var(--ink-3);
    font-size: 12.5px;
    text-decoration: underline;
    text-underline-offset: 3px;
    cursor: pointer;
  }

  .link-btn.quiet {
    color: var(--accent);
  }

  .rights {
    display: grid;
    gap: 2px;
  }

  .rights-row {
    display: grid;
    grid-template-columns: 1fr 70px 90px 70px;
    align-items: center;
    min-height: 30px;
    font-size: 13px;
  }

  .rights-row span:not(:first-child) {
    text-align: center;
  }

  .rights-row small {
    display: block;
    color: var(--ink-3);
    font-size: 11.5px;
  }

  .rights-row small.warn {
    color: var(--warn);
  }

  .rights-row.head {
    color: var(--ink-2);
    font-size: 12px;
  }

  .switches {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 13px;
  }

  .switches label {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .confirm {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 14px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--surface);
  }

  .confirm p {
    margin: 0;
    font-size: 13px;
    color: var(--ink-2);
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
