<script lang="ts">
  import { app, send } from "../lib/store.svelte";

  const t = $derived(app.snap!.transport);
  let editingBpm = $state(false);
  let bpmInput = $state("");

  function startBpmEdit() {
    bpmInput = String(Math.round(t.set_bpm));
    editingBpm = true;
  }
  function commitBpm() {
    const v = parseFloat(bpmInput);
    if (!Number.isNaN(v) && v > 0) send({ type: "setBpm", args: v });
    editingBpm = false;
  }
  const swingPct = $derived(Math.round(t.swing * 100));

  // Focus the BPM field when it appears (replaces the discouraged `autofocus`).
  function focusOnMount(node: HTMLInputElement) {
    node.focus();
    node.select();
  }
</script>

<header class="transport">
  <button
    class="play"
    class:playing={t.playing}
    class:armed={t.armed}
    onclick={() => send({ type: "togglePlay" })}
    aria-label={t.playing ? "Stop" : "Play"}
  >
    {t.playing ? "■ STOP" : t.armed ? "▶ PLAY…" : "▶ PLAY"}
  </button>

  <div class="group bpm">
    <button class="nudge" onclick={() => send({ type: "adjustBpm", args: -1 })} aria-label="BPM down">−</button>
    {#if editingBpm}
      <input
        class="bpm-input mono"
        bind:value={bpmInput}
        onblur={commitBpm}
        onkeydown={(e) => e.key === "Enter" && commitBpm()}
        use:focusOnMount
      />
    {:else}
      <button class="bpm-val mono" onclick={startBpmEdit} title="Edit BPM">
        {Math.round(t.bpm)}<span class="unit">BPM</span>
      </button>
    {/if}
    <button class="nudge" onclick={() => send({ type: "adjustBpm", args: 1 })} aria-label="BPM up">+</button>
    <button class="tap" onclick={() => send({ type: "tap" })} title="Tap tempo">TAP</button>
  </div>

  <div class="group">
    <span class="lbl">SW</span>
    <button class="nudge" onclick={() => send({ type: "adjustSwing", args: -1 })} aria-label="Swing down">−</button>
    <span class="val mono">{swingPct}%</span>
    <button class="nudge" onclick={() => send({ type: "adjustSwing", args: 1 })} aria-label="Swing up">+</button>
  </div>

  <button
    class="link"
    class:on={t.link_enabled}
    class:locked={t.link_enabled && t.link_peers > 0}
    onclick={() => send({ type: "toggleLink" })}
  >
    LINK {t.link_enabled ? (t.link_peers > 0 ? `${t.link_peers} ●` : "on") : "off"}
  </button>

  {#if t.clock_in}
    <span class="clkin mono" class:locked={t.clock_in.locked}>
      CLK {t.clock_in.locked ? "LOCK" : "…"}
    </span>
  {/if}

  <div class="position mono" aria-label="position">
    {String(t.position.bar).padStart(3, "0")}<span class="dot">.</span>{t.position.beat}<span class="dot">.</span>{t.position.sixteenth}
  </div>

  <div class="spacer"></div>

  <div class="group">
    <button class="hist" onclick={() => send({ type: "undo" })} title="Undo (⌘Z)" aria-label="Undo">↶</button>
    <button class="hist" onclick={() => send({ type: "redo" })} title="Redo (⌘⇧Z)" aria-label="Redo">↷</button>
  </div>

  <button class="mirror" class:on={t.mirror} onclick={() => send({ type: "toggleMirror" })}>MIR</button>

  <span class="state" class:edited={t.dirty} class:saved={!t.dirty}>
    {t.dirty ? "EDITED" : "SAVED"}
  </span>

  <button class="panic" onclick={() => send({ type: "panic" })} aria-label="MIDI panic — all notes off">
    PANIC
  </button>
</header>

<style>
  .transport {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 12px;
    height: 46px;
    background: var(--panel);
    border-bottom: var(--border-strong);
    flex: 0 0 auto;
  }
  .group {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .lbl {
    color: var(--fg-dim);
    font-size: 11px;
  }
  .play {
    font-weight: 700;
    letter-spacing: 0.04em;
    min-width: 92px;
    color: var(--fg-dim);
  }
  .play.playing {
    color: var(--ok);
    border-color: var(--ok);
  }
  .play.armed {
    color: var(--warn);
    border-color: var(--warn);
  }
  .nudge {
    padding: 3px 8px;
    min-width: 26px;
  }
  .tap {
    padding: 3px 8px;
    font-size: 11px;
    color: var(--fg-dim);
  }
  .hist {
    padding: 3px 9px;
    font-size: 14px;
    color: var(--fg-dim);
  }
  .bpm-val {
    min-width: 74px;
    font-size: 16px;
    font-weight: 700;
    color: var(--ember);
    border-color: transparent;
    background: transparent;
  }
  .bpm-val:hover {
    border-color: var(--dim);
  }
  .unit {
    font-size: 10px;
    color: var(--fg-dim);
    margin-left: 3px;
    font-weight: 400;
  }
  .bpm-input {
    width: 66px;
    font-size: 16px;
    font-weight: 700;
    color: var(--ember);
  }
  .val {
    min-width: 40px;
    text-align: center;
  }
  .link.on {
    color: var(--aqua);
    border-color: var(--dim-2);
  }
  .link.locked {
    color: var(--aqua);
    border-color: var(--aqua);
  }
  .clkin {
    color: var(--fg-dim);
    font-size: 11px;
  }
  .clkin.locked {
    color: var(--aqua);
  }
  .position {
    font-size: 18px;
    letter-spacing: 0.05em;
    color: var(--fg);
  }
  .position .dot {
    color: var(--dim);
  }
  .spacer {
    flex: 1;
  }
  .mirror {
    color: var(--fg-dim);
    font-size: 11px;
  }
  .mirror.on {
    color: var(--ok);
    border-color: var(--ok);
  }
  .state {
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.05em;
  }
  .state.saved {
    color: var(--ok);
  }
  .state.edited {
    color: var(--warn);
  }
  .panic {
    color: var(--err);
    border-color: var(--dim-2);
    font-weight: 700;
    letter-spacing: 0.05em;
  }
  .panic:hover {
    border-color: var(--err);
  }
</style>
