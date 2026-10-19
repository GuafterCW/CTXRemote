<script lang="ts">
  import { onMount } from "svelte";
  import { RISKY } from "./risky";

  // Asks before a right that hands over more than the screen is switched on.
  let { bit, onconfirm, oncancel }: { bit: number; onconfirm: () => void; oncancel: () => void } = $props();

  const risk = $derived(RISKY[bit]);
  let cancelButton = $state<HTMLButtonElement>();

  // The safe choice has the focus, so Enter does not allow it by accident.
  onMount(() => cancelButton?.focus());
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && oncancel()} />

<div class="scrim" role="presentation">
  <div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="risk-title" aria-describedby="risk-points">
    <h3 id="risk-title">{risk.title}</h3>
    <ul id="risk-points">
      {#each risk.points as point (point)}<li>{point}</li>{/each}
    </ul>
    <p class="trust">Nur erlauben, wenn Sie der Gegenseite voll vertrauen.</p>
    <div class="actions">
      <button bind:this={cancelButton} class="btn btn-quiet small" onclick={oncancel}>Abbrechen</button>
      <button class="btn btn-danger small" onclick={onconfirm}>Trotzdem erlauben</button>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: 16px;
    background: color-mix(in srgb, var(--bg) 75%, transparent);
    color: var(--ink);
  }

  .dialog {
    width: min(420px, 100%);
    padding: 18px;
    border: 1px solid var(--line-strong);
    border-radius: var(--radius);
    background: var(--raised);
    text-align: left;
  }

  h3 {
    margin: 0 0 10px;
    font: 600 15px var(--font-display);
  }

  ul {
    margin: 0 0 12px;
    padding-left: 18px;
    color: var(--ink-2);
    font-size: 13px;
    line-height: 1.5;
  }

  li + li {
    margin-top: 6px;
  }

  .trust {
    margin: 0 0 16px;
    color: var(--warn);
    font-size: 13px;
    font-weight: 600;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .small {
    height: 32px;
    padding: 0 14px;
  }

  .btn-danger {
    background: var(--bad);
    color: #fff;
  }
</style>
