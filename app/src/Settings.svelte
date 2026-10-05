<script lang="ts">
  import { untrack } from "svelte";
  import { api, errorText, type AccountView, type DirectSettings, type Profile } from "./lib/api";
  import Icon from "./lib/Icon.svelte";
  import ProfileCard from "./lib/ProfileCard.svelte";

  let {
    account: initialAccount,
    profile: initialProfile,
    server: initialServer,
    unattended,
    direct: initialDirect,
    directActive: initialActive,
    service,
    version,
    onclose,
  }: {
    account: AccountView | null;
    profile: Profile | null;
    server: string;
    unattended: boolean;
    direct: DirectSettings;
    directActive: boolean;
    service: boolean;
    version: string;
    onclose: () => void;
  } = $props();

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

  // Direct connections take effect at once and are saved only when changed.
  let savedDirect = $state(untrack(() => initialDirect));
  let directActive = $state(untrack(() => initialActive));
  let directEnabled = $state(untrack(() => initialDirect.enabled));
  let directPort = $state(String(untrack(() => initialDirect.port)));
  let directAddresses = $state(untrack(() => initialDirect.addresses.join("\n")));
  let directError = $state("");

  const directAddressList = $derived(directAddresses.split("\n").map((a) => a.trim()).filter(Boolean));
  const directChanged = $derived(
    directEnabled !== savedDirect.enabled ||
      String(directPort) !== String(savedDirect.port) ||
      directAddressList.join("\n") !== savedDirect.addresses.join("\n"),
  );
  // In service mode every save asks for administrator approval, so only what changed is sent.
  const settingsChanged = $derived(server.trim() !== initialServer || passwordChange !== null);

  const directStatus = $derived(
    !savedDirect.enabled
      ? "Aus"
      : directActive
        ? `Aktiv auf Port ${savedDirect.port}`
        : "Nicht aktiv (Port belegt?)",
  );

  /** Returns false if the direct settings were rejected; the error shows in their section. */
  async function saveDirect(): Promise<boolean> {
    directError = "";
    try {
      const port = Number(directPort);
      if (!Number.isInteger(port)) throw "Bitte einen Port als Zahl angeben";
      await api.saveDirect({ enabled: directEnabled, port, addresses: directAddressList });
      const overview = await api.overview();
      savedDirect = overview.direct;
      directActive = overview.directActive;
      return true;
    } catch (err) {
      directError = errorText(err);
      return false;
    }
  }

  // The profile is this user's own and needs no administrator rights.
  const emptyProfile: Profile = { name: "", company: "", message: "", logo: "" };
  const savedProfile = untrack(() => initialProfile ?? emptyProfile);
  let profile = $state<Profile>({ ...savedProfile });
  let profileError = $state("");
  let logoInput = $state<HTMLInputElement>();
  const profileChanged = $derived(
    (["name", "company", "message", "logo"] as const).some((k) => profile[k] !== savedProfile[k]),
  );
  const profileShown = $derived(profile.name.trim() !== "" || profile.company.trim() !== "");

  const MAX_LOGO = 64 * 1024;

  /** Scales the picked image down to a small PNG that fits the protocol limit. */
  async function pickLogo(e: Event) {
    const file = (e.currentTarget as HTMLInputElement).files?.[0];
    (e.currentTarget as HTMLInputElement).value = "";
    if (!file) return;
    profileError = "";
    try {
      const bitmap = await createImageBitmap(file);
      for (const size of [128, 96, 64]) {
        const scale = Math.min(1, size / Math.max(bitmap.width, bitmap.height));
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(bitmap.width * scale));
        canvas.height = Math.max(1, Math.round(bitmap.height * scale));
        canvas.getContext("2d")!.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        const base64 = canvas.toDataURL("image/png").split(",")[1] ?? "";
        if ((base64.length * 3) / 4 <= MAX_LOGO) {
          profile.logo = base64;
          return;
        }
      }
      profileError = "Das Bild ist zu detailreich für ein Logo";
    } catch {
      profileError = "Das Bild konnte nicht gelesen werden";
    }
  }

  async function saveProfile(): Promise<boolean> {
    profileError = "";
    try {
      await api.saveProfile(profile);
      return true;
    } catch (err) {
      profileError = errorText(err);
      return false;
    }
  }

  // Account actions take effect at once, independent of "Speichern".
  let account = $state<AccountView | null>(untrack(() => initialAccount));
  let accountBusy = $state(false);
  let accountError = $state("");
  let joining = $state(false);
  let joinCode = $state("");
  let pairingCode = $state("");

  async function accountAction(action: () => Promise<unknown>) {
    accountBusy = true;
    accountError = "";
    try {
      await action();
      account = (await api.overview()).account ?? null;
    } catch (err) {
      accountError = errorText(err);
    } finally {
      accountBusy = false;
    }
  }

  const createAccount = () => accountAction(() => api.accountCreate());
  const joinAccount = () =>
    accountAction(async () => {
      await api.accountJoin(joinCode);
      joining = false;
      joinCode = "";
    });
  const showPairingCode = () => accountAction(async () => (pairingCode = await api.accountPairingCode()));
  const leaveAccount = () =>
    accountAction(async () => {
      await api.accountLeave();
      pairingCode = "";
    });

  async function save(e: SubmitEvent) {
    e.preventDefault();
    saving = true;
    error = "";
    try {
      if (profileChanged && !(await saveProfile())) return;
      if (directChanged && !(await saveDirect())) return;
      if (settingsChanged) await api.saveSettings(server, passwordChange);
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
    {#if service}
      <p class="note">
        Diese Einstellungen gelten für den CTXRemote-Dienst dieses Geräts. Zum Speichern sind
        Administratorrechte nötig.
      </p>
    {/if}

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

    <section class="group section">
      <h3>Direktverbindung</h3>
      <label class="toggle">
        <span>
          <span class="name">Direkte Verbindungen anbieten</span>
          <span class="note">
            Sitzungen wechseln nach dem Aufbau auf eine direkte Verbindung, wenn sie erreichbar ist.
          </span>
        </span>
        <input type="checkbox" class="switch" bind:checked={directEnabled} />
      </label>

      <label class="group">
        <span class="name">Port</span>
        <input
          class="field"
          type="number"
          min="1024"
          max="65535"
          bind:value={directPort}
        />
        <span class="note">Für Verbindungen aus dem Internet am Router weiterleiten.</span>
      </label>

      <label class="group">
        <span class="name">Zusätzliche Adressen</span>
        <textarea
          class="field area"
          rows="3"
          bind:value={directAddresses}
          spellcheck="false"
          placeholder="meinhaus.dyndns.org:21301"
        ></textarea>
        <span class="note">Eine Adresse pro Zeile, z. B. meinhaus.dyndns.org:21301 bei Portweiterleitung.</span>
      </label>

      <span class="note status">{directStatus}</span>
      {#if directError}
        <p class="error">{directError}</p>
      {/if}
    </section>

    <section class="group section">
      <h3>Konto und Geräteliste</h3>
      {#if account}
        <span class="note status">
          Verbunden · {account.devices === 1 ? "1 Gerät" : `${account.devices} Geräte`} im Konto
        </span>
        <span class="note">
          Ihre Geräteliste mit allen Namen ist auf allen Geräten des Kontos gleich. Sie wird verschlüsselt
          übertragen, der Server kann sie nicht lesen.
        </span>
        {#if account.error}
          <p class="error">Letzter Abgleich fehlgeschlagen: {account.error}</p>
        {/if}
        {#if pairingCode}
          <div class="code-box">
            <span class="code">{pairingCode}</span>
            <span class="note">
              Auf dem anderen Gerät unter Einstellungen → „Mit Code verbinden“ eingeben. Gilt 10 Minuten und
              nur einmal.
            </span>
          </div>
        {:else}
          <div class="logo-row">
            <button type="button" class="btn btn-quiet" disabled={accountBusy} onclick={showPairingCode}>
              Gerät hinzufügen
            </button>
          </div>
        {/if}
        <button type="button" class="link-btn" disabled={accountBusy} onclick={leaveAccount}>
          Dieses Gerät vom Konto abmelden
        </button>
      {:else}
        <span class="note">
          Mit einem Konto ist Ihre Geräteliste samt Namen auf allen Ihren PCs gleich. Ohne E-Mail und Passwort:
          weitere Geräte verbinden Sie mit einem Einmal-Code.
        </span>
        {#if joining}
          <div class="logo-row">
            <input
              class="field code-field"
              bind:value={joinCode}
              placeholder="ABCD-EFGH-JKMN"
              spellcheck="false"
              autocomplete="off"
            />
            <button type="button" class="btn btn-primary" disabled={accountBusy || joinCode.trim().length < 12} onclick={joinAccount}>
              Verbinden
            </button>
          </div>
          <span class="note">Den Code zeigt ein Gerät, das schon im Konto ist, unter „Gerät hinzufügen“.</span>
        {:else}
          <div class="logo-row">
            <button type="button" class="btn btn-quiet" disabled={accountBusy} onclick={createAccount}>Konto anlegen</button>
            <button type="button" class="btn btn-quiet" disabled={accountBusy} onclick={() => (joining = true)}>
              Mit Code verbinden
            </button>
          </div>
        {/if}
      {/if}
      {#if accountError}
        <p class="error">{accountError}</p>
      {/if}
    </section>

    <section class="group section">
      <h3>Ihr Profil</h3>
      <span class="note">
        Wenn Sie sich mit einem anderen Gerät verbinden, sieht die Person dort diese Angaben, z. B. in der
        Zugriffsanfrage der Schnellhilfe. Gilt nur für Sie.
      </span>

      <label class="group">
        <span class="name">Name</span>
        <input class="field" bind:value={profile.name} maxlength="60" placeholder="Max Mustermann" />
      </label>
      <label class="group">
        <span class="name">Firma</span>
        <input class="field" bind:value={profile.company} maxlength="80" placeholder="Muster IT-Service" />
      </label>
      <label class="group">
        <span class="name">Nachricht</span>
        <textarea
          class="field area"
          rows="2"
          bind:value={profile.message}
          maxlength="300"
          placeholder="Ich helfe Ihnen heute bei …"
        ></textarea>
      </label>

      <div class="group">
        <span class="name">Logo</span>
        <div class="logo-row">
          <input bind:this={logoInput} type="file" accept="image/png,image/jpeg,image/webp" hidden onchange={pickLogo} />
          <button type="button" class="btn btn-quiet" onclick={() => logoInput?.click()}>
            {profile.logo ? "Anderes Logo" : "Logo wählen"}
          </button>
          {#if profile.logo}
            <button type="button" class="btn btn-quiet" onclick={() => (profile.logo = "")}>Entfernen</button>
          {/if}
        </div>
        <span class="note">PNG, JPG oder WebP. Wird auf höchstens 128 × 128 Pixel verkleinert.</span>
      </div>

      {#if profileShown}
        <div class="group">
          <span class="note">So sieht es die Gegenseite:</span>
          <ProfileCard {profile} />
        </div>
      {/if}
      {#if profileError}
        <p class="error">{profileError}</p>
      {/if}
    </section>
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

  .section {
    padding-top: 24px;
    border-top: 1px solid var(--line);
    gap: 16px;
  }

  h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--ink-2);
  }

  .area {
    height: auto;
    padding: 10px 14px;
    line-height: 1.4;
    resize: vertical;
    font-family: inherit;
  }

  .status {
    color: var(--ink-2);
  }

  .logo-row {
    display: flex;
    gap: 8px;
  }

  .code-field {
    flex: 1;
    min-width: 0;
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
    font: 600 22px/1.2 ui-monospace, "Cascadia Mono", Consolas, monospace;
    letter-spacing: 0.08em;
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

  .link-btn:hover {
    color: var(--bad);
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
