<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Session from "./Session.svelte";
  import Icon from "./lib/Icon.svelte";
  import { api } from "./lib/api";

  // One window, one tab per session; the backend sends new sessions here.
  let { first }: { first: number } = $props();

  type Tab = { session: number; title: string };
  let tabs = $state<Tab[]>([{ session: untrack(() => first), title: "" }]);
  let active = $state(untrack(() => first));
  const appWindow = getCurrentWindow();

  // The window is named after the session in front, as a lone session window was.
  $effect(() => {
    const title = tabs.find((t) => t.session === active)?.title;
    if (title) appWindow.setTitle(title).catch(() => {});
  });

  async function close(session: number) {
    await api.closeTab(session).catch(() => {});
    const index = tabs.findIndex((t) => t.session === session);
    if (index < 0) return;
    tabs = tabs.filter((t) => t.session !== session);
    if (tabs.length === 0) {
      await appWindow.close();
      return;
    }
    if (active === session) active = tabs[Math.min(index, tabs.length - 1)].session;
  }

  onMount(() => {
    api
      .sessionTitle(first)
      .then((title) => (tabs = tabs.map((t) => (t.session === first ? { ...t, title } : t))))
      .catch(() => {});
    const unlisten = appWindow.listen<Tab>("tab-open", (e) => {
      if (!tabs.some((t) => t.session === e.payload.session)) tabs = [...tabs, e.payload];
      active = e.payload.session;
    });
    return () => {
      unlisten.then((off) => off());
    };
  });
</script>

<div class="window">
  <div class="bar" role="tablist" aria-label="Sitzungen">
    {#each tabs as tab (tab.session)}
      <div
        class="tab"
        class:active={tab.session === active}
        role="tab"
        tabindex="-1"
        aria-selected={tab.session === active}
        onauxclick={(e) => e.button === 1 && close(tab.session)}
      >
        <button class="pick" title={tab.title} onclick={() => (active = tab.session)}>
          {tab.title || "Verbinden …"}
        </button>
        <button class="close" title="Sitzung beenden" aria-label="Sitzung beenden" onclick={() => close(tab.session)}>
          <Icon name="close" size={14} />
        </button>
      </div>
    {/each}
  </div>
  <div class="panes">
    {#each tabs as tab (tab.session)}
      <div class="pane" class:hidden={tab.session !== active} role="tabpanel">
        <Session session={tab.session} active={tab.session === active} onclose={() => close(tab.session)} />
      </div>
    {/each}
  </div>
</div>

<style>
  .window {
    display: grid;
    grid-template-rows: auto 1fr;
    height: 100%;
  }

  .bar {
    display: flex;
    gap: 2px;
    padding: 4px 6px 0;
    overflow-x: auto;
    background: #141412;
    border-bottom: 1px solid #2f2e2b;
    scrollbar-width: none;
  }

  .tab {
    display: flex;
    align-items: center;
    flex: 0 1 220px;
    min-width: 110px;
    height: 30px;
    border: 1px solid transparent;
    border-bottom: 0;
    border-radius: 7px 7px 0 0;
    color: #a5a39c;
  }

  .tab:hover {
    color: #d8d6d0;
  }

  .tab.active {
    background: #0b0b0a;
    border-color: #2f2e2b;
    color: #edebe6;
  }

  .pick {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0 4px 0 12px;
    border: 0;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .close {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    margin-right: 4px;
    border: 0;
    border-radius: 5px;
    background: none;
    color: inherit;
    opacity: 0.7;
  }

  .close:hover {
    background: #2a2926;
    opacity: 1;
  }

  .panes {
    position: relative;
    min-height: 0;
  }

  /* Hidden tabs stay laid out, so their picture keeps its size. */
  .pane {
    position: absolute;
    inset: 0;
  }

  .pane.hidden {
    visibility: hidden;
    pointer-events: none;
  }
</style>
