<script lang="ts">
  import { app, init, send } from "./lib/store.svelte";
  import TransportBar from "./components/TransportBar.svelte";
  import LaneStrip from "./components/LaneStrip.svelte";
  import PatternGrid from "./components/PatternGrid.svelte";
  import StepInspector from "./components/StepInspector.svelte";
  import PatternLibrary from "./components/PatternLibrary.svelte";
  import SongPanel from "./components/SongPanel.svelte";
  import SetupPanel from "./components/SetupPanel.svelte";
  import GeneratePanel from "./components/GeneratePanel.svelte";
  import StatusToast from "./components/StatusToast.svelte";

  type Tab = "perform" | "pattern" | "library" | "song" | "setup";
  let tab = $state<Tab>("perform");
  const tabs: { id: Tab; label: string; later?: boolean }[] = [
    { id: "perform", label: "Perform" },
    { id: "pattern", label: "Pattern" },
    { id: "library", label: "Library" },
    { id: "song", label: "Song" },
    { id: "setup", label: "Setup" },
  ];

  init();

  function isTyping(e: KeyboardEvent): boolean {
    const el = e.target as HTMLElement;
    return el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA");
  }

  function onKey(e: KeyboardEvent) {
    if (isTyping(e) || !app.snap) return;
    const meta = e.metaKey || e.ctrlKey;
    const { row, col } = app.snap.selection;
    const lane = app.snap.focused_lane;

    if (meta && e.key.toLowerCase() === "z") {
      e.preventDefault();
      send({ type: e.shiftKey ? "redo" : "undo" });
    } else if (meta && e.key.toLowerCase() === "s") {
      e.preventDefault();
      send({ type: "save" });
    } else if (e.key === " ") {
      e.preventDefault();
      send({ type: "togglePlay" });
    } else if (e.key === "ArrowLeft") {
      e.preventDefault();
      send({ type: "selectStep", args: { lane, row, col: col - 1 } });
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      send({ type: "selectStep", args: { lane, row, col: col + 1 } });
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      send({ type: "selectStep", args: { lane, row: row - 1, col } });
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      send({ type: "selectStep", args: { lane, row: row + 1, col } });
    } else if (e.key === "Enter") {
      e.preventDefault();
      send({ type: "toggleStep", args: { lane, row, col } });
    } else if (e.key === "Backspace" || e.key === "Delete") {
      e.preventDefault();
      send({ type: "clearStep", args: { lane, row, col } });
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<StatusToast />
<GeneratePanel />

{#if !app.ready}
  <div class="boot">
    {#if app.error}
      <p class="err">Failed to reach engine: {app.error}</p>
    {:else}
      <p class="muted">Starting engine…</p>
    {/if}
  </div>
{:else}
  <div class="app">
    <TransportBar />

    <div class="tabs" role="tablist" aria-label="Views">
      {#each tabs as tb (tb.id)}
        <button
          class="tab"
          class:active={tab === tb.id}
          role="tab"
          aria-selected={tab === tb.id}
          onclick={() => (tab = tb.id)}
        >
          {tb.label}{#if tb.later}<span class="soon">soon</span>{/if}
        </button>
      {/each}
    </div>

    <main class="content">
      {#if tab === "perform"}
        <div class="three">
          <div class="left"><LaneStrip /></div>
          <div class="center"><PatternGrid /></div>
          <div class="right"><StepInspector /></div>
        </div>
      {:else if tab === "pattern"}
        <div class="two">
          <div class="center"><PatternGrid /></div>
          <div class="right"><StepInspector /></div>
        </div>
      {:else if tab === "library"}
        <div class="three">
          <div class="left"><LaneStrip /></div>
          <div class="center"><PatternGrid /></div>
          <div class="right wide"><PatternLibrary /></div>
        </div>
      {:else if tab === "song"}
        <SongPanel />
      {:else if tab === "setup"}
        <SetupPanel />
      {/if}
    </main>

    {#if app.version}
      <footer class="version mono" aria-label="version">midip v{app.version}</footer>
    {/if}
  </div>
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
  }
  .boot {
    display: grid;
    place-items: center;
    height: 100%;
  }
  .err {
    color: var(--err);
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 0 8px;
    background: var(--panel);
    border-bottom: var(--border);
    flex: 0 0 auto;
  }
  .tab {
    background: transparent;
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    padding: 8px 14px;
    color: var(--fg-dim);
    font-size: 12px;
    letter-spacing: 0.03em;
  }
  .tab.active {
    color: var(--fg);
    border-bottom-color: var(--ember);
  }
  .soon {
    font-size: 9px;
    color: var(--dim);
    margin-left: 5px;
    text-transform: uppercase;
  }
  .content {
    flex: 1;
    min-height: 0;
  }
  .version {
    position: fixed;
    right: 8px;
    bottom: 5px;
    font-size: 10px;
    color: var(--dim);
    pointer-events: none;
    z-index: 50;
    letter-spacing: 0.03em;
  }
  .three,
  .two {
    display: grid;
    height: 100%;
    min-height: 0;
  }
  .three {
    grid-template-columns: 260px 1fr 300px;
  }
  .three .right.wide {
    width: 300px;
  }
  .two {
    grid-template-columns: 1fr 300px;
  }
  .left {
    border-right: var(--border);
    overflow-y: auto;
    min-height: 0;
  }
  .center {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }
  .right {
    border-left: var(--border);
    background: var(--panel);
    min-height: 0;
    overflow: hidden;
  }
</style>
