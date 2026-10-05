<script lang="ts">
  import type { Profile } from "./api";

  // `peer`: the technical name ("user (device)"), shown under the profile.
  let { profile, peer = "" }: { profile: Profile; peer?: string } = $props();

  const initials = $derived(
    (profile.company || profile.name)
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => w[0]!.toUpperCase())
      .join(""),
  );
</script>

<div class="card">
  {#if profile.logo}
    <img class="logo" src={`data:image/png;base64,${profile.logo}`} alt="" />
  {:else}
    <span class="logo initials" aria-hidden="true">{initials}</span>
  {/if}
  <div class="text">
    {#if profile.name}
      <strong class="name">{profile.name}</strong>
    {/if}
    {#if profile.company}
      <span class={profile.name ? "company" : "name"}>{profile.company}</span>
    {/if}
    {#if profile.message}
      <p class="message">{profile.message}</p>
    {/if}
    {#if peer}
      <span class="peer">Gerät: {peer}</span>
    {/if}
  </div>
</div>

<style>
  .card {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    padding: 14px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--surface);
    text-align: left;
  }

  .logo {
    flex: none;
    width: 48px;
    height: 48px;
    border-radius: var(--radius-sm);
    object-fit: contain;
  }

  .initials {
    display: grid;
    place-items: center;
    border: 1px solid var(--line);
    color: var(--accent);
    font: 600 16px/1 var(--font-display);
  }

  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .name {
    font-size: 15px;
    font-weight: 600;
  }

  .company {
    color: var(--ink-2);
  }

  .message {
    margin: 6px 0 0;
    color: var(--ink);
    line-height: 1.4;
  }

  .peer {
    margin-top: 6px;
    color: var(--ink-3);
    font-size: 12px;
  }
</style>
