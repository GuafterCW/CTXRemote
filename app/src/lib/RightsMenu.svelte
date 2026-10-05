<script lang="ts">
  import { api, errorText, RIGHT, type Hosted } from "./api";
  import Icon, { type IconName } from "./Icon.svelte";

  // What the viewer of a session at this computer may do, changeable while it
  // runs. Always in view: one switch per right, lit while it is allowed.
  let {
    hosted,
    allowPrivacy = true,
    class: className = "",
  }: { hosted: Hosted; allowPrivacy?: boolean; class?: string } = $props();

  let error = $state("");

  type Item = [bit: number, icon: IconName, label: string];
  const ITEMS: Item[] = [
    [RIGHT.INPUT, "keyboard", "Maus und Tastatur"],
    [RIGHT.FILES, "folder", "Dateien"],
    [RIGHT.CLIPBOARD, "copy", "Zwischenablage"],
    [RIGHT.AUDIO, "volume", "Ton"],
    [RIGHT.RESTART, "power", "Neu starten"],
  ];
  // Only where nobody needs to sit at the computer (not in the quick helper).
  const UNATTENDED: Item[] = [
    [RIGHT.PRIVACY, "eyeOff", "Bildschirm hier schwarz schalten"],
    [RIGHT.TUNNEL, "tunnel", "Port-Tunnel in dieses Netz"],
  ];
  const items = $derived(allowPrivacy ? [...ITEMS, ...UNATTENDED] : ITEMS);
  const viewOnly = $derived((hosted.rights & RIGHT.INPUT) === 0);

  async function toggle(bit: number) {
    error = "";
    const before = hosted.rights;
    hosted.rights = before ^ bit;
    try {
      await api.setHostedRights(hosted.session, hosted.rights);
    } catch (e) {
      hosted.rights = before;
      error = errorText(e);
    }
  }
</script>

<div class="rights {className}" role="group" aria-label="Rechte der Gegenseite">
  {#if viewOnly}<span class="view-only">Nur ansehen</span>{/if}
  {#each items as [bit, icon, label] (bit)}
    {@const on = (hosted.rights & bit) !== 0}
    <button
      class="right"
      class:on
      aria-pressed={on}
      title={`${label}: ${on ? "erlaubt" : "gesperrt"} (klicken zum ${on ? "Sperren" : "Erlauben"})`}
      onclick={() => toggle(bit)}
    >
      <Icon name={icon} size={14} />
    </button>
  {/each}
  {#if error}<span class="error" title={error}>!</span>{/if}
</div>

<style>
  .rights {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    padding: 2px;
    border: 1px solid color-mix(in srgb, currentColor 25%, transparent);
    border-radius: 8px;
  }

  .view-only {
    padding: 0 6px;
    font-size: 11.5px;
    font-weight: 600;
    white-space: nowrap;
  }

  .right {
    position: relative;
    display: grid;
    place-items: center;
    width: 24px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: inherit;
    opacity: 0.45;
    transition:
      background 0.12s,
      opacity 0.12s;
  }

  /* Struck through while the right is withheld. */
  .right:not(.on)::after {
    content: "";
    position: absolute;
    width: 18px;
    height: 1.5px;
    background: currentColor;
    border-radius: 1px;
    transform: rotate(-45deg);
  }

  .right.on {
    background: color-mix(in srgb, currentColor 18%, transparent);
    opacity: 1;
  }

  .right:hover {
    opacity: 1;
    background: color-mix(in srgb, currentColor 26%, transparent);
  }

  .error {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--bad);
    color: white;
    font-size: 11px;
    font-weight: 700;
  }
</style>
