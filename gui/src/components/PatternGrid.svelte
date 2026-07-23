<script lang="ts">
  import { app, send } from "../lib/store.svelte";
  import DrumGrid from "./DrumGrid.svelte";
  import MelodicGrid from "./MelodicGrid.svelte";

  const pat = $derived(app.snap!.focused_pattern);
  const lane = $derived(app.snap!.focused_lane);
</script>

<section class="patwrap">
  <div class="toolbar">
    <span class="name" title={pat.name}>{pat.name || "untitled"}</span>
    <span class="kind">{pat.kind}</span>

    <div class="ctl">
      <span class="lbl">len</span>
      <button onclick={() => send({ type: "adjustPatternLen", args: { lane, delta: -1 } })} aria-label="Shorter">−</button>
      <span class="val mono">{pat.length}</span>
      <button onclick={() => send({ type: "adjustPatternLen", args: { lane, delta: 1 } })} aria-label="Longer">+</button>
      <button class="wide" onclick={() => send({ type: "doubleLength", args: lane })}>×2</button>
    </div>

    {#if pat.kind === "melodic"}
      <div class="ctl">
        <span class="lbl">scale</span>
        <button onclick={() => send({ type: "cycleScale", args: { lane, delta: -1 } })}>‹</button>
        <span class="val">{pat.scale}</span>
        <button onclick={() => send({ type: "cycleScale", args: { lane, delta: 1 } })}>›</button>
      </div>
      <div class="ctl">
        <span class="lbl">root</span>
        <button onclick={() => send({ type: "adjustRoot", args: { lane, delta: -1 } })}>−</button>
        <button onclick={() => send({ type: "adjustRoot", args: { lane, delta: 1 } })}>+</button>
      </div>
      <div class="ctl">
        <span class="lbl">oct</span>
        <button onclick={() => send({ type: "adjustOctave", args: { lane, delta: -1 } })}>−</button>
        <span class="val mono">{pat.octave >= 0 ? "+" : ""}{pat.octave}</span>
        <button onclick={() => send({ type: "adjustOctave", args: { lane, delta: 1 } })}>+</button>
      </div>
    {/if}

    <div class="spacer"></div>
    <button class="danger" onclick={() => send({ type: "clearPattern", args: lane })}>clear</button>
  </div>

  <div class="body">
    {#if pat.kind === "drums"}
      <DrumGrid />
    {:else}
      <MelodicGrid />
    {/if}
  </div>
</section>

<style>
  .patwrap {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 10px;
    background: var(--panel);
    border-bottom: var(--border);
    flex: 0 0 auto;
  }
  .name {
    font-weight: 700;
    color: var(--fg);
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .kind {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--fg-dim);
    border: var(--border);
    border-radius: 2px;
    padding: 1px 5px;
  }
  .ctl {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .ctl .lbl {
    font-size: 10px;
    color: var(--fg-dim);
    text-transform: uppercase;
  }
  .ctl button {
    padding: 2px 7px;
    min-width: 24px;
  }
  .ctl .wide {
    min-width: 28px;
  }
  .val {
    min-width: 30px;
    text-align: center;
  }
  .spacer {
    flex: 1;
  }
  .danger {
    color: var(--err);
    font-size: 11px;
  }
  .body {
    flex: 1;
    min-height: 0;
  }
</style>
