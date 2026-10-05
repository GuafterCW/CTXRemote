<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { api, errorText, type AccountDetails, type AccountView } from "./lib/api";
  import Icon from "./lib/Icon.svelte";

  let { account: initial, onclose }: { account: AccountView | null; onclose: () => void } = $props();

  let account = $state<AccountView | null>(untrack(() => initial));
  let access = $state(false);
  let details = $state<AccountDetails | null>(null);
  let busy = $state(false);
  let error = $state("");

  type Mode = "login" | "register" | "pair" | "recover";
  let mode = $state<Mode>("login");
  let email = $state("");
  let password = $state("");
  let password2 = $state("");
  let code = $state("");

  // Shown once after registering or a new password; the user confirms it is noted.
  let recoveryCode = $state("");
  let recoveryNoted = $state(false);
  let pairingCode = $state("");
  let editingLogin = $state(false);
  let removing = $state<string | null>(null);

  const MIN_PASSWORD = 10;
  const passwordProblem = $derived(
    password.length > 0 && password.length < MIN_PASSWORD
      ? `Mindestens ${MIN_PASSWORD} Zeichen`
      : password2.length > 0 && password !== password2
        ? "Die Passwörter stimmen nicht überein"
        : "",
  );

  async function refresh() {
    const overview = await api.overview();
    account = overview.account ?? null;
    access = overview.accountAccess;
    details = null;
    if (account) {
      try {
        details = await api.accountDetails();
        if (details.email && !email) email = details.email;
      } catch (err) {
        error = errorText(err);
        // Removed from the account or the account deleted: show the login again.
        account = (await api.overview()).account ?? null;
      }
    }
  }

  onMount(refresh);

  async function run(action: () => Promise<unknown>) {
    busy = true;
    error = "";
    try {
      await action();
      password = "";
      password2 = "";
      code = "";
      await refresh();
    } catch (err) {
      error = errorText(err);
    } finally {
      busy = false;
    }
  }

  const login = () => run(() => api.accountLogin(email, password));
  const register = () => run(async () => (recoveryCode = await api.accountRegister(email, password)));
  const join = () => run(() => api.accountJoin(code));
  const recover = () => run(async () => (recoveryCode = await api.accountRecover(email, code, password)));
  const createWithoutLogin = () => run(() => api.accountCreate());
  const setLogin = () =>
    run(async () => {
      recoveryCode = await api.accountSetLogin(email, password);
      editingLogin = false;
    });
  const setAccess = (enabled: boolean) => run(() => api.accountSetAccess(enabled));
  const showPairing = () => run(async () => (pairingCode = await api.accountPairingCode()));
  const remove = (key: string) =>
    run(async () => {
      await api.accountRemoveDevice(key);
      removing = null;
    });
  const leave = () =>
    run(async () => {
      await api.accountLeave();
      pairingCode = "";
      details = null;
    });

  function submit(e: SubmitEvent) {
    e.preventDefault();
    if (busy) return;
    if (editingLogin) return setLogin();
    if (mode === "login") return login();
    if (mode === "register") return register();
    if (mode === "pair") return join();
    return recover();
  }

  const canSubmit = $derived(
    !busy &&
      (mode === "pair" && !account
        ? code.trim().length >= 12
        : mode === "login" && !account && !editingLogin
          ? email.includes("@") && password.length > 0
          : email.includes("@") && password.length >= MIN_PASSWORD && password === password2 &&
            (mode !== "recover" || account !== null || code.trim().length >= 25)),
  );

  function setMode(next: Mode) {
    mode = next;
    error = "";
  }

  async function copyCode() {
    await navigator.clipboard.writeText(recoveryCode);
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && !recoveryCode && onclose()} />

