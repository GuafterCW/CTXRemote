<script lang="ts">
  import { api, errorText, RIGHT, type Hosted } from "./api";

  // What the viewer of a session at this computer may do, changeable while it runs.
  let {
    hosted,
    allowPrivacy = true,
    class: className = "",
  }: { hosted: Hosted; allowPrivacy?: boolean; class?: string } = $props();

  let open = $state(false);
  let error = $state("");
  let anchor = $state<HTMLDivElement>();

  const ITEMS: [number, string][] = [
    [RIGHT.INPUT, "Maus und Tastatur"],
    [RIGHT.FILES, "Dateien"],
    [RIGHT.CLIPBOARD, "Zwischenablage"],
    [RIGHT.AUDIO, "Ton"],
    [RIGHT.RESTART, "Neu starten"],
  ];
  // Only where nobody needs to sit at the computer (not in the quick helper).
  const UNATTENDED: [number, string][] = [
    [RIGHT.PRIVACY, "Bildschirm hier schwarz schalten"],
    [RIGHT.TUNNEL, "Port-Tunnel in dieses Netz"],
  ];
  const items = $derived(allowPrivacy ? [...ITEMS, ...UNATTENDED] : ITEMS);
  const viewOnly = $derived((hosted.rights & RIGHT.INPUT) === 0);

  async function toggle(bit: number) {
    error = "";
    const next = hosted.rights ^ bit;
    const before = hosted.rights;
    hosted.rights = next;
    try {
      await api.setHostedRights(hosted.session, next);
    } catch (e) {
      hosted.rights = before;
      error = errorText(e);
    }
  }

  function onWindowClick(e: MouseEvent) {
    if (open && anchor && !anchor.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={(e) => e.key === "Escape" && (open = false)} />

<div class="anchor {className}" bind:this={anchor}>
  <button class="rights-btn" aria-expanded={open} onclick={() => (open = !open)}>
    {viewOnly ? "Nur ansehen" : "Rechte"}
  </button>
  {#if open}
    <div class="menu" role="menu">
      <p class="title">Die Gegenseite darf</p>
      {#each items as [bit, label] (bit)}
        <label class="item">
          <input type="checkbox" checked={(hosted.rights & bit) !== 0} onchange={() => toggle(bit)} />
          <span>{label}</span>
        </label>
      {/each}
      {#if error}<p class="error">{error}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .anchor {
    position: relative;
  }

  .rights-btn {
    height: 26px;
    padding: 0 12px;
    border: 1px solid color-mix(in srgb, currentColor 35%, transparent);
    border-radius: 6px;
    background: transparent;
    color: inherit;
    font-size: 12.5px;
    font-weight: 600;
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 20;
    min-width: 240px;
    padding: 8px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--raised);
    color: var(--ink);
    box-shadow: 0 8px 24px rgb(0 0 0 / 0.18);
  }

  .title {
    margin: 2px 6px 6px;
    color: var(--ink-2);
    font-size: 12px;
  }

  .item {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px;
    border-radius: 6px;
    font-size: 13px;
    font-weight: 400;
    cursor: pointer;
  }

  .item:hover {
    background: var(--bg);
  }

  .error {
    margin: 6px;
    color: var(--bad);
    font-size: 12px;
  }
</style>
