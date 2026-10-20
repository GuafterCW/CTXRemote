<script lang="ts">
  import { onMount } from "svelte";
  import { ApiError } from "./lib/session";
  import { loadReports, type Outcome, type Report, type ReportRow } from "./lib/team";

  let { onExpired }: { onExpired: () => void } = $props();

  interface Line {
    seq: number;
    report: Report;
    who: string;
    refused: boolean;
  }

  let rows = $state<ReportRow[]>([]);
  let loaded = $state(false);
  let error = $state("");
  let days = $state("30");
  let who = $state("");
  let query = $state("");

  // Same wording as the app's history.
  const OUTCOME: Record<Outcome, string> = {
    oneTimePassword: "Einmalpasswort",
    permanentPassword: "Festes Passwort",
    account: "Konto, ohne Passwort",
    wrongPassword: "Falsches Passwort",
    notMember: "Kein Kontomitglied",
    declined: "Abgelehnt",
    wrongCode: "Falscher Code",
    team: "Teammitglied, ohne Passwort",
    policy: "Von der Teamrichtlinie abgelehnt",
  };
  const REFUSED: Outcome[] = ["wrongPassword", "notMember", "declined", "wrongCode", "policy"];

  onMount(async () => {
    try {
      rows = await loadReports();
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) onExpired();
      else error = err instanceof Error ? err.message : "Unbekannter Fehler";
    } finally {
      loaded = true;
    }
  });

  /** Who the session belongs to: the member (viewer side) or the one who connected (host side). */
  function whoOf(row: ReportRow, r: Report): string {
    if (r.side === "viewer") return row.memberEmail ?? (row.member !== null ? `Konto ${row.member}` : "Unbekannt");
    return r.profile || r.peerName || "Unbekannt";
  }

  const lines = $derived(
    rows
      .filter((row): row is ReportRow & { report: Report } => row.report !== null)
      .map((row): Line => ({
        seq: row.seq,
        report: row.report!,
        who: whoOf(row, row.report!),
        refused: row.report!.outcome !== null && REFUSED.includes(row.report!.outcome),
      }))
      .sort((a, b) => b.report.started - a.report.started || b.seq - a.seq),
  );
  const unreadable = $derived(rows.filter((r) => r.report === null).length);
  const people = $derived([...new Set(lines.map((l) => l.who))].sort((a, b) => a.localeCompare(b, "de")));

  const shown = $derived.by(() => {
    const since = days === "all" ? 0 : Date.now() / 1000 - Number(days) * 86400;
    const needle = query.trim().toLowerCase();
    return lines.filter((l) => {
      const r = l.report;
      if (r.started < since) return false;
      if (who && l.who !== who) return false;
      if (!needle) return true;
      const hay = [l.who, r.deviceName, r.peerName, r.profile ?? "", String(r.device ?? ""), String(r.peer ?? ""), outcomeText(r)]
        .join("\n")
        .toLowerCase();
      return hay.includes(needle);
    });
  });

  const minutesOf = (l: Line) => (l.refused ? 0 : Math.floor(Math.max(0, l.report.ended - l.report.started) / 60));
  const total = $derived(shown.reduce((sum, l) => sum + minutesOf(l), 0));

  function outcomeText(r: Report): string {
    return r.outcome ? OUTCOME[r.outcome] : "";
  }

  function hm(minutes: number): string {
    return `${Math.floor(minutes / 60)}:${String(minutes % 60).padStart(2, "0")}`;
  }

  function when(seconds: number): string {
    return new Date(seconds * 1000).toLocaleString("de-DE", { dateStyle: "short", timeStyle: "short" });
  }

  function formatId(id: number | null): string {
    if (id === null) return "";
    const s = String(id);
    return /^\d{9}$/.test(s) ? s.replace(/(\d{3})(?=\d)/g, "$1 ") : s;
  }

  const direction = (r: Report) => (r.side === "viewer" ? "Ausgehend" : "Eingehend");

  /** One CSV field, quoted when needed; a leading formula character is defused for spreadsheets. */
  function field(value: string | number): string {
    let s = String(value);
    if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`;
    return /[;"\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
  }

  function stamp(seconds: number): string {
    const d = new Date(seconds * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  function exportCsv() {
    const head = ["Beginn", "Ende", "Dauer (Minuten)", "Richtung", "Mitglied", "Gerät", "Gerät-ID", "Gegenstelle", "Gegenstelle-ID", "Art", "Profil"];
    const body = shown.map((l) => {
      const r = l.report;
      return [
        stamp(r.started),
        stamp(r.ended),
        l.refused ? "" : minutesOf(l),
        direction(r),
        l.who,
        r.deviceName,
        r.device ?? "",
        r.peerName,
        r.peer ?? "",
        outcomeText(r),
        r.profile ?? "",
      ];
    });
    const csv = [head, ...body].map((line) => line.map(field).join(";")).join("\r\n") + "\r\n";
    const blob = new Blob(["﻿", csv], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    const now = new Date();
    const p = (n: number) => String(n).padStart(2, "0");
    a.href = url;
    a.download = `remocto-sitzungen-${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}.csv`;
    document.body.append(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }
</script>

<h3>Sitzungsberichte</h3>
{#if !loaded}
  <p class="empty">Wird geladen …</p>
{:else if error}
  <p class="error" role="alert">{error}</p>
{:else if rows.length === 0}
  <p class="empty">Noch keine Sitzungsberichte.</p>
{:else}
  <p class="hint">Verbindungen von Mitgliedern und zu Geräten, die für das Team geöffnet sind. Nur Inhaber und Admins sehen sie.</p>
  <div class="filters">
    <label class="inline">
      <span>Zeitraum</span>
      <select bind:value={days}>
        <option value="7">7 Tage</option>
        <option value="30">30 Tage</option>
        <option value="90">90 Tage</option>
        <option value="all">Alle</option>
      </select>
    </label>
    <label class="inline">
      <span>Mitglied</span>
      <select bind:value={who}>
        <option value="">Alle</option>
        {#each people as p (p)}<option value={p}>{p}</option>{/each}
      </select>
    </label>
    <input type="search" aria-label="Suche" placeholder="Suchen …" bind:value={query} />
    <button class="btn btn-small" type="button" disabled={shown.length === 0} onclick={exportCsv}>CSV exportieren</button>
  </div>
  <p class="summary">{shown.length} {shown.length === 1 ? "Sitzung" : "Sitzungen"} · Summe {hm(total)} Std.</p>
  {#if shown.length === 0}
    <p class="empty">Keine Sitzungen mit diesen Filtern.</p>
  {:else}
    <div class="scroll">
      <table>
        <thead>
          <tr><th>Beginn</th><th>Dauer</th><th>Wer</th><th>Gerät</th><th>Gegenstelle</th><th>Art</th></tr>
        </thead>
        <tbody>
          {#each shown as l (l.seq)}
            {@const r = l.report}
            <tr>
              <td>{when(r.started)}</td>
              <td>{l.refused ? "abgelehnt" : hm(minutesOf(l))}</td>
              <td>{l.who}</td>
              <td>{r.deviceName || "–"}{#if r.device !== null}<div class="sub">{formatId(r.device)}</div>{/if}</td>
              <td>{r.peerName || "–"}{#if r.peer !== null}<div class="sub">{formatId(r.peer)}</div>{/if}</td>
              <td>{outcomeText(r) || "–"}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
  {#if unreadable > 0}
    <p class="hint">{unreadable} {unreadable === 1 ? "Bericht" : "Berichte"} nicht lesbar (Schlüssel einer früheren Epoche fehlt).</p>
  {/if}
{/if}

<style>
  h3 {
    margin: 24px 0 8px;
    font-size: 15px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 12px;
  }

  .filters input {
    flex: 1 1 160px;
    min-width: 0;
  }

  .inline {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--ink-2);
    font-size: 14px;
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

  .summary {
    margin: 12px 0 8px;
    color: var(--ink-2);
    font-size: 14px;
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
    vertical-align: top;
  }

  th {
    color: var(--ink-3);
    font-weight: 600;
    white-space: nowrap;
  }

  .sub {
    color: var(--ink-3);
    font-size: 13px;
  }
</style>