<div class="scrim" role="presentation" onclick={() => !recoveryCode && onclose()}></div>
<form class="sheet" onsubmit={submit}>
  <header>
    <h2>Konto</h2>
    <button type="button" class="icon-btn" title="Schließen" disabled={!!recoveryCode} onclick={onclose}>
      <Icon name="close" />
    </button>
  </header>

  <div class="body">
    {#if recoveryCode}
      <section class="group">
        <h3>Ihr Wiederherstellungscode</h3>
        <div class="code-box">
          <span class="code">{recoveryCode}</span>
          <button type="button" class="link-btn quiet" onclick={copyCode}>Kopieren</button>
        </div>
        <p class="note">
          Notieren Sie den Code und bewahren Sie ihn sicher auf. Mit ihm setzen Sie ein neues Passwort, falls Sie
          es vergessen. Ihre Daten sind Ende-zu-Ende verschlüsselt: Ohne Passwort und ohne diesen Code kann
          niemand, auch wir nicht, Ihr Konto wiederherstellen. Ein älterer Code gilt ab jetzt nicht mehr.
        </p>
        <label class="check">
          <input type="checkbox" bind:checked={recoveryNoted} />
          <span>Ich habe den Code sicher notiert</span>
        </label>
        <button
          type="button"
          class="btn btn-primary"
          disabled={!recoveryNoted}
          onclick={() => {
            recoveryCode = "";
            recoveryNoted = false;
          }}>Fertig</button
        >
      </section>
    {:else if account}
      <section class="group">
        <span class="name">{details?.email ?? "Konto ohne Anmeldung"}</span>
        <span class="note">
          {#if details && !details.email}
            Geräte kommen nur per Code dazu. Mit E-Mail und Passwort melden Sie sich auch auf neuen Geräten und
            im Web an.
          {:else if details && !details.verified}
            E-Mail-Adresse noch nicht bestätigt.
          {:else}
            Geräteliste und Namen sind auf allen Geräten gleich, Ende-zu-Ende verschlüsselt.
          {/if}
        </span>
        {#if account.error}
          <p class="error">Letzter Abgleich fehlgeschlagen: {account.error}</p>
        {/if}
      </section>

      <section class="group section">
        <h3>Geräte im Konto</h3>
        {#if details}
          <ul class="devices">
            {#each details.devices as device (device.publicKey)}
              <li>
                <span class="dot" class:online={device.online} title={device.online ? "Online" : "Offline"}></span>
                <span class="device-text">
                  <span class="device-name">{device.name || "Unbenanntes Gerät"}</span>
                  <span class="device-id">{device.this ? "Dieses Gerät" : device.id ?? "ohne ID"}</span>
                </span>
                {#if !device.this}
                  {#if removing === device.publicKey}
                    <button type="button" class="link-btn danger" disabled={busy} onclick={() => remove(device.publicKey)}>Entfernen</button>
                    <button type="button" class="link-btn" onclick={() => (removing = null)}>Abbrechen</button>
                  {:else}
                    <button type="button" class="icon-btn" title="Aus dem Konto entfernen" onclick={() => (removing = device.publicKey)}>
                      <Icon name="trash" size={16} />
                    </button>
                  {/if}
                {/if}
              </li>
            {/each}
          </ul>
        {:else if error}
          <span class="note">Nicht geladen</span>
        {:else}
          <span class="note">Wird geladen …</span>
        {/if}
        {#if pairingCode}
          <div class="code-box">
            <span class="code">{pairingCode}</span>
            <span class="note">
              Auf dem neuen Gerät unter Konto → „Mit Code verbinden“ eingeben. Gilt 10 Minuten und nur einmal.
            </span>
          </div>
        {:else}
          <button type="button" class="btn btn-quiet start" disabled={busy} onclick={showPairing}>Gerät per Code hinzufügen</button>
        {/if}
      </section>

      <section class="group section">
        <h3>Zugriff ohne Passwort</h3>
        <label class="check top">
          <input
            type="checkbox"
            checked={access}
            disabled={busy}
            onchange={(e) => {
              // Keep the box in step with the stored value until the refresh arrives.
              const enabled = e.currentTarget.checked;
              e.currentTarget.checked = access;
              setAccess(enabled);
            }}
          />
          <span>Geräte dieses Kontos dürfen sich ohne Passwort mit diesem Gerät verbinden</span>
        </label>
        <span class="note">
          Für unbeaufsichtigten Zugriff, z. B. auf den eigenen Büro-PC. Nur Geräte, die zu diesem Zeitpunkt noch im
          Konto sind, kommen rein. Mit dem CTXRemote-Dienst fragt Windows nach Administratorrechten.
        </span>
      </section>

      <section class="group section">
        <h3>Anmeldung</h3>
        {#if editingLogin}
          <input class="field" type="email" bind:value={email} placeholder="E-Mail-Adresse" autocomplete="email" />
          <input class="field" type="password" bind:value={password} placeholder="Neues Passwort" autocomplete="new-password" />
          <input class="field" type="password" bind:value={password2} placeholder="Passwort wiederholen" autocomplete="new-password" />
          {#if passwordProblem}<span class="note bad">{passwordProblem}</span>{/if}
          <div class="row">
            <button class="btn btn-primary" disabled={!canSubmit}>Speichern</button>
            <button type="button" class="btn btn-quiet" onclick={() => (editingLogin = false)}>Abbrechen</button>
          </div>
          <span class="note">Sie erhalten danach einen neuen Wiederherstellungscode.</span>
        {:else}
          <button type="button" class="btn btn-quiet start" onclick={() => (editingLogin = true)}>
            {details?.email ? "Passwort oder E-Mail ändern" : "E-Mail und Passwort festlegen"}
          </button>
        {/if}
        <button type="button" class="link-btn" disabled={busy} onclick={leave}>Dieses Gerät vom Konto abmelden</button>
      </section>
    {:else}
      <nav class="tabs" aria-label="Kontoart">
        <button type="button" class:active={mode === "login"} onclick={() => setMode("login")}>Anmelden</button>
        <button type="button" class:active={mode === "register"} onclick={() => setMode("register")}>Registrieren</button>
        <button type="button" class:active={mode === "pair"} onclick={() => setMode("pair")}>Mit Code</button>
      </nav>

      {#if mode === "pair"}
        <section class="group">
          <span class="note">Den Code zeigt ein Gerät, das schon im Konto ist, unter „Gerät per Code hinzufügen“.</span>
          <input class="field code-field" bind:value={code} placeholder="ABCD-EFGH-JKMN" spellcheck="false" autocomplete="off" />
          <button class="btn btn-primary" disabled={!canSubmit}>Verbinden</button>
          <button type="button" class="link-btn" disabled={busy} onclick={createWithoutLogin}>
            Neues Konto ohne E-Mail anlegen
          </button>
        </section>
      {:else}
        <section class="group">
          {#if mode === "register"}
            <span class="note">
              Ihre Geräteliste wird auf Ihren Geräten verschlüsselt. Der Server kennt weder Ihr Passwort noch Ihre
              Daten.
            </span>
          {:else if mode === "recover"}
            <span class="note">Mit dem Wiederherstellungscode setzen Sie ein neues Passwort.</span>
          {/if}
          <input class="field" type="email" bind:value={email} placeholder="E-Mail-Adresse" autocomplete="email" />
          {#if mode === "recover"}
            <input class="field code-field" bind:value={code} placeholder="Wiederherstellungscode" spellcheck="false" autocomplete="off" />
          {/if}
          <input
            class="field"
            type="password"
            bind:value={password}
            placeholder={mode === "login" ? "Passwort" : "Neues Passwort"}
            autocomplete={mode === "login" ? "current-password" : "new-password"}
          />
          {#if mode !== "login"}
            <input class="field" type="password" bind:value={password2} placeholder="Passwort wiederholen" autocomplete="new-password" />
            {#if passwordProblem}<span class="note bad">{passwordProblem}</span>{/if}
          {/if}
          <button class="btn btn-primary" disabled={!canSubmit}>
            {mode === "login" ? "Anmelden" : mode === "register" ? "Konto erstellen" : "Neues Passwort setzen"}
          </button>
          {#if mode === "login"}
            <button type="button" class="link-btn" onclick={() => setMode("recover")}>Passwort vergessen?</button>
          {/if}
        </section>
      {/if}
    {/if}

    {#if error}
      <p class="error">{error}</p>
    {/if}
  </div>
</form>

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

  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 24px;
    padding: 24px;
    overflow-y: auto;
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

  .name {
    font-weight: 600;
    font-size: 15px;
    overflow-wrap: anywhere;
  }

  .note {
    margin: 0;
    color: var(--ink-3);
    font-size: 12.5px;
  }

  .note.bad,
  .error {
    color: var(--bad);
  }

  .error {
    margin: 0;
    font-size: 13px;
  }

  .tabs {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    border: 1px solid var(--line);
    border-radius: var(--radius);
    overflow: hidden;
  }

  .tabs button {
    height: 36px;
    border: 0;
    background: none;
    color: var(--ink-2);
    font-weight: 600;
  }

  .tabs button + button {
    border-left: 1px solid var(--line);
  }

  .tabs button.active {
    background: var(--accent-soft);
    color: var(--accent);
  }

  .row {
    display: flex;
    gap: 8px;
  }

  .start {
    align-self: flex-start;
  }

  .code-field {
    font-family: ui-monospace, "Cascadia Mono", Consolas, monospace;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .code-box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 14px 16px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
  }

  .code {
    font: 600 17px/1.4 ui-monospace, "Cascadia Mono", Consolas, monospace;
    letter-spacing: 0.06em;
    overflow-wrap: anywhere;
    user-select: all;
  }

  .check {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .check.top {
    align-items: flex-start;
  }

  .devices {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--line);
  }

  .devices li {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 48px;
    border-bottom: 1px solid var(--line);
  }

  .dot {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--line-strong);
  }

  .dot.online {
    background: var(--ok);
  }

  .device-text {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .device-name {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .device-id {
    color: var(--ink-3);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
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
</style>
