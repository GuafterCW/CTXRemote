<script lang="ts">
  import { MIN_PASSWORD } from "./lib/crypto";
  import { login, recover, register } from "./lib/session";

  let {
    notice = "",
    onCode,
    onSignedIn,
  }: { notice?: string; onCode: (code: string) => void; onSignedIn: () => void } = $props();

  type Mode = "login" | "register" | "recover";
  let mode = $state<Mode>("login");
  let email = $state("");
  let password = $state("");
  let repeat = $state("");
  let code = $state("");
  let busy = $state(false);
  let error = $state("");

  const tooShort = $derived([...password].length < MIN_PASSWORD);
  const mismatch = $derived(repeat !== "" && password !== repeat);
  const newPasswordOk = $derived(!tooShort && password === repeat);

  function switchTo(next: Mode) {
    mode = next;
    error = "";
    password = "";
    repeat = "";
    code = "";
  }

  async function run(action: () => Promise<void>) {
    busy = true;
    error = "";
    try {
      await action();
    } catch (err) {
      error = err instanceof Error ? err.message : "Unbekannter Fehler";
    } finally {
      busy = false;
    }
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    if (mode === "login") {
      void run(async () => {
        await login(email, password);
        password = "";
        onSignedIn();
      });
    } else if (mode === "register") {
      if (!newPasswordOk) return;
      void run(async () => {
        const recovery = await register(email, password);
        password = repeat = "";
        onCode(recovery);
      });
    } else {
      if (!newPasswordOk) return;
      void run(async () => {
        const recovery = await recover(email, code, password);
        password = repeat = code = "";
        onCode(recovery);
      });
    }
  }
</script>

<div class="card card-narrow">
  {#if notice}
    <p class="notice" role="status">{notice}</p>
  {/if}

  {#if mode !== "recover"}
    <div class="tabs" role="tablist" aria-label="Konto">
      <button class="tab" role="tab" type="button" aria-selected={mode === "login"} onclick={() => switchTo("login")}>
        Anmelden
      </button>
      <button class="tab" role="tab" type="button" aria-selected={mode === "register"} onclick={() => switchTo("register")}>
        Registrieren
      </button>
    </div>
  {:else}
    <h1>Passwort zurücksetzen</h1>
    <p class="hint">Geben Sie Ihren Wiederherstellungscode ein und wählen Sie ein neues Passwort.</p>
  {/if}

  <form onsubmit={submit}>
    <label class="field">
      <span>E-Mail-Adresse</span>
      <input type="email" autocomplete="username" required bind:value={email} disabled={busy} />
    </label>

    {#if mode === "recover"}
      <label class="field">
        <span>Wiederherstellungscode</span>
        <input
          class="code-input"
          type="text"
          autocomplete="off"
          autocapitalize="characters"
          spellcheck="false"
          placeholder="ABCDE-FGHJK-MNPQR-STVWX-YZ012"
          required
          bind:value={code}
          disabled={busy}
        />
      </label>
    {/if}

    <label class="field">
      <span>{mode === "recover" ? "Neues Passwort" : "Passwort"}</span>
      <input
        type="password"
        autocomplete={mode === "login" ? "current-password" : "new-password"}
        required
        bind:value={password}
        disabled={busy}
      />
    </label>

    {#if mode !== "login"}
      <label class="field">
        <span>Passwort wiederholen</span>
        <input type="password" autocomplete="new-password" required bind:value={repeat} disabled={busy} />
      </label>
      {#if password !== "" && tooShort}
        <p class="hint">Das Passwort braucht mindestens {MIN_PASSWORD} Zeichen.</p>
      {:else if mismatch}
        <p class="hint">Die beiden Passwörter stimmen nicht überein.</p>
      {/if}
    {/if}

    {#if mode === "register"}
      <p class="muted">
        Ihre Geräteliste wird in Ihrem Browser verschlüsselt. Der Server kennt weder Ihr Passwort noch Ihre Daten.
      </p>
    {/if}

    <button class="btn btn-primary btn-block" type="submit" disabled={busy || (mode !== "login" && !newPasswordOk)}>
      {#if busy}
        Bitte warten …
      {:else if mode === "login"}
        Anmelden
      {:else if mode === "register"}
        Konto erstellen
      {:else}
        Neues Passwort setzen
      {/if}
    </button>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
  </form>

  <p class="row-links">
    {#if mode === "login"}
      <button class="link" type="button" disabled={busy} onclick={() => switchTo("recover")}>Passwort vergessen?</button>
    {:else if mode === "recover"}
      <button class="link" type="button" disabled={busy} onclick={() => switchTo("login")}>Zurück zur Anmeldung</button>
    {/if}
  </p>
</div>
