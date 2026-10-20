<script lang="ts">
  import { api, canManageTeam, errorText, type TeamInfo, type TeamMember, type TeamRole } from "./api";
  import Icon from "./Icon.svelte";
  import TeamPolicyPanel from "./TeamPolicy.svelte";
  import TeamReports from "./TeamReports.svelte";

  // The team section of the account panel; the account panel owns the team and hears about changes.
  let { team, onupdate }: { team: TeamInfo | null; onupdate: (team: TeamInfo | null) => void } = $props();

  let busy = $state(false);
  let error = $state("");

  // Without a team.
  let newName = $state("");
  let joinCode = $state("");

  // With a team.
  let renaming = $state(false);
  let nameDraft = $state("");
  let inviteRole = $state<"Admin" | "Technician">("Technician");
  let inviteCode = $state("");
  let copied = $state(false);
  /** A member action waiting for its confirmation. */
  let pending = $state<{ kind: "remove" | "handover"; member: TeamMember } | null>(null);
  let leaving = $state(false);
  let deleting = $state(false);
  let deleteName = $state("");

  const manage = $derived(canManageTeam(team));
  const owner = $derived(team?.role === "Owner");

  const ROLE: Record<TeamRole, string> = { Owner: "Inhaber", Admin: "Admin", Technician: "Techniker" };
  const dateFormat = new Intl.DateTimeFormat("de-DE", { day: "2-digit", month: "2-digit", year: "numeric" });
  const nameOf = (m: TeamMember) => m.email ?? `Konto ${m.id}`;

  async function run(action: () => Promise<unknown>) {
    busy = true;
    error = "";
    try {
      await action();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  const create = () =>
    run(async () => {
      onupdate(await api.teamCreate(newName));
      newName = "";
    });
  const join = () =>
    run(async () => {
      onupdate(await api.teamJoin(joinCode.trim()));
      joinCode = "";
    });
  const rename = () =>
    run(async () => {
      onupdate(await api.teamRename(nameDraft));
      renaming = false;
    });
  const invite = () => run(async () => (inviteCode = await api.teamInvite(inviteRole)));
  const setRole = (member: TeamMember, role: TeamRole) =>
    run(async () => {
      onupdate(await api.teamSetRole(member.id, role));
      pending = null;
    });
  const remove = (member: TeamMember) =>
    run(async () => {
      onupdate(await api.teamRemove(member.id));
      pending = null;
    });
  const leave = () =>
    run(async () => {
      await api.teamLeave();
      leaving = false;
      onupdate(null);
    });
  const destroy = () =>
    run(async () => {
      await api.teamDelete();
      deleting = false;
      deleteName = "";
      onupdate(null);
    });

  function startRename() {
    nameDraft = team?.name ?? "";
    renaming = true;
    error = "";
  }

  async function copyInvite() {
    await navigator.clipboard.writeText(inviteCode);
    copied = true;
    setTimeout(() => (copied = false), 1400);
  }

  /** Enter in a field of the panel must not submit the surrounding account form. */
  function enter(action: () => void) {
    return (e: KeyboardEvent) => {
      if (e.key !== "Enter") return;
      e.preventDefault();
      if (!busy) action();
    };
  }
</script>

<section class="group section">
  <h3>Team</h3>

  {#if !team}
    <span class="note">
      Ein Team teilt ein Adressbuch und lässt Mitglieder sich ohne Passwort mit Geräten verbinden, die das erlauben.
    </span>
    <input
      class="field"
      bind:value={newName}
      maxlength="40"
      placeholder="Name des Teams, z. B. Service Müller"
      autocomplete="off"
      onkeydown={enter(create)}
    />
    <button type="button" class="btn btn-primary start" disabled={busy || !newName.trim()} onclick={create}>Team gründen</button>

    <div class="or">oder</div>

    <span class="note">Den Code erzeugt ein Admin des Teams unter „Mitglied einladen“.</span>
    <input
      class="field code-field"
      bind:value={joinCode}
      placeholder="ABCD-EFGH-JKMN"
      spellcheck="false"
      autocomplete="off"
      onkeydown={enter(join)}
    />
    <button type="button" class="btn btn-quiet start" disabled={busy || joinCode.trim().length < 12} onclick={join}>
      Einem Team beitreten
    </button>
  {:else}
    <div class="head">
      {#if renaming}
        <input
          class="field rename"
          bind:value={nameDraft}
          maxlength="40"
          aria-label="Name des Teams"
          onkeydown={(e) => {
            if (e.key === "Escape") {
              e.stopPropagation();
              renaming = false;
            } else enter(rename)(e);
          }}
        />
        <button type="button" class="link-btn quiet" disabled={busy || !nameDraft.trim()} onclick={rename}>Speichern</button>
        <button type="button" class="link-btn" onclick={() => (renaming = false)}>Abbrechen</button>
      {:else}
        <span class="team-name">{team.name}</span>
        <span class="role-tag" class:owner>{ROLE[team.role]}</span>
        {#if manage}
          <button type="button" class="icon-btn small" title="Team umbenennen" aria-label="Team umbenennen" onclick={startRename}>
            <Icon name="pencil" size={15} />
          </button>
        {/if}
      {/if}
    </div>

    {#if team.error}
      <p class="error">Letzter Abgleich fehlgeschlagen: {team.error}</p>
    {/if}

    <ul class="members">
      {#each team.members as m (m.id)}
        <li>
          <div class="member-line">
            <span class="member-text">
              <span class="member-name">
                {nameOf(m)}{#if m.me}<span class="me">Sie</span>{/if}{#if !m.verified}<span
                    class="unverified"
                    title="Noch nicht bestätigt oder nicht vom Team ausgewiesen; wird beim nächsten Schlüsselwechsel entfernt"
                    >unbestätigt</span
                  >{/if}
              </span>
              <span class="member-meta">{ROLE[m.role]} · seit {dateFormat.format(new Date(m.since * 1000))}</span>
            </span>
            {#if manage && !m.me && m.role !== "Owner"}
              <select
                class="role-select"
                aria-label={`Rolle von ${nameOf(m)}`}
                value={m.role}
                disabled={busy}
                onchange={(e) => {
                  const role = e.currentTarget.value as TeamRole;
                  e.currentTarget.value = m.role;
                  setRole(m, role);
                }}
              >
                <option value="Admin">Admin</option>
                <option value="Technician">Techniker</option>
              </select>
              <button type="button" class="link-btn" disabled={busy} onclick={() => (pending = { kind: "remove", member: m })}>
                Entfernen
              </button>
              {#if owner && m.role === "Admin"}
                <button type="button" class="link-btn" disabled={busy} onclick={() => (pending = { kind: "handover", member: m })}>
                  Inhaber übergeben
                </button>
              {/if}
            {/if}
          </div>
          {#if pending && pending.member.id === m.id}
            <div class="confirm">
              {#if pending.kind === "remove"}
                <p>
                  {nameOf(m)} entfernen? Das Team bekommt dabei einen neuen Schlüssel; {nameOf(m)} kommt danach auf keine
                  Team-Geräte mehr.
                </p>
                <div class="row">
                  <button type="button" class="btn btn-danger small" disabled={busy} onclick={() => remove(m)}>Entfernen</button>
                  <button type="button" class="btn btn-quiet small" onclick={() => (pending = null)}>Abbrechen</button>
                </div>
              {:else}
                <p>
                  Das Team „{team.name}“ an {nameOf(m)} übergeben? Sie sind danach Admin und können das Team weder
                  löschen noch erneut übernehmen.
                </p>
                <div class="row">
                  <button type="button" class="btn btn-danger small" disabled={busy} onclick={() => setRole(m, "Owner")}>
                    Übergeben
                  </button>
                  <button type="button" class="btn btn-quiet small" onclick={() => (pending = null)}>Abbrechen</button>
                </div>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>

    {#if manage}
      {#if inviteCode}
        <div class="code-box">
          <span class="code">{inviteCode}</span>
          <span class="note">
            Gilt 10 Minuten und nur einmal. Bitte selbst weitergeben (Telefon, Chat), nicht per Mail.
          </span>
          <div class="row">
            <button type="button" class="btn btn-quiet small" onclick={copyInvite}>
              <Icon name={copied ? "check" : "copy"} size={14} />
              {copied ? "Kopiert" : "Kopieren"}
            </button>
            <button type="button" class="btn btn-quiet small" onclick={() => (inviteCode = "")}>Fertig</button>
          </div>
        </div>
      {:else}
        <div class="row invite">
          <select class="role-select" aria-label="Rolle des neuen Mitglieds" bind:value={inviteRole}>
            <option value="Technician">Techniker</option>
            <option value="Admin">Admin</option>
          </select>
          <button type="button" class="btn btn-quiet" disabled={busy} onclick={invite}>Mitglied einladen</button>
        </div>
      {/if}
    {/if}

    <TeamPolicyPanel {manage} />

    {#if manage}
      <TeamReports />
    {/if}

    {#if leaving}
      <div class="confirm">
        <p>
          Team „{team.name}“ verlassen? Sie sehen danach keine Team-Geräte mehr und können sich nicht mehr ohne
          Passwort mit ihnen verbinden.
        </p>
        <div class="row">
          <button type="button" class="btn btn-danger small" disabled={busy} onclick={leave}>Verlassen</button>
          <button type="button" class="btn btn-quiet small" onclick={() => (leaving = false)}>Abbrechen</button>
        </div>
      </div>
    {:else if deleting}
      <div class="confirm">
        <p>
          Das Team „{team.name}“ wird für alle Mitglieder gelöscht, samt geteiltem Adressbuch. Zur Bestätigung den
          Namen des Teams eintippen.
        </p>
        <input
          class="field"
          bind:value={deleteName}
          placeholder={team.name}
          aria-label="Name des Teams zur Bestätigung"
          autocomplete="off"
          onkeydown={enter(() => deleteName.trim() === team?.name && destroy())}
        />
        <div class="row">
          <button type="button" class="btn btn-danger small" disabled={busy || deleteName.trim() !== team.name} onclick={destroy}>
            Team löschen
          </button>
          <button type="button" class="btn btn-quiet small" onclick={() => ((deleting = false), (deleteName = ""))}>Abbrechen</button>
        </div>
      </div>
    {:else if owner}
      <span class="note">Als Inhaber können Sie das Team erst verlassen, wenn Sie es an einen Admin übergeben haben.</span>
      <button type="button" class="link-btn danger" disabled={busy} onclick={() => (deleting = true)}>Team löschen</button>
    {:else}
      <button type="button" class="link-btn" disabled={busy} onclick={() => (leaving = true)}>Team verlassen</button>
    {/if}
  {/if}

  {#if error}
    <p class="error">{error}</p>
  {/if}
</section>

<style>
  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .section {
    padding-top: 24px;
    border-top: 1px solid var(--line);
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

  .start {
    align-self: flex-start;
  }

  .or {
    color: var(--ink-3);
    font-size: 12px;
    text-align: center;
  }

  .code-field {
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 28px;
  }

  .team-name {
    min-width: 0;
    font-weight: 600;
    font-size: 15px;
    overflow-wrap: anywhere;
  }

  .rename {
    flex: 1;
    height: 32px;
    font-weight: 600;
  }

  .role-tag,
  .me,
  .unverified {
    flex: none;
    padding: 0 6px;
    border-radius: 999px;
    font-size: 11.5px;
    font-weight: 600;
  }

  .role-tag {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .role-tag.owner {
    border: 1px solid color-mix(in srgb, var(--accent) 40%, var(--line));
  }

  .me {
    margin-left: 8px;
    background: var(--accent-soft);
    color: var(--accent);
  }

  .unverified {
    margin-left: 8px;
    border: 1px solid var(--line-strong);
    color: var(--ink-3);
    font-weight: 400;
    cursor: help;
  }

  .members {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--line);
  }

  .members li {
    padding: 8px 0;
    border-bottom: 1px solid var(--line);
  }

  .member-line {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 12px;
    min-height: 36px;
  }

  .member-text {
    flex: 1 1 150px;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .member-name {
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .member-meta {
    color: var(--ink-3);
    font-size: 12px;
  }

  .member-line .link-btn {
    align-self: center;
  }

  .role-select {
    height: 28px;
    padding: 0 6px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--raised);
    font-size: 12.5px;
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .invite {
    align-items: center;
  }

  .confirm {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-top: 6px;
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

  .code-box {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 14px 16px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
  }

  .code {
    font: 600 20px/1.4 ui-monospace, "Cascadia Mono", Consolas, monospace;
    letter-spacing: 0.08em;
    overflow-wrap: anywhere;
    user-select: all;
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

  .link-btn.quiet {
    color: var(--accent);
  }

  .link-btn.danger {
    color: var(--bad);
  }

  .icon-btn.small {
    width: 28px;
    height: 28px;
  }
</style>
