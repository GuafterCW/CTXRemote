<script lang="ts">
  import Auth from "./Auth.svelte";
  import CodeView from "./CodeView.svelte";
  import Dashboard from "./Dashboard.svelte";
  import { signedIn } from "./lib/session";

  // The key lives only in memory, so a fresh page always starts signed out.
  let view = $state<"auth" | "code" | "dash">(signedIn() ? "dash" : "auth");
  let recoveryCode = $state("");
  let notice = $state("");

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
    {#if view === "auth"}
      <Auth {notice} onCode={showCode} onSignedIn={signedInNow} />
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
