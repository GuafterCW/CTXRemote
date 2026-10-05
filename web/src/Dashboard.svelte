<script lang="ts">
  import { onMount } from "svelte";
  import { MIN_PASSWORD } from "./lib/crypto";
  import {
    account,
    ApiError,
    changeLogin,
    deleteAccount,
    devices,
    loadBook,
    logout,
    pairingCode,
    removeDevice,
    removeEntry,
    renameEntry,
    resendVerification,
    type AccountInfo,
    type Book,
    type Device,
  } from "./lib/session";

  let {
    onExpired,
    onLoggedOut,
    onCode,
  }: { onExpired: () => void; onLoggedOut: () => void; onCode: (code: string) => void } = $props();

  let info = $state<AccountInfo | null>(null);
  let deviceList = $state<Device[] | null>(null);
  let book = $state<Book | null>(null);

  let accountError = $state("");
  let deviceError = $state("");
  let bookError = $state("");
  let loginError = $state("");

  // Device section
  let confirmDevice = $state<string | null>(null);
  let pairing = $state("");
  let pairingBusy = $state(false);
  let busyDevice = $state<string | null>(null);

  // Book section
  let editId = $state<string | null>(null);
  let editText = $state("");
  let confirmEntry = $state<string | null>(null);
  let busyEntry = $state<string | null>(null);

  // Login section
  let email = $state("");
  let password = $state("");
  let repeat = $state("");
  let loginBusy = $state(false);

  const tooShort = $derived([...password].length < MIN_PASSWORD);
  const mismatch = $derived(repeat !== "" && password !== repeat);

  const entries = $derived(
    book
      ? Object.entries(book.entries).sort(([, a], [, b]) => b.last_seen - a.last_seen)
      : [],
  );

  /** Returns the message, or leaves the page when the session is gone. */
  function message(err: unknown): string {
    if (err instanceof ApiError && err.status === 401) {
      onExpired();
      return "";
    }
    return err instanceof Error ? err.message : "Unbekannter Fehler";
  }

  function formatId(id: string): string {
    return /^\d{9}$/.test(id) ? id.replace(/(\d{3})(?=\d)/g, "$1 ") : id;
  }

  function formatSeen(seconds: number): string {
    if (!seconds) return "nie";
    return new Date(seconds * 1000).toLocaleString("de-DE", { dateStyle: "medium", timeStyle: "short" });
  }

  async function loadAccount() {
    try {
      info = await account();
      if (!email && info.email) email = info.email;
    } catch (err) {
      accountError = message(err);
    }
  }

  async function loadDevices() {
    try {
      deviceList = await devices();
    } catch (err) {
      deviceError = message(err);
    }
  }

  async function loadEntries() {
    try {
      book = await loadBook();
    } catch (err) {
      bookError = message(err);
    }
  }

  onMount(() => {
    void loadAccount();
    void loadDevices();
    void loadEntries();
  });

  async function signOut() {
    await logout();
    onLoggedOut();
  }

  async function dropDevice(publicKey: string) {
    busyDevice = publicKey;
    deviceError = "";
    try {
      await removeDevice(publicKey);
      confirmDevice = null;
      deviceList = await devices();
    } catch (err) {
      deviceError = message(err);
    } finally {
      busyDevice = null;
    }
  }

  async function newPairing() {
    pairingBusy = true;
    deviceError = "";
    try {
      pairing = await pairingCode();
    } catch (err) {
      deviceError = message(err);
    } finally {
      pairingBusy = false;
    }
  }

  function startEdit(id: string) {
    const entry = book?.entries[id];
    editId = id;
    editText = entry?.alias ?? entry?.name ?? "";
    confirmEntry = null;
  }

  async function saveEdit(event: SubmitEvent) {
    event.preventDefault();
    if (editId === null) return;
    busyEntry = editId;
    bookError = "";
    try {
      book = await renameEntry(editId, editText);
      editId = null;
    } catch (err) {
      bookError = message(err);
    } finally {
      busyEntry = null;
    }
  }

  async function dropEntry(id: string) {
    busyEntry = id;
    bookError = "";
    try {
      book = await removeEntry(id);
      confirmEntry = null;
    } catch (err) {
      bookError = message(err);
    } finally {
      busyEntry = null;
    }
  }

  async function saveLogin(event: SubmitEvent) {
    event.preventDefault();
    if (tooShort || password !== repeat) return;
    loginBusy = true;
    loginError = "";
    try {
      const code = await changeLogin(email, password);
      password = repeat = "";
      onCode(code);
    } catch (err) {
      loginError = message(err);
    } finally {
      loginBusy = false;
    }
  }

  // Deleting the account: asks for the password once more.
  let deleteOpen = $state(false);
  let deletePassword = $state("");
  let deleteBusy = $state(false);
  let deleteError = $state("");
  async function confirmDelete(event: SubmitEvent) {
    event.preventDefault();
    if (!info?.email || !deletePassword) return;
    deleteBusy = true;
    deleteError = "";
    try {
      await deleteAccount(info.email, deletePassword);
      onLoggedOut();
    } catch (err) {
      deleteError = err instanceof ApiError && err.status === 401 ? "Das Passwort stimmt nicht." : message(err);
    } finally {
      deleteBusy = false;
      deletePassword = "";
    }
  }

  let resendState = $state<"idle" | "busy" | "sent">("idle");
  async function resend() {
    resendState = "busy";
    try {
      await resendVerification();
      resendState = "sent";
    } catch (err) {
      resendState = "idle";
      accountError = err instanceof Error ? err.message : String(err);
    }
  }
