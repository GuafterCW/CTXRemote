<script lang="ts">
  import { api, errorText, type Outcome, type ReportRow } from "./api";
  import { formatDate } from "./format";

  // Session reports of the team; shown to owners and admins only (the parent checks the role).
  let open = $state(false);
  let rows = $state<ReportRow[] | null>(null);
  let busy = $state(false);
  let error = $state("");

  let range = $state<"7" | "30" | "90" | "all">("30");
  let memberFilter = $state("");
  let search = $state("");

  const LABELS: Record<Outcome, string> = {
    oneTimePassword: "Einmalpasswort",
    permanentPassword: "Festes Passwort",
    account: "Konto, ohne Passwort",
    wrongPassword: "Falsches Passwort",
    notMember: "Kein Kontomitglied",
    declined: "Abgelehnt",
    wrongCode: "Falscher Code",
    team: "Teammitglied, ohne Passwort",
  };
  const refused = (o: Outcome | null) => o === "wrongPassword" || o === "notMember" || o === "declined" || o === "wrongCode";

  const memberName = (r: ReportRow) => r.memberEmail ?? (r.member !== null ? `Konto ${r.member}` : "");
  const startOf = (r: ReportRow) => r.report?.started || r.at;

  async function load() {
    busy = true;
    error = "";
    try {
      rows = await api.teamReports();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  function toggle() {
    open = !open;
    if (open && rows === null) load();
  }

  const members = $derived.by(() => {
    const seen = new Map<string, string>();
    for (const r of rows ?? []) if (r.member !== null) seen.set(String(r.member), memberName(r));
    return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1], "de"));
  });

  const filtered = $derived.by(() => {
    const cutoff = range === "all" ? 0 : Date.now() / 1000 - Number(range) * 86400;
    const needle = search.trim().toLowerCase();
    return (rows ?? []).filter((r) => {
      if (startOf(r) < cutoff) return false;
      if (memberFilter && String(r.member) !== memberFilter) return false;
      if (needle) {
        if (!r.report) return false;
        return `${r.report.deviceName}\n${r.report.peerName}`.toLowerCase().includes(needle);
      }
      return true;
    });
  });

  const readable = $derived(filtered.filter((r) => r.report));
  const admitted = (r: ReportRow) => !!r.report && !refused(r.report.outcome);
  const totalSeconds = $derived(
    filtered.reduce((sum, r) => (admitted(r) ? sum + Math.max(0, r.report!.ended - r.report!.started) : sum), 0),
  );

  /** "h:mm" */
  function hm(seconds: number): string {
    const minutes = Math.round(seconds / 60);
    return `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")}`;
  }

  const duration = (r: ReportRow) =>
    !r.report ? "" : refused(r.report.outcome) ? "abgelehnt" : hm(Math.max(0, r.report.ended - r.report.started));

  const kind = (r: ReportRow) => {
    const rep = r.report!;
    const direction = rep.side === "viewer" ? "Ausgehend" : "Eingehend";
    return rep.outcome ? `${direction} · ${LABELS[rep.outcome]}` : direction;
  };

  const who = (r: ReportRow) => {
    const rep = r.report!;
    if (rep.side === "viewer") return memberName(r) || "Unbekannt";
    return rep.profile ? `${rep.peerName || "Unbekannt"} (${rep.profile})` : rep.peerName || "Unbekannt";
  };

  const pad = (n: number) => String(n).padStart(2, "0");
  function stamp(seconds: number): string {
    const d = new Date(seconds * 1000);
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  }

  /** Quotes a CSV field; also defuses spreadsheet formulas. */
  function field(value: string | number | null): string {
    let text = value === null ? "" : String(value);
    if (/^[=+\-@\t\r]/.test(text)) text = `'${text}`;
    return /[;"\r\n]/.test(text) ? `"${text.replace(/"/g, '""')}"` : text;
  }

  /** Where the last export went. */
  let saved = $state<string | null>(null);

  function exportCsv() {
    const head = ["Beginn", "Ende", "Dauer (Minuten)", "Richtung", "Mitglied", "Gerät", "Gerät-ID", "Gegenstelle", "Gegenstelle-ID", "Art", "Profil"];
    const lines = [head.map(field).join(";")];
    for (const r of readable) {
      const rep = r.report!;
      lines.push(
        [
          stamp(rep.started),
          stamp(rep.ended),
          admitted(r) ? Math.round(Math.max(0, rep.ended - rep.started) / 60) : "",
          rep.side === "viewer" ? "Ausgehend" : "Eingehend",
          memberName(r),
          rep.deviceName,
          rep.device,
          rep.peerName,
          rep.peer,
          rep.outcome ? LABELS[rep.outcome] : "",
          rep.profile,
        ]
          .map(field)
          .join(";"),
      );
    }
    saved = null;
    api.teamSaveCsv(lines.join("\r\n") + "\r\n").then(
      (path) => (saved = `Gespeichert unter ${path}`),
      (e) => (error = errorText(e)),
    );
  }
</script>

<div class="reports">
  <div class="bar">
    <button type="button" class="link-btn quiet" aria-expanded={open} onclick={toggle}>
      {open ? "Sitzungsberichte ausblenden" : "Sitzungsberichte anzeigen"}
    </button>
    {#if open}
      <button type="button" class="link-btn" disabled={busy} onclick={load}>Aktualisieren</button>
    {/if}
  </div>

  {#if open}
    <span class="note">Berichte über Verbindungen von und zu Geräten des Teams. Nur Inhaber und Admins sehen sie.</span>

    {#if error}
      <p class="error">{error}</p>
    {/if}

    {#if rows === null}
      {#if busy}<span class="note">Wird geladen …</span>{/if}
    {:else}
      <div class="filters">
        <select class="sel" aria-label="Zeitraum" bind:value={range}>
          <option value="7">Letzte 7 Tage</option>
          <option value="30">Letzte 30 Tage</option>
          <option value="90">Letzte 90 Tage</option>
          <option value="all">Alle</option>
        </select>
        <select class="sel" aria-label="Mitglied" bind:value={memberFilter}>
          <option value="">Alle Mitglieder</option>
          {#each members as [id, name] (id)}
            <option value={id}>{name}</option>
          {/each}
        </select>
        <input class="field search" type="search" bind:value={search} placeholder="Gerät oder Gegenstelle suchen" aria-label="Suche" autocomplete="off" />
      </div>

      {#if filtered.length === 0}
        <span class="note">{rows.length === 0 ? "Noch keine Sitzungsberichte." : "Keine Sitzungen für diese Auswahl."}</span>
      {:else}
        <div class="scroll">
          <table>
            <thead>
              <tr><th>Beginn</th><th>Dauer</th><th>Wer</th><th>Gerät</th><th>Gegenstelle</th><th>Art</th></tr>
            </thead>
            <tbody>
              {#each filtered as r (r.seq)}
                {#if r.report}
                  <tr>
                    <td class="num">{formatDate(r.report.started)}</td>
                    <td class="num" class:bad={refused(r.report.outcome)}>{duration(r)}</td>
                    <td class="strong">{who(r)}</td>
                    <td>{r.report.deviceName || "–"}</td>
                    <td>{r.report.side === "viewer" ? r.report.peerName || "–" : "–"}</td>
                    <td class:bad={refused(r.report.outcome)}>{kind(r)}</td>
                  </tr>
                {:else}
                  <tr class="unreadable">
                    <td class="num">{formatDate(r.at)}</td>
                    <td colspan="5">Bericht nicht lesbar</td>
                  </tr>
                {/if}
              {/each}
            </tbody>
          </table>
        </div>
      {/if}

      <div class="foot">
        <span class="note">
          {readable.length} {readable.length === 1 ? "Sitzung" : "Sitzungen"} · Summe {hm(totalSeconds)} Std. (ohne abgelehnte)
        </span>
        <button type="button" class="btn btn-quiet small" disabled={readable.length === 0} onclick={exportCsv}>CSV exportieren</button>
      </div>
      {#if saved}
        <p class="note" role="status">{saved}</p>
      {/if}
    {/if}
  {/if}
</div>

<style>
  .reports {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .bar,
  .foot,
  .filters {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 14px;
  }

  .foot {
    justify-content: space-between;
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

  .sel {
    height: 28px;
    padding: 0 6px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--raised);
    font-size: 12.5px;
  }

  .search {
    flex: 1 1 160px;
    height: 28px;
    font-size: 12.5px;
  }

  .small {
    height: 32px;
    padding: 0 14px;
  }

  .scroll {
    overflow-x: auto;
    border-top: 1px solid var(--line);
  }

  table {
    width: 100%;
    min-width: 640px;
    border-collapse: collapse;
    font-size: 12.5px;
  }

  th {
    padding: 6px 10px 6px 0;
    color: var(--ink-3);
    font-size: 12px;
    font-weight: 600;
    text-align: left;
    white-space: nowrap;
  }

  td {
    padding: 8px 10px 8px 0;
    border-top: 1px solid var(--line);
    color: var(--ink-2);
    vertical-align: top;
    overflow-wrap: break-word;
  }

  .num {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .strong {
    color: var(--ink);
    font-weight: 600;
  }

  .bad {
    color: var(--bad);
  }

  .unreadable td {
    color: var(--ink-3);
    font-style: italic;
  }
</style>
