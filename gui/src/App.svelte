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
  import CommandPalette from "./components/CommandPalette.svelte";
  import HelpOverlay from "./components/HelpOverlay.svelte";
  import StatusToast from "./components/StatusToast.svelte";

  type Tab = "perform" | "pattern" | "library" | "song" | "setup";
  let tab = $state<Tab>("perform");
  let paletteOpen = $state(false);
  let helpOpen = $state(false);
  let showOnboard = $state(
    typeof localStorage !== "undefined" && !localStorage.getItem("midip-onboarded"),
  );
  function dismissOnboard() {
    showOnboard = false;
    try {
      localStorage.setItem("midip-onboarded", "1");
    } catch {
      /* ignore */
    }
  }

  interface Cmd {
    label: string;
    hint?: string;
    run: () => void;
  }
  function buildActions(): Cmd[] {
    const lane = app.snap?.focused_lane ?? 0;
    const go = (t: Tab) => () => (tab = t);
    return [
      { label: "Transport: Play / Stop", hint: "Space", run: () => send({ type: "togglePlay" }) },
      { label: "Transport: Tap tempo", run: () => send({ type: "tap" }) },
      { label: "Transport: Toggle Link", run: () => send({ type: "toggleLink" }) },
      { label: "Transport: Toggle mirror", run: () => send({ type: "toggleMirror" }) },
      { label: "MIDI panic (all notes off)", run: () => send({ type: "panic" }) },
      { label: "Edit: Undo", hint: "⌘Z", run: () => send({ type: "undo" }) },
      { label: "Edit: Redo", hint: "⌘⇧Z", run: () => send({ type: "redo" }) },
      { label: "Set: Save", hint: "⌘S", run: () => send({ type: "save" }) },
      { label: "Set: New", run: () => send({ type: "newSet" }) },
      { label: "Pattern: Generate…", run: () => send({ type: "openGenerative" }) },
      { label: "Pattern: Clear", run: () => send({ type: "clearPattern", args: lane }) },
      { label: "Pattern: Double length", run: () => send({ type: "doubleLength", args: lane }) },
      { label: "Lane: Mute focused", run: () => send({ type: "toggleMute", args: lane }) },
      { label: "Lane: Solo focused", run: () => send({ type: "toggleSolo", args: lane }) },
      { label: "Focus lane: Drums", run: () => send({ type: "focusLane", args: 0 }) },
      { label: "Focus lane: Bass", run: () => send({ type: "focusLane", args: 1 }) },
      { label: "Focus lane: Synth", run: () => send({ type: "focusLane", args: 2 }) },
      { label: "Go to: Perform", run: go("perform") },
      { label: "Go to: Pattern", run: go("pattern") },
      { label: "Go to: Library", run: go("library") },
      { label: "Go to: Song", run: go("song") },
      { label: "Go to: Setup", run: go("setup") },
      { label: "Help", hint: "?", run: () => (helpOpen = true) },
    ];
  }
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
    const meta0 = e.metaKey || e.ctrlKey;
    // Palette works from anywhere (even while typing in a field).
    if (meta0 && e.key.toLowerCase() === "k") {
      e.preventDefault();
      paletteOpen = !paletteOpen;
      return;
    }
    if (isTyping(e) || !app.snap) return;
    if (e.key === "?") {
      e.preventDefault();
      helpOpen = !helpOpen;
      return;
    }
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
{#if paletteOpen}
  <CommandPalette actions={buildActions()} close={() => (paletteOpen = false)} />
{/if}
{#if helpOpen}
  <HelpOverlay close={() => (helpOpen = false)} />
{/if}

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

    {#if app.snap?.recovery_available}
      <div class="recovery" role="alert">
        <span>Unsaved work from a previous session was found.</span>
        <div class="rec-actions">
          <button class="rec-go" onclick={() => send({ type: "recoveryRecover" })}>Recover</button>
          <button onclick={() => send({ type: "recoveryDiscard" })}>Discard</button>
        </div>
      </div>
    {/if}

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
      <div class="tabspacer"></div>
      <button class="util" onclick={() => (paletteOpen = true)} title="Command palette (⌘K)">⌘K</button>
      <button class="util" onclick={() => (helpOpen = true)} title="Help (?)">?</button>
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

  {#if showOnboard}
    <div class="scrim" role="presentation" onclick={dismissOnboard}></div>
    <div class="onboard" role="dialog" aria-label="Welcome">
      <h2>Welcome to midip</h2>
      <p>A focused MIDI groovebox. To get going:</p>
      <ol>
        <li>Press <b>Space</b> to start the transport.</li>
        <li>Click cells in the <b>grid</b> to place steps (drag to paint drums).</li>
        <li>Pick a lane on the left; tweak the selected step on the right.</li>
        <li>Browse the <b>Library</b>, or hit <b>⚡ Generate</b> to create patterns.</li>
        <li><b>⌘K</b> opens the command palette · <b>?</b> opens help.</li>
      </ol>
      <button class="ob-go" onclick={dismissOnboard}>Start</button>
    </div>
  {/if}
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
  .recovery {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 12px;
    background: var(--panel-2);
    border-bottom: 1px solid var(--warn);
    color: var(--warn);
    font-size: 12px;
    flex: 0 0 auto;
  }
  .rec-actions {
    display: flex;
    gap: 6px;
  }
  .rec-go {
    color: var(--bg);
    background: var(--warn);
    border-color: var(--warn);
    font-weight: 700;
  }
  .tabspacer {
    flex: 1;
  }
  .util {
    align-self: center;
    padding: 3px 8px;
    font-size: 11px;
    color: var(--fg-dim);
    margin: 0 2px;
  }
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 90;
  }
  .onboard {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 440px;
    max-width: 92vw;
    background: var(--panel);
    border: var(--border-strong);
    border-left: 3px solid var(--ember);
    border-radius: 6px;
    box-shadow: 0 16px 48px rgba(0, 0, 0, 0.55);
    z-index: 91;
    padding: 20px 22px;
  }
  .onboard h2 {
    margin: 0 0 8px;
    color: var(--ember);
  }
  .onboard p {
    margin: 0 0 10px;
    color: var(--fg-dim);
  }
  .onboard ol {
    margin: 0 0 16px;
    padding-left: 18px;
  }
  .onboard li {
    font-size: 13px;
    line-height: 1.7;
  }
  .onboard b {
    color: var(--fg);
  }
  .ob-go {
    color: var(--bg);
    background: var(--ember);
    border-color: var(--ember);
    font-weight: 700;
    padding: 6px 16px;
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
