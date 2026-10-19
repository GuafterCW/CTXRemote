<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { ApiError } from "./lib/session";
  import {
    createTeam,
    deleteTeam,
    forgetTeam,
    inviteMember,
    joinTeam,
    leaveTeam,
    manages,
    newEntry,
    rotateTeam,
    setRole,
    syncTeam,
    updateTeamBook,
    type Member,
    type Role,
    type TeamView,
  } from "./lib/team";

  let { onExpired }: { onExpired: () => void } = $props();

  let team = $state<TeamView | null>(null);
  let loaded = $state(false);
  let error = $state("");
  let busy = $state(false);

  const ROLE: Record<Role, string> = { Owner: "Inhaber", Admin: "Admin", Technician: "Techniker" };

  const canManage = $derived(team ? manages(team.role) : false);
  const isOwner = $derived(team?.role === "Owner");
  const devices = $derived(
    team
      ? Object.entries(team.book.book.entries).sort(([, a], [, b]) =>
          (a.alias || a.name).localeCompare(b.alias || b.name, "de"),
        )
      : [],
  );
  const openHosts = $derived(team ? Object.entries(team.access).sort(([a], [b]) => a.localeCompare(b)) : []);

  /** Returns the message, or leaves the page when the session is gone. */
  function message(err: unknown): string {
    if (err instanceof ApiError && err.status === 401) {
      onExpired();
      return "";
    }
    return err instanceof Error ? err.message : "Unbekannter Fehler";
  }

  /** Runs one action with the busy flag and error line. */
  async function run(action: () => Promise<void>): Promise<boolean> {
    busy = true;
    error = "";
    try {
      await action();
      return true;
    } catch (err) {
      error = message(err);
      return false;
    } finally {
      busy = false;
    }
  }

  // The team key held for a fresh join does not outlive the page.
  onDestroy(forgetTeam);

  onMount(async () => {
    try {
      team = await syncTeam();
    } catch (err) {
      error = message(err);
    } finally {
      loaded = true;
    }
  });

  function formatId(id: string): string {
    return /^\d{9}$/.test(id) ? id.replace(/(\d{3})(?=\d)/g, "$1 ") : id;
  }

  function formatSince(seconds: number): string {
    return seconds ? new Date(seconds * 1000).toLocaleDateString("de-DE", { dateStyle: "medium" }) : "";
  }

  function label(m: Member): string {
    return m.email ?? `Konto ${m.id}`;
  }

  function memberName(id: number): string {
    const m = team?.members.find((m) => m.id === id);
    return m ? label(m) : `Konto ${id}`;
  }

  // No team yet: found one or join with a code.
  let newName = $state("");
  let joinCode = $state("");

  async function found(event: SubmitEvent) {
    event.preventDefault();
    if (!newName.trim()) return;
    await run(async () => {
      team = await createTeam(newName);
      newName = "";
    });
  }

  async function join(event: SubmitEvent) {
    event.preventDefault();
    await run(async () => {
      team = await joinTeam(joinCode);
      joinCode = "";
    });
  }

  // Team name
  let renaming = $state(false);
  let renameText = $state("");

  async function saveName(event: SubmitEvent) {
    event.preventDefault();
    const name = renameText.trim();
    if (!name) return;
    if (await run(async () => void (team = await updateTeamBook((b) => (b.name = name))))) renaming = false;
  }

  // Invitations
  let inviteRole = $state<"Technician" | "Admin">("Technician");
  let invite = $state("");
  let copied = $state(false);

  async function newInvite() {
    copied = false;
    await run(async () => void (invite = await inviteMember(inviteRole)));
  }

  async function copyInvite() {
    try {
      await navigator.clipboard.writeText(invite);
      copied = true;
    } catch {
      // No clipboard access: the code is selectable by a click.
    }
  }

  // Members
  let confirmRemove = $state<number | null>(null);
  let confirmOwner = $state<number | null>(null);

  /** What the signed-in member may do with `m` (the server enforces it too). */
  function mayManage(m: Member): boolean {
    return !!team && m.id !== team.me && canManage && m.role !== "Owner";
  }

  async function changeRole(m: Member, role: Role) {
    await run(async () => {
      await setRole(m.id, role);
      team = await syncTeam();
      confirmOwner = null;
    });
  }

  async function removeMember(m: Member) {
    await run(async () => {
      team = await rotateTeam([m.id]);
      confirmRemove = null;
    });
  }

  // Leaving and deleting
  let confirmLeave = $state(false);
  let deleteOpen = $state(false);
  let deleteName = $state("");

  async function leave() {
    await run(async () => {
      await leaveTeam();
      team = null;
      invite = "";
      confirmLeave = false;
    });
  }

  async function remove(event: SubmitEvent) {
    event.preventDefault();
    await run(async () => {
      await deleteTeam();
      team = null;
      invite = "";
      deleteOpen = false;
      deleteName = "";
    });
  }

  // Shared address book
  let editId = $state<string | null>(null);
  let editText = $state("");
  let confirmEntry = $state<string | null>(null);
  let addName = $state("");
  let addId = $state("");

  const addDigits = $derived(addId.replace(/\s/g, ""));
  const addValid = $derived(/^[1-9]\d{8}$/.test(addDigits) && addName.trim() !== "");

  async function addDevice(event: SubmitEvent) {
    event.preventDefault();
    if (!addValid) return;
    const id = addDigits;
    const name = addName.trim();
    const ok = await run(async () => {
      team = await updateTeamBook((b) => {
        delete b.book.removed[id];
        const entry = b.book.entries[id];
        if (entry) {
          entry.alias = name;
          entry.alias_at = Date.now();
        } else {
          b.book.entries[id] = { ...newEntry(name), alias: name, alias_at: Date.now() };
        }
      });
    });
    if (ok) addName = addId = "";
  }

  async function saveEdit(event: SubmitEvent) {
    event.preventDefault();
    const id = editId;
    if (id === null) return;
    const alias = editText.trim();
    if (await run(async () => void (team = await updateTeamBook((b) => {
      const entry = b.book.entries[id];
      if (!entry) return;
      entry.alias = alias || null;
      entry.alias_at = Date.now();
    })))) editId = null;
  }

  async function dropEntry(id: string) {
    if (
      await run(async () => void (team = await updateTeamBook((b) => {
        delete b.book.entries[id];
        b.book.removed[id] = Date.now();
      })))
    ) confirmEntry = null;
  }
