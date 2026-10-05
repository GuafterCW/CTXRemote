<script lang="ts">
  let { code, onDone }: { code: string; onDone: () => void } = $props();

  let noted = $state(false);
  let copied = $state(false);
  let copyFailed = $state(false);

  async function copy() {
    copyFailed = false;
    try {
      await navigator.clipboard.writeText(code);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      copyFailed = true;
    }
  }
</script>

<div class="card card-narrow">
  <h1>Ihr Wiederherstellungscode</h1>
  <p class="code" aria-label="Wiederherstellungscode">{code}</p>
  <div class="code-actions">
    <button class="btn" type="button" onclick={copy}>
      <svg width="16" height="16" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="7" y="7" width="10" height="10" rx="2" />
        <path d="M13 7V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2" />
      </svg>
      {copied ? "Kopiert" : "Kopieren"}
    </button>
    {#if copyFailed}
      <span class="error" role="alert">Kopieren nicht möglich, bitte von Hand abschreiben.</span>
    {/if}
  </div>
  <p class="hint spaced">
    Notieren Sie diesen Code und bewahren Sie ihn sicher auf. Wenn Sie Ihr Passwort vergessen, ist ohne Passwort und
    ohne diesen Code keine Wiederherstellung möglich. Ein älterer Code ist damit ungültig.
  </p>
  <label class="check">
    <input type="checkbox" bind:checked={noted} />
    <span>Ich habe den Code sicher notiert</span>
  </label>
  <button class="btn btn-primary btn-block" type="button" disabled={!noted} onclick={onDone}>Weiter</button>
</div>
