<script lang="ts">
  import { untrack } from "svelte";
  import { api, errorText } from "./lib/api";
  import Icon from "./lib/Icon.svelte";

  let {
    server: initialServer,
    unattended,
    version,
    onclose,
  }: { server: string; unattended: boolean; version: string; onclose: () => void } = $props();

  // The form edits a copy; props are only the starting point.
  let server = $state(untrack(() => initialServer));
  let enableUnattended = $state(untrack(() => unattended));
  let newPassword = $state("");
  let saving = $state(false);
  let error = $state("");

  // An unchanged, still-enabled password is left alone (null); disabling sends "".
  const passwordChange = $derived(
    !enableUnattended ? (unattended ? "" : null) : newPassword ? newPassword : null,
  );
  const needsPassword = $derived(enableUnattended && !unattended && newPassword.length < 8);

  async function save(e: SubmitEvent) {
    e.preventDefault();
    saving = true;
    error = "";
    try {
      await api.saveSettings(server, passwordChange);
      onclose();
    } catch (err) {
      error = errorText(err);
    } finally {
      saving = false;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" role="presentation" onclick={onclose}></div>
<form class="sheet" onsubmit={save}>
  <header>
    <h2>Einstellungen</h2>
    <button type="button" class="icon-btn" title="Schließen" onclick={onclose}>
      <Icon name="close" />
    </button>
  </header>

  <div class="body">
    <label class="group">
      <span class="name">Server</span>
      <input class="field" bind:value={server} spellcheck="false" placeholder="server.example.de:21300" />
      <span class="note">Adresse Ihres CTXRemote-Servers. Ohne Port wird 21300 verwendet.</span>
    </label>

    <div class="group">
      <label class="toggle">
        <span>
          <span class="name">Unbeaufsichtigter Zugriff</span>
          <span class="note">Erlaubt Verbindungen mit einem festen Passwort, auch wenn niemand am Gerät ist.</span>
        </span>
        <input type="checkbox" class="switch" bind:checked={enableUnattended} />
      </label>

      {#if enableUnattended}
        <input
          class="field"
          type="password"
          bind:value={newPassword}
          autocomplete="new-password"
          placeholder={unattended ? "Neues Passwort (leer lassen = unverändert)" : "Festes Passwort, mind. 8 Zeichen"}
        />
      {/if}
    </div>

    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>

  <footer>
    <span class="version">CTXRemote {version}</span>
    <button type="button" class="btn btn-quiet" onclick={onclose}>Abbrechen</button>
    <button class="btn btn-primary" disabled={saving || needsPassword}>Speichern</button>
  </footer>
</form>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, #000 28%, transparent);
    animation: fade 160ms var(--ease);
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

  @keyframes fade {
    from {
      opacity: 0;
    }
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
    gap: 28px;
    padding: 24px;
    overflow-y: auto;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .name {
    display: block;
    font-weight: 600;
  }

  .note {
    display: block;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  .toggle {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
  }

  .switch {
    appearance: none;
    flex: none;
    position: relative;
    width: 36px;
    height: 20px;
    margin: 2px 0 0;
    border-radius: 10px;
    background: var(--line-strong);
    transition: background 140ms var(--ease);
    cursor: pointer;
  }

  .switch::after {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    transition: transform 140ms var(--ease);
  }

  .switch:checked {
    background: var(--accent);
  }

  .switch:checked::after {
    transform: translateX(16px);
  }

  .error {
    margin: 0;
    color: var(--bad);
    font-size: 13px;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 16px 24px;
    border-top: 1px solid var(--line);
  }

  .version {
    margin-right: auto;
    color: var(--ink-3);
    font-size: 12px;
  }
</style>
