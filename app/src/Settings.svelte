<script lang="ts">
  import { untrack } from "svelte";
  import { api, errorText, type DirectSettings, type Profile, type TeamPolicy } from "./lib/api";
  import { RIGHTS } from "./lib/rights";
  import Icon from "./lib/Icon.svelte";
  import ProfileCard from "./lib/ProfileCard.svelte";
  import RiskConfirm from "./lib/RiskConfirm.svelte";
  import { RISKY } from "./lib/risky";
  import { IDLE_KEY, readIdleMinutes } from "./lib/idle";

  let {
    profile: initialProfile,
    server: initialServer,
    unattended,
    direct: initialDirect,
    directActive: initialActive,
    rightsAttended,
    rightsUnattended,
    codeEnabled: initialCode,
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
    codeEnabled: boolean;
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

  // Two-factor for the permanent password; applied at once, not with the form.
  let codeEnabled = $state(untrack(() => initialCode));
  let codeSetup = $state<{ secret: string; uri: string; qr: string } | null>(null);
  let codeConfirm = $state("");
  let codeBusy = $state(false);
  let codeError = $state("");

  async function startCode() {
    codeError = "";
    try {
      codeSetup = await api.codeSetup();
      codeConfirm = "";
    } catch (err) {
      codeError = errorText(err);
    }
  }

  async function enableCode() {
    if (!codeSetup) return;
    codeBusy = true;
    codeError = "";
    try {
      await api.codeEnable(codeSetup.secret, codeConfirm);
      codeEnabled = true;
      codeSetup = null;
    } catch (err) {
      codeError = errorText(err);
    } finally {
      codeBusy = false;
    }
  }

  async function disableCode() {
    codeBusy = true;
    codeError = "";
    try {
      await api.codeEnable(null, null);
      codeEnabled = false;
    } catch (err) {
      codeError = errorText(err);
    } finally {
      codeBusy = false;
    }
  }

  // What new sessions may do; changeable per session in the banner while it runs.
  let attended = $state(untrack(() => rightsAttended));
  let unattendedRights = $state(untrack(() => rightsUnattended));
  const rightsChanged = $derived(attended !== rightsAttended || unattendedRights !== rightsUnattended);
  /** A risky right waiting to be confirmed before it is ticked. */
  let asking = $state<{ bit: number; apply: () => void } | null>(null);

  /** Ticks or unticks a right; switching on a risky one asks first. */
  function flip(column: "attended" | "unattended", bit: number, box: HTMLInputElement) {
    const apply = () => (column === "attended" ? (attended ^= bit) : (unattendedRights ^= bit));
    if (box.checked && RISKY[bit]) {
      // Stays unticked unless confirmed.
      box.checked = false;
      asking = { bit, apply };
    } else {
      apply();
    }
  }

  // The team policy this device follows, if it is released for the team; local settings stay as saved.
  let teamPolicy = $state<TeamPolicy | null>(null);
  api.devicePolicy().then(
    (p) => (teamPolicy = p),
    () => {},
  );

  const names = (mask: number) => RIGHTS.filter(([bit]) => (mask & bit) !== 0).map(([, label]) => label);

  /** What the policy switches off, one line each. */
  const policyLines = $derived.by(() => {
    const p = teamPolicy;
    if (!p) return [];
    const lines: string[] = [];
    if (!p.oneTime) lines.push("Kein Einmalpasswort");
    if (!p.permanent) lines.push("Kein festes Passwort");
    else if (p.requireCode) lines.push("Festes Passwort nur mit Bestätigungscode");
    if (!p.account) lines.push("Kein Zugriff über Geräte des eigenen Kontos");
    const groups: [string, number][] = [
      ["Techniker", p.denyTechnician],
      ["Inhaber und Admins", p.denyAdmin],
      ["Sitzungen ohne Team", p.denyOthers],
    ];
    for (const [who, mask] of groups) if (mask) lines.push(`${who} ohne: ${names(mask).join(", ")}`);
    return lines;
  });

  /** "locked" if the team withdraws the right from everyone, "limited" if from some. */
  function teamState(bit: number): "locked" | "limited" | null {
    const p = teamPolicy;
    if (!p) return null;
    const hits = [p.denyTechnician, p.denyAdmin, p.denyOthers].filter((m) => (m & bit) !== 0).length;
    return hits === 3 ? "locked" : hits > 0 ? "limited" : null;
  }

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

  // Kept in this computer's browser storage, read by each session window.
  let idleMinutes = $state(readIdleMinutes());

  // Applies to sessions opened from now on; saved at once like the idle time.
  let separateWindows = $state(false);
  api.separateWindows().then((on) => (separateWindows = on)).catch(() => {});

  function setSeparateWindows(on: boolean) {
    separateWindows = on;
    api.setSeparateWindows(on).catch((e) => (error = errorText(e)));
  }

  function setIdleMinutes(minutes: number) {
    idleMinutes = minutes;
    try {
      localStorage.setItem(IDLE_KEY, String(minutes));
    } catch {
      // Not remembered.
    }
  }
</script>

<!-- With a question open, Escape only closes that. -->
<svelte:window onkeydown={(e) => e.key === "Escape" && !asking && onclose()} />

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
        Diese Einstellungen gelten für den Remocto-Dienst dieses Geräts. Zum Speichern sind
        Administratorrechte nötig.
      </p>
    {/if}

    <label class="group">
      <span class="name">Server</span>
      <input class="field" bind:value={server} spellcheck="false" placeholder="server.example.de:21300" />
      <span class="note">Adresse Ihres Remocto-Servers. Ohne Port wird 21300 verwendet.</span>
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

      {#if unattended && enableUnattended}
        <div class="code-box">
          <div class="toggle">
            <span>
              <span class="name">Bestätigungscode (Zwei-Faktor)</span>
              <span class="note">
                {codeEnabled
                  ? "Aktiv: Zum festen Passwort ist der Code aus der Authenticator-App nötig."
                  : "Zusätzlich zum festen Passwort einen Code aus einer Authenticator-App verlangen."}
              </span>
            </span>
            {#if codeEnabled}
              <button type="button" class="btn btn-quiet" disabled={codeBusy} onclick={disableCode}>Ausschalten</button>
            {:else if !codeSetup}
              <button type="button" class="btn btn-quiet" onclick={startCode}>Einrichten</button>
            {/if}
          </div>
          {#if codeSetup && !codeEnabled}
            <div class="code-setup">
              <div class="qr" aria-label="QR-Code für die Authenticator-App">{@html codeSetup.qr}</div>
              <div class="code-steps">
                <span class="note">
                  Den QR-Code mit einer Authenticator-App scannen (z. B. Microsoft oder Google Authenticator) oder den
                  Schlüssel von Hand eingeben:
                </span>
                <code class="secret">{codeSetup.secret.replace(/(.{4})/g, "$1 ").trim()}</code>
                <input
                  class="field"
                  bind:value={codeConfirm}
                  inputmode="numeric"
                  maxlength="7"
                  placeholder="Angezeigter Code"
                  autocomplete="one-time-code"
                />
                <div class="code-actions">
                  <button type="button" class="btn btn-quiet" onclick={() => (codeSetup = null)}>Abbrechen</button>
                  <button
                    type="button"
                    class="btn btn-primary"
                    disabled={codeBusy || codeConfirm.replace(/\s/g, "").length !== 6}
                    onclick={enableCode}>Aktivieren</button
                  >
                </div>
              </div>
            </div>
          {/if}
          {#if codeError}<p class="error">{codeError}</p>{/if}
        </div>
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
      {#if teamPolicy}
        <div class="policy-note" role="status">
          <strong>Dieses Gerät folgt der Richtlinie des Teams.</strong>
          {#if policyLines.length}
            <ul>
              {#each policyLines as line (line)}<li>{line}</li>{/each}
            </ul>
          {/if}
        </div>
      {/if}
      <div class="rights" role="table">
        <div class="rights-row head" role="row">
          <span role="columnheader"></span>
          <span role="columnheader">Einmal&shy;passwort</span>
          <span role="columnheader">Unbeaufsichtigt</span>
        </div>
        {#each RIGHTS as [bit, label, hint, warn] (bit)}
          <div class="rights-row" role="row">
            <span role="cell">{label}{#if hint}<small class:warn>{hint}</small>{/if}</span>
            <span role="cell">
              <input
                type="checkbox"
                aria-label={`${label}, mit Einmalpasswort`}
                checked={(attended & bit) !== 0}
                onchange={(e) => flip("attended", bit, e.currentTarget)}
              />
            </span>
            <span role="cell">
              <input
                type="checkbox"
                aria-label={`${label}, unbeaufsichtigt`}
                checked={(unattendedRights & bit) !== 0}
                onchange={(e) => flip("unattended", bit, e.currentTarget)}
              />
              {#if teamState(bit) === "locked"}
                <small class="team-lock">vom Team gesperrt</small>
              {:else if teamState(bit) === "limited"}
                <small class="team-lock">vom Team eingeschränkt</small>
              {/if}
            </span>
          </div>
        {/each}
      </div>
      <span class="note">
        „Unbeaufsichtigt“ gilt für das feste Passwort und für Geräte des Kontos. Bei Sitzungen mit Einmalpasswort sitzt
        meist jemand am Gerät, deshalb sind Schwarzschalten und Port-Tunnel dort standardmäßig aus.
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
      <h3>Ihre Sitzungen</h3>
      <label class="toggle">
        <span>
          <span class="name">Jede Sitzung in eigenem Fenster</span>
          <span class="note">Sonst öffnen sich Sitzungen als Tabs in einem gemeinsamen Fenster.</span>
        </span>
        <input
          type="checkbox"
          class="switch"
          checked={separateWindows}
          onchange={(e) => setSeparateWindows(e.currentTarget.checked)}
        />
      </label>
      <label class="group">
        <span class="name">Bei Inaktivität trennen</span>
        <select class="field" value={idleMinutes} onchange={(e) => setIdleMinutes(Number(e.currentTarget.value))}>
          <option value={0}>Nie</option>
          <option value={15}>Nach 15 Minuten</option>
          <option value={30}>Nach 30 Minuten</option>
          <option value={60}>Nach 1 Stunde</option>
        </select>
        <span class="note">
          Beendet eine Sitzung, in der Sie so lange nichts eingegeben haben. Eine Minute vorher kommt ein Hinweis.
          Gilt sofort für alle Sitzungen auf diesem Computer.
        </span>
      </label>
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
    <span class="version">Remocto {version}</span>
    <button type="button" class="btn btn-quiet" onclick={onclose}>Abbrechen</button>
    <button class="btn btn-primary" disabled={saving || needsPassword}>Speichern</button>
  </footer>
</form>

{#if asking}
  {@const { bit, apply } = asking}
  <RiskConfirm {bit} oncancel={() => (asking = null)} onconfirm={() => ((asking = null), apply())} />
{/if}

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

  .rights-row small {
    display: block;
    color: var(--ink-3);
    font-size: 11.5px;
  }
  .rights-row small.warn {
    color: var(--warn);
  }

  .rights-row small.team-lock {
    color: var(--warn);
  }

  .policy-note {
    padding: 10px 12px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius-sm);
    background: var(--surface);
    font-size: 12.5px;
    color: var(--ink-2);
  }

  .policy-note ul {
    margin: 6px 0 0;
    padding-left: 18px;
    color: var(--ink-3);
  }

  .rights-row.head {
    color: var(--ink-2);
    font-size: 12px;
  }

  .code-box {
    display: grid;
    gap: 10px;
    margin-top: 6px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }

  .code-setup {
    display: grid;
    grid-template-columns: 180px 1fr;
    gap: 14px;
    align-items: start;
  }

  .qr {
    padding: 6px;
    border-radius: 8px;
    background: #fff;
  }

  .qr :global(svg) {
    display: block;
    width: 100%;
    height: auto;
  }

  .code-steps {
    display: grid;
    gap: 8px;
  }

  .secret {
    font-size: 12.5px;
    letter-spacing: 0.04em;
    user-select: all;
    word-break: break-all;
  }

  .code-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