</script>

<div class="who">
  <div>
    <span class="who-mail">{info?.email ?? ""}</span>
    {#if info && !info.verified}
      <span class="badge">Noch nicht bestätigt</span>
      <button class="link-resend" type="button" disabled={resendState === "busy"} onclick={resend}>
        {resendState === "sent" ? "Mail ist unterwegs" : "Bestätigungsmail erneut senden"}
      </button>
    {/if}
    {#if accountError}
      <p class="error" role="alert">{accountError}</p>
    {/if}
  </div>
  <button class="btn btn-small" type="button" onclick={signOut}>Abmelden</button>
</div>

<section class="panel" aria-labelledby="h-devices">
  <div class="panel-head">
    <h2 id="h-devices">Angemeldete Geräte</h2>
    <button class="btn btn-small" type="button" disabled={pairingBusy} onclick={newPairing}>
      {pairingBusy ? "Bitte warten …" : "Gerät per Code hinzufügen"}
    </button>
  </div>

  {#if pairing}
    <p class="code" aria-label="Kopplungscode">{pairing}</p>
    <p class="hint">In der App unter Konto → ‚Mit Code‘ eingeben. Gilt 10 Minuten und nur einmal.</p>
  {/if}

  {#if deviceList === null}
    {#if !deviceError}<p class="empty">Wird geladen …</p>{/if}
  {:else if deviceList.length === 0}
    <p class="empty">Noch kein Gerät angemeldet. Melden Sie sich in der App mit derselben E-Mail-Adresse an.</p>
  {:else}
    <ul class="list">
      {#each deviceList as device (device.publicKey)}
        <li>
          <div class="item-main">
            <span class="dot" class:on={device.online} role="img" aria-label={device.online ? "Online" : "Offline"}></span>
            <div class="item-text">
              <div class="item-name">{device.name || "Unbenanntes Gerät"}</div>
              <div class="item-sub">{device.id ? formatId(device.id) : "ohne ID"}</div>
            </div>
          </div>
          {#if confirmDevice === device.publicKey}
            <div class="confirm">
              <button class="btn btn-small btn-danger" type="button" disabled={busyDevice === device.publicKey} onclick={() => dropDevice(device.publicKey)}>
                {busyDevice === device.publicKey ? "Bitte warten …" : "Entfernen"}
              </button>
              <button class="btn btn-small" type="button" disabled={busyDevice === device.publicKey} onclick={() => (confirmDevice = null)}>Abbrechen</button>
            </div>
          {:else}
            <div class="item-actions">
              <button class="btn btn-small" type="button" onclick={() => (confirmDevice = device.publicKey)}>Entfernen</button>
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if deviceError}
    <p class="error" role="alert">{deviceError}</p>
  {/if}
</section>

<section class="panel" aria-labelledby="h-book">
  <div class="panel-head">
    <h2 id="h-book">Geräteliste</h2>
  </div>

  {#if book === null}
    {#if !bookError}<p class="empty">Wird geladen …</p>{/if}
  {:else if entries.length === 0}
    <p class="empty">Die Geräteliste ist leer. Geräte, mit denen Sie sich in der App verbinden, erscheinen hier.</p>
  {:else}
    <ul class="list">
      {#each entries as [id, entry] (id)}
        <li>
          {#if editId === id}
            <form class="rename" onsubmit={saveEdit}>
              <input type="text" aria-label="Neuer Name" maxlength="64" bind:value={editText} disabled={busyEntry === id} />
              <button class="btn btn-small btn-primary" type="submit" disabled={busyEntry === id}>
                {busyEntry === id ? "Bitte warten …" : "Speichern"}
              </button>
              <button class="btn btn-small" type="button" disabled={busyEntry === id} onclick={() => (editId = null)}>Abbrechen</button>
            </form>
          {:else}
            <div class="item-main">
              <div class="item-text">
                <div class="item-name">{entry.alias || entry.name || "Unbenanntes Gerät"}</div>
                <div class="item-sub">{formatId(id)} · zuletzt verbunden: {formatSeen(entry.last_seen)}</div>
              </div>
            </div>
            {#if confirmEntry === id}
              <div class="confirm">
                <button class="btn btn-small btn-danger" type="button" disabled={busyEntry === id} onclick={() => dropEntry(id)}>
                  {busyEntry === id ? "Bitte warten …" : "Entfernen"}
                </button>
                <button class="btn btn-small" type="button" disabled={busyEntry === id} onclick={() => (confirmEntry = null)}>Abbrechen</button>
              </div>
            {:else}
              <div class="item-actions">
                <button class="icon-btn" type="button" aria-label="Umbenennen" title="Umbenennen" onclick={() => startEdit(id)}>
                  <svg width="18" height="18" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                    <path d="M13.5 3.5l3 3L7 16H4v-3z" />
                    <path d="M11.5 5.5l3 3" />
                  </svg>
                </button>
                <button class="btn btn-small" type="button" onclick={() => { confirmEntry = id; editId = null; }}>Entfernen</button>
              </div>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if bookError}
    <p class="error" role="alert">{bookError}</p>
  {/if}
</section>

<section class="panel" aria-labelledby="h-login">
  <div class="panel-head">
    <h2 id="h-login">Anmeldung</h2>
  </div>
  <p class="hint">E-Mail-Adresse oder Passwort ändern. Dabei erhalten Sie einen neuen Wiederherstellungscode.</p>
  <form onsubmit={saveLogin}>
    <label class="field">
      <span>E-Mail-Adresse</span>
      <input type="email" autocomplete="username" required bind:value={email} disabled={loginBusy} />
    </label>
    <label class="field">
      <span>Neues Passwort</span>
      <input type="password" autocomplete="new-password" required bind:value={password} disabled={loginBusy} />
    </label>
    <label class="field">
      <span>Passwort wiederholen</span>
      <input type="password" autocomplete="new-password" required bind:value={repeat} disabled={loginBusy} />
    </label>
    {#if password !== "" && tooShort}
      <p class="hint">Das Passwort braucht mindestens {MIN_PASSWORD} Zeichen.</p>
    {:else if mismatch}
      <p class="hint">Die beiden Passwörter stimmen nicht überein.</p>
    {/if}
    <button class="btn btn-primary" type="submit" disabled={loginBusy || tooShort || password !== repeat}>
      {loginBusy ? "Bitte warten …" : "Speichern"}
    </button>
    {#if loginError}
      <p class="error" role="alert">{loginError}</p>
    {/if}
  </form>
</section>

<section class="panel" aria-labelledby="h-delete">
  <div class="panel-head">
    <h2 id="h-delete">Konto löschen</h2>
  </div>
  <p class="hint">
    Löscht das Konto mit Anmeldung, Geräteliste und allen Geräten darin, endgültig. Ihre Geräte arbeiten danach ohne
    Konto weiter.
  </p>
  {#if !deleteOpen}
    <button class="btn btn-small btn-danger" type="button" onclick={() => (deleteOpen = true)}>Konto löschen …</button>
  {:else}
    <form onsubmit={confirmDelete}>
      <label class="field">
        <span>Zur Bestätigung Ihr Passwort</span>
        <input type="password" autocomplete="current-password" required bind:value={deletePassword} disabled={deleteBusy} />
      </label>
      <div class="delete-buttons">
        <button class="btn btn-danger" type="submit" disabled={deleteBusy || !deletePassword}>
          {deleteBusy ? "Bitte warten …" : "Endgültig löschen"}
        </button>
        <button class="btn" type="button" disabled={deleteBusy} onclick={() => ((deleteOpen = false), (deleteError = ""))}>
          Abbrechen
        </button>
      </div>
      {#if deleteError}
        <p class="error" role="alert">{deleteError}</p>
      {/if}
    </form>
  {/if}
</section>

<style>
  .delete-buttons {
    display: flex;
    gap: 8px;
  }
</style>