</script>

<section class="panel" aria-labelledby="h-team">
  <div class="panel-head">
    <h2 id="h-team">Team</h2>
  </div>

  {#if !loaded}
    {#if !error}<p class="empty">Wird geladen …</p>{/if}
  {:else if team === null}
    <p class="hint">
      Ein Team teilt ein Adressbuch und Rollen zwischen mehreren Konten, Ende-zu-Ende verschlüsselt. Gründen Sie ein
      Team oder treten Sie mit einem Einladungscode bei.
    </p>
    <form onsubmit={found}>
      <label class="field">
        <span>Neues Team gründen</span>
        <input type="text" maxlength="64" placeholder="Name des Teams, z. B. Werkstatt" bind:value={newName} disabled={busy} />
      </label>
      <button class="btn btn-primary" type="submit" disabled={busy || !newName.trim()}>
        {busy ? "Bitte warten …" : "Team gründen"}
      </button>
    </form>
    <form class="second" onsubmit={join}>
      <label class="field">
        <span>Einem Team beitreten</span>
        <input type="text" class="code-input" autocomplete="off" placeholder="ABCD-EFGH-JKMN" bind:value={joinCode} disabled={busy} />
      </label>
      <button class="btn" type="submit" disabled={busy || !joinCode.trim()}>Beitreten</button>
    </form>
  {:else}
    <div class="team-head">
      {#if renaming}
        <form class="rename" onsubmit={saveName}>
          <input type="text" aria-label="Name des Teams" maxlength="64" bind:value={renameText} disabled={busy} />
          <button class="btn btn-small btn-primary" type="submit" disabled={busy || !renameText.trim()}>Speichern</button>
          <button class="btn btn-small" type="button" disabled={busy} onclick={() => (renaming = false)}>Abbrechen</button>
        </form>
      {:else}
        <div class="item-text">
          <div class="team-name">{team.book.name || "Team ohne Namen"}</div>
          <div class="item-sub">Ihre Rolle: {ROLE[team.role]}</div>
        </div>
        {#if canManage}
          <button class="btn btn-small" type="button" onclick={() => ((renameText = team?.book.name ?? ""), (renaming = true))}>
            Umbenennen
          </button>
        {/if}
      {/if}
    </div>

    <h3>Mitglieder</h3>
    <ul class="list">
      {#each team.members as m (m.id)}
        <li>
          <div class="item-main">
            <div class="item-text">
              <div class="item-name">
                {label(m)}
                {#if m.id === team.me}<span class="badge">Sie</span>{/if}
                {#if team.unverified.includes(m.id)}<span class="badge">unbestätigt</span>{/if}
              </div>
              <div class="item-sub">{ROLE[m.role]}{m.since ? ` · seit ${formatSince(m.since)}` : ""}</div>
            </div>
          </div>
          {#if mayManage(m)}
            {#if confirmRemove === m.id}
              <div class="confirm">
                <span class="note">Das Team bekommt einen neuen Schlüssel.</span>
                <button class="btn btn-small btn-danger" type="button" disabled={busy} onclick={() => removeMember(m)}>
                  {busy ? "Bitte warten …" : "Entfernen"}
                </button>
                <button class="btn btn-small" type="button" disabled={busy} onclick={() => (confirmRemove = null)}>Abbrechen</button>
              </div>
            {:else if confirmOwner === m.id}
              <div class="confirm">
                <span class="note">Sie werden danach Admin.</span>
                <button class="btn btn-small btn-danger" type="button" disabled={busy} onclick={() => changeRole(m, "Owner")}>
                  Zum Inhaber machen
                </button>
                <button class="btn btn-small" type="button" disabled={busy} onclick={() => (confirmOwner = null)}>Abbrechen</button>
              </div>
            {:else}
              <div class="item-actions">
                <select
                  aria-label="Rolle von {label(m)}"
                  value={m.role}
                  disabled={busy}
                  onchange={(e) => changeRole(m, e.currentTarget.value as Role)}
                >
                  <option value="Technician">Techniker</option>
                  <option value="Admin">Admin</option>
                </select>
                {#if isOwner}
                  <button class="btn btn-small" type="button" onclick={() => ((confirmOwner = m.id), (confirmRemove = null))}>Inhaber übergeben</button>
                {/if}
                <button class="btn btn-small" type="button" onclick={() => ((confirmRemove = m.id), (confirmOwner = null))}>Entfernen</button>
              </div>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>

    {#if canManage}
      <h3>Einladen</h3>
      <div class="invite-row">
        <label class="inline">
          <span>Rolle</span>
          <select bind:value={inviteRole} disabled={busy}>
            <option value="Technician">Techniker</option>
            <option value="Admin">Admin</option>
          </select>
        </label>
        <button class="btn btn-small" type="button" disabled={busy} onclick={newInvite}>
          {busy ? "Bitte warten …" : "Einladungscode erzeugen"}
        </button>
      </div>
      {#if invite}
        <p class="code" aria-label="Einladungscode">{invite}</p>
        <div class="code-actions">
          <button class="btn btn-small" type="button" onclick={copyInvite}>{copied ? "Kopiert" : "Kopieren"}</button>
        </div>
        <p class="hint">Gilt 10 Minuten und nur einmal. Bitte selbst weitergeben (Telefon, Chat), nicht per Mail.</p>
      {/if}
    {/if}

    <h3>Team-Adressbuch</h3>
    {#if devices.length === 0}
      <p class="empty">Das Team-Adressbuch ist leer.{canManage ? " Fügen Sie unten ein Gerät hinzu." : ""}</p>
    {:else}
      <ul class="list">
        {#each devices as [id, entry] (id)}
          <li>
            {#if editId === id}
              <form class="rename" onsubmit={saveEdit}>
                <input type="text" aria-label="Neuer Name" maxlength="64" bind:value={editText} disabled={busy} />
                <button class="btn btn-small btn-primary" type="submit" disabled={busy}>Speichern</button>
                <button class="btn btn-small" type="button" disabled={busy} onclick={() => (editId = null)}>Abbrechen</button>
              </form>
            {:else}
              <div class="item-main">
                <div class="item-text">
                  <div class="item-name">{entry.alias || entry.name || "Unbenanntes Gerät"}</div>
                  <div class="item-sub">{formatId(id)}</div>
                </div>
              </div>
              {#if canManage}
                {#if confirmEntry === id}
                  <div class="confirm">
                    <button class="btn btn-small btn-danger" type="button" disabled={busy} onclick={() => dropEntry(id)}>Entfernen</button>
                    <button class="btn btn-small" type="button" disabled={busy} onclick={() => (confirmEntry = null)}>Abbrechen</button>
                  </div>
                {:else}
                  <div class="item-actions">
                    <button class="btn btn-small" type="button" onclick={() => ((editId = id), (editText = entry.alias ?? entry.name ?? ""), (confirmEntry = null))}>
                      Umbenennen
                    </button>
                    <button class="btn btn-small" type="button" onclick={() => ((confirmEntry = id), (editId = null))}>Entfernen</button>
                  </div>
                {/if}
              {/if}
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if canManage}
      <form class="add" onsubmit={addDevice}>
        <input type="text" aria-label="Name des Geräts" maxlength="64" placeholder="Name" bind:value={addName} disabled={busy} />
        <input type="text" aria-label="Geräte-ID" inputmode="numeric" placeholder="Geräte-ID (9 Ziffern)" bind:value={addId} disabled={busy} />
        <button class="btn btn-small" type="submit" disabled={busy || !addValid}>Gerät hinzufügen</button>
      </form>
    {:else}
      <p class="hint">Als Techniker können Sie das Team-Adressbuch nur lesen.</p>
    {/if}

    {#if openHosts.length > 0}
      <h3>Geräte ohne Passwort</h3>
      <p class="hint">Mitglieder haben diese Geräte für das Team geöffnet.</p>
      <ul class="list">
        {#each openHosts as [id, host] (id)}
          <li>
            <div class="item-text">
              <div class="item-name">{team.book.book.entries[id]?.alias || team.book.book.entries[id]?.name || host.name || "Unbenanntes Gerät"}</div>
              <div class="item-sub">{formatId(id)} · ohne Passwort · freigegeben von {memberName(host.member)}</div>
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    <h3>Team verlassen oder löschen</h3>
    {#if isOwner}
      <p class="hint">
        Als Inhaber können Sie das Team erst verlassen, wenn Sie es einem Admin übergeben haben. Löschen entfernt das Team
        für alle Mitglieder, endgültig.
      </p>
      {#if !deleteOpen}
        <button class="btn btn-small btn-danger" type="button" onclick={() => (deleteOpen = true)}>Team löschen …</button>
      {:else}
        <form onsubmit={remove}>
          <label class="field">
            <span>Zur Bestätigung den Namen des Teams eintippen: {team.book.name}</span>
            <input type="text" autocomplete="off" bind:value={deleteName} disabled={busy} />
          </label>
          <div class="buttons">
            <button class="btn btn-danger" type="submit" disabled={busy || deleteName.trim() !== team.book.name.trim()}>
              {busy ? "Bitte warten …" : "Endgültig löschen"}
            </button>
            <button class="btn" type="button" disabled={busy} onclick={() => ((deleteOpen = false), (deleteName = ""))}>Abbrechen</button>
          </div>
        </form>
      {/if}
    {:else if !confirmLeave}
      <button class="btn btn-small" type="button" onclick={() => (confirmLeave = true)}>Team verlassen …</button>
    {:else}
      <div class="buttons">
        <button class="btn btn-danger" type="button" disabled={busy} onclick={leave}>
          {busy ? "Bitte warten …" : "Team wirklich verlassen"}
        </button>
        <button class="btn" type="button" disabled={busy} onclick={() => (confirmLeave = false)}>Abbrechen</button>
      </div>
    {/if}
  {/if}

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}
</section>

<style>
  h3 {
    margin: 24px 0 8px;
    font-size: 15px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .second {
    margin-top: 24px;
  }

  .team-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px 16px;
  }

  .team-name {
    font-size: 18px;
    font-weight: 600;
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .invite-row {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 12px;
  }

  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink-2);
    font-size: 14px;
  }

  .note {
    color: var(--ink-3);
  }

  select {
    height: 34px;
    padding: 0 8px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--ink);
    font: inherit;
    font-size: 14px;
  }

  .add {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }

  .add input {
    flex: 1 1 160px;
    min-width: 0;
  }
</style>
