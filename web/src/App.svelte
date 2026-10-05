<script lang="ts">
  import Auth from "./Auth.svelte";
  import CodeView from "./CodeView.svelte";
  import Dashboard from "./Dashboard.svelte";
  import { onMount } from "svelte";
  import { mayRestore, restore, signedIn, verifyEmail } from "./lib/session";

  // After a reload the tab may still hold its key (see lib/session.ts):
  // show nothing until that is settled, instead of a flash of the login form.
  const restoring = !signedIn() && mayRestore();
  let view = $state<"auth" | "code" | "dash" | "wait">(signedIn() ? "dash" : restoring ? "wait" : "auth");
  onMount(() => {
    if (restoring) restore().then((ok) => (view = ok ? "dash" : "auth"));
  });
  let recoveryCode = $state("");
  let notice = $state("");

  // The link in the confirmation mail: /konto/#bestaetigen=<token>.
  let confirmation = $state<{ ok: boolean; text: string } | null>(null);
  onMount(async () => {
    const token = new URLSearchParams(location.hash.slice(1)).get("bestaetigen");
    if (!token) return;
    history.replaceState(null, "", location.pathname);
    try {
      await verifyEmail(token);
      confirmation = { ok: true, text: "Ihre E-Mail-Adresse ist bestätigt." };
    } catch (err) {
      confirmation = { ok: false, text: err instanceof Error ? err.message : String(err) };
    }
  });

  function showCode(code: string) {
    recoveryCode = code;
    notice = "";
    view = "code";
  }

  function codeDone() {
    recoveryCode = "";
    view = "dash";
  }

  function signedInNow() {
    notice = "";
    view = "dash";
  }

  function toAuth(message = "") {
    recoveryCode = "";
    notice = message;
    view = "auth";
  }
</script>

<header class="top">
  <div class="wrap">
    <a class="brand" href="/">
      <svg width="22" height="22" viewBox="0 0 20 20" aria-hidden="true">
        <rect x="1.5" y="3.5" width="13" height="10" rx="2" fill="none" stroke="currentColor" stroke-width="1.5" />
        <rect x="6.5" y="7.5" width="12" height="9" rx="2" fill="var(--accent)" />
      </svg>
      <span>CTXRemote</span>
    </a>
    <nav class="nav" aria-label="Hauptmenü">
      <a href="/">Start</a>
      <a href="/download.html">Download</a>
      <a href="/konto/" aria-current="page">Konto</a>
    </nav>
  </div>
</header>

<main>
  <div class="wrap">
    {#if confirmation}
      <p class="confirmation" class:bad={!confirmation.ok} role="status">{confirmation.text}</p>
    {/if}
    {#if view === "auth"}
      <Auth {notice} onCode={showCode} onSignedIn={signedInNow} />
    {:else if view === "wait"}
      <p class="wait" aria-busy="true">Einen Moment …</p>
    {:else if view === "code"}
      <CodeView code={recoveryCode} onDone={codeDone} />
    {:else}
      <Dashboard onExpired={() => toAuth("Bitte erneut anmelden")} onLoggedOut={() => toAuth()} onCode={showCode} />
    {/if}
  </div>
</main>

<footer class="foot">
  <div class="wrap">
    <span>© 2026 CTXRemote</span>
    <a href="/impressum.html">Impressum</a>
    <a href="/datenschutz.html">Datenschutz</a>
  </div>
</footer>

<style>
  .wait {
    margin: 64px auto;
    text-align: center;
    color: var(--ink-3);
  }

  .confirmation {
    max-width: 440px;
    margin: 24px auto 0;
    padding: 12px 16px;
    border: 1px solid var(--line);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius);
    color: var(--ink-2);
  }

  .confirmation.bad {
    border-left-color: #b3412f;
  }
</style>
