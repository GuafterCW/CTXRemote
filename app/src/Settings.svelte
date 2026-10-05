<script lang="ts">
  import { untrack } from "svelte";
  import { api, errorText, RIGHT, type DirectSettings, type Profile } from "./lib/api";
  import Icon from "./lib/Icon.svelte";
  import ProfileCard from "./lib/ProfileCard.svelte";

  let {
    profile: initialProfile,
    server: initialServer,
    unattended,
    direct: initialDirect,
    directActive: initialActive,
    rightsAttended,
    rightsUnattended,
    service,
    version,
    onclose,
  }: {
    profile: Profile | null;
    server: string;
    unattended: boolean;
    direct: DirectSettings;
    directActive: boolean;
    rightsAttended: number;
    rightsUnattended: number;
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

  // What new sessions may do; changeable per session in the banner while it runs.
  let attended = $state(untrack(() => rightsAttended));
  let unattendedRights = $state(untrack(() => rightsUnattended));
  const rightsChanged = $derived(attended !== rightsAttended || unattendedRights !== rightsUnattended);
  const RIGHTS: [number, string][] = [
    [RIGHT.INPUT, "Maus und Tastatur"],
    [RIGHT.FILES, "Dateien"],
    [RIGHT.CLIPBOARD, "Zwischenablage"],
    [RIGHT.AUDIO, "Ton"],
    [RIGHT.RESTART, "Neu starten"],
    [RIGHT.PRIVACY, "Bildschirm schwarz schalten"],
  ];

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

  async function save(e: SubmitEvent) {
    e.preventDefault();
    saving = true;
    error = "";
    try {
      if (profileChanged && !(await saveProfile())) return;
      if (directChanged && !(await saveDirect())) return;
      if (settingsChanged) await api.saveSettings(server, passwordChange);
      if (rightsChanged) await api.saveRights(attended, unattendedRights);
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
      <h3>Rechte der Gegenseite</h3>
      <span class="note">
        Gilt für neue Sitzungen. Während einer Sitzung lässt sich das im Hauptfenster unter „Rechte“ ändern.
      </span>
      <div class="rights" role="table">
        <div class="rights-row head" role="row">
          <span role="columnheader"></span>
          <span role="columnheader">Einmal&shy;passwort</span>
          <span role="columnheader">Unbeaufsichtigt</span>
        </div>
        {#each RIGHTS as [bit, label] (bit)}
          <div class="rights-row" role="row">
            <span role="cell">{label}</span>
            <span role="cell">
              <input
                type="checkbox"
                aria-label={`${label}, mit Einmalpasswort`}
                checked={(attended & bit) !== 0}
                onchange={() => (attended ^= bit)}
              />
            </span>
            <span role="cell">
              <input
                type="checkbox"
                aria-label={`${label}, unbeaufsichtigt`}
                checked={(unattendedRights & bit) !== 0}
                onchange={() => (unattendedRights ^= bit)}
              />
            </span>
          </div>
        {/each}
      </div>
      <span class="note">
        „Unbeaufsichtigt“ gilt für das feste Passwort und für Geräte des Kontos. Bei Sitzungen mit Einmalpasswort sitzt
        meist jemand am Gerät, deshalb ist das Schwarzschalten dort standardmäßig aus.
      </span>
    </section>

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

  .rights {
    display: grid;
    gap: 2px;
  }

  .rights-row {
    display: grid;
    grid-template-columns: 1fr 110px 110px;
    align-items: center;
    min-height: 30px;
    font-size: 13.5px;
  }

  .rights-row span:not(:first-child) {
    text-align: center;
  }

  .rights-row.head {
    color: var(--ink-2);
    font-size: 12px;
  }
</style>
